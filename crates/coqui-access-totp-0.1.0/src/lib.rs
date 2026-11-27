//! One-time codes (TOTP, RFC 6238) as a second factor, kept in any [`Table`].
//!
//! An account is enrolled through [`Accounts::put`]: the first call makes a secret and ten
//! one-time recovery codes and answers an `otpauth://` link and the codes once; the second,
//! with a code from the authenticator app, makes the secret the account's. From then on
//! [`Authenticator::authenticate`] takes a code (`method` `totp`) or a recovery code
//! (`method` `recovery_code`) for the account named by `name`, which is the
//! [`Person::id`](coqui_access::Person::id) an earlier step named.
//!
//! The provider does not throttle: a code is one in a million, so the consumer's throttle and
//! its pending sign-in's try cap are what bound guessing. Build it in code with
//! [`TotpAccounts::builder`].
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

mod otp;

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Schema, ToValue};
use coqui_access::{
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, Description, Lookup, PutOutcome,
    Verdict, Waiting, error, methods, takes,
};
use coqui_access_kit::{
    Keyed, MemoryTable, RecordLimits, Reloading, SourceError, Table, check_keys, secret, unix_now,
};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

pub use otp::Algorithm;
use otp::{Check, base32, unbase32, verify};

/// The most accounts the tables may hold, each, unless the builder says otherwise.
pub const DEFAULT_MAX_RECORDS: usize = 10_000;
/// Digits of a code unless the builder says otherwise.
pub const DEFAULT_DIGITS: u32 = 6;
/// Seconds one code lasts unless the builder says otherwise.
pub const DEFAULT_PERIOD: u32 = 30;
/// Steps either side of the present one a code is still accepted in, unless the builder
/// says otherwise.
pub const DEFAULT_SKEW: u32 = 1;
/// Recovery codes made at enrolment unless the builder says otherwise.
pub const DEFAULT_RECOVERY_CODES: u32 = 10;
/// Seconds an enrolment waits for its first code unless the builder says otherwise.
pub const DEFAULT_ENROL_SECONDS: u32 = 600;

/// What a recovery code's digest is taken under, so one kept for this cannot be presented as
/// another secret.
const RECOVERY_DOMAIN: &str = "coqui-access-totp recovery code";
const RECOVERY_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const RECOVERY_LENGTH: usize = 10;

/// An account's secret and what it has used, as a [`Table`] keeps it, keyed by
/// [`name`](TotpRecord::name).
///
/// It derives guatiao's `ToValue`, `FromValue` and `Schema`, so a [`Table`] may keep it in any
/// format. The secret is kept readable, as a time-based code needs it to be: keep the table
/// where only the service can read it.
#[derive(Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct TotpRecord {
    /// The account key: the `Person::id` a first factor named; unique in the table.
    pub name: String,
    /// The label the app shows, when the person gave one.
    pub label: Option<String>,
    /// The shared secret as unpadded base32.
    pub secret: String,
    /// `sha1`, `sha256` or `sha512`.
    pub algorithm: String,
    /// Digits of a code, 6 to 8.
    pub digits: u32,
    /// Seconds a code lasts.
    pub period: u32,
    /// The latest time step a code was accepted for; that step and earlier ones are refused.
    pub last_step: Option<i64>,
    /// Digests of the recovery codes not used yet.
    #[map(default, skip_serializing_if = Vec::is_empty)]
    pub codes: Vec<String>,
    /// A disabled account takes no code.
    #[map(default, skip_serializing_if = core::ops::Not::not)]
    pub disabled: bool,
    /// When it was enrolled, in seconds since the Unix epoch.
    pub made_unix: Option<i64>,
    /// Wrong codes since the last right one; not written while zero.
    #[map(default, skip_serializing_if = is_zero)]
    pub failures: u32,
    /// Until when the account takes no code, in seconds since the Unix epoch, after too many
    /// wrong ones ([`Lockout`]).
    pub locked_until_unix: Option<i64>,
    /// An administrator reset the account: its secret and recovery codes are gone (the secret
    /// is empty) and it takes no code until a new secret is confirmed.
    #[map(default, skip_serializing_if = core::ops::Not::not)]
    pub reset: bool,
    /// When an administrator last cleared the failure count, in seconds since the Unix epoch.
    pub unlocked_unix: Option<i64>,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

