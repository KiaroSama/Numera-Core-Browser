"""Check the native graph monitor and incremental cache provenance."""
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import chromium_cache_probe as probe


class CacheProvenance(unittest.TestCase):
    def test_linux_staged_resume_only_allows_runner_image_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cache = root / 'cache'
            cache.mkdir()
            (cache / 'result').write_bytes(b'compiler result')
            archive = root / 'checkpoint.tar'
            seed_context = {'runner_image': 'seed', 'host': 'Linux', 'cache_tool': 'ccache',
                            'cache_config_sha256': hashlib.sha256(probe.STAGED_CACHE_CONFIG.encode('utf-8')).hexdigest(),
                            'clang_sha256': 'seed-clang'}
            probe.pack_cache(cache, archive, seed_context, None)
            digest = probe.sha256(archive)
            images = {}
            resume_context = dict(seed_context, runner_image='resume')
            output = root / 'restored'
            probe.restore_cache(output, archive, digest, resume_context, linux_runner_images=images)
            self.assertEqual(images, {'seed': 'seed', 'resume': 'resume'})
            self.assertEqual((output / 'result').read_bytes(), b'compiler result')
            with self.assertRaisesRegex(RuntimeError, r'clang_sha256, runner_image'):
                probe.restore_cache(root / 'reject', archive, digest, dict(resume_context, clang_sha256='other-clang'),
                                    linux_runner_images={})
            self.assertFalse((root / 'reject').exists())


class NativeGraphOwnership(unittest.TestCase):
    def test_disk_monitor_error_stops_owned_child(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp)
            child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
            try:
                with (patch.object(probe.subprocess, 'Popen', return_value=child),
                      patch.object(probe.shutil, 'disk_usage', side_effect=[
                          SimpleNamespace(free=100_000_000_000), OSError('injected disk monitor failure')]),
                      self.assertRaisesRegex(OSError, 'injected disk monitor failure')):
                    probe.compile_graph(output / 'source', {}, output)
                self.assertIsNotNone(child.poll(), 'the failed graph monitor left its native subprocess running')
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)
