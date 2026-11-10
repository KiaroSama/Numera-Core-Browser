# ADR 002 — admission and local security gate

Standalone admission primitives implemented; browser spawning and durable descendant supervision are not implemented.

`core-process::StandaloneLease` takes an explicit existing data root, acquires a real OS file lock per profile and atomically persists a bounded, strictly parsed claim with a cryptographic owner nonce. An explicit pre-spawn cancellation records `stopped`. A dropped/killed holder leaves its active claim intact; a free OS lock or reusable PID never authorizes automatic relaunch. Corruption, duplicate keys, oversize claims, unexpected state/owner edits and ambiguous ownership return `recovery_required` without rewriting the evidence. These are library primitives, not a new `session.launch` capability.

Tests exercise real separate processes, same-profile contention, independent profiles, lost replies and a killed holder. Unix checks owner UID, private directory permissions and no-follow/single-link files. Windows rejects reparse points, but full ACL validation and hard-link identity checks are still open. The caller must supply a trusted data root. Directory replacement by a hostile same-user process and power-loss recovery are not proven. No production launch is admitted through this partial boundary.

Managed mode must consume the manager's existing lease/claim admission grant rather than acquire a conflicting second lock. Standalone mode needs an equivalent Core-owned grant. Before a successful launch response, a single logical authority and durable OS ownership must exist. Windows job objects and POSIX ownership need separate tests; a process group or reusable PID alone is insufficient.

A launch ID plus idempotency key must survive lost responses without creating duplicate browsers. Recovery must reconcile durable claims and process identity. Corrupted/ambiguous ownership fails closed. Detached sessions need a surviving supervisor before the original owner exits. Cancellation must clean owned descendants before releasing admission. These are release blockers, not claims about the current code.

Only inherited stdio is implemented. Do not add a loopback debugging or control server without authentication, hostile-origin/DNS-rebinding defenses and scoped authorization. Session credentials must never grant global package administration. No tokens in URLs/command lines; use protected references and channels. Required proxy failure must never fall back to direct traffic.

Keep sandboxing, site isolation, certificate validation, GPU acceleration and safe permission boundaries. No `--no-sandbox`, certificate suppression or global web-security bypass is allowed to make CI pass. Parse requests strictly, bound frames/queues/deadlines, and never echo parser errors containing supplied data. All authored Rust forbids unsafe code.
