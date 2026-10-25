//! Token and cost accounting for Agent Sessions, and the budgets checked against it.
//!
//! Handlers report what the provider reports ([`TokenUsage`]); a [`UsageTracker`] turns
//! that into additions to a ledger, so a resumed Session is never counted twice. Measuring
//! is always on. A [`Budget`] limit exists only when the operator sets one or chooses the
//! `strict` preset.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::directorates::section_00::agent::TokenUsage;

/// Thousandths of a limit at which a warning is raised unless configured otherwise (80%).
pub const DEFAULT_WARN_AT_PERMILLE: u16 = 800;

/// Per-Session token limit of the `strict` preset.
pub const STRICT_SESSION_TOKENS: u64 = 2_000_000;
/// Per-Mission token limit of the `strict` preset.
pub const STRICT_MISSION_TOKENS: u64 = 5_000_000;
/// Per-Mission cost limit of the `strict` preset, in whole dollars.
pub const STRICT_MISSION_COST_USD: f64 = 10.0;
/// Per-Operation daily cost limit of the `strict` preset, in whole dollars.
pub const STRICT_OPERATION_COST_USD_PER_DAY: f64 = 50.0;

/// Accumulated usage: the sum of everything a Session, Mission or day consumed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageTotals {
    /// Prompt tokens that were not served from the cache.
    pub input: u64,
    /// Tokens the model generated.
    pub output: u64,
    /// Prompt tokens read from the provider's cache.
    pub cache_read: u64,
    /// Prompt tokens written to the provider's cache.
    pub cache_write: u64,
    /// Cost in millionths of a dollar.
    pub cost_micro_usd: u64,
}

impl UsageTotals {
    /// Adds `other` to these totals, saturating.
    pub fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.cost_micro_usd = self.cost_micro_usd.saturating_add(other.cost_micro_usd);
    }

    /// The tokens a token budget counts: input, output and cache writes.
    ///
    /// Cache reads re-count the same context on every turn, so they would exhaust a budget
    /// without the Agent doing more work; they stay visible in [`UsageTotals::cache_read`].
    pub fn tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_write)
    }

    /// Whether nothing was consumed.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn from_usage(usage: &TokenUsage) -> Self {
        Self {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cost_micro_usd: usage.cost_micro_usd,
        }
    }

    fn fits_after(&self, earlier: &Self) -> bool {
        self.input >= earlier.input
            && self.output >= earlier.output
            && self.cache_read >= earlier.cache_read
            && self.cache_write >= earlier.cache_write
            && self.cost_micro_usd >= earlier.cost_micro_usd
    }

    fn since(&self, earlier: &Self) -> Self {
        Self {
            input: self.input - earlier.input,
            output: self.output - earlier.output,
            cache_read: self.cache_read - earlier.cache_read,
            cache_write: self.cache_write - earlier.cache_write,
            cost_micro_usd: self.cost_micro_usd - earlier.cost_micro_usd,
        }
    }
}

/// Turns a provider's usage reports for one attached provider process into additions.
///
/// Make one per start or resume: a resumed process starts counting from zero, so its
/// totals must not be compared with the previous process's.
#[derive(Debug, Default)]
pub struct UsageTracker {
    last: UsageTotals,
}

impl UsageTracker {
    /// What `usage` adds to the ledger. A cumulative report that went down is treated as
    /// a new counter and counts in full, so spend is never lost.
    pub fn observe(&mut self, usage: &TokenUsage) -> UsageTotals {
        let reported = UsageTotals::from_usage(usage);
        if !usage.cumulative {
            return reported;
        }
        let delta = if reported.fits_after(&self.last) {
            reported.since(&self.last)
        } else {
            reported
        };
        self.last = reported;
        delta
    }
}

/// What a limit applies to.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    /// Tokens one Session may use.
    SessionTokens,
    /// Tokens all of a Mission's Sessions may use.
    MissionTokens,
    /// Dollars all of a Mission's Sessions may spend.
    MissionCost,
    /// Dollars an Operation's Missions may spend in one UTC day.
    OperationCost,
}

