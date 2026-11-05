//! The bounds and pace of Moneypenny: `[assistant.limits]`, what a job and a report may hold,
//! which a Station and Universal Exports both check; `[assistant.desk]`, how a
//! Station's desk runs her harnesses; and `[assistant.gateway]`, how Universal Exports keeps
//! her conversations. Each default is the value applied when the record says nothing.
//!
//! Counts and sizes are [`Limit`]s and ceilings on durations are [`TimeLimit`]s: `null` (or
//! `"none"` in TOML) takes the bound away. Other durations are [`Seconds`], whole or
//! fractional.

use guatiao::{FromValue, Schema, ToValue};
use guatiao_intake::Form;

use crate::bounds::{Limit, Seconds, TimeLimit};
use crate::directorates::m::records;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

/// `[assistant.limits]`: the bounds of what passes between Universal Exports and a Station
/// for Moneypenny.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct AssistantLimits {
    /// Operations one job grants Cable actions in.
    #[schema(title = "Operations with Cables in one job")]
    #[map(default = AssistantLimits::default().cable_grants)]
    pub cable_grants: Limit,
    /// Bytes of one job's message; Universal Exports refuses a person's longer message.
    #[schema(title = "Longest message", format = "bytes")]
    #[map(default = AssistantLimits::default().message_bytes)]
    pub message_bytes: Limit,
    /// Bytes of a job's standing context and recap together; a recap stays under half.
    #[schema(title = "Standing context and recap", format = "bytes")]
    #[map(default = AssistantLimits::default().context_bytes)]
    pub context_bytes: Limit,
    /// Bytes of one reported event's text; longer text is cut.
    #[schema(title = "Longest reported event", format = "bytes")]
    #[map(default = AssistantLimits::default().event_bytes)]
    pub event_bytes: Limit,
    /// Events in one report.
    #[schema(title = "Events in one report")]
    #[map(default = AssistantLimits::default().events)]
    pub events: Limit,
    /// Bytes of file content in one commit.
    #[schema(title = "File content in one commit", format = "bytes")]
    #[map(default = AssistantLimits::default().change_bytes)]
    pub change_bytes: Limit,
    /// Files in one commit.
    #[schema(title = "Files in one commit")]
    #[map(default = AssistantLimits::default().change_files)]
    pub change_files: Limit,
    /// Bytes of one commit's message.
    #[schema(title = "Longest commit message", format = "bytes")]
    #[map(default = AssistantLimits::default().commit_message_bytes)]
    pub commit_message_bytes: Limit,
    /// Shared-change bases one event carries, and Universal Exports keeps for one conversation
    /// until their calls arrive.
    #[schema(title = "Shared change bases")]
    #[map(default = AssistantLimits::default().shared_bases)]
    pub shared_bases: Limit,
    /// Skills one job gives her.
    #[schema(title = "Skills in one job")]
    #[map(default = AssistantLimits::default().skills)]
    pub skills: Limit,
    /// Files of her personal folder looked at to name what a turn changed.
    #[schema(title = "Files looked at per turn")]
    #[map(default = AssistantLimits::default().personal_walk)]
    pub personal_walk: Limit,
    /// Bytes of one file the Station reads to publish a page.
    #[schema(title = "Largest file read for a page", format = "bytes")]
    #[map(default = AssistantLimits::default().page_file_bytes)]
    pub page_file_bytes: Limit,
    /// Files the Station reads to publish one page.
    #[schema(title = "Files read for a page")]
    #[map(default = AssistantLimits::default().page_files)]
    pub page_files: Limit,
    /// Bytes over all the files of one page the Station reads.
    #[schema(title = "All the files read for a page", format = "bytes")]
    #[map(default = AssistantLimits::default().page_bytes)]
    pub page_bytes: Limit,
    /// Images one message carries.
    #[schema(title = "Images in one message")]
    #[map(default = AssistantLimits::default().images)]
    pub images: Limit,
    /// Bytes of one image a message carries.
    #[schema(title = "Largest image in a message", format = "bytes")]
    #[map(default = AssistantLimits::default().image_bytes)]
    pub image_bytes: Limit,
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
            skills: Limit::at(64),
            personal_walk: Limit::at(10_000),
            page_file_bytes: Limit::at(8 * MIB),
            page_files: Limit::at(200),
            page_bytes: Limit::at(16 * MIB),
            images: Limit::at(4),
            image_bytes: Limit::at(5 * MIB),
        }
    }
}

