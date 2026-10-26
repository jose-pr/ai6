//! What a kept safe house holds on disk: its size, its rebuildable caches and when it last
//! changed. A walk never follows a link, so it never leaves the safe house.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::station::safehouse::SafeHouseHygiene;

/// Entries one walk reads before it stops and reports a partial size.
pub(super) const MAX_ENTRIES: usize = 500_000;

/// A cache directory found in a safe house.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CacheDir {
    /// Its path relative to the safe house, with `/` separators.
    pub(super) path: String,
    /// Its size in bytes, as far as the walk got.
    pub(super) size_bytes: u64,
    #[serde(skip)]
    pub(super) absolute: PathBuf,
}

/// What a walk of one safe house found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Survey {
    /// Total size in bytes, as far as the walk got.
    pub(super) size_bytes: u64,
    /// Whether the walk read every entry; `false` past [`MAX_ENTRIES`].
    pub(super) size_complete: bool,
    /// Seconds since the safe house or anything at its top level last changed.
    pub(super) age_secs: Option<u64>,
    /// The cache directories in it, outermost only.
    pub(super) caches: Vec<CacheDir>,
}

/// Walks `root`, counting file sizes and finding the directories `hygiene` names as caches.
/// `.git` is never a cache, and a cache inside a cache is part of the outer one.
pub(super) fn survey(root: &Path, hygiene: &SafeHouseHygiene, now: SystemTime) -> Survey {
    let mut survey = Survey {
        size_bytes: 0,
        size_complete: true,
        age_secs: None,
        caches: Vec::new(),
    };
    let mut newest = modified(root);
    // (directory, depth, the cache it lies in)
    let mut pending: Vec<(PathBuf, usize, Option<usize>)> = vec![(root.to_path_buf(), 0, None)];
    let mut read = 0_usize;
    'walk: while let Some((directory, depth, cache)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            read += 1;
            if read > MAX_ENTRIES {
                survey.size_complete = false;
                break 'walk;
            }
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if depth == 0 {
                newest = newest.max(metadata.modified().ok());
            }
            if metadata.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                // Version control's own records are never a cache and are never walked.
                if name == ".git" {
                    continue;
                }
                let inner = match cache {
                    Some(index) => Some(index),
                    None if hygiene.is_cache(&name) => {
                        survey.caches.push(CacheDir {
                            path: relative(root, &path),
                            size_bytes: 0,
                            absolute: path.clone(),
                        });
                        Some(survey.caches.len() - 1)
                    }
                    None => None,
                };
                pending.push((path, depth + 1, inner));
            } else {
                // A link counts as its own small entry and is never followed.
                survey.size_bytes += metadata.len();
                if let Some(index) = cache {
                    survey.caches[index].size_bytes += metadata.len();
                }
            }
        }
    }
    survey.age_secs = newest
        .and_then(|newest| now.duration_since(newest).ok())
        .map(|age| age.as_secs());
    survey.caches.sort_by(|a, b| a.path.cmp(&b.path));
    survey
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::symlink_metadata(path).ok()?.modified().ok()
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_survey_sizes_the_tree_and_finds_outermost_caches() {
        let root = tempfile::tempdir().unwrap();
        let house = root.path();
        fs::create_dir_all(house.join("src")).unwrap();
        fs::write(house.join("src/main.rs"), [0_u8; 10]).unwrap();
        fs::create_dir_all(house.join("target/debug/target")).unwrap();
        fs::write(house.join("target/debug/app"), [0_u8; 100]).unwrap();
        fs::write(house.join("target/debug/target/x"), [0_u8; 5]).unwrap();
        fs::create_dir_all(house.join("web/node_modules/a")).unwrap();
        fs::write(house.join("web/node_modules/a/i.js"), [0_u8; 20]).unwrap();
        fs::create_dir_all(house.join(".git/refs/heads/target")).unwrap();
        fs::write(house.join(".git/refs/heads/target/x"), [0_u8; 1]).unwrap();

        let found = survey(house, &SafeHouseHygiene::default(), SystemTime::now());
        assert_eq!(found.size_bytes, 135, ".git is not walked");
        assert!(found.size_complete);
        assert!(found.age_secs.is_some_and(|age| age < 600));
        let caches = found
            .caches
            .iter()
            .map(|cache| (cache.path.as_str(), cache.size_bytes))
            .collect::<Vec<_>>();
        assert_eq!(caches, [("target", 105), ("web/node_modules", 20)]);

        let none = SafeHouseHygiene::default()
            .with_caches(Vec::<String>::new())
            .unwrap();
        assert!(survey(house, &none, SystemTime::now()).caches.is_empty());
        let later = SystemTime::now() + std::time::Duration::from_secs(8 * 24 * 3600);
        assert!(
            survey(house, &SafeHouseHygiene::default(), later)
                .age_secs
                .is_some_and(|age| age >= 8 * 24 * 3600)
        );
    }
}
