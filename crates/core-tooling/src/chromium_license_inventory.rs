use crate::chromium_build::process::{self, Environment};
use crate::chromium_build::report::log;
use crate::sha256_file;
use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const REVIEW_ERROR: &str =
    "The candidate needs matched official version, tag and branding evidence";

fn hex(value: &Value, len: usize) -> bool {
    value.as_str().is_some_and(|text| {
        text.len() == len
            && text
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}

pub fn reviewed_stable_lock(lock: &Value, evidence: &Value) -> Result<Value> {
    let versions = &evidence["observation"]["versions"]["chromium"];
    let checks = evidence["chromium_active_target_tag_checks"]
        .as_array()
        .ok_or(REVIEW_ERROR)?;
    if evidence["engine_approved"] != false || evidence["source_build"] != "NOT_RUN" {
        return Err(REVIEW_ERROR.into());
    }
    let mut pins = Map::new();
    for (platform, feed) in [("windows", "win"), ("linux", "linux")] {
        let version = versions[feed]["version"].as_str().ok_or(REVIEW_ERROR)?;
        let mut components = version.split('.');
        if components.clone().count() != 4
            || !components.all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
        {
            return Err(REVIEW_ERROR.into());
        }
        let record = checks
            .iter()
            .rev()
            .find(|row| row["version"] == version)
            .ok_or(REVIEW_ERROR)?;
        if !hex(&record["revision"], 40)
            || !hex(&record["branding_source_sha256"], 64)
            || record["tag_url"]
                != format!("https://chromium.googlesource.com/chromium/src/+/refs/tags/{version}")
            || record["original_patch_applies"] != true
        {
            return Err(REVIEW_ERROR.into());
        }
        pins.insert(
            platform.into(),
            json!({
                "version": version, "revision": record["revision"],
                "branding_source_sha256": record["branding_source_sha256"],
            }),
        );
    }
    let mut result = lock.clone();
    let fields = result.as_object_mut().ok_or(REVIEW_ERROR)?;
    fields.insert(
        "upstream_version".into(),
        pins["windows"]["version"].clone(),
    );
    fields.insert(
        "upstream_revision".into(),
        pins["windows"]["revision"].clone(),
    );
    fields.insert("platform_pins".into(), Value::Object(pins));
    fields.insert("status".into(), json!("candidate-not-built"));
    fields.insert("approved".into(), json!(false));
    Ok(result)
}

// These fields retain the order used by the pinned source-lock format. Existing
// checkpoint evidence binds the bytes of the candidate lock as well as its values.
struct LockDocument<'a> {
    value: &'a Value,
    fields: &'static [&'static str],
}

impl Serialize for LockDocument<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let Some(object) = self.value.as_object() else {
            return self.value.serialize(serializer);
        };
        let mut map = serializer.serialize_map(Some(object.len()))?;
        for key in self
            .fields
            .iter()
            .copied()
            .filter(|key| object.contains_key(*key))
            .chain(
                object
                    .keys()
                    .map(String::as_str)
                    .filter(|key| !self.fields.contains(key)),
            )
        {
            let value = &object[key];
            match key {
                "patches" => map.serialize_entry(key, &PatchDocuments(value))?,
                "platform_pins" => map.serialize_entry(
                    key,
                    &Self {
                        value,
                        fields: &["windows", "linux"],
                    },
                )?,
                "windows" | "linux" => map.serialize_entry(
                    key,
                    &Self {
                        value,
                        fields: &["version", "revision", "branding_source_sha256"],
                    },
                )?,
                _ => map.serialize_entry(key, value)?,
            }
        }
        map.end()
    }
}

struct PatchDocuments<'a>(&'a Value);

impl Serialize for PatchDocuments<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let Some(items) = self.0.as_array() else {
            return self.0.serialize(serializer);
        };
        let mut sequence = serializer.serialize_seq(Some(items.len()))?;
        for value in items {
            sequence.serialize_element(&LockDocument {
                value,
                fields: &[
                    "path",
                    "sha256",
                    "license",
                    "reason",
                    "regression_test",
                    "rebase_or_remove_when",
                ],
            })?;
        }
        sequence.end()
    }
}

