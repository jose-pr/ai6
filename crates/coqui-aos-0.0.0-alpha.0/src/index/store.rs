//! An index on disk: generations written beside the committed one and swapped in by one rename of `CURRENT`, so a failed write
//! leaves the previous index as it was.
//!
//! ```text
//! <dir>/CURRENT            the name of the committed generation, e.g. gen-000003
//! <dir>/gen-000003/meta.json      settings, source digest, each file's digest
//! <dir>/gen-000003/chunks.jsonl   one passage a line: path, heading, lines, text, vector (little-endian f32, base64)
//! ```

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use std::fs::{self, File};
use std::io::{BufWriter, Write as _};
use std::path::Path;

use data_encoding::BASE64;
use serde::{Deserialize, Serialize};

use super::{Chunk, Error, Settings};

/// The layout version a reader of these files expects.
const FORMAT: u32 = 1;
const CURRENT: &str = "CURRENT";
const META: &str = "meta.json";
const CHUNKS: &str = "chunks.jsonl";
const PREFIX: &str = "gen-";

/// What an index holds besides its passages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Meta {
    pub format: u32,
    pub settings: Settings,
    /// The digest of what the index was made from, as the caller named it (an overlay's tree digest).
    pub source: Option<String>,
    /// Each indexed file's SHA-256, by path.
    pub files: BTreeMap<String, String>,
}

impl Meta {
    pub(super) fn new(
        settings: Settings,
        source: Option<String>,
        files: BTreeMap<String, String>,
    ) -> Self {
        Self {
            format: FORMAT,
            settings,
            source,
            files,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Line {
    path: String,
    heading: String,
    from: usize,
    to: usize,
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vector: Option<String>,
}

/// A committed index as read.
pub(super) struct Loaded {
    pub generation: u64,
    pub meta: Meta,
    pub chunks: Vec<Chunk>,
}

fn generation_name(number: u64) -> String {
    alloc::format!("{PREFIX}{number:06}")
}

fn number_of(name: &str) -> Option<u64> {
    name.strip_prefix(PREFIX)?.parse().ok()
}

/// Reads the committed index in `dir`, or `None` when there is none.
pub(super) fn read(dir: &Path) -> Result<Option<Loaded>, Error> {
    // A writer may remove the generation named between reading `CURRENT` and opening it: read `CURRENT` again.
    let mut last = None;
    for _ in 0..3 {
        let pointer = dir.join(CURRENT);
        let name = match fs::read_to_string(&pointer) {
            Ok(name) => name,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Error::io(&pointer, &error)),
        };
        let name = name.trim();
        let Some(number) = number_of(name) else {
            return Err(Error::Format(alloc::format!(
                "{} names no generation",
                pointer.display()
            )));
        };
        match read_generation(&dir.join(name), number) {
            Ok(loaded) => return Ok(Some(loaded)),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| Error::Format("an index could not be read".into())))
}

fn read_generation(path: &Path, generation: u64) -> Result<Loaded, Error> {
    let meta_path = path.join(META);
    let text = fs::read_to_string(&meta_path).map_err(|error| Error::io(&meta_path, &error))?;
    let meta: Meta = serde_json::from_str(&text)
        .map_err(|error| Error::Format(alloc::format!("{}: {error}", meta_path.display())))?;
    if meta.format != FORMAT {
        return Err(Error::Format(alloc::format!(
            "{} is format {}, this reads {FORMAT}",
            meta_path.display(),
            meta.format
        )));
    }
    let chunks_path = path.join(CHUNKS);
    let text = fs::read_to_string(&chunks_path).map_err(|error| Error::io(&chunks_path, &error))?;
    let mut chunks = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let bad = |why: String| {
            Error::Format(alloc::format!(
                "{} line {}: {why}",
                chunks_path.display(),
                number + 1
            ))
        };
        let line: Line = serde_json::from_str(line).map_err(|error| bad(error.to_string()))?;
        let vector = line
            .vector
            .map(|encoded| decode(&encoded).map_err(&bad))
            .transpose()?;
        chunks.push(Chunk {
            path: line.path,
            heading: line.heading,
            from: line.from,
            to: line.to,
            text: line.text,
            vector,
        });
    }
    Ok(Loaded {
        generation,
        meta,
        chunks,
    })
}

fn encode(vector: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    BASE64.encode(&bytes)
}

fn decode(text: &str) -> Result<Vec<f32>, String> {
    let bytes = BASE64
        .decode(text.as_bytes())
        .map_err(|error| error.to_string())?;
    if bytes.len() % 4 != 0 {
        return Err("a vector is not whole floats".into());
    }
    let (parts, _) = bytes.as_chunks::<4>();
    Ok(parts.iter().map(|part| f32::from_le_bytes(*part)).collect())
}

/// Writes `meta` and `chunks` as the next generation of `dir`, makes it the committed one and removes the others. On an error
/// nothing is committed and what this call wrote is removed.
pub(super) fn commit(dir: &Path, meta: &Meta, chunks: &[Chunk]) -> Result<u64, Error> {
    fs::create_dir_all(dir).map_err(|error| Error::io(dir, &error))?;
    let mut newest = 0;
    let mut stale = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| Error::io(dir, &error))? {
        let entry = entry.map_err(|error| Error::io(dir, &error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".tmp") {
            stale.push(entry.path());
        } else if let Some(number) = number_of(&name) {
            newest = newest.max(number);
        }
    }
    for path in stale {
        remove(&path);
    }
    let number = newest + 1;
    let staging = dir.join(alloc::format!("{}.tmp", generation_name(number)));
    let result = write_generation(dir, &staging, number, meta, chunks);
    if result.is_err() {
        remove(&staging);
    }
    result?;
    // Everything but the committed generation goes; a reader still holding one finds `CURRENT` moved and reads it again.
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if number_of(&name).is_some_and(|other| other != number) {
                remove(&entry.path());
            }
        }
    }
    Ok(number)
}

