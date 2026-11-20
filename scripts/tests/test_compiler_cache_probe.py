"""Process ownership regression for the native graph monitor."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location('compiler_cache_probe', SCRIPTS / 'chromium-compiler-cache-probe.py')
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)


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
