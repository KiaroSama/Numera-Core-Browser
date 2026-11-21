"""Check the native graph monitor and incremental cache provenance."""
import hashlib
import json
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
from chromium_staged_build import reviewed_checkpoint, restore_reviewed_context


class CacheProvenance(unittest.TestCase):
    def test_reviewed_checkpoint_rebinds_producer_identity_without_replacing_build_inputs(self):
        evidence = Path(__file__).resolve().parents[2] / 'docs/evidence'
        checkpoint = reviewed_checkpoint(evidence, '44846110371', '2', 'Linux')
        source = checkpoint['context']
        self.assertEqual(checkpoint['key'], 'chromium-staged-44846110371-1-2-ubuntu-24.04')
        consumer = dict(source, head='5' * 40, run_id='44999999999', run_attempt='1', commands_sha256='a' * 64)
        expected = restore_reviewed_context(consumer, checkpoint)
        self.assertEqual(expected['head'], source['head'])
        self.assertEqual(expected['run_id'], source['run_id'])
        self.assertEqual(expected['commands_sha256'], consumer['commands_sha256'])
        self.assertEqual(consumer['run_id'], '44999999999')
        for run, segment, host in [('44846110371', '2', 'Windows'), ('44846110371', '4', 'Linux'),
                                   ('../44846110371', '2', 'Linux')]:
            with self.subTest(run=run, segment=segment, host=host), self.assertRaises(RuntimeError):
                reviewed_checkpoint(evidence, run, segment, host)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'chromium-staged-44846110371-segment-2.json'
            data = json.loads((evidence / path.name).read_text(encoding='utf-8'))
            data['cache']['exact_key'] = 'a-different-checkpoint'
            path.write_text(json.dumps(data), encoding='utf-8')
            with self.assertRaisesRegex(RuntimeError, 'cache key'):
                reviewed_checkpoint(path.parent, '44846110371', '2', 'Linux')

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
            inventory = {}
            probe.restore_cache(output, archive, digest, resume_context, linux_runner_images=images,
                                restored_entries=inventory)
            self.assertEqual(images, {'seed': 'seed', 'resume': 'resume'})
            self.assertEqual((output / 'result').read_bytes(), b'compiler result')
            self.assertEqual(inventory, {'result': probe.sha256(cache / 'result')})
            (output / 'result').write_bytes(b'changed contents')
            with self.assertRaisesRegex(RuntimeError, r'evicted or changed'):
                probe.pack_cache(output, root / 'next-checkpoint.tar', resume_context, None, required_entries=inventory)
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
