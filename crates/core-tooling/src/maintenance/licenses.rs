#[path = "lock.rs"]
mod lock;

use core_tooling::sha256_hex as digest;
use lock::{Result, read_lock};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

const RUNTIME: &str = "licenses/dependencies";
const FUZZ: &str = "licenses/fuzz";
const TYPED_PATH: &str = "typed-path-0.9.3";
const LIBFUZZER: &str = "libfuzzer-sys-0.4.13";
const APACHE_LLVM_MIT: &str = "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT";
const SPDX: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-3-Clause",
    "BSD-2-Clause",
    "Unicode-3.0",
    "ISC",
    "Zlib",
    "CC0-1.0",
    "Unlicense",
    "0BSD",
];

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
    source: Option<String>,
    license: Option<String>,
    repository: Option<String>,
    manifest_path: PathBuf,
}

pub fn exit(fuzz: bool) -> ExitCode {
    match run(fuzz) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(fuzz: bool) -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let first = arguments.next();
    let name = if fuzz {
        "numera-licenses-fuzz"
    } else {
        "numera-licenses"
    };
    let check = match (
        first.as_deref().and_then(|value| value.to_str()),
        arguments.next(),
    ) {
        (None, None) if first.is_none() => false,
        (Some("--check"), None) => true,
        (Some("-h" | "--help"), None) => {
            println!("Usage: {name} [--check]");
            return Ok(());
        }
        _ => return Err(format!("Usage: {name} [--check]").into()),
    };
    let mut pending = BTreeMap::new();
    let entries = inventory(fuzz, &mut pending)?;
    let count = entries.len();
    let (target, mut document) = if fuzz {
        (
            format!("{FUZZ}/dependencies.json"),
            json!({
                "scope": "Development-only fuzz/Cargo.lock; not the installed Core runtime",
            }),
        )
    } else {
        (
            format!("{RUNTIME}.json"),
            json!({
                "review": "Declared permissive SPDX expressions checked against allowlist; component legal review is still required for any engine release",
                "schema_revision": 1,
                "scope": "Cargo.lock registry dependencies; engine components are not included",
            }),
        )
    };
    document["dependencies"] = Value::Array(entries);
    document.sort_all_objects();
    let mut bytes = serde_json::to_vec_pretty(&document)?;
    bytes.push(b'\n');
    for (path, data) in pending {
        save(Path::new(&path), &data, check, "Dependency notice")?;
    }
    save(Path::new(&target), &bytes, check, "Dependency inventory")?;
    if fuzz {
        println!("PASS: {count} separately locked fuzz dependencies and notices");
    } else {
        let timestamp = format!("{:.6}", jiff::Timestamp::now()).replace('Z', "+00:00");
        eprintln!(
            "{{\"timestamp\": \"{timestamp}\", \"severity\": \"INFO\", \"component\": \"repository-tool\", \"message\": \"PASS: inventoried {count} locked dependencies and preserved license texts\"}}"
        );
    }
    Ok(())
}

