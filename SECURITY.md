# Security status

This development repository has no approved engine, browser GUI, trusted updater or running-session implementation. Do not use it as a production browser or assume fingerprint/emulation/automation capabilities from configuration validation.

Implemented protections include strict recursive duplicate-key rejection, typed payload validation, 1 MiB NDJSON bounds, bounded queues, monotonic deadlines, output-backpressure cancellation, explicit handshake/version checks, safe diagnostics and fail-closed missing-engine/proxy behavior. Authored Rust denies unsafe code except the narrow Windows containment bridge reviewed in [ADR 002](docs/adr/002-process-and-security.md). No local network control endpoint is opened.

Gitleaks scans full fetched history on the default branch and introduced history for pull requests. Two historical contract-manifest checksum lines are known false positives for its generic API-key rule: their values are verified from immutable Git blobs before scanning. The exception requires both the exact manifest path and entire field/value line; it excludes no commits. Seven real-scanner controls verify both public lines and rejection of changed paths, fields, values, trailing content and a coexisting token-like field. Repository suppression files remain disabled, and scanner/Git-child errors still fail closed. These public-content digests are not credentials.

Threats still requiring real-engine acceptance include malicious web pages/extensions reaching local control, compromised update metadata or archives, profile grant races, stale/reused PIDs, escaped descendants, proxy leaks, browser database upgrades and untrusted PR artifacts. Native browser sandbox/security regression suites, browser/native fuzz campaigns and signed update rollback remain release gates.

Do not put credentials, cookies, browsing content or credential-bearing URLs in issues or diagnostics. Use GitHub's private vulnerability reporting if enabled. There is no claimed commercial signing/notarization or signed update root in this repository.
