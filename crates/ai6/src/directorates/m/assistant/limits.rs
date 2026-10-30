//! The bounds and pace of Moneypenny: `[assistant.limits]`, what a job, a report and her
//! memory may hold, which a Station and Universal Exports both check; `[assistant.desk]`, how a
//! Station's desk runs her harnesses; and `[assistant.gateway]`, how Universal Exports keeps
//! her conversations. Each default is the value applied when the record says nothing.
//!
//! Counts and sizes are [`Limit`]s and ceilings on durations are [`TimeLimit`]s: `null` (or
//! `"none"` in TOML) takes the bound away. Other durations are [`Seconds`], whole or
//! fractional.

use serde::{Deserialize, Serialize};

use crate::bounds::{Limit, Seconds, TimeLimit};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

/// `[assistant.limits]`: the bounds of what passes between Universal Exports and a Station
/// for Moneypenny, and of her memory on a Station.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantLimits {
    /// Operations one job grants Cable actions in. Default 64.
    pub cable_grants: Limit,
    /// Bytes of one job's message. Default 32 KiB.
    pub message_bytes: Limit,
    /// Bytes of one job's standing context and recap together. Default 24 KiB.
    pub context_bytes: Limit,
    /// Bytes of one reported event's text; longer text is cut. Default 64 KiB.
    pub event_bytes: Limit,
    /// Events in one report. Default 64.
    pub events: Limit,
    /// Bytes of file content in one memory change, commit or memory write. Default 64 KiB.
    pub change_bytes: Limit,
    /// Files in one memory change or commit. Default 32.
    pub change_files: Limit,
    /// Bytes of one commit's message. Default 4,096.
    pub commit_message_bytes: Limit,
    /// Shared-change bases one event carries, and Universal Exports keeps for one
    /// conversation until their calls arrive. Default 32.
    pub shared_bases: Limit,
    /// Files one memory listing names. Default 1,000.
    pub memory_list_files: Limit,
    /// Bytes of paths one memory listing names. Default 128 KiB.
    pub memory_list_bytes: Limit,
    /// Skills one job gives her. Default 64.
    pub skills: Limit,
    /// Bytes of `MEMORY.md` inlined into her standing context. Default 32 KiB.
    pub inlined_memory_bytes: Limit,
    /// Bytes of one memory file the desk reads; a longer file is read to this length.
    /// Default 256 KiB.
    pub memory_file_bytes: Limit,
    /// Memory files the desk reads at a turn's edge. Default 1,024.
    pub memory_files: Limit,
    /// Bytes over all the memory files the desk reads at a turn's edge. Default 8 MiB.
    pub memory_bytes: Limit,
    /// Files of `personal/` looked at to name what a turn changed. Default 10,000.
    pub memory_walk: Limit,
    /// How deep `@path` imports from her memory are followed to find outside text; Claude
    /// Code follows five, so a smaller value is looser. Default 5.
    pub memory_import_depth: Limit,
    /// Bytes of one file the Station reads to publish a page. Default 8 MiB.
    pub page_file_bytes: Limit,
    /// Files the Station reads to publish one page. Default 200.
    pub page_files: Limit,
    /// Bytes over all the files of one page the Station reads. Default 16 MiB.
    pub page_bytes: Limit,
}

impl Default for AssistantLimits {
    fn default() -> Self {
        Self {
            cable_grants: Limit::at(64),
            message_bytes: Limit::at(32 * KIB),
            context_bytes: Limit::at(24 * KIB),
            event_bytes: Limit::at(64 * KIB),
            events: Limit::at(64),
            change_bytes: Limit::at(64 * KIB),
            change_files: Limit::at(32),
            commit_message_bytes: Limit::at(4_096),
            shared_bases: Limit::at(32),
            memory_list_files: Limit::at(1_000),
            memory_list_bytes: Limit::at(128 * KIB),
            skills: Limit::at(64),
            inlined_memory_bytes: Limit::at(32 * KIB),
            memory_file_bytes: Limit::at(256 * KIB),
            memory_files: Limit::at(1_024),
            memory_bytes: Limit::at(8 * MIB),
            memory_walk: Limit::at(10_000),
            memory_import_depth: Limit::at(5),
            page_file_bytes: Limit::at(8 * MIB),
            page_files: Limit::at(200),
            page_bytes: Limit::at(16 * MIB),
        }
    }
}

