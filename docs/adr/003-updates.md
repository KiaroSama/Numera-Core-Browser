# ADR 003 — observe candidates before trusted activation

Feed observation implemented; installation/activation/rollback is not implemented.

The 6-hour watcher reads official Chromium per-platform stable rollouts, Mozilla version metadata, WebKit revisions and official automation-client releases. It keeps one review-only candidate branch/PR and never changes approved source locks or merges/releases automatically. Puppeteer's client releases are distinguished from its separate browser-download package. New observations are not signed or trusted update metadata.

A production updater must use reviewed, signed, expiring and rollback-resistant metadata with a documented offline trust root and key rotation/revocation. `awslabs/tough` is a reference candidate, not an adopted or validated dependency. A same-source hash alone cannot authenticate a release. Signing keys must stay out of PR jobs.

Stage in versioned directories, verify complete archives and path safety before activation, and use crash-recoverable atomic transitions. Active/recoverable sessions hold leases; activation affects eligible new sessions. No active browsing is killed and pinned engines are preserved. Garbage collection must respect leases. Binary rollback requires compatible native data or a copy-first snapshot; it cannot reverse an upgraded database by renaming a binary.

Until trust roots, metadata verification, extraction limits, staging, leases, recovery and real-data rollback tests exist, all update capabilities remain unsupported. Offline/expired metadata must fail explicitly. No source or binary release is promoted by the current workflows.
