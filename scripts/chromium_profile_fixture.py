"""Check browser storage, a worker, and a download through native WebDriver."""
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import re
from threading import Thread
import time

from chromedriver_fixture import json_command, local_driver, local_session

MARKER = 'Numera Persian داده‌های مرورگر'
DOWNLOAD = b'Numera controlled browser download\x00\xff\n'
PAGE = b'''<!doctype html><meta charset="utf-8"><title>Numera profile fixture</title>
<style>#sample { color: rgb(23, 45, 67) }</style>
<p id="sample">Profile storage fixture</p><a id="download" href="/data">Download</a>'''
WORKER = b"self.onmessage = event => self.postMessage('worker:' + event.data);"


class ProfileFixtureRequest(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.headers.get('Host') != f'127.0.0.1:{self.server.server_port}':
            self.send_error(400)
            return
        routes = {'/': ('text/html; charset=utf-8', PAGE),
                  '/worker.js': ('text/javascript; charset=utf-8', WORKER),
                  '/data': ('application/octet-stream', DOWNLOAD)}
        if self.path not in routes:
            self.send_error(404)
            return
        media_type, body = routes[self.path]
        self.send_response(200)
        self.send_header('Content-Type', media_type)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Cache-Control', 'no-store')
        if self.path == '/data':
            self.send_header('Content-Disposition', 'attachment; filename="numera-download.bin"')
        self.end_headers()
        self.wfile.write(body)


@contextmanager
def local_profile_page():
    with ThreadingHTTPServer(('127.0.0.1', 0), ProfileFixtureRequest) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f'http://127.0.0.1:{server.server_port}/'
        finally:
            server.shutdown()
            thread.join(timeout=5)


def read_profile_marker(port, session, page):
    json_command(port, 'POST', session + '/url', {'url': page})
    return json_command(port, 'POST', session + '/execute/sync', {
        'script': "return localStorage.getItem('numera-fixture');", 'args': []})


def check_download(port, session, folder):
    element = json_command(port, 'POST', session + '/element', {'using': 'css selector', 'value': '#download'})
    element_id = element.get('element-6066-11e4-a52e-4f735466cecf') if isinstance(element, dict) else None
    if not isinstance(element_id, str) or not re.fullmatch(r'[A-Za-z0-9.\-]{1,160}', element_id):
        raise RuntimeError('Native WebDriver did not find the fixture download link')
    json_command(port, 'POST', f'{session}/element/{element_id}/click', {})
    destination = folder / 'numera-download.bin'
    deadline = time.monotonic() + 10
    while not destination.exists() and time.monotonic() < deadline:
        time.sleep(0.1)
    if not destination.is_file() or destination.stat().st_size != len(DOWNLOAD) or destination.read_bytes() != DOWNLOAD:
        raise RuntimeError('The controlled browser download did not produce the exact fixture bytes')


def smoke_profile_data(binary, driver, scratch, output, environment, stop_tree):
    profile = scratch / 'Numera profile داده‌ها'
    independent_profile = scratch / 'Independent profile داده‌ها'
    download_folder = scratch / 'Numera downloads داده‌ها'
    download_folder.mkdir()
    preferences = {'download.default_directory': str(download_folder), 'download.prompt_for_download': False}
    with (local_profile_page() as page,
          local_driver(driver, output / 'profile-data-driver.log', environment, stop_tree) as (port, _)):
        with local_session(port, binary, profile, preferences=preferences) as (session, _):
            if read_profile_marker(port, session, page) is not None:
                raise RuntimeError('The new profile already contained the fixture marker')
            actual = json_command(port, 'POST', session + '/execute/sync', {'args': [MARKER], 'script': """
                localStorage.setItem('numera-fixture', arguments[0]);
                return getComputedStyle(document.getElementById('sample')).color;
            """})
            if actual != 'rgb(23, 45, 67)':
                raise RuntimeError('The browser did not apply the fixture style')
            result = json_command(port, 'POST', session + '/execute/async', {'args': [MARKER], 'script': """
                const done = arguments[arguments.length - 1];
                const worker = new Worker('/worker.js');
                worker.onerror = () => { worker.terminate(); done(null); };
                worker.onmessage = event => { worker.terminate(); done(event.data); };
                worker.postMessage(arguments[0]);
            """})
            if result != 'worker:' + MARKER:
                raise RuntimeError('The dedicated worker did not return the exact Unicode marker')
            check_download(port, session, download_folder)
        with local_session(port, binary, profile) as (session, _):
            if read_profile_marker(port, session, page) != MARKER:
                raise RuntimeError('The browser profile lost its marker after a new native session')
        with local_session(port, binary, independent_profile) as (session, _):
            if read_profile_marker(port, session, page) is not None:
                raise RuntimeError('The browser shared the marker with an independent profile')
    return {'result': 'PASS', 'computed_style': 'PASS', 'dedicated_worker': 'PASS',
            'download_bytes': len(DOWNLOAD), 'profile_restart': 'PASS', 'profile_isolation': 'PASS',
            'profile_and_download_paths': 'Unicode with spaces'}