impl BudgetScope {
    /// The configuration key of the limit.
    pub fn key(self) -> &'static str {
        match self {
            Self::SessionTokens => "session_tokens",
            Self::MissionTokens => "mission_tokens",
            Self::MissionCost => "mission_cost_usd",
            Self::OperationCost => "operation_cost_usd_per_day",
        }
    }

    fn is_cost(self) -> bool {
        matches!(self, Self::MissionCost | Self::OperationCost)
    }
}

/// How far a limit has been used.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetLevel {
    /// The `warn_at` fraction was reached.
    Warn,
    /// The limit was reached.
    Limit,
}

/// One limit that has reached its warning or its ceiling.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BudgetBreach {
    /// Which limit.
    pub scope: BudgetScope,
    /// Warning or ceiling.
    pub level: BudgetLevel,
    /// What has been used, in tokens or micro-dollars.
    pub used: u64,
    /// The limit, in the same unit.
    pub limit: u64,
}

impl fmt::Display for BudgetBreach {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.level {
            BudgetLevel::Warn => "is near",
            BudgetLevel::Limit => "reached",
        };
        if self.scope.is_cost() {
            write!(
                f,
                "{} {what} its limit: ${:.2} of ${:.2}",
                self.scope.key(),
                self.used as f64 / 1e6,
                self.limit as f64 / 1e6
            )
        } else {
            write!(
                f,
                "{} {what} its limit: {} of {} tokens",
                self.scope.key(),
                self.used,
                self.limit
            )
        }
    }
}

/// Named sets of limits.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetPreset {
    /// Only measures; sets no limit. The default.
    Observe,
    /// Conservative ceilings on every scope.
    Strict,
}

/// Spending limits. Every field is optional and an unset limit is not enforced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Budget {
    /// Tokens one Session may use.
    pub session_tokens: Option<u64>,
    /// Tokens all of a Mission's Sessions may use.
    pub mission_tokens: Option<u64>,
    /// Micro-dollars all of a Mission's Sessions may spend.
    pub mission_cost_micro_usd: Option<u64>,
    /// Micro-dollars an Operation's Missions may spend in one UTC day.
    pub operation_cost_micro_usd_per_day: Option<u64>,
    /// Thousandths of a limit at which a warning is raised; [`DEFAULT_WARN_AT_PERMILLE`] when
    /// unset.
    pub warn_at_permille: Option<u16>,
}

impl Budget {
    /// The limits of a named preset.
    pub fn preset(preset: BudgetPreset) -> Self {
        match preset {
            BudgetPreset::Observe => Self::default(),
            BudgetPreset::Strict => Self {
                session_tokens: Some(STRICT_SESSION_TOKENS),
                mission_tokens: Some(STRICT_MISSION_TOKENS),
                mission_cost_micro_usd: Some(micro_usd(STRICT_MISSION_COST_USD)),
                operation_cost_micro_usd_per_day: Some(micro_usd(
                    STRICT_OPERATION_COST_USD_PER_DAY,
                )),
                warn_at_permille: None,
            },
        }
    }

    /// Whether any limit is set.
    pub fn is_limited(&self) -> bool {
        self.session_tokens.is_some()
            || self.mission_tokens.is_some()
            || self.mission_cost_micro_usd.is_some()
            || self.operation_cost_micro_usd_per_day.is_some()
    }

    /// This budget with every field `child` sets taking precedence.
    pub fn merge(&self, child: &Self) -> Self {
        Self {
            session_tokens: child.session_tokens.or(self.session_tokens),
            mission_tokens: child.mission_tokens.or(self.mission_tokens),
            mission_cost_micro_usd: child.mission_cost_micro_usd.or(self.mission_cost_micro_usd),
            operation_cost_micro_usd_per_day: child
                .operation_cost_micro_usd_per_day
                .or(self.operation_cost_micro_usd_per_day),
            warn_at_permille: child.warn_at_permille.or(self.warn_at_permille),
        }
    }