/// `[assistant.desk]`: how a Station's desk runs Moneypenny's harnesses. Read at each harness
/// start, job and memory request; `known`, `parked` and `report_queue` at each use, and
/// `unlocked_stale` when the desk starts.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantDeskLimits {
    /// Seconds a harness waits for the next message, or writes nothing while answering,
    /// before it is closed. Default 600.
    pub idle: TimeLimit,
    /// Harnesses a Station runs at once. Default 4.
    pub live: Limit,
    /// Seconds before its token expires that a running harness is restarted with the job's.
    /// Default 120.
    pub token_margin: Seconds,
    /// Seconds Universal Exports holds a job poll open. Default 1.
    pub job_wait: Seconds,
    /// Bytes of one job as Universal Exports sends it. Default 256 KiB.
    pub job_bytes: Limit,
    /// Seconds a request to Universal Exports may take past what it waits for. Default 2.
    pub call_margin: Seconds,
    /// Seconds before a failed job poll is tried again. Default 0.5.
    pub job_retry: Seconds,
    /// Ordinary reports (her text, her tool calls) waiting to be sent; past it the newest are
    /// dropped. Default 256.
    pub report_queue: Limit,
    /// Times a report is sent before it counts as lost. Default 3.
    pub report_attempts: u32,
    /// Seconds between tries of a report that did not reach Universal Exports. Default 0.2.
    pub report_retry: Seconds,
    /// Memory changes kept on the Station because they could not be delivered; past it the
    /// oldest is dropped. Default 64.
    pub parked: Limit,
    /// Conversations whose provider Session the desk remembers for a resume. Default 1,024.
    pub known: Limit,
    /// Seconds a message waits for the conversation's closing harness. Default 10.
    pub close_wait: Seconds,
    /// Seconds removing a closed harness's directory is tried again. Default 2.
    pub remove_retry: Seconds,
    /// Seconds before a directory of working directories left without its lock, or a lock
    /// without its directory, is swept. Default 3,600.
    pub unlocked_stale: Seconds,
    /// Seconds a clone of one of her repositories may take. Default 120.
    pub clone_timeout: Seconds,
    /// Bytes a clone of one of her repositories may hold on disk. Default 512 MiB.
    pub clone_bytes: Limit,
    /// Bytes one save of her work in `personal/` may commit; past it nothing is saved.
    /// Default 16 MiB.
    pub save_bytes: Limit,
    /// The app's memory requests a Station makes at once; past it the next is refused.
    /// Default 4.
    pub memory_requests: Limit,
}

impl Default for AssistantDeskLimits {
    fn default() -> Self {
        Self {
            idle: TimeLimit::secs(600),
            live: Limit::at(4),
            token_margin: Seconds::new(120),
            job_wait: Seconds::new(1),
            job_bytes: Limit::at(256 * KIB),
            call_margin: Seconds::new(2),
            job_retry: Seconds(std::time::Duration::from_millis(500)),
            report_queue: Limit::at(256),
            report_attempts: 3,
            report_retry: Seconds(std::time::Duration::from_millis(200)),
            parked: Limit::at(64),
            known: Limit::at(1_024),
            close_wait: Seconds::new(10),
            remove_retry: Seconds::new(2),
            unlocked_stale: Seconds::new(3_600),
            clone_timeout: Seconds::new(120),
            clone_bytes: Limit::at(512 * MIB),
            save_bytes: Limit::at(16 * MIB),
            memory_requests: Limit::at(4),
        }
    }
}