fn inventory(fuzz: bool, notices: &mut BTreeMap<String, Vec<u8>>) -> Result<Vec<Value>> {
    let mut cargo = Command::new("cargo");
    cargo.args(["metadata", "--locked", "--format-version", "1"]);
    let (lock, runtime) = if fuzz {
        cargo.args(["--manifest-path", "fuzz/Cargo.toml"]);
        let data: Value = serde_json::from_slice(&fs::read(format!("{RUNTIME}.json"))?)?;
        let entries = data["dependencies"]
            .as_array()
            .ok_or("Invalid runtime inventory")?;
        let mut by_name = BTreeMap::new();
        for entry in entries {
            let key = (
                required_string(entry, "name")?.to_owned(),
                required_string(entry, "version")?.to_owned(),
            );
            if by_name.insert(key, entry.clone()).is_some() {
                return Err("Duplicate runtime inventory dependency".into());
            }
        }
        ("fuzz/Cargo.lock", by_name)
    } else {
        ("Cargo.lock", BTreeMap::new())
    };
    let lock = read_lock(Path::new(lock))?;
    let checksums: BTreeMap<_, _> = lock
        .package
        .iter()
        .map(|package| {
            (
                (
                    package.name.as_str(),
                    package.version.as_str(),
                    package.source.as_deref(),
                ),
                package.checksum.as_deref(),
            )
        })
        .collect();
    let output = cargo.output()?;
    if !output.status.success() {
        return Err(format!(
            "Locked Cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let mut metadata: Metadata = serde_json::from_slice(&output.stdout)?;
    metadata
        .packages
        .sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    let mut entries = Vec::new();
    for package in metadata.packages {
        let Some(source) = package.source.as_deref() else {
            continue;
        };
        let checksum = checksums
            .get(&(
                package.name.as_str(),
                package.version.as_str(),
                Some(source),
            ))
            .ok_or("Cargo metadata dependency missing from lock")?;
        if let Some(original) = runtime.get(&(package.name.clone(), package.version.clone())) {
            entries.push(reuse_notice(original, *checksum)?);
        } else {
            entries.push(dependency(&package, *checksum, fuzz, notices)?);
        }
    }
    Ok(entries)
}

fn required_string<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name]
        .as_str()
        .ok_or_else(|| format!("Invalid inventory field {name}").into())
}

fn reuse_notice(original: &Value, checksum: Option<&str>) -> Result<Value> {
    if original["checksum"] != json!(checksum) {
        return Err("Runtime/fuzz dependency checksum mismatch".into());
    }
    for notice in original["notices"]
        .as_array()
        .ok_or("Invalid runtime notices")?
    {
        let path = required_string(notice, "path")?;
        let notice_path = Path::new(path);
        if !notice_path.starts_with(RUNTIME)
            || !notice_path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err("Invalid runtime dependency notice path".into());
        }
        if digest(&fs::read(notice_path)?) != required_string(notice, "sha256")? {
            return Err(format!("Runtime dependency notice drift: {path}").into());
        }
    }
    let mut entry = original.clone();
    entry["notice_scope"] = json!("Reuses exact runtime dependency notices");
    Ok(entry)
}

fn dependency(
    package: &Package,
    checksum: Option<&str>,
    fuzz: bool,
    notices: &mut BTreeMap<String, Vec<u8>>,
) -> Result<Value> {
    let expression = package.license.as_deref().unwrap_or_default();
    let versioned = format!("{}-{}", package.name, package.version);
    let selected = if fuzz {
        None
    } else {
        match (versioned.as_str(), expression) {
            ("r-efi-5.3.0" | "r-efi-6.0.0", "MIT OR Apache-2.0 OR LGPL-2.1-or-later")
            | (
                "rustix-1.1.6"
                | "linux-raw-sys-0.12.1"
                | "wasi-0.11.1+wasi-snapshot-preview1"
                | "wasip2-1.0.4+wasi-0.2.12"
                | "wit-bindgen-0.57.1",
                APACHE_LLVM_MIT,
            ) => Some("MIT"),
            (TYPED_PATH, "MIT OR Apache-2.0") => Some("Apache-2.0"),
            _ => None,
        }
    };
    let libfuzzer = fuzz && versioned == LIBFUZZER && expression == "(MIT OR Apache-2.0) AND NCSA";
    let mut identifiers = expression
        .split(|character: char| !(character.is_ascii_alphanumeric() || ".+-".contains(character)))
        .filter(|token| !matches!(*token, "" | "OR" | "AND"))
        .peekable();
    let permitted = identifiers.peek().is_some()
        && identifiers.all(|token| SPDX.contains(&token) || (!fuzz && token == "MIT-0"));
    if !libfuzzer && selected.is_none() && !permitted {
        return Err(format!(
            "Unreviewed dependency license: {} {expression}",
            package.name
        )
        .into());
    }
    let root = package
        .manifest_path
        .parent()
        .ok_or("Invalid package manifest path")?;
    let location = if fuzz { FUZZ } else { RUNTIME };
    let mut files = fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
    files.sort_by_key(std::fs::DirEntry::file_name);
    let mut texts = Vec::new();
    for file in files {
        let name = file.file_name();
        let Some(name) = name.to_str() else { continue };
        let uppercase = name.to_ascii_uppercase();
        if file.path().is_file()
            && (["LICENSE", "COPYING", "NOTICE"]
                .iter()
                .any(|prefix| uppercase.starts_with(prefix))
                || (selected.is_some() && name == "AUTHORS")
                || (libfuzzer && name == "README.md"))
        {
            let data = fs::read(file.path())?;
            let target = format!("{location}/{versioned}/{name}");
            texts.push(json!({"path": target, "sha256": digest(&data)}));
            notices.insert(target, data);
        }
    }
    if !fuzz && versioned == TYPED_PATH && selected == Some("Apache-2.0") {
        let statement = format!("{RUNTIME}/{TYPED_PATH}/LICENSE-STATEMENT.md");
        if fs::read(&statement)? != fs::read(root.join("README.md"))? {
            return Err("typed-path license grant changed".into());
        }
        texts.push(reviewed_notice(
            format!("{RUNTIME}/{TYPED_PATH}/LICENSE-APACHE"),
            "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
        )?);
        texts.push(reviewed_notice(
            statement,
            "97cad99fb91dc88b707bc2f5102d0b445e79cf6faa5cc90207f6cf77d114c23b",
        )?);
    }
    let supplement = if !fuzz && texts.is_empty() && expression == "MIT" {
        supplemental_mit_notice(&versioned, package.repository.as_deref())
    } else {
        None
    };
    if let Some((_, sha256)) = supplement {
        texts.push(reviewed_notice(
            format!("{RUNTIME}/{versioned}/LICENSE"),
            sha256,
        )?);
    }
    if texts.is_empty() {
        return Err(format!("Missing dependency license text: {}", package.name).into());
    }
    if libfuzzer {
        texts.push(reviewed_notice(
            format!("{FUZZ}/{LIBFUZZER}/LICENSE-LLVM.txt"),
            "8d85c1057d742e597985c7d4e6320b015a9139385cff4cbae06ffc0ebe89afee",
        )?);
    }
    let mut entry = json!({
        "name": package.name,
        "version": package.version,
        "license": expression,
        "repository": package.repository,
        "checksum": checksum,
        "notices": texts,
    });
    if !fuzz {
        entry["source"] = json!(package.source);
    }
    if let Some(selected) = selected {
        entry["selected_license"] = json!(selected);
    }
    if let Some((source, _)) = supplement {
        entry["supplement_source"] = json!(source);
    }
    if libfuzzer {
        entry["file_level_license"] =
            json!("Apache-2.0 WITH LLVM-exception; declared legacy NCSA text also preserved");
        entry["supplement_source"] = json!("https://llvm.org/LICENSE.txt");
    }
    Ok(entry)
}

