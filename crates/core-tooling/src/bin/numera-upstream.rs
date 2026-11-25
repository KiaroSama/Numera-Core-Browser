#[path = "../maintenance/http.rs"]
mod http;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};
use url::Url;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const GITHUB: &str = "api.github.com";
const MOZILLA: &str = "https://product-details.mozilla.org/1.0/firefox_versions.json";
const WEBKIT: &str = "https://api.github.com/repos/WebKit/WebKit/commits/main";
const PUPPETEER: &str = "https://api.github.com/repos/puppeteer/puppeteer/releases?per_page=50";
const PLAYWRIGHT: &str = "https://api.github.com/repos/microsoft/playwright/releases/latest";
const SELENIUM: &str = "https://api.github.com/repos/SeleniumHQ/selenium/releases/latest";
const COMPATIBILITY: &str = "NOT_RUN; not an engine protocol version lock";

#[derive(Deserialize)]
struct ChromiumRelease {
    version: String,
    serving: Serving,
}

#[derive(Deserialize)]
struct Serving {
    #[serde(rename = "startTime")]
    start_time: String,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    published_at: Option<String>,
    prerelease: Option<bool>,
    draft: Option<bool>,
}

#[derive(Serialize)]
struct Chromium {
    version: String,
    serving_start: String,
    source: String,
}

#[derive(Serialize)]
struct Platforms {
    win: Chromium,
    linux: Chromium,
}

#[derive(Serialize)]
struct Client {
    tag: String,
    published_at: Option<String>,
    source: &'static str,
    compatibility: &'static str,
}

#[derive(Serialize)]
struct Clients {
    playwright: Client,
    puppeteer: Client,
    selenium: Client,
}

#[derive(Serialize)]
struct Gecko {
    stable: String,
    esr: Option<String>,
    source: &'static str,
    scope: &'static str,
}

#[derive(Serialize)]
struct WebKit {
    revision: String,
    source: &'static str,
    scope: &'static str,
    note: &'static str,
}

#[derive(Serialize)]
struct Versions {
    chromium: Platforms,
    automation_clients: Clients,
    gecko: Gecko,
    webkit: WebKit,
}

#[derive(Serialize)]
struct Candidate {
    schema_revision: u32,
    observed_at: String,
    status: &'static str,
    trusted_release: bool,
    versions: Versions,
}

