<p align="center"><img src="assets/logo.svg" width="240" height="240" alt="Numera Core Browser"></p>

# Numera Core Browser

An independent browser runtime for [Numera Browser](https://github.com/KiaroSama/Numera-Browser), following the GoLogin/Orbita product boundary: the manager owns profiles; Core owns the browser runtime. The initial delivery targets an owned Chromium source build and retained browser chrome, with a shared Rust control plane.

**Development status.** Windows and Linux x86-64 are the active targets. The Rust control plane passed native CI on both. The Linux development browser built from pinned Chromium source is available to download. Its native window, relocated runtime and all three automation clients passed controlled tests. The Windows source build and production installation testing remain open. [Linux development browser evidence](docs/evidence/development-chromium-44983567389.json) · [Implementation report](docs/implementation-report.md) · [Scope](docs/scope.md) · [Original handoff](docs/original-build-specification.md)

This is source-available, noncommercial software under the exact [Noncommercial Network Share-Alike License 1.0](LICENSE). Third-party material retains its own terms. It is not described as OSI-approved open source.

The supplied logo is preserved at `assets/logo.svg`. No GoLogin or Orbita code, binary, or proprietary patch is incorporated.

## Download the Linux development browser

The Ubuntu 24.04 x86-64 artifact includes the Chromium 157.0.8103.78 runtime, ChromeDriver, generated notices, matching first-party source and the pinned upstream source reference. The download is 1.74 GB and the extracted runtime uses 8.35 GB. GitHub sign-in is required to download the seven-day Actions artifact. [Download the package](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/44983567389/artifacts/14745018310) · [Download provenance](https://github.com/KiaroSama/Numera-Core-Browser/actions/runs/44983567389/artifacts/14745233722)

After extracting both Actions ZIPs, verify the inner `numera-portable-runtime.tar.gz` and `development-runtime.json` with the extracted attestation bundle. Enforce the `main` ref, source commit `ac5170bbb8161d89034ce3f5130c84b1f0d24605`, the signer workflow recorded in the evidence and the GitHub-hosted runner restriction. The native Rust CLI also verifies the tarball's file layout and recorded SHA-256. [Exact verification evidence](docs/evidence/development-chromium-44983567389.json) · [GitHub verification reference](https://cli.github.com/manual/gh_attestation_verify)

Extract the verified tarball into a fresh directory with `tar -xzf numera-portable-runtime.tar.gz`. The browser executable is `runtime/chrome`. Launch it with a new, explicit `--user-data-dir` to keep the development profile separate from existing browser data. The downloaded browser opened a native window on fresh Ubuntu 22.04 and 24.04 runners. Ubuntu 22.04 ran with the default sandbox and no administrator setup. The Ubuntu 24.04 runner required a separately installed, verified sandbox helper. Installation automation and automatic updates remain open. [Ubuntu 22.04 test](docs/evidence/linux-retained-browser-45002016269.json) · [Ubuntu 24.04 test](docs/evidence/linux-retained-browser-45000980622.json) · [Portability details](docs/portability.md)

## Run the implemented CLI

Install the toolchain pinned in `rust-toolchain.toml`, then:

```sh
cargo build --locked -p core-cli
cargo run --locked -q -p core-cli -- --json doctor
cargo run --locked -q -p core-cli -- engines list
cargo run --locked -q -p core-cli -- serve
```

`doctor` reports command success separately from browser readiness. Configure a signed package store and a private data directory to enable standalone sessions.

`numera-core --json engines verify-archive FILE SHA256` verifies a portable Chromium `tar.gz` layout and SHA-256 digest. Supply the digest from a separately verified build report or signed release metadata. The command reads the archive and reports its file count and expanded size.

Build and check the project with the pinned Rust toolchain. Run `cargo run --locked -p core-tooling --bin numera-build -- --mode check` or use `--mode build` for a release build. External Chromium source-build and SDK tests use their pinned upstream toolchains.

Successful native CI jobs produce `development-cli-*` artifacts with a `development-cli.tar.gz` bundle containing the CLI, optional desktop launcher, notices and matching `complete-source.zip`. After extracting the artifact ZIP, extract the tar bundle with `tar -xzf development-cli.tar.gz`. Run `./numera-core --json doctor` from the extracted directory, or `.\numera-core.exe --json doctor` in PowerShell. These unsigned development archives require a separate signed Chromium package for browser sessions. Clean-machine installation validation remains open.

For Windows or Linux x86-64, select the corresponding `development-cli-*` artifact on a recent successful Windows and Linux native CI run. GitHub may require login, and each archive expires seven days after its run. Check the source commit in `development-manifest.json`. [Native CI runs](https://github.com/KiaroSama/Numera-Core-Browser/actions/workflows/rust.yml)

The `numera-core-contracts-v1-*` artifacts contain the versioned schemas, fixtures, theme tokens and conformance harness. The Windows and Linux jobs tested the extracted contract against their respective native CLIs and produced the same contract ZIP bytes. [Verified artifacts](docs/evidence/development-artifacts-45004659038.json) · [Contract changelog](contracts/CHANGELOG.md)

Successful `main` builds publish a `development-artifact-provenance` bundle. Verify the inner CLI tar or contract ZIP with `gh attestation verify`, the extracted bundle, and the expected workflow, ref and source commit. The evidence records exact subject digests and the signer workflow. [Provenance and downloads](docs/evidence/development-artifacts-45004659038.json) · [GitHub verification reference](https://cli.github.com/manual/gh_attestation_verify)

## Install and launch from signed offline packages

Use an operator-provisioned publisher root, an offline TUF repository and a private writable data directory. Select absolute paths for the store and data directory. The store and data directory must be separate. The publisher-root file comes from an independent trusted channel. The public Linux development download above has separate provenance and uses its own launch procedure.

```sh
numera-core engines install "$STORE" "$TRUSTED_ROOT" "$REPOSITORY"
numera-core-gui "$STORE" "$DATA_ROOT" work https://example.org/
```

The launcher opens the installed Chromium window and uses a separate profile folder for each named profile. Existing profiles retain their package digest across package activation. Build `numera-core` and `numera-core-gui` with `cargo build --locked -p core-cli`. On Windows use the `.exe` binaries with PowerShell variables and absolute paths.

Use `numera-core serve STORE DATA_ROOT` for inherited-stdio standalone sessions, or `numera-core launch STORE DATA_ROOT SNAPSHOT.json` for a foreground launch with an explicit snapshot. Existing `serve` and `plan SNAPSHOT.json` also work without a configured store for protocol and configuration inspection. [Protocol](contracts/PROTOCOL.md) · [Offline publisher and update commands](docs/adr/003-updates.md) · [Profile snapshots](docs/portability.md)

## Source layout

| Path | Implemented responsibility |
| --- | --- |
| `crates/core-contracts` | Typed NCB-1 messages, strict JSON, schemas and NDJSON bounds |
| `crates/core-identity` | Coherence validation and deterministic identity keys; no browser overrides |
| `crates/core-engines` | Signed offline package staging, integrity verification and version selection |
| `crates/core-process` | Standalone admission claims, Windows job and Linux worker ownership, profile snapshots |
| `crates/core-runtime` | Pure launch planning, native session lifecycle and NCB-1 negotiation |
| `crates/core-cli` | Package administration, inherited-stdio service and Chromium desktop launcher |
| `contracts` | Generated canonical schemas, fixtures and manager theme tokens |
| `upstream` | Per-platform source pins, minimal branding patch and unreviewed observations |
| `crates/core-client` | Bounded Rust client for the local NCB-1 service |
| `crates/core-tooling` | Build orchestration, dependency review, packaging and conformance |

The launcher supports standalone Chromium sessions. Managed grants, detached reconnect, credentialed proxy transport, mobile emulation and public publisher release acceptance remain open.

## Validate

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --locked -p core-cli
cargo run --locked -p core-tooling --bin numera-export-contracts -- --binary target/debug/numera-core --check
cargo run --locked -p core-tooling --bin numera-check-contracts -- --binary target/debug/numera-core
cargo run --locked -p core-tooling --bin numera-licenses -- --check
cargo run --locked -p core-tooling --bin numera-security-audit
```

Windows executable paths end in `.exe`. Use the native CI matrix for Windows and Linux verification. [Protocol](contracts/PROTOCOL.md) · [Security](SECURITY.md) · [Updates](docs/adr/003-updates.md) · [Feasibility](docs/feasibility.md) · [Manager integration](docs/manager-integration.md)

The external consumer also tests shutdown with a saturated stderr pipe. On Linux it compiles a Rust preload fixture with the pinned `rustc` to reproduce file replacement during opening; this fixture is never linked into the installed CLI.

[Bounded IPC fuzzing](fuzz/README.md) has its own pinned development toolchain,
dependency inventory and CI job. It does not add nightly Rust or libFuzzer to
the installed CLI or imply native browser security acceptance.
