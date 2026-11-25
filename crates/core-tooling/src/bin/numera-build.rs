use serde_json::json;
use std::env;
use std::io;
use std::path::Path;
use std::process::{Command, ExitCode};

fn cargo(root: &Path, arguments: &[&str]) -> io::Result<()> {
    let status = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(arguments)
        .current_dir(root)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "cargo {} failed ({status})",
            arguments[0]
        )))
    }
}

fn run() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let mode = match args.next() {
        None => "check".to_owned(),
        Some(flag) if flag == "--mode" => args
            .next()
            .ok_or_else(|| io::Error::other("Usage: numera-build [--mode check|build|doctor]"))?,
        _ => {
            return Err(io::Error::other(
                "Usage: numera-build [--mode check|build|doctor]",
            ));
        }
    };
    if args.next().is_some() {
        return Err(io::Error::other("Unexpected build arguments"));
    }
    let current = env::current_dir()?;
    let root = current
        .ancestors()
        .find(|path| {
            path.join("rust-toolchain.toml").is_file() && path.join("crates/core-cli").is_dir()
        })
        .ok_or_else(|| io::Error::other("Run the build command inside the Numera Core checkout"))?;
    match mode.to_ascii_lowercase().as_str() {
        "check" => {
            cargo(root, &["fmt", "--all", "--", "--check"])?;
            cargo(
                root,
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ],
            )?;
            cargo(root, &["test", "--workspace", "--locked"])
        }
        "build" => cargo(root, &["build", "--release", "--locked", "-p", "core-cli"]),
        "doctor" => cargo(
            root,
            &[
                "run", "--locked", "-q", "-p", "core-cli", "--", "--json", "doctor",
            ],
        ),
        _ => Err(io::Error::other(
            "Build mode must be check, build, or doctor",
        )),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                json!({
                    "timestamp": jiff::Timestamp::now().to_string(),
                    "severity": "ERROR",
                    "component": "build",
                    "message": error.to_string(),
                })
            );
            ExitCode::FAILURE
        }
    }
}
