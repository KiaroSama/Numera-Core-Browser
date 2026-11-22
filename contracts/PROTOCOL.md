# NCB-1 development contract

Namespace `numera.core`, wire major `1`, schema revision `1`. This is distinct from the manager's existing adapter protocol numbered `1`. Canonical request/response/manifest schemas are generated from Rust types with `scripts/export-contracts.py`; fixtures and a real producer/consumer harness live alongside them. This is not a released manager-integration contract.

Inherited stdio is the only transport. Frames are UTF-8 JSON followed by LF, at most 1 MiB including LF. Duplicate keys at every depth, malformed encodings, trailing JSON, unknown fields and invalid typed payloads are rejected. Empty EOF exits successfully. EOF within a frame, oversized frames and framing errors terminate with a sanitized error and exit `2`. Parsed per-request errors leave the connection open.

Requests contain `namespace`, `protocol`, `id`, `deadline_ms`, `operation`, and a typed `payload`. IDs are 1–128 ASCII letters/digits/dot/underscore/hyphen. Deadlines are relative milliseconds in `1..30000`, measured locally using monotonic time. No peer monotonic timestamp is compared. After 1024 request IDs the caller must reconnect. Duplicate IDs are rejected; no browser launch idempotency claim is made because launching is disabled.

Begin with `hello` and overlapping protocol/schema ranges, then `capabilities.get`. Other operations are unavailable until both succeed. A repeated hello is rejected. Failed negotiation never permits mutation or spawning.

| Operation | Current behavior |
| --- | --- |
| `hello` | Negotiate wire/schema range |
| `capabilities.get` | Report host, configured active digest, session scope and limitations |
| `engines.list` | List signed packages from the configured store and identify deferred families |
| `launch.plan` | Validate the snapshot and resolve the signed package descriptor without creating state or spawning |
| `session.launch` | Verify the installed package, acquire durable admission and start an owned standalone browser process |
| `session.status`, `session.attach` | Inspect a session on this inherited connection using `{ "launch_id": "..." }` |
| `session.list` | List the session views retained on this connection |
| `session.stop` | Stop the exact owned session using `{ "launch_id": "..." }` |
| `session.events` | Poll retained observations using `launch_id` and `after_sequence` |
| `diagnostics.collect` | Sanitized build/host/capability evidence without paths, browsing content or secrets |

The administrative CLI handles signed package operations. On an unconfigured service, launch still returns `missing_package`. The current endpoint supervises standalone sessions and reports the running observation and terminal observation with increasing sequence numbers. A repeated immutable launch on the same connection returns its current view. A reserved launch ID or idempotency key cannot start a second browser after reconnect. Reconnecting cannot seize a running worker; managed grants and detached reconnection require another authority. The `automation_endpoints` list remains empty until the package has an authenticated session transport.

Responses correlate `id` and use `status: ok` with `result`, or `status: error` with stable `error.code` and safe `message`. Invalid untrusted envelopes use a null ID. Valid request IDs are echoed; callers must not use credentials as IDs.

Input and output queues have one slot. Partial or idle input expires after 60 seconds. Poll `session.status` or `session.events` while holding an interactive session over inherited stdio. Dispatch and response writes honor the request budget and blocked output also expires after five seconds. A stalled peer treats a partial final frame as a failed connection. EOF closes the Core worker channels and starts native process cleanup. The worker releases admission after the owned tree and required profile locks clear. An interrupted worker leaves durable recovery state.

CLI stdout and service stdout contain JSON only. Logs go to stderr with a timestamp/timezone, uppercase primary severity and component. Exit codes: `0` completed command/clean EOF, `2` invalid input/framing, `3` missing package for applicable CLI errors, `4` cancellation/output timeout. A blocked `plan` or `doctor` can return exit `0`: inspect the reported readiness/status.

The launch snapshot is immutable and typed. Pure planning checks lexical native paths, viewport/screen/scale coherence, seed syntax, basic language/timezone syntax, requested engine, required capabilities, proxy configuration and explicit testing-only emulation acceptance. It does not validate the IANA timezone catalog, filesystem authorization, actual device behavior, native database compatibility or a running engine. Those require later evidence.
