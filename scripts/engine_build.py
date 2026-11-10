#!/usr/bin/env python3
"""Cold source compilation on authorized hosted runners, with live evidence.

This is build tooling, never the installed Core runtime. No compiled browser or
engine objects are downloaded, cached or published by this experiment.
"""
import argparse
import datetime
import hashlib
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time
import urllib.request

from logging_util import log


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def write_json(path, value):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')
    temporary.replace(path)


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as file:
        for block in iter(lambda: file.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


class RendererFixture(HTMLParser):
    def __init__(self):
        super().__init__()
        self.in_result = False
        self.result = ''

    def handle_starttag(self, tag, attributes):
        if tag == 'strong' and dict(attributes).get('id') == 'result':
            self.in_result = True

    def handle_endtag(self, tag):
        if tag == 'strong':
            self.in_result = False

    def handle_data(self, value):
        if self.in_result:
            self.result += value


def renderer_passed(html):
    parser = RendererFixture()
    parser.feed(html)
    return parser.result == 'NUMERA_RENDERER_FIXTURE_PASS'


def stop_tree(process):
    """Stop only the tracked build subtree, with PID/create-time validation."""
    import psutil
    try:
        parent = psutil.Process(process.pid)
        children = parent.children(recursive=True)
    except psutil.NoSuchProcess:
        return
    for child in [*reversed(children), parent]:
        try:
            child.terminate()
        except psutil.NoSuchProcess:
            pass
    _, alive = psutil.wait_procs([*children, parent], timeout=5)
    for child in alive:
        try:
            child.kill()
        except psutil.NoSuchProcess:
            pass
    psutil.wait_procs(alive, timeout=5)


class CheckRun:
    def __init__(self):
        self.token = os.environ.get('GH_TOKEN')
        self.repo = os.environ['GITHUB_REPOSITORY']
        self.id = None

    def update(self, report, complete=False):
        summary = (f"Stage: `{report['stage']}`\n\nElapsed: {report['elapsed_seconds']:.0f} seconds. "
                   f"Free disk: {report['disk_free_bytes']/1e9:.2f} GB. "
                   f"Peak sampled process-tree RSS: {report['peak_tree_rss_bytes']/1e9:.2f} GB.\n\n"
                   f"Result: **{report['result']}**. Release ready: **false**.\n\n"
                   "Source compilation and a controlled renderer fixture are separate from full browser/automation/release acceptance.")
        body = {'name': 'Chromium cold source build telemetry', 'status': 'completed' if complete else 'in_progress',
                'details_url': report['run_url'], 'output': {'title': report['stage'], 'summary': summary}}
        if complete:
            body['conclusion'] = 'success' if report['result'] == 'PASS' else 'failure'
        if self.id is None:
            body['head_sha'] = report['commit']
        path = f'/repos/{self.repo}/check-runs' + (f'/{self.id}' if self.id else '')
        request = urllib.request.Request('https://api.github.com' + path, data=json.dumps(body).encode(),
            method='PATCH' if self.id else 'POST', headers={'Authorization': 'Bearer ' + self.token,
            'Accept': 'application/vnd.github+json', 'X-GitHub-Api-Version': '2026-03-10', 'User-Agent': 'Numera-Core-Browser'})
        with urllib.request.urlopen(request, timeout=20) as response:
            self.id = json.load(response)['id']


def smoke(binary, scratch, output, environment):
    profile = scratch / 'fixture-profile'
    fixture = (Path('tests/fixtures/engine-smoke.html').resolve()).as_uri()
    commands = [
        ('version', [str(binary), '--version']),
        ('renderer', [str(binary), '--headless=new', '--no-first-run', '--no-default-browser-check',
                      '--user-data-dir=' + str(profile), '--dump-dom', fixture]),
        ('screenshot', [str(binary), '--headless=new', '--no-first-run', '--no-default-browser-check',
                       '--user-data-dir=' + str(profile), '--window-size=800,600',
                       '--screenshot=' + str((output / 'renderer.png').resolve()), fixture]),
    ]
    for name, command in commands:
        with (output / f'smoke-{name}.stdout').open('wb') as stdout, (output / f'smoke-{name}.stderr').open('wb') as stderr:
            process = subprocess.Popen(command, cwd=binary.parent, env=environment, stdout=stdout, stderr=stderr)
            try:
                code = process.wait(timeout=60)
            except subprocess.TimeoutExpired:
                stop_tree(process)
                raise RuntimeError('Controlled renderer fixture timed out') from None
        if code != 0:
            raise RuntimeError('Controlled source-built browser exited with a failure')
    if not renderer_passed((output / 'smoke-renderer.stdout').read_text(encoding='utf-8', errors='replace')):
        raise RuntimeError('Renderer did not execute the fixture script')
    if not (output / 'renderer.png').read_bytes().startswith(b'\x89PNG\r\n\x1a\n'):
        raise RuntimeError('Screenshot fixture did not produce a PNG')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', required=True)
    parser.add_argument('--budget-minutes', type=int, default=330)
    parser.add_argument('--component-build', action='store_true')
    args = parser.parse_args()
    if not 10 <= args.budget_minutes <= 330:
        parser.error('Budget must be between 10 and 330 minutes')
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted' or os.environ.get('GITHUB_REPOSITORY') != 'KiaroSama/Numera-Core-Browser':
        parser.error('Engine builds require the authorized public GitHub-hosted repository')
    import psutil
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    scratch = Path(os.environ['RUNNER_TEMP']) / ('numera-cold-' + os.environ['GITHUB_RUN_ID'])
    scratch.mkdir(exist_ok=False)
    started = time.monotonic()
    initial_disk = shutil.disk_usage(scratch)
    report = {'schema_revision': 1, 'timestamp': now(), 'commit': os.environ['GITHUB_SHA'],
        'run_id': os.environ['GITHUB_RUN_ID'], 'run_url': f"https://github.com/{os.environ['GITHUB_REPOSITORY']}/actions/runs/{os.environ['GITHUB_RUN_ID']}",
        'runner_os': platform.system(), 'runner_arch': os.environ['RUNNER_ARCH'],
        'runner_image_version': os.environ.get('ImageVersion'), 'psutil_version': psutil.__version__,
        'cpu_count': os.cpu_count(), 'physical_memory_bytes': psutil.virtual_memory().total,
        'budget_minutes': args.budget_minutes, 'component_build': args.component_build,
        'parallelism': 2, 'native_source_targets': ['chrome', 'chromedriver'],
        'disk_total_bytes': initial_disk.total, 'disk_free_initial_bytes': initial_disk.free,
        'disk_free_bytes': initial_disk.free, 'minimum_free_disk_bytes': initial_disk.free,
        'elapsed_seconds': 0, 'peak_tree_rss_bytes': 0, 'peak_tree_process_count': 0,
        'stage': 'resource-preflight', 'result': 'RUNNING', 'release_ready': False,
        'cold_cache': True, 'precompiled_engine_objects_reused': False, 'source_build': 'NOT_RUN',
        'renderer_fixture': 'NOT_RUN', 'automation_suites': 'NOT_RUN', 'source_lock': json.loads(Path('upstream/chromium/lock.json').read_text())}
    checks = CheckRun()
    process = None
    next_publish = 0
    environment = dict(os.environ)
    for key in ['GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_RUNTIME_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN']:
        environment.pop(key, None)
    try:
        if initial_disk.free < 100_000_000_000:
            raise RuntimeError('Conservative 100 GB source-build disk preflight failed')
        command = ['pwsh', '-NoProfile', '-File', 'scripts/Build-Chromium.ps1', '-SourceRoot', str(scratch), '-Phase', 'Build', '-Parallelism', '2', '-BuildDriver']
        if args.component_build:
            command.append('-ComponentBuild')
        with (output / 'native-build.log').open('wb') as log_file:
            process = subprocess.Popen(command, stdout=log_file, stderr=subprocess.STDOUT, env=environment)
            parent = psutil.Process(process.pid)
            while process.poll() is None:
                elapsed = time.monotonic() - started
                report['elapsed_seconds'] = elapsed
                report['disk_free_bytes'] = shutil.disk_usage(scratch).free
                report['minimum_free_disk_bytes'] = min(report['minimum_free_disk_bytes'], report['disk_free_bytes'])
                phase = scratch / 'phase.txt'
                if phase.exists():
                    report['stage'] = phase.read_text(encoding='utf-8-sig').strip()
                try:
                    tree = [parent, *parent.children(recursive=True)]
                    rss = 0
                    for child in tree:
                        try:
                            if child.is_running():
                                rss += child.memory_info().rss
                        except (psutil.NoSuchProcess, psutil.AccessDenied):
                            pass
                    report['peak_tree_rss_bytes'] = max(report['peak_tree_rss_bytes'], rss)
                    report['peak_tree_process_count'] = max(report['peak_tree_process_count'], len(tree))
                except psutil.NoSuchProcess:
                    pass
                if elapsed >= args.budget_minutes * 60:
                    raise RuntimeError('Bounded cold source-build time budget exhausted')
                if report['disk_free_bytes'] < 20_000_000_000:
                    raise RuntimeError('Free disk guard reached; preserve 20 GB')
                if elapsed >= next_publish:
                    report['timestamp'] = now()
                    write_json(output / 'report.json', report)
                    checks.update(report)
                    log(f"{report['stage']}: {elapsed:.0f}s, {report['disk_free_bytes']/1e9:.2f} GB free", component='engine-build')
                    next_publish = elapsed + 30
                time.sleep(1)
        report['native_exit_code'] = process.returncode
        if process.returncode != 0:
            raise RuntimeError('Pinned native source-build recipe failed; inspect retained native-build.log')
        report['source_build'] = 'PASS'
        report['stage'] = 'controlled-renderer-fixture'
        binary = scratch / 'checkout/src/out/Numera' / ('chrome.exe' if os.name == 'nt' else 'Chromium.app/Contents/MacOS/Chromium')
        report['binary_sha256'] = sha256(binary)
        report['binary_bytes'] = binary.stat().st_size
        smoke(binary.resolve(), scratch, output, environment)
        report['renderer_fixture'] = 'PASS'
        report['result'] = 'PASS'
        report['stage'] = 'source-and-renderer-pass-release-gates-open'
    except Exception as error:
        if process is not None and process.poll() is None:
            stop_tree(process)
        report['result'] = 'BLOCKED'
        report['error_type'] = type(error).__name__
        report['reason'] = str(error) if isinstance(error, RuntimeError) else 'Build monitor failed; inspect the retained exception type and native log'
        log(report['reason'], severity='ERROR', component='engine-build')
    finally:
        report['timestamp'] = now()
        report['elapsed_seconds'] = round(time.monotonic() - started, 3)
        for name in ['phase.txt', 'phases.ndjson', 'gn-args.txt']:
            if (scratch / name).is_file():
                shutil.copyfile(scratch / name, output / name)
        report['sampled_volume_delta_bytes'] = initial_disk.free - report['minimum_free_disk_bytes']
        report['measurement_note'] = 'RSS sums sampled process-tree resident memory; shared pages may be double-counted. Volume delta is not exact engine disk allocation.'
        write_json(output / 'report.json', report)
        try:
            checks.update(report, complete=True)
        except Exception as error:
            report['telemetry_publish_error_type'] = type(error).__name__
            write_json(output / 'report.json', report)
        log('Cold source-build result: ' + report['result'] + '; release ready: false', component='engine-build')
    return 0 if report['result'] == 'PASS' else 1


if __name__ == '__main__':
    raise SystemExit(main())
