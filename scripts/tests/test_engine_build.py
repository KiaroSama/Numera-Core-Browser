import importlib.util
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

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


class TelemetryIsolation(unittest.TestCase):
    def test_updates_only_own_job_output_and_leaves_conclusion_to_actions(self):
        calls = []

        def request(path, body=None):
            calls.append((path, body))
            if body is None:
                return {'jobs': [
                    {'name': 'native (ubuntu-24.04)', 'status': 'in_progress', 'check_run_url': 'https://api.github.com/check-runs/11'},
                    {'name': 'source', 'status': 'completed', 'check_run_url': 'https://api.github.com/check-runs/12'},
                    {'name': 'source', 'status': 'in_progress', 'check_run_url': 'https://api.github.com/check-runs/13'},
                ]}
            return {}

        with patch.dict(module.os.environ, {'GITHUB_REPOSITORY': 'owner/repo', 'GITHUB_RUN_ID': '7', 'GITHUB_JOB': 'source'}):
            check = module.CheckRun()
            with patch.object(check, 'request', side_effect=request):
                for result in ['RUNNING', 'BLOCKED', 'PASS']:
                    check.update({'stage': 'compile-link', 'elapsed_seconds': 10,
                        'disk_free_bytes': 200000000000, 'peak_tree_rss_bytes': 1000, 'result': result})
        self.assertEqual(len(calls), 4)
        self.assertIn('/actions/runs/7/jobs?', calls[0][0])
        for path, body in calls[1:]:
            self.assertEqual(path, '/repos/owner/repo/check-runs/13')
            self.assertEqual(set(body), {'output'})

    def test_ambiguous_job_does_not_mutate_any_check(self):
        with patch.dict(module.os.environ, {'GITHUB_REPOSITORY': 'owner/repo', 'GITHUB_RUN_ID': '7', 'GITHUB_JOB': 'source'}):
            check = module.CheckRun()
            with patch.object(check, 'request', return_value={'jobs': []}) as request:
                with self.assertRaises(RuntimeError):
                    check.update({})
                self.assertEqual(request.call_count, 1)


if __name__ == '__main__':
    unittest.main()