/// `[assistant.desk]`: how a Station's desk runs Moneypenny's harnesses. Read at each harness
/// start and job; `known` and `report_queue` at each use, and
/// `unlocked_stale` when the desk starts.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct AssistantDeskLimits {
    /// Seconds a harness waits for the next message, or writes nothing while answering, before it is
    /// closed; `"none"` keeps it.
    #[schema(title = "Close an idle harness after")]
    #[map(default = AssistantDeskLimits::default().idle)]
    pub idle: TimeLimit,
    /// Harnesses a Station runs or starts at once; past it a message is refused and another Station
    /// is asked.
    #[schema(title = "Harnesses on one Station")]
    #[map(default = AssistantDeskLimits::default().live)]
    pub live: Limit,
    /// Seconds before its tool token expires that a running harness is restarted with the job's; a
    /// job with another token restarts it whatever is left.
    #[schema(title = "Restart before a token expires")]
    #[map(default = AssistantDeskLimits::default().token_margin)]
    pub token_margin: Seconds,
    /// Seconds Universal Exports holds a job poll open.
    #[schema(title = "Job poll held open")]
    #[map(default = AssistantDeskLimits::default().job_wait)]
    pub job_wait: Seconds,
    /// Bytes of one job as Universal Exports sends it.
    #[schema(title = "Largest job", format = "bytes")]
    #[map(default = AssistantDeskLimits::default().job_bytes)]
    pub job_bytes: Limit,
    /// Seconds a request to Universal Exports may take past what it waits for.
    #[schema(title = "Extra time for a request")]
    #[map(default = AssistantDeskLimits::default().call_margin)]
    pub call_margin: Seconds,
    /// Seconds before a failed job poll is tried again, and the least between polls when Universal
    /// Exports answers early.
    #[schema(title = "Retry a failed job poll after")]
    #[map(default = AssistantDeskLimits::default().job_retry)]
    pub job_retry: Seconds,
    /// Ordinary reports (her text, her tool calls) waiting to be sent; past it the newest are
    /// dropped.
    #[schema(title = "Reports waiting to be sent")]
    #[map(default = AssistantDeskLimits::default().report_queue)]
    pub report_queue: Limit,
    /// Times a report is sent before it counts as lost.
    #[schema(title = "Tries to send a report")]
    #[map(default = AssistantDeskLimits::default().report_attempts)]
    pub report_attempts: u32,
    /// Seconds between tries of a report that did not reach Universal Exports.
    #[schema(title = "Retry a report after")]
    #[map(default = AssistantDeskLimits::default().report_retry)]
    pub report_retry: Seconds,
    /// Conversations whose provider Session the desk remembers for a resume.
    #[schema(title = "Conversations remembered for a resume")]
    #[map(default = AssistantDeskLimits::default().known)]
    pub known: Limit,
    /// Seconds a message waits for the conversation's closing harness.
    #[schema(title = "Wait for a closing harness")]
    #[map(default = AssistantDeskLimits::default().close_wait)]
    pub close_wait: Seconds,
    /// Seconds a route or account stays out when a harness reports a limit with no reset time.
    #[schema(title = "Out for this long at a limit with no reset time")]
    #[map(default = AssistantDeskLimits::default().limit_fallback)]
    pub limit_fallback: Seconds,
    /// Seconds removing a closed harness's directory is tried again.
    #[schema(title = "Retry removing a directory for")]
    #[map(default = AssistantDeskLimits::default().remove_retry)]
    pub remove_retry: Seconds,
    /// Seconds before a directory of working directories left without its lock, or a lock
    /// without its directory, is swept.
    #[schema(title = "Sweep abandoned directories after")]
    #[map(default = AssistantDeskLimits::default().unlocked_stale)]
    pub unlocked_stale: Seconds,
    /// Seconds a clone of one of her repositories may take.
    #[schema(title = "Longest clone")]
    #[map(default = AssistantDeskLimits::default().clone_timeout)]
    pub clone_timeout: Seconds,
    /// Bytes a clone of one of her repositories may hold on disk, then it is given up and removed.
    #[schema(title = "Largest clone", format = "bytes")]
    #[map(default = AssistantDeskLimits::default().clone_bytes)]
    pub clone_bytes: Limit,
    /// Bytes one save of her work in her personal folder may commit; past it nothing is saved and she
    /// is told.
    #[schema(title = "Largest save of her work", format = "bytes")]
    #[map(default = AssistantDeskLimits::default().save_bytes)]
    pub save_bytes: Limit,
    /// Cables one conversation holds for the person's confirmation; past it a send is refused,
    /// and nothing the person has not seen to is dropped to make room.
    #[schema(title = "Cables held per conversation")]
    #[map(default = AssistantDeskLimits::default().held_cables)]
    pub held_cables: Limit,
    /// Seconds a Cable held for the person's confirmation waits before it is dropped.
    #[schema(title = "A held Cable waits")]
    #[map(default = AssistantDeskLimits::default().held_cable_ttl)]
    pub held_cable_ttl: TimeLimit,
    /// Seconds sending one file of a page to Universal Exports may take.
    #[schema(title = "Longest page file upload")]
    #[map(default = AssistantDeskLimits::default().page_upload_timeout)]
    pub page_upload_timeout: Seconds,
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
            known: Limit::at(1_024),
            close_wait: Seconds::new(10),
            limit_fallback: Seconds::new(
                crate::directorates::m::limits::DEFAULT_LIMIT_FALLBACK_SECS,
            ),
            remove_retry: Seconds::new(2),
            unlocked_stale: Seconds::new(3_600),
            clone_timeout: Seconds::new(120),
            clone_bytes: Limit::at(512 * MIB),
            save_bytes: Limit::at(16 * MIB),
            held_cables: Limit::at(32),
            held_cable_ttl: TimeLimit::secs(3_600),
            page_upload_timeout: Seconds::new(60),
        }
    }
}

