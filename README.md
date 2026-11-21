<p align="center"><img src="assets/logo.svg" width="240" height="240" alt="Numera Core Browser"></p>

# Numera Core Browser

An independent browser runtime for [Numera Browser](https://github.com/KiaroSama/Numera-Browser), following the GoLogin/Orbita product boundary: the manager owns profiles; Core owns the browser runtime. The initial delivery targets an owned Chromium source build and retained browser chrome, with a shared Rust control plane.

**Development status.** Windows and Linux x86-64 are the active targets. The Rust control plane has passed native CI on both. On Linux, Chromium and ChromeDriver built from pinned source passed headless rendering, a raw W3C WebDriver session, and controlled Playwright, Puppeteer and Selenium tests. The Windows source build and Linux portable archive replay are in progress. Headed browser acceptance and end-user distribution remain open. [Owned Linux result](docs/evidence/chromium-replay-44910785784.json) · [Implementation report](docs/implementation-report.md) · [Scope](docs/scope.md) · [Original handoff](docs/original-build-specification.md)

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

The compiled CLI requires no Node.js, Python, display server or manager installation. Development tooling uses Python, and native automation fixtures also use Node.js. On Windows use PowerShell 7: `./scripts/Build.ps1 -Mode Check` or `-Mode Build`.

Successful native CI jobs produce `development-cli-*` artifacts with a `development-cli.tar.gz` bundle containing the tested CLI, notices and matching `complete-source.zip`. After extracting the artifact ZIP, extract the tar bundle with `tar -xzf development-cli.tar.gz`. In its `development-cli` directory run `./numera-core --json doctor` (PowerShell: `.\numera-core.exe --json doctor`). These unsigned development archives contain the control plane; browser/GUI and clean-machine installation acceptance remain open.

For Windows or Linux x86-64, select the corresponding `development-cli-*` artifact on a recent successful Windows and Linux native CI run. GitHub may require login, and each archive expires seven days after its run. Check the source commit in `development-manifest.json`. [Native CI runs](https://github.com/KiaroSama/Numera-Core-Browser/actions/workflows/rust.yml)

The `numera-core-contracts-v1-*` artifacts contain the versioned schemas, fixtures, theme tokens and conformance harness. The Windows and Linux jobs tested the extracted contract against their respective native CLIs and produced the same contract ZIP bytes. [Verified artifacts](docs/evidence/development-artifacts-44934635290.json) · [Contract changelog](contracts/CHANGELOG.md)

Successful `main` builds publish a `development-artifact-provenance` bundle. Verify the inner CLI tar or contract ZIP with `gh attestation verify`, the extracted bundle, and the expected workflow, ref and source commit. The evidence records exact subject digests and the signer workflow. [Provenance and downloads](docs/evidence/development-artifacts-44934635290.json) · [GitHub verification reference](https://cli.github.com/manual/gh_attestation_verify)

## Source layout

| Path | Implemented responsibility |
| --- | --- |
| `crates/core-contracts` | Typed NCB-1 messages, strict JSON, schemas and NDJSON bounds |
| `crates/core-identity` | Coherence validation and deterministic identity keys; no browser overrides |
| `crates/core-engines` | Package-manifest validation, hashing and truthful empty inventory |
| `crates/core-process` | Standalone admission claims and native ownership primitives for Windows jobs and dedicated Linux workers |
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

Windows executable paths end in `.exe`. Use the native CI matrix for Windows and Linux verification. [Protocol](contracts/PROTOCOL.md) · [Security](SECURITY.md) · [Updates](docs/adr/003-updates.md) · [Feasibility](docs/feasibility.md) · [Manager integration](docs/manager-integration.md)

The external consumer also tests shutdown with a saturated stderr pipe. On Linux it compiles a small development fixture with the system `cc` to reproduce file replacement during opening; this fixture is never linked into the installed CLI.

[Bounded IPC fuzzing](fuzz/README.md) has its own pinned development toolchain,
dependency inventory and CI job. It does not add nightly Rust or libFuzzer to
the installed CLI or imply native browser security acceptance.
