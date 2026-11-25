use super::{Result, process, require};
use serde_json::Value;
use std::path::Path;

pub const MANIFEST: &str = "contracts/v1/manifest.json";
pub const PROBE_EVIDENCE: &str = "docs/evidence/chromium-object-probe-45003337977.json";
pub const PROBE_OBJECT: &str = "obj/content/browser/browser/agent_cluster_key.obj";
pub const PROBE_SHA256: &str = "72d63ad2833f4066c19e796d7936bb45045f1c5055296ab9487cecfe8fade53a";
pub const CHECKSUMS: [(&str, &str); 2] = [
    (
        "df19315c7a5ab7a3cf94a5290cbc6a051162acbb4ec98a3d00c70474ed6b91ea",
        "fixtures/negative-duplicate-key.ndjson",
    ),
    (
        "c238c6cd6815b81e2dc304d722b380701a14589c456438fb5d305e6acd0449a0",
        "v1/theme.tokens.json",
    ),
];

pub fn revision(value: &Value) -> Result<String> {
    let text = value
        .as_str()
        .ok_or("The pull request revision is invalid.")?;
    require(
        text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "The pull request revision is invalid.",
    )?;
    Ok(text.to_owned())
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        if ".^$*+?{}[]\\|()-".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn pattern(name: &str, digest: &str) -> String {
    format!(r#"^\s*"{}"\s*:\s*"{digest}"\s*,?\s*$"#, regex_escape(name))
}

pub fn render_config() -> String {
    // Every exception requires generic-api-key AND the exact path AND a whole line.
    // Commit exclusions and repository suppression files are deliberately absent.
    let mut text = "[extend]\nuseDefault = true\n\n[[allowlists]]\ndescription = \"Verified public contract checksum lines\"\ntargetRules = [\"generic-api-key\"]\ncondition = \"AND\"\npaths = ['''^contracts/v1/manifest\\.json$''']\nregexTarget = \"line\"\nregexes = [\n".to_owned();
    for (digest, name) in CHECKSUMS {
        text.push_str(&format!("'''{}''',\n", pattern(name, digest)));
    }
    text.push_str("]\n\n[[allowlists]]\ndescription = \"Verified public compiler object checksum line\"\ntargetRules = [\"generic-api-key\"]\ncondition = \"AND\"\n");
    text.push_str(&format!(
        "paths = ['''^{}$''']\nregexTarget = \"line\"\nregexes = ['''{}''']\n",
        regex_escape(PROBE_EVIDENCE),
        pattern(PROBE_OBJECT, PROBE_SHA256)
    ));
    text
}

fn blob(git: &Path, directory: &Path, reference: &str) -> Result<Vec<u8>> {
    let result = process::capture(
        process::command(git)
            .arg("-C")
            .arg(directory)
            .args(["show", reference]),
        15,
        1024 * 1024,
    )?;
    require(
        result.status.success(),
        "A historical public checksum could not be verified.",
    )?;
    Ok(result.stdout)
}

pub fn scanner_config(git: &Path, directory: &Path) -> Result<String> {
    for (digest, name) in CHECKSUMS {
        let bytes = blob(
            git,
            directory,
            &format!("7237e7ef222c8d6e072c99b937fed280c9cb64f2:contracts/{name}"),
        )?;
        require(
            core_tooling::sha256_hex(&bytes) == digest,
            "A historical public contract checksum could not be verified.",
        )?;
    }
    let bytes = blob(
        git,
        directory,
        &format!("0ea1a277d507eda77407b675e781cd94f0020424:{PROBE_EVIDENCE}"),
    )?;
    require(
        core_tooling::sha256_hex(&bytes)
            == "59aac59214aded922e5f92b6311b36cad5e2ea7349aa73c28b259cd4809377b6",
        "The historical compiler measurement could not be verified.",
    )?;
    let probe: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "The historical compiler measurement is invalid.")?;
    require(
        probe["selected_object_sha256"][PROBE_OBJECT] == PROBE_SHA256,
        "The historical public compiler checksum could not be verified.",
    )?;
    Ok(render_config())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_accepts_only_exact_commit_ids() {
        assert!(revision(&serde_json::json!("A".repeat(40))).is_ok());
        for value in [
            Value::Null,
            serde_json::json!("a".repeat(39)),
            serde_json::json!("a".repeat(41)),
            serde_json::json!(format!("{} ", "a".repeat(39))),
            serde_json::json!("--all"),
        ] {
            assert!(revision(&value).is_err());
        }
    }

    #[test]
    fn configuration_has_only_the_existing_three_exact_lines() {
        let text = render_config();
        let parsed: toml::Value = toml::from_str(&text).expect("valid scanner TOML");
        assert_eq!(
            parsed["allowlists"].as_array().expect("allowlists").len(),
            2
        );
        for entry in parsed["allowlists"].as_array().expect("allowlists") {
            assert_eq!(entry["condition"].as_str(), Some("AND"));
            assert_eq!(
                entry["targetRules"].as_array().expect("target rules").len(),
                1
            );
            assert_eq!(entry["targetRules"][0].as_str(), Some("generic-api-key"));
            assert!(entry.get("commits").is_none());
        }
        assert_eq!(
            parsed["allowlists"][0]["regexes"]
                .as_array()
                .expect("regexes")
                .len(),
            2
        );
        assert_eq!(
            parsed["allowlists"][1]["regexes"]
                .as_array()
                .expect("regexes")
                .len(),
            1
        );
        assert!(text.contains("^contracts/v1/manifest\\.json$"));
    }
}
