//! The bounds and pace of Universal Exports that the `unex` record sets, one table per area:
//! `[gateway]`, `[controls]`, `[timeline]`, `[login]` and `[text]`. Each default is the value
//! applied when the record says nothing, and the record applies at restart.
//!
//! Counts and sizes are [`Limit`]s and ceilings on durations are [`TimeLimit`]s: `null` (or
//! `"none"` in TOML) takes the bound away. Other durations are [`Seconds`], whole or
//! fractional.

use guatiao::{FromValue, Schema, ToValue};

use crate::bounds::{Limit, Seconds, TimeLimit};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;
const DAY: u64 = 24 * 60 * 60;

/// Every table of the `unex` record that bounds or paces the gateway.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct UnexGatewayLimits {
    /// Live provider Sessions.
    #[schema(title = "Live Sessions")]
    #[map(default = UnexGatewayLimits::default().sessions)]
    pub sessions: Limit,
    /// Simultaneous browser connections to one provider Session.
    #[schema(title = "Browser connections per Session")]
    #[map(default = UnexGatewayLimits::default().clients_per_session)]
    pub clients_per_session: Limit,
    /// Value frames kept per provider Session for a client that joins late.
    #[schema(title = "Frames kept for a late client")]
    #[map(default = UnexGatewayLimits::default().scrollback_events)]
    pub scrollback_events: Limit,
    /// Bytes of one WebSocket or ingress frame. A Station is told at registration how much terminal
    /// output that leaves room for, and splits longer output to fit.
    #[schema(title = "Largest frame", format = "bytes")]
    #[map(default = UnexGatewayLimits::default().frame_bytes)]
    pub frame_bytes: Limit,
    /// Browser input frames accepted per connection per second.
    #[schema(title = "Browser input frames a second")]
    #[map(default = UnexGatewayLimits::default().input_frames_per_second)]
    pub input_frames_per_second: Limit,
    /// Capacity of the queue between the gateway and its Stations, at least 1.
    #[schema(title = "Queue to the Stations")]
    #[map(default = UnexGatewayLimits::default().input_queue)]
    pub input_queue: u64,
    /// Capacity of each per-Session viewer queue, at least 1.
    #[schema(title = "Queue per Session viewer")]
    #[map(default = UnexGatewayLimits::default().fanout)]
    pub fanout: u64,
    /// Bytes the gateway may hold in buffers across every Session and connection; the configuration
    /// is refused when its limits could exceed it.
    #[schema(title = "All the gateway's buffers", format = "bytes")]
    #[map(default = UnexGatewayLimits::default().buffer_budget_bytes)]
    pub buffer_budget_bytes: Limit,
    /// Browser connections the gateway keeps slots for; the configuration is refused when its
    /// Sessions times connections per Session could exceed it.
    #[schema(title = "Browser connection slots")]
    #[map(default = UnexGatewayLimits::default().client_slots)]
    pub client_slots: Limit,
    /// Seconds an attach token lives, more than 0.
    #[schema(title = "An attach token lasts")]
    #[map(default = UnexGatewayLimits::default().attach_token_ttl)]
    pub attach_token_ttl: Seconds,
    /// Seconds a terminal write lease lives, more than 0.
    #[schema(title = "A terminal write lease lasts")]
    #[map(default = UnexGatewayLimits::default().write_lease_ttl)]
    pub write_lease_ttl: Seconds,
    /// Characters the Station token holds at least; a smaller value is looser.
    #[schema(title = "Shortest Station token", format = "bytes")]
    #[map(default = UnexGatewayLimits::default().station_token_min_bytes)]
    pub station_token_min_bytes: u64,
    /// Bytes of the public origin.
    #[schema(title = "Longest public origin", format = "bytes")]
    #[map(default = UnexGatewayLimits::default().origin_bytes)]
    pub origin_bytes: Limit,
    /// Seconds a sign-in that names no lifetime gets, and each renewal gives from then, more than 0
    /// and within `access_token_max_ttl`; longer is looser.
    #[schema(title = "A sign-in lasts")]
    #[map(default = UnexGatewayLimits::default().access_token_ttl)]
    pub access_token_ttl: Seconds,
    /// The longest lifetime a request for a token may name, in seconds.
    #[schema(title = "Longest token a request may ask for")]
    #[map(default = UnexGatewayLimits::default().access_token_max_ttl)]
    pub access_token_max_ttl: TimeLimit,
    /// How long renewing keeps a sign-in going, in seconds from when it was made; the password is not
    /// asked again until then. `0` turns renewal off, `"none"` lets it run on.
    #[schema(title = "Longest renewed sign-in")]
    #[map(default = UnexGatewayLimits::default().session_max)]
    pub session_max: TimeLimit,
    /// Whether a browser's sign-in also gets an HttpOnly cookie, so a reload signs it back in up
    /// to `session_max` after signing in; with `false`, or `session_max` at `0`, a sign-in lives
    /// in the app's memory alone.
    #[schema(title = "Keep a browser's sign-in in a cookie")]
    #[map(default = UnexGatewayLimits::default().session_cookie)]
    pub session_cookie: bool,
    /// Seconds between audit records of one person's reads of one resource allowed only by one
    /// compartment; the app polls, so `0` writes one for every read.
    #[schema(title = "Between audit records of a compartment read")]
    #[map(default = UnexGatewayLimits::default().compartment_audit)]
    pub compartment_audit: Seconds,
    /// Bytes the write buffer of a browser's socket holds at least, whatever `frame_bytes`
    /// (it holds twice that when that is more).
    #[schema(title = "Smallest socket write buffer", format = "bytes")]
    #[map(default = UnexGatewayLimits::default().socket_buffer_bytes)]
    pub socket_buffer_bytes: Limit,
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
            access_token_ttl: Seconds::new(3_600),
            access_token_max_ttl: TimeLimit::secs(DAY),
            session_max: TimeLimit::secs(43_200),
            session_cookie: true,
            compartment_audit: Seconds::new(60),
            socket_buffer_bytes: Limit::at(256 * KIB),
        }
    }
}

