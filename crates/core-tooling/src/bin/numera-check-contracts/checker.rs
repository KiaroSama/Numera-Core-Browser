#[path = "pipe_fill.rs"]
mod pipe_fill;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "race.rs"]
mod race;

use crate::common::{Result, capture, info};
use jsonschema::{Validator, draft202012};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn json_file(root: &Path, filename: &str) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(root.join(filename))?)?)
}

fn validate(validator: &Validator, value: &Value) -> Result<()> {
    validator
        .validate(value)
        .map_err(|error| error.to_string().into())
}

pub fn run(binary: &Path, root: &Path) -> Result<()> {
    let binary = binary.canonicalize()?;
    let root = root.canonicalize()?;
    let request = json_file(&root, "contracts/v1/request.schema.json")?;
    let response = json_file(&root, "contracts/v1/response.schema.json")?;
    for schema in [&request, &response] {
        draft202012::meta::validate(schema).map_err(|error| error.to_string())?;
    }
    let request = draft202012::new(&request)?;
    let response = draft202012::new(&response)?;
    let hello = json_file(&root, "contracts/fixtures/hello.json")?;
    let capabilities = json_file(&root, "contracts/fixtures/capabilities.json")?;
    validate(&request, &hello)?;
    validate(&request, &capabilities)?;

    let namespace = json_file(&root, "contracts/fixtures/negative-namespace.json")?;
    let mut bad_field = hello.clone();
    bad_field["unknown_required"] = json!(true);
    let mut bad_payload = capabilities.clone();
    bad_payload["payload"] = json!({"unrecognized": true});
    for bad in [&namespace, &bad_field, &bad_payload] {
        if request.is_valid(bad) {
            return Err("Request schema accepted an invalid conformance fixture".into());
        }
    }

    let mut snapshot = json_file(&root, "contracts/fixtures/launch-desktop.json")?;
    let native = root.join("build/never-create-this-profile");
    for grant in ["native_profile_dir", "data_root", "cache_root"] {
        snapshot["resources"][grant] = json!(native.to_str().ok_or("Invalid native profile path")?);
    }
    let mut launch = hello.clone();
    launch["id"] = json!("launch-1");
    launch["operation"] = json!("session.launch");
    launch["payload"] = snapshot.clone();
    validate(&request, &launch)?;
    let inputs = [
        hello,
        capabilities,
        launch,
        bad_field,
        bad_payload,
        namespace,
    ];
    let mut input = Vec::new();
    for frame in &inputs {
        serde_json::to_writer(&mut input, frame)?;
        input.push(b'\n');
    }
    let output = capture(
        Command::new(&binary).arg("serve").current_dir(&root),
        Some(input),
        Duration::from_secs(15),
    )?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err("Native producer failed the NCB-1 conversation".into());
    }
    let frames = std::str::from_utf8(&output.stdout)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<serde_json::Result<Vec<_>>>()?;
    if frames.len() != inputs.len() {
        return Err("Native producer returned unexpected frame count".into());
    }
    for frame in &frames {
        validate(&response, frame)?;
    }
    if frames[..2].iter().any(|frame| frame["status"] != "ok")
        || frames[2]["error"]["code"] != "missing_package"
        || frames[3..].iter().any(|frame| frame["status"] != "error")
        || native.exists()
    {
        return Err("Native producer returned unexpected conformance outcomes".into());
    }

    saturated_stderr(&binary, &response)?;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    race::fifo_replacement(&binary, &root, &snapshot, &response)?;

    info(
        "PASS: external consumer/schema fixtures and real producer agree; real-browser integration remains NOT_RUN",
    );
    Ok(())
}

struct GuardedChild(Child);

impl GuardedChild {
    fn wait(&mut self, duration: Duration) -> Result<std::process::ExitStatus> {
        let deadline = Instant::now() + duration;
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err("Native producer exceeded the conformance deadline".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for GuardedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn saturated_stderr(binary: &Path, response: &Validator) -> Result<()> {
    let (_reader, mut writer) = io::pipe()?;
    pipe_fill::saturate(&mut writer)?;

    let mut process = GuardedChild(
        Command::new(binary)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(writer)
            .spawn()?,
    );
    let mut stdin = process
        .0
        .stdin
        .take()
        .ok_or("Missing conformance input pipe")?;
    stdin.write_all(b"{}")?;
    drop(stdin);
    let status = process.wait(Duration::from_secs(5))?;
    use std::io::Read as _;
    let mut bytes = Vec::new();
    process
        .0
        .stdout
        .take()
        .ok_or("Missing conformance output pipe")?
        .read_to_end(&mut bytes)?;
    let frame = serde_json::from_slice(&bytes)?;
    validate(response, &frame)?;
    if status.code() != Some(2) || frame["error"]["code"] != "invalid_configuration" {
        return Err("Native producer did not complete with a saturated diagnostic pipe".into());
    }
    Ok(())
}