/// `[assistant.gateway]`: how Universal Exports keeps Moneypenny's conversations. Universal
/// Exports reads it when it starts.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantGatewayLimits {
    /// Conversations kept for one person; the oldest goes first. Default 20.
    pub conversations_per_person: Limit,
    /// Conversations kept in all. Default 2,000.
    pub conversations: Limit,
    /// Seconds a conversation is kept untouched. Default 7 days.
    pub conversation_age: TimeLimit,
    /// Entries kept per conversation. Default 500.
    pub conversation_entries: Limit,
    /// Bytes of entries kept per conversation. Default 1 MiB.
    pub conversation_bytes: Limit,
    /// Bytes of entries kept per person. Default 4 MiB.
    pub person_bytes: Limit,
    /// One person's conversations with a harness running or a message being answered at
    /// once. Default 2.
    pub live_per_person: Limit,
    /// Tool calls one conversation makes in a minute. Default 120.
    pub tool_calls_per_minute: Limit,
    /// The app's memory requests waiting for a Station for one person. Default 2.
    pub memory_requests_per_person: Limit,
    /// The app's memory requests waiting for a Station in all. Default 16.
    pub memory_requests: Limit,
    /// Seconds a token given to a harness for its tools is accepted. Default 3,600.
    pub token_ttl: Seconds,
    /// Seconds a running harness's token must still have to be kept for the next message.
    /// Default 600.
    pub token_reuse_margin: Seconds,
    /// Seconds a Station has to take up a message before another is asked. Default 60.
    pub answer_timeout: Seconds,
    /// Seconds an accepted message may go without word from its Station. Default 600.
    pub answer_deadline: Seconds,
    /// Seconds a Station has to answer a memory request. Default 30.
    pub memory_timeout: Seconds,
    /// Seconds a proposal waits for the person's confirmation. Default 3,600.
    pub proposal_ttl: Seconds,
    /// Proposals waiting for the person in one conversation. Default 32.
    pub proposals: Limit,
    /// Bytes of one message the person writes. Default 16 KiB.
    pub message_bytes: Limit,
    /// Bytes of a commit message the person writes for a memory change. Default 1,024.
    pub memory_message_bytes: Limit,
    /// Characters of the one-line message of a shared change she proposes. Default 200.
    pub change_message_chars: Limit,
    /// Bytes of the recap a resumed conversation starts with. Default 8 KiB.
    pub recap_bytes: Limit,
    /// Entries the app is sent when it opens a conversation. Default 100.
    pub ready_entries: Limit,
    /// Bytes of entries the app is sent when it opens a conversation. Default 512 KiB.
    pub ready_bytes: Limit,
    /// Bytes of one message on the app's socket. Default 64 KiB.
    pub socket_message_bytes: Limit,
    /// Seconds the app's socket has to say hello. Default 10.
    pub hello_timeout: Seconds,
    /// Seconds a Station's job poll may be held open. Default 25.
    pub max_wait: Seconds,
    /// Seconds between sweeps of expired answers and proposals. Default 1.
    pub sweep_every: Seconds,
    /// Seconds a shared change waits for its base from the Station. Default 5.
    pub shared_base_wait: Seconds,
    /// Events queued for one conversation's open sockets; a socket that falls further behind
    /// is closed. Default 256.
    pub event_buffer: u64,
    /// Bytes of an AI6 route's answer to one of her tools. Default 4 MiB.
    pub route_bytes: Limit,
    /// Characters of one tool's answer she is given; longer is cut. Default 200,000.
    pub result_chars: Limit,
    /// Characters of a person's name and email on her commits. Default 200.
    pub author_chars: Limit,
    /// Bytes of an audit record's target. Default 512.
    pub audit_target_bytes: Limit,
    /// Sources a conversation lists as having exposed it, and a proposal carries as
    /// `exposed_by`. Default 16.
    pub exposed_sources: Limit,
    /// Bytes of one such source. Default 200.
    pub source_bytes: Limit,
    /// Read tool calls of one turn a proposal lists as what she had read. Default 16.
    pub proposal_reads: Limit,
}

impl Default for AssistantGatewayLimits {
    fn default() -> Self {
        Self {
            conversations_per_person: Limit::at(20),
            conversations: Limit::at(2_000),
            conversation_age: TimeLimit::secs(7 * 24 * 3_600),
            conversation_entries: Limit::at(500),
            conversation_bytes: Limit::at(MIB),
            person_bytes: Limit::at(4 * MIB),
            live_per_person: Limit::at(2),
            tool_calls_per_minute: Limit::at(120),
            memory_requests_per_person: Limit::at(2),
            memory_requests: Limit::at(16),
            token_ttl: Seconds::new(3_600),
            token_reuse_margin: Seconds::new(600),
            answer_timeout: Seconds::new(60),
            answer_deadline: Seconds::new(600),
            memory_timeout: Seconds::new(30),
            proposal_ttl: Seconds::new(3_600),
            proposals: Limit::at(32),
            message_bytes: Limit::at(16 * KIB),
            memory_message_bytes: Limit::at(1_024),
            change_message_chars: Limit::at(200),
            recap_bytes: Limit::at(8 * KIB),
            ready_entries: Limit::at(100),
            ready_bytes: Limit::at(512 * KIB),
            socket_message_bytes: Limit::at(64 * KIB),
            hello_timeout: Seconds::new(10),
            max_wait: Seconds::new(25),
            sweep_every: Seconds::new(1),
            shared_base_wait: Seconds::new(5),
            event_buffer: 256,
            route_bytes: Limit::at(4 * MIB),
            result_chars: Limit::at(200_000),
            author_chars: Limit::at(200),
            audit_target_bytes: Limit::at(512),
            exposed_sources: Limit::at(16),
            source_bytes: Limit::at(200),
            proposal_reads: Limit::at(16),
        }
    }
}

