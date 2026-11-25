use super::{GuardedChild, validate};
use crate::common::{Result, checked};
use jsonschema::Validator;
use rustix::fs::{CWD, Mode, mkfifoat};
use serde_json::Value;
use std::{
    env, fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(super) fn fifo_replacement(
    binary: &Path,
    checkout: &Path,
    snapshot: &Value,
    response: &Validator,
) -> Result<()> {
    let root = tempfile::Builder::new()
        .prefix("Numera-file-race-")
        .tempdir()?;
    let fixture = root.path().join("open_barrier.rs");
    fs::write(&fixture, include_bytes!("open_barrier.rs"))?;
    let library = root.path().join("open-barrier.so");
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    checked(
        Command::new(rustc)
            .args([
                "--edition=2024",
                "--crate-type",
                "cdylib",
                "-D",
                "warnings",
                "-o",
            ])
            .arg(&library)
            .arg(&fixture),
        30,
    )?;

    let cases = [
        ("plan", vec!["plan"], serde_json::to_vec(snapshot)?),
        (
            "manifest",
            vec!["manifest", "inspect"],
            fs::read(checkout.join("contracts/fixtures/engine-manifest.json"))?,
        ),
        ("hash", vec!["hash-file"], b"hello".to_vec()),
    ];
    for (name, command, contents) in cases {
        let scratch = root.path().join(name);
        fs::create_dir(&scratch)?;
        let selected = scratch.join("selected-input");
        fs::write(&selected, contents)?;
        let baseline = checked(Command::new(binary).args(&command).arg(&selected), 5)?;
        if name == "hash"
            && serde_json::from_slice::<Value>(&baseline.stdout)?["sha256"]
                != "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        {
            return Err("Digest command did not accept its regular input control".into());
        }

        let ready = scratch.join("ready");
        let resume = scratch.join("resume");
        let mut raced = GuardedChild(
            Command::new(binary)
                .args(&command)
                .arg(&selected)
                .env("LD_PRELOAD", &library)
                .env("NUMERA_TEST_OPEN_TARGET", &selected)
                .env("NUMERA_TEST_OPEN_READY", &ready)
                .env("NUMERA_TEST_OPEN_RESUME", &resume)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.try_exists()? {
            if raced.0.try_wait()?.is_some() || Instant::now() >= deadline {
                return Err(format!(
                    "Native producer did not reach its file-open barrier for {name}"
                )
                .into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        fs::remove_file(&selected)?;
        mkfifoat(CWD, &selected, Mode::RUSR | Mode::WUSR)?;
        fs::File::create_new(resume)?;
        let status = raced.wait(Duration::from_secs(3))?;
        let mut bytes = Vec::new();
        raced
            .0
            .stdout
            .take()
            .ok_or("Missing file-race output pipe")?
            .read_to_end(&mut bytes)?;
        let frame = serde_json::from_slice(&bytes)?;
        validate(response, &frame)?;
        if status.code() != Some(2) || frame["error"]["code"] != "invalid_configuration" {
            return Err(format!("Native producer accepted a FIFO substituted for {name}").into());
        }
    }
    Ok(())
}
