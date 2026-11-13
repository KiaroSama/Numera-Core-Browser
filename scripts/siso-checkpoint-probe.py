#!/usr/bin/env python3
"""Two-job, tiny C fixture for checkpoint research; never an engine build."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import urllib.request
import zipfile

from logging_util import log

REVISION = 'b521b230cb7c56c821235e498285cf8e548a73ee'
ZIP_SHA = '3eec3371462bbc77e5f02edb41524b34d8ab3c2e020af74dcf5ae85e732bd928'
BINARY_SHA = '8df1ad4f15ad38ba5232bb91d54426ab84893ce9e28fd63fe64bf44fb1cb8036'
TOOLCHAIN_IMAGE = 'python@sha256:934f4289019d6c3dc94d5e6341ff3a70c82f19bb118756eb03bdd700fe4b5c98'
DELIVERY = {'complete-source.zip', 'LICENSE'}
STATE = {'a.o', '.siso_deps', '.siso_fs_state', '.siso_config', '.siso_filegroups', '.ninja_log'} | DELIVERY


def digest(data):
    return hashlib.sha256(data).hexdigest()


def inventory(root, names):
    return {name: {'sha256': digest((root / name).read_bytes()), 'mtime_ns': (root / name).stat().st_mtime_ns}
            for name in sorted(names)}


def matches(root, expected):
    return all((root / name).is_file() and digest((root / name).read_bytes()) == item['sha256']
               for name, item in expected.items())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('phase', choices=['seed', 'resume'])
    parser.add_argument('--checkpoint', type=Path)
    parser.add_argument('--expected-sha')
    args = parser.parse_args()
    output = Path('build/siso-probe').resolve()
    output.mkdir(parents=True, exist_ok=False)
    root = output / 'workspace'
    root.mkdir()
    environment = {k: v for k, v in os.environ.items() if k not in {'GH_TOKEN', 'GITHUB_TOKEN'}}
    url = 'https://chrome-infra-packages.appspot.com/dl/build/siso/linux-amd64/+/git_revision:' + REVISION
    with urllib.request.urlopen(url, timeout=30) as response:
        data = response.read(40 * 1024 * 1024 + 1)
    if digest(data) != ZIP_SHA:
        raise RuntimeError('Pinned Siso transport digest mismatch')
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        binary = archive.read('siso')
    if digest(binary) != BINARY_SHA:
        raise RuntimeError('Pinned Siso executable digest mismatch')
    tool = output / 'siso'
    tool.write_bytes(binary)
    tool.chmod(0o700)
    compiler = Path(shutil.which('cc')).resolve(strict=True)
    sources = {'a.c': '#include "value.h"\nint answer(void) { return VALUE; }\n',
               'main.c': 'int answer(void);\nint main(void) { return answer(); }\n',
               'value.h': '#define VALUE 7\n',
               'build.ninja': f'rule cc\n  command = {compiler} -MMD -MF $out.d -c $in -o $out\n  depfile = $out.d\n  deps = gcc\nrule link\n  command = {compiler} $in -o $out\nbuild a.o: cc a.c\nbuild main.o: cc main.c\nbuild sample: link a.o main.o\ndefault sample\n'}
    for name, text in sources.items():
        (root / name).write_text(text, encoding='utf-8')
    container = os.environ.get('PROBE_TOOLCHAIN_IMAGE')
    if container is not None and container != TOOLCHAIN_IMAGE:
        raise RuntimeError('Unexpected research toolchain image')
    subprocess.run(['git', 'archive', '--format=zip', '--output', str(root / 'complete-source.zip'), 'HEAD'], check=True)
    with zipfile.ZipFile(root / 'complete-source.zip') as archive:
        if archive.read('scripts/siso-checkpoint-probe.py') != Path(__file__).read_bytes():
            raise RuntimeError('Research script must match the complete committed source')
        if any(name.split('/')[0] in {'.ai', '.codex', '.agents'} for name in archive.namelist()):
            raise RuntimeError('Private assistant files cannot be distributed')
        (root / 'LICENSE').write_bytes(archive.read('LICENSE'))
    context = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
               'probe_script_sha256': digest(Path(__file__).read_bytes()),
               'run_id': os.environ.get('GITHUB_RUN_ID'), 'run_attempt': os.environ.get('GITHUB_RUN_ATTEMPT'),
               'runner_image': os.environ.get('ImageVersion') if container is None else None,
               'toolchain_container': container, 'workspace': str(root),
               'siso_sha256': BINARY_SHA, 'compiler': str(compiler),
               'compiler_sha256': digest(compiler.read_bytes())}
    (output / 'context.json').write_text(json.dumps(context, indent=2) + '\n', encoding='utf-8')
    results = []

    def build(label, target, expected_steps, expected_exit=None):
        command = [str(tool), 'ninja', '--offline', '--local_jobs=2', '--cache_dir=' + str(output / 'cache'),
                   '-d', 'explain', '-C', str(root), target]
        run = subprocess.run(command, capture_output=True, text=True, encoding='utf-8',
                             timeout=30, env=environment)
        native_log = run.stdout + run.stderr
        (output / (label + '.log')).write_text(native_log, encoding='utf-8')
        steps = re.search(r'Build Succeeded: (\d+) steps', native_log)
        count = int(steps[1]) if steps else None
        if run.returncode != 0 or count != expected_steps:
            raise RuntimeError('Unexpected native result for ' + label)
        if expected_exit is not None:
            sample = subprocess.run([str(root / 'sample')], env=environment, timeout=5)
            if sample.returncode != expected_exit:
                raise RuntimeError('Wrong compiled sample result for ' + label)
        results.append({'case': label, 'steps': count, 'program_exit': expected_exit, 'result': 'PASS'})

    if args.phase == 'seed':
        build('cold-partial', 'a.o', 1)
        manifest = {'context': context, 'inputs': inventory(root, sources), 'outputs': inventory(root, STATE)}
        (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        with tarfile.open(output / 'checkpoint.tar.gz', 'x:gz') as archive:
            for name in sorted(STATE):
                archive.add(root / name, arcname=name)
            archive.add(output / 'manifest.json', arcname='manifest.json')
        results.append({'case': 'checkpoint-size', 'bytes': (output / 'checkpoint.tar.gz').stat().st_size})
        if os.environ.get('GITHUB_OUTPUT'):
            with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as result:
                result.write('checkpoint_sha=' + digest((output / 'checkpoint.tar.gz').read_bytes()) + '\n')
    else:
        if args.checkpoint is None or args.checkpoint.stat().st_size > 1024 * 1024:
            raise RuntimeError('Select a bounded checkpoint from this run')
        if not args.expected_sha or digest(args.checkpoint.read_bytes()) != args.expected_sha:
            raise RuntimeError('Checkpoint does not match the seed job output digest')
        with tarfile.open(args.checkpoint, 'r:gz') as archive:
            members = archive.getmembers()
            if {m.name for m in members} != STATE | {'manifest.json'} or len(members) != len(STATE) + 1:
                raise RuntimeError('Unexpected checkpoint members')
            if any(not m.isfile() or m.size > 1024 * 1024 for m in members):
                raise RuntimeError('Unsafe checkpoint member')
            manifest = json.loads(archive.extractfile('manifest.json').read())
            changed = sorted(key for key in manifest['context'].keys() | context.keys()
                             if key not in manifest['context'] or key not in context
                             or manifest['context'][key] != context[key])
            if changed or set(manifest['inputs']) != set(sources) or set(manifest['outputs']) != STATE:
                raise RuntimeError('Checkpoint provenance/context changed; resume is blocked: ' + ', '.join(changed))
            if not matches(root, manifest['inputs']):
                raise RuntimeError('Fresh source input changed; resume is blocked')
            if not matches(root, {name: manifest['outputs'][name] for name in DELIVERY}):
                raise RuntimeError('Checkpoint matching-source delivery changed')
            for name in STATE:
                payload = archive.extractfile(name).read()
                if digest(payload) != manifest['outputs'][name]['sha256']:
                    raise RuntimeError('Checkpoint output digest mismatch')
                (root / name).write_bytes(payload)
        for name, item in (manifest['inputs'] | manifest['outputs']).items():
            os.utime(root / name, ns=(item['mtime_ns'], item['mtime_ns']))
        build('restored-partial-checkpoint', 'sample', 2, 7)
        build('unchanged', 'sample', 0, 7)
        # A changed depfile header must rebuild a.o and the link, not main.o.
        (root / 'value.h').write_text('#define VALUE 8\n', encoding='utf-8')
        os.utime(root / 'value.h', ns=(0, (root / 'a.o').stat().st_mtime_ns + 1_000_000_000))
        build('header-invalidation', 'sample', 2, 8)
        # Content guards must reject corruption even when size/mtime are forged.
        expected = inventory(root, {'value.h', 'a.o'})
        for name in expected:
            path = root / name
            original = path.read_bytes()
            changed = bytearray(original)
            changed[-2] ^= 1
            path.write_bytes(changed)
            os.utime(path, ns=(expected[name]['mtime_ns'], expected[name]['mtime_ns']))
            if matches(root, expected):
                raise RuntimeError('Content guard accepted a corrupted input/output')
            path.write_bytes(original)
            os.utime(path, ns=(expected[name]['mtime_ns'], expected[name]['mtime_ns']))
            results.append({'case': 'same-size-mtime-corruption-' + name, 'result': 'REJECTED'})
    report = {'schema_revision': 1, 'phase': args.phase, 'context': context, 'results': results,
              'execution_context': 'github-actions' if os.environ.get('GITHUB_ACTIONS') == 'true' else 'local-simulation',
              'job': os.environ.get('GITHUB_JOB'), 'host_runner_image': os.environ.get('ImageVersion'),
              'checkpoint_sha256': digest((args.checkpoint if args.phase == 'resume' else output / 'checkpoint.tar.gz').read_bytes()),
              'engine_source_acceptance': 'NOT_RUN',
              'limits': 'Tiny authored Linux C fixture only. Full Chromium outputs, transitive toolchain/SDK provenance, Windows resume, storage allowance and final-link fit remain unproven.'}
    (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    log('PASS: tiny ' + args.phase + ' checkpoint fixture; engine acceptance remains unproven')


if __name__ == '__main__':
    main()
