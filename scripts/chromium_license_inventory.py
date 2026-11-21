#!/usr/bin/env python3
"""Generate review evidence with the pinned Chromium licensing tool and GN graph."""
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
from urllib.parse import urlsplit

from chromium_cache_probe import RELEASES, REPOSITORY, compile_graph, install_tool
from chromium_portable_fixture import describe_portable_runtime
from engine_build import now, sha256, write_json
from logging_util import log


def git_source_inventory(root, source, output, pin, environment):
    inventory = output / 'git-source-revisions.json'
    raw = root / 'git-source-revisions.unverified'
    with (output / 'source-inventory.log').open('wb') as diagnostics:
        subprocess.run([sys.executable, str(root / 'depot_tools/gclient.py'), 'revinfo', '--actual',
                        '--ignore-dep-type', 'cipd', '--ignore-dep-type', 'gcs', '--output-json', str(raw)],
                       cwd=source.parent, env=environment, stdout=diagnostics, stderr=subprocess.STDOUT,
                       timeout=180, check=True)
    if not 0 < raw.stat().st_size <= 4 * 1024**2:
        raise RuntimeError('The Chromium checkout revision inventory is out of bounds')
    revisions = json.loads(raw.read_text(encoding='utf-8'))
    raw.unlink()
    checked_out = {}
    for name, item in sorted(revisions.items()):
        origin, revision = item['url'], item['rev']
        if origin is None and revision is None:
            continue  # gclient includes disabled platform dependencies.
        url = urlsplit(origin)
        if (not re.fullmatch(r'src(?:/[A-Za-z0-9_.+\-]+)*', name)
                or any(part in {'.', '..'} for part in name.split('/'))
                or url.scheme != 'https' or not url.hostname or url.username is not None
                or url.password is not None or url.query or url.fragment
                or not re.fullmatch(r'[0-9a-f]{40}', revision or '')):
            raise RuntimeError('The native source inventory contains an invalid checkout record')
        checked_out[name] = {'url': origin, 'revision': revision}
    if len(checked_out) < 2 or checked_out.get('src', {}).get('revision') != pin['revision']:
        raise RuntimeError('The source inventory does not match the pinned Chromium checkout')
    deps = source / 'DEPS'
    write_json(inventory, {'schema_revision': 1, 'chromium_revision': pin['revision'],
               'workspace_dependencies_sha256': sha256(deps), 'git_checkouts': checked_out,
               'scope': 'Actual Git revisions. CIPD and GCS are recorded in the pinned DEPS reference.'})
    return {'git_checkouts': len(checked_out), 'sha256': sha256(inventory), 'deps_sha256': sha256(deps)}


def main():
    host = platform.system()
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text(encoding='utf-8'))
    if (os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'
            or os.environ.get('GITHUB_REPOSITORY') != REPOSITORY or event['repository']['private'] is not False
            or os.environ.get('RUNNER_ARCH') != 'X64' or host not in RELEASES):
        raise RuntimeError('Chromium license evidence requires an authorized public native runner')
    output = Path('build/chromium-license-evidence').resolve()
    output.mkdir(parents=True, exist_ok=False)
    root = Path(os.environ['RUNNER_TEMP']).resolve() / 'numera-chromium-licenses'
    tools = root.with_name(root.name + '-tools')
    tools.mkdir(exist_ok=False)
    environment = {key: value for key, value in os.environ.items() if key.upper() not in
                   {'GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_RUNTIME_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN'}}
    environment.update(DEPOT_TOOLS_UPDATE='0', DEPOT_TOOLS_WIN_TOOLCHAIN='0', PYTHONUTF8='1')
    environment['PATH'] = os.pathsep.join((str(tools), str(root / 'depot_tools'), environment['PATH']))
    lock = Path('upstream/chromium/lock.json')
    pin = json.loads(lock.read_text(encoding='utf-8'))['platform_pins'][host.lower()]
    report = {'schema_revision': 1, 'source_commit': os.environ['GITHUB_SHA'], 'host': host,
              'runner_image': os.environ['ImageVersion'], 'chromium_revision': pin['revision'],
              'chromium_version': pin['version'], 'source_lock_sha256': sha256(lock), 'targets': {},
              'engine_compilation': 'NOT_RUN', 'redistribution_approval': False, 'result': 'BLOCKED'}
    try:
        wrapper = install_tool(tools, RELEASES[host][0])
        compile_graph(root, environment, output, wrapper.stem)
        source = root / 'checkout/src'
        native = source / 'out/Numera'
        license_tool = source / 'tools/licenses/licenses.py'
        report.update(gn_args_sha256=sha256(native / 'args.gn'), license_tool_sha256=sha256(license_tool))
        runtime_layout = describe_portable_runtime(source, host, environment)
        layout_file = output / 'runtime-layout.json'
        write_json(layout_file, runtime_layout)
        report['runtime_layout'] = {'sha256': sha256(layout_file), 'paths': len(runtime_layout['dependencies']),
                                    'file_contents': 'NOT_RUN'}
        report['source_inventory'] = git_source_inventory(root, source, output, pin, environment)
        targets = {'chromium': '//chrome:chrome', 'chromedriver': '//chrome/test/chromedriver:chromedriver_server'}
        if host == 'Linux':
            targets['chrome-sandbox'] = '//sandbox/linux:chrome_sandbox'
        with (output / 'license-tool.log').open('wb') as diagnostics:
            for name, label in targets.items():
                arguments = ['--gn-target', label, '--gn-out-dir', str(native), '--target-os',
                             'win' if host == 'Windows' else 'linux']
                notice = output / f'{name}-NOTICE.txt'
                spdx = output / f'{name}.spdx.json'
                link = f"https://chromium.googlesource.com/chromium/src/+/{pin['revision']}/"
                for mode, file in [('notice', notice), ('spdx', spdx)]:
                    command = [sys.executable, str(license_tool), 'license_file', str(file), *arguments,
                               '--format', mode, '--spdx-link', link]
                    subprocess.run(command, cwd=source, env=environment, stdout=diagnostics,
                                   stderr=subprocess.STDOUT, timeout=240, check=True)
                if not 0 < notice.stat().st_size <= 16 * 1024**2 or not 0 < spdx.stat().st_size <= 16 * 1024**2:
                    raise RuntimeError('Pinned Chromium license tool emitted an out-of-bounds inventory')
                inventory = json.loads(spdx.read_text(encoding='utf-8'))
                if inventory.get('spdxVersion') != 'SPDX-2.2' or not inventory.get('packages'):
                    raise RuntimeError('Pinned Chromium license tool emitted an incomplete SPDX report')
                report['targets'][name] = {'gn_label': label, 'packages': len(inventory['packages']),
                                           'notice_sha256': sha256(notice), 'spdx_sha256': sha256(spdx)}
                log(f'Collected pinned-source notices and SPDX for {label}', component='chromium-license-inventory')
        report['result'] = 'PASS'
    finally:
        report['timestamp'] = now()
        write_json(output / 'report.json', report)


if __name__ == '__main__':
    main()
