// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Copying a directory tree for a Mission, and re-attaching to the copy later.

use std::fs;
use std::path::{Path, PathBuf};

use ai6::directorates::q::safehouse::SafeHouseSpec;

/// An open safe house: a copy of the source, or the source itself when not isolated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tree {
    pub(crate) path: PathBuf,
    pub(crate) isolated: bool,
}

impl Tree {
    /// Copies, re-attaches to or adopts the safe house `spec` describes.
    pub(crate) fn open(spec: &SafeHouseSpec, name_chars: usize) -> Result<Self, String> {
        let source = canonical(Path::new(&spec.source))?;
        if !source.is_dir() {
            return Err(format!("{} is not a directory", source.display()));
        }
        if !spec.isolate {
            return Ok(Self {
                path: source,
                isolated: false,
            });
        }
        let root = Path::new(&spec.state_root);
        fs::create_dir_all(root).map_err(|error| format!("{}: {error}", root.display()))?;
        let root = canonical(root)?;
        let path = root
            .join("safe-houses")
            .join(segment(&spec.operation_id, name_chars))
            .join(segment(&spec.mission_id, name_chars));
        if let Some(report) = &spec.reopen {
            let kept = canonical(Path::new(&report.path))?;
            if !report.kept || kept != path || !kept.is_dir() {
                return Err("safe-house report does not match an existing copy".to_owned());
            }
            return Ok(Self {
                path: kept,
                isolated: true,
            });
        }
        if root.starts_with(&source) {
            return Err(format!(
                "safe-house root {} must be outside source {}",
                root.display(),
                source.display()
            ));
        }
        if path.exists() {
            return Err(format!("safe-house path {} already exists", path.display()));
        }
        copy_tree(&source, &path)?;
        Ok(Self {
            path,
            isolated: true,
        })
    }
}

/// Copies files and directories; a symbolic link is skipped so the copy never reaches
/// outside the source.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))? {
        let entry = entry.map_err(|error| format!("{}: {error}", from.display()))?;
        let kind = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)
                .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(strip_verbatim(path))
}

/// Drops the verbatim prefix `canonicalize` adds on Windows, so paths compare and print as
/// the Agent will see them.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

/// A directory name for `value`: at most `name_chars` safe characters plus a hash, so distinct IDs never collide.
fn segment(value: &str, name_chars: usize) -> String {
    let stem: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .take(name_chars)
        .collect();
    let stem = stem.trim_matches('-');
    let stem = if stem.is_empty() { "id" } else { stem };
    let hash = value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{stem}-{hash:016x}")
}
