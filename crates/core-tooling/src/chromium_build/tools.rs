//! Download reviewed native tools and generate the original Chromium build graph.
use super::{
    cache::{PROBE_CONFIG, STAGED_CONFIG, sha256},
    metadata::Host,
    process::{self, Environment},
};
use flate2::read::GzDecoder;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const EXTRACT_LIMIT: u64 = 32 * 1024 * 1024;

struct Release {
    url: &'static str,
    sha256: &'static str,
    size: u64,
    binary: &'static str,
}

const CCACHE: Release = Release {
    url: "https://github.com/ccache/ccache/releases/download/v4.14.3/ccache-4.14.3-linux-x86_64-musl-static.tar.gz",
    sha256: "e4583fd653b794a480b9915358679a75cdd7a7010dd865eaf4cbdc61e665a3f0",
    size: 2_122_678,
    binary: "ccache",
};
const SCCACHE: Release = Release {
    url: "https://github.com/mozilla/sccache/releases/download/v0.19.0/sccache-v0.19.0-x86_64-pc-windows-msvc.tar.gz",
    sha256: "130e4f8eb672e76590e9993c2ab890c4763b88238577bb11d8062158c7698353",
    size: 9_830_016,
    binary: "sccache.exe",
};
const NINJA_LINUX: Release = Release {
    url: "https://github.com/ninja-build/ninja/releases/download/v1.13.2/ninja-linux.zip",
    sha256: "5749cbc4e668273514150a80e387a957f933c6ed3f5f11e03fb30955e2bbead6",
    size: 134_040,
    binary: "ninja",
};
const NINJA_WINDOWS: Release = Release {
    url: "https://github.com/ninja-build/ninja/releases/download/v1.13.2/ninja-win.zip",
    sha256: "07fc8261b42b20e71d1720b39068c2e14ffcee6396b76fb7a795fb460b78dc65",
    size: 291_570,
    binary: "ninja.exe",
};

pub struct Toolset {
    pub cache: PathBuf,
    pub ninja: PathBuf,
    pub cache_tool: &'static str,
}

pub fn download(
    url: &str,
    output: &Path,
    max_bytes: u64,
    environment: &Environment,
) -> io::Result<()> {
    let result = process::capture(
        process::command("curl", environment)
            .args([
                "--disable",
                "--location",
                "--fail",
                "--silent",
                "--show-error",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--max-time",
                "30",
                "--max-filesize",
                &max_bytes.to_string(),
                "--output",
            ])
            .arg(output)
            .arg(url),
        Duration::from_secs(32),
        1024 * 1024,
    )?;
    if !result.status.success() || fs::metadata(output)?.len() > max_bytes {
        return Err(io::Error::other("Pinned native tool download failed"));
    }
    Ok(())
}

fn read_member(mut reader: impl Read, size: u64, maximum: u64) -> io::Result<Vec<u8>> {
    if size > maximum {
        return Err(io::Error::other(
            "Native tool release member exceeds its extraction limit",
        ));
    }
    let mut output = Vec::new();
    reader.by_ref().take(size + 1).read_to_end(&mut output)?;
    if output.len() as u64 != size {
        return Err(io::Error::other(
            "Native tool release member has unexpected size",
        ));
    }
    Ok(output)
}

pub fn zip_binary(file: File, binary: &str) -> io::Result<Vec<u8>> {
    zip_binary_bounded(file, binary, EXTRACT_LIMIT)
}

pub fn zip_binary_bounded(file: File, binary: &str, maximum: u64) -> io::Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(file)?;
    let mut output = None;
    for index in 0..archive.len() {
        let member = archive.by_index(index)?;
        if !member.is_file() || member.name()?.rsplit('/').next() != Some(binary) {
            continue;
        }
        if output.is_some() {
            return Err(io::Error::other(
                "Native tool release contains duplicate executable members",
            ));
        }
        let size = member.size();
        output = Some(read_member(member, size, maximum)?);
    }
    output.ok_or_else(|| io::Error::other("Native tool release executable is missing"))
}

fn tar_binary(file: File, binary: &str) -> io::Result<Vec<u8>> {
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    let mut output = None;
    for member in archive.entries()? {
        let member = member?;
        if !member.header().entry_type().is_file()
            || member.path()?.file_name().and_then(|name| name.to_str()) != Some(binary)
        {
            continue;
        }
        if output.is_some() {
            return Err(io::Error::other(
                "Native tool release contains duplicate executable members",
            ));
        }
        let size = member.size();
        output = Some(read_member(member, size, EXTRACT_LIMIT)?);
    }
    output.ok_or_else(|| io::Error::other("Native tool release executable is missing"))
}

