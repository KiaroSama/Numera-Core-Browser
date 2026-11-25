use super::distribution::{
    MANIFEST, PROOF, RUNTIME, SOURCE, Selection, verify_download, verify_subjects,
};
use core_tooling::{chromium_build::metadata::Host, sha256_file};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture {
    _directory: tempfile::TempDir,
    repository: PathBuf,
    package: PathBuf,
    provenance: PathBuf,
    revision: String,
    manifest: Value,
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Synthetic fixture")
        .env("GIT_COMMITTER_NAME", "Synthetic fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap();
    assert!(output.status.success(), "fixture Git operation failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("Numera proof فارسی ")
            .tempdir()
            .unwrap();
        let repository = directory.path().join("repo");
        let package = directory.path().join("runtime bundle");
        let provenance = directory.path().join("provenance");
        for path in [&repository, &package, &provenance] {
            fs::create_dir(path).unwrap();
        }
        fs::write(provenance.join("bundle.jsonl"), b"synthetic fixture\n").unwrap();
        let proof = json!({"host":"Linux", "arch":"x86-64", "chromium_revision":"fixture-revision",
            "chromium_version":"1.2.3.4", "source_archive":{"sha256":"a".repeat(64),"bytes":1}});
        let evidence = repository.join("docs/evidence");
        fs::create_dir_all(&evidence).unwrap();
        let original = evidence.join("chromium-source-export-1-linux.json");
        fs::write(&original, serde_json::to_vec(&proof).unwrap()).unwrap();
        git(&repository, &["init", "-q"]);
        git(&repository, &["add", "."]);
        git(
            &repository,
            &["commit", "-q", "-m", "Synthetic source fixture"],
        );
        let revision = git(&repository, &["rev-parse", "HEAD"]);
        git(
            &repository,
            &[
                "archive",
                "--format=zip",
                "--output",
                package.join(SOURCE).to_str().unwrap(),
                &revision,
            ],
        );
        fs::copy(original, package.join(PROOF)).unwrap();
        fs::write(package.join("runtime-inventory.json"), b"{}\n").unwrap();
        fs::write(package.join(RUNTIME), b"synthetic runtime payload\n").unwrap();
        let digest = |filename| sha256_file(&package.join(filename)).unwrap();
        let inventory = digest("runtime-inventory.json");
        let manifest = json!({"target":"x86_64-unknown-linux-gnu", "build_mode":"development",
            "numera_commit":revision, "workflow_run":42, "trusted_release":false,
            "runtime":{"file":RUNTIME,"sha256":digest(RUNTIME),"bytes":26,"files":1},
            "numera_source":{"file":SOURCE,"sha256":digest(SOURCE)},
            "upstream_source":proof["source_archive"],"chromium_revision":proof["chromium_revision"],
            "chromium_version":proof["chromium_version"],
            "attachments_sha256":{"runtime-inventory.json":inventory,PROOF:digest(PROOF)},
            "runtime_inventory_sha256":inventory});
        let fixture = Self {
            _directory: directory,
            repository,
            package,
            provenance,
            revision,
            manifest,
        };
        fixture.save();
        fixture
    }

    fn selection(&self, host: Host) -> Selection<'_> {
        Selection {
            host,
            revision: &self.revision,
            run: 42,
        }
    }

    fn save(&self) {
        fs::write(
            self.package.join(MANIFEST),
            serde_json::to_vec(&self.manifest).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn the_other_platform_is_rejected_before_attestation() {
    let fixture = Fixture::new();
    assert!(
        verify_download(
            &fixture.package,
            &fixture.provenance,
            &fixture.repository,
            fixture.selection(Host::Windows),
            |_| panic!("wrong platform reached attestation")
        )
        .is_err()
    );
}

#[test]
fn the_download_checks_three_subjects_under_the_exact_workflow_policy() {
    let fixture = Fixture::new();
    let mut names = Vec::new();
    verify_download(&fixture.package, &fixture.provenance, &fixture.repository,
        fixture.selection(Host::Linux), |command| {
            let flags = command.get_args().collect::<Vec<_>>();
            assert_eq!(command.get_program(), "gh");
            assert_eq!(&flags[..2], &["attestation", "verify"]);
            assert_eq!(flags[3], "--repo");
            assert_eq!(flags[4], "KiaroSama/Numera-Core-Browser");
            assert_eq!(flags[5], "--bundle");
            assert_eq!(Path::new(flags[6]), fixture.provenance.join("bundle.jsonl"));
            assert_eq!(&flags[7..10], &["--source-ref", "refs/heads/main", "--source-digest"]);
            assert_eq!(flags[10], fixture.revision.as_str());
            assert_eq!(&flags[11..], &["--signer-workflow", "KiaroSama/Numera-Core-Browser/.github/workflows/chromium-development-provenance.yml", "--deny-self-hosted-runners"]);
            names.push(Path::new(flags[2]).file_name().unwrap().to_owned());
            Ok(())
        }).unwrap();
    assert_eq!(names, [MANIFEST, RUNTIME, SOURCE]);
}

#[test]
fn preattestation_checks_the_exact_file_set_and_bytes() {
    let fixture = Fixture::new();
    let verify = || {
        verify_subjects(
            &fixture.package,
            &fixture.repository,
            fixture.selection(Host::Linux),
        )
    };
    assert!(verify().is_ok());
    fs::write(fixture.package.join("extra"), b"").unwrap();
    assert!(verify().is_err());
    fs::remove_file(fixture.package.join("extra")).unwrap();
    fs::write(fixture.package.join(RUNTIME), b"altered runtime payload\n").unwrap();
    assert!(verify().is_err());
}

#[test]
fn a_rehashed_proof_must_match_the_pinned_commit() {
    let mut fixture = Fixture::new();
    let proof = fixture.package.join(PROOF);
    let mut bytes = fs::read(&proof).unwrap();
    bytes.push(b'\n');
    fs::write(&proof, bytes).unwrap();
    fixture.manifest["attachments_sha256"][PROOF] = json!(sha256_file(&proof).unwrap());
    fixture.save();
    assert!(
        verify_subjects(
            &fixture.package,
            &fixture.repository,
            fixture.selection(Host::Linux)
        )
        .is_err()
    );
}
