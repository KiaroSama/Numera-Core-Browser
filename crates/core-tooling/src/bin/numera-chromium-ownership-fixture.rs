use core_tooling::fixtures::{FixtureKind, run};

fn main() -> std::process::ExitCode {
    run(FixtureKind::Ownership)
}
