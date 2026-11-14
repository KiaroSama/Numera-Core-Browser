import importlib.util
import json
from pathlib import Path
import plistlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location('macos_preparation', Path(__file__).parents[1] / 'prepare_macos_arm.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class MacPreparation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        # macOS temporary roots may use /var -> /private/var. Fixtures need
        # canonical operands just like the reviewed production SDK paths.
        self.root = Path(self.temp.name).resolve()
        self.apps = self.root / 'Applications'
        self.sdk = self.root / 'Android/sdk'
        for name in ['cmdline-tools', 'licenses']:
            (self.sdk / name).mkdir(parents=True)
        self.protected = self.apps / 'Xcode_26.3.app'
        for name, version in module.UNUSED_XCODES.items():
            self.write_info(self.apps / name, version)
        self.write_info(self.protected, '26.3')
        event = self.root / 'event.json'
        event.write_text(json.dumps({'repository': {'private': False}}), encoding='utf-8')
        self.env = {'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted',
            'GITHUB_REPOSITORY': 'KiaroSama/Numera-Core-Browser', 'RUNNER_ARCH': 'ARM64',
            'GITHUB_REF': 'refs/heads/main', 'GITHUB_EVENT_NAME': 'push',
            'GITHUB_EVENT_PATH': str(event), 'ImageVersion': '20261104.0514.1',
            'ANDROID_HOME': str(self.sdk)}
        self.free = 49_000_000_000
        self.free_after = 103_000_000_000
        self.reported_sdk = '26.2'
        self.commands = []

    def write_info(self, app, version, identifier='com.apple.dt.Xcode'):
        path = app / 'Contents/Info.plist'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(plistlib.dumps({'CFBundleIdentifier': identifier,
            'CFBundleShortVersionString': version}))

    def native(self, arguments, **_):
        self.commands.append(arguments)
        if '/bin/rm' in arguments:
            position = arguments.index('/bin/rm')
            self.assertEqual(arguments[position + 1:position + 4], ['-rf', '-x', '--'])
            for name in arguments[position + 4:]:
                self.assertNotEqual(Path(name), self.protected)
                shutil.rmtree(name)
            self.free = self.free_after
            return SimpleNamespace(stdout='', returncode=0)
        if arguments[0] == 'xcodebuild':
            return SimpleNamespace(stdout='Xcode 26.3\nBuild version 17C529\n', returncode=0)
        if arguments[0] == 'xcrun':
            return SimpleNamespace(stdout=self.reported_sdk + '\n', returncode=0)
        raise AssertionError('Unexpected native command')

    def run_preparation(self):
        with patch.object(module, 'APPLICATIONS', self.apps), patch.object(module, 'ANDROID_SDK', self.sdk), \
                patch.object(module.platform, 'system', return_value='Darwin'), \
                patch.object(module.shutil, 'disk_usage', side_effect=lambda _: SimpleNamespace(free=self.free)), \
                patch.object(module.subprocess, 'run', side_effect=self.native):
            result = module.prepare(self.root / 'evidence', self.env)
        retained = json.loads((self.root / 'evidence/mac-arm-preparation.json').read_text(encoding='utf-8'))
        self.assertEqual(result, retained)
        self.assertEqual(retained['source_build'], 'NOT_RUN')
        self.assertFalse(retained['release_ready'])
        return result

    def assert_rejected_before_native(self):
        self.assertEqual(self.run_preparation()['result'], 'BLOCKED')
        self.assertEqual(self.commands, [])
        self.assertTrue(self.protected.is_dir())
        self.assertTrue(self.sdk.is_dir())

    def test_private_repository_is_rejected_before_native_commands(self):
        Path(self.env['GITHUB_EVENT_PATH']).write_text('{"repository":{"private":true}}', encoding='utf-8')
        self.assert_rejected_before_native()

    def test_other_repository_is_rejected_before_native_commands(self):
        self.env['GITHUB_REPOSITORY'] = 'someone/else'
        self.assert_rejected_before_native()

    def test_other_architecture_is_rejected_before_native_commands(self):
        self.env['RUNNER_ARCH'] = 'X64'
        self.assert_rejected_before_native()

    def test_other_branch_is_rejected_before_native_commands(self):
        self.env['GITHUB_REF'] = 'refs/heads/unreviewed'
        self.assert_rejected_before_native()

    def test_changed_runner_image_is_rejected_before_native_commands(self):
        self.env['ImageVersion'] = 'unreviewed'
        self.assert_rejected_before_native()

    def test_symlink_replacement_is_rejected_before_native_commands(self):
        target = self.apps / 'Xcode_16.1.app'
        shutil.rmtree(target)
        target.symlink_to(self.protected, target_is_directory=True)
        self.assert_rejected_before_native()

    def test_unexpected_bundle_is_rejected_before_native_commands(self):
        self.write_info(self.apps / 'Xcode_16.2.app', '16.2', 'example.unrelated')
        self.assert_rejected_before_native()

    def test_enough_capacity_skips_every_native_operation(self):
        self.free = 105_000_000_000
        result = self.run_preparation()
        self.assertEqual(result['result'], 'PASS', result)
        self.assertEqual(result['removed_paths'], [])
        self.assertEqual(self.commands, [])

    def test_changed_sdk_probe_blocks_removal(self):
        self.reported_sdk = '26.5'
        result = self.run_preparation()
        self.assertEqual(result['result'], 'BLOCKED')
        self.assertEqual(len(self.commands), 2)
        self.assertTrue(all('/bin/rm' not in command for command in self.commands))
        self.assertTrue(self.sdk.is_dir())

    def test_actual_owned_fixture_cleanup_preserves_selected_xcode(self):
        result = self.run_preparation()
        self.assertEqual(result['result'], 'PASS', result)
        self.assertEqual(len(result['removed_paths']), 9)
        self.assertEqual(result['free_after_bytes'], self.free_after)
        self.assertTrue(self.protected.is_dir())
        self.assertFalse(self.sdk.exists())

    def test_insufficient_reclaimed_space_remains_blocked_and_retains_evidence(self):
        self.free_after = 99_000_000_000
        result = self.run_preparation()
        self.assertEqual(result['result'], 'BLOCKED')
        self.assertEqual(len(result['removed_paths']), 9)
        self.assertEqual(result['free_after_bytes'], self.free_after)
        self.assertTrue(self.protected.is_dir())

    @unittest.skipUnless(hasattr(signal, 'SIGALRM'), 'Unix kernel alarm fixture')
    def test_kernel_deadline_survives_exec_into_an_actual_owned_child(self):
        started = time.monotonic()
        result = subprocess.run([sys.executable, '-I', '-S', '-c', module.ALARM_EXEC, str(started + 1),
            sys.executable, '-I', '-S', '-c', 'import time; time.sleep(30)'],
            capture_output=True, timeout=6)
        self.assertEqual(result.returncode, -signal.SIGALRM)
        self.assertLess(time.monotonic() - started, 5)

    @unittest.skipUnless(hasattr(signal, 'SIGALRM'), 'Unix kernel alarm fixture')
    def test_expired_deadline_refuses_exec_even_if_privileged_start_was_delayed(self):
        canary = self.root / 'should-not-exist'
        result = subprocess.run([sys.executable, '-I', '-S', '-c', module.ALARM_EXEC,
            str(time.monotonic() - 1), sys.executable, '-I', '-S', '-c',
            'from pathlib import Path; import sys; Path(sys.argv[1]).touch()', str(canary)],
            capture_output=True, timeout=6)
        self.assertEqual(result.returncode, 124)
        self.assertFalse(canary.exists())


if __name__ == '__main__':
    unittest.main()
