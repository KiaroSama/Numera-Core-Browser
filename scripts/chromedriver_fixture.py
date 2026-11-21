#!/usr/bin/env python3
"""Exercise an owned ChromeDriver and Chromium pair on a local fixture."""
from contextlib import contextmanager, suppress
import http.client
import json
from pathlib import Path
import platform
import re
import subprocess
import time

MAX_RESPONSE = 256 * 1024
STARTED = re.compile(rb'(?m)^ChromeDriver was started successfully on port ([1-9][0-9]{0,4})\.\r?$')
FIXTURE_TITLE = 'Numera source build fixture'
DRIVER_ERRORS = {'invalid argument', 'invalid session id', 'no such window', 'session not created',
                 'timeout', 'unknown error', 'unsupported operation'}


def json_command(port, method, path, data=None, timeout=15):
    body = json.dumps(data).encode('utf-8') if data is not None else None
    headers = {'Content-Type': 'application/json; charset=utf-8'} if data is not None else {}
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=timeout)
    try:
        connection.request(method, path, body=body, headers=headers)
        response = connection.getresponse()
        raw = response.read(MAX_RESPONSE + 1)
        if len(raw) > MAX_RESPONSE:
            raise RuntimeError('Native ChromeDriver rejected the controlled command (response size limit exceeded)')
        try:
            value = json.loads(raw)
        except (ValueError, RecursionError):
            value = None
        if response.status != 200:
            detail = value.get('value') if isinstance(value, dict) else None
            error = detail.get('error') if isinstance(detail, dict) else None
            status = f'HTTP {response.status}'
            if isinstance(error, str) and error in DRIVER_ERRORS:
                status += f', {error}'
            raise RuntimeError(f'Native ChromeDriver rejected the controlled command ({status})')
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


def verify_native_window(browser_pid, environment):
    if type(browser_pid) is not int or browser_pid <= 0:
        raise RuntimeError('Native ChromeDriver did not identify the headed browser process')
    host = platform.system()
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if host == 'Linux':
            display_environment = dict(environment, LC_ALL='C.UTF-8')
            tree = subprocess.check_output(['xwininfo', '-root', '-tree'], env=display_environment, timeout=3,
                                           encoding='utf-8')
            window_pattern = rf'^\s*(0x[0-9a-f]+) "{re.escape(FIXTURE_TITLE)}(?: - [^"]+)?":'
            for window in re.findall(window_pattern, tree, re.MULTILINE):
                owner = subprocess.run(['xprop', '-id', window, '_NET_WM_PID'], env=display_environment, timeout=3,
                                       stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, encoding='utf-8', check=False)
                if owner.returncode != 0 or owner.stdout.strip() != f'_NET_WM_PID(CARDINAL) = {browser_pid}':
                    continue
                state = subprocess.run(['xwininfo', '-id', window, '-stats'], env=display_environment, timeout=3,
                                       stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, encoding='utf-8', check=False)
                if state.returncode == 0 and 'Map State: IsViewable' in state.stdout:
                    return 'x11-viewable-window-with-browser-pid'
        elif host == 'Windows':
            # The PID comes from this session and is validated as an integer before use.
            command = (f'$Browser = Get-Process -Id {browser_pid} -ErrorAction Stop\n'
                       'if ($Browser.MainWindowHandle -ne 0) { $Browser.MainWindowTitle }')
            title = subprocess.check_output(['pwsh', '-NoProfile', '-NonInteractive', '-Command', command],
                                            env=environment, timeout=5, encoding='utf-8').strip()
            if title == FIXTURE_TITLE or title.startswith(FIXTURE_TITLE + ' - '):
                return 'windows-main-window-with-browser-pid'
        else:
            raise RuntimeError('The headed fixture requires native Linux or Windows')
        time.sleep(0.2)
    raise RuntimeError('The headed browser did not expose the controlled native window')


def smoke_driver(binary, driver, scratch, output, environment, stop_tree, *, headed=False):
    session = None
    with local_driver(driver, output / 'chromedriver-fixture.log', environment, stop_tree) as (port, version):
        try:
            profile = scratch / 'driver-fixture-profile'
            arguments = ['--no-first-run', '--no-default-browser-check', '--user-data-dir=' + str(profile)]
            if not headed:
                arguments.insert(0, '--headless=new')
            session_data = json_command(port, 'POST', '/session', {'capabilities': {'alwaysMatch': {
                'browserName': 'chrome', 'goog:chromeOptions': {'binary': str(binary), 'args': arguments}
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
            report = {'result': 'PASS', 'driver_version': version,
                      'scope': 'ChromeDriver W3C HTTP session and a controlled local document'}
            if headed:
                process_id = session_data.get('capabilities', {}).get('goog:processID')
                report['native_window'] = verify_native_window(process_id, environment)
                report['scope'] = 'Headed W3C navigation and an OS window owned by the browser process'
            return report
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
