use core_tooling::{
    chromium_build::{
        cache,
        process::{self, Environment},
        report, tools,
    },
    sha256_hex,
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs::{self, File, FileTimes},
    io::Read,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, UNIX_EPOCH},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const REVISION: &str = "b521b230cb7c56c821235e498285cf8e548a73ee";
const ZIP_SHA: &str = "3eec3371462bbc77e5f02edb41524b34d8ab3c2e020af74dcf5ae85e732bd928";
const SISO_SHA: &str = "8df1ad4f15ad38ba5232bb91d54426ab84893ce9e28fd63fe64bf44fb1cb8036";
const TOOLCHAIN_IMAGE: &str =
    "python@sha256:934f4289019d6c3dc94d5e6341ff3a70c82f19bb118756eb03bdd700fe4b5c98";
const PROGRAM_SOURCE: &str = "crates/core-tooling/src/bin/numera-siso-checkpoint-probe.rs";
const DELIVERY: [&str; 2] = ["LICENSE", "complete-source.zip"];
const STATE: [&str; 8] = [
    ".ninja_log",
    ".siso_config",
    ".siso_deps",
    ".siso_filegroups",
    ".siso_fs_state",
    "LICENSE",
    "a.o",
    "complete-source.zip",
];
const INPUTS: [&str; 4] = ["a.c", "build.ninja", "main.c", "value.h"];
// The complete-source ZIP already exceeds one MiB, before adding native state.
const CHECKPOINT_LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
struct Entry {
    sha256: String,
    mtime_ns: u64,
}

type Inventory = BTreeMap<String, Entry>;

#[derive(Deserialize, Serialize)]
struct Manifest {
    context: Value,
    inputs: Inventory,
    outputs: Inventory,
}

fn inventory(root: &Path, names: &[&str]) -> Result<Inventory> {
    names
        .iter()
        .map(|name| {
            let path = root.join(name);
            let mtime_ns = path
                .metadata()?
                .modified()?
                .duration_since(UNIX_EPOCH)?
                .as_nanos()
                .try_into()?;
            Ok((
                (*name).into(),
                Entry {
                    sha256: cache::sha256(&path)?,
                    mtime_ns,
                },
            ))
        })
        .collect()
}

fn matches(root: &Path, expected: &Inventory, names: &[&str]) -> bool {
    names.iter().all(|name| {
        let file = root.join(name);
        file.is_file()
            && cache::sha256(&file)
                .ok()
                .is_some_and(|digest| digest == expected[*name].sha256)
    })
}

fn set_times(path: &Path, nanos: u64) -> Result<()> {
    let time = UNIX_EPOCH + Duration::from_nanos(nanos);
    File::open(path)?.set_times(FileTimes::new().set_accessed(time).set_modified(time))?;
    Ok(())
}

struct Probe {
    output: PathBuf,
    root: PathBuf,
    tool: PathBuf,
    context: Value,
    env: Environment,
    results: Vec<Value>,
}

