# Source-build feasibility — hosted spike dispatched

The initial GitHub integration denied both repository-creation routes: GraphQL `createRepository` and REST `POST /user/repos` returned `Resource not accessible by integration` (REST HTTP 403). The user subsequently created the exact public repository and its prepared source history was published. No existing repository was renamed or overwritten.

The first public hosted spike is [run 42871431763](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/42871431763), dispatched against source commit `8c0bcf819c0fcded67e5b7a20d69c7fd68902419`. Its results are pending. No compiled engine or workflow success is claimed. Hosted measurements will be retained separately from the earlier local-only observation in `docs/evidence`.

Official GitHub documentation retrieved on 2026-11-10 lists 14 GB SSD for standard public runners and a 6-hour job limit. Current Chromium Windows prerequisites require at least 100 GB free NTFS space; current source paths must avoid spaces. These are a documented concern, not proof of the actual disk available on an unmeasured runner.

The prepared workflow uses only standard `ubuntu-24.04`, `windows-2025-vs2026`, `macos-15` ARM64 and `macos-15-intel` jobs. It measures actual image/tools/disk before/after task-owned scratch preparation. It does not delete installed runner SDKs or unrelated system data. It initially uses a conservative 100 GB source-build safety budget based on the documented Windows minimum; per-platform minimums and toolchains still require validation.

If that budget is available, the bounded native recipe acquires pinned depot_tools and Chromium plus full source dependencies, checks the pinned revision and patch hashes, generates a reduced-symbol release build, and attempts the `chrome` target at parallelism 2. No browser objects, stock binaries, CEF packages, hidden remote execution or warm caches are used. A five-minute spike budget or 20 GB free-disk guard stops the owned source-tool process tree and preserves the exact attempted stage. Null measurements remain unmeasured. A successful compilation alone still cannot pass native acceptance/package/release gates.

The source recipe is a candidate, with PowerShell parsing checked separately from execution. Native SDK availability, GN arguments, sandbox behavior, final link/package size and reproducibility require actual hosted execution. No split-DAG or incremental cache approach has been proven. No paid/self-hosted fallback is authorized.

Next action: retain every native spike artifact, review exact failing stages, and update this report with measured evidence. If source acquisition or compilation still cannot fit allowed infrastructure, leave the target BLOCKED and continue independent work without claiming an owned browser exists.

Sources:

- https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- https://docs.github.com/en/actions/reference/limits
- https://chromium.googlesource.com/chromium/src/+/main/docs/windows_build_instructions.md
