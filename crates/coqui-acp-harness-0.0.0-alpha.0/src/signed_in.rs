//! The parts of [`AccountProvider::signed_in`](coqui_acp_api::AccountProvider::signed_in) every provider shares: the
//! variable a login may come from, the file it may be kept in, and the answer, which names what was looked at and never what
//! was found.
//!
//! Nothing here starts a program or reaches the network. A file is read only as far as it takes to tell whether anything is
//! in it, and no text of one is kept past that.

use alloc::string::{String, ToString};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use coqui_acp_api::{Environment, SignedInCheck};

/// The most of a login file that is read to tell whether it holds a login: a real one is a few hundred bytes.
pub const FILE_LIMIT: u64 = 1 << 20;

/// The value of `name` in `environment`, else in this process's environment, when it is set and not empty.
#[must_use]
pub fn value(environment: &Environment, name: &str) -> Option<String> {
    let named = environment
        .get(name)
        .map(ToString::to_string)
        .or_else(|| {
            // Windows' variable names are not case sensitive, and a map built from its block keeps the spelling it was given.
            cfg!(windows)
                .then(|| {
                    environment
                        .iter()
                        .find(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.clone())
                })
                .flatten()
        })
        .or_else(|| std::env::var(name).ok());
    named.filter(|text| !text.trim().is_empty())
}

/// The first of `names` that is set and not empty, as its name.
#[must_use]
pub fn first_set<'a>(environment: &Environment, names: &[&'a str]) -> Option<&'a str> {
    names
        .iter()
        .copied()
        .find(|name| value(environment, name).is_some())
}

/// The answer when one of `names` is set: signed in, by that variable.
#[must_use]
pub fn by_variable(environment: &Environment, names: &[&str]) -> Option<SignedInCheck> {
    first_set(environment, names).map(SignedInCheck::by_variable)
}

/// The directory `home` names: `USERPROFILE` on Windows, else `HOME`, from `environment` and then this process's.
#[must_use]
pub fn home(environment: &Environment) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["USERPROFILE", "HOME"]
    } else {
        &["HOME"]
    };
    names
        .iter()
        .find_map(|name| value(environment, name))
        .map(PathBuf::from)
}

/// What a login file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// There is no file.
    Missing,
    /// There is a file and nothing in it: no bytes, blank text, or JSON that is an empty object, an empty array or `null`.
    Empty,
    /// There is a file with something in it.
    Present,
    /// The file could not be read: a permission, a folder in its place, a file past [`FILE_LIMIT`].
    Unreadable,
}

/// What the file at `path` holds.
#[must_use]
pub fn file_state(path: &Path) -> FileState {
    let mut bytes = alloc::vec::Vec::new();
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return FileState::Missing,
        Err(_) => return FileState::Unreadable,
    };
    if file.take(FILE_LIMIT + 1).read_to_end(&mut bytes).is_err() {
        return FileState::Unreadable;
    }
    if bytes.len() as u64 > FILE_LIMIT {
        return FileState::Unreadable;
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return FileState::Empty;
    }
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(serde_json::Value::Null) => FileState::Empty,
        Ok(serde_json::Value::Object(map)) if map.is_empty() => FileState::Empty,
        Ok(serde_json::Value::Array(list)) if list.is_empty() => FileState::Empty,
        _ => FileState::Present,
    }
}

/// The answer for a program whose login is a file `place` names (`"Grok's auth.json"`) and whose own variables are `names`.
///
/// `path` is the file, or `None` when the folder it is in could not be worked out. The reason says what was looked at in
/// words, never a path, since a path may be a value of the environment.
#[must_use]
pub fn answer_for_file(
    environment: &Environment,
    names: &[&str],
    path: Option<&Path>,
    place: &str,
) -> SignedInCheck {
    if let Some(found) = by_variable(environment, names) {
        return found;
    }
    let Some(path) = path else {
        return SignedInCheck::unknown(alloc::format!(
            "the folder {place} is kept in could not be worked out"
        ));
    };
    match file_state(path) {
        FileState::Present => SignedInCheck::signed_in(),
        FileState::Missing => SignedInCheck::signed_out(alloc::format!(
            "{place} is not there{}",
            variables_note(names)
        )),
        FileState::Empty => SignedInCheck::signed_out(alloc::format!(
            "{place} holds no login{}",
            variables_note(names)
        )),
        FileState::Unreadable => {
            SignedInCheck::unknown(alloc::format!("{place} could not be read"))
        }
    }
}

/// The words that say which variables were looked at too.
#[must_use]
pub fn variables_note(names: &[&str]) -> String {
    if names.is_empty() {
        String::new()
    } else {
        alloc::format!(", and none of {} is set", names.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(alloc::format!(
            "coqui-acp-signed-in-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");
        dir
    }

    #[test]
    fn a_file_is_missing_empty_or_present() {
        let dir = folder("states");
        let at = |name: &str, text: &str| {
            let path = dir.join(name);
            std::fs::write(&path, text).expect("a file");
            file_state(&path)
        };
        assert_eq!(file_state(&dir.join("none")), FileState::Missing);
        assert_eq!(at("zero", ""), FileState::Empty);
        assert_eq!(at("blank", " \r\n"), FileState::Empty);
        assert_eq!(at("object", "{}"), FileState::Empty);
        assert_eq!(at("null", "null\n"), FileState::Empty);
        assert_eq!(at("list", "[]"), FileState::Empty);
        assert_eq!(at("login", r#"{"token":"x"}"#), FileState::Present);
        assert_eq!(at("text", "not json"), FileState::Present);
        assert_eq!(file_state(&dir), FileState::Unreadable, "a folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_variable_counts_when_it_is_not_blank() {
        let environment = Environment::from_iter([("A_KEY", "secret-a"), ("B_KEY", "  ")]);
        assert_eq!(first_set(&environment, &["B_KEY", "A_KEY"]), Some("A_KEY"));
        assert_eq!(first_set(&environment, &["B_KEY"]), None);
        let found = by_variable(&environment, &["A_KEY"]).expect("signed in by it");
        assert_eq!(found, SignedInCheck::by_variable("A_KEY"));
        assert!(!alloc::format!("{found:?}").contains("secret-a"));
    }

    #[test]
    fn the_reason_names_what_was_looked_at_and_no_path() {
        let dir = folder("reason");
        let environment = Environment::new();
        let answer = answer_for_file(
            &environment,
            &["COQUI_ACP_NO_SUCH_KEY"],
            Some(&dir.join("auth.json")),
            "Grok's auth.json",
        );
        let reason = answer.reason.expect("signed out with a reason");
        assert!(reason.contains("Grok's auth.json"), "{reason}");
        assert!(reason.contains("COQUI_ACP_NO_SUCH_KEY"), "{reason}");
        assert!(!reason.contains(&*dir.to_string_lossy()), "{reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
