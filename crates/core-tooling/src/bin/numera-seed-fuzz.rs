use core_contracts::{Empty, Hello, LaunchSnapshot};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::ser::{Formatter, Serializer};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

#[derive(Clone, Deserialize, Serialize)]
struct Frame<Payload> {
    namespace: String,
    protocol: u16,
    id: String,
    deadline_ms: u32,
    operation: String,
    payload: Payload,
}

/// Retain the original corpus spacing and Unicode escapes for byte-for-byte replay.
struct CorpusFormatter;

impl Formatter for CorpusFormatter {
    fn begin_array_value<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b": ")
    }

    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        let mut wide = [0u16; 2];
        for ch in fragment.chars() {
            if ch.is_ascii() && ch != '\u{7f}' {
                writer.write_all(&[ch as u8])?;
            } else {
                for unit in ch.encode_utf16(&mut wide) {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }
}

fn read_fixture<T: DeserializeOwned>(fixtures: &Path, name: &str) -> Result<T, Box<dyn Error>> {
    let value = core_contracts::strict_json(&fs::read(fixtures.join(name))?)
        .map_err(|_| format!("Invalid wire fixture {name}"))?;
    Ok(serde_json::from_value(value)?)
}

fn append_frame(output: &mut impl Write, frame: &impl Serialize) -> Result<(), Box<dyn Error>> {
    frame.serialize(&mut Serializer::with_formatter(
        &mut *output,
        CorpusFormatter,
    ))?;
    output.write_all(if cfg!(windows) { b"\r\n" } else { b"\n" })?;
    Ok(())
}

fn seed(root: &Path) -> Result<(), Box<dyn Error>> {
    let fixtures = root.join("contracts/fixtures");
    let corpus = root.join("fuzz/corpus/wire");
    fs::create_dir_all(&corpus)?;
    for source in fs::read_dir(&fixtures)? {
        let source = source?;
        if matches!(
            source.path().extension().and_then(|suffix| suffix.to_str()),
            Some("json" | "ndjson")
        ) {
            fs::copy(source.path(), corpus.join(source.file_name()))?;
        }
    }

    let hello: Frame<Hello> = read_fixture(&fixtures, "hello.json")?;
    let capabilities: Frame<Empty> = read_fixture(&fixtures, "capabilities.json")?;
    let mut snapshot: LaunchSnapshot = read_fixture(&fixtures, "launch-desktop.json")?;
    let unused_path = root.join("fuzz/never-create-this-profile");
    let unused_path = unused_path
        .to_str()
        .ok_or("Fuzz fixtures require a UTF-8 path")?;
    snapshot.resources.native_profile_dir = unused_path.to_owned();
    snapshot.resources.data_root = unused_path.to_owned();
    snapshot.resources.cache_root = unused_path.to_owned();

    let mut output = BufWriter::new(File::create(corpus.join("conversation.ndjson"))?);
    append_frame(&mut output, &hello)?;
    append_frame(&mut output, &capabilities)?;
    for (index, operation) in ["launch.plan", "session.launch"].into_iter().enumerate() {
        append_frame(
            &mut output,
            &Frame {
                namespace: hello.namespace.clone(),
                protocol: hello.protocol,
                id: format!("fuzz-seed-{index}"),
                deadline_ms: hello.deadline_ms,
                operation: operation.to_owned(),
                payload: &snapshot,
            },
        )?;
    }
    output.flush()?;
    Ok(())
}

fn main() -> ExitCode {
    match std::env::current_dir()
        .map_err(Into::into)
        .and_then(|root| seed(&root))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Fuzz seed generation failed: {error}");
            ExitCode::FAILURE
        }
    }
}
