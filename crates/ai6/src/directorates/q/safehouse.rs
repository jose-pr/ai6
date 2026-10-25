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