/// `[assistant.gateway]`: how Universal Exports keeps Moneypenny's conversations. Universal
/// Exports reads it when it starts.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct AssistantGatewayLimits {
    /// Conversations kept for one person; the oldest goes first.
    #[schema(title = "Conversations per person")]
    #[map(default = AssistantGatewayLimits::default().conversations_per_person)]
    pub conversations_per_person: Limit,
    /// Conversations kept in all.
    #[schema(title = "Conversations in all")]
    #[map(default = AssistantGatewayLimits::default().conversations)]
    pub conversations: Limit,
    /// Seconds a conversation is kept untouched.
    #[schema(title = "Keep an untouched conversation for")]
    #[map(default = AssistantGatewayLimits::default().conversation_age)]
    pub conversation_age: TimeLimit,
    /// Entries kept per conversation.
    #[schema(title = "Entries per conversation")]
    #[map(default = AssistantGatewayLimits::default().conversation_entries)]
    pub conversation_entries: Limit,
    /// Bytes of entries kept per conversation; the oldest go first.
    #[schema(title = "Entries kept per conversation, in size", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().conversation_bytes)]
    pub conversation_bytes: Limit,
    /// Bytes of entries kept per person; a person's views go first.
    #[schema(title = "Entries kept per person, in size", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().person_bytes)]
    pub person_bytes: Limit,
    /// One person's conversations with a harness running or an answer pending; the least recently
    /// used idle harness is closed to make room.
    #[schema(title = "Live conversations per person")]
    #[map(default = AssistantGatewayLimits::default().live_per_person)]
    pub live_per_person: Limit,
    /// Tool calls one conversation makes in a minute.
    #[schema(title = "Tool calls a minute")]
    #[map(default = AssistantGatewayLimits::default().tool_calls_per_minute)]
    pub tool_calls_per_minute: Limit,
    /// Seconds a tool token given to a harness is accepted; past it, while that harness's answer runs
    /// and its Station was heard from within `answer_deadline`, until the person stops it.
    #[schema(title = "A tool token lasts")]
    #[map(default = AssistantGatewayLimits::default().token_ttl)]
    pub token_ttl: Seconds,
    /// Seconds a token must still have to be given to the next message; kept above `[assistant.desk]
    /// token_margin`.
    #[schema(title = "Keep a token if it still has")]
    #[map(default = AssistantGatewayLimits::default().token_reuse_margin)]
    pub token_reuse_margin: Seconds,
    /// Seconds a Station has to take up a message before another is asked.
    #[schema(title = "A Station takes up a message within")]
    #[map(default = AssistantGatewayLimits::default().answer_timeout)]
    pub answer_timeout: Seconds,
    /// Seconds an accepted message may go without word from its Station before it is given up.
    #[schema(title = "Give up on a silent answer after")]
    #[map(default = AssistantGatewayLimits::default().answer_deadline)]
    pub answer_deadline: Seconds,
    /// Seconds a proposal waits for the person's confirmation.
    #[schema(title = "A proposal waits")]
    #[map(default = AssistantGatewayLimits::default().proposal_ttl)]
    pub proposal_ttl: Seconds,
    /// Proposals waiting for the person in one conversation.
    #[schema(title = "Proposals waiting per conversation")]
    #[map(default = AssistantGatewayLimits::default().proposals)]
    pub proposals: Limit,
    /// Bytes of one message the person writes.
    #[schema(title = "Longest message a person writes", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().message_bytes)]
    pub message_bytes: Limit,
    /// Bytes of her memory's summaries in a new harness's context; the job's context bound
    /// applies as well, and she can search for what does not fit.
    #[schema(title = "Memory summaries in her context", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().memory_context_bytes)]
    pub memory_context_bytes: Limit,
    /// Characters of the one-line message of a shared change she proposes.
    #[schema(title = "Longest shared change message")]
    #[map(default = AssistantGatewayLimits::default().change_message_chars)]
    pub change_message_chars: Limit,
    /// Bytes of the recap a conversation's new harness starts with.
    #[schema(title = "Longest recap", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().recap_bytes)]
    pub recap_bytes: Limit,
    /// Entries the app is sent when it opens a conversation.
    #[schema(title = "Entries sent when a conversation opens")]
    #[map(default = AssistantGatewayLimits::default().ready_entries)]
    pub ready_entries: Limit,
    /// Bytes of entries the app is sent when it opens a conversation.
    #[schema(title = "Size of the entries sent then", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().ready_bytes)]
    pub ready_bytes: Limit,
    /// Bytes of one message on the app's socket.
    #[schema(title = "Longest message on the app's socket", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().socket_message_bytes)]
    pub socket_message_bytes: Limit,
    /// Seconds the app's socket has to say hello.
    #[schema(title = "The app's socket says hello within")]
    #[map(default = AssistantGatewayLimits::default().hello_timeout)]
    pub hello_timeout: Seconds,
    /// Seconds a Station's job poll may be held open, also while `assistant = false`.
    #[schema(title = "Longest job poll")]
    #[map(default = AssistantGatewayLimits::default().max_wait)]
    pub max_wait: Seconds,
    /// Seconds between an open conversation's looks for jobs no Station took up.
    #[schema(title = "Look for jobs nobody took every")]
    #[map(default = AssistantGatewayLimits::default().sweep_every)]
    pub sweep_every: Seconds,
    /// Seconds a shared change waits for its Station's report of what it was made from.
    #[schema(title = "A shared change waits for its base")]
    #[map(default = AssistantGatewayLimits::default().shared_base_wait)]
    pub shared_base_wait: Seconds,
    /// Events queued for one conversation's open sockets; a socket that falls further behind is
    /// closed.
    #[schema(title = "Events queued for open sockets")]
    #[map(default = AssistantGatewayLimits::default().event_buffer)]
    pub event_buffer: u64,
    /// Bytes of an AI6 route's answer to one of her tools.
    #[schema(title = "Largest route answer to her tools", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().route_bytes)]
    pub route_bytes: Limit,
    /// Characters of one tool's answer she is given; longer is cut.
    #[schema(title = "Longest tool answer she is given")]
    #[map(default = AssistantGatewayLimits::default().result_chars)]
    pub result_chars: Limit,
    /// Characters of a person's name and email on her commits.
    #[schema(title = "Longest name and email on her commits")]
    #[map(default = AssistantGatewayLimits::default().author_chars)]
    pub author_chars: Limit,
    /// Bytes of an audit record's target.
    #[schema(title = "Longest audit record target", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().audit_target_bytes)]
    pub audit_target_bytes: Limit,
    /// Read tool calls of one turn a proposal lists as what she had read.
    #[schema(title = "Reads a proposal lists")]
    #[map(default = AssistantGatewayLimits::default().proposal_reads)]
    pub proposal_reads: Limit,
    /// Seconds an entry of her conversation log is kept; `none` keeps every entry.
    #[schema(title = "Keep her conversation log for")]
    #[map(default = AssistantGatewayLimits::default().log_age)]
    pub log_age: TimeLimit,
    /// Bytes of her conversation log kept, every conversation's together; the oldest entries go
    /// first, and `none` keeps every entry.
    #[schema(title = "Her conversation log, in size", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().log_bytes)]
    pub log_bytes: Limit,
    /// Seconds between two passes that drop what her conversation log keeps no longer.
    #[schema(title = "Trim her conversation log every")]
    #[map(default = AssistantGatewayLimits::default().log_prune_every)]
    pub log_prune_every: Seconds,
    /// Seconds a stopping gateway waits for her conversation log to keep what is still queued;
    /// `none` waits for all of it.
    #[schema(title = "Wait for her conversation log at a stop for")]
    #[map(default = AssistantGatewayLimits::default().log_flush_timeout)]
    pub log_flush_timeout: TimeLimit,
    /// Comments of a tracker item `source_item` reads, the latest.
    #[schema(title = "Comments read of a tracker item")]
    #[map(default = AssistantGatewayLimits::default().source_comments)]
    pub source_comments: Limit,
    /// Items `source_items` lists.
    #[schema(title = "Tracker items listed")]
    #[map(default = AssistantGatewayLimits::default().source_items)]
    pub source_items: Limit,
    /// Seconds a live tracker read waits for the Headquarters Station to answer.
    #[schema(title = "Wait for a live tracker read")]
    #[map(default = AssistantGatewayLimits::default().source_wait)]
    pub source_wait: Seconds,
    /// Live tracker reads waiting for the Headquarters Station at once.
    #[schema(title = "Live tracker reads waiting")]
    #[map(default = AssistantGatewayLimits::default().source_reads)]
    pub source_reads: Limit,
    /// Bytes of the Headquarters Station's answer to one live tracker read.
    #[schema(title = "Longest live tracker read answer", format = "bytes")]
    #[map(default = AssistantGatewayLimits::default().source_answer_bytes)]
    pub source_answer_bytes: Limit,
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
            token_ttl: Seconds::new(3_600),
            token_reuse_margin: Seconds::new(600),
            answer_timeout: Seconds::new(60),
            answer_deadline: Seconds::new(600),
            proposal_ttl: Seconds::new(3_600),
            proposals: Limit::at(32),
            message_bytes: Limit::at(16 * KIB),
            memory_context_bytes: Limit::at(8 * KIB),
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
            proposal_reads: Limit::at(16),
            log_age: TimeLimit::secs(90 * 24 * 3_600),
            log_bytes: Limit::at(1024 * MIB),
            log_prune_every: Seconds::new(3_600),
            log_flush_timeout: TimeLimit::secs(10),
            source_comments: Limit::at(10),
            source_items: Limit::at(50),
            source_wait: Seconds::new(30),
            source_reads: Limit::at(32),
            source_answer_bytes: Limit::at(MIB),
        }
    }
}