impl Keyed for TotpRecord {
    fn key(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.name)
    }
}

impl fmt::Debug for TotpRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TotpRecord")
            .field("name", &"[REDACTED]")
            .field("secret", &"[REDACTED]")
            .field("algorithm", &self.algorithm)
            .field("digits", &self.digits)
            .field("period", &self.period)
            .field("codes", &self.codes.len())
            .field("disabled", &self.disabled)
            .field("failures", &self.failures)
            .field("reset", &self.reset)
            .finish()
    }
}

/// How an account that is given wrong codes slows down: after `after` wrong ones in a row it
/// takes no code for `first_seconds`, and for twice as long with each further wrong one, up to
/// `longest_seconds`. A right code ends it; an administrator's `disabled` toggle resets it.
///
/// This bounds guessing per account whatever address the guesses come from. It also lets
/// anyone who can name an account keep that account's owner out for up to `longest_seconds` at
/// a time; the consumer's own throttle per address and account is what limits that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Lockout {
    /// Wrong codes in a row before the first delay; 0 never delays.
    pub after: u32,
    /// The first delay, in seconds.
    pub first_seconds: u32,
    /// The longest delay, in seconds.
    pub longest_seconds: u32,
}

impl Lockout {
    /// Five wrong codes, then 30 s, doubling to an hour: the default.
    pub const DEFAULT: Self = Self {
        after: 5,
        first_seconds: 30,
        longest_seconds: 3_600,
    };
    /// Never delays. A weakening the caller chooses; the consumer must then count and limit
    /// second-factor failures itself.
    pub const OFF: Self = Self {
        after: 0,
        first_seconds: 0,
        longest_seconds: 0,
    };

    /// A lockout after `after` wrong codes in a row, delaying `first_seconds` and doubling up
    /// to `longest_seconds`. `after` 0 never delays.
    pub const fn new(after: u32, first_seconds: u32, longest_seconds: u32) -> Self {
        Self {
            after,
            first_seconds,
            longest_seconds,
        }
    }

    /// The delay once `failures` wrong codes have come in a row.
    fn delay(self, failures: u32) -> Option<i64> {
        if self.after == 0 || failures < self.after {
            return None;
        }
        let doublings = (failures - self.after).min(32);
        let seconds = u64::from(self.first_seconds)
            .saturating_mul(1u64 << doublings)
            .min(u64::from(self.longest_seconds));
        Some(i64::try_from(seconds).unwrap_or(i64::MAX))
    }
}

impl Default for Lockout {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// An enrolment waiting for its first code, keyed by [`name`](PendingTotp::name).
///
/// It derives guatiao's `ToValue`, `FromValue` and `Schema`. It holds the secret readable,
/// like [`TotpRecord`].
#[derive(Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct PendingTotp {
    /// The account key, as in [`TotpRecord`].
    pub name: String,
    /// The label the app shows.
    pub label: Option<String>,
    /// The shared secret as unpadded base32.
    pub secret: String,
    /// `sha1`, `sha256` or `sha512`.
    pub algorithm: String,
    /// Digits of a code.
    pub digits: u32,
    /// Seconds a code lasts.
    pub period: u32,
    /// Digests of the recovery codes made with it.
    #[map(default, skip_serializing_if = Vec::is_empty)]
    pub codes: Vec<String>,
    /// When it was begun, in seconds since the Unix epoch.
    pub created_unix: i64,
}

impl Keyed for PendingTotp {
    fn key(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.name)
    }
}

impl fmt::Debug for PendingTotp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingTotp")
            .field("name", &"[REDACTED]")
            .field("secret", &"[REDACTED]")
            .field("codes", &self.codes.len())
            .field("created_unix", &self.created_unix)
            .finish()
    }
}

type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// What presenting a code came to, and the record it leaves.
struct Judged {
    taken: bool,
    next: Option<TotpRecord>,
}

