//! The person's memory as the app reads and changes it: a [`MemoryOp`] made in a clone of the
//! person's repository of its own, which goes afterwards, with no harness involved.
//!
//! A change is committed to `main` exactly as asked, authored by the person, and only there:
//! when `main` will not take it, nothing is saved anywhere else. A conversation running on
//! this or another Station keeps the memory it read until its harness starts again; a memory
//! change it makes from a file changed here meanwhile is refused, since its base no longer
//! matches.

use std::fs;
use std::path::Path;

use crate::directorates::m::assistant::{AssistantLimits, RepoKind};
use crate::directorates::q::safehouse::{CheckpointFile, CheckpointSpec};
use crate::directorates::section_00::assistant::{
    MemoryAction, MemoryAnswer, MemoryCommit, MemoryFile, MemoryOp,
};
use crate::station::safehouse::SafeHouse;

use super::workspace::{
    CloneLimits, Houses, MAIN, MEMORY, in_tree, no_links, open_clone, sha256_hex, walk,
};

/// Why an operation was refused, with what its answer says of it.
#[derive(Debug, Default)]
pub(super) struct Refusal {
    pub(super) reason: String,
    /// The file is not there.
    pub(super) missing: bool,
    /// The file no longer holds the write's base.
    pub(super) changed: bool,
}

impl Refusal {
    /// A refusal for `reason` alone.
    pub(super) fn of(reason: String) -> Self {
        Self {
            reason,
            ..Self::default()
        }
    }
}

/// Makes `op` in a fresh shallow clone of its repository, made within `limits`, a listing and
/// a read within `bounds`; `digest` names the clone's safe house. Answers what was done and
/// found, or why nothing was.
pub(super) fn run(
    houses: &Houses,
    op: &MemoryOp,
    digest: &str,
    limits: CloneLimits,
    bounds: &AssistantLimits,
) -> Result<(String, MemoryAnswer), Refusal> {
    let kind = RepoKind::of(&op.repo).map_err(plain)?;
    let changes = matches!(
        op.action,
        MemoryAction::Write { .. } | MemoryAction::Forget { .. }
    );
    // A local repository is created on the first change, as a conversation's would be; a look
    // at one not made yet finds an empty memory and makes nothing.
    if !changes && RepoKind::local_path(&op.repo).is_some_and(|path| !path.exists()) {
        return match &op.action {
            MemoryAction::Read { path } => Err(Refusal {
                reason: format!("{path} is not in the person's memory"),
                missing: true,
                ..Refusal::default()
            }),
            _ => Ok(("memory/ holds 0 files".to_owned(), MemoryAnswer::default())),
        };
    }
    let provider = houses().map_err(plain)?;
    let scratch = tempfile::Builder::new()
        .prefix("ai6-assistant-memory-")
        .tempdir()
        .map_err(|error| plain(error.to_string()))?;
    let root = scratch.path().join("repository");
    let shallow = CloneLimits {
        depth: Some(1),
        ..limits
    };
    let create = changes && kind == RepoKind::Local;
    let house = open_clone(
        provider.as_ref(),
        &op.repo,
        &root,
        None,
        create,
        digest,
        shallow,
    )
    .map_err(|reason| {
        plain(format!(
            "the personal repository could not be opened: {reason}"
        ))
    })?;
    let answer = MemoryAnswer {
        commit: house
            .base()
            .filter(|commit| !commit.is_empty())
            .map(str::to_owned),
        ..MemoryAnswer::default()
    };
    let outcome = act(&house, &root, &op.action, answer, bounds);
    let _ = house.discard();
    outcome
}