/// What a page may reach from the browser, `[assistant.pages] network`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
pub enum PageNetwork {
    /// Anything: the page loads and fetches from any host. The default.
    #[default]
    #[map(rename = "open")]
    #[schema(label = "Anywhere")]
    Open,
    /// Only its own files and feeds, `data:` and `blob:`.
    #[map(rename = "own")]
    #[schema(label = "Its own files and feeds")]
    Own,
    /// Its own files only: no feeds, no outside request.
    #[map(rename = "none")]
    #[schema(label = "Its own files only")]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct AssistantPageLimits {
    /// Bytes of one file of a page.
    #[schema(title = "Largest file of a page", format = "bytes")]
    #[map(default = AssistantPageLimits::default().max_file_bytes)]
    pub max_file_bytes: Limit,
    /// Bytes of one page.
    #[schema(title = "Largest page", format = "bytes")]
    #[map(default = AssistantPageLimits::default().max_page_bytes)]
    pub max_page_bytes: Limit,
    /// Files of one page.
    #[schema(title = "Files in a page")]
    #[map(default = AssistantPageLimits::default().max_page_files)]
    pub max_page_files: Limit,
    /// Pages one person keeps.
    #[schema(title = "Pages per person")]
    #[map(default = AssistantPageLimits::default().max_pages)]
    pub max_pages: Limit,
    /// Bytes of all one person's pages.
    #[schema(title = "All of one person's pages", format = "bytes")]
    #[map(default = AssistantPageLimits::default().max_person_bytes)]
    pub max_person_bytes: Limit,
    /// Bytes of all pages on one gateway.
    #[schema(title = "All pages on this gateway", format = "bytes")]
    #[map(default = AssistantPageLimits::default().max_total_bytes)]
    pub max_total_bytes: Limit,
    /// Bytes of an inline page's content.
    #[schema(title = "Largest inline page", format = "bytes")]
    #[map(default = AssistantPageLimits::default().inline_bytes)]
    pub inline_bytes: Limit,
    /// Characters of a page's title.
    #[schema(title = "Longest page title")]
    #[map(default = AssistantPageLimits::default().title_chars)]
    pub title_chars: Limit,
    /// Feeds one dashboard declares.
    #[schema(title = "Feeds in a dashboard")]
    #[map(default = AssistantPageLimits::default().max_feeds)]
    pub max_feeds: Limit,
    /// Feed reads one link makes in a minute.
    #[schema(title = "Feed reads a minute, per link")]
    #[map(default = AssistantPageLimits::default().feed_reads_per_minute)]
    pub feed_reads_per_minute: Limit,
    /// Pages Universal Exports keeps staged from Stations at once.
    #[schema(title = "Uploads held from Stations")]
    #[map(default = AssistantPageLimits::default().staged_uploads)]
    pub staged_uploads: Limit,
    /// Seconds a page lives unless remembered.
    #[schema(title = "Keep a page for")]
    #[map(default = AssistantPageLimits::default().default_ttl)]
    pub default_ttl: Seconds,
    /// Seconds a frame's view token, which the page's files and feeds are named under, is valid.
    /// A reload names a new one.
    #[schema(title = "A view of a page lasts")]
    #[map(default = AssistantPageLimits::default().open_ttl)]
    pub open_ttl: Seconds,
    /// Seconds a hand-off, the single-use token an app's request gives a browser that holds no
    /// page cookie, may wait to be used. A restart ends one not yet used.
    #[schema(title = "A hand-off to a browser lasts")]
    #[map(default = AssistantPageLimits::default().handoff_ttl)]
    pub handoff_ttl: Seconds,
    /// Hand-offs one person holds unused at once; past it, their oldest ends.
    #[schema(title = "Unused hand-offs per person")]
    #[map(default = AssistantPageLimits::default().handoffs)]
    pub handoffs: Limit,
    /// Seconds a publish waits for the Station's files.
    #[schema(title = "Wait for a Station's files")]
    #[map(default = AssistantPageLimits::default().upload_wait)]
    pub upload_wait: Seconds,
    /// Seconds a staged upload no call claims is kept.
    #[schema(title = "Keep an unclaimed upload for")]
    #[map(default = AssistantPageLimits::default().stage_ttl)]
    pub stage_ttl: Seconds,
    /// Seconds between sweeps of expired pages, links and staged uploads.
    #[schema(title = "Sweep expired pages every")]
    #[map(default = AssistantPageLimits::default().sweep_every)]
    pub sweep_every: Seconds,
    /// Whether a page's scripts run.
    #[schema(title = "Run pages' scripts")]
    #[map(default = AssistantPageLimits::default().scripts)]
    pub scripts: bool,
    /// What a page may reach from the browser.
    #[schema(title = "What a page may reach")]
    #[map(default = AssistantPageLimits::default().network)]
    pub network: PageNetwork,
    /// Whether dashboards may read live feeds.
    #[schema(title = "Dashboards read live feeds")]
    #[map(default = AssistantPageLimits::default().feeds)]
    pub feeds: bool,
    /// Whether a page's frame shows the classification bar. The page stays in the gateway's
    /// frame either way, which names the view token of its files and feeds.
    #[schema(title = "Show the classification bar")]
    #[map(default = AssistantPageLimits::default().frame)]
    pub frame: bool,
    /// Whether a person may share a page.
    #[schema(title = "People may share pages")]
    #[map(default = AssistantPageLimits::default().sharing)]
    pub sharing: bool,
    /// Subjects one page is shared with.
    #[schema(title = "Subjects a page is shared with")]
    #[map(default = AssistantPageLimits::default().max_shares)]
    pub max_shares: Limit,
    /// Whether a person may give a page a public link, which opens it for anyone holding the
    /// link, with no sign-in and no feeds. Off, every public link stops answering.
    #[schema(title = "People may make public links")]
    #[map(default = AssistantPageLimits::default().public)]
    pub public: bool,
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
            handoff_ttl: Seconds::new(30),
            handoffs: Limit::at(16),
            upload_wait: Seconds::new(30),
            stage_ttl: Seconds::new(300),
            sweep_every: Seconds::new(60),
            scripts: true,
            network: PageNetwork::Open,
            feeds: true,
            frame: true,
            sharing: true,
            max_shares: Limit::at(32),
            public: true,
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
    "limit_fallback",
    "remove_retry",
    "unlocked_stale",
    "page_upload_timeout",
];
const GATEWAY_PACE: &[&str] = &[
    "token_reuse_margin",
    "sweep_every",
    "shared_base_wait",
    "log_prune_every",
    "log_flush_timeout",
];

