# Security status

This development repository has no approved engine, browser GUI, trusted updater or running-session implementation. Do not use it as a production browser or assume fingerprint/emulation/automation capabilities from configuration validation.

Implemented protections include strict recursive duplicate-key rejection, typed payload validation, 1 MiB NDJSON bounds, bounded queues, monotonic deadlines, output-backpressure cancellation, explicit handshake/version checks, safe diagnostics and fail-closed missing-engine/proxy behavior. Rust unsafe code is forbidden. No local network control endpoint is opened.

Threats still requiring real-engine acceptance include malicious web pages/extensions reaching local control, compromised update metadata or archives, profile grant races, stale/reused PIDs, escaped descendants, proxy leaks, browser database upgrades and untrusted PR artifacts. Native sandbox/security regression suites, fuzz campaigns and signed update rollback remain release gates.

Do not put credentials, cookies, browsing content or credential-bearing URLs in issues or diagnostics. Use GitHub's private vulnerability reporting if enabled. There is no claimed commercial signing/notarization or signed update root in this repository.
