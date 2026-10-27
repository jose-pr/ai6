// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The safe-house contract, as Guatiao kinds: where a Mission's Agent works.
//!
//! A [`SafeHouseProvider`] turns an Operation's source (a Git checkout, a plain
//! directory, ...) into an isolated working directory for one Mission and hands back a
//! [`SafeHouseSession`] the caller owns. The provider runs where the Agent's Session
//! runs, so a Station loads it; the headquarters never does. Slot order is the ABI:
//! append, never reorder.
//!
//! A provider without history ([`SafeHouseInfo::supports_history`] is `false`) returns
//! no [`Change`]s; cover checks then report that the work cannot be checked instead of
//! passing it.

use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, ToValue, Value};

/// What to open. Crosses as a value.
#[derive(
    ToValue, FromValue, Schema, serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq,
)]
#[map(crate = ::guatiao)]
pub struct SafeHouseSpec {
    /// The Operation codename.
    pub operation_id: String,
    /// The Mission ID.
    pub mission_id: String,
    /// The Mission brief, used to name the branch or directory.
    pub title: String,
    /// Where the work comes from: a local path today.
    pub source: String,
    /// The revision the safe house starts from; a provider without revisions ignores it.
    pub base: String,
    /// Branch name template; `{id}`, `{slug}` and `{operation}` are filled in.
    pub branch: String,
    /// Whether to cut a separate working directory. `false` works in `source` itself.
    pub isolate: bool,
    /// AI6's state directory; isolated safe houses are placed beneath it.
    pub state_root: String,
    /// A report from an earlier open: re-attach to that safe house instead of creating one.
    pub reopen: Option<SafeHouseReport>,
    /// Work an earlier attempt saved elsewhere: start the new safe house from its commit, on
    /// its branch, instead of from `base`. A provider without checkpoints ignores it.
    pub checkpoint: Option<SafeHouseCheckpoint>,
    /// Make the working directory a clone of `source` (a repository URL, `user@host:path`,
    /// or a local repository, bare or not) on its branch `base`, instead of a Mission's working
    /// directory beneath `state_root`. `branch`, `isolate`, `reopen` and `checkpoint` are then
    /// ignored; a provider that cannot clone refuses it.
    pub clone: Option<SafeHouseClone>,
}

/// Where a cloned safe house goes. Crosses as a value, inside a [`SafeHouseSpec`].
#[derive(
    ToValue, FromValue, Schema, serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq,
)]
#[map(crate = ::guatiao)]
pub struct SafeHouseClone {
    /// The working directory: a directory that must not exist yet.
    pub path: String,
    /// First make `source` an empty bare repository whose `HEAD` is the spec's `base` when it
    /// is a local path or `file://` URL that does not exist.
    pub create_source: bool,
    /// Keep the repository's git directory here, a directory that must not exist yet, instead
    /// of `.git` inside `path`: the working directory then holds no `.git` at all, so whoever
    /// writes there cannot reach the repository's configuration or hooks.
    pub git_dir: Option<String>,
}

/// Work in progress saved where another Station can fetch it: a commit pushed to a branch.
#[derive(
    ToValue, FromValue, Schema, serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq,
)]
#[map(crate = ::guatiao)]
pub struct SafeHouseCheckpoint {
    /// The Git remote, by name or URL, the commit was pushed to.
    pub remote: String,
    /// The branch on the remote that holds the commit.
    pub branch: String,
    /// The commit: the safe house's history plus its uncommitted changes.
    pub commit: String,
    /// The revision the work started from, when the provider has revisions.
    pub base: Option<String>,
}

