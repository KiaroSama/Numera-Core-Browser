#!/usr/bin/env python3
"""Stage a tested development CLI, notices and matching complete first-party source."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

from logging_util import log


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as file:
        for block in iter(lambda: file.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--target', required=True)
    args = parser.parse_args()
    repo = Path.cwd().resolve()
    binary = Path(args.binary).resolve(strict=True)
    if not binary.is_file() or not binary.is_relative_to(repo / 'target'):
        raise RuntimeError('Select a compiled workspace CLI')
    subprocess.run(['git', 'diff', '--exit-code', 'HEAD', '--'], check=True, capture_output=True)
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True, encoding='utf-8').strip()
    if os.environ.get('GITHUB_SHA', commit) != commit:
        raise RuntimeError('Package source does not match the workflow revision')
    if any(line.startswith(b'160000 ') for line in subprocess.check_output(['git', 'ls-files', '--stage', '-z']).split(b'\0')):
        raise RuntimeError('Complete source packaging does not support Git submodules')
    output = Path(args.output).resolve()
    if not output.is_relative_to(repo / 'build'):
        raise RuntimeError('Package output must be a new workspace build directory')
    output.mkdir(parents=True, exist_ok=False)
    staged = output / binary.name
    shutil.copy2(binary, staged)
    source = output / 'complete-source.zip'
    subprocess.run(['git', 'archive', '--format=zip', '--output', str(source), commit], check=True, capture_output=True)
    tracked = set(subprocess.check_output(['git', 'ls-tree', '-rz', '--name-only', commit]).decode('utf-8').rstrip('\0').split('\0'))
    if any(path.split('/')[0] in {'.ai', '.codex', '.agents'} for path in tracked):
        raise RuntimeError('Private assistant files cannot be distributed')
    with zipfile.ZipFile(source) as archive:
        archived = {item.filename for item in archive.infolist() if not item.is_dir()}
        if archived != tracked:
            raise RuntimeError('Source archive omitted or added tracked files')
        # Read notices/contracts from exact HEAD, never untracked/ignored files
        # beside them in a developer checkout or generated build workspace.
        for name in sorted(tracked):
            if name not in {'LICENSE', 'THIRD_PARTY_NOTICES', 'Cargo.lock', 'licenses/dependencies.json'} and not name.startswith(('contracts/', 'licenses/dependencies/')):
                continue
            path = PurePosixPath(name)
            if path.is_absolute() or '..' in path.parts or '\\' in name or ':' in name:
                raise RuntimeError('Unsafe package data path')
            information = archive.getinfo(name)
            if information.external_attr >> 16 & 0o170000 == 0o120000:
                raise RuntimeError('Runtime notices/contracts cannot be symlinks')
            target = output.joinpath(*path.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(archive.read(name))
    # Run from the staged directory with no client/toolchain PATH. This proves
    # native execution here, not clean-machine CRT/notarization acceptance.
    environment = dict(os.environ, PATH='')
    for key in ['DISPLAY', 'WAYLAND_DISPLAY', 'LD_LIBRARY_PATH', 'DYLD_LIBRARY_PATH']:
        environment.pop(key, None)
    result = subprocess.run([str(staged), '--json', 'doctor'], cwd=output,
                            env=environment, capture_output=True, timeout=15)
    if result.returncode != 0:
        raise RuntimeError('Staged native CLI diagnostics failed')
    doctor = json.loads(result.stdout)
    if doctor.get('owned_engine_available') is not False or doctor.get('python_required') is not False or doctor.get('node_required') is not False:
        raise RuntimeError('Development package reported unexpected runtime capabilities')
    version = json.loads(subprocess.check_output([str(staged), '--version'], cwd=output, timeout=15))
    manifest = {'schema_revision': 1, 'product': 'Numera Core Browser development CLI',
        'channel': 'development-control-plane', 'version': version['version'],
        'source_commit': commit, 'runner_target': args.target, 'native_system': platform.system(),
        'native_machine': platform.machine(), 'executable': staged.name, 'executable_sha256': sha256(staged),
        'complete_source': {'file': source.name, 'sha256': sha256(source),
            'scope': 'Complete first-party repository tree; independent third-party source locations and notices are in licenses/dependencies.json'},
        'engine_ready': False, 'browser_gui': False, 'archive_signature': 'none',
        'publisher_signing': 'NOT_CONFIGURED', 'platform_signing': 'NOT_EVALUATED',
        'clean_machine_installation': 'NOT_RUN', 'native_staged_doctor': 'PASS'}
    (output / 'development-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    (output / 'README.txt').write_text(
        'Numera Core Browser — development control-plane CLI\n\n'
        'Run ./numera-core --json doctor (PowerShell: .\\numera-core.exe --json doctor) from this directory.\n'
        'The CLI provides protocol diagnostics and blocked planning. No browser engine or GUI is included.\n'
        'This is unsigned development software, not an integration-ready or production browser release.\n'
        'Native execution was tested on the named CI target; clean-machine OS/CRT prerequisites are not certified.\n'
        'Complete matching first-party source is in complete-source.zip. Read its README for pinned build instructions.\n'
        'LICENSE and third-party notices apply; commercial use is not granted by the custom project license.\n',
        encoding='utf-8')
    # Artifact transport normalizes loose-file permissions. A tar member keeps
    # the executable mode, and its actual extracted payload is tested here.
    distributable = output.with_name(output.name + '.tar.gz')
    with tarfile.open(distributable, 'x:gz') as archive:
        archive.add(output, arcname='development-cli')
    with tempfile.TemporaryDirectory(prefix='Numera CLI فارسی ') as directory:
        extracted = Path(directory)
        with tarfile.open(distributable, 'r:gz') as archive:
            archive.extractall(extracted, filter='data')
        executable = extracted / 'development-cli' / binary.name
        result = subprocess.run([str(executable), '--json', 'doctor'], cwd=executable.parent,
                                env=environment, capture_output=True, timeout=15)
        if result.returncode != 0 or json.loads(result.stdout).get('owned_engine_available') is not False:
            raise RuntimeError('Extracted development archive did not execute natively')
        if sha256(executable) != manifest['executable_sha256']:
            raise RuntimeError('Extracted executable hash does not match the package')
    log('PASS: extracted native development CLI with matching source/notices; browser readiness remains false')


if __name__ == '__main__':
    main()
