"""Verify the local transport bounds independently of the native driver."""
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
from pathlib import Path
import sys
from threading import Thread
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from chromedriver_fixture import MAX_RESPONSE, json_command


class FixtureHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({'value': {'ready': True}}).encode('utf-8')
        if self.path == '/oversized':
            body = b' ' * MAX_RESPONSE + body
        self.send_response(302 if self.path == '/redirect' else 200)
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
            finally:
                server.shutdown()
                thread.join(timeout=5)