/// Options for [`TotpAccounts`]; every one has a default.
pub struct TotpBuilder {
    table: Option<Arc<dyn Table<TotpRecord>>>,
    pending: Option<Arc<dyn Table<PendingTotp>>>,
    issuer: Option<String>,
    algorithm: Algorithm,
    digits: u32,
    period: u32,
    skew: u32,
    recovery_codes: u32,
    enrol_seconds: u32,
    max_records: usize,
    limits: RecordLimits,
    lockout: Lockout,
    clock: Clock,
}

impl fmt::Debug for TotpBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TotpBuilder")
            .field("algorithm", &self.algorithm)
            .field("digits", &self.digits)
            .field("period", &self.period)
            .field("skew", &self.skew)
            .finish_non_exhaustive()
    }
}

impl TotpBuilder {
    /// Keeps the enrolled accounts in `table` instead of in memory.
    #[must_use]
    pub fn table(mut self, table: Arc<dyn Table<TotpRecord>>) -> Self {
        self.table = Some(table);
        self
    }

    /// Keeps the enrolments waiting for a first code in `table` instead of in memory.
    #[must_use]
    pub fn pending(mut self, table: Arc<dyn Table<PendingTotp>>) -> Self {
        self.pending = Some(table);
        self
    }

    /// The name an authenticator app shows beside the account (`issuer` in the link).
    #[must_use]
    pub fn issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// The hash new secrets use. Default [`Algorithm::Sha1`], which every app reads.
    #[must_use]
    pub fn algorithm(mut self, algorithm: Algorithm) -> Self {
        self.algorithm = algorithm;
        self
    }

    /// Digits of a new secret's codes, 6 to 8. Default [`DEFAULT_DIGITS`].
    #[must_use]
    pub fn digits(mut self, digits: u32) -> Self {
        self.digits = digits;
        self
    }

    /// Seconds a new secret's code lasts, at least 1. Default [`DEFAULT_PERIOD`].
    #[must_use]
    pub fn period(mut self, seconds: u32) -> Self {
        self.period = seconds;
        self
    }

    /// Steps either side of the present one a code is accepted in. Default
    /// [`DEFAULT_SKEW`]; 0 accepts the present step only. Each step widens the window a
    /// guess has to land in, and costs one code computed per attempt.
    #[must_use]
    pub fn skew(mut self, steps: u32) -> Self {
        self.skew = steps;
        self
    }

    /// Recovery codes made at enrolment. Default [`DEFAULT_RECOVERY_CODES`]; 0 makes none.
    #[must_use]
    pub fn recovery_codes(mut self, count: u32) -> Self {
        self.recovery_codes = count;
        self
    }

    /// Seconds an enrolment waits for its first code. Default [`DEFAULT_ENROL_SECONDS`].
    #[must_use]
    pub fn enrol_seconds(mut self, seconds: u32) -> Self {
        self.enrol_seconds = seconds;
        self
    }

    /// The most accounts, and the most enrolments waiting, the tables may hold. Default
    /// [`DEFAULT_MAX_RECORDS`].
    #[must_use]
    pub fn max_records(mut self, max: usize) -> Self {
        self.max_records = max;
        self
    }

    /// How large an account's name and label may be. Default [`RecordLimits::DEFAULT`].
    #[must_use]
    pub fn record_limits(mut self, limits: RecordLimits) -> Self {
        self.limits = limits;
        self
    }

    /// How an account slows down under wrong codes. Default [`Lockout::DEFAULT`];
    /// [`Lockout::OFF`] lifts it.
    #[must_use]
    pub fn lockout(mut self, lockout: Lockout) -> Self {
        self.lockout = lockout;
        self
    }

