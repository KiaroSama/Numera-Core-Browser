#[path = "numera-check-contracts/checker.rs"]
mod checker;
#[path = "numera-package-cli/common.rs"]
mod common;

use common::{
    Arguments, Result, checked, cli, git_archive_zip, git_head, git_tree, hash_file, info,
    lexical_relative, write_new,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter, write::SimpleFileOptions};

const ARTIFACT: &str = "numera-core-contracts-v1";
const CHECKER_SOURCE: &str = "crates/core-tooling/src/bin/numera-check-contracts.rs";
const COMMON_SOURCE: &str = "crates/core-tooling/src/bin/numera-package-cli/common.rs";
const INCLUDED: &[&str] = &[
    "LICENSE",
    "contracts",
    CHECKER_SOURCE,
    "crates/core-tooling/src/bin/numera-check-contracts",
    COMMON_SOURCE,
];

fn main() -> ExitCode {
    cli(
        "usage: numera-package-contracts --binary BINARY",
        &["--binary"],
        &[],
        package,
    )
}

fn package(args: Arguments) -> Result<()> {
    let binary = args.path("--binary")?.canonicalize()?;
    let revision = git_head()?;
    checked(
        Command::new("git")
            .args(["diff", "--quiet", "HEAD", "--"])
            .args(INCLUDED),
        30,
    )?;
    let mut expected = git_tree(&revision, INCLUDED)?;
    if ![
        CHECKER_SOURCE,
        COMMON_SOURCE,
        "contracts/v1/manifest.json",
        "LICENSE",
    ]
    .iter()
    .all(|name| expected.contains(*name))
    {
        return Err(
            "Contract sources and conformance checker must be committed before packaging".into(),
        );
    }
    expected.insert("source-commit.txt".to_owned());

    let repository = env::current_dir()?.canonicalize()?;
    let build = repository.join("build");
    fs::create_dir_all(&build)?;
    let build = build.canonicalize()?;
    if !build.starts_with(repository) {
        return Err("Contract package output must stay in the workspace".into());
    }
    let output = build.join("contracts");
    fs::create_dir(&output)?;
    let archive_path = output.join(format!("{ARTIFACT}.zip"));
    git_archive_zip(
        &archive_path,
        &revision,
        Some(&format!("{ARTIFACT}/")),
        INCLUDED,
        30,
    )?;
    append_provenance(&archive_path, &revision)?;

    let temp = tempfile::Builder::new()
        .prefix("Numera NCB-1 فارسی ")
        .tempdir()?;
    let extracted = extract_contracts(&archive_path, &expected, temp.path())?;
    verify_hashes(&extracted)?;
    checker::run(&binary, &extracted)?;
    write_new(
        &output.join(format!("{ARTIFACT}.sha256")),
        format!("{}  {ARTIFACT}.zip\n", hash_file(&archive_path)?).as_bytes(),
    )?;
    info("PASS: extracted NCB-1 contract archive passed real native producer conformance");
    Ok(())
}

fn append_provenance(path: &Path, revision: &str) -> Result<()> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut zip = ZipWriter::new_append(file)?;
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o600);
    zip.start_file(format!("{ARTIFACT}/source-commit.txt"), options)?;
    writeln!(zip, "{revision}")?;
    zip.finish()?;
    Ok(())
}

fn extract_contracts(
    archive: &Path,
    expected: &BTreeSet<String>,
    output: &Path,
) -> Result<PathBuf> {
    let mut archive = ZipArchive::new(File::open(archive)?)?;
    let mut names = BTreeSet::new();
    let root = output.join(ARTIFACT);
    let prefix = format!("{ARTIFACT}/");
    for index in 0..archive.len() {
        let mut item = archive.by_index(index)?;
        if item.is_dir() {
            continue;
        }
        let member = item.name()?;
        let name = member
            .strip_prefix(&prefix)
            .ok_or("Invalid contract archive directory")?;
        if !lexical_relative(name)
            || !expected.contains(name)
            || !names.insert(name.to_owned())
            || item
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(
                "Contract archive does not contain exactly the expected regular source files"
                    .into(),
            );
        }
        let path = root.join(name);
        fs::create_dir_all(path.parent().ok_or("Missing contract path parent")?)?;
        io::copy(&mut item, &mut File::create_new(path)?)?;
    }
    if &names != expected {
        return Err("Contract archive omitted checked source files".into());
    }
    Ok(root)
}

fn verify_hashes(extracted: &Path) -> Result<()> {
    let contracts = extracted.join("contracts").canonicalize()?;
    let manifest: Value = serde_json::from_slice(&fs::read(contracts.join("v1/manifest.json"))?)?;
    if manifest["artifact"] != ARTIFACT {
        return Err("Contract archive identity differs from the canonical manifest".into());
    }
    let hashes = manifest["sha256"]
        .as_object()
        .ok_or("Missing canonical manifest hashes")?;
    for (name, expected) in hashes {
        if !lexical_relative(name) {
            return Err("Canonical manifest path escapes the contract directory".into());
        }
        let path = if name.ends_with(".schema.json") {
            contracts.join("v1").join(name)
        } else {
            contracts.join(name)
        };
        let path = path.canonicalize()?;
        if !path.starts_with(&contracts) || Some(hash_file(&path)?.as_str()) != expected.as_str() {
            return Err("Archived contract member does not match its canonical SHA-256".into());
        }
    }
    Ok(())
}
