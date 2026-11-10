# NCB-1 development contract

Namespace `numera.core`, wire major `1`, schema revision `1`. This is distinct from the manager's existing adapter protocol numbered `1`. Canonical request/response/manifest schemas are generated from Rust types with `scripts/export-contracts.py`; fixtures and a real producer/consumer harness live alongside them. This is not a released manager-integration contract.

Inherited stdio is the only transport. Frames are UTF-8 JSON followed by LF, at most 1 MiB including LF. Duplicate keys at every depth, malformed encodings, trailing JSON, unknown fields and invalid typed payloads are rejected. Empty EOF exits successfully. EOF within a frame, oversized frames and framing errors terminate with a sanitized error and exit `2`. Parsed per-request errors leave the connection open.

Requests contain `namespace`, `protocol`, `id`, `deadline_ms`, `operation`, and a typed `payload`. IDs are 1–128 ASCII letters/digits/dot/underscore/hyphen. Deadlines are relative milliseconds in `1..30000`, measured locally using monotonic time. No peer monotonic timestamp is compared. After 1024 request IDs the caller must reconnect. Duplicate IDs are rejected; no browser launch idempotency claim is made because launching is disabled.

Begin with `hello` and overlapping protocol/schema ranges, then `capabilities.get`. Other operations are unavailable until both succeed. A repeated hello is rejected. Failed negotiation never permits mutation or spawning.

| Operation | Current behavior |
| --- | --- |
| `hello` | Negotiate wire/schema range |
| `capabilities.get` | Scope capabilities to exact host and missing engine build |
| `engines.list` | Report no installed/approved engines and explicit deferred families |
| `launch.plan` | Pure validation; return a blocked effective plan, no storage/network/process side effects |
| `session.launch` | Validate, then return `missing_package`; never spawn |
| `diagnostics.collect` | Sanitized build/host/capability evidence without paths, browsing content or secrets |

Package administration, session attachment/status/events/stop, theme application, and update operations remain reserved requirements. They are not exposed as successful no-op operations. A missing feature or engine build never means support.

Responses correlate `id` and use `status: ok` with `result`, or `status: error` with stable `error.code` and safe `message`. Invalid untrusted envelopes use a null ID. Valid request IDs are echoed; callers must not use credentials as IDs.

Input and output queues have one slot. Partial/idle input expires after 60 seconds. Dispatch and response writes honor the request budget; blocked output also expires after 5 seconds. A stalled peer can receive a partial final frame before the process exits; it must treat partial EOF as a failed connection. EOF cannot orphan an engine because this version cannot create a session.

CLI stdout and service stdout contain JSON only. Logs go to stderr with a timestamp/timezone, uppercase primary severity and component. Exit codes: `0` completed command/clean EOF, `2` invalid input/framing, `3` missing package for applicable CLI errors, `4` cancellation/output timeout. A blocked `plan` or `doctor` can return exit `0`: inspect the reported readiness/status.

The launch snapshot is immutable and typed. Pure planning checks lexical native paths, viewport/screen/scale coherence, seed syntax, basic language/timezone syntax, requested engine, required capabilities, proxy configuration and explicit testing-only emulation acceptance. It does not validate the IANA timezone catalog, filesystem authorization, actual device behavior, native database compatibility or a running engine. Those require later evidence.