impl Probe {
    fn new() -> Result<Self> {
        if !cfg!(target_os = "linux") {
            return Err("The Siso research fixture requires Linux".into());
        }
        let project = env::current_dir()?;
        let output = project.join("build/siso-probe");
        let root = output.join("workspace");
        fs::create_dir_all(project.join("build"))?;
        fs::create_dir(&output)?;
        fs::create_dir(&root)?;
        let env = process::build_environment();
        let image = env::var("PROBE_TOOLCHAIN_IMAGE").ok();
        if image
            .as_deref()
            .is_some_and(|image| image != TOOLCHAIN_IMAGE)
        {
            return Err("Unexpected research toolchain image".into());
        }
        let zip = tempfile::NamedTempFile::new_in(&output)?;
        tools::download(
            &format!(
                "https://chrome-infra-packages.appspot.com/dl/build/siso/linux-amd64/+/git_revision:{REVISION}"
            ),
            zip.path(),
            40 * 1024 * 1024,
            &env,
        )?;
        if cache::sha256(zip.path())? != ZIP_SHA {
            return Err("Pinned Siso transport digest mismatch".into());
        }
        let binary = tools::zip_binary_bounded(File::open(zip.path())?, "siso", 128 * 1024 * 1024)?;
        if sha256_hex(&binary) != SISO_SHA {
            return Err("Pinned Siso executable digest mismatch".into());
        }
        let tool = output.join("siso");
        tools::write_executable(&tool, &binary)?;
        let compiler = env::var_os("PATH")
            .into_iter()
            .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
            .map(|folder| folder.join("cc"))
            .find(|path| path.is_file())
            .ok_or("No native C compiler was found")?
            .canonicalize()?;
        let graph = format!(
            "rule cc\n  command = {} -MMD -MF $out.d -c $in -o $out\n  depfile = $out.d\n  deps = gcc\nrule link\n  command = {} $in -o $out\nbuild a.o: cc a.c\nbuild main.o: cc main.c\nbuild sample: link a.o main.o\ndefault sample\n",
            compiler.display(),
            compiler.display()
        );
        for (name, contents) in [
            (
                "a.c",
                "#include \"value.h\"\nint answer(void) { return VALUE; }\n".into(),
            ),
            (
                "main.c",
                "int answer(void);\nint main(void) { return answer(); }\n".into(),
            ),
            ("value.h", "#define VALUE 7\n".into()),
            ("build.ninja", graph),
        ] {
            fs::write(root.join(name), contents)?;
        }
        let git = |arguments: &[&str]| {
            let mut command = process::command("git", &env);
            command
                .arg("-c")
                .arg(format!("safe.directory={}", project.display()))
                .args(arguments);
            command
        };
        let source = root.join("complete-source.zip");
        process::stdout(
            git(&["archive", "--format=zip", "--output"])
                .arg(&source)
                .arg("HEAD"),
            Duration::from_secs(60),
            CHECKPOINT_LIMIT,
        )?;
        let mut archive = zip::ZipArchive::new(File::open(&source)?)?;
        for index in 0..archive.len() {
            let member = archive.by_index(index)?;
            if matches!(
                member.name()?.split('/').next(),
                Some(".ai" | ".codex" | ".agents")
            ) {
                return Err("Private assistant files cannot be distributed".into());
            }
        }
        let mut archived_source = Vec::new();
        archive
            .by_name(PROGRAM_SOURCE)?
            .take(CHECKPOINT_LIMIT)
            .read_to_end(&mut archived_source)?;
        if archived_source != include_bytes!("numera-siso-checkpoint-probe.rs") {
            return Err("Research source must match the complete committed source".into());
        }
        let mut license = archive.by_name("LICENSE")?;
        std::io::copy(&mut license, &mut File::create_new(root.join("LICENSE"))?)?;
        let commit = process::stdout(
            &mut git(&["rev-parse", "HEAD"]),
            Duration::from_secs(10),
            CHECKPOINT_LIMIT,
        )?;
        let context = json!({"source_commit":std::str::from_utf8(&commit)?.trim(),"probe_script_sha256":sha256_hex(&archived_source),
            "run_id":env::var("GITHUB_RUN_ID").ok(),"run_attempt":env::var("GITHUB_RUN_ATTEMPT").ok(),
            "runner_image":if image.is_none() { env::var("ImageVersion").ok() } else { None }, "toolchain_container":image,
            "workspace":root,"siso_sha256":SISO_SHA,"compiler":compiler,"compiler_sha256":cache::sha256(&compiler)?});
        report::write_json(&output.join("context.json"), &context)?;
        Ok(Self {
            output,
            root,
            tool,
            context,
            env,
            results: Vec::new(),
        })
    }

    fn build(&mut self, label: &str, target: &str, count: u32, exit: Option<i32>) -> Result<()> {
        let mut command = process::command(&self.tool, &self.env);
        command
            .args(["ninja", "--offline", "--local_jobs=2"])
            .arg(format!(
                "--cache_dir={}",
                self.output.join("cache").display()
            ))
            .args(["-d", "explain", "-C"])
            .arg(&self.root)
            .arg(target);
        let result = process::capture(&mut command, Duration::from_secs(30), CHECKPOINT_LIMIT)?;
        let log = String::from_utf8([result.stdout, result.stderr].concat())?;
        fs::write(self.output.join(format!("{label}.log")), &log)?;
        let steps = log.lines().find_map(|line| {
            line.split_once("Build Succeeded: ")?
                .1
                .split_once(" steps")?
                .0
                .parse::<u32>()
                .ok()
        });
        if !result.status.success() || steps != Some(count) {
            return Err(format!("Unexpected native result for {label}").into());
        }
        if let Some(exit) = exit
            && process::capture(
                &mut process::command(self.root.join("sample"), &self.env),
                Duration::from_secs(5),
                CHECKPOINT_LIMIT,
            )?
            .status
            .code()
                != Some(exit)
        {
            return Err(format!("Wrong compiled sample result for {label}").into());
        }
        self.results
            .push(json!({"case":label,"steps":steps,"program_exit":exit,"result":"PASS"}));
        Ok(())
    }