    /// Every limit that has reached its warning or its ceiling.
    ///
    /// `operation_day_cost` is the Operation's spend so far today (UTC); pass `None` when
    /// it was not computed. A ceiling is reported in place of the warning for the same limit.
    pub fn check(
        &self,
        session: &UsageTotals,
        mission: &UsageTotals,
        operation_day_cost: Option<u64>,
    ) -> Vec<BudgetBreach> {
        let warn_at =
            f64::from(self.warn_at_permille.unwrap_or(DEFAULT_WARN_AT_PERMILLE)) / 1_000.0;
        [
            (
                BudgetScope::SessionTokens,
                self.session_tokens,
                Some(session.tokens()),
            ),
            (
                BudgetScope::MissionTokens,
                self.mission_tokens,
                Some(mission.tokens()),
            ),
            (
                BudgetScope::MissionCost,
                self.mission_cost_micro_usd,
                Some(mission.cost_micro_usd),
            ),
            (
                BudgetScope::OperationCost,
                self.operation_cost_micro_usd_per_day,
                operation_day_cost,
            ),
        ]
        .into_iter()
        .filter_map(|(scope, limit, used)| {
            let (limit, used) = (limit?, used?);
            let level = if used >= limit {
                BudgetLevel::Limit
            } else if used as f64 >= limit as f64 * warn_at {
                BudgetLevel::Warn
            } else {
                return None;
            };
            Some(BudgetBreach {
                scope,
                level,
                used,
                limit,
            })
        })
        .collect()
    }
}

/// Dollars as micro-dollars, rounded; negative or non-finite values become zero.
pub fn micro_usd(dollars: f64) -> u64 {
    if dollars.is_finite() && dollars > 0.0 {
        (dollars * 1_000_000.0).round() as u64
    } else {
        0
    }
}

/// The limits as written in configuration (`[operation.ID.budget]`, `[agent.ID.budget]`).
///
/// Values stay untyped here so a bad one is refused with the key's name.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetDefinition {
    preset: Option<BudgetPreset>,
    session_tokens: Option<toml::Value>,
    mission_tokens: Option<toml::Value>,
    mission_cost_usd: Option<toml::Value>,
    operation_cost_usd_per_day: Option<toml::Value>,
    warn_at: Option<toml::Value>,
}

impl BudgetDefinition {
    /// Validates the written values and expands the preset under them.
    ///
    /// # Errors
    ///
    /// Returns a message naming the key when a value is not a non-negative number, or when
    /// `warn_at` is not above 0 and at most 1.
    pub fn resolve(&self) -> Result<Budget, String> {
        let mut budget = self.preset.map(Budget::preset).unwrap_or_default();
        let explicit = Budget {
            session_tokens: tokens("session_tokens", self.session_tokens.as_ref())?,
            mission_tokens: tokens("mission_tokens", self.mission_tokens.as_ref())?,
            mission_cost_micro_usd: dollars("mission_cost_usd", self.mission_cost_usd.as_ref())?,
            operation_cost_micro_usd_per_day: dollars(
                "operation_cost_usd_per_day",
                self.operation_cost_usd_per_day.as_ref(),
            )?,
            warn_at_permille: fraction("warn_at", self.warn_at.as_ref())?,
        };
        budget = budget.merge(&explicit);
        Ok(budget)
    }
}

fn number(key: &str, value: &toml::Value) -> Result<f64, String> {
    let number = match value {
        toml::Value::Integer(integer) => *integer as f64,
        toml::Value::Float(float) => *float,
        _ => return Err(format!("budget {key} must be a number")),
    };
    if !number.is_finite() || number < 0.0 {
        return Err(format!("budget {key} must not be negative"));
    }
    Ok(number)
}

fn tokens(key: &str, value: Option<&toml::Value>) -> Result<Option<u64>, String> {
    value
        .map(|value| match value {
            toml::Value::Integer(integer) if *integer >= 0 => Ok(*integer as u64),
            toml::Value::Integer(_) => Err(format!("budget {key} must not be negative")),
            _ => Err(format!("budget {key} must be a whole number of tokens")),
        })
        .transpose()
}

fn dollars(key: &str, value: Option<&toml::Value>) -> Result<Option<u64>, String> {
    value
        .map(|value| number(key, value).map(micro_usd))
        .transpose()
}

fn fraction(key: &str, value: Option<&toml::Value>) -> Result<Option<u16>, String> {
    value
        .map(|value| {
            let fraction = number(key, value)?;
            if fraction == 0.0 || fraction > 1.0 {
                return Err(format!("budget {key} must be above 0 and at most 1"));
            }
            Ok(((fraction * 1_000.0).round() as u16).max(1))
        })
        .transpose()
}

