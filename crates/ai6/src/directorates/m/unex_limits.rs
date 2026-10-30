//! The bounds and pace of Universal Exports that the `unex` record sets, one table per area:
//! `[gateway]`, `[controls]`, `[timeline]`, `[login]` and `[text]`. Each default is the value
//! applied when the record says nothing, and the record applies at restart.
//!
//! Counts and sizes are [`Limit`]s and ceilings on durations are [`TimeLimit`]s: `null` (or
//! `"none"` in TOML) takes the bound away. Other durations are [`Seconds`], whole or
//! fractional.

use serde::{Deserialize, Serialize};

use crate::bounds::{Limit, Seconds, TimeLimit};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;
const DAY: u64 = 24 * 60 * 60;

/// Every table of the `unex` record that bounds or paces the gateway.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UnexLimits {
    /// `[gateway]`: live Sessions, connections and frames.
    pub gateway: UnexGatewayLimits,
    /// `[controls]`: the operator controls waiting for a Station.
    pub controls: ControlLimits,
    /// `[timeline]`: a Mission's timeline pages.
    pub timeline: UnexTimelineLimits,
    /// `[login]`: how password and key tries are told apart and counted.
    pub login: LoginTable,
    /// `[text]`: the longest single request fields.
    pub text: TextLimits,
}

/// `[gateway]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UnexGatewayLimits {
    /// Live provider Sessions. Default 16.
    pub sessions: Limit,
    /// Simultaneous browser connections to one provider Session. Default 12.
    pub clients_per_session: Limit,
    /// Value frames kept per provider Session for a client that joins late. Default 64.
    pub scrollback_events: Limit,
    /// Bytes of one WebSocket or ingress frame. Default 256 KiB.
    pub frame_bytes: Limit,
    /// Browser input frames accepted per connection per second. Default 40.
    pub input_frames_per_second: Limit,
    /// Capacity of the queue between the gateway and its Stations, at least 1. Default 64.
    pub input_queue: u64,
    /// Capacity of each per-Session viewer queue, at least 1. Default 8.
    pub fanout: u64,
    /// Bytes the gateway may hold in buffers across every Session and connection; the
    /// configuration is refused when its limits could exceed it. Default 512 MiB.
    pub buffer_budget_bytes: Limit,
    /// Browser connections the gateway keeps slots for; the configuration is refused when its
    /// Sessions times connections per Session could exceed it. Default 100,000.
    pub client_slots: Limit,
    /// Seconds an attach token lives, more than 0. Default 300.
    pub attach_token_ttl: Seconds,
    /// Seconds a terminal write lease lives, more than 0. Default 30.
    pub write_lease_ttl: Seconds,
    /// Characters the Station token holds at least; a smaller value is looser. Default 32.
    pub station_token_min_bytes: u64,
    /// Bytes of the public origin. Default 2,048.
    pub origin_bytes: Limit,
}

impl Default for UnexGatewayLimits {
    fn default() -> Self {
        Self {
            sessions: Limit::at(16),
            clients_per_session: Limit::at(12),
            scrollback_events: Limit::at(64),
            frame_bytes: Limit::at(256 * KIB),
            input_frames_per_second: Limit::at(40),
            input_queue: 64,
            fanout: 8,
            buffer_budget_bytes: Limit::at(512 * MIB),
            client_slots: Limit::at(100_000),
            attach_token_ttl: Seconds::new(300),
            write_lease_ttl: Seconds::new(30),
            station_token_min_bytes: 32,
            origin_bytes: Limit::at(2048),
        }
    }
}

/// `[controls]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControlLimits {
    /// Controls held at once; past it the oldest closed one goes, or the control is refused.
    /// Default 1,024.
    pub controls: Limit,
    /// Controls one Station may have open. Default 64.
    pub pending_per_station: Limit,
    /// Seconds an open control waits for its Station before it expires. Default 120.
    pub ttl: Seconds,
    /// Seconds a Station's poll waits for a control when it names no wait. Default 1.
    pub poll_wait: Seconds,
    /// The longest wait a Station's poll may ask for, in seconds. Default 25.
    pub max_poll_wait: TimeLimit,
    /// Bytes of an idempotency key. Default 128.
    pub idempotency_key_bytes: Limit,
    /// Characters of an operator's reason; longer text is cut. Default 512.
    pub reason_chars: Limit,
}

