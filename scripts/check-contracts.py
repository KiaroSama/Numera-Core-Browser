#!/usr/bin/env python3
"""External NCB-1 consumer against the real Rust CLI producer, no mock server."""
import argparse
from logging_util import log
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
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

# Terminal diagnostics must not defeat the protocol's bounded cancellation.
# Fill a real inherited stderr pipe nonblocking, then restore blocking mode:
# the child encounters an actually full pipe, independent of platform capacity.
reader, writer = os.pipe()
terminal = None
try:
    os.set_blocking(writer, False)
    block = b'x' * 4096
    filled = 0
    while True:
        try:
            written = os.write(writer, block)
        except BlockingIOError:
            written = 0
        if written == 0:
            if len(block) == 1:
                break
            block = b'x'
            continue
        filled += written
        assert filled <= 8 * 1024 * 1024, 'Stderr pipe did not reach bounded saturation'
    os.set_blocking(writer, True)
    terminal = subprocess.Popen([args.binary, 'serve'], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=writer)
    try:
        terminal_output, _ = terminal.communicate(b'{}', timeout=5)
    except subprocess.TimeoutExpired:
        terminal.kill()
        terminal.communicate(timeout=5)
        raise AssertionError('A full stderr pipe blocked terminal protocol shutdown') from None
    assert terminal.returncode == 2
    terminal_frame = json.loads(terminal_output)
    sv.validate(terminal_frame)
    assert terminal_frame['error']['code'] == 'invalid_configuration'
finally:
    if terminal is not None and terminal.poll() is None:
        terminal.kill()
        terminal.wait(timeout=5)
    os.close(writer)
    os.close(reader)

# Linux syscall interposition gives a deterministic pre-open barrier without
# production hooks. Swap the selected regular file only after CLI preflight,
# then require bounded rejection of the new FIFO by all file-input commands.
if sys.platform == 'linux':
    with tempfile.TemporaryDirectory(prefix='Numera-file-race-') as temporary:
        root = Path(temporary)
        library = root / 'open-barrier.so'
        subprocess.run(['cc', '-shared', '-fPIC', '-Wall', '-Wextra', '-Werror',
                        'crates/core-cli/tests/fixtures/open-barrier.c', '-ldl',
                        '-o', str(library)], check=True)
        cases = [
            ('plan', ['plan'], json.dumps(snapshot).encode()),
            ('manifest', ['manifest', 'inspect'], Path('contracts/fixtures/engine-manifest.json').read_bytes()),
            ('hash', ['hash-file'], b'hello'),
        ]
        blocked = []
        for name, command, contents in cases:
            directory = root / name
            directory.mkdir()
            selected = directory / 'selected-input'
            selected.write_bytes(contents)
            baseline = subprocess.run([args.binary, *command, str(selected)],
                                      capture_output=True, timeout=5)
            assert baseline.returncode == 0, f'Regular-file control failed: {name}'
            if name == 'hash':
                assert json.loads(baseline.stdout)['sha256'] == '2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824'
            ready, resume = directory / 'ready', directory / 'resume'
            environment = dict(os.environ, LD_PRELOAD=str(library),
                               NUMERA_TEST_OPEN_TARGET=str(selected),
                               NUMERA_TEST_OPEN_READY=str(ready),
                               NUMERA_TEST_OPEN_RESUME=str(resume))
            raced = subprocess.Popen([args.binary, *command, str(selected)],
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment)
            try:
                deadline = time.monotonic() + 5
                while not ready.exists():
                    assert raced.poll() is None, f'CLI exited before open barrier: {name}'
                    assert time.monotonic() < deadline, f'Open barrier was not reached: {name}'
                    time.sleep(0.01)
                selected.unlink()
                os.mkfifo(selected, 0o600)
                resume.touch()
                try:
                    stdout, _ = raced.communicate(timeout=3)
                except subprocess.TimeoutExpired:
                    raced.kill()
                    raced.communicate(timeout=5)
                    blocked.append(name)
                    continue
                assert raced.returncode == 2, f'Special file was accepted: {name}'
                frame = json.loads(stdout)
                sv.validate(frame)
                assert frame['error']['code'] == 'invalid_configuration'
            finally:
                if raced.poll() is None:
                    raced.kill()
                    raced.communicate(timeout=5)
        assert not blocked, f'File replacement blocked CLI commands: {blocked}'

log("PASS: external consumer/schema fixtures and real producer agree; real-browser integration remains NOT_RUN")