    /// The clock, in seconds since the Unix epoch. Default the system's; a test supplies its
    /// own.
    #[must_use]
    pub fn clock(mut self, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Loads the tables and makes the provider.
    ///
    /// # Errors
    ///
    /// When a setting means nothing (digits outside 6 to 8, a period of 0 seconds, no time to
    /// enrol), or a table cannot be loaded or holds an invalid or duplicated account.
    pub fn build(self) -> Result<TotpAccounts, SourceError> {
        if !(6..=8).contains(&self.digits)
            || self.period == 0
            || self.enrol_seconds == 0
            || self
                .issuer
                .as_deref()
                .is_some_and(|issuer| issuer.is_empty() || issuer.chars().any(char::is_control))
        {
            return Err(SourceError::invalid(
                "a one-time code setting is out of range",
            ));
        }
        let table = self
            .table
            .unwrap_or_else(|| Arc::new(MemoryTable::<TotpRecord>::new()));
        let pending = self
            .pending
            .unwrap_or_else(|| Arc::new(MemoryTable::<PendingTotp>::new()));
        let (max, limits) = (self.max_records, self.limits);
        Ok(TotpAccounts {
            records: Reloading::new(table, move |records| validate(records, max, &limits))?,
            pending: Reloading::new(pending, move |records| {
                validate_pending(records, max, &limits)
            })?,
            limits,
            issuer: self.issuer,
            algorithm: self.algorithm,
            digits: self.digits,
            period: self.period,
            skew: self.skew,
            recovery_codes: self.recovery_codes,
            enrol_seconds: self.enrol_seconds,
            lockout: self.lockout,
            clock: self.clock,
        })
    }
}

/// One-time codes as an [`Authenticator`] and [`Accounts`] implementation.
///
/// ```
/// use coqui_access::{AccountPut, Accounts, Attempt, Authenticator, Verdict};
/// use coqui_access_totp::TotpAccounts;
///
/// let totp = TotpAccounts::builder().issuer("Example").build().unwrap();
/// let begun = totp
///     .put(AccountPut { name: "id-alice".into(), ..AccountPut::default() })
///     .unwrap();
/// assert!(begun.secret.unwrap().starts_with("otpauth://totp/"));
/// assert_eq!(begun.codes.unwrap().len(), 10);
/// // Nothing is a factor until the first code is confirmed.
/// let verdict = totp
///     .authenticate(Attempt {
///         name: Some("id-alice".into()),
///         secret: Some("123456".into()),
///         ..Attempt::default()
///     })
///     .unwrap();
/// assert_eq!(verdict, Verdict::Unknown);
/// ```
pub struct TotpAccounts {
    records: Reloading<TotpRecord>,
    pending: Reloading<PendingTotp>,
    limits: RecordLimits,
    issuer: Option<String>,
    algorithm: Algorithm,
    digits: u32,
    period: u32,
    skew: u32,
    recovery_codes: u32,
    enrol_seconds: u32,
    lockout: Lockout,
    clock: Clock,
}

impl fmt::Debug for TotpAccounts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TotpAccounts")
            .field("algorithm", &self.algorithm)
            .field("digits", &self.digits)
            .field("period", &self.period)
            .field("skew", &self.skew)
            .finish_non_exhaustive()
    }
}