impl Default for ControlLimits {
    fn default() -> Self {
        Self {
            controls: Limit::at(1_024),
            pending_per_station: Limit::at(64),
            ttl: Seconds::new(120),
            poll_wait: Seconds::new(1),
            max_poll_wait: TimeLimit::secs(25),
            idempotency_key_bytes: Limit::at(128),
            reason_chars: Limit::at(512),
        }
    }
}

/// `[timeline]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UnexTimelineLimits {
    /// Items in a page when the request names no limit, at least 1. Default 50.
    pub page: u64,
    /// The most items a page may ask for. Default 200.
    pub max_page: Limit,
    /// Characters of one item's text; longer text is cut. Default 256.
    pub text_chars: Limit,
}

impl Default for UnexTimelineLimits {
    fn default() -> Self {
        Self {
            page: 50,
            max_page: Limit::at(200),
            text_chars: Limit::at(256),
        }
    }
}

/// `[login]`. The tries allowed are the flat `login_tries_per_address` and
/// `login_tries_per_name`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoginTable {
    /// Bytes of an account name the tries are counted under; longer names are cut. Default 64.
    pub name_bytes: Limit,
    /// Tries between sweeps that forget old counters, at least 1. Default 256.
    pub forget_every: u64,
    /// Seconds one client address's tries are counted over. Default 60.
    pub address_window: Seconds,
    /// Seconds one account name's tries are counted over. Default 600.
    pub name_window: Seconds,
}

impl Default for LoginTable {
    fn default() -> Self {
        Self {
            name_bytes: Limit::at(64),
            forget_every: 256,
            address_window: Seconds::new(60),
            name_window: Seconds::new(600),
        }
    }
}

/// `[text]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextLimits {
    /// Characters of the reason that lowers an Agent's clearance. Default 512.
    pub agent_reason_chars: Limit,
    /// The longest a green ink grant or a clearance may last, in seconds. Default one year.
    pub green_ink_max_grant: TimeLimit,
    /// Characters of a green ink note. Default 1,024.
    pub green_ink_note_chars: Limit,
    /// Characters of the reason for a held brief's decision. Default 1,024.
    pub held_reason_chars: Limit,
    /// Bytes of a roster record's target line; longer text is cut. Default 512.
    pub roster_target_bytes: Limit,
    /// Bytes of a roster record's key. Default 128.
    pub roster_key_bytes: Limit,
}

impl Default for TextLimits {
    fn default() -> Self {
        Self {
            agent_reason_chars: Limit::at(512),
            green_ink_max_grant: TimeLimit::secs(365 * DAY),
            green_ink_note_chars: Limit::at(1_024),
            held_reason_chars: Limit::at(1_024),
            roster_target_bytes: Limit::at(512),
            roster_key_bytes: Limit::at(128),
        }
    }
}

/// The keys of each table that pace work, not bound it: they are not weakenings.
const PACE: &[&str] = &[
    "poll_wait",
    "page",
    "forget_every",
    "address_window",
    "name_window",
    "attach_token_ttl",
    "write_lease_ttl",
    "input_queue",
    "fanout",
];

/// One line per bound of `current` looser than its default, its pace keys left out.
fn looser<T: Serialize + Default>(prefix: &str, current: &T) -> Vec<String> {
    let (Ok(mut current), Ok(default)) = (
        serde_json::to_value(current),
        serde_json::to_value(T::default()),
    ) else {
        return Vec::new();
    };
    if let Some(table) = current.as_object_mut() {
        table.retain(|key, _| !PACE.contains(&key.as_str()));
    }
    crate::bounds::looser(prefix, &current, &default, &["station_token_min_bytes"])
}

