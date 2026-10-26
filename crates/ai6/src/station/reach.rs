//! Whether a harness the Station supervises on another host can still be reached.
//!
//! A runner or a wrapped harness that stops answering is given a grace period (an Operation's
//! `lost_after_secs`). Once it has been out of reach for all of it, the Station probes it once
//! more through the Theater's transport; if that fails too, the Session is lost and the
//! reason is kept here for the Station to act on.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::directorates::m::recovery::DEFAULT_LOST_AFTER_SECS;

/// Shared between the reader of a remote harness and the Station's event pump.
#[derive(Clone)]
pub(crate) struct ReachWatch(Arc<Inner>);

struct Inner {
    grace: Duration,
    since: Mutex<Option<Instant>>,
    lost: Mutex<Option<String>>,
}

impl Default for ReachWatch {
    fn default() -> Self {
        Self::new(Duration::from_secs(DEFAULT_LOST_AFTER_SECS))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ReachWatch {
    /// A watch that gives a harness `grace` out of reach before probing it.
    pub(crate) fn new(grace: Duration) -> Self {
        Self(Arc::new(Inner {
            grace,
            since: Mutex::new(None),
            lost: Mutex::new(None),
        }))
    }

    /// The grace this watch gives.
    pub(crate) fn grace(&self) -> Duration {
        self.0.grace
    }

    /// Notes that the harness answered, so the grace starts over the next time it does not.
    pub(crate) fn reached(&self) {
        *lock(&self.0.since) = None;
    }

    /// Notes that the harness did not answer. Once it has been out of reach for the whole
    /// grace, `probe` runs: its error is why the Session is lost, and success starts the
    /// grace over. Answers whether this was the first miss since it last answered.
    pub(crate) fn missed(&self, probe: impl FnOnce() -> Result<(), String>) -> bool {
        let now = Instant::now();
        let since = *lock(&self.0.since).get_or_insert(now);
        if since == now {
            return true;
        }
        if now.duration_since(since) < self.0.grace || self.lost().is_some() {
            return false;
        }
        match probe() {
            Ok(()) => *lock(&self.0.since) = Some(Instant::now()),
            Err(why) => self.lose(format!(
                "its harness was out of reach for {}s and a probe through the Theater's \
                 transport failed: {why}",
                self.0.grace.as_secs()
            )),
        }
        false
    }

    /// Gives the Session up as lost, keeping the first reason given.
    pub(crate) fn lose(&self, reason: impl Into<String>) {
        lock(&self.0.lost).get_or_insert_with(|| reason.into());
    }

    /// Why the Session was given up, once it was.
    pub(crate) fn lost(&self) -> Option<String> {
        lock(&self.0.lost).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_is_probed_only_after_the_whole_grace_and_lost_only_if_the_probe_fails() {
        let watch = ReachWatch::new(Duration::from_millis(50));
        assert!(watch.missed(|| panic!("probed on the first miss")));
        assert!(!watch.missed(|| panic!("probed within the grace")));
        std::thread::sleep(Duration::from_millis(60));
        // The probe answers: the grace starts over.
        assert!(!watch.missed(|| Ok(())));
        assert_eq!(watch.lost(), None);
        assert!(!watch.missed(|| panic!("probed again within the new grace")));
        // It answers between misses: a later miss is the first of a new outage.
        watch.reached();
        assert!(watch.missed(|| panic!("probed on the first miss of an outage")));
        std::thread::sleep(Duration::from_millis(60));
        watch.missed(|| Err("connection refused".to_owned()));
        let reason = watch.lost().unwrap();
        assert!(reason.contains("connection refused"), "{reason}");
        watch.lose("a later reason");
        assert_eq!(watch.lost().unwrap(), reason);
    }
}
