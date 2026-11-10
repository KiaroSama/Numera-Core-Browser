# Bounded IPC fuzzing

This separate development workspace uses cargo-fuzz 0.13.2, libfuzzer-sys 0.4.13
and nightly-2026-11-10. It does not change the CLI's pinned stable toolchain or
runtime dependencies. The target exercises strict JSON, engine-manifest parsing,
bounded framing, up to 32 stateful inherited-stdio requests and response
serialization. Valid mutating/session operations are explicitly excluded, so
future installer/launch support cannot turn this target into a process launcher.
It never installs or launches an engine or probes the network.

```sh
cargo install cargo-fuzz --version '=0.13.2' --locked
python scripts/seed-fuzz.py
cargo +nightly-2026-11-10 fuzz run wire -- -max_total_time=60 -max_len=1048576 -rss_limit_mb=1024 -timeout=2 -seed=20261110
python scripts/licenses-fuzz.py --check
python scripts/security-audit.py --lock-file fuzz/Cargo.lock --output build/fuzz-security.json
```

Use an explicit nightly selection; do not change a user's global Rust default.
Local campaign scope is x86-64 Linux with AddressSanitizer and coverage guidance.
A bounded pass is not an exhaustive security audit, native browser sanitizer
campaign, Windows bridge validation or hostile-origin gateway test. Release
security acceptance remains open until those separate requirements pass.

Reference: https://rust-fuzz.github.io/book/cargo-fuzz/setup.html
