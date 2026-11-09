use alloc::sync::Arc;
use core::fmt;
use core::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

/// The wall clock a [`TrustStore`](crate::TrustStore) or a pairing desk reads for expiry and
/// retirement times.
///
/// [`Clock::system`] (the default) reads the system time; [`Clock::from_fn`] takes any
/// function, so a test can move time without sleeping.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> SystemTime + Send + Sync>);

impl Clock {
    /// The system clock.
    pub fn system() -> Self {
        Self(Arc::new(SystemTime::now))
    }

    /// A clock that reads `now`.
    pub fn from_fn(now: impl Fn() -> SystemTime + Send + Sync + 'static) -> Self {
        Self(Arc::new(now))
    }

    /// The current time.
    pub fn now(&self) -> SystemTime {
        (self.0)()
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::system()
    }
}

impl fmt::Debug for Clock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Clock")
    }
}

/// Whole seconds since the Unix epoch; a time before it is 0.
pub(crate) fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The time `seconds` after the Unix epoch, saturating at the latest time this platform holds.
pub(crate) fn from_unix_seconds(seconds: u64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .unwrap_or_else(|| UNIX_EPOCH + Duration::from_secs(u64::from(u32::MAX)))
}

/// `time` plus `by`, or the latest of `time` plus a century, a year, or nothing that the platform
/// can hold: an option of `Duration::MAX` means "never" and must not overflow.
pub(crate) fn later(time: SystemTime, by: Duration) -> SystemTime {
    [
        by,
        Duration::from_secs(100 * 365 * 86_400),
        Duration::from_secs(365 * 86_400),
    ]
    .into_iter()
    .filter(|step| *step <= by)
    .find_map(|step| time.checked_add(step))
    .unwrap_or(time)
}
