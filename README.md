<p align="center"><img src="assets/logo.svg" width="240" height="240" alt="Numera Core Browser"></p>

# Numera Core Browser

An independent browser runtime for [Numera Browser](https://github.com/KiaroSama/Numera-Browser), following the GoLogin/Orbita product boundary: the manager owns profiles; Core owns the browser runtime. The initial delivery targets an owned Chromium source build and retained browser chrome, with a shared Rust control plane.

**Development status: the Rust control plane passed native CI on Windows, Linux and both macOS architectures. Owned Chromium build acceptance remains unresolved; no browser GUI or integration-ready release is available.** A passing Rust check never implies a passing browser-source build. See [implementation report](docs/implementation-report.md), [hosted evidence](docs/evidence/hosted-verification.json), [scope](docs/scope.md), and the [original handoff](docs/original-build-specification.md).

This is source-available, noncommercial software under the exact [Noncommercial Network Share-Alike License 1.0](LICENSE). Third-party material retains its own terms. It is not described as OSI-approved open source.

The supplied logo is preserved at `assets/logo.svg`. No GoLogin or Orbita code, binary, or proprietary patch is incorporated.

## Run the implemented CLI

Install the toolchain pinned in `rust-toolchain.toml`, then:

```sh
cargo build --locked -p core-cli
cargo run --locked -q -p core-cli -- --json doctor
cargo run --locked -q -p core-cli -- engines list
cargo run --locked -q -p core-cli -- serve
```

`doctor` reports command success separately from browser readiness. `launch.plan` returns a blocked plan until a reviewed engine exists. `session.launch` fails with `missing_package`; it never downloads or relabels another product's browser.

The compiled CLI requires no Node.js, Python, display server or manager installation. Python is used only by development/research/conformance tools. On Windows use PowerShell 7: `./scripts/Build.ps1 -Mode Check` or `-Mode Build`.

## Source layout

Successful native CI jobs produce `development-cli-*` artifacts with a `development-cli.tar.gz` bundle containing the tested CLI, notices and matching `complete-source.zip`. After extracting the artifact ZIP, extract the tar bundle with `tar -xzf development-cli.tar.gz`. In its `development-cli` directory run `./numera-core --json doctor` (PowerShell: `.\numera-core.exe --json doctor`). These unsigned development archives contain the control plane; browser/GUI and clean-machine installation acceptance remain open.

| Path | Implemented responsibility |
| --- | --- |
| `crates/core-contracts` | Typed NCB-1 messages, strict JSON, schemas and NDJSON bounds |
| `crates/core-identity` | Coherence validation and deterministic identity keys; no browser overrides |
| `crates/core-engines` | Package-manifest validation, hashing and truthful empty inventory |
| `crates/core-process` | Standalone admission claims and a tested Windows atomic job primitive; production session integration remains open |
| `crates/core-runtime` | Shared planning, connection/version negotiation and diagnostics |
| `crates/core-cli` | Standalone CLI and bounded inherited-stdio service |
| `contracts` | Generated canonical schemas, fixtures and manager theme tokens |
| `upstream` | Per-platform source pins, minimal branding patch and unreviewed observations |
| `scripts` | Build, dependency review, source feasibility and external conformance tools |

Process supervision, proxy transport, native automation gateways, trusted updating and actual browser GUI remain implementation gates. Empty crates are not used to imply these features exist.

## Validate

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python -m pip install -r requirements-dev.txt
python scripts/export-contracts.py --binary target/debug/numera-core --check
python scripts/check-contracts.py --binary target/debug/numera-core
python scripts/licenses.py --check
python scripts/security-audit.py
```

Windows executable paths end in `.exe`. Use the native CI matrix for Windows/Linux/macOS verification. [Protocol](contracts/PROTOCOL.md), [security](SECURITY.md), [updates](docs/adr/003-updates.md), [feasibility](docs/feasibility.md), and [manager checklist](docs/manager-integration.md) describe the current boundaries.

The external consumer also tests shutdown with a saturated stderr pipe. On Linux it compiles a small development fixture with the system `cc` to reproduce file replacement during opening; this fixture is never linked into the installed CLI.

[Bounded IPC fuzzing](fuzz/README.md) has its own pinned development toolchain,
dependency inventory and CI job. It does not add nightly Rust or libFuzzer to
the installed CLI or imply native browser security acceptance.
