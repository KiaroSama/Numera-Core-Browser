#!/usr/bin/env python3
"""External NCB-1 consumer against the real Rust CLI producer, no mock server."""
import argparse
from logging_util import log
import json
from pathlib import Path
import subprocess
from jsonschema import Draft202012Validator

parser = argparse.ArgumentParser()
parser.add_argument("--binary", required=True)
args = parser.parse_args()
request = json.loads(Path("contracts/v1/request.schema.json").read_text())
response = json.loads(Path("contracts/v1/response.schema.json").read_text())
Draft202012Validator.check_schema(request)
Draft202012Validator.check_schema(response)
rv, sv = Draft202012Validator(request), Draft202012Validator(response)
hello = json.loads(Path("contracts/fixtures/hello.json").read_text())
caps = json.loads(Path("contracts/fixtures/capabilities.json").read_text())
rv.validate(hello)
rv.validate(caps)
bad = json.loads(Path("contracts/fixtures/negative-namespace.json").read_text())
assert not rv.is_valid(bad)
bad_field = dict(hello, unknown_required=True)
assert not rv.is_valid(bad_field)
bad_payload = dict(caps, payload={"unrecognized": True})
assert not rv.is_valid(bad_payload)
snapshot = json.loads(Path("contracts/fixtures/launch-desktop.json").read_text())
# Native path syntax is host-specific. No directory is created by planning.
native_root = str(Path.cwd().resolve() / "build" / "never-create-this-profile")
snapshot["resources"].update(native_profile_dir=native_root, data_root=native_root, cache_root=native_root)
launch = dict(hello, id="launch-1", operation="session.launch", payload=snapshot)
rv.validate(launch)
input_frames = [hello, caps, launch, bad_field, bad_payload, bad]
process = subprocess.run([args.binary, "serve"], input="".join(json.dumps(v) + "\n" for v in input_frames),
                         capture_output=True, text=True, timeout=15)
assert process.returncode == 0 and not process.stderr
frames = [json.loads(s) for s in process.stdout.splitlines()]
assert len(frames) == len(input_frames)
for frame in frames:
    sv.validate(frame)
assert frames[0]["status"] == "ok" and frames[1]["status"] == "ok"
assert frames[2]["error"]["code"] == "missing_package"
assert all(f["status"] == "error" for f in frames[3:])
assert not Path(native_root).exists()
log("PASS: external consumer/schema fixtures and real producer agree; real-browser integration remains NOT_RUN")
