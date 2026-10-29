//! Monotonic Session activity, prompt and repeated-output monitoring.

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;
use serde::Deserialize;
use serde::de::Error as _;
use sha2::{Digest, Sha256};

/// Default time without provider or terminal activity before a Session is stalled.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Default delay before a quiet prompt-shaped Session is marked blocked.
pub const DEFAULT_BLOCKED_AFTER: Duration = Duration::from_secs(10);
/// Default number of repeated lines required before a Session is considered looping.
pub const DEFAULT_LOOP_MIN_LINES: usize = 50;
/// Default number of distinct terminal lines retained for loop detection.
pub const DEFAULT_LINE_HISTORY: usize = 4096;
/// Per-Agent overrides layered over the daemon's WatchdogConfig.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WatchdogOverrides {
    /// Maximum quiet period without provider or terminal activity.
    #[serde(deserialize_with = "deserialize_optional_duration")]
    pub stall_timeout: Option<Duration>,
    /// Quiet period after repeated output before loop detection fires.
    #[serde(deserialize_with = "deserialize_optional_duration")]
    pub loop_timeout: Option<Duration>,
    /// Repeated lines required before loop detection applies.
    pub loop_min_lines: Option<usize>,
    /// Quiet period before a prompt-shaped Session is marked blocked.
    #[serde(deserialize_with = "deserialize_optional_duration")]
    pub blocked_after: Option<Duration>,
    /// Optional maximum Session runtime.
    #[serde(deserialize_with = "deserialize_optional_duration")]
    pub max_runtime: Option<Duration>,
    /// Distinct terminal lines retained for repeated-output detection.
    pub history: Option<usize>,
    /// Normalize digit runs before comparing terminal lines.
    pub normalize_digits: Option<bool>,
}

impl WatchdogOverrides {
    pub(crate) fn merge(&self, child: &Self) -> Self {
        Self {
            stall_timeout: child.stall_timeout.or(self.stall_timeout),
            loop_timeout: child.loop_timeout.or(self.loop_timeout),
            loop_min_lines: child.loop_min_lines.or(self.loop_min_lines),
            blocked_after: child.blocked_after.or(self.blocked_after),
            max_runtime: child.max_runtime.or(self.max_runtime),
            history: child.history.or(self.history),
            normalize_digits: child.normalize_digits.or(self.normalize_digits),
        }
    }

    /// Applies the configured values over daemon-wide watchdog settings.
    pub fn apply_to(&self, base: &WatchdogConfig) -> WatchdogConfig {
        let mut config = base.clone();
        if let Some(timeout) = self.stall_timeout {
            config = config.with_stall_timeout(timeout);
        }
        if let Some(timeout) = self.loop_timeout {
            config = config.with_loop_timeout(timeout);
        }
        if let Some(count) = self.loop_min_lines {
            config = config.with_loop_min_lines(count);
        }
        if let Some(timeout) = self.blocked_after {
            config = config.with_blocked_after(timeout);
        }
        if let Some(timeout) = self.max_runtime {
            config = config.with_max_runtime(Some(timeout));
        }
        if let Some(count) = self.history {
            config = config.with_history(count);
        }
        if let Some(normalize) = self.normalize_digits {
            config = config.with_normalize_digits(normalize);
        }
        config
    }
}

fn deserialize_optional_duration<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<f64>::deserialize(deserializer)?
        .map(Duration::try_from_secs_f64)
        .transpose()
        .map_err(D::Error::custom)
}

/// Thresholds used to detect quiet, blocked and looping Agent Sessions.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchdogConfig {
    stall_timeout: Duration,
    loop_timeout: Duration,
    loop_min_lines: usize,
    blocked_after: Duration,
    max_runtime: Option<Duration>,
    history: usize,
    normalize_digits: bool,
    partial_line_bytes: crate::bounds::Limit,
    csi_bytes: crate::bounds::Limit,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            stall_timeout: DEFAULT_STALL_TIMEOUT,
            loop_timeout: DEFAULT_STALL_TIMEOUT,
            loop_min_lines: DEFAULT_LOOP_MIN_LINES,
            blocked_after: DEFAULT_BLOCKED_AFTER,
            max_runtime: None,
            history: DEFAULT_LINE_HISTORY,
            normalize_digits: false,
            partial_line_bytes: crate::bounds::Limit::at(4096),
            csi_bytes: crate::station::limits::TerminalOutputLimits::default().csi_bytes,
        }
    }
}

