"""Manager-side NCB-1 client for a locally provisioned Numera Core executable."""

from contextlib import contextmanager
from dataclasses import dataclass, field
import errno
import json
import math
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import threading
import time
from types import MappingProxyType


NAMESPACE = "numera.core"
MAX_FRAME = 1_048_576  # Including LF
MAX_REQUESTS = 1024
MAX_DEADLINE_MS = 30_000
POLL_SECONDS = 0.005
IDENTIFIER = re.compile(r"[A-Za-z0-9._-]{1,128}")
DIGEST = re.compile(r"[a-f0-9]{64}")
ERROR_CODES = frozenset({
    "protocol_mismatch", "unsupported_capability", "incompatible_engine_data_format",
    "missing_package", "ownership_conflict", "admission_timeout", "authentication_failure",
    "proxy_failure", "invalid_configuration", "update_signature_failure",
    "insufficient_resources", "launch_failure", "cancellation", "recovery_required",
})
SERVICE_ENVIRONMENT = (
    "HOME", "LANG", "LC_ALL", "LC_CTYPE", "DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY",
    "XDG_RUNTIME_DIR", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "DBUS_SESSION_BUS_ADDRESS",
    "SystemRoot", "WINDIR", "SYSTEMDRIVE", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "PROGRAMDATA",
)


class CoreRequestError(RuntimeError):
    """A known request rejection. The connection can still be used."""

    def __init__(self, code):
        self.code = code
        super().__init__(f"NCB-1 request failed ({code})")


class CoreTransportError(RuntimeError):
    """A connection failure. The request outcome may be unknown."""


def _encode(value):
    try:
        return json.dumps(value, ensure_ascii=False, allow_nan=False,
                          separators=(",", ":")).encode("utf-8")
    except (TypeError, ValueError, RecursionError):
        raise CoreRequestError("invalid_configuration") from None


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("Duplicate JSON key")
        result[key] = value
    return result


def _finite_number(value):
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("Non-finite JSON number")
    return number


def _check_json(value, depth=0):
    if depth >= 128:
        raise ValueError("JSON nesting exceeds the NCB-1 limit")
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str):
                raise ValueError("JSON object keys must be strings")
            key.encode("utf-8")
            _check_json(item, depth + 1)
    elif isinstance(value, (list, tuple)):
        for item in value:
            _check_json(item, depth + 1)
    elif isinstance(value, str):
        # Python's JSON decoder can accept escaped lone surrogates.
        value.encode("utf-8")


def _freeze(value):
    if isinstance(value, dict):
        return MappingProxyType({key: _freeze(item) for key, item in value.items()})
    if isinstance(value, list):
        return tuple(map(_freeze, value))
    return value


def _matches(pattern, value):
    return isinstance(value, str) and pattern.fullmatch(value) is not None


def _uint(value):
    return type(value) is int and 0 <= value < 2**64


def _error_code(value):
    if (not isinstance(value, dict) or set(value) != {"code", "message"}
            or not isinstance(value["code"], str) or value["code"] not in ERROR_CODES
            or not isinstance(value["message"], str)):
        raise CoreTransportError("Invalid NCB-1 error response")
    return value["code"]


@dataclass(frozen=True, init=False)
class LaunchSnapshot:
    """An immutable copy of a JSON launch snapshot, without a printable payload."""

    _payload: bytes = field(repr=False)

    def __init__(self, payload):
        if not isinstance(payload, dict):
            raise CoreRequestError("invalid_configuration")
        try:
            _check_json(payload)
        except ValueError:
            raise CoreRequestError("invalid_configuration") from None
        encoded = _encode(payload)
        if len(encoded) >= MAX_FRAME:
            raise CoreRequestError("insufficient_resources")
        object.__setattr__(self, "_payload", encoded)


