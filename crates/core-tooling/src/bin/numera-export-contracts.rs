#[path = "numera-package-cli/common.rs"]
mod common;

use common::{Arguments, Result, canonical_json, checked, cli, hash_bytes, hash_file, info};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    cli(
        "usage: numera-export-contracts --binary BINARY [--check]",
        &["--binary"],
        &["--check"],
        export,
    )
}

fn export(args: Arguments) -> Result<()> {
    let binary = args.path("--binary")?;
    let contracts = Path::new("contracts");
    let root = contracts.join("v1");
    let check = args.flag("--check");
    if !check {
        fs::create_dir_all(&root)?;
    }
    let mut hashes = BTreeMap::new();
    for kind in ["request", "response", "manifest"] {
        let output = checked(Command::new(&binary).args(["schema", kind]), 10)?;
        let bytes = canonical_json(serde_json::from_slice::<Value>(&output.stdout)?)?;
        let name = format!("{kind}.schema.json");
        verify_or_write(&root.join(&name), &bytes, check)?;
        hashes.insert(name, hash_bytes(&bytes));
    }

    let fixtures = contracts.join("fixtures");
    let inputs = fs::read_dir(fixtures)?
        .map(|entry| entry.map(|entry| entry.path()))
        .chain(["theme.tokens.json", "compatibility.json"].map(|name| Ok(root.join(name))));
    for path in inputs {
        let path = path?;
        let name = path
            .strip_prefix(contracts)?
            .to_str()
            .ok_or("Invalid contract filename")?;
        hashes.insert(name.replace('\\', "/"), hash_file(&path)?);
    }
    let bytes = canonical_json(json!({
        "artifact": "numera-core-contracts-v1",
        "wire_major": core_contracts::PROTOCOL,
        "schema_revision": core_contracts::SCHEMA_REVISION,
        "status": "development; not an integration-ready release",
        "sha256": hashes
    }))?;
    verify_or_write(&root.join("manifest.json"), &bytes, check)?;
    info("PASS: canonical schemas match Rust producer types");
    Ok(())
}

fn verify_or_write(path: &Path, bytes: &[u8], check: bool) -> Result<()> {
    if check {
        if fs::read(path)? != bytes {
            return Err(format!("Canonical contract drift at {}", path.display()).into());
        }
    } else {
        fs::write(path, bytes)?;
    }
    Ok(())
}
