"""Exercise the Python manager transport over inherited, bounded pipes."""

import json
import os
from pathlib import Path
import platform
import subprocess
import sys
from tempfile import TemporaryDirectory
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
try:
    from ncb1_client import CoreClient, CoreRequestError, CoreTransportError, LaunchSnapshot
except ModuleNotFoundError as error:
    if error.name != "ncb1_client":
        raise
    raise AssertionError("The Manager's NCB-1 client is missing") from None


class ManagerConnection(unittest.TestCase):
    def peer(self, script):
        spawn = subprocess.Popen

        def substitute(_args, **pipes):
            process = spawn([sys.executable, "-c", script], **pipes)
            self.addCleanup(process.wait, timeout=3)
            return process

        return patch("ncb1_client.subprocess.Popen", side_effect=substitute)

    @unittest.skipUnless(os.environ.get("NUMERA_CORE_TEST_BINARY"),
                         "Set NUMERA_CORE_TEST_BINARY to a locally built Core executable")
    def test_configured_core_negotiates_and_keeps_empty_store_unmodified(self):
        with TemporaryDirectory(prefix="numera-é-") as temporary:
            root = Path(temporary).resolve()
            store = root / "store"
            data_root = root / "data"
            data_root.mkdir()
            values = json.loads((ROOT / "contracts/fixtures/launch-desktop.json")
                                .read_text(encoding="utf-8"))
            values["resources"].update(data_root=str(data_root),
                                       native_profile_dir=str(data_root / "profile"),
                                       cache_root=str(data_root / "cache"))
            values["build_policy"] = {"kind": "pinned", "sha256": "a" * 64}
            snapshot = LaunchSnapshot(values)
            values["launch_id"] = "changed-after-snapshot"
            with CoreClient(Path(os.environ["NUMERA_CORE_TEST_BINARY"]).resolve(), store, data_root) as core:
                self.assertEqual(core.capabilities["namespace"], "numera.core")
                self.assertEqual(core.list(), ())
                plan = core.plan(snapshot)
                self.assertEqual((plan["launch_id"], plan["status"]), ("fixture-launch", "blocked"))
                with self.assertRaises(CoreRequestError) as error:
                    core.launch(snapshot)
                self.assertEqual(error.exception.code, "missing_package")
                with self.assertRaises(CoreRequestError) as error:
                    core.status("fixture-launch")
                self.assertEqual(error.exception.code, "invalid_configuration")
                self.assertEqual(core.list(), ())
            self.assertEqual(core.close(), 0)
            self.assertFalse(store.exists())
            self.assertEqual(list(data_root.iterdir()), [])

    def test_wrong_response_id_closes_inherited_stdin(self):
        with TemporaryDirectory() as temporary:
            marker = Path(temporary, "eof")
            script = (
                "import json, pathlib, sys\n"
                "request = json.loads(sys.stdin.readline())\n"
                "assert request['operation'] == 'hello'\n"
                "print(json.dumps(dict(namespace='numera.core', protocol=1, id='wrong-id', "
                "status='ok', result={})), flush=True)\n"
                "assert sys.stdin.buffer.read() == b''\n"
                f"pathlib.Path({str(marker)!r}).touch()\n"
            )
            with self.peer(script):
                with self.assertRaisesRegex(CoreTransportError, "correspond"):
                    CoreClient(Path(sys.executable), marker.parent, marker.parent)
            deadline = time.monotonic() + 3
            while not marker.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(marker.is_file(), "A protocol error did not deliver EOF to the service")

    def test_request_deadline_includes_a_full_stdin_pipe(self):
        host = {"os": "windows" if sys.platform == "win32" else sys.platform,
                "arch": platform.machine().lower().replace("amd64", "x86_64")}
        hello = {"product": "Numera Core Browser", "core_version": "0", "protocol": 1, "schema_revision": 1}
        capabilities = {"namespace": "numera.core", "protocol": 1, "schema_revision": 1,
                        "core_version": "0", "host": host, "features": {"protocol.ndjson": {"status": "supported"}},
                        "scope": {"engine_build": None, "device_mode": None, "automation_transport": None}}
        script = (
            "import json, sys, time\n"
            f"for result in ({hello!r}, {capabilities!r}):\n"
            "    request = json.loads(sys.stdin.readline())\n"
            "    print(json.dumps(dict(namespace='numera.core', protocol=1, id=request['id'], "
            "status='ok', result=result)), flush=True)\n"
            "time.sleep(0.6)\n"
            "assert len(sys.stdin.buffer.read()) < 900000\n"
        )
        with self.peer(script):
            root = Path.cwd()
            with CoreClient(Path(sys.executable), root, root) as core:
                oversized_for_pipe = LaunchSnapshot({"padding": "x" * 900_000})
                start = time.monotonic()
                with self.assertRaisesRegex(CoreTransportError, "deadline"):
                    core.plan(oversized_for_pipe, deadline_ms=100)
                self.assertLess(time.monotonic() - start, 0.5)
                with self.assertRaisesRegex(CoreTransportError, "closed"):
                    core.plan(oversized_for_pipe)

    def test_unterminated_frame_cannot_grow_past_limit(self):
        script = (
            "import sys\n"
            "sys.stdin.readline()\n"
            "sys.stdout.buffer.write(b'x' * 1048576)\n"
            "sys.stdout.buffer.flush()\n"
            "sys.stdin.buffer.read()\n"
        )
        with self.peer(script):
            root = Path.cwd()
            with self.assertRaisesRegex(CoreTransportError, "frame limit"):
                CoreClient(Path(sys.executable), root, root)


if __name__ == "__main__":
    unittest.main()
