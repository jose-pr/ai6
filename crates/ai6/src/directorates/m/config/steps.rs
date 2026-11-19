//! One-time steps: a stored record rewritten, once, from the shape an earlier release wrote to the
//! one this release reads. `ai6 config migrate` runs every step against the configuration the
//! bootstrap file names, a SQL database or a directory of family files.
//!
//! A step is named, runs again with nothing left to change, and is reported by name. Once it has
//! run, the release reads only the new shape: a record still in the old one is refused, and the
//! refusal names the step.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{ConfigRecord, ConfigScope};
use crate::bootstrap::format::EXTENSIONS;

/// The step that renames the account kind `openai-api-key` to `codex-api-key`, the id Codex's
/// account provider gives it.
pub const CODEX_API_KEY_KIND: &str = "codex-api-key-kind";

/// An account kind an earlier release spelled otherwise: the old spelling, the new one and the
/// step that moved every stored record from one to the other.
const RENAMED_KINDS: [(&str, &str, &str); 1] =
    [("openai-api-key", "codex-api-key", CODEX_API_KEY_KIND)];

/// The refusal for an account kind that a one-time step renamed, or `None` for any other kind.
pub(crate) fn renamed_kind_refusal(kind: &str) -> Option<String> {
    RENAMED_KINDS.iter().find(|(was, ..)| *was == kind).map(|(was, now, step)| {
        format!(
            "kind {was:?} is {now:?} since the migration step {step:?}, which has not been run on \
             this configuration: run `ai6 config migrate` once with every Station and gateway \
             stopped"
        )
    })
}

/// One step: the family it rewrites and how it rewrites one record.
struct Step {
    name: &'static str,
    says: &'static str,
    family: ConfigRecord,
    /// Rewrites the record; `true` when it was in the old shape.
    apply: fn(&mut Value) -> bool,
}

/// Every step, oldest first. A new one goes at the end.
const STEPS: &[Step] = &[Step {
    name: CODEX_API_KEY_KIND,
    says: "an account's kind `openai-api-key` is `codex-api-key`",
    family: ConfigRecord::Pool,
    apply: account_kinds,
}];

fn account_kinds(definition: &mut Value) -> bool {
    let Some(accounts) = definition.get_mut("account").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    for account in accounts.values_mut() {
        let Some(kind) = account.get_mut("kind") else {
            continue;
        };
        let Some((_, now, _)) = RENAMED_KINDS
            .iter()
            .find(|(was, ..)| kind.as_str() == Some(was))
        else {
            continue;
        };
        *kind = Value::String((*now).to_owned());
        changed = true;
    }
    changed
}

/// A record or file a step rewrote.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stepped {
    /// The step's name.
    pub step: &'static str,
    /// What the step changes, in a sentence.
    pub says: &'static str,
    /// Where the change was made: a record as `<family> <key>` at its scope, or a file's path.
    pub place: String,
}

/// Applies every step of `family` to one stored record and returns the steps that changed it, as
/// `(name, says)`; a record no step changes is left as it was.
pub(super) fn rewrite(
    family: ConfigRecord,
    definition: &mut Value,
) -> Vec<(&'static str, &'static str)> {
    STEPS
        .iter()
        .filter(|step| step.family == family && (step.apply)(definition))
        .map(|step| (step.name, step.says))
        .collect()
}

/// Where a step changed a record: `<family> "<key>"`, and its scope when a Station's own.
pub(super) fn place(family: ConfigRecord, scope: &ConfigScope, key: &str) -> String {
    match scope {
        ConfigScope::Global => format!("{family} {key:?}"),
        other => format!("{family} {key:?} at {}", other.as_text()),
    }
}

/// Runs every step on the family files of the directory `root` (and of each
/// `stations/<id>/` under it), rewriting the old spelling in place and leaving comments,
/// ordering and every other line as they are. A second run finds nothing.
///
/// # Errors
///
/// An I/O failure reading or writing a family file.
pub fn run_on_files(root: &Path) -> io::Result<Vec<Stepped>> {
    let mut dirs = vec![root.to_path_buf()];
    if let Ok(entries) = fs::read_dir(root.join("stations")) {
        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                dirs.push(path);
            }
        }
    }
    let mut done = Vec::new();
    for dir in dirs {
        for step in STEPS {
            for path in family_files(&dir, step.family) {
                let text = fs::read_to_string(&path)?;
                let Some(moved) = respell_kinds(&text) else {
                    continue;
                };
                fs::write(&path, moved)?;
                done.push(Stepped {
                    step: step.name,
                    says: step.says,
                    place: path.display().to_string(),
                });
            }
        }
    }
    Ok(done)
}