fn supplemental_mit_notice(
    package: &str,
    repository: Option<&str>,
) -> Option<(&'static str, &'static str)> {
    match (package, repository) {
        (
            "jsonschema-regex-0.63.1" | "jsonschema-value-0.63.1",
            Some("https://github.com/Stranger6667/jsonschema"),
        ) => Some((
            "https://raw.githubusercontent.com/Stranger6667/jsonschema/fe0cbd3546801b24ab9dc2ff423cbc8b1f6e8a1e/LICENSE",
            "117829c3ca21efb132d81a44b55363d395ab8eea18526873bc828da4c0e5f038",
        )),
        ("napi-sys-3.4.1", Some("https://github.com/napi-rs/napi-rs")) => Some((
            "https://raw.githubusercontent.com/napi-rs/napi-rs/d9cb526c15e63ba30e06b4d169068c9854180595/LICENSE",
            "3f1ce66533302df3a32edbfdfc0b78f0dd34659e4c1f5817162e5ea3c2297215",
        )),
        ("uuid-simd-0.8.0" | "vsimd-0.8.0", Some("https://github.com/Nugine/simd")) => Some((
            "https://raw.githubusercontent.com/Nugine/simd/d74c030d9dc4f3cae02146d1f497ff62726ef09a/LICENSE",
            "71674605ec4c087fe9eb534e3e4f9e26eb2e4aabcd76a29fd156c6a844d44b3d",
        )),
        _ => None,
    }
}

fn reviewed_notice(path: String, expected: &str) -> Result<Value> {
    if digest(&fs::read(&path)?) != expected {
        return Err(format!("Reviewed dependency notice changed: {path}").into());
    }
    Ok(json!({"path": path, "sha256": expected}))
}

fn save(path: &Path, bytes: &[u8], check: bool, scope: &str) -> Result<()> {
    if check {
        if fs::read(path)? != bytes {
            return Err(format!("{scope} drift: {}", path.display()).into());
        }
    } else {
        let parent = path.parent().ok_or("Invalid inventory path")?;
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(bytes)?;
        temp.persist(path)?;
    }
    Ok(())
}