/// `[controls]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ControlLimits {
    /// Controls held for status reads and replays; past it the oldest closed one goes, or the control
    /// is refused.
    #[schema(title = "Controls held")]
    #[map(default = ControlLimits::default().controls)]
    pub controls: Limit,
    /// Controls one Station may have open.
    #[schema(title = "Open controls per Station")]
    #[map(default = ControlLimits::default().pending_per_station)]
    pub pending_per_station: Limit,
    /// Seconds an open control waits for its Station before it expires.
    #[schema(title = "An open control waits")]
    #[map(default = ControlLimits::default().ttl)]
    pub ttl: Seconds,
    /// Seconds a Station's poll waits for a control when it names no wait.
    #[schema(title = "A Station's poll waits")]
    #[map(default = ControlLimits::default().poll_wait)]
    pub poll_wait: Seconds,
    /// The longest wait a Station's poll may ask for, in seconds.
    #[schema(title = "Longest poll a Station may ask for")]
    #[map(default = ControlLimits::default().max_poll_wait)]
    pub max_poll_wait: TimeLimit,
    /// Bytes of an idempotency key.
    #[schema(title = "Longest idempotency key", format = "bytes")]
    #[map(default = ControlLimits::default().idempotency_key_bytes)]
    pub idempotency_key_bytes: Limit,
    /// Characters of an operator's reason; longer text is cut.
    #[schema(title = "Longest operator reason")]
    #[map(default = ControlLimits::default().reason_chars)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct UnexTimelineLimits {
    /// Items in a page when the request names no limit, at least 1.
    #[schema(title = "Items in a page")]
    #[map(default = UnexTimelineLimits::default().page)]
    pub page: u64,
    /// The most items a page may ask for.
    #[schema(title = "Most items a page may ask for")]
    #[map(default = UnexTimelineLimits::default().max_page)]
    pub max_page: Limit,
    /// Characters of one item's text; longer text is cut.
    #[schema(title = "Longest item text")]
    #[map(default = UnexTimelineLimits::default().text_chars)]
    pub text_chars: Limit,
    /// Records in a page of the audit log when the request names no limit, at least 1.
    #[schema(title = "Audit records in a page")]
    #[map(default = UnexTimelineLimits::default().audit_page)]
    pub audit_page: u64,
    /// The most records a page of the audit log may ask for.
    #[schema(title = "Most audit records a page may ask for")]
    #[map(default = UnexTimelineLimits::default().max_audit_page)]
    pub max_audit_page: Limit,
}

