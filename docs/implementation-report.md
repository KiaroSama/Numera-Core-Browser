# Implementation report

## Delivered development source

- A Rust workspace with real CLI commands and an inherited-stdio NCB-1 producer: diagnostics, capabilities, engine inventory, pure launch planning, manifest inspection, hashing and schema export.
- Strict JSON/NDJSON parsing, recursive duplicate-key checks, version/handshake state, bounded queues/frames/deadlines, output-backpressure cancellation and sanitized errors. Missing engines, unsupported capabilities and required proxies fail closed.
- Deterministic identity keys and configuration coherence checks with explicit emulation limitations. No page-visible identity or mobile behavior is claimed.
- A standalone Rust admission library with real cross-process profile locks, atomic claims and fail-closed stale/corrupt ownership. Its Windows kernel-job primitive atomically contains new child processes and passed five real native ownership fixtures. Production browser-session integration, manager grants and recovery repair remain open.
- Canonical generated schemas, positive/negative fixtures, an external Python consumer against the actual Rust process, versioned theme tokens and explicit engine/tool compatibility metadata.
- Per-platform Chromium source/depot_tools pins and a hashed BSD-licensed branding patch. It changes product strings only; the browser engine has not been compiled.
- Candidate PowerShell native source-build recipe and bounded public-hosted feasibility workflow; a separate native Rust CI matrix; official feed observation and one review-only candidate PR workflow.
- Exact selected custom license, user-supplied logo, 63 locked dependency entries with preserved license texts, public-reference research, security/update/process ADRs and a manager integration checklist. The optional UEFI dependency r-efi 6.0.0 is explicitly used under its MIT alternative, with its complete AUTHORS grant preserved.

The user created the exact public repository after the initial API creation denial, and its prepared history was published without rewriting it. [Native control-plane CI](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/42873209957) passed on Windows x86-64, Linux x86-64, macOS ARM64 and macOS Intel, including 31 Rust tests per target, schema/real-consumer conformance, strict lints and license checks. The dependency-security job also passed. Exact commits/jobs are retained in `docs/evidence/hosted-verification.json`; earlier local evidence keeps its own context.

The newer [Windows ownership job](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/42890284816/job/130089345115) passed 44 Rust tests, including all five new native process fixtures, schema/actual-consumer conformance, strict lints and all 63 dependency notices. Linux passed its 40-test control-plane slice on that source as well. This is additional scoped evidence, not a claim that the new containment bridge passed every OS or ran a browser. [The retained ownership report](evidence/native-ownership-windows.json) records its limits and the discovered asynchronous Windows resource-release boundary.

[Run 42891689345](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/42891689345) subsequently passed the complete four-target control-plane matrix at `7709781086255648a53e9a844e5230ca41173073`: 44 tests on Windows, 40 each on Linux/macOS Intel/macOS ARM64, strict lints, real consumer/schema conformance, 63 dependency notices and vulnerability checks. Windows job containment is still a Windows-only primitive; passing other targets does not invent POSIX containment.

A separate development-only libFuzzer/AddressSanitizer campaign executed 159,936 cases in 61 seconds with no reported crash or sanitizer error. Its target allows only pure protocol operations and malformed-input rejection; real launch/install commands are excluded even if those implementations grow later. [The evidence](evidence/fuzz-wire-2026-11-10.json) binds target/dependency hashes and exact parameters. The fuzz graph has 64 separately inventoried/audited dependencies, including preserved legacy NCSA and file-level LLVM notices. This bounded pass does not certify browser internals, Windows FFI, hostile-origin defenses or exhaustive IPC security.

The upstream watcher passed after fixing candidate checkout/idempotency and adding a real local-Git regression fixture. The owner has now enabled the required Actions permission: [run 42876574326](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/42876574326) successfully created [draft PR #4](https://github.com/KiaroSama/Numera-Core-Browser/pull/4) as `app/github-actions`. Future observations remain subject to review; this is not automatic engine promotion.

At the user's explicit request, all three existing side branches were merged into main: [checkout update #1](https://github.com/KiaroSama/Numera-Core-Browser/pull/1), [upload-artifact update #2](https://github.com/KiaroSama/Numera-Core-Browser/pull/2), and [upstream observations #3](https://github.com/KiaroSama/Numera-Core-Browser/pull/3). Actions remain pinned to exact reviewed revisions. The logo display is 240×240, matching the current Numera Gen MCP/CHA Solver and manager migration READMEs. No browser binary release is available.

The later [observation PR #4](https://github.com/KiaroSama/Numera-Core-Browser/pull/4) was manually reviewed and merged under that instruction: only its observation timestamp and deferred WebKit commit changed. The official WebKit commit was verified; no approved engine lock, implementation support or release trust changed. The temporary authorization-test PR #5 was closed without merging and its task-owned ref removed.

## Open gates

| Gate | Status | Remaining acceptance |
| --- | --- | --- |
| A: repository, license, baseline, feasibility | BLOCKED | Repository/license/baseline and bounded hosted measurements complete; full cold source-build viability remains unresolved |
| B: working owned browser slice | BLOCKED | Build Chromium, native browser GUI, durable process ownership, real Playwright/Puppeteer/Selenium tests |
| C: broader engines/device/package scope | NOT_RUN | Engine-applied emulation, modular installers and themes; Gecko/WebKit deferred after the Orbita direction |
| D: trusted updates and integration | NOT_RUN | Signed metadata, staging/leases/activation/real-data rollback and paired manager/browser smoke suite |
| E: release acceptance | NOT_RUN | Full native matrix, upstream/security/licenses, packaging, performance and complete-source provenance |

No engine download or alternative wrapper was used to satisfy these gates. No Tauri/system-webview shell, imitation settings dashboard, unreviewed engine promotion or fake release badge was created. The Numera Browser manager repository is unchanged. Core PRs were merged only after the user explicitly requested merging the side branches; observation metadata is still not trusted engine release metadata.

## Publication and continuation

Initial GraphQL and REST repository creation failed with `Resource not accessible by integration` (REST 403). The user then created `KiaroSama/Numera-Core-Browser`; the connection can write that repository. The initial access blocker is resolved. No token or secret was supplied in chat.

The source and preserved history are available at https://github.com/KiaroSama/Numera-Core-Browser. Four initial hosted feasibility artifacts are retained in `docs/evidence/hosted-feasibility-42871431763`. Later Windows cold attempts completed source acquisition/dependencies/branding, exposing and fixing the pinned Python bootstrap and Windows GN argument quoting. The third reached toolchain setup and identified missing SDK 10.0.28000 headers on the hosted image. A hash-pinned, Microsoft-publisher-verified SDK preparation step is now under hosted validation before the next complete cold build. See [measured feasibility](feasibility.md). No compilation/link/package result or real browser acceptance has passed. A Rust green check cannot replace those gates.
