#[path = "numera-package-cli/common.rs"]
mod common;
#[path = "numera-package-cli/stage.rs"]
mod stage;

use common::{Arguments, Result, info};
use std::{env, io::Write, process::ExitCode};
use zip::ZipArchive;

fn main() -> ExitCode {
    common::cli(
        "usage: numera-verify-cli-package --binary BINARY [--launcher LAUNCHER] --output DIRECTORY --target RUNNER",
        &["--binary", "--launcher", "--output", "--target"],
        &[],
        verify,
    )
}

fn verify(args: Arguments) -> Result<()> {
    let root = env::current_dir()?.canonicalize()?;
    let canaries = [("contracts", ".txt"), ("licenses/dependencies", ".tmp")]
        .into_iter()
        .map(|(directory, suffix)| {
            let mut file = tempfile::Builder::new()
                .prefix(".local-only-package-")
                .suffix(suffix)
                .tempfile_in(root.join(directory))?;
            file.write_all(b"Synthetic local-only packaging canary; not a credential.\n")?;
            Ok(file)
        })
        .collect::<Result<Vec<_>>>()?;
    let package = stage::run(args)?;
    let (archive_names, source) = stage::archive_names_and_source(&package)?;
    let mut source = ZipArchive::new(source)?;
    for file in &canaries {
        let relative = file
            .path()
            .strip_prefix(&root)?
            .to_str()
            .ok_or("Invalid filename")?
            .replace('\\', "/");
        if archive_names.contains(&format!("development-cli/{relative}")) {
            return Err("Local-only files entered the runtime bundle".into());
        }
        if source.by_name(&relative).is_ok() {
            return Err("Local-only files entered the source bundle".into());
        }
    }

    info("PASS: untracked local data excluded from runtime and matching source");
    Ok(())
}
