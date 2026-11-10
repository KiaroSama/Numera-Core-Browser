import importlib.util
from pathlib import Path
import sys
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location('engine_build', SCRIPTS / 'engine_build.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class RendererEvidence(unittest.TestCase):
    def test_script_literal_is_not_proof_of_renderer_execution(self):
        html = '<strong id="result">Waiting</strong><script>const text="NUMERA_RENDERER_FIXTURE_PASS"</script>'
        self.assertFalse(module.renderer_passed(html))

    def test_only_the_executed_dom_marker_passes(self):
        self.assertTrue(module.renderer_passed('<strong id="result">NUMERA_RENDERER_FIXTURE_PASS</strong>'))


if __name__ == '__main__':
    unittest.main()