pub fn prepare_candidate_lock(
    lock: &Value,
    evidence: &str,
    output: &Path,
) -> Result<(PathBuf, Value, Value)> {
    let stamp = evidence
        .strip_prefix("upstream-review-")
        .and_then(|value| value.strip_suffix(".json"));
    if !stamp.is_some_and(|stamp| {
        stamp.len() == 10
            && stamp.bytes().enumerate().all(|(index, c)| {
                if matches!(index, 4 | 7) {
                    c == b'-'
                } else {
                    c.is_ascii_digit()
                }
            })
    }) {
        return Err("Select a reviewed upstream evidence filename from docs/evidence".into());
    }
    let review = Path::new("docs/evidence").join(evidence);
    let reviewed = reviewed_stable_lock(lock, &serde_json::from_slice(&fs::read(&review)?)?)?;
    let path = output.join("candidate-source-lock.json");
    let document = LockDocument {
        value: &reviewed,
        fields: &[
            "schema_revision",
            "family",
            "package_id",
            "upstream_url",
            "upstream_version",
            "upstream_revision",
            "depot_tools_url",
            "depot_tools_revision",
            "patches",
            "status",
            "approved",
            "build_mode",
            "target",
            "platform_pins",
        ],
    };
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&document)?),
    )?;
    Ok((
        path,
        reviewed,
        json!({
            "file": format!("docs/evidence/{evidence}"), "sha256": sha256_file(&review)?,
        }),
    ))
}

#[derive(Serialize)]
struct Checkout {
    url: String,
    revision: String,
}

#[derive(Serialize)]
struct SourceInventory<'a> {
    schema_revision: u32,
    chromium_revision: &'a str,
    workspace_dependencies_sha256: &'a str,
    git_checkouts: BTreeMap<String, Checkout>,
    scope: &'a str,
}

pub fn git_source_inventory(
    root: &Path,
    source: &Path,
    output: &Path,
    pin: &Value,
    environment: &Environment,
) -> Result<Value> {
    let raw = root.join("git-source-revisions.unverified");
    let path = output.join("git-source-revisions.json");
    let mut command = process::command(
        if cfg!(windows) { "python" } else { "python3" },
        environment,
    );
    command
        .current_dir(source.parent().ok_or("Missing source parent")?)
        .arg(root.join("depot_tools/gclient.py"))
        .args([
            "revinfo",
            "--actual",
            "--ignore-dep-type",
            "cipd",
            "--ignore-dep-type",
            "gcs",
            "--output-json",
        ])
        .arg(&raw);
    if !process::stream_log(&mut command, &output.join("source-inventory.log"))?
        .wait(Duration::from_secs(180))?
        .success()
    {
        return Err("Chromium checkout revision inventory failed".into());
    }
    if !(1..=4 * 1024 * 1024).contains(&raw.metadata()?.len()) {
        return Err("The Chromium checkout revision inventory is out of bounds".into());
    }
    let revisions: Map<String, Value> = serde_json::from_slice(&fs::read(&raw)?)?;
    fs::remove_file(&raw)?;
    let mut checked_out = BTreeMap::new();
    for (name, item) in revisions {
        let origin = &item["url"];
        let revision = &item["rev"];
        if origin.is_null() && revision.is_null() {
            continue;
        }
        let url = Url::parse(origin.as_str().ok_or("Invalid checkout origin")?)?;
        let mut parts = name.split('/');
        if parts.next() != Some("src")
            || !parts.all(|part| {
                !matches!(part, "" | "." | "..")
                    && part
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.+-".contains(&c))
            })
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !hex(revision, 40)
        {
            return Err("The native source inventory contains an invalid checkout record".into());
        }
        checked_out.insert(
            name,
            Checkout {
                url: origin.as_str().ok_or("Invalid checkout URL")?.to_owned(),
                revision: revision
                    .as_str()
                    .ok_or("Invalid checkout revision")?
                    .to_owned(),
            },
        );
    }
    if checked_out.len() < 2
        || checked_out.get("src").map(|item| item.revision.as_str()) != pin["revision"].as_str()
    {
        return Err("The source inventory does not match the pinned Chromium checkout".into());
    }
    let deps = sha256_file(&source.join("DEPS"))?;
    let count = checked_out.len();
    let inventory = SourceInventory {
        schema_revision: 1,
        chromium_revision: pin["revision"]
            .as_str()
            .ok_or("Missing Chromium revision")?,
        workspace_dependencies_sha256: &deps,
        git_checkouts: checked_out,
        scope: "Actual Git revisions. CIPD and GCS are recorded in the pinned DEPS reference.",
    };
    let document = format!("{}\n", serde_json::to_string_pretty(&inventory)?);
    // Source-export evidence authenticates the historical inventory bytes.
    let document = if cfg!(windows) {
        document.replace('\n', "\r\n")
    } else {
        document
    };
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, document)?;
    fs::rename(temporary, &path)?;
    Ok(json!({"git_checkouts": count, "sha256": sha256_file(&path)?, "deps_sha256": deps}))
}

