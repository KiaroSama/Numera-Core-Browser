#!/usr/bin/env python3
"""Inventory exact Cargo.lock dependencies and preserve shipped license texts."""
import argparse
from logging_util import log
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib

parser = argparse.ArgumentParser()
parser.add_argument("--check", action="store_true")
args = parser.parse_args()
metadata = json.loads(subprocess.run(["cargo", "metadata", "--locked", "--format-version", "1"], check=True, capture_output=True, text=True).stdout)
lock = tomllib.loads(Path("Cargo.lock").read_text())
checksums = {(p["name"], p["version"]): p.get("checksum") for p in lock["package"]}
allowed = {"MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "Unicode-3.0", "ISC", "Zlib", "CC0-1.0", "Unlicense", "0BSD"}
# Reviewed exact-version alternative, not an expansion of accepted copyleft
# licenses. r-efi embeds the complete MIT grant and copyright in AUTHORS.
alternatives = {("r-efi", "6.0.0", "MIT OR Apache-2.0 OR LGPL-2.1-or-later"): "MIT"}
entries = []
for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
    if package["source"] is None:
        continue
    license_expression = package.get("license") or ""
    selected = alternatives.get((package["name"], package["version"], license_expression))
    tokens = set(re.findall(r"[A-Za-z0-9.+-]+", license_expression)) - {"OR", "AND"}
    assert (selected in allowed and selected in license_expression.split(" OR ")) or (tokens and tokens <= allowed), f"Unreviewed dependency license: {package['name']} {license_expression}"
    root = Path(package["manifest_path"]).parent
    texts = []
    for file in sorted(root.iterdir()):
        if file.is_file() and (file.name.upper().startswith(("LICENSE", "COPYING", "NOTICE")) or (selected and file.name == "AUTHORS")):
            data = file.read_bytes()
            target = Path("licenses/dependencies") / f"{package['name']}-{package['version']}" / file.name
            if args.check:
                assert target.read_bytes() == data, f"License notice drift: {target}"
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
            texts.append({"path": target.as_posix(), "sha256": hashlib.sha256(data).hexdigest()})
    assert texts, f"Missing dependency license text: {package['name']}"
    entry = {"name": package["name"], "version": package["version"], "license": license_expression,
        "repository": package["repository"], "source": package["source"],
        "checksum": checksums[(package["name"], package["version"])], "notices": texts}
    if selected:
        entry["selected_license"] = selected
    entries.append(entry)
text = json.dumps({"schema_revision": 1, "scope": "Cargo.lock registry dependencies; engine components are not included",
    "review": "Declared permissive SPDX expressions checked against allowlist; component legal review is still required for any engine release",
    "dependencies": entries}, sort_keys=True, indent=2) + "\n"
target = Path("licenses/dependencies.json")
if args.check:
    assert target.read_text(encoding="utf-8") == text, "Dependency inventory drift"
else:
    target.write_text(text, encoding="utf-8")
log(f"PASS: inventoried {len(entries)} locked dependencies and preserved license texts")