    fn seed(&mut self) -> Result<String> {
        self.build("cold-partial", "a.o", 1, None)?;
        let manifest = Manifest {
            context: self.context.clone(),
            inputs: inventory(&self.root, &INPUTS)?,
            outputs: inventory(&self.root, &STATE)?,
        };
        let manifest_path = self.output.join("manifest.json");
        report::write_json(&manifest_path, &serde_json::to_value(manifest)?)?;
        let checkpoint = self.output.join("checkpoint.tar.gz");
        let mut tar = tar::Builder::new(GzEncoder::new(
            File::create_new(&checkpoint)?,
            Compression::default(),
        ));
        for name in STATE {
            tar.append_path_with_name(self.root.join(name), name)?;
        }
        tar.append_path_with_name(manifest_path, "manifest.json")?;
        tar.into_inner()?.finish()?;
        self.results
            .push(json!({"case":"checkpoint-size","bytes":checkpoint.metadata()?.len()}));
        let digest = cache::sha256(&checkpoint)?;
        report::github_output(&[("checkpoint_sha", &digest)], false)?;
        Ok(digest)
    }

    fn resume(&mut self, checkpoint: &Path, digest: &str) -> Result<String> {
        if checkpoint.metadata()?.len() > CHECKPOINT_LIMIT
            || !cache::valid_sha256(digest)
            || cache::sha256(checkpoint)? != digest
        {
            return Err("Checkpoint does not match the bounded seed job output digest".into());
        }
        let mut archive = tar::Archive::new(GzDecoder::new(File::open(checkpoint)?));
        let mut members = BTreeMap::new();
        for member in archive.entries()? {
            let mut member = member?;
            let path = member.path_bytes();
            let name = std::str::from_utf8(&path)?.to_owned();
            if !member.header().entry_type().is_file()
                || member.size() > CHECKPOINT_LIMIT
                || (name != "manifest.json" && !STATE.contains(&name.as_str()))
                || members.contains_key(&name)
            {
                return Err("Unsafe checkpoint member".into());
            }
            let mut bytes = Vec::new();
            member.read_to_end(&mut bytes)?;
            members.insert(name, bytes);
        }
        if members.len() != STATE.len() + 1 {
            return Err("Unexpected checkpoint members".into());
        }
        let manifest: Manifest = serde_json::from_slice(&members["manifest.json"])?;
        if manifest.context != self.context
            || manifest.inputs.keys().map(String::as_str).ne(INPUTS)
            || manifest.outputs.keys().map(String::as_str).ne(STATE)
        {
            return Err("Checkpoint provenance/context changed; resume is blocked".into());
        }
        if !matches(&self.root, &manifest.inputs, &INPUTS) {
            return Err("Fresh source input changed; resume is blocked".into());
        }
        if !matches(&self.root, &manifest.outputs, &DELIVERY) {
            return Err("Checkpoint matching-source delivery changed".into());
        }
        for name in STATE {
            if sha256_hex(&members[name]) != manifest.outputs[name].sha256 {
                return Err("Checkpoint output digest mismatch".into());
            }
        }
        for name in STATE {
            fs::write(self.root.join(name), &members[name])?;
        }
        for (name, entry) in manifest.inputs.iter().chain(&manifest.outputs) {
            set_times(&self.root.join(name), entry.mtime_ns)?;
        }
        self.build("restored-partial-checkpoint", "sample", 2, Some(7))?;
        self.build("unchanged", "sample", 0, Some(7))?;
        let header = self.root.join("value.h");
        fs::write(&header, b"#define VALUE 8\n")?;
        let mtime = self
            .root
            .join("a.o")
            .metadata()?
            .modified()?
            .duration_since(UNIX_EPOCH)?
            + Duration::from_secs(1);
        set_times(&header, mtime.as_nanos().try_into()?)?;
        self.build("header-invalidation", "sample", 2, Some(8))?;
        let expected = inventory(&self.root, &["value.h", "a.o"])?;
        for (name, entry) in &expected {
            let path = self.root.join(name);
            let original = fs::read(&path)?;
            let mut changed = original.clone();
            let index = changed
                .len()
                .checked_sub(2)
                .ok_or("Corruption fixture is too short")?;
            changed[index] ^= 1;
            fs::write(&path, changed)?;
            set_times(&path, entry.mtime_ns)?;
            let accepted = matches(&self.root, &expected, &["value.h", "a.o"]);
            fs::write(&path, original)?;
            set_times(&path, entry.mtime_ns)?;
            if accepted {
                return Err("Content guard accepted a corrupted input/output".into());
            }
            self.results.push(
                json!({"case":format!("same-size-mtime-corruption-{name}"),"result":"REJECTED"}),
            );
        }
        Ok(digest.to_owned())
    }
}

