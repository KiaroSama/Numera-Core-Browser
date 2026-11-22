"""Check that native verification requires the correct package platform."""
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from chromium_download_smoke import MANIFEST, TARGETS, verify_distribution


class RetainedDownload(unittest.TestCase):
    def test_other_platform_is_rejected_before_running_the_verifier(self):
        with TemporaryDirectory() as directory:
            package = Path(directory) / 'package'
            package.mkdir()
            provenance = Path(directory) / 'provenance'
            provenance.mkdir()
            (provenance / 'bundle.jsonl').touch()
            revision = 'a' * 40
            for local, remote in [('Linux', 'Windows'), ('Windows', 'Linux')]:
                with self.subTest(host=local):
                    manifest = {'target': TARGETS[remote], 'numera_commit': revision,
                                'workflow_run': 1, 'trusted_release': False, 'build_mode': 'development'}
                    (package / MANIFEST).write_text(json.dumps(manifest), encoding='utf-8')
                    with patch('chromium_download_smoke.subprocess.run') as verify:
                        with self.assertRaisesRegex(RuntimeError, 'selected development run'):
                            verify_distribution(package, provenance, revision, 1, local)
                        verify.assert_not_called()
