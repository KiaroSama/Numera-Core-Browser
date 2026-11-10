# Scope and instruction precedence

The supplied `original-build-specification.md` is the requirements baseline, not proof that work already exists. Subsequent user direction takes precedence:

- On 2026-11-10 the user requested fresh research and updates, asked about one common engine, then requested the Orbita approach.
- The user later created the exact public repository, requested matching logo dimensions, and explicitly authorized merging all existing Core side branches. The related current Numera READMEs use a 240-pixel logo. This merge authorization applies to the current Core branches, not automatic approval of future engine releases or changes to the manager repository.
- Official GoLogin documentation says Orbita is Chromium-based and does not offer Firefox profiles. The initial milestone therefore focuses on an owned Chromium-derived browser with a shared Rust runtime, separate from the manager. Gecko and WebKit remain recorded future engine paths; they are not advertised as implemented. The original three-engine acceptance matrix is retained, with deferred entries explicit.
- Desktop Android/iOS emulation is a later tested feature, not genuine Android/iOS or Safari. No native mobile application is implied.
- Use free standard public GitHub-hosted runners only for engine compilation. No paid/self-hosted build service, stock browser relabeling, or security disabling.

## Inspection

Manager main: `bf3196394458c4b09d25ca6c0dccbd9754041c42`.
Manager migration branch: `3bf1d839fd33e42a10b982bd8607a7052ea7c4cd`.
Open migration PR: [#56](https://github.com/KiaroSama/Numera-Browser/pull/56). Python remains selected on main; the native Rust library is a non-default migration candidate. No manager branch is changed, merged, or promoted here.

No AGENTS.md, private Rules, Spec Kit installation, `.specify` workflow, or repository hooks were found in the available workspace or either inspected manager checkout. No private instructions or command chain are invented. Git author identity follows the configured environment identity.

## Gate policy

Resource measurements and source-build acceptance are separate. A preflight can complete while engine feasibility is BLOCKED. No browser release is permitted without cold source acquisition, compile/link/package measurements, native tests, complete corresponding source, license review, and real automation tests. NOT_RUN is never support.
