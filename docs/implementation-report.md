# Implementation report

## Delivered development source

- A Rust workspace with real CLI commands and an inherited-stdio NCB-1 producer: diagnostics, capabilities, engine inventory, pure launch planning, manifest inspection, hashing and schema export.
- Strict JSON/NDJSON parsing, recursive duplicate-key checks, version/handshake state, bounded queues/frames/deadlines, output-backpressure cancellation and sanitized errors. Missing engines, unsupported capabilities and required proxies fail closed.
- Deterministic identity keys and configuration coherence checks with explicit emulation limitations. No page-visible identity or mobile behavior is claimed.
- Canonical generated schemas, positive/negative fixtures, an external Python consumer against the actual Rust process, versioned theme tokens and explicit engine/tool compatibility metadata.
- Per-platform Chromium source/depot_tools pins and a hashed BSD-licensed branding patch. It changes product strings only; the browser engine has not been compiled.
- Candidate PowerShell native source-build recipe and bounded public-hosted feasibility workflow; a separate native Rust CI matrix; official feed observation and one review-only candidate PR workflow.
- Exact selected custom license, user-supplied logo, 56 locked dependency entries with preserved license texts, public-reference research, security/update/process ADRs and a manager integration checklist.

The checked-in evidence describes actual tests and exact source commits. Local managed-Linux tests are separate from GitHub/native browser acceptance. There are no published successful Actions runs, PRs, source/binary releases or remote commits because repository creation was denied.

## Open gates

| Gate | Status | Remaining acceptance |
| --- | --- | --- |
| A: repository, license, baseline, feasibility | BLOCKED | Publish exact public repository and run the bounded hosted cold source-build spike; retain real runner metrics |
| B: working owned browser slice | BLOCKED | Build Chromium, native browser GUI, durable process ownership, real Playwright/Puppeteer/Selenium tests |
| C: broader engines/device/package scope | NOT_RUN | Engine-applied emulation, modular installers and themes; Gecko/WebKit deferred after the Orbita direction |
| D: trusted updates and integration | NOT_RUN | Signed metadata, staging/leases/activation/real-data rollback and paired manager/browser smoke suite |
| E: release acceptance | NOT_RUN | Full native matrix, upstream/security/licenses, packaging, performance and complete-source provenance |

No engine download or alternative wrapper was used to satisfy these gates. No Tauri/system-webview shell, imitation settings dashboard, unreviewed engine promotion or fake release badge was created. No changes were made to the manager, open PRs or issues.

## Access blocker and continuation

GraphQL and REST repository creation failed with `Resource not accessible by integration` (REST 403). Existing GitHub connectivity can read the manager but cannot create `KiaroSama/Numera-Core-Browser`. No token or secret is needed in chat. Publication can continue when the exact public empty repository exists and the connection can write it, or repository-creation access is provided.

The local repository and complete source archive remain reviewable and usable. After access is available: preserve the local history, publish it without force-pushing, dispatch source feasibility, fix measured failures, then proceed through owned-browser gates. A Rust green check cannot replace a blocked engine build.
