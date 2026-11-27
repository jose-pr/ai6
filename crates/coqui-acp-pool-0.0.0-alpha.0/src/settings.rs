//! The settings of a pool and its strategy. Every bound is a setting with a default and no floor, and a value that is not the
//! default is visible through [`Settings::non_defaults`].

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::str::FromStr;

/// How a pool orders the members that can serve.
///
/// Members that compare equal on every key of a strategy are taken in the pool's listed order, under every strategy: a host
/// that lists its members in an order of its own (sorted by name, say) gets the same pick whatever order its registry holds
/// them in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Strategy {
    /// Highest priority first; equal priorities in the listed order, so with no priorities it is the listed order.
    #[default]
    Priority,
    /// Lowest reported use first (use not reported counts as [`Settings::unknown_permille`]), then fewest active runs, then
    /// least recently used, then priority, then the listed order.
    Headroom,
    /// The member after the last one that took a turn, in the listed order.
    RoundRobin,
    /// The member used longest ago first: one never used, then the oldest `last_used` (the later of
    /// [`Standing::last_used`](crate::Standing::last_used) and what [`Memory`](crate::Memory) recorded), then priority, then
    /// the listed order. It keeps no cursor, so a member that was out for a while and is back goes first, and a host that
    /// reports every station's use through the standing shares it across stations.
    LeastRecent,
}

impl Strategy {
    /// The spelling a configuration uses: `priority`, `headroom`, `round_robin`, `least_recent`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::Headroom => "headroom",
            Self::RoundRobin => "round_robin",
            Self::LeastRecent => "least_recent",
        }
    }
}

impl fmt::Display for Strategy {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(self.name())
    }
}

/// A strategy name no strategy has.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnknownStrategy;

impl fmt::Display for UnknownStrategy {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("a strategy is priority, headroom, round_robin or least_recent")
    }
}

impl core::error::Error for UnknownStrategy {}

impl FromStr for Strategy {
    type Err = UnknownStrategy;

    fn from_str(text: &str) -> Result<Self, UnknownStrategy> {
        match text {
            "priority" => Ok(Self::Priority),
            "headroom" => Ok(Self::Headroom),
            "round_robin" => Ok(Self::RoundRobin),
            "least_recent" => Ok(Self::LeastRecent),
            _ => Err(UnknownStrategy),
        }
    }
}

/// A setting that is not at its default.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NonDefault {
    /// The setting's name.
    pub setting: String,
    /// Its value.
    pub value: String,
    /// The default.
    pub default: String,
}

/// The bounds of a pool. Any value is accepted: nothing here is a floor or a ceiling the library imposes beyond its default.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Settings {
    /// What a member whose use is not reported counts as under [`Strategy::Headroom`], in thousandths. Default 500.
    pub unknown_permille: u16,
    /// The use, in thousandths of a bucket that covers the model, at which a member stops taking turns, unless the member says its
    /// own. Default 1000: only a bucket that is full.
    pub stop_permille: u16,
    /// The runs a member takes at once unless the member says its own. Default none.
    pub max_concurrent: Option<u32>,
    /// The longest a pool or member name may be, in characters. Default 64.
    pub max_name_chars: usize,
    /// The most members a pool may hold. Default 256.
    pub max_members: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            unknown_permille: 500,
            stop_permille: 1000,
            max_concurrent: None,
            max_name_chars: 64,
            max_members: 256,
        }
    }
}

impl Settings {
    /// The defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets [`Settings::unknown_permille`].
    #[must_use]
    pub const fn unknown_permille(mut self, permille: u16) -> Self {
        self.unknown_permille = permille;
        self
    }

    /// Sets [`Settings::stop_permille`].
    #[must_use]
    pub const fn stop_permille(mut self, permille: u16) -> Self {
        self.stop_permille = permille;
        self
    }

    /// Sets [`Settings::max_concurrent`].
    #[must_use]
    pub const fn max_concurrent(mut self, runs: Option<u32>) -> Self {
        self.max_concurrent = runs;
        self
    }

    /// Sets [`Settings::max_name_chars`].
    #[must_use]
    pub const fn max_name_chars(mut self, chars: usize) -> Self {
        self.max_name_chars = chars;
        self
    }

    /// Sets [`Settings::max_members`].
    #[must_use]
    pub const fn max_members(mut self, members: usize) -> Self {
        self.max_members = members;
        self
    }

    /// Every setting that is not its default, in declaration order, so a changed bound is never silent.
    #[must_use]
    pub fn non_defaults(&self) -> Vec<NonDefault> {
        let base = Self::default();
        let mut found = Vec::new();
        let mut note = |setting: &str, value: String, default: String| {
            if value != default {
                found.push(NonDefault {
                    setting: setting.to_string(),
                    value,
                    default,
                });
            }
        };
        note(
            "unknown_permille",
            self.unknown_permille.to_string(),
            base.unknown_permille.to_string(),
        );
        note(
            "stop_permille",
            self.stop_permille.to_string(),
            base.stop_permille.to_string(),
        );
        note(
            "max_concurrent",
            format!("{:?}", self.max_concurrent),
            format!("{:?}", base.max_concurrent),
        );
        note(
            "max_name_chars",
            self.max_name_chars.to_string(),
            base.max_name_chars.to_string(),
        );
        note(
            "max_members",
            self.max_members.to_string(),
            base.max_members.to_string(),
        );
        found
    }
}
