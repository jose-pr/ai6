//! What the host says of one member now, and the checks that decide whether it can take a turn.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use coqui_acp_api::{Bucket, Coverage, Refill, Status, UsageReport};

use crate::pool::Member;
use crate::settings::Settings;

/// What a usage source answers for one member.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
#[allow(
    clippy::large_enum_variant,
    reason = "one value per member per decision; a box would only make hosts wrap every report"
)]
pub enum Usage {
    /// The member's provider reports its allowances.
    Known(UsageReport),
    /// It reports none, or could not be asked: the member stays usable.
    #[default]
    Unknown,
    /// A report was asked for and has not come yet (another station holds the account). The member stays usable for now; ask again
    /// later. The library never waits.
    Pending,
}

/// Something that holds a member back, such as a spending limit's stop. The host decides its scope (account, profile, operation).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Hold {
    /// What a person reads.
    pub reason: String,
    /// When it lets go, in Unix seconds; `None` when nothing says.
    pub until: Option<i64>,
}

impl Hold {
    /// A hold with no end.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            until: None,
        }
    }

    /// The same hold ending at `until`.
    #[must_use]
    pub const fn until(mut self, until: i64) -> Self {
        self.until = Some(until);
        self
    }
}

/// What the host says of one member now. The default is a member that is available with no usage known, no holds, no runs and no
/// last use.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Standing {
    /// Why the member cannot serve whatever it has left: the program is missing, the account is not signed in or is held by another
    /// connection, the model is not listed, the entry is not offered to this connection.
    pub unavailable: Option<String>,
    /// What its provider reports of its allowances.
    pub usage: Usage,
    /// Until when a turn that a limit refused holds it back, in Unix seconds; `i64::MAX` when the refusal said no time.
    pub limited_until: Option<i64>,
    /// What else holds it back.
    pub holds: Vec<Hold>,
    /// The runs on it now, not counting the asking session's own.
    pub active: u32,
    /// When it last took a turn, in Unix seconds.
    pub last_used: Option<i64>,
    /// Whether a running session's history can move onto it. Only a move asks; a new session ignores it.
    pub carries: bool,
}

impl Standing {
    /// An available member: nothing known against it. A running session's history can move onto it.
    #[must_use]
    pub fn new() -> Self {
        Self {
            carries: true,
            ..Self::default()
        }
    }

    /// Marks it unavailable for `reason`.
    #[must_use]
    pub fn unavailable(mut self, reason: impl Into<String>) -> Self {
        self.unavailable = Some(reason.into());
        self
    }

    /// Sets what its provider reports.
    #[must_use]
    pub fn usage(mut self, usage: Usage) -> Self {
        self.usage = usage;
        self
    }

    /// Sets the end of a refused turn's hold.
    #[must_use]
    pub const fn limited_until(mut self, until: i64) -> Self {
        self.limited_until = Some(until);
        self
    }

    /// Adds a hold.
    #[must_use]
    pub fn hold(mut self, hold: Hold) -> Self {
        self.holds.push(hold);
        self
    }

    /// Sets the runs on it.
    #[must_use]
    pub const fn active(mut self, runs: u32) -> Self {
        self.active = runs;
        self
    }

    /// Sets when it last took a turn.
    #[must_use]
    pub const fn last_used(mut self, at: i64) -> Self {
        self.last_used = Some(at);
        self
    }

    /// Sets whether a running session's history can move onto it.
    #[must_use]
    pub const fn carries(mut self, carries: bool) -> Self {
        self.carries = carries;
        self
    }
}

/// Why a member cannot take the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reason {
    /// The host said so: [`Standing::unavailable`].
    Unavailable(String),
    /// It is at its cap of runs.
    AtCapacity {
        /// The runs on it.
        active: u32,
        /// Its cap.
        max: u32,
    },
    /// A turn on it was refused for a limit that has not lifted.
    Refused {
        /// When it lifts; `None` when nothing says.
        until: Option<i64>,
    },
    /// A bucket covering the model is used up, or at the member's stop point, and its window has not ended.
    UsedUp {
        /// The bucket's label or id.
        bucket: String,
        /// Its use and the stop point, in thousandths, when it is held for its figures and not its status.
        level: Option<(u16, u16)>,
        /// When the window ends.
        until: Option<i64>,
    },
    /// A [`Hold`].
    Held {
        /// The hold's reason.
        reason: String,
        /// When it lets go.
        until: Option<i64>,
    },
}

