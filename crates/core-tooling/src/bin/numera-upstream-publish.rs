use core_tooling::chromium_build::{process, report};
use serde_json::Value;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Duration;
use std::{env, error::Error, fs, io};

const REPOSITORY: &str = "KiaroSama/Numera-Core-Browser";
const BRANCH: &str = "automation/upstream-candidate";
const OBSERVATION: &str = "upstream/candidates.json";
type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Candidate<'a> {
    root: &'a Path,
    credentials: bool,
}

impl Candidate<'_> {
    fn git(&self, args: &[&str]) -> Result<process::Captured> {
        let mut git = Command::new("git");
        git.current_dir(self.root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ]);
        if self.credentials {
            git.args([
                "-c",
                "credential.helper=",
                "-c",
                "credential.helper=!gh auth git-credential",
            ]);
        }
        Ok(process::capture(
            git.args(args),
            Duration::from_secs(60),
            1024 * 1024,
        )?)
    }

    fn checked(&self, args: &[&str]) -> Result<Vec<u8>> {
        let child = self.git(args)?;
        if !child.status.success() {
            return Err(format!("Git {} failed ({})", args[0], child.status).into());
        }
        Ok(child.stdout)
    }

    fn changed(&self) -> Result<bool> {
        match self
            .git(&["diff", "--quiet", "HEAD", "--", OBSERVATION])?
            .status
            .code()
        {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err("Could not inspect the candidate observation".into()),
        }
    }

    fn update(&self) -> Result<bool> {
        if !self.changed()? {
            return Ok(false);
        }
        let source = self.root.join(OBSERVATION);
        let updated = fs::read(&source)?;
        let _: Value = serde_json::from_slice(&updated)?;
        self.checked(&[
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "--",
            OBSERVATION,
        ])?;
        let branch = format!("refs/heads/{BRANCH}");
        match self
            .git(&["ls-remote", "--exit-code", "--heads", "origin", &branch])?
            .status
            .code()
        {
            Some(0) => {
                self.checked(&["fetch", "origin", &branch])?;
                self.checked(&["checkout", "-b", BRANCH, "FETCH_HEAD"])?;
            }
            Some(2) => {
                self.checked(&["checkout", "-b", BRANCH])?;
            }
            _ => return Err("Could not resolve the candidate branch".into()),
        }
        fs::write(source, updated)?;
        if self.changed()? {
            self.checked(&["add", "--", OBSERVATION])?;
            self.checked(&[
                "-c",
                "user.name=github-actions[bot]",
                "-c",
                "user.email=41898282+github-actions[bot]@users.noreply.github.com",
                "commit",
                "-m",
                "chore: refresh unreviewed official upstream candidates",
                "--",
                OBSERVATION,
            ])?;
            self.checked(&["push", "origin", &format!("{branch}:{branch}")])?;
        }
        Ok(true)
    }
}

fn run() -> Result<()> {
    if env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || env::var("GITHUB_REPOSITORY").as_deref() != Ok(REPOSITORY)
    {
        return Err("Candidate publication requires the authorized repository workflow".into());
    }
    let candidate = Candidate {
        root: Path::new("."),
        credentials: true,
    };
    let origin = candidate.checked(&["remote", "get-url", "origin"])?;
    let origin = std::str::from_utf8(&origin)?.trim();
    if !matches!(
        origin,
        "https://github.com/KiaroSama/Numera-Core-Browser"
            | "https://github.com/KiaroSama/Numera-Core-Browser.git"
    ) {
        return Err("Unexpected upstream candidate publication origin".into());
    }
    if !candidate.update()? {
        return Ok(());
    }
    let mut command = Command::new("gh");
    command.args([
        "pr", "list", "--repo", REPOSITORY, "--head", BRANCH, "--state", "open", "--json", "number",
    ]);
    let open: Vec<Value> = serde_json::from_slice(&process::stdout(
        &mut command,
        Duration::from_secs(30),
        1024 * 1024,
    )?)?;
    if open.is_empty() {
        let body = process::ChildWatch::spawn(Command::new("gh").args([
            "pr",
            "create",
            "--repo",
            REPOSITORY,
            "--draft",
            "--base",
            "main",
            "--head",
            BRANCH,
            "--title",
            "chore: review upstream release candidates",
            "--body-file",
            ".github/upstream-pr.md",
        ]))?
        .wait(Duration::from_secs(60))?;
        if !body.success() {
            return Err(io::Error::other("Creating the review-only candidate PR failed").into());
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    if let Err(error) = run() {
        report::log("ERROR", "upstream-publication", &error.to_string());
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_candidate_is_updated_once_without_changing_main() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let origin = root.join("origin.git");
        let origin = origin.to_str().unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let repo = Candidate {
                root: dir,
                credentials: false,
            };
            String::from_utf8(repo.checked(args).unwrap())
                .unwrap()
                .trim()
                .to_owned()
        };
        git(root, &["init", "--bare", "--initial-branch=main", origin]);
        git(root, &["clone", origin, "seed"]);
        let seed = root.join("seed");
        fs::create_dir(seed.join("upstream")).unwrap();
        fs::write(seed.join(OBSERVATION), r#"{"version":1}"#).unwrap();
        git(&seed, &["config", "user.name", "Fixture Author"]);
        git(&seed, &["config", "user.email", "fixture@example.invalid"]);
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-m", "baseline"]);
        git(&seed, &["push", "origin", "main"]);
        git(&seed, &["checkout", "-b", BRANCH]);
        fs::write(seed.join(OBSERVATION), r#"{"version":2}"#).unwrap();
        git(&seed, &["commit", "-am", "previous candidate"]);
        git(&seed, &["push", "origin", BRANCH]);
        let origin_path = Path::new(origin);
        let reference = format!("refs/heads/{BRANCH}");
        let mut hashes = Vec::new();
        for run in ["run-1", "run-2"] {
            git(root, &["clone", origin, run]);
            let checkout = root.join(run);
            fs::write(checkout.join(OBSERVATION), r#"{"version":3}"#).unwrap();
            assert!(
                Candidate {
                    root: &checkout,
                    credentials: false
                }
                .update()
                .unwrap()
            );
            hashes.push(git(origin_path, &["rev-parse", &reference]));
            assert_eq!(
                git(
                    origin_path,
                    &["show", &format!("{reference}:{OBSERVATION}")]
                ),
                r#"{"version":3}"#
            );
        }
        assert_eq!(hashes[0], hashes[1]);
        assert_eq!(
            git(origin_path, &["show", &format!("main:{OBSERVATION}")]),
            r#"{"version":1}"#
        );
    }
}