pub fn write_executable(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    File::create_new(path)?.write_all(contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn install(directory: &Path, release: &Release, environment: &Environment) -> io::Result<PathBuf> {
    let temporary = tempfile::NamedTempFile::new_in(directory)?;
    download(release.url, temporary.path(), release.size, environment)?;
    if temporary.path().metadata()?.len() != release.size
        || sha256(temporary.path())? != release.sha256
    {
        return Err(io::Error::other(format!(
            "Official {} transport verification failed",
            release.binary
        )));
    }
    let file = File::open(temporary.path())?;
    let contents = if release.url.ends_with(".zip") {
        zip_binary(file, release.binary)?
    } else {
        tar_binary(file, release.binary)?
    };
    let result = directory.join(release.binary);
    write_executable(&result, &contents)?;
    Ok(result)
}

pub fn install_tools(
    directory: &Path,
    host: Host,
    environment: &Environment,
) -> io::Result<Toolset> {
    let (cache, ninja, cache_tool) = if host == Host::Windows {
        (&SCCACHE, &NINJA_WINDOWS, "sccache")
    } else {
        (&CCACHE, &NINJA_LINUX, "ccache")
    };
    Ok(Toolset {
        cache: install(directory, cache, environment)?,
        ninja: install(directory, ninja, environment)?,
        cache_tool,
    })
}

pub fn compile_graph(
    root: &Path,
    environment: &Environment,
    output: &Path,
    cache_tool: &str,
    review_lock: Option<&Path>,
) -> io::Result<()> {
    if process::disk(
        root.parent()
            .ok_or_else(|| io::Error::other("Missing graph scratch parent"))?,
    )?
    .free
        < 80_000_000_000
    {
        return Err(io::Error::other(
            "Real-graph probe requires at least 80 GB free before acquisition",
        ));
    }
    let mut command = process::command(
        process::binary_sibling("numera-build-chromium")?,
        environment,
    );
    command.arg("--source-root").arg(root).args([
        "--phase",
        "Generate",
        "--compiler-cache-probe",
        "--probe-cache-tool",
        cache_tool,
        "--component-build",
        "--debug-build",
    ]);
    if let Some(lock) = review_lock {
        command.arg("--reviewed-graph-lock").arg(lock);
    }
    let mut child = process::stream_log(&mut command, &output.join("graph.log"))?;
    let deadline = Instant::now() + Duration::from_secs(40 * 60);
    let mut previous = String::new();
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(io::Error::other(
                    "Pinned Chromium graph preparation failed; inspect graph.log",
                ))
            };
        }
        if Instant::now() >= deadline
            || process::disk(root.parent().expect("checked before spawn"))?.free < 25_000_000_000
        {
            return Err(io::Error::other(
                "Real-graph probe reached its acquisition time or disk guard",
            ));
        }
        if let Ok(phase) = fs::read_to_string(root.join("phase.txt")) {
            let phase = phase.trim_start_matches('\u{feff}').trim();
            if !phase.is_empty()
                && phase != previous
                && phase
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
            {
                super::report::log(
                    "INFO",
                    "compiler-cache-probe",
                    &format!("Graph preparation: {phase}"),
                );
                previous = phase.to_owned();
            }
        }
        thread::sleep(Duration::from_secs(2));
    }
}

pub fn configure_cache(
    environment: &mut Environment,
    toolset: &Toolset,
    directory: &Path,
    entries: &Path,
    staged: bool,
) -> io::Result<PathBuf> {
    let config = directory.join("local-cache.conf");
    if toolset.cache_tool == "sccache" {
        fs::write(&config, [])?;
        environment.insert("SCCACHE_CONF".into(), config.clone().into());
        environment.insert("SCCACHE_DIR".into(), entries.as_os_str().into());
        environment.insert(
            "SCCACHE_CACHE_SIZE".into(),
            OsString::from(if staged { "4G" } else { "24M" }),
        );
        environment.insert("SCCACHE_SERVER_PORT".into(), "4234".into());
        environment.insert("SCCACHE_DIRECT".into(), "false".into());
    } else {
        fs::write(&config, if staged { STAGED_CONFIG } else { PROBE_CONFIG })?;
        environment.insert("CCACHE_DIR".into(), entries.as_os_str().into());
        environment.insert("CCACHE_CONFIGPATH".into(), config.clone().into());
        if staged {
            let temporary = directory.join("temp");
            fs::create_dir(&temporary)?;
            environment.insert("CCACHE_TEMPDIR".into(), temporary.into());
        }
    }
    Ok(config)
}

pub fn cache_call(
    toolset: &Toolset,
    environment: &Environment,
    arguments: &[&str],
) -> io::Result<Vec<u8>> {
    process::stdout(
        process::command(&toolset.cache, environment).args(arguments),
        Duration::from_secs(120),
        1024 * 1024,
    )
}

pub fn cache_runtime<T>(
    toolset: &Toolset,
    env: &Environment,
    execute: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    if toolset.cache_tool == "ccache" {
        return execute();
    }
    cache_call(toolset, env, &["--start-server"])?;
    let outcome = execute();
    let stop = cache_call(toolset, env, &["--stop-server"]);
    let output = outcome?;
    stop?;
    Ok(output)
}

pub fn sccache_count(stats: &Value, key: &str) -> u64 {
    stats[key]["counts"]
        .as_object()
        .into_iter()
        .flat_map(|counts| counts.values())
        .filter_map(Value::as_u64)
        .sum()
}

pub fn statistics(toolset: &Toolset, environment: &Environment) -> io::Result<Value> {
    if toolset.cache_tool == "ccache" {
        let raw = cache_call(toolset, environment, &["--print-stats", "--format=json"])?;
        return Ok(serde_json::from_slice(&raw)?);
    }
    let raw = cache_call(
        toolset,
        environment,
        &["--show-stats", "--stats-format=json"],
    )?;
    let result: Value = serde_json::from_slice(&raw)?;
    if !result["cache_location"]
        .as_str()
        .is_some_and(|location| location.starts_with("Local disk: "))
        || result["max_cache_size"].as_u64() != Some(4 * 1024_u64.pow(3))
    {
        return Err(io::Error::other(
            "The staged sccache server did not confirm the expected local storage policy",
        ));
    }
    let stats = &result["stats"];
    Ok(
        json!({ "preprocessed_cache_hit": sccache_count(stats, "cache_hits"),
        "local_storage_write": stats["cache_writes"],
        "cache_size_kibibyte": result["cache_size"].as_u64().unwrap_or(0) / 1024,
        "backend_statistics": stats }),
    )
}