class CoreClient:
    """Own one inherited stdio connection and its standalone sessions."""

    def __init__(self, binary, store, data_root, *, deadline_ms=MAX_DEADLINE_MS):
        deadline = self._deadline(deadline_ms)
        paths = [Path(value) for value in (binary, store, data_root)]
        if any(not path.is_absolute() for path in paths):
            raise CoreRequestError("invalid_configuration")
        if sys.platform == "win32" and sys.version_info < (3, 12):
            raise CoreTransportError("NCB-1 pipe deadlines on Windows require Python 3.12 or later")
        self._lock = threading.Lock()
        self._closed = False
        self._reaper = None
        self._requests = 0
        environment = {name: os.environ[name] for name in SERVICE_ENVIRONMENT if name in os.environ}
        creation_flags = subprocess.CREATE_NEW_PROCESS_GROUP if sys.platform == "win32" else 0
        try:
            self._process = subprocess.Popen(
                [str(paths[0]), "serve", str(paths[1]), str(paths[2])],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0,
                env=environment, start_new_session=(os.name == "posix"), creationflags=creation_flags,
            )
        except (OSError, ValueError):
            raise CoreTransportError("Cannot start the configured Numera Core executable") from None
        try:
            try:
                self._input = self._process.stdin.fileno()
                self._output = self._process.stdout.fileno()
                os.set_blocking(self._input, False)
                os.set_blocking(self._output, False)
            except OSError:
                raise CoreTransportError("Cannot configure bounded NCB-1 pipes") from None
            versions = {"min": 1, "max": 1}
            hello = self._request("hello", {"protocols": versions, "schemas": versions}, deadline)
            self._check_version(hello)
            if hello.get("product") != "Numera Core Browser":
                raise CoreTransportError("Unexpected NCB-1 producer identity")
            report = self._request("capabilities.get", {}, deadline)
            self._check_capabilities(report, hello)
            self._capabilities = _freeze(report)
        except BaseException:
            self._close_pipes()
            self._reap()
            raise

    @property
    def capabilities(self):
        return self._capabilities

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        try:
            status = self.close()
        except CoreTransportError:
            if kind is None:
                raise
        else:
            if status != 0 and kind is None:
                raise CoreTransportError("Numera Core exited with a connection error")

    def _close_pipes(self):
        if not self._closed:
            self._closed = True
            # EOF lets Core supervise and stop its workers. Closing the output
            # end also releases a producer blocked on an unread response.
            self._process.stdin.close()
            self._process.stdout.close()

    def _reap(self):
        if self._reaper is None and self._process.poll() is None:
            self._reaper = threading.Thread(target=self._process.wait, daemon=True,
                                            name="numera-core-reap")
            self._reaper.start()

    def close(self):
        """Send EOF, wait up to eight seconds and return Core's exit status."""
        with self._exclusive():
            self._close_pipes()
            try:
                return self._process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self._reap()
                raise CoreTransportError("Numera Core did not finish closing within the cleanup budget") from None
            except BaseException:
                self._reap()
                raise

    @contextmanager
    def _exclusive(self):
        if not self._lock.acquire(blocking=False):
            raise CoreRequestError("insufficient_resources")
        try:
            yield
        finally:
            self._lock.release()

    @contextmanager
    def _operation(self, deadline_ms):
        deadline = self._deadline(deadline_ms)
        with self._exclusive():
            if self._closed:
                raise CoreTransportError("NCB-1 connection is closed")
            try:
                yield deadline
            except (CoreTransportError, KeyboardInterrupt):
                self._close_pipes()
                self._reap()
                raise

    @staticmethod
    def _deadline(milliseconds):
        if type(milliseconds) is not int or not 1 <= milliseconds <= MAX_DEADLINE_MS:
            raise CoreRequestError("invalid_configuration")
        return time.monotonic() + milliseconds / 1000

    @staticmethod
    def _remaining(deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise CoreTransportError("NCB-1 request deadline expired")
        return remaining

    def _write(self, frame, deadline):
        pending = memoryview(frame)
        while pending:
            remaining = self._remaining(deadline)
            try:
                count = os.write(self._input, pending)
            except BlockingIOError:
                count = 0
            except OSError as error:
                # Windows CRT reports a full nonblocking pipe as ENOSPC.
                if sys.platform != "win32" or error.errno != errno.ENOSPC:
                    raise
                count = 0
            if count:
                pending = pending[count:]
            else:
                time.sleep(min(remaining, POLL_SECONDS))

    def _read(self, deadline):
        frame = bytearray()
        while len(frame) < MAX_FRAME:
            remaining = self._remaining(deadline)
            try:
                chunk = os.read(self._output, min(65_536, MAX_FRAME - len(frame)))
            except BlockingIOError:
                time.sleep(min(remaining, POLL_SECONDS))
                continue
            if not chunk:
                raise CoreTransportError("NCB-1 closed before a complete response")
            frame.extend(chunk)
            if b"\n" in chunk:
                if chunk.count(b"\n") != 1 or not chunk.endswith(b"\n"):
                    raise CoreTransportError("Unexpected additional NCB-1 response data")
                return frame
        raise CoreTransportError("NCB-1 response exceeds the frame limit")

    def _request(self, operation, payload, deadline):
        if self._requests == MAX_REQUESTS:
            raise CoreRequestError("insufficient_resources")
        identifier = f"manager-{self._requests + 1}"
        request = _encode({"namespace": NAMESPACE, "protocol": 1, "id": identifier,
                           "deadline_ms": min(MAX_DEADLINE_MS, math.ceil(self._remaining(deadline) * 1000)),
                           "operation": operation, "payload": payload}) + b"\n"
        if len(request) > MAX_FRAME:
            raise CoreRequestError("insufficient_resources")
        self._requests += 1
        try:
            self._write(request, deadline)
            frame = self._read(deadline)
            response = json.loads(frame.decode("utf-8"), object_pairs_hook=_unique_object,
                                  parse_float=_finite_number, parse_constant=_finite_number)
            _check_json(response)
        except (OSError, ValueError, RecursionError):
            raise CoreTransportError("Invalid or unavailable NCB-1 response") from None
        self._remaining(deadline)
        if (not isinstance(response, dict) or response.get("namespace") != NAMESPACE
                or type(response.get("protocol")) is not int or response["protocol"] != 1
                or response.get("id") != identifier):
            raise CoreTransportError("NCB-1 response does not correspond to the request")
        envelope = {"namespace", "protocol", "id", "status"}
        if response.get("status") == "error" and set(response) == envelope | {"error"}:
            raise CoreRequestError(_error_code(response["error"]))
        if (response.get("status") == "ok" and set(response) == envelope | {"result"}
                and isinstance(response["result"], dict)):
            return response["result"]
        raise CoreTransportError("Invalid NCB-1 response envelope")

    @staticmethod
    def _check_version(report):
        if any(type(report.get(key)) is not int or report[key] != 1 for key in ("protocol", "schema_revision")):
            raise CoreTransportError("NCB-1 wire or schema version was not negotiated")

    def _check_capabilities(self, report, hello):
        self._check_version(report)
        os_name = "windows" if sys.platform == "win32" else sys.platform
        arch = platform.machine().lower().replace("amd64", "x86_64")
        scope, features = report.get("scope"), report.get("features")
        if (report.get("namespace") != NAMESPACE or report.get("host") != {"os": os_name, "arch": arch}
                or not isinstance(hello.get("core_version"), str) or report.get("core_version") != hello["core_version"]
                or not isinstance(scope, dict) or set(scope) != {"engine_build", "device_mode", "automation_transport"}
                or (scope["engine_build"] is not None and not _matches(DIGEST, scope["engine_build"]))
                or not isinstance(features, dict)
                or any(not isinstance(feature, dict) or feature.get("status") not in (
                    "supported", "partial", "unsupported") for feature in features.values())
                or features.get("protocol.ndjson", {}).get("status") != "supported"):
            raise CoreTransportError("Invalid NCB-1 capability scope or transport")

    def _support(self, capability):
        return self.capabilities["features"].get(capability, {}).get("status")

    @staticmethod
    def _payload(snapshot):
        if not isinstance(snapshot, LaunchSnapshot):
            raise CoreRequestError("invalid_configuration")
        return json.loads(snapshot._payload.decode("utf-8"))

    def _plan(self, payload, deadline):
        result = self._request("launch.plan", payload, deadline)
        if (any(result.get(field) != payload.get(field) for field in ("launch_id", "profile_id", "device_mode"))
                or result.get("requested_engine") != payload.get("engine")
                or result.get("status") not in ("planned", "blocked")
                or not isinstance(result.get("blockers"), list)
                or (result["status"] == "blocked") != bool(result["blockers"])):
            raise CoreTransportError("Invalid NCB-1 launch plan")
        for blocker in result["blockers"]:
            _error_code(blocker)
        return result

    def plan(self, snapshot, *, deadline_ms=MAX_DEADLINE_MS):
        with self._operation(deadline_ms) as deadline:
            return _freeze(self._plan(self._payload(snapshot), deadline))

    def launch(self, snapshot, *, deadline_ms=MAX_DEADLINE_MS):
        """Plan and launch one immutable, digest-pinned standalone snapshot."""
        with self._operation(deadline_ms) as deadline:
            payload = self._payload(snapshot)
            policy = payload.get("build_policy")
            if (not isinstance(policy, dict) or policy.get("kind") != "pinned"
                    or not _matches(DIGEST, policy.get("sha256"))):
                raise CoreRequestError("invalid_configuration")
            required = payload.get("required_capabilities")
            if not isinstance(required, list) or any(not isinstance(name, str) for name in required):
                raise CoreRequestError("invalid_configuration")
            if (payload.get("lifetime") != "standalone" or payload.get("device_mode") != "desktop"
                    or payload.get("engine") != "chromium"):
                raise CoreRequestError("unsupported_capability")
            plan = self._plan(payload, deadline)
            if plan["blockers"]:
                raise CoreRequestError(_error_code(plan["blockers"][0]))
            digest = policy["sha256"]
            selected = plan.get("selected_build")
            if not isinstance(selected, dict) or selected.get("sha256") != digest:
                raise CoreTransportError("NCB-1 launch plan selected a different package")
            scope = {"engine_build": digest, "device_mode": payload.get("device_mode"),
                     "automation_transport": None}
            if (scope != self.capabilities["scope"] or self._support("session.launch") not in ("partial", "supported")
                    or any(self._support(name) != "supported" for name in required)):
                raise CoreRequestError("unsupported_capability")
            result = self._view(self._request("session.launch", payload, deadline), payload["launch_id"])
            if (result["profile_id"] != payload["profile_id"] or result["package_sha256"] != digest
                    or result["device_mode"] != payload["device_mode"] or not result["browser_process_started"]):
                raise CoreTransportError("NCB-1 launch returned a different session scope")
            return result

    @staticmethod
    def _view(result, launch_id):
        if (not isinstance(result, dict) or not _matches(IDENTIFIER, result.get("launch_id"))
                or (launch_id is not None and result["launch_id"] != launch_id)
                or not _matches(IDENTIFIER, result.get("profile_id"))
                or not _matches(DIGEST, result.get("package_sha256"))
                or result.get("device_mode") != "desktop"
                or result.get("state") not in ("running", "stopped", "recovery_required")
                or not _uint(result.get("sequence")) or result["sequence"] == 0
                or type(result.get("browser_process_started")) is not bool
                or not isinstance(result.get("automation_endpoints"), list)
                or any(not isinstance(item, str) for item in result["automation_endpoints"])):
            raise CoreTransportError("Invalid or unrelated NCB-1 session response")
        return _freeze(result)

    def _session_request(self, operation, payload, deadline):
        if self._support(operation) != "supported":
            raise CoreRequestError("unsupported_capability")
        if "launch_id" in payload and not _matches(IDENTIFIER, payload["launch_id"]):
            raise CoreRequestError("invalid_configuration")
        return self._request(operation, payload, deadline)

    def _session(self, operation, launch_id, deadline_ms):
        with self._operation(deadline_ms) as deadline:
            result = self._view(self._session_request(operation, {"launch_id": launch_id}, deadline), launch_id)
            if operation == "session.stop" and result["state"] != "stopped":
                raise CoreTransportError("NCB-1 stop did not confirm session cleanup")
            return result

    def attach(self, launch_id, *, deadline_ms=MAX_DEADLINE_MS):
        return self._session("session.attach", launch_id, deadline_ms)

    def status(self, launch_id, *, deadline_ms=MAX_DEADLINE_MS):
        return self._session("session.status", launch_id, deadline_ms)

    def stop(self, launch_id, *, deadline_ms=MAX_DEADLINE_MS):
        return self._session("session.stop", launch_id, deadline_ms)

    def list(self, *, deadline_ms=MAX_DEADLINE_MS):
        with self._operation(deadline_ms) as deadline:
            result = self._session_request("session.list", {}, deadline)
            if not isinstance(result.get("sessions"), list):
                raise CoreTransportError("Invalid NCB-1 session list")
            views = tuple(self._view(view, None) for view in result["sessions"])
            if len({view["launch_id"] for view in views}) != len(views):
                raise CoreTransportError("Duplicate NCB-1 session handles")
            return views

    def events(self, launch_id, after_sequence=0, *, deadline_ms=MAX_DEADLINE_MS):
        if not _uint(after_sequence):
            raise CoreRequestError("invalid_configuration")
        with self._operation(deadline_ms) as deadline:
            result = self._session_request("session.events", {"launch_id": launch_id, "after_sequence": after_sequence}, deadline)
            next_sequence = result.get("next_sequence")
            if not _uint(next_sequence) or not isinstance(result.get("events"), list):
                raise CoreTransportError("Invalid NCB-1 session events")
            views = tuple(self._view(view, launch_id) for view in result["events"])
            if (len(views) != next_sequence - after_sequence or any(
                    view["sequence"] != sequence for sequence, view in enumerate(views, after_sequence + 1))):
                raise CoreTransportError("NCB-1 session event sequence has a gap")
            return views, next_sequence
