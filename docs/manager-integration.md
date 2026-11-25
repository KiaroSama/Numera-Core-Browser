# Numera Browser integration

The `core-client` crate connects to the inherited-stdio `numera.core` development protocol. It starts a local `numera-core serve STORE DATA_ROOT`, negotiates wire major 1 and schema revision 1, and reads the capabilities of the selected build. The Manager adapter has its own protocol and message envelope. [NCB-1 contract](../contracts/PROTOCOL.md) · [Rust client](../crates/core-client/src/lib.rs)

## Connect from a Manager provider

Use absolute paths for the trusted Core executable, operator-provisioned signed Chromium store, and private data root. Supply a typed `LaunchSnapshot` with separate profile and cache directories under the configured data root. Keep the launch ID and idempotency key stable on a deliberate retry over the same connection. [Launch fixture](../contracts/fixtures/launch-desktop.json)

```rust
use core_client::CoreClient;
use core_contracts::{BuildPolicy, LaunchSnapshot, MAX_DEADLINE_MS};
use std::path::Path;

fn launch_and_stop(
    binary: &Path,
    store: &Path,
    data_root: &Path,
    mut snapshot: LaunchSnapshot,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = MAX_DEADLINE_MS;
    let mut core = CoreClient::connect(binary, store, data_root, deadline)?;
    let sha256 = core.capabilities().scope.engine_build.clone()
        .ok_or("No approved Core browser package is available")?;
    snapshot.build_policy = BuildPolicy::Pinned { sha256 };
    let session = core.launch(&snapshot, deadline)?;
    let id = &session.launch_id;
    let _attached = core.attach(id, deadline)?;
    let _sessions = core.list(deadline)?;
    let _current = core.status(id, deadline)?;
    let events = core.events(id, 0, deadline)?;
    let _stopped = core.stop(id, deadline)?;
    let _final_events = core.events(id, events.next_sequence, deadline)?;
    if !core.close()?.success() {
        return Err("Core service exited unsuccessfully".into());
    }
    Ok(())
}
```

Use `core.plan(&snapshot, deadline)` to inspect an operation before launch. The client checks that the selected digest and device match the negotiated scope and that required capabilities are supported. The current scope supports standalone desktop Chromium. `attach` observes a session. Application automation requires a separately advertised transport.

## Own the connection for the session lifetime

The client allows one in-flight operation and uses bounded UTF-8 NDJSON frames. A single monotonic budget covers planning, writing and reading, up to 30 seconds. `ClientError::Request` contains the stable NCB-1 error code. `ClientError::Transport` invalidates the connection and can leave the outcome of an in-flight request unknown. Keep request IDs, raw payloads, credentials and endpoint contents out of logs.

Keep one connection alive while using its sessions. Send a status or event poll before the service's 60-second input idle limit. The service accepts 1024 requests per connection including negotiation. Plan that budget before starting long-running work. Call `stop` to confirm cleanup for a specific profile. On normal close, stdin EOF requests Core worker cleanup and the client waits for the service's exit status. If exit exceeds the eight-second cleanup budget, the client reports a transport error and continues to reap the service in the background. A fresh connection starts with its own session records. [Framing and budgets](../contracts/PROTOCOL.md)

## Remaining Manager acceptance

The Rust client connects to Core's standalone authority. Managed admission still needs a real grant from the Manager's existing lease mechanism. Detached supervision, session reconciliation after reconnect and a paired smoke test using the actual Manager and a signed native browser on each active platform remain open. The Manager repository has only been inspected. [Manager adapter](https://github.com/KiaroSama/Numera-Browser/blob/6b486e16653bca1b18eb0007238a50175421d54a/engine-rust/src/browser/adapter.rs)

An integration release still needs an explicit Manager provider choice, a pinned and verified Core contract, and an owned Chromium package with source-build and native browser evidence. The Manager owns profile business records and credentials at rest. Core owns engine-specific settings, proxy transport and package updates. The existing versioned contract download is development evidence. [Versioned development artifacts](evidence/development-artifacts-44907851253.json) · [Original integration contract](original-build-specification.md#shared-integration-contract--ncb-1)

Acceptance on Windows and Linux includes version skew, lost replies, same- and different-profile concurrency, cancellation, crash and EOF handling, and stopping one profile while others stay active. It also includes data-format compatibility, copy-first migration and snapshots, pinned updates with running sessions, portable paths, themes, RTL and sanitized diagnostics.
