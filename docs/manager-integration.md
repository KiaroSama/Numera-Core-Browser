# Numera Browser integration

Core provides a Python client for its inherited-stdio `numera.core` development protocol. It starts the selected `numera-core serve STORE DATA_ROOT` executable, negotiates wire major 1 and schema revision 1, then reads capabilities scoped to the actual host and approved engine build. The Manager's existing adapter protocol numbered `1` uses a different envelope. [NCB-1 contract](../contracts/PROTOCOL.md) · [Python client](../scripts/ncb1_client.py)

## Connect from a Manager provider

Use a trusted local Core executable, an operator-provisioned signed Chromium store, and a private data root. Pass absolute paths to all three. For a local smoke run, put this checkout's `scripts` directory on `PYTHONPATH`. The client uses the Python standard library, with Python 3.12 or later required on Windows for nonblocking pipes. The service receives the platform and desktop session environment. Protocol frames and raw stderr stay out of Manager logs.

The example assumes an existing `snapshot_payload` built from the canonical launch schema. Set its resource fields to the configured data root and separate native profile and cache directories under it. Use a unique launch ID and idempotency key, and keep these values stable for any deliberate retry on the same connection. The sample uses standalone, desktop Chromium, direct proxy mode and system theme. [Launch fixture](../contracts/fixtures/launch-desktop.json) · [Request schema](../contracts/v1/request.schema.json)

```python
from ncb1_client import CoreClient, LaunchSnapshot

with CoreClient(core_binary, engine_store, data_root) as core:
    engine_build = core.capabilities["scope"]["engine_build"]
    if engine_build is None:
        raise RuntimeError("No approved Core browser package is available")
    snapshot = LaunchSnapshot({
        **snapshot_payload,
        "build_policy": {"kind": "pinned", "sha256": engine_build},
    })
    session = core.launch(snapshot)
    launch_id = session["launch_id"]
    attached = core.attach(launch_id)
    sessions = core.list()
    current = core.status(launch_id)
    events, cursor = core.events(launch_id, after_sequence=0)
    stopped = core.stop(launch_id)
    final_events, cursor = core.events(launch_id, after_sequence=cursor)
```

`LaunchSnapshot` copies the supplied configuration. The caller can inspect a side-effect-free plan using `core.plan(snapshot)`. Launch repeats the plan and sends the same snapshot only after the selected digest and device match the negotiated scope and every required capability is `supported`. The currently advertised `partial` launch capability covers this standalone mode. An inspectable plan can report `blocked` without a process start.

`attach`, `status` and `stop` use the connection's launch ID. `list` includes retained records for that connection. `events` returns ordered session views and the next cursor, which the caller passes to the following poll. Results are read-only. `attach` observes a Core session. Application automation requires a separately advertised transport, which the current standalone path does not expose.

## Own the connection for the session lifetime

The client allows one in-flight operation and uses bounded UTF-8 NDJSON frames. A single monotonic budget covers planning, writing and reading, up to 30 seconds. `CoreRequestError.code` exposes stable failures including `missing_package` and `unsupported_capability`. A `CoreTransportError` means the connection failed, so the Manager must treat an in-flight outcome as unknown. Keep request IDs, raw payloads, credentials and endpoint contents out of logs.

Keep one connection alive while using its sessions. Send a status or event poll before the service's 60-second input idle limit. The service accepts 1024 requests per connection including negotiation. Plan that budget before starting long-running work. Call `stop` to confirm cleanup for a specific profile. On normal close, stdin EOF requests Core worker cleanup and the client waits for the service's exit status. If exit exceeds the eight-second cleanup budget, the client reports a transport error and continues to reap the service in the background. A fresh connection starts with its own session records. [Framing and budgets](../contracts/PROTOCOL.md)

## Remaining Manager acceptance

The Python client connects to Core's standalone authority. Managed admission still needs a real grant from the Manager's existing lease mechanism. Detached supervision, session reconciliation after reconnect and a paired smoke test using the actual Manager and a signed native browser on each active platform remain open. The Manager repository has only been inspected. [Manager adapter](https://github.com/KiaroSama/Numera-Browser/blob/6b486e16653bca1b18eb0007238a50175421d54a/engine-rust/src/browser/adapter.rs)

An integration release still needs an explicit Manager provider choice, a pinned and verified Core contract, and an owned Chromium package with source-build and native browser evidence. The Manager owns profile business records and credentials at rest. Core owns engine-specific settings, proxy transport and package updates. The existing versioned contract download is development evidence. [Versioned development artifacts](evidence/development-artifacts-44907851253.json) · [Original integration contract](original-build-specification.md#shared-integration-contract--ncb-1)

Acceptance on Windows and Linux includes version skew, lost replies, same- and different-profile concurrency, cancellation, crash and EOF handling, and stopping one profile while others stay active. It also includes data-format compatibility, copy-first migration and snapshots, pinned updates with running sessions, portable paths, themes, RTL and sanitized diagnostics.
