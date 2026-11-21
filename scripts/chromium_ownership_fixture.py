"""Exercise an actual Chromium process tree through the Rust ownership fixture."""
import http.client
import json
from pathlib import Path
import platform
import re
import subprocess
import time
from urllib.parse import urlsplit
from urllib.request import url2pathname

import psutil
from engine_build import stop_tree


def local_file_url_path(url):
    parts = urlsplit(url)
    if parts.scheme == 'file' and parts.netloc in {'', 'localhost'} and not parts.query and not parts.fragment:
        return Path(url2pathname(parts.path))
    return None


def observe_browser(port_file, fixture):
    try:
        with port_file.open('rb') as file:
            port = file.readline(8).strip()
    except FileNotFoundError:
        return 'waiting for the debugger port file', False
    except OSError:
        return 'debugger port file was unreadable', False
    if not re.fullmatch(rb'[1-9][0-9]{0,4}', port) or not 1 <= int(port) <= 65535:
        return 'debugger port file held an invalid port', False
    try:
        request = http.client.HTTPConnection('127.0.0.1', int(port), timeout=2)
        try:
            request.request('GET', '/json/list')
            response = request.getresponse()
            data = response.read(65_537)
            if response.status != 200 or len(data) > 65_536:
                return 'debugger returned an invalid target response', False
            targets = json.loads(data)
            if isinstance(targets, list):
                expected = local_file_url_path(fixture)
                for item in targets:
                    if (isinstance(item, dict) and item.get('type') == 'page'
                            and item.get('title') == 'Numera source build fixture'
                            and isinstance(item.get('url'), str) and expected is not None
                            and local_file_url_path(item['url']) == expected):
                        return 'rendered', item['url'] != fixture
            return 'debugger responded without the controlled page', False
        finally:
            request.close()
    except (OSError, ValueError, http.client.HTTPException):
        return 'debugger did not return usable target data', False


def capture_startup_sites(source, output):
    if source.is_file():
        with source.open('rb') as stream:
            stream.seek(max(0, source.stat().st_size - 64 * 1024))
            logs = stream.read(64 * 1024)
        # Keep fixed severity and source locations. Browser-supplied text can contain private paths.
        sites = re.findall(rb':(WARNING|ERROR|FATAL):([A-Za-z0-9_.\\/]+):(\d+)\]', logs)
        records = [{'level': level.decode('ascii'), 'location': path.decode('ascii'), 'line': int(line)}
                   for level, path, line in sites[-30:]]
        (output / 'chromium-startup-sites.json').write_text(json.dumps(records) + '\n', encoding='utf-8')


def capture_process_shape(worker, profile, output):
    counts = {'browser': 0, 'renderer': 0, 'gpu-process': 0, 'utility': 0, 'other': 0}
    root_arguments = None
    try:
        observed = psutil.Process(worker.pid).children(recursive=True)
    except (psutil.NoSuchProcess, psutil.AccessDenied):
        observed = []
    for process in observed:
        try:
            arguments = process.cmdline()[1:]
            parent_pid = process.ppid()
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
        role = next((arg.partition('=')[2] for arg in arguments if arg.startswith('--type=')), 'browser')
        counts[role if role in counts else 'other'] += 1
        if parent_pid == worker.pid and role == 'browser':
            root_arguments = {
                'headless': '--headless=new' in arguments,
                'auto_debugging_port': '--remote-debugging-port=0' in arguments,
                'explicit_profile': any(arg.startswith('--user-data-dir=') and
                                        Path(arg.partition('=')[2]) == profile for arg in arguments),
            }
    summary = {'process_roles': counts, 'browser_arguments': root_arguments,
               'profile_created': profile.is_dir(), 'local_state_created': (profile / 'Local State').is_file(),
               'debugger_port_created': (profile / 'DevToolsActivePort').is_file()}
    (output / 'chromium-ownership-startup.json').write_text(json.dumps(summary) + '\n', encoding='utf-8')


def smoke_browser_ownership(browser, scratch, output, environment):
    suffix = '.exe' if platform.system() == 'Windows' else ''
    supervisor = (Path('target/debug') / f'numera-process-fixture{suffix}').resolve(strict=True)
    root = scratch / 'Rust browser ownership'
    root.mkdir()
    fixture = Path('tests/fixtures/engine-smoke.html').resolve(strict=True).as_uri()
    profile = root / 'داده‌های مرورگر'
    port_file = profile / 'DevToolsActivePort'
    with (output / 'chromium-ownership.log').open('wb') as log:
        worker = subprocess.Popen([str(supervisor), 'chromium-owner', str(root), str(browser), fixture],
                                  env=environment, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 45
            observation, equivalent_url = observe_browser(port_file, fixture)
            while observation != 'rendered':
                if worker.poll() is not None or time.monotonic() >= deadline:
                    capture_process_shape(worker, profile, output)
                    raise RuntimeError(f'The Rust ownership fixture did not observe the controlled page ({observation})')
                time.sleep(0.1)
                observation, equivalent_url = observe_browser(port_file, fixture)
            observed = psutil.Process(worker.pid).children(recursive=True)
            if len(observed) < 2:
                raise RuntimeError('The Chromium ownership fixture did not observe a browser tree')
            worker.stdin.write(b's')
            worker.stdin.flush()
            if worker.wait(timeout=16) != 0:
                raise RuntimeError('The Rust ownership fixture could not stop the Chromium tree')
            _, remaining = psutil.wait_procs(observed, timeout=5)
            if remaining:
                raise RuntimeError('Observed Chromium processes remained after fixture shutdown')
        finally:
            worker.stdin.close()
            if worker.poll() is None:
                try:
                    worker.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    stop_tree(worker)
                    worker.wait(timeout=5)
            capture_startup_sites(root / 'chromium-startup.log', output)
    return {'result': 'PASS', 'rendered': 'PASS', 'owned_tree_shutdown': 'PASS',
            'observed_processes': len(observed), 'profile_path': 'Unicode with spaces',
            'normalized_file_url': equivalent_url}