fn family_files(dir: &Path, family: ConfigRecord) -> Vec<PathBuf> {
    EXTENSIONS
        .iter()
        .map(|extension| dir.join(format!("{}.{extension}", family.file())))
        .filter(|path| path.is_file())
        .collect()
}

/// `text` with each account kind that a step renamed written as its new spelling, in a line that
/// sets `kind` in TOML, JSON or YAML; `None` when no line has one.
fn respell_kinds(text: &str) -> Option<String> {
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let mut line = line.to_owned();
        if line.contains("kind") {
            for (was, now, _) in RENAMED_KINDS {
                for (open, close) in [("\"", "\""), ("'", "'")] {
                    let from = format!("{open}{was}{close}");
                    if line.contains(&from) {
                        line = line.replace(&from, &format!("{open}{now}{close}"));
                        changed = true;
                    }
                }
                let bare = line.trim_end();
                let (body, ending) = line.split_at(bare.len());
                if let Some(head) = body.strip_suffix(was)
                    && head.trim_end().ends_with("kind:")
                {
                    line = format!("{head}{now}{ending}");
                    changed = true;
                }
            }
        }
        out.push_str(&line);
    }
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_pool_account_of_the_old_kind_is_respelled_and_nothing_else_moves() {
        let mut pool = json!({
            "handler": "codex",
            "account": {
                "carol": {"kind": "openai-api-key", "secret": "carol-key", "priority": 2},
                "dave": {"kind": "codex-access-token"},
                "erin": {"kind": "config-dir", "dir": "/logins/erin"},
                "frank": {"kind": "harness-login"},
            }
        });
        assert!(account_kinds(&mut pool));
        assert_eq!(
            pool,
            json!({
                "handler": "codex",
                "account": {
                    "carol": {"kind": "codex-api-key", "secret": "carol-key", "priority": 2},
                    "dave": {"kind": "codex-access-token"},
                    "erin": {"kind": "config-dir", "dir": "/logins/erin"},
                    "frank": {"kind": "harness-login"},
                }
            })
        );
        assert!(!account_kinds(&mut pool), "a second pass changes nothing");
        for odd in [
            json!({"handler": "codex"}),
            json!({"handler": "codex", "account": {"a": {"kind": 3}, "b": 4, "c": {}}}),
        ] {
            let mut odd = odd;
            let before = odd.clone();
            assert!(!account_kinds(&mut odd));
            assert_eq!(odd, before);
        }
    }

    #[test]
    fn the_old_kind_is_refused_with_the_step_that_moves_it() {
        let refusal = renamed_kind_refusal("openai-api-key").expect("a renamed kind");
        assert!(refusal.contains(CODEX_API_KEY_KIND), "{refusal}");
        assert!(refusal.contains("ai6 config migrate"), "{refusal}");
        assert!(renamed_kind_refusal("codex-api-key").is_none());
        assert!(renamed_kind_refusal("no-such-kind").is_none());
    }

    #[test]
    fn the_old_spelling_is_respelled_in_each_format_and_only_where_a_kind_is_set() {
        let toml = "# keys\n[pool.team.account.a]\nkind = \"openai-api-key\" # the key\n\
                    [pool.team.account.b]\nkind = 'openai-api-key'\n\
                    [secret.openai-api-key]\nenv = \"X\"\n";
        let moved = respell_kinds(toml).expect("two kinds");
        assert_eq!(
            moved,
            "# keys\n[pool.team.account.a]\nkind = \"codex-api-key\" # the key\n\
             [pool.team.account.b]\nkind = 'codex-api-key'\n\
             [secret.openai-api-key]\nenv = \"X\"\n"
        );
        assert!(
            respell_kinds(&moved).is_none(),
            "a second pass changes nothing"
        );
        let json = "{\"account\": {\"a\": {\"kind\": \"openai-api-key\"}}}\n";
        assert_eq!(
            respell_kinds(json).as_deref(),
            Some("{\"account\": {\"a\": {\"kind\": \"codex-api-key\"}}}\n")
        );
        let yaml = "account:\n  a:\n    kind: openai-api-key\n  b:\n    kind: \"openai-api-key\"\n";
        assert_eq!(
            respell_kinds(yaml).as_deref(),
            Some("account:\n  a:\n    kind: codex-api-key\n  b:\n    kind: \"codex-api-key\"\n")
        );
        assert!(respell_kinds("name = \"openai-api-key\"\n").is_none());
    }
}
