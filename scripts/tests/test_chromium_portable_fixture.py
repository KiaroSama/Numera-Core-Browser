from pathlib import Path
import tempfile
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import chromium_portable_fixture as fixture


class PortableRuntimeFixture(unittest.TestCase):
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
