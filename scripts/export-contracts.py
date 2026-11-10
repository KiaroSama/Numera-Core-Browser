#!/usr/bin/env python3
import argparse
from logging_util import log
import hashlib
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--binary", required=True)
parser.add_argument("--check", action="store_true")
args = parser.parse_args()
root = Path("contracts/v1")
root.mkdir(parents=True, exist_ok=True)
hashes = {}
for kind in ("request", "response", "manifest"):
    result = subprocess.run([args.binary, "schema", kind], check=True, capture_output=True, text=True, timeout=10)
    text = json.dumps(json.loads(result.stdout), sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    path = root / f"{kind}.schema.json"
    if args.check:
        assert path.read_text(encoding="utf-8") == text, f"Canonical {kind} schema drift"
    else:
        path.write_text(text, encoding="utf-8")
    hashes[path.name] = hashlib.sha256(text.encode()).hexdigest()
for path in sorted([*Path('contracts/fixtures').glob('*'), root/'theme.tokens.json', root/'compatibility.json']):
    hashes[path.relative_to(Path('contracts')).as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
manifest = json.dumps({"artifact": "numera-core-contracts-v1", "wire_major": 1, "schema_revision": 1,
    "status": "development; not an integration-ready release", "sha256": hashes}, sort_keys=True, indent=2) + "\n"
path = root / "manifest.json"
if args.check:
    assert path.read_text(encoding="utf-8") == manifest, "Contract manifest drift"
else:
    path.write_text(manifest, encoding="utf-8")
log("PASS: canonical schemas match Rust producer types")
