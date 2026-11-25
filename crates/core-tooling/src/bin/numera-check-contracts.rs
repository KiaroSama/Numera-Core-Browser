#[path = "numera-check-contracts/checker.rs"]
mod checker;
#[path = "numera-package-cli/common.rs"]
mod common;

use std::{env, process::ExitCode};

fn main() -> ExitCode {
    common::cli(
        "usage: numera-check-contracts --binary BINARY",
        &["--binary"],
        &[],
        |args| checker::run(&args.path("--binary")?, &env::current_dir()?),
    )
}
