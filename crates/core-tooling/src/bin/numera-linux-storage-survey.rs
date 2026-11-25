#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    match linux::run() {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("The hosted storage survey requires Linux");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
mod linux {
    use core_tooling::chromium_build::{process, report};
    use rustix::fs::{Access, access};
    use serde_json::{Value, json};
    use std::{
        env, fs, io,
        os::unix::fs::MetadataExt,
        path::{Path, PathBuf},
        process::Command,
        time::Duration,
    };

    pub fn run() -> io::Result<bool> {
        let commit = required_identity("GITHUB_SHA")?;
        let run = required_identity("GITHUB_RUN_ID")?;
        let mut measurements = Vec::new();
        for candidate in ["GITHUB_WORKSPACE", "RUNNER_TEMP", "/mnt", "/tmp"] {
            let path =
                env::var_os(candidate).map_or_else(|| PathBuf::from(candidate), PathBuf::from);
            measurements.push(measure(candidate, &path)?);
        }
        let sdk = measure_sdk(Path::new("/usr/local/lib/android/sdk"))?;
        let success = !matches!(
            sdk["measurement_result"].as_str(),
            Some("TIMEOUT" | "BLOCKED")
        );
        let evidence = json!({
            "schema_revision": 1,
            "source_commit": commit,
            "run_id": run,
            "image_version": env::var("ImageVersion").ok(),
            "measurements": measurements,
            "optional_preinstalled_android_sdk": sdk,
            "engine_acceptance": "NOT_RUN",
            "note": "Read-only filesystem observations. Repeated device IDs share capacity; this is not proof of combined build fit or permission to clean installed tools.",
        });
        fs::create_dir_all("build")?;
        let output = Path::new("build/linux-storage-survey");
        fs::create_dir(output)?;
        report::write_json(&output.join("report.json"), &evidence)?;
        println!("{}", serde_json::to_string_pretty(&evidence)?);
        Ok(success)
    }

    fn required_identity(key: &str) -> io::Result<String> {
        env::var(key).map_err(|_| {
            io::Error::other(format!("Missing observation environment variable {key}"))
        })
    }

    fn measure(name: &str, path: &Path) -> io::Result<Value> {
        let Ok(metadata) = fs::metadata(path) else {
            return Ok(json!({"candidate": name, "available": false}));
        };
        if !metadata.is_dir() {
            return Ok(json!({"candidate": name, "available": false}));
        }
        let disk = process::disk(path)?;
        Ok(json!({
            "candidate": name,
            "path": fs::canonicalize(path)?,
            "device_id": metadata.dev(),
            "total_bytes": disk.total,
            "free_bytes": disk.free,
            "write_access_reported": access(path, Access::WRITE_OK).is_ok(),
        }))
    }

    fn measure_sdk(path: &Path) -> io::Result<Value> {
        let exists = path.is_dir();
        let symlink = path.is_symlink();
        let mut sdk = json!({
            "path": path,
            "exists": exists,
            "symlink": symlink,
            "resolved_path": path.canonicalize().unwrap_or_else(|_| path.to_owned()),
            "android_home": env::var("ANDROID_HOME").ok(),
            "measured_bytes": null,
            "measurement_result": "NOT_PRESENT",
        });
        if exists && !symlink {
            let mut command = Command::new("du");
            command.args(["-sx", "--block-size=1", "--"]).arg(path);
            match process::capture(&mut command, Duration::from_secs(120), 16_384) {
                Ok(result) => {
                    sdk["du_exit"] = json!(result.status.code());
                    sdk["measurement_result"] = json!("BLOCKED");
                    if result.status.success()
                        && let Some(bytes) = std::str::from_utf8(&result.stdout)
                            .ok()
                            .and_then(|output| output.split_whitespace().next())
                            .and_then(|word| word.parse::<u64>().ok())
                    {
                        sdk["measured_bytes"] = json!(bytes);
                        sdk["measurement_result"] = json!("PASS");
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                    sdk["measurement_result"] = json!("TIMEOUT")
                }
                Err(_) => sdk["measurement_result"] = json!("BLOCKED"),
            }
        }
        Ok(sdk)
    }

    #[cfg(test)]
    mod tests {
        use super::{measure, measure_sdk};

        #[test]
        fn checks_alias_volumes_without_writing_into_them() {
            let root = tempfile::tempdir().unwrap();
            let alias = root.path().join("fixture-alias");
            std::os::unix::fs::symlink(root.path(), &alias).unwrap();
            let target = measure("target", root.path()).unwrap();
            let observed = measure("alias", &alias).unwrap();
            assert_eq!(observed["device_id"], target["device_id"]);
            assert_eq!(observed["path"], target["path"]);
            assert_eq!(
                measure_sdk(&alias).unwrap()["measurement_result"],
                "NOT_PRESENT"
            );
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }
}
