//! How an Operation's Source items become work: reported to a watch, or queued as briefs.
//!
//! By default a polled item is filed as a Report and its Operation's watch decides what it
//! becomes (a Mission, a duplicate, nothing, or later); with no watch Agent a person decides.
//! `direct` keeps the older path, where each item is queued as a brief by itself.
//!
//! ```toml
//! [operation.G]
//! intake = "direct"
//! ```

use serde::{Deserialize, Serialize};

/// Who decides what a Source's item becomes (Operation key `intake`).
#[non_exhaustive]
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    guatiao::ToValue,
    guatiao::FromValue,
    guatiao::Schema,
)]
#[serde(rename_all = "lowercase")]
#[map(rename_all = "lowercase")]
pub enum SourceIntake {
    /// The item is a Report. The Operation's watch decides what becomes a Mission, or a person
    /// does when the Operation has no watch Agent; nothing becomes one by itself.
    #[default]
    #[schema(label = "Report to the watch, which decides")]
    Watch,
    /// The item is queued as a brief at once, with no one deciding.
    #[schema(label = "Queue each item as a brief")]
    Direct,
}

impl SourceIntake {
    /// `watch` or `direct`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Watch => "watch",
            Self::Direct => "direct",
        }
    }

    /// What `direct` changes from the default, worded for a log line and the check-in.
    pub fn non_default(self) -> Option<&'static str> {
        match self {
            Self::Watch => None,
            Self::Direct => Some(
                "intake = \"direct\" (default \"watch\"): each Source item is queued as a brief \
                 with no watch or person deciding it first",
            ),
        }
    }
}