impl TotpAccounts {
    /// Options for the provider, kept in memory unless
    /// [`table`](TotpBuilder::table) says otherwise.
    pub fn builder() -> TotpBuilder {
        TotpBuilder {
            table: None,
            pending: None,
            issuer: None,
            algorithm: Algorithm::Sha1,
            digits: DEFAULT_DIGITS,
            period: DEFAULT_PERIOD,
            skew: DEFAULT_SKEW,
            recovery_codes: DEFAULT_RECOVERY_CODES,
            enrol_seconds: DEFAULT_ENROL_SECONDS,
            max_records: DEFAULT_MAX_RECORDS,
            limits: RecordLimits::DEFAULT,
            lockout: Lockout::DEFAULT,
            clock: Arc::new(unix_now),
        }
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn check_of(&self, algorithm: &str, digits: u32, period: u32) -> Option<Check> {
        Some(Check {
            algorithm: Algorithm::parse_word(algorithm)?,
            digits,
            period,
            skew: self.skew,
        })
    }

    /// Decides what presenting `presented` to `record` comes to, and the record it leaves:
    /// `None` when nothing is to be written.
    fn judge(&self, record: &TotpRecord, presented: &str, recovery: bool, now: i64) -> Judged {
        if record.disabled
            || record.reset
            || record.locked_until_unix.is_some_and(|until| now < until)
        {
            return Judged {
                taken: false,
                next: None,
            };
        }
        let mut next = record.clone();
        let taken = if recovery {
            self.use_recovery(&mut next, presented)
        } else {
            self.use_code(&mut next, presented, now)
        };
        if taken {
            next.failures = 0;
            next.locked_until_unix = None;
        } else if self.lockout.after > 0 {
            next.failures = next.failures.saturating_add(1);
            next.locked_until_unix = self
                .lockout
                .delay(next.failures)
                .map(|delay| now.saturating_add(delay));
        } else {
            return Judged {
                taken: false,
                next: None,
            };
        }
        Judged {
            taken,
            next: Some(next),
        }
    }

    /// Checks a time-based code and, if right, notes the step it was for in `record`.
    fn use_code(&self, record: &mut TotpRecord, presented: &str, now: i64) -> bool {
        let (Some(check), Some(key)) = (
            self.check_of(&record.algorithm, record.digits, record.period),
            unbase32(&record.secret),
        ) else {
            return false;
        };
        match verify(check, &key, presented, now, record.last_step) {
            Some(step) => {
                record.last_step = Some(step);
                true
            }
            None => false,
        }
    }

    /// Checks a recovery code and, if right, removes it from `record`. Every code is
    /// compared, none stopping the others.
    fn use_recovery(&self, record: &mut TotpRecord, presented: &str) -> bool {
        let Some(normal) = normal_recovery(presented) else {
            return false;
        };
        let digest = secret::hash(&normal, Some(RECOVERY_DOMAIN));
        let mut held = false;
        for code in &record.codes {
            held |= secret::same(code, &digest);
        }
        if held {
            record.codes.retain(|code| !secret::same(code, &digest));
        }
        held
    }

    /// Presents a code to `record`, written down before the answer.
    ///
    /// The judgement is made against the record read under the table's lock, so two requests
    /// with one code (here or in another process) cannot both succeed, and a wrong code is
    /// counted. Only this account's record is read and written. A write that fails is an
    /// error, never a success that could be replayed.
    fn present(
        &self,
        record: &TotpRecord,
        presented: &str,
        recovery: bool,
    ) -> Result<Verdict, ProviderError> {
        let now = self.now();
        // Nothing to write (a disabled or delayed account, a lifted lockout): no write.
        if self.judge(record, presented, recovery, now).next.is_none() {
            return Ok(Verdict::Invalid);
        }
        self.records
            .update_one(&record.name, |current| {
                let Some(current) = current else {
                    return Ok((None, Verdict::Unknown));
                };
                let judged = self.judge(current, presented, recovery, now);
                let verdict = verdict(judged.taken, &current.name);
                Ok((judged.next, verdict))
            })
            .map_err(|reason| reason.writing("one-time code table"))
    }

    /// An administrator's `reset` and `unlock` for an enrolled account, in one write under the
    /// table's lock. `disabled`, when given with them, is applied in the same write.
    fn administer(&self, account: &AccountPut) -> Result<PutOutcome, ProviderError> {
        let now = self.now();
        let found = self
            .records
            .update_one(&account.name, |current| {
                let Some(current) = current else {
                    return Ok((None, false));
                };
                let mut next = current.clone();
                if account.reset {
                    next.reset = true;
                    next.secret = String::new();
                    next.codes = Vec::new();
                    next.last_step = None;
                }
                if account.unlock {
                    next.unlocked_unix = Some(now);
                }
                if account.unlock || account.reset {
                    next.failures = 0;
                    next.locked_until_unix = None;
                }
                if let Some(disabled) = account.disabled {
                    next.disabled = disabled;
                }
                Ok((Some(next), true))
            })
            .map_err(|reason| reason.writing("one-time code table"))?;
        if account.reset {
            // An enrolment begun before the reset would let the person set the factor up
            // again without an administrator.
            self.pending
                .remove(&account.name)
                .map_err(|reason| reason.writing("one-time code table"))?;
        }
        if found {
            Ok(PutOutcome::default())
        } else {
            Err(error::not_found("there is no such account"))
        }
    }

    fn begin(&self, account: &AccountPut) -> Result<PutOutcome, ProviderError> {
        // Confirming replaces the factor the person holds, so beginning that replacement is
        // for a caller that has checked the current one.
        if !account.proved_current {
            let enrolled = self
                .records
                .read(|records| {
                    records
                        .iter()
                        .any(|record| record.name == account.name && !record.reset)
                })
                .map_err(|reason| reason.reading("one-time code table"))?;
            if enrolled {
                return Err(error::bad_value(
                    "replacing the one-time code secret needs the caller to prove the current one first",
                ));
            }
        }
        let mut key = [0u8; 20];
        getrandom::fill(&mut key)
            .map_err(|_| error::internal("system random source is unavailable"))?;
        let mut codes = Vec::new();
        let mut shown = Vec::new();
        for _ in 0..self.recovery_codes {
            let code = recovery_code()?;
            codes.push(secret::hash(
                &normal_recovery(&code).unwrap_or_default(),
                Some(RECOVERY_DOMAIN),
            ));
            shown.push(code);
        }
        let label = account.display_name.clone();
        if label
            .as_deref()
            .is_some_and(|label| !self.limits.allows_display_name(label))
        {
            return Err(error::bad_value("that label is not valid"));
        }
        let entry = PendingTotp {
            name: account.name.clone(),
            label: label.clone(),
            secret: base32(&key),
            algorithm: self.algorithm.as_str().to_owned(),
            digits: self.digits,
            period: self.period,
            codes,
            created_unix: self.now(),
        };
        let link = self.link(label.as_deref().unwrap_or(&account.name), &entry);
        self.pending
            .put_with(|_| Ok((entry, ())))
            .map_err(|reason| reason.writing("one-time code table"))?;
        Ok(PutOutcome {
            secret: Some(link),
            codes: Some(shown),
        })
    }

    fn confirm(&self, name: &str, code: &str) -> Result<PutOutcome, ProviderError> {
        let entry = self
            .pending
            .read(|entries| entries.iter().find(|entry| entry.name == name).cloned())
            .map_err(|reason| reason.reading("one-time code table"))?
            .ok_or_else(|| error::not_found("no enrolment is waiting for that account"))?;
        let now = self.now();
        if now.saturating_sub(entry.created_unix) > i64::from(self.enrol_seconds) {
            let _ = self.pending.remove(name);
            return Err(error::bad_value("the enrolment has expired"));
        }
        let (Some(check), Some(key)) = (
            self.check_of(&entry.algorithm, entry.digits, entry.period),
            unbase32(&entry.secret),
        ) else {
            return Err(error::internal("an enrolment's secret cannot be read"));
        };
        let Some(step) = verify(check, &key, code, now, None) else {
            return Err(error::bad_value("that code is not right"));
        };
        let record = TotpRecord {
            name: entry.name.clone(),
            label: entry.label.clone(),
            secret: entry.secret.clone(),
            algorithm: entry.algorithm.clone(),
            digits: entry.digits,
            period: entry.period,
            // The code that proved the secret is spent.
            last_step: Some(step),
            codes: entry.codes.clone(),
            disabled: false,
            made_unix: Some(now),
            failures: 0,
            locked_until_unix: None,
            reset: false,
            unlocked_unix: None,
        };
        self.records
            .put_with(|_| Ok((record, ())))
            .map_err(|reason| reason.writing("one-time code table"))?;
        self.pending
            .remove(name)
            .map_err(|reason| reason.writing("one-time code table"))?;
        Ok(PutOutcome::default())
    }

    /// The `otpauth://` link an app reads, from the label and the enrolment's own settings.
    fn link(&self, label: &str, entry: &PendingTotp) -> String {
        let encode = |text: &str| utf8_percent_encode(text, LINK).to_string();
        let mut link = String::from("otpauth://totp/");
        if let Some(issuer) = &self.issuer {
            link.push_str(&encode(issuer));
            link.push(':');
        }
        link.push_str(&encode(label));
        link.push_str("?secret=");
        link.push_str(&entry.secret);
        if let Some(issuer) = &self.issuer {
            link.push_str("&issuer=");
            link.push_str(&encode(issuer));
        }
        let algorithm = Algorithm::parse_word(&entry.algorithm).unwrap_or_default();
        link.push_str(&format!(
            "&algorithm={}&digits={}&period={}",
            algorithm.uri_word(),
            entry.digits,
            entry.period
        ));
        link
    }
}

/// What a link leaves unescaped: letters, digits and `-._~`.
const LINK: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn verdict(taken: bool, name: &str) -> Verdict {
    if taken {
        Verdict::Valid(Lookup {
            by: "id".to_owned(),
            value: name.to_owned(),
            claims: None,
        })
    } else {
        Verdict::Invalid
    }
}

/// A recovery code, `XXXXX-XXXXX`: the hyphen keeps its shape apart from a numeric code.
fn recovery_code() -> Result<String, ProviderError> {
    let mut random = [0u8; RECOVERY_LENGTH];
    getrandom::fill(&mut random)
        .map_err(|_| error::internal("system random source is unavailable"))?;
    let mut code = String::with_capacity(RECOVERY_LENGTH + 1);
    for (index, byte) in random.iter().enumerate() {
        if index == RECOVERY_LENGTH / 2 {
            code.push('-');
        }
        code.push(char::from(RECOVERY_ALPHABET[usize::from(byte & 31)]));
    }
    Ok(code)
}

/// A recovery code as it is compared: upper case, without the hyphen or spaces; `None` when
/// it cannot be one.
fn normal_recovery(text: &str) -> Option<String> {
    let normal: String = text
        .chars()
        .filter(|character| *character != '-' && !character.is_whitespace())
        .map(|character| character.to_ascii_uppercase())
        .collect();
    (normal.len() == RECOVERY_LENGTH
        && normal.bytes().all(|byte| RECOVERY_ALPHABET.contains(&byte)))
    .then_some(normal)
}

impl Authenticator for TotpAccounts {
    /// `name` is the account (a `Person::id`) and `secret` the code. `method` is absent or
    /// `totp` for a time-based code, `recovery_code` for a recovery code; any other method
    /// is `Unknown`, as is an account not enrolled. A disabled account, a wrong code and a
    /// step already used are `Invalid`. A success is written down before it is answered, and
    /// a write that fails is an error, never a success that could be used again.
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let (Some(name), Some(presented)) = (attempt.name, attempt.secret) else {
            return Ok(Verdict::Unknown);
        };
        let recovery = match attempt.method.as_deref() {
            None | Some(methods::TOTP) => false,
            Some(methods::RECOVERY_CODE) => true,
            Some(_) => return Ok(Verdict::Unknown),
        };
        // Only this account's record is read: another's, unreadable or not, is nothing to this
        // sign-in, and one that cannot be read fails its own account alone.
        let Some(record) = self
            .records
            .get(&name)
            .map_err(|reason| reason.reading("one-time code table"))?
        else {
            return Ok(Verdict::Unknown);
        };
        // A reset account holds no factor: it is not enrolled until a new secret is confirmed.
        if record.reset {
            return Ok(Verdict::Unknown);
        }
        self.present(&record, &presented, recovery)
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            name: Some("One-time codes".to_owned()),
            takes: Some(takes::OTP.to_owned()),
        })
    }
}

