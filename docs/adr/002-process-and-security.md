# ADR 002 — admission and local security gate

Design accepted; durable supervision is not implemented.

Managed mode must consume the manager's existing lease/claim admission grant rather than acquire a conflicting second lock. Standalone mode needs an equivalent Core-owned grant. Before a successful launch response, a single logical authority and durable OS ownership must exist. Windows job objects and POSIX ownership need separate tests; a process group or reusable PID alone is insufficient.

A launch ID plus idempotency key must survive lost responses without creating duplicate browsers. Recovery must reconcile durable claims and process identity. Corrupted/ambiguous ownership fails closed. Detached sessions need a surviving supervisor before the original owner exits. Cancellation must clean owned descendants before releasing admission. These are release blockers, not claims about the current code.

Only inherited stdio is implemented. Do not add a loopback debugging or control server without authentication, hostile-origin/DNS-rebinding defenses and scoped authorization. Session credentials must never grant global package administration. No tokens in URLs/command lines; use protected references and channels. Required proxy failure must never fall back to direct traffic.

Keep sandboxing, site isolation, certificate validation, GPU acceleration and safe permission boundaries. No `--no-sandbox`, certificate suppression or global web-security bypass is allowed to make CI pass. Parse requests strictly, bound frames/queues/deadlines, and never echo parser errors containing supplied data. All authored Rust forbids unsafe code.
