#!/usr/bin/env python3
"""Check only against an explicitly supplied exact upstream source file."""
import argparse
from logging_util import log
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("--upstream-file", required=True)
args = parser.parse_args()
lock = json.loads(Path("upstream/chromium/lock.json").read_text())
patch = lock["patches"][0]
patch_path = Path(patch["path"]).resolve()
assert hashlib.sha256(patch_path.read_bytes()).hexdigest() == patch["sha256"]
source_bytes = Path(args.upstream_file).read_bytes()
assert hashlib.sha256(source_bytes).hexdigest() in {pin['branding_source_sha256'] for pin in lock['platform_pins'].values()}, "Supplied source file is not the pinned upstream fixture"
with tempfile.TemporaryDirectory(prefix="numera-patch-") as directory:
    root = Path(directory)
    target = root / "chrome/app/chromium_strings.grd"
    target.parent.mkdir(parents=True)
    target.write_bytes(source_bytes)
    subprocess.run(["git", "apply", "--check", str(patch_path)], cwd=root, check=True)
    subprocess.run(["git", "apply", str(patch_path)], cwd=root, check=True)
    text = target.read_text()
    assert text.count("            Numera Core Browser\n") == 2
    assert "Google Chrome for Testing" in text
log("PASS: patch applies to supplied pinned source; browser compilation remains NOT_RUN")