impl Accounts for TotpAccounts {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        self.records
            .read(|records| {
                records
                    .iter()
                    .map(|record| AccountInfo {
                        id: record.name.clone(),
                        name: record.name.clone(),
                        groups: Vec::new(),
                        disabled: Some(record.disabled),
                        made_unix: record.made_unix,
                        expires_unix: None,
                        reset: record.reset.then_some(true),
                        unlocked_unix: record.unlocked_unix,
                        ..AccountInfo::default()
                    })
                    .collect()
            })
            .map_err(|reason| reason.reading("one-time code table"))
    }

    /// Enrols, confirms, or disables.
    ///
    /// - With no `secret` and no `disabled`: begins an enrolment, replacing one waiting for
    ///   the account. The answer's `secret` is the `otpauth://` link (its label the
    ///   `display_name`, else the name) and `codes` the recovery codes, both said once. For
    ///   an account that is already enrolled (a rotation) it needs `proved_current`, the
    ///   caller's statement that it checked the current secret, and is refused with
    ///   `GUATIAO_ERR_BAD_VALUE` without it.
    /// - With a `secret` that is a code from the app: confirms the enrolment waiting for the
    ///   account, within `enrol_seconds`. The new secret replaces any the account had, which
    ///   keeps working until then. A wrong code is `GUATIAO_ERR_BAD_VALUE`; no enrolment
    ///   waiting `GUATIAO_ERR_NOT_FOUND`.
    /// - With `disabled` and no `secret`: disables or enables the enrolled account.
    ///
    /// `groups` and `expires_unix` are refused.
    fn put(&self, account: AccountPut) -> Result<PutOutcome, ProviderError> {
        if !self.limits.allows_field(&account.name) {
            return Err(error::bad_value("an account has a name"));
        }
        if account.groups.is_some() || account.expires_unix.is_some() {
            return Err(error::bad_value(
                "one-time code accounts have no groups and do not expire",
            ));
        }
        if account.reset || account.unlock {
            if account.secret.is_some() {
                return Err(error::bad_value(
                    "a reset or an unlock does not come with a secret",
                ));
            }
            return self.administer(&account);
        }
        match (&account.secret, account.disabled) {
            (Some(code), _) => self.confirm(&account.name, code),
            (None, Some(disabled)) => {
                let known = self
                    .records
                    .read(|records| records.iter().any(|record| record.name == account.name))
                    .map_err(|reason| reason.reading("one-time code table"))?;
                if !known {
                    return Err(error::not_found("there is no such account"));
                }
                self.records
                    .put_with(|records| {
                        let mut next = records
                            .iter()
                            .find(|record| record.name == account.name)
                            .cloned()
                            .ok_or_else(|| SourceError::invalid("the account is gone"))?;
                        next.disabled = disabled;
                        next.failures = 0;
                        next.locked_until_unix = None;
                        Ok((next, ()))
                    })
                    .map_err(|reason| reason.writing("one-time code table"))?;
                Ok(PutOutcome::default())
            }
            (None, None) => self.begin(&account),
        }
    }

    /// Takes the account's secret and any enrolment waiting away: an administrator's reset.
    fn remove(&self, name: String) -> Result<(), ProviderError> {
        let enrolled = self
            .records
            .remove(&name)
            .map_err(|reason| reason.writing("one-time code table"))?;
        let waiting = self
            .pending
            .remove(&name)
            .map_err(|reason| reason.writing("one-time code table"))?;
        if enrolled || waiting {
            Ok(())
        } else {
            Err(error::not_found("there is no such account"))
        }
    }

    /// The enrolments that have not been confirmed, `shown` being the account name.
    fn waiting(&self) -> Result<Vec<Waiting>, ProviderError> {
        self.pending
            .read(|entries| {
                entries
                    .iter()
                    .map(|entry| Waiting {
                        shown: entry.name.clone(),
                        peer: None,
                        seen_unix: Some(entry.created_unix),
                    })
                    .collect()
            })
            .map_err(|reason| reason.reading("one-time code table"))
    }
}

