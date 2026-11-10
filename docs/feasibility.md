# Source-build feasibility — measured bounded spike, full build unresolved

The initial GitHub integration denied both repository-creation routes: GraphQL `createRepository` and REST `POST /user/repos` returned `Resource not accessible by integration` (REST HTTP 403). The user subsequently created the exact public repository and its prepared source history was published. No existing repository was renamed or overwritten.

The first public hosted spike is [run 42871431763](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/42871431763), against source commit `8c0bcf819c0fcded67e5b7a20d69c7fd68902419`. All four jobs retained real measurement artifacts and correctly failed the owned-engine gate. They did not build a browser. Exact runner image versions, tool paths, bytes, phases and times are retained in `docs/evidence/hosted-feasibility-42871431763`.

| Standard hosted target | Free disk before scratch preparation (decimal GB) | Last stage | Observed outcome |
| --- | ---: | --- | --- |
| Ubuntu 24.04 x86-64 | 92.13 | resource-preflight | Below the initial 100 GB safety budget; no source acquisition |
| Windows 2025 / VS2026 x86-64 | 236.08 | source-acquisition | Five-minute budget exhausted; 766,883,663 logical bytes in partial task files |
| macOS 15 ARM64 | 49.41 | resource-preflight | Below the initial 100 GB safety budget; no source acquisition |
| macOS 15 Intel | 116.20 | source-acquisition | Five-minute budget exhausted; 709,874,509 logical bytes in partial task files |

Source acquisition remained incomplete on Windows and Intel macOS. Compilation, final linking, engine artifact size, exact peak engine working set/disk and warm-build behavior remain unmeasured. The scratch-file counts include source/build-tool acquisition files and are not the size of a complete engine checkout. The five-minute budget is a bounded probe limit, not GitHub's six-hour limit: these observations do **not** establish that a full build either can or cannot fit an allowed six-hour job.

Official GitHub documentation retrieved on 2026-11-10 lists 14 GB SSD for standard public runners and a 6-hour job limit. Current Chromium Windows prerequisites require at least 100 GB free NTFS space; current source paths must avoid spaces. These are a documented concern, not proof of the actual disk available on an unmeasured runner.

The prepared workflow uses only standard `ubuntu-24.04`, `windows-2025-vs2026`, `macos-15` ARM64 and `macos-15-intel` jobs. It measures actual image/tools/disk before/after task-owned scratch preparation. It does not delete installed runner SDKs or unrelated system data. It initially uses a conservative 100 GB source-build safety budget based on the documented Windows minimum; per-platform minimums and toolchains still require validation.

If that budget is available, the bounded native recipe acquires pinned depot_tools and Chromium plus full source dependencies, checks the pinned revision and patch hashes, generates a reduced-symbol release build, and attempts the `chrome` target at parallelism 2. No browser objects, stock binaries, CEF packages, hidden remote execution or warm caches are used. A five-minute spike budget or 20 GB free-disk guard stops the owned source-tool process tree and preserves the exact attempted stage. Null measurements remain unmeasured. A successful compilation alone still cannot pass native acceptance/package/release gates.

The source recipe is a candidate, with PowerShell parsing checked separately from execution. Native SDK availability, GN arguments, sandbox behavior, final link/package size and reproducibility require actual hosted execution. No split-DAG or incremental cache approach has been proven. No paid/self-hosted fallback is authorized.

Next action: a longer bounded cold-bootstrap experiment on the Windows/Intel macOS targets, validated platform budgets and reviewed safe cleanup where applicable. Only measured source acquisition, dependency invalidation, final link/package and provenance can establish feasibility. Until then leave engine acceptance BLOCKED and continue independent work without claiming an owned browser exists. The native Rust control-plane CI passed separately on all four targets; its result is not substituted for this failed engine gate.

Sources:

- https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- https://docs.github.com/en/actions/reference/limits
- https://chromium.googlesource.com/chromium/src/+/main/docs/windows_build_instructions.md
