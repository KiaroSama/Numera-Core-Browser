"""Exercise an actual Chromium process tree through the Rust ownership fixture."""
import http.client
import json
from pathlib import Path
import platform
import re
import subprocess
import time

import psutil
from engine_build import stop_tree


def browser_rendered(port_file, fixture):
    try:
        with port_file.open('rb') as file:
            port = file.readline(8).strip()
        if not re.fullmatch(rb'[1-9][0-9]{0,4}', port) or not 1 <= int(port) <= 65535:
            return False
        request = http.client.HTTPConnection('127.0.0.1', int(port), timeout=2)
        try:
            request.request('GET', '/json/list')
            response = request.getresponse()
            data = response.read(65_537)
            if response.status != 200 or len(data) > 65_536:
                return False
            targets = json.loads(data)
            return isinstance(targets, list) and any(
                isinstance(item, dict) and item.get('type') == 'page' and item.get('url') == fixture
                and item.get('title') == 'Numera source build fixture' for item in targets)
        finally:
            request.close()
    except (OSError, ValueError, http.client.HTTPException):
        return False


def smoke_browser_ownership(browser, scratch, output, environment):
    suffix = '.exe' if platform.system() == 'Windows' else ''
    supervisor = (Path('target/debug') / f'numera-process-fixture{suffix}').resolve(strict=True)
    root = scratch / 'Rust browser ownership'
    root.mkdir()
    fixture = Path('tests/fixtures/engine-smoke.html').resolve(strict=True).as_uri()
    port_file = root / 'داده‌های مرورگر' / 'DevToolsActivePort'
    with (output / 'chromium-ownership.log').open('wb') as log:
        worker = subprocess.Popen([str(supervisor), 'chromium-owner', str(root), str(browser), fixture],
                                  env=environment, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 45
            while not browser_rendered(port_file, fixture):
                if worker.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError('The Rust ownership fixture did not observe Chromium rendering the local page')
                time.sleep(0.1)
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
    return {'result': 'PASS', 'rendered': 'PASS', 'owned_tree_shutdown': 'PASS',
            'observed_processes': len(observed), 'profile_path': 'Unicode with spaces'}