fn validate(records: &[TotpRecord], max: usize, limits: &RecordLimits) -> Result<(), SourceError> {
    check_keys(records, max)?;
    for record in records {
        let valid = limits.allows_field(&record.name)
            && Algorithm::parse_word(&record.algorithm).is_some()
            && (6..=8).contains(&record.digits)
            && record.period >= 1
            && (record.reset || unbase32(&record.secret).is_some_and(|key| !key.is_empty()))
            && record.codes.iter().all(|code| secret::is_hash(code))
            && record
                .label
                .as_deref()
                .is_none_or(|label| limits.allows_display_name(label));
        if !valid {
            return Err(SourceError::invalid("an account entry is invalid"));
        }
    }
    Ok(())
}

fn validate_pending(
    entries: &[PendingTotp],
    max: usize,
    limits: &RecordLimits,
) -> Result<(), SourceError> {
    check_keys(entries, max)?;
    for entry in entries {
        let valid = limits.allows_field(&entry.name)
            && Algorithm::parse_word(&entry.algorithm).is_some()
            && (6..=8).contains(&entry.digits)
            && entry.period >= 1
            && unbase32(&entry.secret).is_some_and(|key| !key.is_empty())
            && entry.codes.iter().all(|code| secret::is_hash(code));
        if !valid {
            return Err(SourceError::invalid("an enrolment entry is invalid"));
        }
    }
    Ok(())
}
