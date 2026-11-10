# Source-build feasibility — BLOCKED before hosted execution

No GitHub-hosted source-build run or compiled engine is available. The GitHub integration denied both repository-creation routes: GraphQL `createRepository` and REST `POST /user/repos` returned `Resource not accessible by integration` (REST HTTP 403). The exact requested repository was absent when inspected. No existing repository was renamed or overwritten.

Consequently the required public hosted spike could not be dispatched. Runner image, hosted free disk, engine working set, cold/warm acquisition, compile/link/package durations and resulting artifacts are **NOT_RUN**, not inferred from this managed environment. No workflow success URL is claimed. See `docs/evidence/coverage.json` and the retained local observation, explicitly labeled as local.

Official GitHub documentation retrieved on 2026-11-10 lists 14 GB SSD for standard public runners and a 6-hour job limit. Current Chromium Windows prerequisites require at least 100 GB free NTFS space; current source paths must avoid spaces. These are a documented concern, not proof of the actual disk available on an unmeasured runner.

The prepared workflow uses only standard `ubuntu-24.04`, `windows-2025-vs2026`, `macos-15` ARM64 and `macos-15-intel` jobs. It measures actual image/tools/disk before/after task-owned scratch preparation. It does not delete installed runner SDKs or unrelated system data. It initially uses a conservative 100 GB source-build safety budget based on the documented Windows minimum; per-platform minimums and toolchains still require validation.

If that budget is available, the bounded native recipe acquires pinned depot_tools and Chromium plus full source dependencies, checks the pinned revision and patch hashes, generates a reduced-symbol release build, and attempts the `chrome` target at parallelism 2. No browser objects, stock binaries, CEF packages, hidden remote execution or warm caches are used. A five-minute spike budget or 20 GB free-disk guard stops the owned source-tool process tree and preserves the exact attempted stage. Null measurements remain unmeasured. A successful compilation alone still cannot pass native acceptance/package/release gates.

The source recipe is a candidate, with PowerShell parsing checked separately from execution. Native SDK availability, GN arguments, sandbox behavior, final link/package size and reproducibility require actual hosted execution. No split-DAG or incremental cache approach has been proven. No paid/self-hosted fallback is authorized.

Next action after repository access is available: push the prepared repository, dispatch `engine-feasibility.yml`, retain every native artifact, review the failures, and update this report with measured evidence. If source acquisition or compilation still cannot fit allowed infrastructure, leave the target BLOCKED and continue independent work without claiming an owned browser exists.

Sources:

- https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- https://docs.github.com/en/actions/reference/limits
- https://chromium.googlesource.com/chromium/src/+/main/docs/windows_build_instructions.md