fn write_generation(
    dir: &Path,
    staging: &Path,
    number: u64,
    meta: &Meta,
    chunks: &[Chunk],
) -> Result<(), Error> {
    fs::create_dir(staging).map_err(|error| Error::io(staging, &error))?;
    let meta_text =
        serde_json::to_string_pretty(meta).map_err(|error| Error::Format(error.to_string()))?;
    write_file(&staging.join(META), |out| {
        out.write_all(meta_text.as_bytes())
    })?;
    write_file(&staging.join(CHUNKS), |out| {
        for chunk in chunks {
            let line = Line {
                path: chunk.path.clone(),
                heading: chunk.heading.clone(),
                from: chunk.from,
                to: chunk.to,
                text: chunk.text.clone(),
                vector: chunk.vector.as_deref().map(encode),
            };
            serde_json::to_writer(&mut *out, &line).map_err(std::io::Error::from)?;
            out.write_all(b"\n")?;
        }
        Ok(())
    })?;
    let name = generation_name(number);
    let target = dir.join(&name);
    fs::rename(staging, &target).map_err(|error| Error::io(&target, &error))?;
    let pointer_staging = dir.join("CURRENT.tmp");
    let pointer = dir.join(CURRENT);
    let written =
        write_file(&pointer_staging, |out| out.write_all(name.as_bytes())).and_then(|()| {
            fs::rename(&pointer_staging, &pointer).map_err(|error| Error::io(&pointer, &error))
        });
    if written.is_err() {
        remove(&pointer_staging);
        // The new generation is not the committed one: take it away so a later write does not meet it.
        remove(&target);
    }
    written
}

fn write_file(
    path: &Path,
    body: impl FnOnce(&mut BufWriter<File>) -> std::io::Result<()>,
) -> Result<(), Error> {
    let file = File::create(path).map_err(|error| Error::io(path, &error))?;
    let mut out = BufWriter::new(file);
    body(&mut out).map_err(|error| Error::io(path, &error))?;
    out.flush().map_err(|error| Error::io(path, &error))?;
    out.into_inner()
        .map_err(|error| Error::io(path, error.error()))?
        .sync_all()
        .map_err(|error| Error::io(path, &error))
}

/// Removes a file or a folder under an index's directory; a failure is left for the next write to meet again.
fn remove(path: &Path) {
    if fs::remove_dir_all(path).is_err() {
        let _ = fs::remove_file(path);
    }
}
