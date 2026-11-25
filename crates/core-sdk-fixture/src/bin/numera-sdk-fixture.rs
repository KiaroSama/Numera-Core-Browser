use std::{
    env,
    ffi::OsString,
    path::Path,
    process::{Command, ExitCode, Stdio},
};

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.len() != 8
        || !args[..3]
            .iter()
            .all(|path| Path::new(path).is_absolute() && Path::new(path).is_file())
        || Path::new(&args[1]).extension().and_then(|ext| ext.to_str()) != Some("node")
    {
        eprintln!(
            "usage: numera-sdk-fixture NODE ADDON.node PACKAGE.json DRIVER BROWSER PROFILE FIXTURE REPORT"
        );
        return ExitCode::from(2);
    }

    let mut node = Command::new(&args[0]);
    node.arg("--require")
        .arg(&args[1])
        .args(["--eval", "", "--"])
        .args(&args[3..])
        .env("NUMERA_SDK_PACKAGE", &args[2])
        .env("NO_PROXY", "localhost,127.0.0.1")
        .stdin(Stdio::null());
    for (key, _) in env::vars_os() {
        if [
            "NODE_OPTIONS",
            "NODE_PATH",
            "NODE_USE_ENV_PROXY",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
        ]
        .iter()
        .any(|name| key.eq_ignore_ascii_case(name))
        {
            node.env_remove(key);
        }
    }
    launch(node)
}

#[cfg(unix)]
fn launch(mut node: Command) -> ExitCode {
    use std::os::unix::process::CommandExt;

    // Replace the adapter so the fixture supervisor observes Node directly.
    let error = node.exec();
    eprintln!("Could not start the pinned Node runtime: {error}");
    ExitCode::FAILURE
}

#[cfg(windows)]
fn launch(mut node: Command) -> ExitCode {
    // The native fixture supervisor terminates descendants on timeout.
    match node.status() {
        Ok(status) => ExitCode::from(
            status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1),
        ),
        Err(error) => {
            eprintln!("Could not start the pinned Node runtime: {error}");
            ExitCode::FAILURE
        }
    }
}