/// How to save a checkpoint. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct CheckpointSpec {
    /// The Git remote, by name or URL, to push to.
    pub remote: String,
    /// The message of the commit that holds uncommitted changes, already under the
    /// Operation's cover policy.
    pub message: String,
    /// The author and committer name, when the Operation commits under a cover identity.
    pub author_name: Option<String>,
    /// The author and committer email, when the Operation commits under a cover identity.
    pub author_email: Option<String>,
    /// Commit the work in the working directory itself (`HEAD` moves to the commit) and push
    /// it to this branch of `remote` as a fast-forward, instead of force-pushing a commit on
    /// top of `HEAD` to the safe house's own branch. A push the remote refuses is tried once
    /// more after the work is replayed on the remote branch. The answer's `branch` is where the
    /// work landed, and its `commit` is empty when there was nothing to save.
    pub branch: Option<String>,
    /// With `branch`, where the work goes when the remote branch still refuses it: this
    /// branch of `remote`, which each such push replaces. Without it the refusal is an error.
    pub fallback: Option<String>,
    /// With `branch`, commit exactly these files with exactly this text, and nothing else the
    /// working directory holds, whoever is writing there meanwhile.
    pub files: Option<Vec<CheckpointFile>>,
    /// With `branch` and no `files`: a directory, relative to the repository's root, in which
    /// the save records, in the same commit, each file it adds or changes. One entry per file,
    /// holding its path and a newline and named by the Git object ID of that text. The paths
    /// come from what is committed, so none is missed; a save that cannot name them fails.
    pub record: Option<String>,
    /// With `branch` and no `files`: paths, relative to the repository's root, that the save
    /// leaves out with everything under them, as it leaves out paths with a name starting
    /// with `.`.
    pub exclude: Option<Vec<String>>,
}

/// One file a save commits with exactly its text. Crosses as a value, inside a
/// [`CheckpointSpec`].
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct CheckpointFile {
    /// The path, `/`-separated and relative to the repository's root; a path naming a parent
    /// directory or `.git` is refused.
    pub path: String,
    /// The file's text; `None` removes it.
    pub text: Option<String>,
    /// What the file must hold on the remote branch for the save to go ahead: the hex
    /// SHA-256 of its text, or empty when it must not exist there. `None` writes it whatever
    /// it holds.
    pub base: Option<String>,
}

/// Where an open safe house is and what it can tell.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct SafeHouseInfo {
    /// The working directory.
    pub path: String,
    /// The branch assigned to the Mission, when the provider has branches.
    pub branch: Option<String>,
    /// The revision the work started from, when the provider has revisions.
    pub base: Option<String>,
    /// Whether [`SafeHouseSession::changes`] can answer; `false` means cover is not checkable.
    pub supports_history: bool,
    /// Whether `path` is a separate directory made for this Mission.
    pub isolated: bool,
}

/// One change made since the base: a commit, for a version-control provider.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct Change {
    /// The full change identifier.
    pub id: String,
    /// Author in `Name <email>` form.
    pub author: String,
    /// Committer in `Name <email>` form.
    pub committer: String,
    /// The full message.
    pub message: String,
}

/// The changes since the base, newest first.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Default)]
#[map(crate = ::guatiao)]
pub struct ChangeLog {
    /// The changes.
    pub changes: Vec<Change>,
}

/// What a safe house held when the Mission ended, attached to the Mission record.
#[derive(
    ToValue, FromValue, Schema, serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq,
)]
#[map(crate = ::guatiao)]
pub struct SafeHouseReport {
    /// The working directory.
    pub path: String,
    /// The branch assigned to the Mission, when the provider has branches.
    pub branch: Option<String>,
    /// The revision the work started from, when the provider has revisions.
    pub base: Option<String>,
    /// Whether the safe house was retained because it holds work.
    pub kept: bool,
    /// One-line summaries of the changes since the base, newest first.
    pub changes: Vec<String>,
}

/// What of a safe house's work exists nowhere else.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Default)]
#[map(crate = ::guatiao)]
pub struct SafeHouseExports {
    /// One-line summaries of the changes made since the base that no remote holds, newest
    /// first: for a version-control provider, commits on no remote-tracking ref.
    pub unexported: Vec<String>,
    /// Whether the working directory holds changes that were never recorded as a change,
    /// including untracked files.
    pub dirty: bool,
    /// Paths, relative to the safe house, that version control ignores: a directory ignored
    /// whole is one entry. They exist nowhere else; a caller decides which are rebuildable.
    pub ignored: Vec<String>,
}

impl SafeHouseExports {
    /// Whether everything in the safe house also exists elsewhere, counting an ignored path
    /// as rebuildable only when `rebuildable` says so.
    pub fn is_exported(&self, rebuildable: impl Fn(&str) -> bool) -> bool {
        self.unexported.is_empty()
            && !self.dirty
            && self.ignored.iter().all(|path| rebuildable(path))
    }
}

