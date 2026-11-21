import json
from pathlib import Path
import tempfile
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import chromium_portable_fixture as fixture


class PortableRuntimeFixture(unittest.TestCase):
    def test_linux_uses_linked_driver_dependencies_and_the_stripped_executable(self):
        with tempfile.TemporaryDirectory() as tmp:
            scratch = Path(tmp).resolve()
            source = scratch / 'src'
            build = source / 'out/Numera'
            build.mkdir(parents=True)
            output = scratch / 'evidence'
            output.mkdir()
            gn = source / 'buildtools/linux64/gn'
            gn.parent.mkdir(parents=True)
            gn.write_bytes(b'controlled GN fixture')
            names = {'chrome', 'chromedriver', 'chrome_sandbox', 'libdriver-component.so'}
            for name in names:
                (build / name).write_bytes(name.encode('utf-8'))

            def describe(command, **kwargs):
                self.assertEqual(command[1:3], ['desc', 'out/Numera'])
                self.assertEqual(kwargs['cwd'], source)
                if command[3] == '//chrome:chrome_initial':
                    return b'./chrome\n'
                if command[3] == '//chrome/test/chromedriver:chromedriver_server.unstripped':
                    return b'./chromedriver.unstripped\n./libdriver-component.so\n'
                return b''  # The stripped executable's action has no runtime_deps.

            runtime = scratch / 'Numera extracted مسیر' / 'runtime'
            original_environment = {'CHROME_DEVEL_SANDBOX': '/original/helper'}
            helper = scratch / 'portable-sandbox' / 'numera-source-sandbox'

            def install(staged, destination, environment):
                self.assertEqual(staged, runtime)
                self.assertEqual(destination, helper.parent)
                self.assertTrue(destination.is_dir())
                self.assertEqual((staged / 'chrome_sandbox').read_bytes(), b'chrome_sandbox')
                environment['CHROME_DEVEL_SANDBOX'] = str(helper)
                return {'mode': '4755'}

            def render(browser, profile, evidence, environment):
                self.assertFalse(build.exists())
                self.assertEqual(browser, runtime / 'chrome')
                self.assertEqual(environment['CHROME_DEVEL_SANDBOX'], str(helper))

            with (patch.object(fixture.subprocess, 'check_output', side_effect=describe),
                  patch.object(fixture, 'install_linux_sandbox', side_effect=install) as installed,
                  patch.object(fixture, 'smoke', side_effect=render),
                  patch.object(fixture, 'smoke_driver', return_value={'result': 'PASS'}) as webdriver):
                report = fixture.smoke_portable_runtime(source, scratch, output, 'Linux', original_environment)
            installed.assert_called_once()
            self.assertEqual(webdriver.call_args.args[1], runtime / 'chromedriver')
            self.assertEqual(original_environment['CHROME_DEVEL_SANDBOX'], '/original/helper')
            self.assertEqual(report['sandbox_helper']['mode'], '4755')
            package = scratch / 'numera-portable-runtime.tar.gz'
            self.assertEqual(report['archive']['sha256'], fixture.sha256(package))
            self.assertEqual(report['archive']['files'], len(names))
            self.assertEqual({path.name for path in runtime.iterdir()}, names)
            inventory = json.loads((output / 'portable-runtime-inventory.json').read_text(encoding='utf-8'))
            self.assertEqual(set(inventory['files']), names)

    def test_source_output_is_restored_when_relocated_smoke_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            scratch = Path(tmp).resolve()
            source = scratch / 'src'
            build = source / 'out/Numera'
            build.mkdir(parents=True)
            output = scratch / 'evidence'
            output.mkdir()
            gn = source / 'buildtools/win/gn.exe'
            gn.parent.mkdir(parents=True)
            gn.write_bytes(b'controlled GN fixture')
            for name in ('chrome.exe', 'chromedriver.exe', 'component.dll'):
                (build / name).write_bytes(name.encode('utf-8'))

            def failure(binary, profile, evidence, environment):
                self.assertFalse(build.exists())
                self.assertEqual(binary.read_bytes(), b'chrome.exe')
                self.assertEqual((binary.parent / 'component.dll').read_bytes(), b'component.dll')
                self.assertIn('مسیر', str(binary))
                self.assertTrue(profile.is_dir())
                raise RuntimeError('Controlled renderer failure')

            inventory = b'./initialexe/chrome.exe\n./component.dll\n'
            with (patch.object(fixture.subprocess, 'check_output', return_value=inventory),
                  patch.object(fixture, 'smoke', side_effect=failure),
                  self.assertRaisesRegex(RuntimeError, 'Controlled renderer failure')):
                fixture.smoke_portable_runtime(source, scratch, output, 'Windows', {})
            self.assertEqual((build / 'chrome.exe').read_bytes(), b'chrome.exe')
            self.assertFalse((build.parent / 'Numera-fixture-held').exists())

    def test_runtime_inventory_rejects_outside_and_ambiguous_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            build = Path(tmp).resolve()
            (build / 'file.dat').write_bytes(b'fixture')
            for dependency in ('../file.dat', '/file.dat', 'C:file.dat', 'sub\\file.dat'):
                with self.subTest(dependency=dependency), self.assertRaisesRegex(RuntimeError, 'outside'):
                    fixture.resolve_runtime_files(build, [dependency])
            self.assertEqual(fixture.resolve_runtime_files(build, ['./file.dat', 'file.dat']), ['file.dat'])

    def test_runtime_inventory_rejects_case_conflicts_in_directory_components(self):
        with tempfile.TemporaryDirectory() as tmp:
            build = Path(tmp).resolve()
            (build / 'Locales').mkdir()
            if (build / 'locales').exists():
                self.skipTest('The test filesystem already rejects distinct directory spellings')
            (build / 'locales').mkdir()
            (build / 'Locales/a.pak').write_bytes(b'a')
            (build / 'locales/b.pak').write_bytes(b'b')
            with self.assertRaisesRegex(RuntimeError, 'names conflict'):
                fixture.resolve_runtime_files(build, ['Locales', 'locales'])

    def test_archive_rejects_same_size_changes_to_staged_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            staged = root / 'runtime'
            staged.mkdir()
            asset = staged / 'asset.dat'
            asset.write_bytes(b'pass')
            inventory = {'files': {'asset.dat': {'bytes': 4, 'sha256': fixture.sha256(asset)}}, 'bytes': 4}
            asset.write_bytes(b'fail')
            with self.assertRaisesRegex(RuntimeError, 'not byte-identical'):
                fixture.extract_portable_archive(staged, root, inventory)