impl Default for UnexTimelineLimits {
    fn default() -> Self {
        Self {
            page: 50,
            max_page: Limit::at(200),
            text_chars: Limit::at(256),
            audit_page: 100,
            max_audit_page: Limit::at(500),
        }
    }
}

/// `[login]`. The tries allowed are the flat `login_tries_per_address` and
/// `login_tries_per_name`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct LoginTable {
    /// Bytes of an account name the tries are counted under; longer names are cut.
    #[schema(title = "Longest account name counted", format = "bytes")]
    #[map(default = LoginTable::default().name_bytes)]
    pub name_bytes: Limit,
    /// Tries between sweeps that forget old counters, at least 1.
    #[schema(title = "Tries between sweeps of old counters")]
    #[map(default = LoginTable::default().forget_every)]
    pub forget_every: u64,
    /// Seconds one client address's tries are counted over.
    #[schema(title = "One address's tries are counted over")]
    #[map(default = LoginTable::default().address_window)]
    pub address_window: Seconds,
    /// Seconds one account name's tries are counted over.
    #[schema(title = "One account name's tries are counted over")]
    #[map(default = LoginTable::default().name_window)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct TextLimits {
    /// Characters of the reason that lowers an Agent's clearance.
    #[schema(title = "Longest reason for lowering a clearance")]
    #[map(default = TextLimits::default().agent_reason_chars)]
    pub agent_reason_chars: Limit,
    /// The longest a green ink grant or a clearance may last, in seconds.
    #[schema(title = "Longest green ink grant or clearance")]
    #[map(default = TextLimits::default().green_ink_max_grant)]
    pub green_ink_max_grant: TimeLimit,
    /// Characters of a green ink note.
    #[schema(title = "Longest green ink note")]
    #[map(default = TextLimits::default().green_ink_note_chars)]
    pub green_ink_note_chars: Limit,
    /// How far back the list of expired green ink requests reaches when it is not told.
    #[schema(title = "Expired green ink listed by default")]
    #[map(default = TextLimits::default().green_ink_expired_window)]
    pub green_ink_expired_window: TimeLimit,
    /// Characters of the reason for a held brief's decision.
    #[schema(title = "Longest reason for a held brief's decision")]
    #[map(default = TextLimits::default().held_reason_chars)]
    pub held_reason_chars: Limit,
    /// Bytes of a roster record's target line; longer text is cut.
    #[schema(title = "Longest roster target line", format = "bytes")]
    #[map(default = TextLimits::default().roster_target_bytes)]
    pub roster_target_bytes: Limit,
    /// Bytes of a roster record's key.
    #[schema(title = "Longest roster key", format = "bytes")]
    #[map(default = TextLimits::default().roster_key_bytes)]
    pub roster_key_bytes: Limit,
    /// Bytes of a role name, an Operation ID or an account name in the access and account routes.
    #[schema(title = "Longest role, Operation or account name", format = "bytes")]
    #[map(default = TextLimits::default().access_name_bytes)]
    pub access_name_bytes: Limit,
    /// Characters of the title of a Mission a person starts.
    #[schema(title = "Longest Mission title")]
    #[map(default = TextLimits::default().mission_title_chars)]
    pub mission_title_chars: Limit,
    /// Bytes of the instructions of a Mission a person starts.
    #[schema(title = "Longest Mission instructions", format = "bytes")]
    #[map(default = TextLimits::default().mission_prompt_bytes)]
    pub mission_prompt_bytes: Limit,
}