fn main() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn execute() -> Result<()> {
    let mut output = PathBuf::from("upstream/candidates.json");
    let mut refresh = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(option) = args.next() {
        match option.to_str() {
            Some("-h" | "--help") => {
                println!("Usage: numera-upstream [--output PATH] [--refresh-observation]");
                return Ok(());
            }
            Some("--output") => output = args.next().ok_or("Missing --output path")?.into(),
            Some("--refresh-observation") => refresh = true,
            _ => return Err("Unknown upstream observation option".into()),
        }
    }
    let token = ["GH_TOKEN", "GITHUB_TOKEN"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    let versions = collect(|source| {
        let credential = Url::parse(source)
            .ok()
            .filter(|url| url.host_str() == Some(GITHUB))
            .and(token.as_deref());
        Ok(http::request_json(
            source,
            "Numera-Core-Browser-upstream-watcher",
            Duration::from_secs(30),
            credential,
            None,
        )?)
    })?;
    let timestamp = timestamp();
    let rewritten = save(&output, refresh, timestamp.clone(), versions)?;
    let message = if rewritten {
        "Wrote unreviewed upstream candidate; no engine was built, installed or promoted"
    } else {
        "Official versions unchanged; candidate retained"
    };
    eprintln!(
        "{{\"timestamp\": \"{timestamp}\", \"severity\": \"INFO\", \"component\": \"repository-tool\", \"message\": \"{message}\"}}"
    );
    Ok(())
}

fn timestamp() -> String {
    format!("{:.6}", jiff::Timestamp::now()).replace('Z', "+00:00")
}

fn collect(mut fetch: impl FnMut(&str) -> Result<Value>) -> Result<Versions> {
    let chromium = Platforms {
        win: chromium("win", &mut fetch)?,
        linux: chromium("linux", &mut fetch)?,
    };
    let mozilla = fetch(MOZILLA)?;
    let webkit = fetch(WEBKIT)?;
    let playwright = serde_json::from_value(fetch(PLAYWRIGHT)?)?;
    let puppeteer = serde_json::from_value::<Vec<GitHubRelease>>(fetch(PUPPETEER)?)?
        .into_iter()
        .find(|row| {
            row.tag_name.starts_with("puppeteer-v")
                && row.prerelease == Some(false)
                && row.draft == Some(false)
        })
        .ok_or("No published Puppeteer client release")?;
    let selenium = serde_json::from_value(fetch(SELENIUM)?)?;
    Ok(Versions {
        chromium,
        automation_clients: Clients {
            playwright: client(playwright, PLAYWRIGHT),
            puppeteer: client(puppeteer, PUPPETEER),
            selenium: client(selenium, SELENIUM),
        },
        gecko: Gecko {
            stable: mozilla["LATEST_FIREFOX_VERSION"]
                .as_str()
                .ok_or("Missing stable Firefox version")?
                .to_owned(),
            esr: serde_json::from_value(mozilla["FIREFOX_ESR"].clone())?,
            source: MOZILLA,
            scope: "deferred",
        },
        webkit: WebKit {
            revision: webkit["sha"]
                .as_str()
                .ok_or("Missing WebKit revision")?
                .to_owned(),
            source: WEBKIT,
            scope: "deferred",
            note: "Not a universal Safari release version",
        },
    })
}

fn chromium(platform: &str, fetch: &mut impl FnMut(&str) -> Result<Value>) -> Result<Chromium> {
    let source = format!(
        "https://versionhistory.googleapis.com/v1/chrome/platforms/{platform}/channels/stable/versions/all/releases?filter=endtime=none"
    );
    let response = fetch(&source)?;
    let releases = response["releases"]
        .as_array()
        .ok_or("Missing Chromium releases")?;
    let mut eligible = Vec::new();
    for release in releases.iter().filter(|release| {
        release["pinnable"].as_bool() == Some(true)
            && release["fraction"]
                .as_f64()
                .is_some_and(|fraction| fraction >= 1.0)
    }) {
        let release: ChromiumRelease = serde_json::from_value(release.clone())?;
        let version: Vec<u32> = release
            .version
            .split('.')
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()?;
        eligible.push((release, version));
    }
    let (release, _) = eligible
        .into_iter()
        .max_by(|(a, av), (b, bv)| (&a.serving.start_time, av).cmp(&(&b.serving.start_time, bv)))
        .ok_or(
            "Official feed has no fully rolled out pinnable stable release; manual review required",
        )?;
    Ok(Chromium {
        version: release.version,
        serving_start: release.serving.start_time,
        source,
    })
}

fn client(release: GitHubRelease, source: &'static str) -> Client {
    Client {
        tag: release.tag_name,
        published_at: release.published_at,
        source,
        compatibility: COMPATIBILITY,
    }
}

fn save(path: &Path, refresh: bool, timestamp: String, versions: Versions) -> Result<bool> {
    if path.try_exists()? {
        let old: Value = serde_json::from_slice(&fs::read(path)?)?;
        if !refresh && old["versions"] == serde_json::to_value(&versions)? {
            return Ok(false);
        }
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut output = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(
        &mut output,
        &Candidate {
            schema_revision: 1,
            observed_at: timestamp,
            status: "unreviewed-candidate",
            trusted_release: false,
            versions,
        },
    )?;
    writeln!(output)?;
    output.persist(path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{MOZILLA, PUPPETEER, WEBKIT, chromium, collect, save};
    use serde_json::json;

    #[test]
    fn selects_the_fully_rolled_out_numeric_version() {
        let release = chromium("linux", &mut |_| Ok(json!({"releases": [
            {"pinnable": true, "fraction": 1.0, "version": "9.3", "serving": {"startTime": "2026-11-25T00:00:00Z"}},
            {"pinnable": true, "fraction": 1.0, "version": "10.2", "serving": {"startTime": "2026-11-25T00:00:00Z"}},
            {"pinnable": false, "fraction": 1.0, "version": "99.4", "serving": {"startTime": "2026-11-26T00:00:00Z"}},
            {"pinnable": true, "fraction": 0.9, "version": "99.5", "serving": {"startTime": "2026-11-26T00:00:00Z"}},
        ]}))).unwrap();
        assert_eq!(release.version, "10.2");
    }

    #[test]
    fn keeps_an_unchanged_candidate_and_never_marks_it_trusted() {
        let fixture = || {
            collect(|source| Ok(match source {
            MOZILLA => json!({"LATEST_FIREFOX_VERSION": "1.2.3"}),
            WEBKIT => json!({"sha": "fixture"}),
            PUPPETEER => json!([
                {"tag_name": "@puppeteer/browsers-v9.9.9"},
                {"tag_name": "puppeteer-v9", "prerelease": true},
                    {"tag_name": "puppeteer-v3.2.1", "prerelease": false, "draft": false},
            ]),
            _ if source.contains("googleapis") => json!({"releases": [
                {"pinnable": true, "fraction": 1.0, "version": "10.2", "serving": {"startTime": "fixture"}},
            ]}),
            _ => json!({"tag_name": "v1", "published_at": "fixture"}),
        })).unwrap()
        };
        assert_eq!(
            fixture().automation_clients.puppeteer.tag,
            "puppeteer-v3.2.1"
        );
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("candidate.json");
        assert!(save(&path, false, "first".into(), fixture()).unwrap());
        assert!(!save(&path, false, "second".into(), fixture()).unwrap());
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(record["observed_at"], "first");
        assert_eq!(record["trusted_release"], false);
        assert!(save(&path, true, "second".into(), fixture()).unwrap());
    }
}
