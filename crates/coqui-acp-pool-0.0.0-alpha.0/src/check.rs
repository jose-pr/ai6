//! The checks of a pool's name and members, for a host that reads pools or roles from its configuration.

use alloc::string::{String, ToString};
use core::fmt;

use coqui_acp_api::standard::is_standard;

use crate::accounts::automatic_standard;
use crate::settings::Settings;

/// The host's names, so a pool's own can be checked against them.
pub trait Known {
    /// Whether `name` is already something else of the host's that a pool may not share a name with (an account, the automatic
    /// `local`).
    fn taken(&self, name: &str) -> bool;

    /// Whether `name` is a member the pool may hold (an account, the automatic `local`).
    fn member(&self, name: &str) -> bool;
}

/// What is wrong with a pool's definition. Each variant names the thing, never a value the host holds secret.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PoolIssue {
    /// The name is empty, too long (`max` characters) or holds a control character.
    InvalidLabel {
        /// The longest a name may be.
        max: usize,
    },
    /// The name is another thing's.
    NameTaken(String),
    /// The name is the host's own pool of a standard.
    AutomaticPool(String),
    /// The standard is not a standard's id.
    InvalidStandard,
    /// A member the host does not know.
    UnknownMember(String),
    /// A member named twice.
    DuplicateMember(String),
    /// More members than [`Settings::max_members`].
    TooManyMembers {
        /// The most a pool may hold.
        max: usize,
    },
}

impl fmt::Display for PoolIssue {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLabel { max } => write!(
                out,
                "a name has 1 to {max} characters, none of them control characters"
            ),
            Self::NameTaken(name) => write!(out, "the name {name} is taken by something else"),
            Self::AutomaticPool(name) => write!(
                out,
                "the name {name} is taken by the pool the host makes of every account that provides that standard"
            ),
            Self::InvalidStandard => out.write_str(
                "standard is a credential standard's id: lower-case letters, digits and single hyphens",
            ),
            Self::UnknownMember(name) => write!(out, "{name} is not a known member"),
            Self::DuplicateMember(name) => write!(out, "{name} is named twice"),
            Self::TooManyMembers { max } => write!(out, "a pool holds at most {max} members"),
        }
    }
}

impl core::error::Error for PoolIssue {}

/// Checks a pool's `name`, its `standard` (when it has one: a role has none), and its `members` against what the host `known`s, in
/// this order: the name's shape, whether another thing has it, whether it is a host pool's, the standard, the member count, and each
/// member's existence and uniqueness.
///
/// # Errors
///
/// The first thing wrong.
pub fn check_pool<S: AsRef<str>>(
    name: &str,
    standard: Option<&str>,
    members: &[S],
    known: &dyn Known,
    settings: &Settings,
) -> Result<(), PoolIssue> {
    if name.is_empty()
        || name.chars().count() > settings.max_name_chars
        || name.chars().any(char::is_control)
    {
        return Err(PoolIssue::InvalidLabel {
            max: settings.max_name_chars,
        });
    }
    if known.taken(name) {
        return Err(PoolIssue::NameTaken(name.to_string()));
    }
    if let Some(standard) = standard {
        if automatic_standard(name).is_some() {
            return Err(PoolIssue::AutomaticPool(name.to_string()));
        }
        if !is_standard(standard) {
            return Err(PoolIssue::InvalidStandard);
        }
    }
    if members.len() > settings.max_members {
        return Err(PoolIssue::TooManyMembers {
            max: settings.max_members,
        });
    }
    for (at, member) in members.iter().map(AsRef::as_ref).enumerate() {
        if !known.member(member) {
            return Err(PoolIssue::UnknownMember(member.to_string()));
        }
        if members[..at].iter().any(|each| each.as_ref() == member) {
            return Err(PoolIssue::DuplicateMember(member.to_string()));
        }
    }
    Ok(())
}