/// One line per bound of `current` looser than its default, its pace keys left out.
fn looser<T: ToValue + Default>(
    prefix: &str,
    current: &T,
    pace: &[&str],
    smaller_is_looser: &[&str],
) -> Vec<String> {
    let (Some(mut current), Some(default)) =
        (records::to_json(current), records::to_json(&T::default()))
    else {
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
        looser("assistant.limits", self, &[], &[])
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
        if self.log_prune_every.0.is_zero() {
            return Err("assistant.gateway.log_prune_every: more than 0".to_owned());
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
        let set: AssistantPageLimits = crate::directorates::m::records::from_toml(
            "max_pages = 500
network = \"own\"
scripts = false
default_ttl = 3600",
        )
        .unwrap();
        assert_eq!(set.network, PageNetwork::Own);
        assert!(!set.scripts);
        assert_eq!(set.max_pages.get(), Some(500));
        assert!(
            crate::directorates::m::records::from_toml::<AssistantPageLimits>("nonsense = 1")
                .is_err(),
            "unknown keys are refused"
        );
        let zero: AssistantPageLimits =
            crate::directorates::m::records::from_toml("max_pages = 0").unwrap();
        assert_eq!(
            zero.validate(),
            Err("assistant.pages.max_pages: at least 1".to_owned())
        );
        let ttl: AssistantPageLimits =
            crate::directorates::m::records::from_toml("open_ttl = 0").unwrap();
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
        let desk: AssistantDeskLimits = crate::directorates::m::records::from_toml(
            "live = \"none\"\njob_retry = 9\nreport_attempts = 9",
        )
        .unwrap();
        assert_eq!(
            desk.weakenings(),
            ["assistant.desk.live none is looser than the default 4"]
        );
        let limits: AssistantLimits =
            crate::directorates::m::records::from_toml("personal_walk = 20000").unwrap();
        assert_eq!(
            limits.weakenings(),
            ["assistant.limits.personal_walk 20000 is looser than the default 10000"]
        );
        let gateway: AssistantGatewayLimits =
            crate::directorates::m::records::from_toml("token_ttl = 7200\nsweep_every = 5")
                .unwrap();
        assert_eq!(
            gateway.weakenings(),
            ["assistant.gateway.token_ttl 7200.0 is looser than the default 3600.0"]
        );
    }

    #[test]
    fn a_bound_nothing_could_pass_is_refused() {
        let limits: AssistantLimits =
            crate::directorates::m::records::from_toml("message_bytes = 0").unwrap();
        assert_eq!(
            limits.validate(),
            Err("assistant.limits.message_bytes: at least 1".to_owned())
        );
        let gateway: AssistantGatewayLimits =
            crate::directorates::m::records::from_toml("sweep_every = 0").unwrap();
        assert!(gateway.validate().unwrap_err().contains("sweep_every"));
        let desk: AssistantDeskLimits =
            crate::directorates::m::records::from_toml("report_attempts = 0").unwrap();
        assert!(desk.validate().unwrap_err().contains("report_attempts"));
        assert_eq!(AssistantLimits::default().validate(), Ok(()));
        assert_eq!(AssistantDeskLimits::default().validate(), Ok(()));
        assert_eq!(AssistantGatewayLimits::default().validate(), Ok(()));
        assert!(
            crate::directorates::m::records::from_toml::<AssistantDeskLimits>("lives = 2").is_err()
        );
    }
}