/// What a page may reach from the browser, `[assistant.pages] network`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageNetwork {
    /// Anything: the page loads and fetches from any host. The default.
    #[default]
    Open,
    /// Only its own files and feeds, `data:` and `blob:`.
    Own,
    /// Its own files only: no feeds, no outside request.
    None,
}

impl PageNetwork {
    /// The word as the record and the app spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Own => "own",
            Self::None => "none",
        }
    }

    /// The tighter of two profiles.
    #[must_use]
    pub fn tighter(self, other: Self) -> Self {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Open => 0,
            Self::Own => 1,
            Self::None => 2,
        }
    }
}

/// `[assistant.pages]`: the pages she publishes and how Universal Exports serves them. Pages
/// are open by default (scripts, network, feeds, the frame); an administrator may restrict
/// any of it, and raise or lower any quota, which is a plain change and never a weakening.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantPageLimits {
    /// Bytes of one file of a page. Default 8 MiB.
    pub max_file_bytes: Limit,
    /// Bytes of one page. Default 16 MiB.
    pub max_page_bytes: Limit,
    /// Files of one page. Default 200.
    pub max_page_files: Limit,
    /// Pages one person keeps. Default 50.
    pub max_pages: Limit,
    /// Bytes of all one person's pages. Default 64 MiB.
    pub max_person_bytes: Limit,
    /// Bytes of all pages on one gateway. Default 2 GiB.
    pub max_total_bytes: Limit,
    /// Bytes of an inline page's content. Default 192 KiB.
    pub inline_bytes: Limit,
    /// Characters of a page's title. Default 120.
    pub title_chars: Limit,
    /// Feeds one dashboard declares. Default 8.
    pub max_feeds: Limit,
    /// Feed reads one link makes in a minute. Default 60.
    pub feed_reads_per_minute: Limit,
    /// Pages Universal Exports keeps staged from Stations at once. Default 64.
    pub staged_uploads: Limit,
    /// Seconds a page lives unless remembered. Default 1 day.
    pub default_ttl: Seconds,
    /// Seconds a link to a page is valid. Default 1 hour.
    pub open_ttl: Seconds,
    /// Seconds a publish waits for the Station's files. Default 30.
    pub upload_wait: Seconds,
    /// Seconds a staged upload no call claims is kept. Default 300.
    pub stage_ttl: Seconds,
    /// Seconds between sweeps of expired pages, links and staged uploads. Default 60.
    pub sweep_every: Seconds,
    /// Whether a page's scripts run. Default true.
    pub scripts: bool,
    /// What a page may reach from the browser. Default `open`.
    pub network: PageNetwork,
    /// Whether dashboards may read live feeds. Default true.
    pub feeds: bool,
    /// Whether a page is shown in the gateway's frame with the classification bar. A page
    /// made after she read outside text always is. Default true.
    pub frame: bool,
    /// Whether a person may share a page. Default true.
    pub sharing: bool,
}

impl Default for AssistantPageLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: Limit::at(8 * MIB),
            max_page_bytes: Limit::at(16 * MIB),
            max_page_files: Limit::at(200),
            max_pages: Limit::at(50),
            max_person_bytes: Limit::at(64 * MIB),
            max_total_bytes: Limit::at(2 * 1024 * MIB),
            inline_bytes: Limit::at(192 * KIB),
            title_chars: Limit::at(120),
            max_feeds: Limit::at(8),
            feed_reads_per_minute: Limit::at(60),
            staged_uploads: Limit::at(64),
            default_ttl: Seconds::new(24 * 3_600),
            open_ttl: Seconds::new(3_600),
            upload_wait: Seconds::new(30),
            stage_ttl: Seconds::new(300),
            sweep_every: Seconds::new(60),
            scripts: true,
            network: PageNetwork::Open,
            feeds: true,
            frame: true,
            sharing: true,
        }
    }
}

