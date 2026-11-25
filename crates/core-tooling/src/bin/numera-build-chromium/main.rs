mod settings;

use core_tooling::sha256_file;
use serde_json::json;
use settings::{Options, Phase, SourceLock};
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn stage(root: &Path, phase: &str) -> Result<()> {
    fs::write(root.join("phase.txt"), phase)?;
    let timestamp = jiff::Timestamp::now().to_string();
    let mut phases = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("phases.ndjson"))?;
    writeln!(
        phases,
        "{}",
        json!({"timestamp": timestamp, "phase": phase})
    )?;
    log("INFO", &format!("Build phase: {phase}"));
    Ok(())
}

fn log(level: &str, message: &str) {
    eprintln!(
        "{}",
        json!({
            "timestamp": jiff::Timestamp::now().to_string(), "severity": level,
            "component": "chromium-source", "message": message
        })
    );
}

struct Native {
    environment: BTreeMap<OsString, OsString>,
    depot: PathBuf,
}

impl Native {
    fn command(&self, program: impl AsRef<OsStr>, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command.current_dir(cwd).env_clear().envs(&self.environment);
        command
    }

    fn checked(
        &self,
        program: impl AsRef<OsStr>,
        cwd: &Path,
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> Result<()> {
        let program = program.as_ref();
        let status = self.command(program, cwd).args(args).status()?;
        if !status.success() {
            return Err(format!("{} failed ({status})", program.to_string_lossy()).into());
        }
        Ok(())
    }

    fn tool(&self, name: &str, cwd: &Path, args: &[&str]) -> Result<()> {
        let depot_tool = self.depot.join(format!("{name}.bat"));
        if cfg!(windows) && depot_tool.is_file() {
            self.checked(depot_tool, cwd, args)
        } else {
            self.checked(name, cwd, args)
        }
    }

    fn revision(&self, dir: &Path) -> Result<String> {
        let result = self
            .command("git", dir)
            .args(["rev-parse", "HEAD"])
            .output()?;
        if !result.status.success() {
            return Err("Could not verify Git revision".into());
        }
        Ok(std::str::from_utf8(&result.stdout)?.trim().to_owned())
    }
}

fn bootstrap(native: &mut Native, root: &Path, repository: &Path, lock: &SourceLock) -> Result<()> {
    let depot = native.depot.clone();
    if depot.exists() {
        return Err("Cold spike requires a fresh task-owned source directory".into());
    }
    native.checked("git", repository, [OsStr::new("init"), depot.as_os_str()])?;
    native.checked(
        "git",
        &depot,
        ["remote", "add", "origin", &lock.depot_tools_url],
    )?;
    native.checked(
        "git",
        &depot,
        ["fetch", "--depth=1", "origin", &lock.depot_tools_revision],
    )?;
    native.checked("git", &depot, ["checkout", "--detach", "FETCH_HEAD"])?;

    let path = native
        .environment
        .get(OsStr::new("PATH"))
        .ok_or("PATH is required")?;
    let path = env::join_paths(std::iter::once(depot.clone()).chain(env::split_paths(path)))?;
    native.environment.insert("PATH".into(), path);
    native
        .environment
        .insert("DEPOT_TOOLS_UPDATE".into(), "0".into());
    native
        .environment
        .insert("DEPOT_TOOLS_WIN_TOOLCHAIN".into(), "0".into());
    if cfg!(windows) {
        let config = root.join("build.gitconfig");
        fs::write(
            &config,
            "[core]\n autocrlf = false\n longpaths = true\n preloadindex = true\n fscache = true\r\n",
        )?;
        native
            .environment
            .insert("GIT_CONFIG_GLOBAL".into(), config.into_os_string());
        if let Some(program_files) = env::var_os("ProgramFiles(x86)") {
            let vswhere =
                PathBuf::from(program_files).join("Microsoft Visual Studio/Installer/vswhere.exe");
            if vswhere.is_file() {
                let result = native
                    .command(vswhere, repository)
                    .args([
                        "-latest",
                        "-products",
                        "*",
                        "-requires",
                        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                        "-property",
                        "installationPath",
                    ])
                    .output()?;
                let location = std::str::from_utf8(&result.stdout)?.trim();
                if result.status.success() && !location.is_empty() {
                    native
                        .environment
                        .insert("vs2026_install".into(), location.into());
                }
            }
        }
        for script in ["cipd_bin_setup.bat", "bootstrap/win_tools.bat"] {
            native.checked(depot.join(script), repository, std::iter::empty::<&str>())?;
        }
    } else {
        stage(root, "pinned-python-bootstrap")?;
        native.checked("bash", repository, [depot.join("ensure_bootstrap")])?;
    }
    if !depot.join("python3_bin_reldir.txt").is_file() {
        return Err("Pinned Python bootstrap did not create its launcher marker".into());
    }
    let python = if cfg!(windows) {
        "python-bin/python3.bat"
    } else {
        "python-bin/python3"
    };
    native.checked(depot.join(python), repository, ["--version"])?;
    if native.revision(&depot)? != lock.depot_tools_revision {
        return Err("Build-tool revision changed during bootstrap".into());
    }
    Ok(())
}

fn run(options: Options) -> Result<()> {
    let environment: BTreeMap<_, _> = env::vars_os().collect();
    let named = |name: &str, value: &str| {
        environment
            .get(OsStr::new(name))
            .is_some_and(|item| item == value)
    };
    if !named("GITHUB_ACTIONS", "true")
        || !named("RUNNER_ENVIRONMENT", "github-hosted")
        || !named("GITHUB_REPOSITORY", "KiaroSama/Numera-Core-Browser")
    {
        return Err(
            "Engine compilation requires the authorized public GitHub-hosted repository runner"
                .into(),
        );
    }
    if !options.root.is_absolute()
        || options
            .root
            .to_string_lossy()
            .chars()
            .any(char::is_whitespace)
    {
        return Err("Provide an absolute scratch source path without spaces".into());
    }
    let host = env::consts::OS;
    if !matches!(host, "windows" | "linux") || !named("RUNNER_ARCH", "X64") {
        return Err("Chromium source builds require an x86-64 Windows or Linux runner".into());
    }
    let repository = env::current_dir()?;
    let lock_path = match &options.reviewed_lock {
        Some(path)
            if options.probe && path.is_absolute() && named("GITHUB_REF", "refs/heads/main") =>
        {
            path.clone()
        }
        Some(_) => {
            return Err(
                "A stable candidate override is restricted to a graph probe on main".into(),
            );
        }
        None => repository.join("upstream/chromium/lock.json"),
    };
    let lock: SourceLock = serde_json::from_slice(&fs::read(lock_path)?)?;
    let pin = lock
        .platform_pins
        .get(host)
        .ok_or("No reviewed source pin for host platform")?;
    fs::create_dir_all(&options.root)?;
    if cfg!(windows) {
        stage(&options.root, "windows-sdk-preparation")?;
        let sdk = env::current_exe()?.with_file_name("numera-install-windows-sdk.exe");
        let status = Command::new(sdk)
            .arg("--evidence-root")
            .arg(&options.root)
            .status()?;
        if !status.success() {
            return Err("Windows SDK preparation failed".into());
        }
        if options.phase == Phase::Prerequisites {
            return stage(&options.root, "prerequisites-complete");
        }
    }

    let mut native = Native {
        environment,
        depot: options.root.join("depot_tools"),
    };
    stage(&options.root, "build-tool-acquisition")?;
    bootstrap(&mut native, &options.root, &repository, &lock)?;
    if options.phase == Phase::Prerequisites {
        return stage(&options.root, "prerequisites-complete");
    }

    let checkout = options.root.join("checkout");
    fs::create_dir_all(&checkout)?;
    stage(&options.root, "source-acquisition")?;
    native.tool(
        "gclient",
        &checkout,
        &["config", "--unmanaged", &lock.upstream_url],
    )?;
    native.tool(
        "gclient",
        &checkout,
        &[
            "sync",
            "--no-history",
            "--jobs=2",
            "--revision",
            &format!("src@{}", pin.revision),
        ],
    )?;
    let source = checkout.join("src");
    if native.revision(&source)? != pin.revision {
        return Err("Upstream source revision mismatch".into());
    }
    stage(&options.root, "patch-application")?;
    for patch in lock.patches {
        let file = repository.join(&patch.path);
        if sha256_file(&file)? != patch.sha256 {
            return Err("Patch hash mismatch".into());
        }
        native.checked(
            "git",
            &source,
            [OsStr::new("apply"), OsStr::new("--check"), file.as_os_str()],
        )?;
        native.checked("git", &source, [OsStr::new("apply"), file.as_os_str()])?;
    }
    if options.phase == Phase::Acquire {
        return Ok(());
    }
    generate(&options, &native, &source)
}

fn generate(options: &Options, native: &Native, source: &Path) -> Result<()> {
    if cfg!(target_os = "linux") {
        stage(&options.root, "linux-build-dependencies")?;
        native.checked(
            "sudo",
            source,
            [
                "-n",
                "--",
                "python3",
                "build/install-build-deps.py",
                "--no-prompt",
                "--no-android",
                "--no-arm",
                "--no-chromeos-fonts",
                "--no-backwards-compatible",
                "--no-syms",
            ],
        )?;
    }
    stage(&options.root, "gn-generation")?;
    let mut args = format!(
        "is_debug={} is_component_build={} symbol_level=0 blink_symbol_level=0 v8_symbol_level=0 use_remoteexec=false is_official_build=false is_chrome_branded=false proprietary_codecs=false ffmpeg_branding=\"Chromium\" target_cpu=\"x64\"",
        options.debug, options.component
    );
    if options.probe {
        args.push_str(&format!(
            " use_siso=false cc_wrapper=\"{}\" use_clang_modules=false",
            options.wrapper
        ));
    }
    args.push_str(if cfg!(windows) { "\r\n" } else { "\n" });
    fs::write(options.root.join("gn-args.txt"), &args)?;
    let native_output = source.join("out/Numera");
    fs::create_dir_all(&native_output)?;
    fs::write(native_output.join("args.gn"), &args)?;
    native.tool("gn", source, &["gen", "out/Numera"])?;
    if options.phase == Phase::Generate {
        return stage(&options.root, "generate-complete");
    }
    stage(&options.root, "compile-link")?;
    let workers = options.workers.to_string();
    let mut targets = vec!["-C", "out/Numera", "-j", &workers, "chrome"];
    if options.driver {
        targets.push("chromedriver");
    }
    if cfg!(target_os = "linux") {
        targets.push("chrome_sandbox");
    }
    native.tool("autoninja", source, &targets)?;
    stage(&options.root, "compile-complete")?;
    log(
        "INFO",
        "Compilation completed; native regression, source packaging and release approval are still required",
    );
    Ok(())
}

fn main() -> ExitCode {
    match Options::parse().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log("ERROR", &error.to_string());
            ExitCode::FAILURE
        }
    }
}