/// The UTC calendar day of a Unix time, as `YYYY-MM-DD`.
pub fn utc_day(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    // Civil-from-days (H. Hinnant), valid for the whole proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A Mission's usage as recorded in the Registry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MissionUsage {
    /// Everything the Mission's Sessions consumed.
    pub total: UsageTotals,
    /// Each Session's share, keyed by AI6 Session ID.
    pub sessions: BTreeMap<String, UsageTotals>,
    /// Cost per UTC day (`YYYY-MM-DD`) in micro-dollars, for the last few days.
    pub daily_cost_micro_usd: BTreeMap<String, u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64, cost: u64, cumulative: bool) -> TokenUsage {
        TokenUsage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
            cost_micro_usd: cost,
            cumulative,
        }
    }

    #[test]
    fn cumulative_reports_become_additions_and_a_restart_counts_in_full() {
        let mut tracker = UsageTracker::default();
        assert_eq!(tracker.observe(&usage(100, 10, 5, true)).tokens(), 110);
        assert_eq!(tracker.observe(&usage(250, 30, 12, true)).tokens(), 170);
        // A resumed provider process counts from zero again.
        let restarted = tracker.observe(&usage(40, 5, 2, true));
        assert_eq!((restarted.input, restarted.cost_micro_usd), (40, 2));
        // Per-step reports are additions already.
        assert_eq!(tracker.observe(&usage(7, 1, 1, false)).tokens(), 8);
    }

    #[test]
    fn a_limit_warns_then_stops_and_an_unset_limit_never_fires() {
        let budget = Budget {
            mission_tokens: Some(1_000),
            ..Budget::default()
        };
        let totals = |input| UsageTotals {
            input,
            ..UsageTotals::default()
        };
        let none = totals(0);
        assert!(budget.check(&none, &totals(799), None).is_empty());
        let warn = budget.check(&none, &totals(800), None);
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].level, BudgetLevel::Warn);
        let stop = budget.check(&none, &totals(1_000), None);
        assert_eq!(stop[0].level, BudgetLevel::Limit);
        assert!(
            Budget::default()
                .check(&totals(u64::MAX), &totals(u64::MAX), Some(u64::MAX))
                .is_empty()
        );
    }

    #[test]
    fn cache_reads_do_not_count_as_budget_tokens() {
        let totals = UsageTotals {
            input: 1,
            output: 2,
            cache_read: 1_000_000,
            cache_write: 4,
            cost_micro_usd: 0,
        };
        assert_eq!(totals.tokens(), 7);
    }

    #[test]
    fn the_strict_preset_sets_limits_and_keys_override_it() {
        let definition: BudgetDefinition =
            toml::from_str("preset = \"strict\"\nmission_cost_usd = 2.5\nwarn_at = 0.5\n").unwrap();
        let budget = definition.resolve().unwrap();
        assert_eq!(budget.mission_cost_micro_usd, Some(2_500_000));
        assert_eq!(budget.session_tokens, Some(STRICT_SESSION_TOKENS));
        assert_eq!(budget.warn_at_permille, Some(500));
        assert!(!Budget::default().is_limited());
    }

    #[test]
    fn bad_values_are_refused_with_the_key_name() {
        for (text, key) in [
            ("session_tokens = -1", "session_tokens"),
            ("mission_tokens = 1.5", "mission_tokens"),
            ("mission_cost_usd = -2.0", "mission_cost_usd"),
            (
                "operation_cost_usd_per_day = \"lots\"",
                "operation_cost_usd_per_day",
            ),
            ("warn_at = 1.5", "warn_at"),
            ("warn_at = 0", "warn_at"),
        ] {
            let definition: BudgetDefinition = toml::from_str(text).unwrap();
            let message = definition.resolve().unwrap_err();
            assert!(message.contains(key), "{text}: {message}");
        }
    }

    #[test]
    fn utc_days_are_calendar_days() {
        assert_eq!(utc_day(0), "1970-01-01");
        assert_eq!(utc_day(951_782_400), "2000-02-29");
        assert_eq!(utc_day(1_790_000_000), "2026-09-21");
        assert_eq!(utc_day(-1), "1969-12-31");
    }
}