const USAGE: &str =
    "Usage: numera-siso-checkpoint-probe seed | resume --checkpoint PATH --expected-sha SHA256";

enum Options {
    Seed,
    Resume { checkpoint: PathBuf, digest: String },
}

fn options() -> Result<Option<Options>> {
    let (mut phase, mut checkpoint, mut digest) = (None, None, None);
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        let (flag, inline) = match argument.to_str().and_then(|text| text.split_once('=')) {
            Some((flag @ ("--checkpoint" | "--expected-sha"), value)) => (Some(flag), Some(value)),
            _ => (argument.to_str(), None),
        };
        let mut value = || {
            inline
                .map(std::ffi::OsString::from)
                .or_else(|| arguments.next())
                .ok_or(USAGE)
        };
        match flag {
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(None);
            }
            Some("seed") if phase.is_none() => phase = Some("seed"),
            Some("resume") if phase.is_none() => phase = Some("resume"),
            Some("--checkpoint") => checkpoint = Some(PathBuf::from(value()?)),
            Some("--expected-sha") => digest = Some(value()?.into_string().map_err(|_| USAGE)?),
            _ => return Err(USAGE.into()),
        }
    }
    let options = match phase {
        Some("seed") => Options::Seed,
        Some("resume") => Options::Resume {
            checkpoint: checkpoint.ok_or(USAGE)?,
            digest: digest.ok_or(USAGE)?,
        },
        _ => return Err(USAGE.into()),
    };
    Ok(Some(options))
}

fn run(options: Options) -> Result<()> {
    let mut probe = Probe::new()?;
    let (phase, digest) = match options {
        Options::Seed => ("seed", probe.seed()?),
        Options::Resume { checkpoint, digest } => ("resume", probe.resume(&checkpoint, &digest)?),
    };
    let result = json!({"schema_revision":1,"phase":phase,"context":probe.context,"results":probe.results,
        "execution_context":if report::getenv("GITHUB_ACTIONS") == "true" { "github-actions" } else { "local-simulation" },
        "job":env::var("GITHUB_JOB").ok(),"host_runner_image":env::var("ImageVersion").ok(),"checkpoint_sha256":digest,
        "engine_source_acceptance":"NOT_RUN","limits":"Tiny authored Linux C fixture only. Full Chromium outputs, transitive toolchain/SDK provenance, Windows resume, storage allowance and final-link fit remain unproven."});
    report::write_json(&probe.output.join("report.json"), &result)?;
    report::log(
        "INFO",
        "siso-checkpoint-probe",
        &format!("PASS: tiny {phase} checkpoint fixture; engine acceptance remains unproven"),
    );
    Ok(())
}

fn main() -> ExitCode {
    let options = match options() {
        Ok(None) => return ExitCode::SUCCESS,
        Ok(Some(options)) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report::log("ERROR", "siso-checkpoint-probe", &error.to_string());
            ExitCode::FAILURE
        }
    }
}