impl Reason {
    /// The words a person reads.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Unavailable(why) => why.clone(),
            Self::AtCapacity { active, max } => format!("{active} of {max} runs are in use"),
            Self::Refused { .. } => "a turn on it was refused for a limit".to_string(),
            Self::UsedUp {
                bucket,
                level: Some((used, stop)),
                ..
            } if *used < 1000 => {
                format!("{bucket} is at {used} permille, its stop point is {stop}")
            }
            Self::UsedUp { bucket, .. } => format!("{bucket} is used up"),
            Self::Held { reason, .. } => reason.clone(),
        }
    }

    /// When it lets go, in Unix seconds.
    #[must_use]
    pub const fn until(&self) -> Option<i64> {
        match self {
            Self::Refused { until } | Self::UsedUp { until, .. } | Self::Held { until, .. } => {
                *until
            }
            Self::Unavailable(_) | Self::AtCapacity { .. } => None,
        }
    }

    /// Whether an allowance holds it back, as against the member being unavailable or full: the kinds a refill ends.
    #[must_use]
    pub const fn is_allowance(&self) -> bool {
        !matches!(self, Self::Unavailable(_) | Self::AtCapacity { .. })
    }
}

/// A member that was passed over and why.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Skipped {
    /// The member.
    pub member: String,
    /// Why.
    pub reason: Reason,
}

/// How a member's usage reads for a turn on a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reading {
    /// The use of the most used bucket that covers the model, in thousandths; `None` when no bucket does.
    pub(crate) permille: Option<u16>,
    pub(crate) pending: bool,
}

fn covers(bucket: &Bucket, model: Option<&str>) -> bool {
    match &bucket.models {
        Coverage::Only { models } => model.is_some_and(|asked| models.iter().any(|m| m == asked)),
        _ => true,
    }
}

fn ends(bucket: &Bucket) -> Option<i64> {
    match &bucket.refill {
        Refill::FixedWindow {
            resets_at: Some(at),
            ..
        } => Some(*at),
        _ => None,
    }
}

/// The buckets of `usage` that count for a turn on `model` at `now`: they cover the model, apply, and have not refilled.
fn counting<'a>(
    usage: &'a Usage,
    model: Option<&'a str>,
    now: i64,
) -> impl Iterator<Item = &'a Bucket> {
    let buckets: &[Bucket] = match usage {
        Usage::Known(report) => &report.buckets,
        _ => &[],
    };
    buckets
        .iter()
        .filter(move |bucket| bucket.status != Status::Disabled && covers(bucket, model))
        .filter(move |bucket| ends(bucket).is_none_or(|at| at > now))
}

fn used_of(bucket: &Bucket) -> Option<u16> {
    if bucket.status == Status::Exhausted {
        return Some(1000);
    }
    UsageReport::fraction_permille(bucket)
}

pub(crate) fn reading(standing: &Standing, model: Option<&str>, now: i64) -> Reading {
    Reading {
        permille: counting(&standing.usage, model, now)
            .filter_map(used_of)
            .max(),
        pending: standing.usage == Usage::Pending,
    }
}

/// The first check a member fails, in the order the library defines: unavailable, at its cap, a refused turn, a bucket, a hold.
pub(crate) fn judge<T>(
    member: &Member<T>,
    standing: &Standing,
    settings: &Settings,
    model: Option<&str>,
    now: i64,
) -> Option<Reason> {
    if let Some(why) = &standing.unavailable {
        return Some(Reason::Unavailable(why.clone()));
    }
    if let Some(max) = member.max_concurrent.or(settings.max_concurrent)
        && standing.active >= max
    {
        return Some(Reason::AtCapacity {
            active: standing.active,
            max,
        });
    }
    if let Some(until) = standing.limited_until
        && until > now
    {
        return Some(Reason::Refused {
            until: (until != i64::MAX).then_some(until),
        });
    }
    let stop = member.stop_permille.unwrap_or(settings.stop_permille);
    let bucket = counting(&standing.usage, model, now)
        .filter_map(|bucket| {
            let used = used_of(bucket)?;
            let by_status = bucket.status == Status::Exhausted;
            (by_status || used >= stop).then(|| Reason::UsedUp {
                bucket: bucket.label.clone().unwrap_or_else(|| bucket.id.clone()),
                level: (!by_status).then_some((used, stop)),
                until: ends(bucket),
            })
        })
        .min_by_key(|reason| reason.until().unwrap_or(i64::MAX));
    if bucket.is_some() {
        return bucket;
    }
    standing
        .holds
        .iter()
        .filter(|hold| hold.until.is_none_or(|until| until > now))
        .map(|hold| Reason::Held {
            reason: hold.reason.clone(),
            until: hold.until,
        })
        .min_by_key(|reason| reason.until().unwrap_or(i64::MAX))
}

impl Default for Standing {
    fn default() -> Self {
        Self {
            unavailable: None,
            usage: Usage::Unknown,
            limited_until: None,
            holds: Vec::new(),
            active: 0,
            last_used: None,
            carries: true,
        }
    }
}
