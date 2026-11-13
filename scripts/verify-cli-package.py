#!/usr/bin/env python3
"""Exercise development packaging with deliberately untracked local data."""
import argparse
import io
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import zipfile

from logging_util import log

parser = argparse.ArgumentParser()
parser.add_argument('--binary', required=True)
parser.add_argument('--output', required=True)
parser.add_argument('--target', required=True)
args = parser.parse_args()
created = []
try:
    for directory, suffix in [(Path('contracts'), '.txt'), (Path('licenses/dependencies'), '.tmp')]:
        descriptor, name = tempfile.mkstemp(prefix='.local-only-package-', suffix=suffix, dir=directory)
        created.append(Path(name).resolve().relative_to(Path.cwd().resolve()))
        with os.fdopen(descriptor, 'w', encoding='utf-8') as file:
            file.write('Synthetic local-only packaging canary; not a credential.\n')
    subprocess.run([sys.executable, 'scripts/package-cli.py', '--binary', args.binary,
                    '--output', args.output, '--target', args.target], check=True)
    output = Path(args.output).resolve()
    with tarfile.open(output.with_name(output.name + '.tar.gz'), 'r:gz') as archive:
        names = set(archive.getnames())
        if any('development-cli/' + path.as_posix() in names for path in created):
            raise RuntimeError('Local-only files entered the runtime bundle')
        with zipfile.ZipFile(io.BytesIO(archive.extractfile('development-cli/complete-source.zip').read())) as source:
            if any(path.as_posix() in source.namelist() for path in created):
                raise RuntimeError('Local-only files entered the source bundle')
    log('PASS: untracked local data excluded from runtime and matching source')
finally:
    for path in created:
        path.unlink()
