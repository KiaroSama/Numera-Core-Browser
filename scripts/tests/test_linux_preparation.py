import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

WORKFLOW = Path(__file__).resolve().parents[2] / '.github/workflows/chromium-source.yml'
LINES = WORKFLOW.read_text(encoding='utf-8').splitlines()
START = next(i + 1 for i, line in enumerate(LINES) if "python3 - <<'PY'" in line)
END = next(i for i in range(START, len(LINES)) if LINES[i].strip() == 'PY')
PREPARATION = compile('\n'.join(line[10:] for line in LINES[START:END]), str(WORKFLOW), 'exec')


class LinuxPreparationGuards(unittest.TestCase):
    def check(self, free, private=False, repository='KiaroSama/Numera-Core-Browser', blocked=False):
        before = Path.cwd()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event = root / 'event.json'
            event.write_text(json.dumps({'repository': {'private': private}}), encoding='utf-8')
            environment = {'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted',
                'GITHUB_REPOSITORY': repository, 'GITHUB_RUN_ID': 'owned-test', 'GITHUB_SHA': 'fixture',
                'GITHUB_EVENT_PATH': str(event), 'ANDROID_HOME': 'different-sdk-path'}
            try:
                os.chdir(root)
                with patch.dict(os.environ, environment), patch('shutil.disk_usage', return_value=SimpleNamespace(free=free)), \
                        patch('subprocess.run', side_effect=AssertionError('No native operation is allowed in this fixture')) as native, \
                        contextlib.redirect_stdout(io.StringIO()):
                    if blocked:
                        with self.assertRaises(RuntimeError):
                            exec(PREPARATION, {})
                    else:
                        exec(PREPARATION, {})
                    native.assert_not_called()
                report = json.loads((root / 'build/engine-evidence/linux-preparation.json').read_text(encoding='utf-8'))
                self.assertEqual(report['result'], 'BLOCKED' if blocked else 'PASS')
                self.assertIsNone(report['removed_path'])
                self.assertEqual(report['engine_acceptance'], 'NOT_RUN')
            finally:
                os.chdir(before)

    def test_enough_space_never_removes_an_optional_tool(self):
        self.check(120_000_000_000)

    def test_private_repository_cannot_prepare_a_runner(self):
        self.check(91_000_000_000, private=True, blocked=True)

    def test_other_repository_cannot_prepare_a_runner(self):
        self.check(91_000_000_000, repository='other/repository', blocked=True)

    def test_changed_sdk_identity_never_reaches_a_native_command(self):
        self.check(91_000_000_000, blocked=True)


if __name__ == '__main__':
    unittest.main()
