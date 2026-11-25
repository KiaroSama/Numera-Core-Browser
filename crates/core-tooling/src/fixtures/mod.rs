//! Native Chromium development fixtures and portable runtime evidence.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    error::Error,
    ffi::{OsStr, OsString},
    path::Path,
};

pub mod bundle;
pub mod headed;
pub mod ownership;
pub mod portable;
pub mod profile;
mod support;
pub mod webdriver;

pub use bundle::{read_source_export, retain_development_bundle};
pub use headed::smoke_headed_browser;
pub use ownership::smoke_browser_ownership;
pub use portable::{describe_portable_runtime, smoke_portable_runtime};
pub use profile::smoke_profile_data;
pub use support::{install_linux_sandbox, smoke_renderer, timed as bounded_child};
pub use webdriver::smoke_driver;

pub type Environment = BTreeMap<OsString, OsString>;
pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy)]
pub enum FixtureKind {
    Driver,
    Headed,
    Ownership,
    Portable,
    Profile,
    Bundle,
}

fn call(kind: FixtureKind, args: &[OsString], environment: &Environment) -> Result<Value> {
    let path = |index: usize| Path::new(&args[index]);
    let current_host = if cfg!(windows) { "Windows" } else { "Linux" };
    match (kind, args) {
        (FixtureKind::Driver, [_, _, _, _]) => {
            smoke_driver(path(0), path(1), path(2), path(3), environment, false)
        }
        (FixtureKind::Driver, [_, _, _, _, flag]) if flag == "--headed" => {
            smoke_driver(path(0), path(1), path(2), path(3), environment, true)
        }
        (FixtureKind::Headed, [mode, _, _, _, _]) if mode == "under-x11" => {
            let value = smoke_driver(path(1), path(2), path(3), path(4), environment, true)?;
            support::write_json(&path(4).join("report.json"), &value)?;
            Ok(value)
        }
        (FixtureKind::Headed, [_, _, _, _]) => {
            smoke_headed_browser(path(0), path(1), path(2), path(3), environment)
        }
        (FixtureKind::Ownership, [mode, url]) if mode == "file-url" => {
            Ok(json!(ownership::local_file_url_path(support::utf8(url)?)))
        }
        (FixtureKind::Ownership, [_, _, _]) => {
            smoke_browser_ownership(path(0), path(1), path(2), environment)
        }
        (FixtureKind::Profile, [_, _, _, _]) => {
            smoke_profile_data(path(0), path(1), path(2), path(3), environment)
        }
        (FixtureKind::Portable, [mode, name]) if mode == "runtime-path" => Ok(json!(
            portable::runtime_dependency_path(support::utf8(name)?)?
        )),
        (FixtureKind::Portable, [mode, _]) if mode == "describe" => {
            describe_portable_runtime(path(1), current_host, environment)
        }
        (FixtureKind::Portable, [_, _, _]) => {
            smoke_portable_runtime(path(0), path(1), path(2), current_host, environment)
        }
        (FixtureKind::Bundle, [mode, filename, host, digest]) if mode == "source-export" => {
            let (file, proof) = read_source_export(
                support::utf8(filename)?,
                support::utf8(host)?,
                support::utf8(digest)?,
            )?;
            Ok(json!({"file":file,"proof":proof}))
        }
        (FixtureKind::Bundle, [_, _, _, filename, host, digest]) => {
            let proof = read_source_export(
                support::utf8(filename)?,
                support::utf8(host)?,
                support::utf8(digest)?,
            )?;
            let report: Value = support::read_json(path(2))?;
            retain_development_bundle(path(0), path(1), &report, &proof, environment)
        }
        _ => Err(support::failure("Invalid browser fixture arguments")),
    }
}

pub fn run(kind: FixtureKind) -> std::process::ExitCode {
    let environment = std::env::vars_os()
        .filter(|(key, _)| {
            ![
                "GH_TOKEN",
                "GITHUB_TOKEN",
                "ACTIONS_RUNTIME_TOKEN",
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                "ACTIONS_ID_TOKEN_REQUEST_URL",
            ]
            .iter()
            .any(|secret| key.eq_ignore_ascii_case(OsStr::new(secret)))
        })
        .collect();
    match call(
        kind,
        &std::env::args_os().skip(1).collect::<Vec<_>>(),
        &environment,
    ) {
        Ok(result) => {
            println!("{result}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::from(2)
        }
    }
}
