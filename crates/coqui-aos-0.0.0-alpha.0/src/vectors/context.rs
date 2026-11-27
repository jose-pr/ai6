//! `context.json`: a project's context files for a working directory.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Report, Runner};
use crate::fs::{FileKind, FileSystem, Metadata};
use crate::{ProjectContext, context_files};

/// The case's files, and the directories above them, as a filesystem.
#[derive(Default)]
struct Memory {
    entries: BTreeMap<PathBuf, FileKind>,
}

impl Memory {
    fn of(case: &Value) -> Self {
        let mut memory = Self::default();
        for path in case["files"].as_array().expect("files") {
            let path = PathBuf::from(path.as_str().expect("a path"));
            for ancestor in path.ancestors().skip(1) {
                memory
                    .entries
                    .insert(ancestor.to_path_buf(), FileKind::Directory);
            }
            memory.entries.insert(path, FileKind::File);
        }
        memory
    }
}

impl FileSystem for Memory {
    fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        self.entries
            .get(path)
            .map(|kind| Metadata::new(*kind, 0))
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn read(&self, _path: &Path, _max: u64) -> io::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn list(&self, path: &Path) -> io::Result<Vec<OsString>> {
        if self.entries.get(path) != Some(&FileKind::Directory) {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        Ok(self
            .entries
            .keys()
            .filter(|entry| entry.parent() == Some(path))
            .filter_map(|entry| entry.file_name().map(OsString::from))
            .collect())
    }
}

fn texts(case: &Value, key: &str) -> Vec<String> {
    case.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().expect("text").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn shown(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect()
}

fn context_case(case: &Value) -> Result<(), String> {
    let text = |key: &str| {
        case["env"]
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let query = ProjectContext::new(
        case.get("root").and_then(Value::as_str).map(PathBuf::from),
        PathBuf::from(case["cwd"].as_str().expect("cwd")),
    )
    .agents_dir(text("AGENTS_DIR"))
    .agents_context(text("AGENTS_CONTEXT"))
    .separator(if case["os"] == "windows" { ';' } else { ':' })
    .native(texts(case, "native"))
    .delivered(texts(case, "delivered"));
    let got = context_files(&Memory::of(case), &query);
    let expect = &case["expect"];
    let compare = |key: &str, got: Vec<String>| -> Result<(), String> {
        let want = if expect.get(key).is_some() {
            texts(expect, key)
        } else if key == "ignored" {
            Vec::new()
        } else {
            return Ok(());
        };
        (got == want)
            .then_some(())
            .ok_or_else(|| format!("{key} {got:?}, expected {want:?}"))
    };
    compare("files", shown(&got.files))?;
    compare("skipped", shown(&got.skipped))?;
    compare("ignored", got.ignored)?;
    compare("levels", shown(&got.levels))
}

pub(super) fn context(runner: &Runner) -> Report {
    runner.each("context.json", context_case)
}
