//! Finding a harness's own record of a Session on the host it ran on, for a handler's
//! [`session_record`](super::agent::AgentHandler::session_record) answer.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The home directory a harness started with `environment` uses: its `HOME` (or
/// `USERPROFILE`), else this process's.
pub fn home(environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|name| environment.get(*name).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .or_else(|| {
            ["HOME", "USERPROFILE"]
                .iter()
                .find_map(std::env::var_os)
                .map(PathBuf::from)
        })
}

/// `environment`'s value of `name` as a directory, else `fallback` under [`home`].
pub fn configured_dir(
    environment: &BTreeMap<String, String>,
    name: &str,
    fallback: &str,
) -> Option<PathBuf> {
    environment
        .get(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home(environment).map(|home| home.join(fallback)))
}

/// Whether `session` can be part of a file name: no separator, no `..`, nothing that is not
/// printable.
pub fn file_safe(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 256
        && session != "."
        && !session.contains("..")
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The first file under `root`, at most `depth` directories down, whose name `wanted`
/// accepts; directories are read in name order, newest names last.
pub fn find_file(root: &Path, depth: usize, wanted: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(root).ok()?.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut dirs = Vec::new();
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_file() && entry.file_name().to_str().is_some_and(wanted) {
            return Some(path);
        }
        if kind.is_dir() {
            dirs.push(path);
        }
    }
    if depth == 0 {
        return None;
    }
    dirs.iter()
        .rev()
        .find_map(|dir| find_file(dir, depth - 1, wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_found_by_name_within_its_depth_and_unsafe_names_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let deep = root.path().join("2026").join("11").join("04");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("rollout-1-abc.jsonl"), "{}\n").unwrap();
        let wanted = |name: &str| name.ends_with("-abc.jsonl");
        assert_eq!(
            find_file(root.path(), 3, &wanted),
            Some(deep.join("rollout-1-abc.jsonl"))
        );
        assert_eq!(find_file(root.path(), 2, &wanted), None);
        assert!(file_safe("0195a-b_c.d"));
        for bad in ["", "../x", "a/b", "a\\b", "..", "a b"] {
            assert!(!file_safe(bad), "{bad:?}");
        }
        let env = BTreeMap::from([("CODEX_HOME".to_owned(), "/x/codex".to_owned())]);
        assert_eq!(
            configured_dir(&env, "CODEX_HOME", ".codex"),
            Some(PathBuf::from("/x/codex"))
        );
        let env = BTreeMap::from([("HOME".to_owned(), "/home/m".to_owned())]);
        assert_eq!(
            configured_dir(&env, "CODEX_HOME", ".codex"),
            Some(PathBuf::from("/home/m").join(".codex"))
        );
    }
}
