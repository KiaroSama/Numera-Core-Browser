#!/usr/bin/env python3
"""Audit the separately locked, development-only fuzz graph and its notices."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib

parser = argparse.ArgumentParser()
parser.add_argument('--check', action='store_true')
args = parser.parse_args()
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--manifest-path', 'fuzz/Cargo.toml', '--locked', '--format-version', '1'], text=True))
lock = tomllib.loads(Path('fuzz/Cargo.lock').read_text())
checksums = {(p['name'], p['version']): p.get('checksum') for p in lock['package']}
runtime = {(p['name'], p['version']): p for p in json.loads(Path('licenses/dependencies.json').read_text())['dependencies']}
allowed = {'MIT', 'Apache-2.0', 'BSD-3-Clause', 'BSD-2-Clause', 'Unicode-3.0', 'ISC', 'Zlib', 'CC0-1.0', 'Unlicense', '0BSD'}
entries = []
for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
    if package['source'] is None:
        continue
    key = package['name'], package['version']
    if key in runtime:
        entry = dict(runtime[key])
        assert entry['checksum'] == checksums[key], 'Runtime/fuzz dependency checksum mismatch'
        for notice in entry['notices']:
            assert hashlib.sha256(Path(notice['path']).read_bytes()).hexdigest() == notice['sha256']
        entry['notice_scope'] = 'Reuses exact runtime dependency notices'
    else:
        expression = package.get('license') or ''
        libfuzzer = key == ('libfuzzer-sys', '0.4.13') and expression == '(MIT OR Apache-2.0) AND NCSA'
        tokens = set(re.findall(r'[A-Za-z0-9.+-]+', expression)) - {'OR', 'AND'}
        assert libfuzzer or (tokens and tokens <= allowed), 'Unreviewed fuzz dependency rights'
        texts = []
        source = Path(package['manifest_path']).parent
        for file in sorted(source.iterdir()):
            if file.is_file() and (file.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE')) or (libfuzzer and file.name == 'README.md')):
                data = file.read_bytes()
                target = Path('licenses/fuzz') / f'{key[0]}-{key[1]}' / file.name
                if args.check:
                    assert target.read_bytes() == data, 'Fuzz notice drift'
                else:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(data)
                texts.append({'path': target.as_posix(), 'sha256': hashlib.sha256(data).hexdigest()})
        assert texts, 'Missing fuzz dependency license text'
        entry = {'name': key[0], 'version': key[1], 'license': expression,
            'checksum': checksums[key], 'repository': package['repository'], 'notices': texts}
        if libfuzzer:
            # Cargo/README still declare NCSA, while vendored C++ headers declare
            # Apache-2.0 WITH LLVM-exception. Preserve the authoritative LLVM
            # text including legacy NCSA, alongside both wrapper alternatives.
            notice = Path('licenses/fuzz/libfuzzer-sys-0.4.13/LICENSE-LLVM.txt')
            digest = hashlib.sha256(notice.read_bytes()).hexdigest()
            assert digest == '8d85c1057d742e597985c7d4e6320b015a9139385cff4cbae06ffc0ebe89afee'
            entry['notices'].append({'path': notice.as_posix(), 'sha256': digest})
            entry['file_level_license'] = 'Apache-2.0 WITH LLVM-exception; declared legacy NCSA text also preserved'
            entry['supplement_source'] = 'https://llvm.org/LICENSE.txt'
    entries.append(entry)
text = json.dumps({'scope': 'Development-only fuzz/Cargo.lock; not the installed Core runtime', 'dependencies': entries}, indent=2, sort_keys=True) + '\n'
target = Path('licenses/fuzz/dependencies.json')
if args.check:
    assert target.read_text() == text, 'Fuzz dependency inventory drift'
else:
    target.write_text(text)
print(f'PASS: {len(entries)} separately locked fuzz dependencies and notices')
