#!/usr/bin/env python3
"""Seed a development-only mutation corpus from the public wire fixtures."""
import json
from pathlib import Path
import shutil

fixtures = Path('contracts/fixtures')
corpus = Path('fuzz/corpus/wire')
corpus.mkdir(parents=True, exist_ok=True)
for source in fixtures.iterdir():
    if source.suffix in ('.json', '.ndjson'):
        shutil.copyfile(source, corpus / source.name)
hello = json.loads((fixtures / 'hello.json').read_text())
caps = json.loads((fixtures / 'capabilities.json').read_text())
snapshot = json.loads((fixtures / 'launch-desktop.json').read_text())
root = str((Path.cwd() / 'fuzz' / 'never-create-this-profile').resolve())
snapshot['resources'].update(native_profile_dir=root, data_root=root, cache_root=root)
frames = [hello, caps]
for index, operation in enumerate(['launch.plan', 'session.launch']):
    frames.append(dict(hello, id=f'fuzz-seed-{index}', operation=operation, payload=snapshot))
(corpus / 'conversation.ndjson').write_text(''.join(json.dumps(frame) + '\n' for frame in frames))