impl WatchdogConfig {
    /// Sets the maximum period without provider or terminal activity.
    pub fn with_stall_timeout(mut self, timeout: Duration) -> Self {
        self.stall_timeout = timeout;
        self
    }

    /// Sets the quiet period used after repeated terminal output.
    pub fn with_loop_timeout(mut self, timeout: Duration) -> Self {
        self.loop_timeout = timeout;
        self
    }

    /// Sets how many repeated lines are required before loop detection fires.
    pub fn with_loop_min_lines(mut self, count: usize) -> Self {
        self.loop_min_lines = count;
        self
    }

    /// Sets the quiet period before a prompt-shaped terminal tail is marked blocked.
    pub fn with_blocked_after(mut self, timeout: Duration) -> Self {
        self.blocked_after = timeout;
        self
    }

    /// Sets an optional maximum Session runtime; `None` disables this limit.
    pub fn with_max_runtime(mut self, timeout: Option<Duration>) -> Self {
        self.max_runtime = timeout;
        self
    }

    /// Sets the number of distinct terminal lines retained for loop detection.
    pub fn with_history(mut self, count: usize) -> Self {
        self.history = count;
        self
    }

    /// Treats digit runs as `#` while comparing terminal lines for loops.
    pub fn with_normalize_digits(mut self, normalize: bool) -> Self {
        self.normalize_digits = normalize;
        self
    }

    /// Sets how many bytes of a terminal line are held before it counts as a line of its own
    /// (default 4,096); `Limit::UNBOUNDED` waits for its end.
    pub fn with_partial_line_bytes(mut self, bytes: crate::bounds::Limit) -> Self {
        self.partial_line_bytes = bytes;
        self
    }

    /// Sets how long a terminal control sequence may be before it is read as text
    /// (default 64 bytes, `[terminal_output] csi_bytes`).
    pub fn with_csi_bytes(mut self, bytes: crate::bounds::Limit) -> Self {
        self.csi_bytes = bytes;
        self
    }

    /// Returns the maximum period without provider or terminal activity.
    pub fn stall_timeout(&self) -> Duration {
        self.stall_timeout
    }

    /// Returns the quiet period used after repeated terminal output.
    pub fn loop_timeout(&self) -> Duration {
        self.loop_timeout
    }

    /// Returns the repeated-line count required before loop detection fires.
    pub fn loop_min_lines(&self) -> usize {
        self.loop_min_lines
    }

    /// Returns the quiet period before a prompt-shaped terminal tail is marked blocked.
    pub fn blocked_after(&self) -> Duration {
        self.blocked_after
    }

    /// Returns the optional maximum Session runtime.
    pub fn max_runtime(&self) -> Option<Duration> {
        self.max_runtime
    }

    /// Returns the number of distinct terminal lines retained for loop detection.
    pub fn history(&self) -> usize {
        self.history
    }

    /// Returns whether digit runs are normalized before terminal-line comparison.
    pub fn normalize_digits(&self) -> bool {
        self.normalize_digits
    }
}

/// Measures activity, prompt waits and repeated terminal output for an Agent Session.
#[derive(Debug, Clone)]
pub struct Watchdog {
    config: WatchdogConfig,
    state: Arc<Mutex<ActivityState>>,
}

#[derive(Debug)]
struct ActivityState {
    started: Instant,
    last_activity: Instant,
    last_novel: Instant,
    lines_since_novel: usize,
    partial_line: Vec<u8>,
    seen_lines: LruCache<[u8; 32], ()>,
    output_revision: u64,
    paused_since: Option<Instant>,
}

impl Default for Watchdog {
    fn default() -> Self {
        Self::new(DEFAULT_STALL_TIMEOUT)
    }
}

