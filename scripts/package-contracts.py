#!/usr/bin/env python3
"""Package the versioned canonical contract and test it with the native producer."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

from logging_util import log


ARTIFACT = 'numera-core-contracts-v1'
INCLUDED = ('LICENSE', 'contracts', 'requirements-dev.txt', 'scripts/check-contracts.py',
            'scripts/logging_util.py', 'crates/core-cli/tests/fixtures/open-barrier.c')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, required=True)
    binary = parser.parse_args().binary.resolve(strict=True)
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], encoding='utf-8').strip()
    if os.environ.get('GITHUB_SHA', commit) != commit:
        raise RuntimeError('Contract source does not match the workflow revision')
    subprocess.run(['git', 'diff', '--quiet', 'HEAD', '--', *INCLUDED], check=True)
    inventory = subprocess.check_output(['git', 'ls-tree', '-rz', '--name-only', commit, '--', *INCLUDED],
                                        encoding='utf-8')
    expected = set(inventory.rstrip('\0').split('\0')) | {'source-commit.txt'}
    output = Path('build/contracts').resolve()
    output.mkdir(parents=True, exist_ok=False)
    package = output / f'{ARTIFACT}.zip'
    subprocess.run(['git', 'archive', '--format=zip', f'--prefix={ARTIFACT}/', '--output', str(package),
                    commit, '--', *INCLUDED], check=True)
    with zipfile.ZipFile(package, 'a') as archive:
        archive.writestr(zipfile.ZipInfo(f'{ARTIFACT}/source-commit.txt'), commit + '\n')
    with tempfile.TemporaryDirectory(prefix='Numera NCB-1 فارسی ') as directory:
        with zipfile.ZipFile(package) as archive:
            files = [entry for entry in archive.infolist() if not entry.is_dir()]
            names = {entry.filename for entry in files}
            if (names != {f'{ARTIFACT}/{name}' for name in expected} or len(files) != len(names)
                    or any(entry.external_attr >> 16 & 0o170000 == 0o120000 for entry in files)):
                raise RuntimeError('Contract archive does not contain exactly the expected regular source files')
            archive.extractall(directory, files)
        # A temporary root can be expressed through a symlink or Windows 8.3
        # alias. Compare both sides using the actual extraction directory.
        extracted = (Path(directory) / ARTIFACT).resolve(strict=True)
        contracts = extracted / 'contracts'
        manifest = json.loads((contracts / 'v1/manifest.json').read_text(encoding='utf-8'))
        if manifest['artifact'] != ARTIFACT:
            raise RuntimeError('Contract archive identity differs from the canonical manifest')
        for name, digest in manifest['sha256'].items():
            path = contracts / ('v1' if name.endswith('.schema.json') else '') / name
            if not path.resolve(strict=True).is_relative_to(contracts):
                raise RuntimeError('Canonical manifest path escapes the contract directory')
            if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                raise RuntimeError('Archived contract member does not match its canonical SHA-256')
        subprocess.run([sys.executable, 'scripts/check-contracts.py', '--binary', str(binary)],
                       cwd=extracted, check=True, timeout=120)
    (output / f'{ARTIFACT}.sha256').write_text(
        f'{hashlib.sha256(package.read_bytes()).hexdigest()}  {package.name}\n', encoding='utf-8')
    log('PASS: extracted NCB-1 contract archive passed real native producer conformance')


if __name__ == '__main__':
    main()