pub fn chromium_notices(
    source: &Path,
    output: &Path,
    host: &str,
    revision: &str,
    environment: &Environment,
) -> Result<Value> {
    let mut labels = vec![
        ("chromium", "//chrome:chrome"),
        (
            "chromedriver",
            "//chrome/test/chromedriver:chromedriver_server",
        ),
    ];
    if host == "Linux" {
        labels.push(("chrome-sandbox", "//sandbox/linux:chrome_sandbox"));
    }
    let native = source.join("out/Numera");
    let tool = source.join("tools/licenses/licenses.py");
    let link = format!("https://chromium.googlesource.com/chromium/src/+/{revision}/");
    let mut reports = Map::new();
    let diagnostics = fs::File::create(output.join("license-tool.log"))?;
    for (name, label) in labels {
        let notice = output.join(format!("{name}-NOTICE.txt"));
        let spdx = output.join(format!("{name}.spdx.json"));
        for (mode, file) in [("notice", &notice), ("spdx", &spdx)] {
            let mut command = process::command(
                if cfg!(windows) { "python" } else { "python3" },
                environment,
            );
            command
                .current_dir(source)
                .arg(&tool)
                .arg("license_file")
                .arg(file)
                .args(["--gn-target", label, "--gn-out-dir"])
                .arg(&native)
                .args([
                    "--target-os",
                    if host == "Windows" { "win" } else { "linux" },
                    "--format",
                    mode,
                    "--spdx-link",
                    &link,
                ])
                .stdout(diagnostics.try_clone()?)
                .stderr(diagnostics.try_clone()?);
            if !process::ChildWatch::spawn(&mut command)?
                .wait(Duration::from_secs(240))?
                .success()
            {
                return Err(format!("Pinned Chromium licensing tool failed for {label}").into());
            }
        }
        for path in [&notice, &spdx] {
            if !(1..=16 * 1024 * 1024).contains(&path.metadata()?.len()) {
                return Err(
                    "Pinned Chromium license tool emitted an out-of-bounds inventory".into(),
                );
            }
        }
        let inventory: Value = serde_json::from_slice(&fs::read(&spdx)?)?;
        let packages = inventory["packages"]
            .as_array()
            .ok_or("Missing SPDX packages")?;
        if inventory["spdxVersion"] != "SPDX-2.2" || packages.is_empty() {
            return Err("Pinned Chromium license tool emitted an incomplete SPDX report".into());
        }
        reports.insert(
            name.into(),
            json!({
                "gn_label": label, "packages": packages.len(),
                "notice_sha256": sha256_file(&notice)?, "spdx_sha256": sha256_file(&spdx)?,
            }),
        );
        log(
            "INFO",
            "chromium-license-inventory",
            &format!("Collected pinned-source notices and SPDX for {label}"),
        );
    }
    Ok(Value::Object(reports))
}

pub fn linked_build_materials(
    root: &Path,
    source: &Path,
    output: &Path,
    host: &str,
    environment: &Environment,
    lock_path: &Path,
) -> Result<Value> {
    fs::create_dir(output)?;
    let lock: Value = serde_json::from_slice(&fs::read(lock_path)?)?;
    let pin = &lock["platform_pins"][host.to_ascii_lowercase()];
    let revision = pin["revision"]
        .as_str()
        .ok_or("Missing reviewed source revision")?;
    let inventory = git_source_inventory(root, source, output, pin, environment)?;
    let mut inputs = Map::new();
    for (name, path) in [
        ("chromium-source-lock.json", lock_path.to_owned()),
        ("chromium-DEPS", source.join("DEPS")),
        ("chromium-args.gn", source.join("out/Numera/args.gn")),
    ] {
        let target = output.join(name);
        fs::copy(path, &target)?;
        inputs.insert(name.into(), json!(sha256_file(&target)?));
    }
    Ok(json!({
        "source_inventory": inventory, "chromium_revision": revision, "chromium_version": pin["version"],
        "inputs_sha256": inputs, "targets": chromium_notices(source, output, host, revision, environment)?,
        "complete_source_archive": "NOT_PREPARED", "redistribution_approval": false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Value, Value) {
        let lock =
            serde_json::from_str(include_str!("../../../upstream/chromium/lock.json")).unwrap();
        let review = serde_json::from_str(include_str!(
            "../../../docs/evidence/upstream-review-2026-11-21.json"
        ))
        .unwrap();
        (lock, review)
    }

    #[test]
    fn reviewed_candidate_uses_independent_pins_and_rejects_failed_branding() {
        let (lock, mut evidence) = fixture();
        let candidate = reviewed_stable_lock(&lock, &evidence).unwrap();
        assert_eq!(candidate["approved"], false);
        assert_eq!(candidate["patches"], lock["patches"]);
        assert_eq!(
            candidate["platform_pins"]["linux"]["version"],
            "158.0.8127.36"
        );
        assert_eq!(
            candidate["platform_pins"]["windows"]["revision"],
            "b8c2be851f62f6a394ec57955d7f31335d7cb6d4"
        );

        evidence["chromium_active_target_tag_checks"][0]["original_patch_applies"] = json!(false);
        assert!(reviewed_stable_lock(&lock, &evidence).is_err());
    }
}