impl AssistantPageLimits {
    /// Checks what cannot work: a lifetime, a wait or a sweep of zero, a quota of zero.
    ///
    /// # Errors
    ///
    /// Names the first key that is not acceptable.
    pub fn validate(&self) -> Result<(), String> {
        for (key, seconds) in [
            ("default_ttl", self.default_ttl),
            ("open_ttl", self.open_ttl),
            ("upload_wait", self.upload_wait),
            ("stage_ttl", self.stage_ttl),
            ("sweep_every", self.sweep_every),
        ] {
            if seconds.0.is_zero() {
                return Err(format!("assistant.pages.{key}: more than 0"));
            }
        }
        at_least_one(
            "assistant.pages",
            &[
                ("max_file_bytes", self.max_file_bytes),
                ("max_page_bytes", self.max_page_bytes),
                ("max_page_files", self.max_page_files),
                ("max_pages", self.max_pages),
                ("max_person_bytes", self.max_person_bytes),
                ("max_total_bytes", self.max_total_bytes),
                ("inline_bytes", self.inline_bytes),
                ("title_chars", self.title_chars),
                ("feed_reads_per_minute", self.feed_reads_per_minute),
                ("staged_uploads", self.staged_uploads),
            ],
        )
    }
}

/// The keys of each table that pace work rather than bound it: never a weakening.
const DESK_PACE: &[&str] = &[
    "token_margin",
    "job_wait",
    "call_margin",
    "job_retry",
    "report_attempts",
    "report_retry",
    "close_wait",
    "remove_retry",
    "unlocked_stale",
];
const GATEWAY_PACE: &[&str] = &["token_reuse_margin", "sweep_every", "shared_base_wait"];

/// One line per bound of `current` looser than its default, its pace keys left out.
fn looser<T: Serialize + Default>(
    prefix: &str,
    current: &T,
    pace: &[&str],
    smaller_is_looser: &[&str],
) -> Vec<String> {
    let (Ok(mut current), Ok(default)) = (
        serde_json::to_value(current),
        serde_json::to_value(T::default()),
    ) else {
        return Vec::new();
    };
    if let Some(table) = current.as_object_mut() {
        table.retain(|key, _| !pace.contains(&key.as_str()));
    }
    crate::bounds::looser(prefix, &current, &default, smaller_is_looser)
}

/// Refuses a zero for each named [`Limit`]: nothing could pass it.
fn at_least_one(prefix: &str, limits: &[(&str, Limit)]) -> Result<(), String> {
    match limits.iter().find(|(_, limit)| limit.get() == Some(0)) {
        Some((key, _)) => Err(format!("{prefix}.{key}: at least 1")),
        None => Ok(()),
    }
}

impl AssistantLimits {
    /// Checks what cannot work: a message, context or report bound of zero.
    ///
    /// # Errors
    ///
    /// Names the first key that is not acceptable.
    pub fn validate(&self) -> Result<(), String> {
        at_least_one(
            "assistant.limits",
            &[
                ("message_bytes", self.message_bytes),
                ("context_bytes", self.context_bytes),
                ("events", self.events),
            ],
        )
    }

    /// One line per bound looser than its default.
    pub fn weakenings(&self) -> Vec<String> {
        looser("assistant.limits", self, &[], &["memory_import_depth"])
    }
}

impl AssistantDeskLimits {
    /// Checks what cannot work: a job size or report attempt count of zero.
    ///
    /// # Errors
    ///
    /// Names the first key that is not acceptable.
    pub fn validate(&self) -> Result<(), String> {
        if self.report_attempts == 0 {
            return Err("assistant.desk.report_attempts: at least 1".to_owned());
        }
        at_least_one("assistant.desk", &[("job_bytes", self.job_bytes)])
    }

    /// One line per bound looser than its default; the keys that pace the desk are left out.
    pub fn weakenings(&self) -> Vec<String> {
        looser("assistant.desk", self, DESK_PACE, &[])
    }
}