impl Default for TextLimits {
    fn default() -> Self {
        Self {
            agent_reason_chars: Limit::at(512),
            green_ink_max_grant: TimeLimit::secs(365 * DAY),
            green_ink_note_chars: Limit::at(1_024),
            green_ink_expired_window: TimeLimit::secs(30 * DAY),
            held_reason_chars: Limit::at(1_024),
            roster_target_bytes: Limit::at(512),
            roster_key_bytes: Limit::at(128),
            access_name_bytes: Limit::at(128),
            mission_title_chars: Limit::at(512),
            mission_prompt_bytes: Limit::at(64 * KIB),
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
    "socket_buffer_bytes",
    "audit_page",
];

/// One line per bound of `current` looser than its default, its pace keys left out.
fn looser<T: ToValue + Default>(prefix: &str, current: &T) -> Vec<String> {
    let (Some(mut current), Some(default)) = (
        crate::directorates::m::records::to_json(current),
        crate::directorates::m::records::to_json(&T::default()),
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
            ("access_token_ttl", gateway.access_token_ttl),
        ] {
            if value.0.is_zero() {
                return Err(format!("gateway.{key}: more than 0"));
            }
        }
        // A sign-in naming no lifetime gets `access_token_ttl`, which the bound would refuse.
        if !gateway
            .access_token_max_ttl
            .allows(gateway.access_token_ttl.0)
        {
            return Err("gateway.access_token_ttl: at most access_token_max_ttl".to_owned());
        }
        if self.timeline.page == 0 {
            return Err("timeline.page: at least 1".to_owned());
        }
        if self.timeline.audit_page == 0 {
            return Err("timeline.audit_page: at least 1".to_owned());
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
    use crate::directorates::m::config::UnexRecord;

    #[test]
    fn defaults_have_no_weakening_and_a_looser_bound_is_one() {
        assert!(UnexLimits::default().weakenings().is_empty());
        let mut limits = UnexLimits::default();
        limits.gateway.frame_bytes = Limit::UNBOUNDED;
        limits.gateway.station_token_min_bytes = 8;
        limits.controls.pending_per_station = Limit::at(128);
        limits.controls.poll_wait = Seconds::new(9);
        limits.gateway.session_max = TimeLimit::secs(86_400);
        limits.gateway.access_token_ttl = Seconds::new(7_200);
        let found = limits.weakenings();
        assert_eq!(found.len(), 5, "{found:?}");
        assert!(
            found
                .iter()
                .any(|line| line.starts_with("unex.gateway.session_max 86400")),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|line| line.starts_with("unex.gateway.access_token_ttl 7200")),
            "{found:?}"
        );
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
        let read = crate::directorates::m::records::from_toml::<UnexRecord>;
        let limits = read(
            "[gateway]\nframe_bytes = \"none\"\nattach_token_ttl = 600\n[controls]\npending_per_station = 1\n",
        )
        .unwrap()
        .limits();
        assert_eq!(limits.gateway.frame_bytes, Limit::UNBOUNDED);
        assert_eq!(limits.gateway.attach_token_ttl, Seconds::new(600));
        assert_eq!(limits.controls.pending_per_station, Limit::at(1));
        assert!(limits.validate().is_ok());
        assert!(read("[gateway]\nno_such = 1\n").is_err());
        let mut zero = UnexLimits::default();
        zero.gateway.sessions = Limit::at(0);
        assert_eq!(
            zero.validate(),
            Err("gateway.sessions: at least 1".to_owned())
        );
        let mut token = UnexLimits::default();
        token.gateway.access_token_ttl = Seconds::new(0);
        assert_eq!(
            token.validate(),
            Err("gateway.access_token_ttl: more than 0".to_owned())
        );
        token.gateway.access_token_ttl = Seconds::new(7_200);
        token.gateway.access_token_max_ttl = TimeLimit::secs(3_600);
        assert_eq!(
            token.validate(),
            Err("gateway.access_token_ttl: at most access_token_max_ttl".to_owned())
        );
        let mut off = UnexLimits::default();
        off.gateway.session_max = TimeLimit::secs(0);
        assert!(off.validate().is_ok(), "no renewal is a choice");
        assert!(off.weakenings().is_empty(), "and a stricter one");
        let table = read("[gateway]\nsession_cookie = false\n")
            .unwrap()
            .limits();
        assert!(!table.gateway.session_cookie);
        assert!(UnexLimits::default().gateway.session_cookie);
        assert!(table.weakenings().is_empty(), "memory alone is stricter");
    }
}
