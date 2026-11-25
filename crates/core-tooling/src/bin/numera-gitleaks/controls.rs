use super::{Result, config::*, log, process, read_json, require, write_new};
use std::{fs, path::Path};

pub fn verify(binary: &Path, git: &Path, config: &Path, temporary: &Path) -> Result<()> {
    let digest = CHECKSUMS[0].0;
    let other = CHECKSUMS[1].0;
    let known = format!("    \"fixtures/negative-duplicate-key.ndjson\": \"{digest}\",\n");
    let probe = format!("    \"{PROBE_OBJECT}\": \"{PROBE_SHA256}\",\n");
    let cases = [
        ("public-checksum", MANIFEST, known.clone(), 0),
        (
            "second-public-checksum",
            MANIFEST,
            format!("    \"v1/theme.tokens.json\": \"{other}\",\n"),
            0,
        ),
        ("other-path", "other.json", known.clone(), 2),
        (
            "other-field",
            MANIFEST,
            format!("    \"api_key\": \"{digest}\",\n"),
            2,
        ),
        ("other-value", MANIFEST, known.replace(digest, other), 2),
        (
            "coexisting-field",
            MANIFEST,
            format!("{known}    \"api_key\": \"{digest}\",\n"),
            2,
        ),
        (
            "trailing-content",
            MANIFEST,
            format!("{} extra content\n", known.trim_end_matches('\n')),
            2,
        ),
        ("compiler-checksum", PROBE_EVIDENCE, probe.clone(), 0),
        ("compiler-other-path", "other.json", probe.clone(), 2),
        (
            "compiler-other-value",
            PROBE_EVIDENCE,
            probe.replace(PROBE_SHA256, digest),
            2,
        ),
        (
            "compiler-coexisting-field",
            PROBE_EVIDENCE,
            format!("{probe}    \"api_key\": \"{PROBE_SHA256}\",\n"),
            2,
        ),
    ];
    for (name, relative, text, expected) in &cases {
        let root = temporary.join(format!("control-{name}"));
        fs::create_dir(&root).map_err(|_| "A synthetic scanner control could not be prepared.")?;
        let target = root.join(relative);
        fs::create_dir_all(
            target
                .parent()
                .ok_or("A scanner control path is invalid.")?,
        )
        .map_err(|_| "A synthetic scanner control could not be prepared.")?;
        write_new(&target, text.as_bytes())?;
        for arguments in [
            vec!["init", "--quiet"],
            vec!["add", "."],
            vec!["commit", "--quiet", "-m", "Synthetic scanner control"],
        ] {
            let mut child = process::command(git);
            child
                .arg("-C")
                .arg(&root)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "user.name=Numera scan fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                ])
                .args(arguments);
            require(
                process::capture(&mut child, 15, 1024 * 1024)?
                    .status
                    .success(),
                "A synthetic scanner control could not be prepared.",
            )?;
        }
        let report = temporary.join(format!("{name}.json"));
        let result = process::capture(
            &mut process::scan_command(
                binary,
                &root.join(".git"),
                config,
                temporary,
                &report,
                "--full-history --no-ext-diff --no-textconv --all",
            ),
            30,
            1024 * 1024,
        )?;
        require(
            result.status.code() == Some(*expected)
                && result.stdout.trim_ascii().is_empty()
                && result.stderr.trim_ascii().is_empty(),
            "A public-checksum scanner control failed; no clean result was recorded.",
        )?;
        let findings = read_json(&report)?;
        require(
            findings
                .as_array()
                .is_some_and(|items| items.len() == usize::from(*expected != 0)),
            "A scanner control produced an unexpected finding count.",
        )?;
    }
    log(
        "INFO",
        &format!("All {} public-checksum scope controls passed.", cases.len()),
    );
    Ok(())
}