/// Makes safe houses for Missions.
#[guatiao::kind(name = "ai6_safehouse")]
pub trait SafeHouseProvider: Send + Sync {
    /// Opens the safe house described by `spec` (a [`SafeHouseSpec`] as a value).
    fn open(&self, spec: &Value) -> Result<Object<dyn SafeHouseSession>, ProviderError>;
}

/// One Mission's safe house. Every slot takes `&self`.
#[guatiao::kind(object, name = "ai6_safehouse_session")]
pub trait SafeHouseSession: Send + Sync {
    /// Answers a [`SafeHouseInfo`] as a value.
    fn info(&self) -> Result<Value, ProviderError>;
    /// Answers a [`ChangeLog`] as a value: the changes since the base, newest first.
    /// Empty when the provider has no history.
    fn changes(&self) -> Result<Value, ProviderError>;
    /// Removes an untouched isolated safe house and retains one that holds work; answers
    /// a [`SafeHouseReport`] as a value. Idempotent; a safe house in place is left alone.
    fn finish(&self) -> Result<Value, ProviderError>;
    /// Saves the work so far where another Station can fetch it, as `spec` (a
    /// [`CheckpointSpec`] as a value) says, without touching the working directory, and
    /// answers a [`SafeHouseCheckpoint`] as a value. Unsupported unless the provider has it.
    fn checkpoint(&self, spec: &Value) -> Result<Value, ProviderError> {
        let _ = spec;
        Err(ProviderError::new(
            guatiao::Status::GUATIAO_ERR_UNSUPPORTED,
            "this safe-house provider does not save checkpoints",
        ))
    }
    /// Answers a [`SafeHouseExports`] as a value: what of the work exists only here.
    /// Unsupported unless the provider can tell; a caller then treats all of it as
    /// unexported.
    fn exports(&self) -> Result<Value, ProviderError> {
        Err(ProviderError::new(
            guatiao::Status::GUATIAO_ERR_UNSUPPORTED,
            "this safe-house provider cannot tell what of its work exists elsewhere",
        ))
    }
    /// Removes an isolated safe house whatever it holds: its working directory and, for a
    /// version-control provider, the Mission's branch. A safe house in place is refused.
    /// Unsupported unless the provider has it.
    fn discard(&self) -> Result<(), ProviderError> {
        Err(ProviderError::new(
            guatiao::Status::GUATIAO_ERR_UNSUPPORTED,
            "this safe-house provider cannot remove a safe house that holds work",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A changed required signature affects every built safe-house provider.
    #[test]
    fn floor_hashes_are_pinned() {
        assert_eq!(SafeHouseProviderVtable::FLOOR_HASH, 3612232285);
        assert_eq!(SafeHouseSessionVtable::FLOOR_HASH, 3614149196);
    }

    fn spec() -> SafeHouseSpec {
        SafeHouseSpec {
            operation_id: "G".to_owned(),
            mission_id: "mission-1".to_owned(),
            title: "Inspect".to_owned(),
            source: "/work/project".to_owned(),
            base: "HEAD".to_owned(),
            branch: "{id}-{slug}".to_owned(),
            isolate: true,
            state_root: "/state".to_owned(),
            reopen: Some(SafeHouseReport {
                path: "/state/safe-houses/G/mission-1".to_owned(),
                branch: Some("mission-1-inspect".to_owned()),
                base: Some("abc".to_owned()),
                kept: true,
                changes: vec!["abc1234 add work".to_owned()],
            }),
            checkpoint: None,
            clone: None,
        }
    }

    // A model grows by optional fields only: these keys are what built providers read.
    #[test]
    fn models_keep_their_required_keys() {
        let json = serde_json::to_value(spec()).unwrap();
        let keys = json.as_object().unwrap().keys().collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "base",
                "branch",
                "checkpoint",
                "clone",
                "isolate",
                "mission_id",
                "operation_id",
                "reopen",
                "source",
                "state_root",
                "title"
            ]
        );
    }

    #[test]
    fn a_spec_round_trips_through_a_value() {
        let spec = spec();
        let value = spec.to_value().unwrap();
        assert_eq!(SafeHouseSpec::from_value(&value).unwrap(), spec);
    }
}