fn act(
    house: &SafeHouse,
    root: &Path,
    action: &MemoryAction,
    mut answer: MemoryAnswer,
    bounds: &AssistantLimits,
) -> Result<(String, MemoryAnswer), Refusal> {
    match action {
        MemoryAction::List {} => {
            let (files, more) = list(root, bounds);
            let said = format!(
                "memory/ holds {}{} files",
                files.len(),
                if more { " or more" } else { "" }
            );
            answer.files = files;
            answer.more = more;
            Ok((said, answer))
        }
        MemoryAction::Read { path } => {
            let text = read(root, path, bounds)?;
            answer.digest = Some(sha256_hex(text.as_bytes()));
            answer.text = Some(text);
            Ok((format!("read {path}"), answer))
        }
        MemoryAction::Write {
            path,
            text,
            base,
            commit,
        } => {
            if let Some(base) = base {
                let holds = match read(root, path, bounds) {
                    Ok(now) => Some(sha256_hex(now.as_bytes())),
                    Err(Refusal { missing: true, .. }) => None,
                    Err(refusal) => return Err(refusal),
                };
                if holds.as_deref() != Some(base.as_str()) {
                    return Err(Refusal {
                        reason: format!(
                            "{path} changed since it was opened, so nothing was saved: open it \
                             again and make the change from what it holds now"
                        ),
                        changed: true,
                        ..Refusal::default()
                    });
                }
            }
            let file = CheckpointFile {
                path: path.clone(),
                text: Some(text.clone()),
                // Checked again against `main` as the commit is made.
                base: base.clone(),
            };
            answer.commit = save(house, commit, file)?;
            answer.digest = Some(sha256_hex(text.as_bytes()));
            Ok((format!("saved {path}"), answer))
        }
        MemoryAction::Forget { path, commit } => {
            // Any file goes, text or not, large or not.
            present(root, path)?;
            let file = CheckpointFile {
                path: path.clone(),
                text: None,
                base: None,
            };
            answer.commit = save(house, commit, file)?;
            Ok((format!("forgot {path}"), answer))
        }
    }
}

/// The files under `memory/`, in path order, and whether there were more than fit an answer.
fn list(root: &Path, bounds: &AssistantLimits) -> (Vec<MemoryFile>, bool) {
    let mut found = Vec::new();
    let mut more = false;
    let files = bounds.memory_list_files.or_max();
    walk(&root.join(MEMORY), MEMORY, &mut |path, file| {
        // A bounded walk: past four times the answer's bound, the rest only make `more`.
        if found.len() > files.saturating_mul(4) {
            more = true;
        } else if let Ok(metadata) = fs::symlink_metadata(file) {
            found.push(MemoryFile {
                path,
                bytes: metadata.len(),
            });
        }
    });
    found.sort_by(|a, b| a.path.cmp(&b.path));
    let mut bytes = 0;
    let fits = found
        .iter()
        .take(files)
        .take_while(|file| {
            bytes += file.path.len();
            bounds.memory_list_bytes.allows(bytes)
        })
        .count();
    more |= fits < found.len();
    found.truncate(fits);
    (found, more)
}

/// The size of `path` in the clone at `root`, a plain file reached through no link.
fn present(root: &Path, path: &str) -> Result<u64, Refusal> {
    let file = in_tree(root, path);
    no_links(root, &file).map_err(|error| plain(error.to_string()))?;
    match fs::symlink_metadata(&file) {
        Ok(metadata) if metadata.is_file() => Ok(metadata.len()),
        _ => Err(Refusal {
            reason: format!("{path} is not in the person's memory"),
            missing: true,
            ..Refusal::default()
        }),
    }
}

/// The text of `path` in the clone at `root`, a plain file reached through no link.
fn read(root: &Path, path: &str, bounds: &AssistantLimits) -> Result<String, Refusal> {
    if !bounds.change_bytes.allows(present(root, path)?) {
        return Err(plain(format!(
            "{path} holds more than {} bytes; open it in the repository",
            bounds.change_bytes
        )));
    }
    let bytes = fs::read(in_tree(root, path)).map_err(|error| plain(format!("{path}: {error}")))?;
    String::from_utf8(bytes).map_err(|_| plain(format!("{path} is not text")))
}

/// Commits exactly `file` to `main` as the person and pushes it; the commit made, `None` when
/// the repository still has none. A `main` that refuses it twice refuses the change.
fn save(
    house: &SafeHouse,
    commit: &MemoryCommit,
    file: CheckpointFile,
) -> Result<Option<String>, Refusal> {
    let saved = house
        .checkpoint(&CheckpointSpec {
            remote: "origin".to_owned(),
            message: commit.message.clone(),
            author_name: Some(commit.author_name.clone()),
            author_email: Some(commit.author_email.clone()),
            branch: Some(MAIN.to_owned()),
            fallback: None,
            files: Some(vec![file]),
            record: None,
            exclude: None,
            max_bytes: None,
        })
        .map_err(|error| plain(format!("nothing was saved: {error}")))?;
    Ok((!saved.commit.is_empty()).then_some(saved.commit))
}

fn plain(reason: String) -> Refusal {
    Refusal::of(reason)
}
