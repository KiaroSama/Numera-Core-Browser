#[path = "numera-package-cli/common.rs"]
mod common;
#[path = "numera-package-cli/stage.rs"]
mod stage;

use std::process::ExitCode;

fn main() -> ExitCode {
    common::cli(
        "usage: numera-package-cli --binary BINARY [--launcher LAUNCHER] --output DIRECTORY --target RUNNER",
        &["--binary", "--launcher", "--output", "--target"],
        &[],
        |args| stage::run(args).map(|_| ()),
    )
}