impl AssistantGatewayLimits {
    /// Checks what cannot work: a conversation, message or socket bound of zero, and a sweep
    /// interval or event buffer of zero.
    ///
    /// # Errors
    ///
    /// Names the first key that is not acceptable.
    pub fn validate(&self) -> Result<(), String> {
        if self.sweep_every.0.is_zero() {
            return Err("assistant.gateway.sweep_every: more than 0".to_owned());
        }
        if self.event_buffer == 0 {
            return Err("assistant.gateway.event_buffer: at least 1".to_owned());
        }
        at_least_one(
            "assistant.gateway",
            &[
                ("conversations_per_person", self.conversations_per_person),
                ("conversations", self.conversations),
                ("conversation_entries", self.conversation_entries),
                ("live_per_person", self.live_per_person),
                ("message_bytes", self.message_bytes),
                ("socket_message_bytes", self.socket_message_bytes),
            ],
        )
    }

    /// One line per bound looser than its default; the keys that pace the gateway are left
    /// out.
    pub fn weakenings(&self) -> Vec<String> {
        looser("assistant.gateway", self, GATEWAY_PACE, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_limits_default_open_validate_and_never_weaken() {
        let pages = AssistantPageLimits::default();
        assert!(pages.validate().is_ok());
        assert!(pages.scripts && pages.feeds && pages.frame && pages.sharing);
        assert_eq!(pages.network, PageNetwork::Open);
        let set: AssistantPageLimits = serde_json::from_str(
            r#"{"max_pages": 500, "network": "own", "scripts": false, "default_ttl": 3600}"#,
        )
        .unwrap();
        assert_eq!(set.network, PageNetwork::Own);
        assert!(!set.scripts);
        assert_eq!(set.max_pages.get(), Some(500));
        assert!(
            serde_json::from_str::<AssistantPageLimits>(r#"{"nonsense": 1}"#).is_err(),
            "unknown keys are refused"
        );
        let zero: AssistantPageLimits = serde_json::from_str(r#"{"max_pages": 0}"#).unwrap();
        assert_eq!(
            zero.validate(),
            Err("assistant.pages.max_pages: at least 1".to_owned())
        );
        let ttl: AssistantPageLimits = serde_json::from_str(r#"{"open_ttl": 0}"#).unwrap();
        assert_eq!(
            ttl.validate(),
            Err("assistant.pages.open_ttl: more than 0".to_owned())
        );
        assert_eq!(
            PageNetwork::Open.tighter(PageNetwork::None),
            PageNetwork::None
        );
        assert_eq!(
            PageNetwork::Own.tighter(PageNetwork::Open),
            PageNetwork::Own
        );
    }

    #[test]
    fn defaults_have_no_weakening_and_a_wider_bound_is_one() {
        assert!(AssistantLimits::default().weakenings().is_empty());
        assert!(AssistantDeskLimits::default().weakenings().is_empty());
        assert!(AssistantGatewayLimits::default().weakenings().is_empty());
        let desk: AssistantDeskLimits =
            toml::from_str("live = \"none\"\njob_retry = 9\nreport_attempts = 9").unwrap();
        assert_eq!(
            desk.weakenings(),
            ["assistant.desk.live none is looser than the default 4"]
        );
        let limits: AssistantLimits = toml::from_str("memory_import_depth = 2").unwrap();
        assert_eq!(
            limits.weakenings(),
            ["assistant.limits.memory_import_depth 2 is looser than the default 5"]
        );
        let gateway: AssistantGatewayLimits =
            toml::from_str("token_ttl = 7200\nsweep_every = 5").unwrap();
        assert_eq!(
            gateway.weakenings(),
            ["assistant.gateway.token_ttl 7200.0 is looser than the default 3600.0"]
        );
    }

    #[test]
    fn a_bound_nothing_could_pass_is_refused() {
        let limits: AssistantLimits = toml::from_str("message_bytes = 0").unwrap();
        assert_eq!(
            limits.validate(),
            Err("assistant.limits.message_bytes: at least 1".to_owned())
        );
        let gateway: AssistantGatewayLimits = toml::from_str("sweep_every = 0").unwrap();
        assert!(gateway.validate().unwrap_err().contains("sweep_every"));
        let desk: AssistantDeskLimits = toml::from_str("report_attempts = 0").unwrap();
        assert!(desk.validate().unwrap_err().contains("report_attempts"));
        assert_eq!(AssistantLimits::default().validate(), Ok(()));
        assert_eq!(AssistantDeskLimits::default().validate(), Ok(()));
        assert_eq!(AssistantGatewayLimits::default().validate(), Ok(()));
        assert!(toml::from_str::<AssistantDeskLimits>("lives = 2").is_err());
    }
}
