import importlib.util
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

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


class BuildProgress(unittest.TestCase):
    def test_extracts_last_valid_action_counter_without_decoding_binary_noise(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'native.log'
            path.write_bytes(b'\xff\r[12/300] CXX a.cc\r[13/300] LINK a.dll\n[999/2] invalid')
            self.assertEqual(module.read_progress(path), {'completed': 13, 'total': 300})

    def test_absent_counter_does_not_invent_a_percentage(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'native.log'
            path.write_bytes(b'source acquisition still running')
            self.assertIsNone(module.read_progress(path))

    def test_compile_timeout_records_attempted_gate_and_preserves_passed_gate(self):
        report = {'result': 'RUNNING', 'source_build': 'NOT_RUN', 'renderer_fixture': 'NOT_RUN'}
        module.record_stage(report, 'compile-link')
        self.assertEqual(report['source_build'], 'RUNNING')
        report['result'] = 'BLOCKED'
        module.record_stage(report, 'compile-link')
        self.assertEqual(report['source_build'], 'BLOCKED')
        self.assertEqual(report['renderer_fixture'], 'NOT_RUN')
        report.update(result='RUNNING', source_build='PASS')
        module.record_stage(report, 'controlled-renderer-fixture')
        report['result'] = 'BLOCKED'
        module.record_stage(report, 'controlled-renderer-fixture')
        self.assertEqual(report['source_build'], 'PASS')
        self.assertEqual(report['renderer_fixture'], 'BLOCKED')


class TelemetryIsolation(unittest.TestCase):
    def test_api_outage_keeps_source_gate_running_and_logs_a_valid_warning(self):
        report = {'result': 'RUNNING', 'source_build': 'RUNNING', 'release_ready': False}
        check = Mock()
        check.update.side_effect = TimeoutError('synthetic-unlogged-value')
        captured = io.StringIO()
        with contextlib.redirect_stderr(captured):
            self.assertFalse(module.publish_check(report, check))
        self.assertEqual(report['result'], 'RUNNING')
        self.assertEqual(report['source_build'], 'RUNNING')
        self.assertFalse(report['release_ready'])
        self.assertEqual(report['telemetry_publish_error_type'], 'TimeoutError')
        self.assertEqual(json.loads(captured.getvalue())['severity'], 'WARNING')
        self.assertNotIn('synthetic-unlogged-value', captured.getvalue())

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
