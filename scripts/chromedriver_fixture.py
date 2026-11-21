#!/usr/bin/env python3
"""Exercise an owned ChromeDriver and Chromium pair on a local fixture."""
from contextlib import contextmanager, suppress
import http.client
import json
from pathlib import Path
import re
import subprocess
import time

MAX_RESPONSE = 256 * 1024
STARTED = re.compile(rb'(?m)^ChromeDriver was started successfully on port ([1-9][0-9]{0,4})\.\r?$')


def json_command(port, method, path, data=None, timeout=15):
    body = json.dumps(data).encode('utf-8') if data is not None else None
    headers = {'Content-Type': 'application/json; charset=utf-8'} if data is not None else {}
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=timeout)
    try:
        connection.request(method, path, body=body, headers=headers)
        response = connection.getresponse()
        raw = response.read(MAX_RESPONSE + 1)
        if response.status != 200 or len(raw) > MAX_RESPONSE:
            raise RuntimeError('Native ChromeDriver rejected the controlled command')
        value = json.loads(raw)
        if not isinstance(value, dict) or 'value' not in value:
            raise RuntimeError('Native ChromeDriver returned an invalid W3C response')
        return value['value']
    finally:
        connection.close()


def driver_port(process, log_path, timeout=25):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError('Native ChromeDriver ended before exposing its loopback port')
        with log_path.open('rb') as log:
            log.seek(0, 2)
            log.seek(max(0, log.tell() - 16_384))
            banner = STARTED.search(log.read(16_384))
        if banner:
            port = int(banner[1])
            if 0 < port <= 65535:
                return port
        time.sleep(0.1)
    raise RuntimeError('Native ChromeDriver did not expose its loopback port in time')


@contextmanager
def local_driver(driver, logfile, environment, stop_tree):
    version = subprocess.check_output([str(driver), '--version'], cwd=driver.parent, env=environment,
                                      timeout=10, stderr=subprocess.STDOUT).decode('utf-8').strip()
    if not re.match(r'^ChromeDriver [0-9]+\.[0-9]+\.[0-9]+\.[0-9]+\b', version):
        raise RuntimeError('Unrecognized native ChromeDriver version')
    with logfile.open('wb') as output_log:
        # The driver chooses the port atomically and keeps its default local-only binding.
        process = subprocess.Popen([str(driver), '--port=0'], cwd=driver.parent, env=environment,
                                   stdout=output_log, stderr=subprocess.STDOUT)
        try:
            port = driver_port(process, logfile)
            status = json_command(port, 'GET', '/status')
            if not isinstance(status, dict) or status.get('ready') is not True:
                raise RuntimeError('Native ChromeDriver did not report readiness')
            yield port, version.split()[1]
        finally:
            if process.poll() is None:
                stop_tree(process)
            process.wait(timeout=5)


def smoke_driver(binary, driver, scratch, output, environment, stop_tree):
    session = None
    with local_driver(driver, output / 'chromedriver-fixture.log', environment, stop_tree) as (port, version):
        try:
            profile = scratch / 'driver-fixture-profile'
            session_data = json_command(port, 'POST', '/session', {'capabilities': {'alwaysMatch': {
                'browserName': 'chrome', 'goog:chromeOptions': {'binary': str(binary), 'args': [
                    '--headless=new', '--no-first-run', '--no-default-browser-check', '--user-data-dir=' + str(profile)
                ]}
            }}}, timeout=45)
            session_id = session_data.get('sessionId') if isinstance(session_data, dict) else None
            if not isinstance(session_id, str) or not re.fullmatch(r'[0-9a-fA-F]{32,64}', session_id):
                raise RuntimeError('Native ChromeDriver returned an invalid session identifier')
            session = '/session/' + session_id
            fixture = Path('tests/fixtures/engine-smoke.html').resolve(strict=True).as_uri()
            json_command(port, 'POST', session + '/url', {'url': fixture})
            actual = json_command(port, 'POST', session + '/execute/sync', {
                'script': "return document.getElementById('result').textContent;", 'args': []})
            if actual != 'NUMERA_RENDERER_FIXTURE_PASS':
                raise RuntimeError('Native WebDriver did not observe the renderer fixture marker')
            return {'result': 'PASS', 'driver_version': version,
                    'scope': 'ChromeDriver W3C HTTP session and a controlled local document'}
        finally:
            if session is not None:
                with suppress(OSError, ValueError, RuntimeError, http.client.HTTPException):
                    json_command(port, 'DELETE', session, timeout=5)


def smoke_clients(binary, driver, scratch, output, environment, stop_tree):
    isolated = {key: value for key, value in environment.items()
                if key.upper() not in {'NODE_OPTIONS', 'NODE_PATH', 'NODE_USE_ENV_PROXY',
                                       'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY'}}
    isolated['NO_PROXY'] = '127.0.0.1,localhost'
    report_path = output / 'automation-clients.json'
    fixture = Path('tests/fixtures/engine-smoke.html').resolve(strict=True).as_uri()
    suite = Path('tests/native-automation/clients.mjs').resolve(strict=True)
    with local_driver(driver, output / 'automation-driver.log', isolated, stop_tree) as (port, _):
        command = ['node', str(suite), f'127.0.0.1:{port}', str(binary),
                   str(scratch / 'automation-client-profile'), fixture, str(report_path)]
        with (output / 'automation-clients.log').open('wb') as stream:
            process = subprocess.Popen(command, env=isolated, stdout=stream, stderr=subprocess.STDOUT)
            try:
                if process.wait(timeout=180) != 0:
                    raise RuntimeError('A native automation client failed; inspect automation-clients.log')
            finally:
                if process.poll() is None:
                    stop_tree(process)
    report = json.loads(report_path.read_text(encoding='utf-8'))
    if report.get('result') != 'PASS' or set(report.get('tests', {})) != {'selenium', 'puppeteer', 'playwright'}:
        raise RuntimeError('Native automation client results are incomplete')
    return report
