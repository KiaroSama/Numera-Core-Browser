# Scope and instruction precedence

The supplied `original-build-specification.md` is the requirements baseline, not proof that work already exists. Subsequent user direction takes precedence:

- On 2026-11-10 the user requested fresh research and updates, asked about one common engine, then requested the Orbita approach.
- The user later created the exact public repository, requested matching logo dimensions, and explicitly authorized merging all existing Core side branches. The related current Numera READMEs use a 240-pixel logo. This merge authorization applies to the current Core branches, not automatic approval of future engine releases or changes to the manager repository.
- Official GoLogin documentation says Orbita is Chromium-based and does not offer Firefox profiles. The initial milestone therefore focuses on an owned Chromium-derived browser with a shared Rust runtime, separate from the manager. Gecko and WebKit remain recorded future engine paths; they are not advertised as implemented. The original three-engine acceptance matrix is retained, with deferred entries explicit.
- On 2026-11-21 the user removed both macOS architectures from the product. Active source builds, CI and distribution target Windows and Linux x86-64. Existing macOS observations are historical.
- Desktop Android/iOS emulation is a later tested feature, not genuine Android/iOS or Safari. No native mobile application is implied.
- Use free standard public GitHub-hosted runners only for engine compilation. No paid/self-hosted build service, stock browser relabeling, or security disabling.

## Inspection

The manager's main branch was inspected at `6b486e16653bca1b18eb0007238a50175421d54a` on November 22. Its change since the earlier inspection adds a secret-scan workflow. Python remains the default engine and the native Rust migration remains separate. [Manager comparison](https://github.com/KiaroSama/Numera-Browser/compare/bf3196394458c4b09d25ca6c0dccbd9754041c42...6b486e16653bca1b18eb0007238a50175421d54a)

The manager's current SDK adapter sends a numeric request ID and a `hello` operation. A fixture using that initial envelope was rejected by the Core CLI because NCB-1 requires a namespace, string ID, deadline and typed payload. A manager NCB-1 client and a paired native browser test remain open. [Manager adapter](https://github.com/KiaroSama/Numera-Browser/blob/6b486e16653bca1b18eb0007238a50175421d54a/engine-rust/src/browser/adapter.rs) · [NCB-1 types](../crates/core-contracts/src/lib.rs)

The inspected migration PR is closed and its head is `3bf1d839fd33e42a10b982bd8607a7052ea7c4cd`. Manager inspection was read-only. [Manager migration PR #56](https://github.com/KiaroSama/Numera-Browser/pull/56)

No AGENTS.md, private Rules, Spec Kit installation, `.specify` workflow, or repository hooks were found in the available workspace or either inspected manager checkout. No private instructions or command chain are invented. Git author identity follows the configured environment identity.

## Gate policy

Resource measurements and source-build acceptance are separate. A preflight can complete while engine feasibility is BLOCKED. No browser release is permitted without cold source acquisition, compile/link/package measurements, native tests, complete corresponding source, license review, and real automation tests. NOT_RUN is never support.
