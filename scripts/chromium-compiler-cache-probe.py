#!/usr/bin/env python3
"""Measure reuse of one real Chromium object on fresh standard hosted jobs."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import subprocess
import tarfile
import time
import urllib.request
import zipfile

from engine_build import sha256, stop_tree, write_json
from logging_util import log

REPOSITORY = 'KiaroSama/Numera-Core-Browser'
ARCHIVE_LIMIT = 32 * 1024 * 1024
RELEASES = {
    'Linux': (
        ('mozilla/sccache', 'v0.19.0', 'sccache-v0.19.0-x86_64-unknown-linux-musl.tar.gz',
         'aa73c7270c997894038c0d8f0d297865f57569e1687883322bc431713d4bb9dd', 9879308, 'sccache'),
        ('ninja-build/ninja', 'v1.13.2', 'ninja-linux.zip',
         '5749cbc4e668273514150a80e387a957f933c6ed3f5f11e03fb30955e2bbead6', 134040, 'ninja'),
    ),
    'Windows': (
        ('mozilla/sccache', 'v0.19.0', 'sccache-v0.19.0-x86_64-pc-windows-msvc.tar.gz',
         '130e4f8eb672e76590e9993c2ab890c4763b88238577bb11d8062158c7698353', 9830016, 'sccache.exe'),
        ('ninja-build/ninja', 'v1.13.2', 'ninja-win.zip',
         '07fc8261b42b20e71d1720b39068c2e14ffcee6396b76fb7a795fb460b78dc65', 291570, 'ninja.exe'),
    ),
}


def install_tool(directory, release):
    repo, tag, asset, expected, size, executable = release
    url = f'https://github.com/{repo}/releases/download/{tag}/{asset}'
    with urllib.request.urlopen(url, timeout=30) as response:
        payload = response.read(size + 1)
    if len(payload) != size or hashlib.sha256(payload).hexdigest() != expected:
        raise RuntimeError(f'Official {executable} transport verification failed')
    if asset.endswith('.zip'):
        with zipfile.ZipFile(io.BytesIO(payload)) as archive:
            binary = archive.read(executable)
    else:
        with tarfile.open(fileobj=io.BytesIO(payload), mode='r:gz') as archive:
            members = [m for m in archive if m.isfile() and PurePosixPath(m.name).name == executable]
            if len(members) != 1 or members[0].size > ARCHIVE_LIMIT:
                raise RuntimeError(f'Unexpected {executable} release member')
            binary = archive.extractfile(members[0]).read()
    path = directory / executable
    path.write_bytes(binary)
    path.chmod(0o700)
    return path


def compile_graph(root, environment, output):
    if shutil.disk_usage(root.parent).free < 80_000_000_000:
        raise RuntimeError('Real-graph probe requires at least 80 GB free before acquisition')
    command = ['pwsh', '-NoProfile', '-File', 'scripts/Build-Chromium.ps1', '-SourceRoot', str(root),
               '-Phase', 'Generate', '-CompilerCacheProbe', '-ComponentBuild', '-DebugBuild']
    with (output / 'graph.log').open('wb') as log_file:
        child = subprocess.Popen(command, env=environment, stdout=log_file, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 40 * 60
        phase = root / 'phase.txt'
        previous = ''
        try:
            while child.poll() is None:
                if time.monotonic() >= deadline or shutil.disk_usage(root.parent).free < 25_000_000_000:
                    raise RuntimeError('Real-graph probe reached its acquisition time or disk guard')
                if phase.is_file():
                    current = phase.read_text(encoding='utf-8-sig').strip()
                    if current != previous and re.fullmatch(r'[a-z-]+', current):
                        log(f'Graph preparation: {current}', component='compiler-cache-probe')
                        previous = current
                time.sleep(2)
        finally:
            if child.poll() is None:
                stop_tree(child)
    if child.returncode != 0:
        raise RuntimeError('Pinned Chromium graph preparation failed; inspect graph.log')


def pack_cache(root, archive, context, object_hash):
    entries = sorted(root.rglob('*'))
    if any(p.is_symlink() or not (p.is_dir() or p.is_file()) for p in entries):
        raise RuntimeError('Unexpected link or special file in local compiler cache')
    files = [p for p in entries if p.is_file() and p.name != 'CACHEDIR.TAG']
    if not files or len(files) > 512 or sum(p.stat().st_size for p in files) >= ARCHIVE_LIMIT - 1024 * 1024:
        raise RuntimeError('Compiler cache is empty or exceeds the bounded probe allowance')
    manifest = {'context': context, 'object_sha256': object_hash,
                'files': {p.relative_to(root).as_posix(): sha256(p) for p in files}}
    contents = json.dumps(manifest, sort_keys=True).encode('utf-8')
    with tarfile.open(archive, 'x') as bundle:
        info = tarfile.TarInfo('manifest.json')
        info.size = len(contents)
        bundle.addfile(info, io.BytesIO(contents))
        for path in files:
            bundle.add(path, arcname=path.relative_to(root).as_posix(), recursive=False)
    if archive.stat().st_size > ARCHIVE_LIMIT:
        raise RuntimeError('Compiler cache transport exceeds 32 MiB')


def restore_cache(root, archive, expected, context):
    if not re.fullmatch(r'[0-9a-f]{64}', expected or '') or archive.stat().st_size > ARCHIVE_LIMIT or sha256(archive) != expected:
        raise RuntimeError('Checkpoint does not match this attempt’s seed digest')
    with tarfile.open(archive, 'r:') as bundle:
        members = bundle.getmembers()
        names = [m.name for m in members]
        if (not 2 <= len(members) <= 513 or len({n.casefold() for n in names}) != len(names)
                or sum(m.size for m in members) > ARCHIVE_LIMIT
                or any(not m.isfile() or not re.fullmatch(r'[A-Za-z0-9_-]+(?:/[A-Za-z0-9_-]+)*|manifest\.json', m.name)
                       for m in members)):
            raise RuntimeError('Unexpected compiler-cache archive members')
        manifest = json.load(bundle.extractfile('manifest.json'))
        if manifest['context'] != context:
            changed = sorted(k for k in context if manifest['context'].get(k) != context[k])
            raise RuntimeError('Checkpoint build context changed: ' + ', '.join(changed))
        names.remove('manifest.json')
        if set(manifest['files']) != set(names):
            raise RuntimeError('Checkpoint inventory is incomplete')
        for name in names:
            if hashlib.sha256(bundle.extractfile(name).read()).hexdigest() != manifest['files'][name]:
                raise RuntimeError('Checkpoint cache member digest mismatch')
        for name in names:
            path = root.joinpath(*PurePosixPath(name).parts)
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open('xb') as output:
                shutil.copyfileobj(bundle.extractfile(name), output)
    return manifest['object_sha256']


def stats(tool, environment):
    result = subprocess.check_output([str(tool), '--show-stats', '--stats-format=json'], env=environment, timeout=20)
    return json.loads(result)['stats']


def count(statistics, metric):
    value = statistics[metric]
    return sum(value['counts'].values()) if isinstance(value, dict) else value


def native_object(ninja, cache, environment, source, target, output, phase):
    obj = source / 'out/Numera' / target
    obj.unlink(missing_ok=True)
    subprocess.run([str(cache), '--zero-stats'], env=environment, timeout=20, check=True, stdout=subprocess.DEVNULL)
    with (output / f'{phase}.log').open('wb') as stream:
        command = [str(ninja), '-C', 'out/Numera', '-j', '4', target]
        child = subprocess.Popen(command, cwd=source, env=environment, stdout=stream, stderr=subprocess.STDOUT)
        try:
            exit_code = child.wait(timeout=600)
        except subprocess.TimeoutExpired:
            raise RuntimeError(f'{phase} object build timed out') from None
        finally:
            if child.poll() is None:
                stop_tree(child)
        if exit_code != 0:
            raise RuntimeError(f'{phase} object build failed; inspect {phase}.log')
    values = stats(cache, environment)
    write_json(output / f'{phase}-statistics.json', values)
    return sha256(obj), values


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('phase', choices=['seed', 'resume'])
    args = parser.parse_args()
    host = platform.system()
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text(encoding='utf-8'))
    if (os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'
            or os.environ.get('GITHUB_REPOSITORY') != REPOSITORY or event['repository']['private'] is not False
            or os.environ.get('RUNNER_ARCH') != 'X64' or host not in RELEASES):
        raise RuntimeError('The probe requires this public repository and a standard Windows or Linux x86-64 job')
    output = Path('build/chromium-cache-evidence').resolve()
    output.mkdir(parents=True, exist_ok=False)
    transfer = Path('build/chromium-cache-transfer/seed.tar').resolve()
    root = Path(os.environ['RUNNER_TEMP']).resolve() / 'numera-compiler-cache-probe'
    tools = root.with_name(root.name + '-tools')
    tools.mkdir(exist_ok=False)
    cache = tools / 'entries'
    cache.mkdir()
    report = {'phase': args.phase, 'runner': host, 'source_commit': os.environ['GITHUB_SHA'],
              'result': 'BLOCKED', 'engine_source_acceptance': 'NOT_RUN', 'release_ready': False}
    environment = {k: v for k, v in os.environ.items() if not k.upper().startswith('SCCACHE_') and k not in
                   {'GH_TOKEN', 'GITHUB_TOKEN', 'ACTIONS_RUNTIME_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN'}}
    environment.update(SCCACHE_DIR=str(cache), SCCACHE_CACHE_SIZE='24M', SCCACHE_IDLE_TIMEOUT='1200',
                       SCCACHE_SERVER_PORT='4234', DEPOT_TOOLS_UPDATE='0', DEPOT_TOOLS_WIN_TOOLCHAIN='0')
    environment['PATH'] = os.pathsep.join((str(tools), str(root / 'depot_tools'), environment['PATH']))
    wrapper = None
    try:
        wrapper, ninja = [install_tool(tools, spec) for spec in RELEASES[host]]
        log('Acquiring pinned Chromium source and generating the native graph', component='compiler-cache-probe')
        compile_graph(root, environment, output)
        source = root / 'checkout/src'
        gn_dir = source / 'out/Numera'
        target = 'obj/base/base/logging.obj' if host == 'Windows' else 'obj/base/base/logging.o'
        commands = subprocess.check_output([str(ninja), '-C', 'out/Numera', '-t', 'commands', target],
                                           cwd=source, env=environment, timeout=30)
        if b'sccache ' not in commands.splitlines()[-1]:
            raise RuntimeError('The actual GN compile command did not use the pinned local compiler wrapper')
        clang = source / ('third_party/llvm-build/Release+Asserts/bin/clang-cl.exe' if host == 'Windows'
                          else 'third_party/llvm-build/Release+Asserts/bin/clang')
        lock = Path('upstream/chromium/lock.json')
        context = {'run_id': os.environ['GITHUB_RUN_ID'], 'run_attempt': os.environ['GITHUB_RUN_ATTEMPT'],
                   'runner_image': os.environ['ImageVersion'], 'host': host, 'head': os.environ['GITHUB_SHA'],
                   'scratch': str(root), 'target': target, 'lock_sha256': sha256(lock), 'gn_args_sha256': sha256(gn_dir / 'args.gn'),
                   'sccache_sha256': sha256(wrapper), 'ninja_sha256': sha256(ninja), 'clang_sha256': sha256(clang),
                   'command_sha256': hashlib.sha256(commands.splitlines()[-1]).hexdigest(),
                   'input_sha256': sha256(source / 'base/logging.cc')}
        if host == 'Windows':
            sdk = json.loads((root / 'windows-sdk.json').read_text(encoding='utf-8-sig'))
            context['sdk'] = {k: sdk[k] for k in ('directory_version', 'debugging_tools_version', 'requested_release')}
        report['context'] = context
        if args.phase == 'resume':
            if (gn_dir / target).exists():
                raise RuntimeError('Fresh runner unexpectedly contains the compiled probe object')
            seed_hash = restore_cache(cache, transfer, os.environ.get('EXPECTED_CHECKPOINT_SHA'), context)
        subprocess.run([str(wrapper), '--start-server'], env=environment, check=True, timeout=20)
        if args.phase == 'seed':
            digest, cold = native_object(ninja, wrapper, environment, source, target, output, 'cold')
            if count(cold, 'cache_hits') != 0 or not 0 < cold['compile_requests'] == count(cold, 'cache_misses') == cold['cache_writes']:
                raise RuntimeError('Cold Chromium compilation did not populate an initially empty cache')
            subprocess.run([str(wrapper), '--stop-server'], env=environment, timeout=20, check=True)
            wrapper = None
            transfer.parent.mkdir(parents=True, exist_ok=False)
            pack_cache(cache, transfer, context, digest)
            report.update(checkpoint_bytes=transfer.stat().st_size, checkpoint_sha256=sha256(transfer), object_sha256=digest)
            with Path(os.environ['GITHUB_OUTPUT']).open('a', encoding='utf-8') as exported:
                exported.write('checkpoint_sha=' + report['checkpoint_sha256'] + '\n')
        else:
            digest, warm = native_object(ninja, wrapper, environment, source, target, output, 'restored')
            hits = count(warm, 'cache_hits')
            if not 0 < hits == warm['compile_requests'] or count(warm, 'cache_misses') != 0 or warm['cache_writes'] != 0 or digest != seed_hash:
                raise RuntimeError('Different runner did not reuse the seed object')
            input_file = source / 'base/logging.cc'
            with input_file.open('ab') as edited:
                edited.write(b'\nextern "C" int NumeraCacheProbeValue();\nextern "C" int NumeraCacheProbeValue() { return 17; }\n')
            changed, edited = native_object(ninja, wrapper, environment, source, target, output, 'changed-source')
            misses = count(edited, 'cache_misses')
            if misses != 1 or edited['compile_requests'] != 1 or count(edited, 'cache_hits') != 0 or changed == digest:
                raise RuntimeError('A changed Chromium input failed to invalidate the cached output')
            report.update(object_sha256=digest, changed_object_sha256=changed, restored_cache_hits=hits,
                          changed_source_cache_misses=misses)
        report['result'] = 'PASS'
        log(f'{args.phase} passed for a real Chromium object', component='compiler-cache-probe')
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        report['error_type'] = type(error).__name__
        if isinstance(error, RuntimeError):
            report['diagnostic'] = str(error)
        raise
    finally:
        try:
            if wrapper is not None:
                subprocess.run([str(wrapper), '--stop-server'], env=environment, timeout=20, check=False,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        finally:
            write_json(output / 'report.json', report)


if __name__ == '__main__':
    main()