/// Refuses a zero for each named [`Limit`]: nothing could pass it.
fn at_least_one(prefix: &str, limits: &[(&str, Limit)]) -> Result<(), String> {
    match limits.iter().find(|(_, limit)| limit.get() == Some(0)) {
        Some((key, _)) => Err(format!("{prefix}.{key}: at least 1")),
        None => Ok(()),
    }
}

impl UnexLimits {
    /// Checks what cannot work: a zero where nothing could pass or fit.
    ///
    /// # Errors
    /// Names the first key that is not acceptable.
    pub fn validate(&self) -> Result<(), String> {
        let gateway = &self.gateway;
        at_least_one(
            "gateway",
            &[
                ("sessions", gateway.sessions),
                ("clients_per_session", gateway.clients_per_session),
                ("scrollback_events", gateway.scrollback_events),
                ("frame_bytes", gateway.frame_bytes),
                ("input_frames_per_second", gateway.input_frames_per_second),
            ],
        )?;
        for (key, value) in [
            ("input_queue", gateway.input_queue),
            ("fanout", gateway.fanout),
        ] {
            if value == 0 {
                return Err(format!("gateway.{key}: at least 1"));
            }
        }
        for (key, value) in [
            ("attach_token_ttl", gateway.attach_token_ttl),
            ("write_lease_ttl", gateway.write_lease_ttl),
        ] {
            if value.0.is_zero() {
                return Err(format!("gateway.{key}: more than 0"));
            }
        }
        if self.timeline.page == 0 {
            return Err("timeline.page: at least 1".to_owned());
        }
        at_least_one("timeline", &[("max_page", self.timeline.max_page)])?;
        if self.login.forget_every == 0 {
            return Err("login.forget_every: at least 1".to_owned());
        }
        Ok(())
    }

    /// One line per bound looser than its default; the keys that pace the gateway are left
    /// out. The Station token's smallest size is looser the smaller it is.
    pub fn weakenings(&self) -> Vec<String> {
        let mut found = looser("unex.gateway", &self.gateway);
        found.extend(looser("unex.controls", &self.controls));
        found.extend(looser("unex.timeline", &self.timeline));
        found.extend(looser("unex.login", &self.login));
        found.extend(looser("unex.text", &self.text));
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_weakening_and_a_looser_bound_is_one() {
        assert!(UnexLimits::default().weakenings().is_empty());
        let mut limits = UnexLimits::default();
        limits.gateway.frame_bytes = Limit::UNBOUNDED;
        limits.gateway.station_token_min_bytes = 8;
        limits.controls.pending_per_station = Limit::at(128);
        limits.controls.poll_wait = Seconds::new(9);
        let found = limits.weakenings();
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(
            found
                .iter()
                .any(|line| line.starts_with("unex.gateway.frame_bytes none"))
        );
        assert!(
            found
                .iter()
                .any(|line| line.contains("station_token_min_bytes 8"))
        );
        assert!(
            found
                .iter()
                .any(|line| line.contains("pending_per_station 128"))
        );
    }

    #[test]
    fn tables_read_numbers_none_and_refuse_nonsense() {
        let limits: UnexLimits = toml::from_str(
            "[gateway]\nframe_bytes = \"none\"\nattach_token_ttl = 600\n[controls]\npending_per_station = 1\n",
        )
        .unwrap();
        assert_eq!(limits.gateway.frame_bytes, Limit::UNBOUNDED);
        assert_eq!(limits.gateway.attach_token_ttl, Seconds::new(600));
        assert_eq!(limits.controls.pending_per_station, Limit::at(1));
        assert!(limits.validate().is_ok());
        assert!(toml::from_str::<UnexLimits>("[gateway]\nno_such = 1\n").is_err());
        let mut zero = UnexLimits::default();
        zero.gateway.sessions = Limit::at(0);
        assert_eq!(
            zero.validate(),
            Err("gateway.sessions: at least 1".to_owned())
        );
    }
}
