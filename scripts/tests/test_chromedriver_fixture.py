"""Verify the local transport bounds independently of the native driver."""
from contextlib import nullcontext
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
from pathlib import Path
import subprocess
import sys
from threading import Thread
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from chromedriver_fixture import MAX_RESPONSE, json_command, smoke_driver, verify_native_window


class FixtureHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({'value': {'ready': True}}).encode('utf-8')
        status = 302 if self.path == '/redirect' else 200
        if self.path == '/oversized':
            body = b' ' * MAX_RESPONSE + body
        if self.path == '/session-failure':
            status = 500
            body = json.dumps({'value': {'error': 'session not created',
                                         'message': 'synthetic-secret-for-untrusted-body'}}).encode('utf-8')
        self.send_response(status)
        if self.path == '/redirect':
            self.send_header('Location', '/status')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


class DriverFixtureTransport(unittest.TestCase):
    def test_local_w3c_response_is_bounded_and_redirects_do_not_run(self):
        with HTTPServer(('127.0.0.1', 0), FixtureHandler) as server:
            thread = Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                port = server.server_port
                self.assertEqual(json_command(port, 'GET', '/status'), {'ready': True})
                for path in ('/oversized', '/redirect'):
                    with self.subTest(path=path), self.assertRaisesRegex(RuntimeError, 'controlled command'):
                        json_command(port, 'GET', path)
                with self.assertRaisesRegex(RuntimeError, 'HTTP 500, session not created') as failure:
                    json_command(port, 'GET', '/session-failure')
                self.assertNotIn('synthetic-secret', str(failure.exception))
            finally:
                server.shutdown()
                thread.join(timeout=5)

    def test_headed_probe_runs_in_the_validated_browser_session(self):
        requests = []
        browser_pid = 1234
        session_id = 'a' * 32

        def respond(port, method, path, data=None, timeout=15):
            requests.append((method, path, data))
            if path == '/session':
                return {'sessionId': session_id, 'capabilities': {'goog:processID': browser_pid}}
            if path.endswith('/execute/sync'):
                return 'NUMERA_RENDERER_FIXTURE_PASS'
            return None

        with (patch('chromedriver_fixture.local_driver', return_value=nullcontext((123, '157.0.8103.78'))),
              patch('chromedriver_fixture.json_command', side_effect=respond),
              patch('chromedriver_fixture.verify_native_window', return_value='test-window') as verify):
            report = smoke_driver(Path('/browser'), Path('/driver'), Path('profile'), Path('evidence'),
                                  {'DISPLAY': ':1'}, None, headed=True)
        options = requests[0][2]['capabilities']['alwaysMatch']['goog:chromeOptions']
        self.assertEqual(options['binary'], str(Path('/browser')))
        self.assertFalse(any(argument.startswith('--headless') for argument in options['args']))
        verify.assert_called_once_with(browser_pid, {'DISPLAY': ':1'})
        self.assertEqual(report['native_window'], 'test-window')
        self.assertEqual(requests[-1][:2], ('DELETE', f'/session/{session_id}'))

    def test_linux_native_window_requires_a_viewable_browser_window(self):
        tree = '   0xab "Numera source build fixture - Chromium": ("chromium" "Chromium")'
        owner = subprocess.CompletedProcess([], 0, '_NET_WM_PID(CARDINAL) = 1234\n')
        unmapped = subprocess.CompletedProcess([], 0, '  Map State: IsUnMapped\n')
        mapped = subprocess.CompletedProcess([], 0, '  Map State: IsViewable\n')
        with (patch('chromedriver_fixture.platform.system', return_value='Linux'),
              patch('chromedriver_fixture.subprocess.check_output', return_value=tree),
              patch('chromedriver_fixture.subprocess.run', side_effect=[owner, unmapped, owner, mapped]) as query,
              patch('chromedriver_fixture.time.monotonic', side_effect=[0, 0, 5, 13]),
              patch('chromedriver_fixture.time.sleep')):
            self.assertEqual(verify_native_window(1234, {'DISPLAY': ':1'}), 'x11-viewable-window-with-browser-pid')
        self.assertEqual(query.call_count, 4)
