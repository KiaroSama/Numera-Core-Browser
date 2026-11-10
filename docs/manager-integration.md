# Numera Browser integration checklist

Not integration-ready. The manager repository has only been inspected and is unchanged.

1. Finish the owned Chromium cold source-build gate and native browser/automation suites. Publish reviewed source/build provenance and a tested engine package.
2. Implement durable process ownership and the manager's admission grant bridge; test both repositories against the same lifecycle fixtures.
3. Publish a versioned `numera-core-contracts-v1` artifact with hashes, schemas, fixtures, theme tokens and compatibility evidence. The current artifact is development-only.
4. Add a `numera-core` provider in an appropriate manager implementation PR under its actual rules. Do not promote the existing migration branch or replace the selected Python runtime implicitly.
5. Consume a pinned canonical contract; negotiate `numera.core` wire/schema ranges and required capabilities. Existing manager adapter protocol `1` is a different namespace.
6. Send a single immutable snapshot and protected secret/ownership references. Do not duplicate fingerprint injection, Client Hints overrides, proxy relays or package updating in the manager.
7. Run version-skew, lost-reply/idempotency, same/different-profile concurrency, cancellation/crash/EOF, single-session stop and preserved-session update tests with real engine artifacts.
8. Verify native data formats, copy-first migration/snapshots, engine pinning, portable paths, themes, RTL and safe diagnostics on each actual target OS.

No companion manager amendment is fabricated: the original handoff says it applies after a real Core integration release exists, and no such release exists yet.