impl Watchdog {
    /// Starts monitoring with a maximum quiet period.
    pub fn new(timeout: Duration) -> Self {
        Self::with_config(
            WatchdogConfig::default()
                .with_stall_timeout(timeout)
                .with_loop_timeout(timeout),
        )
    }

    /// Starts monitoring with explicit thresholds and history settings.
    #[must_use]
    pub fn with_config(config: WatchdogConfig) -> Self {
        let now = Instant::now();
        let history = NonZeroUsize::new(config.history.max(1)).unwrap_or(NonZeroUsize::MIN);
        Self {
            config,
            state: Arc::new(Mutex::new(ActivityState {
                started: now,
                last_activity: now,
                last_novel: now,
                lines_since_novel: 0,
                partial_line: Vec::new(),
                seen_lines: LruCache::new(history),
                output_revision: 0,
                paused_since: None,
            })),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_limits(
        timeout: Duration,
        loop_timeout: Duration,
        blocked_after: Duration,
        loop_min_lines: usize,
    ) -> Self {
        Self::with_config(
            WatchdogConfig::default()
                .with_stall_timeout(timeout)
                .with_loop_timeout(loop_timeout)
                .with_blocked_after(blocked_after)
                .with_loop_min_lines(loop_min_lines),
        )
    }

    /// Stops judging the Session while it waits on a person, such as for green ink: a paused
    /// Session is never stalled, looping, blocked at a prompt or past its maximum runtime.
    /// Pausing again is harmless.
    pub fn pause(&self) {
        let mut state = self.lock_state();
        if state.paused_since.is_none() {
            state.paused_since = Some(Instant::now());
        }
    }

    /// Judges the Session again as if it had just been active; the time spent paused does
    /// not count toward the maximum runtime.
    pub fn resume(&self) {
        let now = Instant::now();
        let mut state = self.lock_state();
        if let Some(since) = state.paused_since.take() {
            state.started += now.saturating_duration_since(since);
        }
        state.last_activity = now;
        state.last_novel = now;
        state.lines_since_novel = 0;
    }

    /// Whether the Session is paused.
    pub fn is_paused(&self) -> bool {
        self.lock_state().paused_since.is_some()
    }

    /// Records provider output or operator input as Session activity.
    pub fn record_activity(&self) {
        self.lock_state().last_activity = Instant::now();
    }

    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    pub(crate) fn record_operator_input(&self) {
        let now = Instant::now();
        let mut state = self.lock_state();
        state.last_activity = now;
        state.last_novel = now;
        state.lines_since_novel = 0;
    }

    pub(crate) fn record_terminal_output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut state = self.lock_state();
        state.last_activity = now;
        state.output_revision = state.output_revision.wrapping_add(1);
        for byte in bytes {
            if *byte == b'\r' || *byte == b'\n' {
                observe_line(&mut state, now, &self.config);
            } else {
                state.partial_line.push(*byte);
                if !self
                    .config
                    .partial_line_bytes
                    .allows(state.partial_line.len())
                {
                    observe_line(&mut state, now, &self.config);
                }
            }
        }
    }

    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    pub(crate) fn output_revision(&self) -> u64 {
        self.lock_state().output_revision
    }

    /// Returns the configured quiet period.
    pub fn timeout(&self) -> Duration {
        self.config.stall_timeout
    }

    /// Returns the optional maximum Session runtime.
    pub fn max_runtime(&self) -> Option<Duration> {
        self.config.max_runtime
    }

    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    pub(crate) fn loop_timeout(&self) -> Duration {
        self.config.loop_timeout
    }

    /// Returns whether the Session exceeded its optional maximum runtime.
    pub fn is_max_runtime_exceeded(&self) -> bool {
        self.config.max_runtime.is_some_and(|timeout| {
            let state = self.lock_state();
            state.paused_since.is_none() && state.started.elapsed() >= timeout
        })
    }

    /// Returns whether the Session has exceeded its quiet period.
    pub fn is_stalled(&self) -> bool {
        let state = self.lock_state();
        state.paused_since.is_none() && state.last_activity.elapsed() >= self.config.stall_timeout
    }

    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    pub(crate) fn is_looping(&self) -> bool {
        let state = self.lock_state();
        state.paused_since.is_none()
            && state.lines_since_novel >= self.config.loop_min_lines
            && state.last_novel.elapsed() >= self.config.loop_timeout
    }

    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    pub(crate) fn is_waiting_at_prompt(&self) -> bool {
        let state = self.lock_state();
        state.paused_since.is_none()
            && looks_like_prompt(&state.partial_line, self.config.csi_bytes)
            && state.last_activity.elapsed() >= self.config.blocked_after
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ActivityState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn observe_line(state: &mut ActivityState, now: Instant, config: &WatchdogConfig) {
    let visible =
        crate::directorates::q::terminal::strip_controls(&state.partial_line, config.csi_bytes);
    state.partial_line.clear();
    let mut normalized = Vec::with_capacity(visible.len());
    let mut digit_run = false;
    for byte in &visible {
        if config.normalize_digits && byte.is_ascii_digit() {
            if !digit_run {
                normalized.push(b'#');
                digit_run = true;
            }
        } else {
            normalized.push(*byte);
            digit_run = false;
        }
    }
    let line = trim_ascii(&normalized);
    if line.is_empty() || config.history == 0 {
        return;
    }
    let digest: [u8; 32] = Sha256::digest(line).into();
    if state.seen_lines.get(&digest).is_some() {
        state.lines_since_novel = state.lines_since_novel.saturating_add(1);
        return;
    }
    state.seen_lines.put(digest, ());
    state.last_novel = now;
    state.lines_since_novel = 0;
}

#[cfg_attr(
    not(feature = "daemon"),
    allow(dead_code, reason = "used only by the daemon")
)]
fn looks_like_prompt(raw: &[u8], csi_bytes: crate::bounds::Limit) -> bool {
    let visible = crate::directorates::q::terminal::strip_controls(raw, csi_bytes);
    let tail = visible
        .get(visible.len().saturating_sub(256)..)
        .unwrap_or(&visible);
    let tail = trim_ascii_end(tail);
    if tail.is_empty() {
        return false;
    }
    let lower = tail.iter().map(u8::to_ascii_lowercase).collect::<Vec<_>>();
    lower
        .windows(b"[y/n]".len())
        .any(|window| window == b"[y/n]")
        || lower
            .windows(b"(y/n)".len())
            .any(|window| window == b"(y/n)")
        || lower
            .windows(b"password".len())
            .any(|window| window == b"password")
        || tail
            .last()
            .is_some_and(|byte| matches!(byte, b'?' | b':' | b'>' | b'$' | b'#' | b']' | b')'))
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    trim_ascii_end(trim_ascii_start(bytes))
}

fn trim_ascii_start(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn trim_ascii_end(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |index| index + 1);
    &bytes[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_configuration_and_monitor_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WatchdogConfig>();
        assert_send_sync::<WatchdogOverrides>();
        assert_send_sync::<Watchdog>();
    }

    #[test]
    fn agent_watchdog_overrides_layer_over_daemon_defaults() {
        let limits: WatchdogOverrides = toml::from_str(
            "stall_timeout = 12.5\nloop_min_lines = 3\nmax_runtime = 60\nnormalize_digits = true\n",
        )
        .unwrap();
        let base = WatchdogConfig::default()
            .with_loop_timeout(Duration::from_secs(90))
            .with_history(128);

        let config = limits.apply_to(&base);

        assert_eq!(config.stall_timeout(), Duration::from_millis(12_500));
        assert_eq!(config.loop_timeout(), Duration::from_secs(90));
        assert_eq!(config.loop_min_lines(), 3);
        assert_eq!(config.max_runtime(), Some(Duration::from_secs(60)));
        assert_eq!(config.history(), 128);
        assert!(config.normalize_digits());
    }

    #[test]
    fn agent_watchdog_overrides_reject_invalid_values_and_unknown_fields() {
        assert!(toml::from_str::<WatchdogOverrides>("stall_timeout = -1.0").is_err());
        assert!(toml::from_str::<WatchdogOverrides>("stall_timeot = 1").is_err());
    }

    #[test]
    fn activity_resets_the_inactivity_deadline() {
        let monitor = Watchdog::new(Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(15));
        assert!(monitor.is_stalled());

        monitor.record_activity();
        assert!(!monitor.is_stalled());
    }

    #[test]
    fn a_paused_session_is_not_judged_and_resumes_with_a_fresh_deadline() {
        let monitor = Watchdog::with_config(
            WatchdogConfig::default()
                .with_stall_timeout(Duration::from_millis(10))
                .with_max_runtime(Some(Duration::from_millis(40))),
        );
        monitor.pause();
        monitor.pause();
        std::thread::sleep(Duration::from_millis(50));
        assert!(monitor.is_paused());
        assert!(!monitor.is_stalled());
        assert!(!monitor.is_max_runtime_exceeded());

        monitor.resume();
        assert!(!monitor.is_paused());
        assert!(!monitor.is_stalled());
        assert!(
            !monitor.is_max_runtime_exceeded(),
            "the time spent paused does not count"
        );
        std::thread::sleep(Duration::from_millis(15));
        assert!(monitor.is_stalled());
    }

    #[test]
    fn cloned_monitor_observes_activity_from_the_other_owner() {
        let monitor = Watchdog::new(Duration::from_millis(10));
        let operator = monitor.clone();
        std::thread::sleep(Duration::from_millis(15));
        assert!(monitor.is_stalled());

        operator.record_activity();

        assert!(!monitor.is_stalled());
    }

    #[test]
    fn ansi_color_and_carriage_return_do_not_hide_a_repeated_line() {
        let monitor = Watchdog::with_limits(
            Duration::from_secs(1),
            Duration::from_millis(20),
            Duration::from_millis(20),
            5,
        );
        for _ in 0..6 {
            monitor.record_terminal_output(b"\x1b[31mretrying\x1b[0m\r");
        }
        std::thread::sleep(Duration::from_millis(25));

        assert!(monitor.is_looping());
        assert!(!monitor.is_stalled());
    }

    #[test]
    fn novel_terminal_lines_reset_the_loop_counter() {
        let monitor = Watchdog::with_limits(
            Duration::from_secs(1),
            Duration::from_millis(20),
            Duration::from_millis(20),
            5,
        );
        for line in [b"step one\n".as_slice(), b"step two\n", b"step three\n"] {
            monitor.record_terminal_output(line);
        }
        std::thread::sleep(Duration::from_millis(25));

        assert!(!monitor.is_looping());
    }

    #[test]
    fn prompt_tail_blocks_after_quiet_period_and_operator_input_resets_it() {
        let monitor = Watchdog::with_limits(
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_millis(20),
            5,
        );
        monitor.record_terminal_output(b"Continue? [y/N] ");
        std::thread::sleep(Duration::from_millis(25));
        assert!(monitor.is_waiting_at_prompt());

        monitor.record_operator_input();

        assert!(!monitor.is_waiting_at_prompt());
        assert!(!monitor.is_stalled());
    }

    #[test]
    fn digit_normalization_groups_counter_output_as_a_repeated_line() {
        let monitor = Watchdog::with_config(
            WatchdogConfig::default()
                .with_loop_min_lines(1)
                .with_loop_timeout(Duration::ZERO)
                .with_normalize_digits(true),
        );
        monitor.record_terminal_output(b"retry 1\nretry 2\n");

        assert!(monitor.is_looping());
    }

    #[test]
    fn line_history_limit_retains_recently_repeated_lines() {
        let monitor = Watchdog::with_config(
            WatchdogConfig::default()
                .with_history(2)
                .with_loop_min_lines(1)
                .with_loop_timeout(Duration::ZERO),
        );
        monitor.record_terminal_output(b"one\ntwo\none\n");
        assert!(monitor.is_looping());

        monitor.record_terminal_output(b"three\none\n");

        assert!(monitor.is_looping());
    }

    #[test]
    fn optional_maximum_runtime_survives_activity() {
        let monitor = Watchdog::with_config(
            WatchdogConfig::default().with_max_runtime(Some(Duration::from_millis(10))),
        );
        std::thread::sleep(Duration::from_millis(15));
        monitor.record_activity();

        assert!(monitor.is_max_runtime_exceeded());
    }
}
