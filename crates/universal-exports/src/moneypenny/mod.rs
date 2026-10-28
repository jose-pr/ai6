//! Moneypenny, the operator's assistant, as Universal Exports serves her.
//!
//! The model runs on a Station, through the same AgentHandler providers as every Agent: the
//! roster's assistant Agent (`moneypenny` unless configured otherwise) with its handler,
//! model, effort and model credentials. Universal Exports keeps the conversations, relays
//! each message to a Station as an [`ai6::AssistantJob`] and what the harness says back to
//! the app, and serves her tools on an MCP endpoint of its own. A tool runs as the person
//! who is chatting, through the same routes and authorization as the app.
//!
//! When the roster's `[assistant]` gives people personal repositories, each message names the
//! person's workspace (their repository, their groups' shared ones, their name for commits).
//! A memory change the Station sets aside is committed at once in a conversation that has
//! read nothing others wrote, and is a proposal once it has; a change to a shared repository
//! is a proposal unless the group's repository is `write = true`.
//!
//! Exposure has one record, `Conversation.exposed`, never cleared. A read tool marks it
//! unless it is one of [`tools::QUIET_TOOLS`], and a Station marks it
//! (`POST /v1/station/assistant/exposure`) for what only it sees. Group and organisation
//! repositories are trusted text and expose nothing. Jobs carry it as
//! [`ai6::AssistantJob::exposed`].
//!
//! Routes: `GET /v1/user/moneypenny` (the app's WebSocket), `POST /v1/moneypenny/mcp` (the
//! harness's tools, with a token bound to the person and the conversation),
//! `/v1/user/assistant/memory` (the person's memory, [`memory`]),
//! `GET /v1/station/assistant/next`, `POST /v1/station/assistant/reports` and
//! `GET`/`POST /v1/station/assistant/exposure` (the Stations).

pub(crate) mod actions;
pub(crate) mod cables;
mod guide;
pub(crate) mod mcp;
pub(crate) mod memory;
mod store;
pub(crate) mod tools;
pub(crate) mod views;

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ai6::{
    AssistantChange, AssistantCommit, AssistantEvent, AssistantJob, AssistantReach,
    AssistantReport, AssistantSettings, AssistantWorkspace, COMMIT_MEMORY, COMMIT_SHARED,
    EVENT_ACCEPTED, EVENT_CABLE_PROPOSAL, EVENT_CABLE_SENT, EVENT_CLOSED, EVENT_COMPLETED,
    EVENT_FAILED, EVENT_INTERRUPTED, EVENT_LIMIT, EVENT_MEMORY_CHANGES, EVENT_NOTICE,
    EVENT_REJECTED, EVENT_TEXT, EVENT_TOOL, MAX_ASSISTANT_CONTEXT_BYTES, MAX_ASSISTANT_EVENTS,
    MAX_ASSISTANT_MESSAGE_BYTES, outside_tool,
};
use axum::Json;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, broadcast};
use uuid::Uuid;

use crate::{
    ApiError, Gateway, GatewayError, authorize_resource_action, bearer, resolve_user,
    unix_timestamp,
};
use store::{Bounds, Conversation, Entry, Pending, Role, Store};

/// Moneypenny's settings: which Agent she is and how much the gateway keeps.
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantConfig {
    agent_id: String,
    per_person: usize,
    total: usize,
    max_age: Duration,
    max_entries: usize,
    token_ttl: Duration,
    answer_timeout: Duration,
    answer_deadline: Duration,
    tool_calls_per_minute: u32,
    live_per_person: usize,
    conversation_bytes: usize,
    person_bytes: usize,
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            agent_id: "moneypenny".to_owned(),
            per_person: 20,
            total: 2_000,
            max_age: Duration::from_secs(7 * 24 * 3_600),
            max_entries: 500,
            token_ttl: Duration::from_secs(3_600),
            answer_timeout: Duration::from_secs(60),
            answer_deadline: Duration::from_secs(10 * 60),
            tool_calls_per_minute: 120,
            live_per_person: 2,
            conversation_bytes: 1024 * 1024,
            person_bytes: 4 * 1024 * 1024,
        }
    }
}

impl AssistantConfig {
    /// Moneypenny is the roster Agent `agent_id`; every other setting at its default.
    ///
    /// # Errors
    ///
    /// [`GatewayError::InvalidConfiguration`] for an empty identifier, one longer than 256
    /// bytes or one with control characters.
    pub fn new(agent_id: impl Into<String>) -> Result<Self, GatewayError> {
        let agent_id = agent_id.into();
        if !ai6::directorates::section_00::assistant::valid_identity(&agent_id) {
            return Err(GatewayError::InvalidConfiguration);
        }
        Ok(Self {
            agent_id,
            ..Self::default()
        })
    }

    /// The roster Agent that is Moneypenny.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Keeps at most `per_person` conversations for one person (default 20) and `total` in
    /// all (default 2,000), each until it has been untouched for `max_age` (default 7 days).
    /// The oldest goes first; zero counts become one.
    #[must_use]
    pub fn with_conversation_limits(
        mut self,
        per_person: usize,
        total: usize,
        max_age: Duration,
    ) -> Self {
        self.per_person = per_person.max(1);
        self.total = total.max(1);
        self.max_age = max_age;
        self
    }

    /// How long a token given to a harness for its tools is accepted (default one hour,
    /// at least one minute). A harness is restarted with a fresh token before its own
    /// expires.
    #[must_use]
    pub fn with_token_ttl(mut self, ttl: Duration) -> Self {
        self.token_ttl = ttl.max(Duration::from_secs(60));
        self
    }

    /// How long a Station has to take up a message before another is asked (default 60
    /// seconds).
    #[must_use]
    pub fn with_answer_timeout(mut self, timeout: Duration) -> Self {
        self.answer_timeout = timeout;
        self
    }

    /// How long an accepted message may go without word from its Station before the answer
    /// is given up (default 10 minutes).
    #[must_use]
    pub fn with_answer_deadline(mut self, deadline: Duration) -> Self {
        self.answer_deadline = deadline;
        self
    }

    /// How many of one person's conversations may have a harness running or a message being
    /// answered at once (default 2, at least 1). Past it, the person's least recently used
    /// idle harness is closed; when every one is answering, the message is refused.
    #[must_use]
    pub fn with_live_conversations_per_person(mut self, conversations: usize) -> Self {
        self.live_per_person = conversations.max(1);
        self
    }

    /// The bytes of entries kept per conversation (default 1 MiB) and per person (default
    /// 4 MiB); the oldest go first, a person's views before anything else.
    #[must_use]
    pub fn with_memory_limits(mut self, per_conversation: usize, per_person: usize) -> Self {
        self.conversation_bytes = per_conversation.max(64 * 1024);
        self.person_bytes = per_person.max(self.conversation_bytes);
        self
    }

    fn bounds(&self) -> Bounds {
        Bounds {
            entries: self.max_entries,
            bytes: self.conversation_bytes,
        }
    }

    /// How many tool calls one conversation may make in a minute (default 120).
    #[must_use]
    pub fn with_tool_calls_per_minute(mut self, calls: u32) -> Self {
        self.tool_calls_per_minute = calls.max(1);
        self
    }
}

/// The token a harness presents: whose conversation it is, and the Station whose harness was
/// given it. It is withdrawn when the conversation moves to another Station.
struct Grant {
    conversation_id: String,
    person_id: String,
    station: String,
    expires_at: f64,
}

#[derive(Default)]
struct Desk {
    store: Store,
    /// Tokens by their SHA-256.
    tokens: HashMap<String, Grant>,
    /// Tool calls in the current minute, by conversation.
    calls: HashMap<String, (Instant, u32)>,
    /// Jobs that close or interrupt a harness, by the Station that runs it.
    controls: Vec<(String, AssistantJob)>,
    /// The control jobs a Station has not answered yet, by ID, with the Station given each.
    controlling: HashMap<String, (String, Control)>,
    /// Confirmed Cables waiting for their Station, by control job ID: the conversation, the
    /// proposal and when they are given up, Unix seconds.
    cable_deadlines: HashMap<String, (String, String, f64)>,
    /// Memory requests waiting for their Station's answer, by job ID: the Station given the
    /// job, and where its answer goes.
    memory: HashMap<String, (String, tokio::sync::oneshot::Sender<AssistantReport>)>,
}

/// What a control job asks of a Station.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Control {
    Close,
    Interrupt,
    /// A commit: a memory change or a shared change, the proposal it carries out when the
    /// person confirmed one, and what it is, for the note that tells her how it went.
    Commit {
        proposal: Option<String>,
        what: String,
    },
    /// Send the Cable held for the proposal with this ID.
    CableConfirm(String),
    /// Drop the Cable held for the proposal with this ID.
    CableDiscard(String),
}

/// A job that closes or interrupts the harness of `conversation`, or makes `commit`, sending
/// no message; a Cable decision is the caller's to fill in.
fn control_job(
    conversation: &Conversation,
    agent_id: &str,
    control: &Control,
    commit: Option<AssistantCommit>,
) -> AssistantJob {
    AssistantJob {
        job_id: Uuid::new_v4().to_string(),
        conversation_id: conversation.id.clone(),
        agent_id: agent_id.to_owned(),
        message: String::new(),
        context: Vec::new(),
        recap: None,
        // Never accepted anywhere: a control starts no harness.
        mcp_token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        token_expires_at: now() + 60.0,
        session_id: conversation.session_id.clone(),
        close: *control == Control::Close,
        interrupt: *control == Control::Interrupt,
        exposed: conversation.exposed,
        person_id: Some(conversation.person_id.clone()),
        workspace: None,
        commit,
        cables: Vec::new(),
        cable_confirm: None,
        cable_discard: None,
        memory: None,
        reach: AssistantReach::default(),
        // A control starts no harness, so it carries no reach or groups.
        groups: None,
    }
}

impl Desk {
    /// Asks the Station running `conversation_id`'s harness to close it, and withdraws the
    /// conversation's tokens.
    fn close_harness(&mut self, conversation_id: &str, agent_id: &str) {
        let Some(conversation) = self.store.conversations.get_mut(conversation_id) else {
            return;
        };
        conversation.live = false;
        conversation.token = None;
        let Some(station) = conversation.station.clone() else {
            return;
        };
        let job = control_job(conversation, agent_id, &Control::Close, None);
        self.tokens
            .retain(|_, grant| grant.conversation_id != conversation_id);
        self.controlling
            .insert(job.job_id.clone(), (station.clone(), Control::Close));
        self.controls.push((station, job));
    }

    /// Queues `commit` of `conversation_id` for `station`; its answer is handled as `control`.
    fn queue_commit(
        &mut self,
        conversation_id: &str,
        agent_id: &str,
        station: String,
        commit: AssistantCommit,
        control: Control,
    ) {
        let Some(conversation) = self.store.conversations.get(conversation_id) else {
            return;
        };
        let job = control_job(conversation, agent_id, &control, Some(commit));
        self.controlling
            .insert(job.job_id.clone(), (station.clone(), control));
        self.controls.push((station, job));
    }
}

/// The settings of Moneypenny's workspaces: the roster's `[assistant]`, the default without a
/// roster.
async fn assistant_settings(gateway: &Gateway) -> Result<AssistantSettings, String> {
    let Some(source) = gateway.roster.clone() else {
        return Ok(AssistantSettings::default());
    };
    tokio::task::spawn_blocking(move || source.load())
        .await
        .map_err(|error| error.to_string())?
        .map(|snapshot| snapshot.roster.assistant().clone())
        .map_err(|error| format!("the roster could not be read: {error}"))
}

/// Where `person`'s conversations keep their work, `None` when the deployment gives nobody a
/// personal repository; an `Err` says why this person cannot have one.
fn workspace_for(
    settings: &AssistantSettings,
    person: &ai6::Person,
) -> Result<Option<AssistantWorkspace>, String> {
    let Some(personal) = settings.personal_repo_for(&person.id)? else {
        return Ok(None);
    };
    let (author_name, author_email) = author_of(person);
    Ok(Some(AssistantWorkspace {
        personal,
        shared: settings
            .shared_for(&person.groups)
            .map(|(name, repo)| (name.to_owned(), repo.repo().to_owned()))
            .collect(),
        author_name,
        author_email,
    }))
}

/// The name and email `person`'s commits are authored with: the directory's, without control
/// characters, `<` or `>`; an email under the reserved `.invalid` domain when it has none.
fn author_of(person: &ai6::Person) -> (String, String) {
    let printable = |value: &str| -> String {
        value
            .chars()
            .map(|c| {
                if c.is_control() || matches!(c, '<' | '>') {
                    ' '
                } else {
                    c
                }
            })
            .collect::<String>()
            .trim()
            .chars()
            .take(200)
            .collect()
    };
    let name = printable(person.display_name.as_deref().unwrap_or(&person.name));
    let email = person
        .email
        .as_deref()
        .map(printable)
        .filter(|email| !email.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{}@people.ai6.invalid",
                ai6::directorates::m::assistant::person_digest(&person.id)
            )
        });
    let name = if name.is_empty() {
        printable(&person.id)
    } else {
        name
    };
    (name, email)
}

/// Settles proposal `id` of `conversation` as done or failed with `result`, and tells her
/// with the next message.
fn settle(conversation: &mut Conversation, id: &str, done: bool, result: &str) {
    let Some(proposal) = conversation
        .proposals
        .iter_mut()
        .find(|proposal| proposal.id == id)
    else {
        return;
    };
    proposal.status = if done { "done" } else { "failed" };
    proposal.result = Some(result.to_owned());
    let shown = proposal.clone();
    conversation.show_proposal(&shown);
    conversation.notes.push(format!(
        "[App: the person confirmed {id} ({}). {}: {result}]",
        shown.draft.summary,
        if done { "Done" } else { "It failed" }
    ));
}

/// The audit resource of commits to the repositories of the roster's `[assistant]` table.
const ASSISTANT_AUDIT_RESOURCE: &str = "config:assistant";

/// The commit trailer naming Moneypenny and the conversation.
fn trailer(conversation_id: &str) -> String {
    format!("Via: Moneypenny (conversation {conversation_id})")
}

/// The paths of a change, for a summary.
fn paths(change: &AssistantChange) -> String {
    change
        .files
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Moneypenny's state in one gateway.
pub(crate) struct Assistant {
    config: AssistantConfig,
    desk: Mutex<Desk>,
    ready: Notify,
}

impl fmt::Debug for Assistant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Assistant")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// A token with less left than this is not given to another message: a Station restarts a
/// harness whose token has under two minutes left, so the new one must outlast that.
const TOKEN_REUSE_MARGIN_SECS: f64 = 600.0;
/// The most entries `ready` sends, newest first, and their most bytes.
const READY_ENTRIES: usize = 100;
const READY_BYTES: usize = 512 * 1024;
/// The longest message the app may send, in bytes.
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// How much of the conversation so far a new harness is given, in bytes.
const MAX_RECAP_BYTES: usize = 8 * 1024;
/// How long the app has to say hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest a Station's poll is held open.
const MAX_WAIT_MS: u64 = 25_000;
/// How often an open conversation socket looks for jobs no Station took up.
const SWEEP_EVERY: Duration = Duration::from_secs(1);

fn digest(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn now() -> f64 {
    unix_timestamp().unwrap_or(0.0)
}

impl Assistant {
    pub(crate) fn new(config: AssistantConfig) -> Self {
        Self {
            config,
            desk: Mutex::default(),
            ready: Notify::new(),
        }
    }

    fn desk(&self) -> std::sync::MutexGuard<'_, Desk> {
        // A panic while holding it leaves bounded maps that are still consistent.
        self.desk
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Opens `requested` when it is the person's, else a new conversation; returns its ID,
    /// what the app shows and a subscription to what follows.
    fn open(
        &self,
        person_id: &str,
        bearer: &str,
        requested: Option<&str>,
    ) -> (String, Value, broadcast::Receiver<Value>) {
        let mut desk = self.desk();
        let now = now();
        if let Some(id) = requested
            && let Some(conversation) = desk.store.owned(id, person_id)
        {
            conversation.bearer = bearer.to_owned();
            return (
                conversation.id.clone(),
                ready(conversation),
                conversation.events.subscribe(),
            );
        }
        desk.store.make_room(
            person_id,
            now,
            self.config.max_age.as_secs_f64(),
            self.config.per_person,
            self.config.total,
        );
        let id = Uuid::new_v4().to_string();
        let conversation =
            Conversation::new(id.clone(), person_id.to_owned(), bearer.to_owned(), now);
        let shown = ready(&conversation);
        let events = conversation.events.subscribe();
        desk.store.conversations.insert(id.clone(), conversation);
        (id, shown, events)
    }

    /// Queues the person's message for a Station.
    async fn message(
        &self,
        gateway: &Gateway,
        conversation_id: &str,
        person_id: &str,
        text: &str,
        page: Option<&Value>,
    ) -> Result<(), (&'static str, String)> {
        if text.trim().is_empty() || text.len() > MAX_MESSAGE_BYTES {
            return Err((
                "bad_request",
                format!("A message is 1 to {MAX_MESSAGE_BYTES} bytes."),
            ));
        }
        let bearer = self
            .desk()
            .store
            .owned(conversation_id, person_id)
            .map(|conversation| conversation.bearer.clone())
            .ok_or(("not_found", "The conversation is gone.".to_owned()))?;
        // The tools run with this token: a message after it ended starts nothing.
        resolve_user(gateway, &bearer).await.map_err(|_| {
            (
                "unauthorized",
                "Sign in again to talk to Moneypenny.".to_owned(),
            )
        })?;
        let message = match page.and_then(page_line) {
            Some(line) => format!("{line}\n\n{text}"),
            None => text.to_owned(),
        };
        self.queue(gateway, conversation_id, person_id, &message, Some(text))
            .await
    }

    /// Queues `message` for a Station, after any notes for Moneypenny the app left since her
    /// last message; `shown` is the person's entry, when the person wrote it.
    async fn queue(
        &self,
        gateway: &Gateway,
        conversation_id: &str,
        person_id: &str,
        message: &str,
        shown: Option<&str>,
    ) -> Result<(), (&'static str, String)> {
        let now = now();
        // The workspace is the person's, as their own sign-in names them.
        let bearer = self
            .desk()
            .store
            .owned(conversation_id, person_id)
            .map(|conversation| conversation.bearer.clone())
            .ok_or(("not_found", "The conversation is gone.".to_owned()))?;
        let person = resolve_user(gateway, &bearer).await.map_err(|_| {
            (
                "unauthorized",
                "Sign in again to talk to Moneypenny.".to_owned(),
            )
        })?;
        let settings = assistant_settings(gateway).await.map_err(|reason| {
            (
                "unavailable",
                format!("Moneypenny cannot keep a workspace for you: {reason}"),
            )
        })?;
        let workspace = workspace_for(&settings, &person).map_err(|reason| {
            (
                "unavailable",
                format!("Moneypenny cannot keep a workspace for you: {reason}"),
            )
        })?;
        // Read again for every message, so a narrowed table reaches her next turn.
        let reach = settings.reach_for(&person.id, &person.groups);
        // Read again for every message, so a permission removed reaches her next turn.
        let cables = cables::grants(gateway, &person).await;
        {
            let mut desk = self.desk();
            let conversation = desk
                .store
                .owned(conversation_id, person_id)
                .ok_or(("not_found", "The conversation is gone.".to_owned()))?;
            if conversation.busy {
                return Err((
                    "busy",
                    "Moneypenny is still answering the last message.".to_owned(),
                ));
            }
            // At most `live_per_person` of the person's conversations hold a harness or wait
            // for an answer: the least recently used idle harness makes room.
            let mut others: Vec<(f64, String, bool)> = desk
                .store
                .conversations
                .values()
                .filter(|other| other.person_id == person_id && other.id != conversation_id)
                .filter(|other| other.busy || other.live)
                .map(|other| (other.updated_at, other.id.clone(), other.busy))
                .collect();
            let over = (others.len() + 1).saturating_sub(self.config.live_per_person);
            if over > 0 {
                others.retain(|(_, _, busy)| !busy);
                if others.len() < over {
                    return Err((
                        "busy_elsewhere",
                        "Moneypenny is answering in your other conversations; wait for one \
                         to finish."
                            .to_owned(),
                    ));
                }
                others.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (_, other, _) in others.into_iter().take(over) {
                    desk.close_harness(&other, &self.config.agent_id);
                }
            }
            let conversation = desk
                .store
                .owned(conversation_id, person_id)
                .ok_or(("not_found", "The conversation is gone.".to_owned()))?;
            let context = guide::context();
            let recap = recap(conversation);
            let mut parts = std::mem::take(&mut conversation.notes);
            parts.push(message.to_owned());
            let job = AssistantJob {
                job_id: Uuid::new_v4().to_string(),
                conversation_id: conversation_id.to_owned(),
                agent_id: self.config.agent_id.clone(),
                message: cut(&parts.join("\n\n"), MAX_ASSISTANT_MESSAGE_BYTES),
                context,
                recap,
                // Set when the job is placed: the token is bound to its Station.
                mcp_token: String::new(),
                token_expires_at: 0.0,
                session_id: conversation.session_id.clone(),
                close: false,
                interrupt: false,
                // Shared repositories are trusted text: checking them out exposes nothing.
                exposed: conversation.exposed,
                person_id: Some(person_id.to_owned()),
                workspace,
                commit: None,
                cables,
                cable_confirm: None,
                cable_discard: None,
                memory: None,
                reach,
                // The person's directory groups, so the Station applies its per-group narrowing.
                groups: Some(person.groups.clone()),
            };
            conversation.workspace = job.workspace.clone();
            // Sticky for the conversation: a harness started under a wider reach may still be running
            // its script when a narrower message arrives, so its `Bash` still counts as outside text.
            conversation.scripts_reach_hosts |= job.reach.scripts_reach_hosts();
            if let Some(text) = shown {
                conversation.push(
                    Entry {
                        role: Role::Person,
                        text: text.to_owned(),
                        at: now,
                        data: None,
                    },
                    self.config.bounds(),
                );
            }
            conversation.answered = false;
            conversation.set_busy(true);
            conversation.pending = Some(Pending {
                job,
                station: None,
                taken_at: None,
                queued_at: now,
                refused_by: Vec::new(),
                reason: None,
            });
        }
        self.ready.notify_waiters();
        self.place(gateway, conversation_id).await;
        Ok(())
    }

    /// Queues the conversation's pending job for the Station that runs its harness, else the
    /// Headquarters Station, else any healthy Station that has not refused it.
    async fn place(&self, gateway: &Gateway, conversation_id: &str) {
        let (preferred, refused) = {
            let mut desk = self.desk();
            let Some(conversation) = desk.store.conversations.get_mut(conversation_id) else {
                return;
            };
            let Some(pending) = &conversation.pending else {
                return;
            };
            (conversation.station.clone(), pending.refused_by.clone())
        };
        let healthy = healthy_stations(gateway).await;
        let chosen = preferred
            .filter(|station| healthy.contains(station))
            .into_iter()
            .chain(healthy)
            .find(|station| !refused.contains(station));
        let mut desk = self.desk();
        let max_entries = self.config.bounds();
        let Some(conversation) = desk.store.conversations.get_mut(conversation_id) else {
            return;
        };
        let Some(pending) = conversation.pending.as_mut() else {
            return;
        };
        match chosen {
            Some(station) => {
                // The harness on that Station keeps its token while it lasts; a new one is
                // made for a new Station or before the old one runs out, and the tokens of
                // any other Station are withdrawn, so a harness left there stops working.
                let now = now();
                let reuse = conversation
                    .token
                    .clone()
                    .filter(|(_, held_by, expires_at)| {
                        *held_by == station && *expires_at > now + TOKEN_REUSE_MARGIN_SECS
                    });
                let (token, expires_at) = match reuse {
                    Some((token, _, expires_at)) => (token, expires_at),
                    None => (
                        format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
                        now + self.config.token_ttl.as_secs_f64(),
                    ),
                };
                conversation.token = Some((token.clone(), station.clone(), expires_at));
                pending.job.mcp_token = token.clone();
                pending.job.token_expires_at = expires_at;
                pending.station = Some(station.clone());
                pending.taken_at = None;
                pending.queued_at = now;
                let person_id = conversation.person_id.clone();
                desk.tokens.retain(|_, grant| {
                    grant.expires_at > now
                        && (grant.conversation_id != conversation_id || grant.station == station)
                });
                desk.tokens.insert(
                    digest(&token),
                    Grant {
                        conversation_id: conversation_id.to_owned(),
                        person_id,
                        station,
                        expires_at,
                    },
                );
                drop(desk);
                self.ready.notify_waiters();
            }
            None => {
                let reason = pending
                    .reason
                    .clone()
                    .unwrap_or_else(|| "no Station is checking in".to_owned());
                conversation.pending = None;
                conversation.push(
                    Entry {
                        role: Role::Error,
                        text: format!("No Station can run Moneypenny now: {reason}."),
                        at: now(),
                        data: None,
                    },
                    max_entries,
                );
                conversation.set_busy(false);
            }
        }
    }

    /// Gives up on jobs their Station did not take up in time and offers them elsewhere. Run
    /// by every Station poll and by every open conversation socket, so a job is moved on
    /// even when no Station polls at all.
    async fn sweep(&self, gateway: &Gateway) {
        let timeout = self.config.answer_timeout.as_secs_f64();
        let now = now();
        let mut again = Vec::new();
        {
            let mut desk = self.desk();
            for conversation in desk.store.conversations.values_mut() {
                let Some(pending) = conversation.pending.as_mut() else {
                    continue;
                };
                let since = pending.taken_at.unwrap_or(pending.queued_at);
                if let Some(late) = pending.station.clone()
                    && now - since > timeout
                {
                    pending.refused_by.push(late);
                    pending.reason = Some("the Station did not answer".to_owned());
                    pending.station = None;
                    again.push(conversation.id.clone());
                }
            }
            // An accepted message whose Station went quiet is given up, so the conversation
            // can take another message and be evicted like any idle one.
            let deadline = self.config.answer_deadline.as_secs_f64();
            let bounds = self.config.bounds();
            for conversation in desk.store.conversations.values_mut() {
                if conversation.busy
                    && conversation.pending.is_none()
                    && now - conversation.heard_at > deadline
                {
                    conversation.live = false;
                    conversation.push(
                        Entry {
                            role: Role::Error,
                            text: "No word from Moneypenny's Station for too long; her answer \
                                   was given up. Ask again."
                                .to_owned(),
                            at: now,
                            data: None,
                        },
                        bounds,
                    );
                    conversation.set_busy(false);
                }
            }
        }
        for conversation in again {
            self.place(gateway, &conversation).await;
        }
        self.expire_confirms(gateway).await;
    }

    /// The next job for `station`: a close or interrupt first, then the oldest message.
    async fn take(&self, gateway: &Gateway, station: &str) -> Option<AssistantJob> {
        self.sweep(gateway).await;
        let now = now();
        let mut desk = self.desk();
        if let Some(index) = desk
            .controls
            .iter()
            .position(|(held_by, _)| held_by == station)
        {
            return Some(desk.controls.remove(index).1);
        }
        desk.store
            .conversations
            .values_mut()
            .filter_map(|conversation| conversation.pending.as_mut())
            .filter(|pending| {
                pending.station.as_deref() == Some(station) && pending.taken_at.is_none()
            })
            .min_by(|a, b| a.queued_at.total_cmp(&b.queued_at))
            .map(|pending| {
                pending.taken_at = Some(now);
                pending.job.clone()
            })
    }

    /// Applies a Station's report; `Err` with the HTTP status it earns.
    async fn report(&self, gateway: &Gateway, report: AssistantReport) -> Result<(), StatusCode> {
        if report.events.len() > MAX_ASSISTANT_EVENTS
            || !ai6::directorates::section_00::assistant::valid_identity(&report.station_id)
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        let max_entries = self.config.bounds();
        let mut refused = false;
        let mut notify = false;
        let mut after = cables::After::default();
        'locked: {
            let mut desk = self.desk();
            if let Some(job_id) = &report.job_id
                && let Some((given_to, _)) = desk.controlling.get(job_id)
            {
                // Only the Station the job was given to answers it.
                if *given_to != report.station_id {
                    return Err(StatusCode::FORBIDDEN);
                }
                let Some((_, control)) = desk.controlling.remove(job_id) else {
                    break 'locked;
                };
                desk.cable_deadlines.remove(job_id);
                // The answer to a close or an interrupt: the harness reports `closed` when it
                // is gone and `interrupted` when its turn stops. An interrupt the Station could
                // not make is shown, since the answer goes on.
                let refused = report
                    .events
                    .first()
                    .filter(|event| event.kind == EVENT_REJECTED);
                if control == Control::Interrupt
                    && let Some(refused) = refused
                    && let Some(conversation) =
                        desk.store.conversations.get_mut(&report.conversation_id)
                {
                    conversation.push(
                        Entry {
                            role: Role::Error,
                            text: format!(
                                "Moneypenny could not be stopped: {}",
                                or_unknown(refused.text.as_deref().unwrap_or_default())
                            ),
                            at: now(),
                            data: None,
                        },
                        max_entries,
                    );
                }
                let conversation = desk.store.conversations.get_mut(&report.conversation_id);
                match (control, conversation) {
                    (Control::Commit { proposal, what }, Some(conversation)) => {
                        committed(
                            conversation,
                            proposal.as_deref(),
                            &what,
                            &report,
                            max_entries,
                        );
                    }
                    (Control::CableConfirm(proposal), Some(conversation)) => {
                        let reason = refused.map(|event| event.text.clone().unwrap_or_default());
                        cables::answered(conversation, &proposal, reason, &mut after);
                    }
                    _ => {}
                }
                break 'locked;
            }
            let conversation = desk
                .store
                .conversations
                .get_mut(&report.conversation_id)
                .ok_or(StatusCode::NOT_FOUND)?;
            conversation.heard_at = now();
            if let Some(job_id) = &report.job_id {
                let pending = conversation
                    .pending
                    .as_mut()
                    .filter(|pending| {
                        pending.job.job_id == *job_id
                            && pending.station.as_deref() == Some(report.station_id.as_str())
                    })
                    .ok_or(StatusCode::CONFLICT)?;
                let answer = report.events.first().ok_or(StatusCode::BAD_REQUEST)?;
                match answer.kind.as_str() {
                    EVENT_ACCEPTED => {
                        conversation.pending = None;
                        conversation.live = true;
                        if conversation.station.as_deref() != Some(report.station_id.as_str()) {
                            conversation.station = Some(report.station_id.clone());
                        }
                    }
                    EVENT_REJECTED => {
                        pending.refused_by.push(report.station_id.clone());
                        pending.reason = answer.text.clone();
                        pending.station = None;
                        refused = true;
                    }
                    _ => return Err(StatusCode::BAD_REQUEST),
                }
            } else if conversation.station.as_deref() != Some(report.station_id.as_str()) {
                return Err(StatusCode::FORBIDDEN);
            }
            if let Some(session_id) = report
                .session_id
                .filter(|id| ai6::directorates::section_00::assistant::valid_identity(id))
            {
                conversation.session_id = Some(session_id);
            }
            let mut set_aside = Vec::new();
            for event in report
                .events
                .iter()
                .filter(|event| report.job_id.is_none() || event.kind != EVENT_ACCEPTED)
            {
                match (&event.change, event.kind.as_str()) {
                    (Some(change), EVENT_MEMORY_CHANGES) => {
                        set_aside.push((change.clone(), event.text.clone().unwrap_or_default()));
                    }
                    (_, EVENT_CABLE_PROPOSAL | EVENT_CABLE_SENT) => cables::apply(
                        conversation,
                        event,
                        &report.station_id,
                        max_entries,
                        &mut after,
                    ),
                    _ => apply(conversation, event, max_entries),
                }
            }
            for (change, diff) in set_aside {
                notify |= self.memory_change(
                    &mut desk,
                    &report.conversation_id,
                    &report.station_id,
                    change,
                    &diff,
                );
            }
            if let Some(conversation) = desk.store.conversations.get(&report.conversation_id) {
                let person = conversation.person_id.clone();
                desk.store.trim_person(&person, self.config.person_bytes);
            }
        }
        self.follow_up(gateway, after).await;
        if notify {
            self.ready.notify_waiters();
        }
        if refused {
            self.place(gateway, &report.conversation_id).await;
        }
        Ok(())
    }

    /// Handles a memory change a Station set aside for `conversation_id`: in a conversation
    /// that read others' text it becomes a proposal the person confirms (`memory_commit`) or
    /// cancels (`memory_discard`: it was set aside already); otherwise `station` is asked to
    /// commit it at once, and the person sees what was kept. Answers whether a job was
    /// queued.
    fn memory_change(
        &self,
        desk: &mut Desk,
        conversation_id: &str,
        station: &str,
        change: AssistantChange,
        diff: &str,
    ) -> bool {
        let bounds = self.config.bounds();
        let Some(conversation) = desk.store.conversations.get_mut(conversation_id) else {
            return false;
        };
        let Some(workspace) = conversation.workspace.clone() else {
            return false;
        };
        if let Err(reason) = change.validate(true) {
            conversation.push(
                Entry {
                    role: Role::Error,
                    text: format!("Moneypenny's memory change was refused: {reason}."),
                    at: now(),
                    data: None,
                },
                bounds,
            );
            return false;
        }
        let paths = paths(&change);
        let commit = AssistantCommit {
            kind: COMMIT_MEMORY.to_owned(),
            repo: workspace.personal.clone(),
            change,
            message: format!(
                "moneypenny: remember {paths}\n\n{}",
                trailer(conversation_id)
            ),
            author_name: workspace.author_name.clone(),
            author_email: workspace.author_email.clone(),
        };
        if conversation.exposed {
            let mut proposal = commit_proposal(
                actions::MEMORY_COMMIT,
                digest(&conversation.bearer),
                "memory/ of your personal repository".to_owned(),
                actions::Scope::Resource("your memory".to_owned()),
                "your confirmation",
                format!("Remember: {paths}"),
                commit,
            );
            if let Some(body) = proposal.draft.call.body.as_mut() {
                body["diff"] = json!(diff);
            }
            if record_proposal(conversation, proposal, bounds) {
                conversation.notes.push(format!(
                    "[App: your memory change ({paths}) waits for the person's confirmation, \
                     because this conversation read text others wrote.]"
                ));
            } else {
                conversation.push(
                    Entry {
                        role: Role::Error,
                        text: format!(
                            "Moneypenny's memory change ({paths}) was not kept: {PROPOSALS_FULL}"
                        ),
                        at: now(),
                        data: None,
                    },
                    bounds,
                );
                conversation.notes.push(format!(
                    "[App: your memory change ({paths}) was not kept: {PROPOSALS_FULL}]"
                ));
            }
            return false;
        }
        conversation.push(
            Entry {
                role: Role::Notice,
                text: format!("Moneypenny is saving to her memory: {paths}."),
                at: now(),
                data: Some(json!({"diff": diff})),
            },
            bounds,
        );
        desk.queue_commit(
            conversation_id,
            &self.config.agent_id,
            station.to_owned(),
            commit,
            Control::Commit {
                proposal: None,
                what: format!("Saving {paths} to Moneypenny's memory"),
            },
        );
        true
    }

    /// The conversation `token` serves, its person and the person's access token, while the
    /// token is accepted.
    pub(crate) fn grant(&self, token: &str) -> Option<Granted> {
        let mut desk = self.desk();
        let now = now();
        let grant = desk.tokens.get(&digest(token))?;
        if grant.expires_at <= now {
            return None;
        }
        let (conversation_id, person_id) = (grant.conversation_id.clone(), grant.person_id.clone());
        let conversation = desk.store.owned(&conversation_id, &person_id)?;
        Some(Granted {
            conversation_id,
            person_id,
            bearer: conversation.bearer.clone(),
        })
    }

    /// Runs Moneypenny's write tool `tool`: checks the input and that the person may do it,
    /// then records a proposal for the person to confirm. Nothing else happens.
    pub(crate) async fn propose(
        &self,
        gateway: &Gateway,
        granted: &Granted,
        tool: &actions::WriteTool,
        input: &Value,
    ) -> tools::Outcome {
        let id = format!("proposal-{}", Uuid::new_v4().simple());
        let draft = match tool.draft(input, &id) {
            Ok(draft) => draft,
            Err(outcome) => return outcome,
        };
        let Ok(person) = resolve_user(gateway, &granted.bearer).await else {
            return tools::Outcome::error("The person's sign-in has ended; nothing was proposed.");
        };
        if authorize_resource_action(gateway, person, draft.scope.resource(), draft.action)
            .await
            .is_err()
        {
            return tools::Outcome::error(format!(
                "Refused: the person may not {} on {}, so nothing was proposed. Tell them so; \
                 do not try another way.",
                draft.action,
                draft.scope.resource()
            ));
        }
        let summary = draft.summary.clone();
        let proposal = actions::Proposal {
            id: id.clone(),
            tool: tool.name,
            draft,
            status: "pending",
            created_at: now(),
            session: digest(&granted.bearer),
            result: None,
            work: None,
            cable: None,
        };
        let max_entries = self.config.bounds();
        {
            let mut desk = self.desk();
            let Some(conversation) = desk
                .store
                .owned(&granted.conversation_id, &granted.person_id)
            else {
                return tools::Outcome::error("The conversation is gone.");
            };
            if !record_proposal(conversation, proposal, max_entries) {
                return tools::Outcome::error(PROPOSALS_FULL);
            }
        }
        tools::Outcome {
            text: format!(
                "Proposed as {id}: {summary}. Nothing has changed. The person confirms or \
                 cancels it in the app; you will be told which."
            ),
            is_error: false,
        }
    }

    /// Runs a pending proposal the person confirmed on a socket signed in with `token`, the
    /// session it was proposed in, and tells Moneypenny how it went.
    async fn confirm(
        &self,
        gateway: &Gateway,
        conversation_id: &str,
        person: &ai6::Person,
        token: &str,
        proposal_id: &str,
    ) -> Result<(), (&'static str, String)> {
        let draft = {
            let mut desk = self.desk();
            let conversation = desk
                .store
                .owned(conversation_id, &person.id)
                .ok_or(("not_found", "The conversation is gone.".to_owned()))?;
            expire_proposals(conversation);
            let proposal = conversation
                .proposals
                .iter_mut()
                .find(|proposal| proposal.id == proposal_id)
                .ok_or(("not_found", "There is no such proposal.".to_owned()))?;
            if proposal.status != "pending" {
                return Err(("conflict", format!("The proposal is {}.", proposal.status)));
            }
            if proposal.session != digest(token) {
                return Err((
                    "forbidden",
                    "Only the signed-in session it was proposed in can confirm it.".to_owned(),
                ));
            }
            proposal.status = "confirming";
            let shown = proposal.clone();
            conversation.show_proposal(&shown);
            shown
        };
        if let Some(commit) = draft.work.clone() {
            self.confirm_commit(gateway, conversation_id, person, &draft, commit)
                .await;
            return Ok(());
        }
        // A held Cable is sent by the Station that holds it, which reports how it went.
        if let Some(hold) = draft.cable.clone() {
            self.confirm_cable(gateway, conversation_id, person, &draft, hold)
                .await;
            return Ok(());
        }
        let outcome = self
            .execute(gateway, conversation_id, person, token, &draft)
            .await;
        let (status, result) = match &outcome {
            Ok(text) => ("done", text.clone()),
            Err(text) => ("failed", text.clone()),
        };
        let note = format!(
            "[App: the person confirmed {} ({}). {}]",
            draft.id, draft.draft.summary, result
        );
        let idle = {
            let mut desk = self.desk();
            let Some(conversation) = desk.store.owned(conversation_id, &person.id) else {
                return Ok(());
            };
            if let Some(proposal) = conversation
                .proposals
                .iter_mut()
                .find(|proposal| proposal.id == proposal_id)
            {
                proposal.status = status;
                proposal.result = Some(result);
                let shown = proposal.clone();
                conversation.show_proposal(&shown);
            }
            if conversation.busy {
                conversation.notes.push(note.clone());
                false
            } else {
                true
            }
        };
        // The result goes back to Moneypenny: now when she is idle, else with the next message.
        if idle {
            let _ = self
                .queue(gateway, conversation_id, &person.id, &note, None)
                .await;
        }
        Ok(())
    }

    /// Audits a confirmed commit proposal, then asks a Station to make the commit; the
    /// Station's answer settles the proposal ([`Self::report`]). One that cannot be audited
    /// or sent fails here.
    async fn confirm_commit(
        &self,
        gateway: &Gateway,
        conversation_id: &str,
        person: &ai6::Person,
        proposal: &actions::Proposal,
        commit: AssistantCommit,
    ) {
        let target = format!(
            "{conversation_id} {} {} {}",
            proposal.id, proposal.tool, proposal.draft.call.path
        );
        let sent = match audit(
            gateway,
            person,
            "moneypenny.confirmed",
            String::new(),
            Some(ASSISTANT_AUDIT_RESOURCE.to_owned()),
            &target,
        )
        .await
        {
            Ok(()) => self
                .commit_station(gateway, conversation_id)
                .await
                .ok_or_else(|| "no Station is checking in".to_owned()),
            Err(reason) => Err(reason),
        };
        let mut desk = self.desk();
        match sent {
            Ok(station) => {
                desk.queue_commit(
                    conversation_id,
                    &self.config.agent_id,
                    station,
                    commit,
                    Control::Commit {
                        proposal: Some(proposal.id.clone()),
                        what: proposal.draft.summary.clone(),
                    },
                );
                drop(desk);
                self.ready.notify_waiters();
            }
            Err(reason) => {
                if let Some(conversation) = desk.store.owned(conversation_id, &person.id) {
                    settle(conversation, &proposal.id, false, &reason);
                }
            }
        }
    }

    /// The Station a commit of `conversation_id` goes to: the one running its harness when
    /// it is healthy, else the first healthy one.
    async fn commit_station(&self, gateway: &Gateway, conversation_id: &str) -> Option<String> {
        let preferred = self
            .desk()
            .store
            .conversations
            .get(conversation_id)
            .and_then(|conversation| conversation.station.clone());
        let healthy = healthy_stations(gateway).await;
        preferred
            .filter(|station| healthy.contains(station))
            .or_else(|| healthy.into_iter().next())
    }

    /// Marks the conversation exposed for good: it read text written outside AI6's own
    /// records. `Conversation.exposed` is the one record of it; jobs carry it to the Station.
    pub(crate) fn mark_exposed(&self, conversation_id: &str) {
        if let Some(conversation) = self.desk().store.conversations.get_mut(conversation_id) {
            conversation.exposed = true;
        }
    }

    /// The conversation `station` runs, whether it is exposed after `mark` marks it; `404`
    /// for one this gateway does not hold, `403` for one another Station runs.
    fn run_by(&self, station: &str, conversation_id: &str, mark: bool) -> Result<bool, ApiError> {
        let mut desk = self.desk();
        let conversation = desk
            .store
            .conversations
            .get_mut(conversation_id)
            .ok_or_else(ApiError::not_found)?;
        if conversation.station.as_deref() != Some(station) {
            return Err(ApiError::forbidden());
        }
        conversation.exposed |= mark;
        Ok(conversation.exposed)
    }

    /// Runs [`actions::SHARED_CHANGE`]: a change to one file of a shared repository of a
    /// group the person is in, proposed, or sent at once (audited `moneypenny.shared.write`)
    /// when the deployment lets that group's changes through.
    pub(crate) async fn propose_shared(
        &self,
        gateway: &Gateway,
        granted: &Granted,
        input: &Value,
    ) -> tools::Outcome {
        let (group, path, content, message) = match actions::shared_change(input) {
            Ok(change) => change,
            Err(outcome) => return outcome,
        };
        let Ok(person) = resolve_user(gateway, &granted.bearer).await else {
            return tools::Outcome::error("The person's sign-in has ended; nothing was proposed.");
        };
        let settings = match assistant_settings(gateway).await {
            Ok(settings) => settings,
            Err(reason) => return tools::Outcome::error(format!("Nothing was proposed: {reason}")),
        };
        let Some(shared) = settings.shared_named(group, &person.groups) else {
            return tools::Outcome::error(format!(
                "Refused: the person is not a member of {group:?}, or it shares no repository, \
                 so nothing was proposed. Tell them so; do not try another way."
            ));
        };
        let (author_name, author_email) = author_of(&person);
        let commit = AssistantCommit {
            kind: COMMIT_SHARED.to_owned(),
            repo: shared.repo().to_owned(),
            change: AssistantChange::new(std::collections::BTreeMap::from([(
                path.to_owned(),
                Some(content.to_owned()),
            )])),
            message: format!("{message}\n\n{}", trailer(&granted.conversation_id)),
            author_name,
            author_email,
        };
        let summary = format!("Write {path} in {group}'s shared repository: {message}");
        if shared.write() {
            let target = format!("{} {group} {path}", granted.conversation_id);
            if let Err(reason) = audit(
                gateway,
                &person,
                "moneypenny.shared.write",
                String::new(),
                Some(ASSISTANT_AUDIT_RESOURCE.to_owned()),
                &target,
            )
            .await
            {
                return tools::Outcome::error(format!("Nothing was written: {reason}."));
            }
            let Some(station) = self.commit_station(gateway, &granted.conversation_id).await else {
                return tools::Outcome::error("Nothing was written: no Station is checking in.");
            };
            let mut desk = self.desk();
            desk.queue_commit(
                &granted.conversation_id,
                &self.config.agent_id,
                station,
                commit,
                Control::Commit {
                    proposal: None,
                    what: summary,
                },
            );
            drop(desk);
            self.ready.notify_waiters();
            return tools::Outcome {
                text: format!(
                    "Sent to be committed to {group}'s repository at once: the deployment lets \
                     {group}'s changes through without a confirmation. You will be told how it \
                     went."
                ),
                is_error: false,
            };
        }
        let proposal = commit_proposal(
            actions::SHARED_CHANGE,
            digest(&granted.bearer),
            format!("{group} shared repository: {path}"),
            actions::Scope::Resource(format!("group:{group}")),
            "membership",
            summary.clone(),
            commit,
        );
        let id = proposal.id.clone();
        let bounds = self.config.bounds();
        let mut desk = self.desk();
        let Some(conversation) = desk
            .store
            .owned(&granted.conversation_id, &granted.person_id)
        else {
            return tools::Outcome::error("The conversation is gone.");
        };
        if !record_proposal(conversation, proposal, bounds) {
            return tools::Outcome::error(PROPOSALS_FULL);
        }
        tools::Outcome {
            text: format!(
                "Proposed as {id}: {summary}. Nothing has changed. The person confirms or \
                 cancels it in the app; you will be told which."
            ),
            is_error: false,
        }
    }

    /// Audits, then sends, a confirmed proposal's request as the person; the text is what
    /// AI6 answered, wrapped as data, or why it did not run.
    async fn execute(
        &self,
        gateway: &Gateway,
        conversation_id: &str,
        person: &ai6::Person,
        token: &str,
        proposal: &actions::Proposal,
    ) -> Result<String, String> {
        let (operation, resource) = match &proposal.draft.scope {
            actions::Scope::Operation(operation) => (operation.clone(), None),
            actions::Scope::Resource(resource) => (String::new(), Some(resource.clone())),
        };
        let target = format!(
            "{conversation_id} {} {} {}",
            proposal.id, proposal.tool, proposal.draft.call.path
        );
        // Recorded before it runs: a confirmation that cannot be audited is not carried out.
        audit(
            gateway,
            person,
            "moneypenny.confirmed",
            operation,
            resource,
            &target,
        )
        .await?;
        match tools::route(gateway, token, &proposal.draft.call).await {
            Ok((status, value)) if status.is_success() => Ok(format!(
                "AI6 answered {}: {}",
                status.as_u16(),
                tools::wrap(proposal.tool, &value)
            )),
            Ok((status, value)) => Err(tools::refusal(status, &value)),
            Err(error) => Err(format!("The request failed: {error}")),
        }
    }

    /// Shows a view Moneypenny built, when it is exactly the view schema; otherwise tells her
    /// why and shows nothing.
    pub(crate) fn show(&self, granted: &Granted, input: &Value) -> tools::Outcome {
        let view = match views::parse(input) {
            Ok(view) => view,
            Err(reason) => {
                return tools::Outcome::error(format!(
                    "View refused: {reason}. Nothing was shown; fix the view and call show again."
                ));
            }
        };
        let id = format!("view-{}", Uuid::new_v4().simple());
        let max_entries = self.config.bounds();
        let mut desk = self.desk();
        let Some(conversation) = desk
            .store
            .owned(&granted.conversation_id, &granted.person_id)
        else {
            return tools::Outcome::error("The conversation is gone.");
        };
        conversation.push(
            Entry {
                role: Role::View,
                text: view.title.clone().unwrap_or_default(),
                at: now(),
                data: Some(json!({"id": id, "view": view})),
            },
            max_entries,
        );
        desk.store
            .trim_person(&granted.person_id, self.config.person_bytes);
        tools::Outcome {
            text: format!("Shown to the person as {id}."),
            is_error: false,
        }
    }

    /// Turns a submitted form into a proposal of its tool, made in this socket's session.
    async fn submit(
        &self,
        gateway: &Gateway,
        conversation_id: &str,
        person_id: &str,
        token: &str,
        view_id: &str,
        values: &serde_json::Map<String, Value>,
    ) -> Result<(), (&'static str, String)> {
        let view = {
            let mut desk = self.desk();
            let conversation = desk
                .store
                .owned(conversation_id, person_id)
                .ok_or(("not_found", "The conversation is gone.".to_owned()))?;
            let id = Value::String(view_id.to_owned());
            conversation
                .entries
                .iter()
                .filter(|entry| entry.role == Role::View)
                .filter_map(|entry| entry.data.as_ref())
                .find(|data| data.get("id") == Some(&id))
                .and_then(|data| serde_json::from_value::<views::View>(data["view"].clone()).ok())
                .ok_or(("not_found", "There is no such view.".to_owned()))?
        };
        let (tool, input) =
            views::form_input(&view, values).map_err(|reason| ("bad_request", reason))?;
        let tool = actions::write_tools()
            .iter()
            .find(|write| write.name == tool)
            .ok_or(("bad_request", "The form names no tool.".to_owned()))?;
        let granted = Granted {
            conversation_id: conversation_id.to_owned(),
            person_id: person_id.to_owned(),
            bearer: token.to_owned(),
        };
        let outcome = self.propose(gateway, &granted, tool, &input).await;
        if outcome.is_error {
            return Err(("refused", outcome.text));
        }
        if let Some(conversation) = self.desk().store.owned(conversation_id, person_id) {
            conversation.notes.push(format!(
                "[App: the person submitted the form of {view_id}. {}]",
                outcome.text
            ));
        }
        Ok(())
    }
    /// Cancels a pending proposal; Moneypenny hears of it with the next message.
    fn cancel(
        &self,
        conversation_id: &str,
        person_id: &str,
        proposal_id: &str,
    ) -> Result<(), (&'static str, String)> {
        let mut desk = self.desk();
        let conversation = desk
            .store
            .owned(conversation_id, person_id)
            .ok_or(("not_found", "The conversation is gone.".to_owned()))?;
        let proposal = conversation
            .proposals
            .iter_mut()
            .find(|proposal| proposal.id == proposal_id && proposal.status == "pending")
            .ok_or(("not_found", "There is no such pending proposal.".to_owned()))?;
        proposal.status = "cancelled";
        let shown = proposal.clone();
        conversation.notes.push(format!(
            "[App: the person cancelled {proposal_id} ({}).]",
            shown.draft.summary
        ));
        conversation.show_proposal(&shown);
        drop(desk);
        // A held Cable is dropped by the Station that holds it.
        if let Some(hold) = shown.cable {
            self.discard_cable(conversation_id, proposal_id, hold);
        }
        Ok(())
    }

    /// Stops the answer the person's conversation waits for: a message no harness has yet is
    /// withdrawn, and the Station whose harness is answering is asked to interrupt it, which it
    /// reports as `interrupted`. `Ok(false)` when nothing is being answered.
    fn interrupt(
        &self,
        conversation_id: &str,
        person_id: &str,
    ) -> Result<bool, (&'static str, String)> {
        let bounds = self.config.bounds();
        let mut desk = self.desk();
        let conversation = desk
            .store
            .owned(conversation_id, person_id)
            .ok_or(("not_found", "The conversation is gone.".to_owned()))?;
        if !conversation.busy {
            return Ok(false);
        }
        conversation
            .notes
            .push("[App: the person stopped your last answer.]".to_owned());
        let station = conversation.station.clone();
        // A Station that already took a withdrawn message has its late `accepted` refused,
        // which makes it close the harness it started.
        if conversation.pending.take().is_some() || station.is_none() {
            stopped(conversation, bounds);
            return Ok(true);
        }
        let job = control_job(
            conversation,
            &self.config.agent_id,
            &Control::Interrupt,
            None,
        );
        let asked = desk
            .controls
            .iter()
            .any(|(_, queued)| queued.interrupt && queued.conversation_id == conversation_id);
        if !asked && let Some(station) = station {
            desk.controlling
                .insert(job.job_id.clone(), (station.clone(), Control::Interrupt));
            desk.controls.push((station, job));
            drop(desk);
            self.ready.notify_waiters();
        }
        Ok(true)
    }

    /// Counts one tool call of the conversation; `false` past the minute's allowance.
    pub(crate) fn count_call(&self, conversation_id: &str) -> bool {
        let mut desk = self.desk();
        let limit = self.config.tool_calls_per_minute;
        desk.calls
            .retain(|_, (started, _)| started.elapsed() < Duration::from_secs(60));
        let (_, calls) = desk
            .calls
            .entry(conversation_id.to_owned())
            .or_insert_with(|| (Instant::now(), 0));
        *calls += 1;
        *calls <= limit
    }
}

/// Records `action` by `person` in the audit log; `Err` says that nothing may run.
async fn audit(
    gateway: &Gateway,
    person: &ai6::Person,
    action: &str,
    operation: String,
    resource: Option<String>,
    target: &str,
) -> Result<(), String> {
    let registry = gateway
        .registry
        .clone()
        .ok_or("this gateway keeps no audit log, so it ran nothing")?;
    let record = ai6::AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: action.to_owned(),
        operation,
        resource,
        target: cut(target, 512),
        at: now(),
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| "it could not be audited, so it did not run".to_owned())?
        .map_err(|_| "it could not be audited, so it did not run".to_owned())
}

/// Adds `proposal` to `conversation`, after expired ones are marked and room is made, and
/// shows it.
fn record_proposal(
    conversation: &mut Conversation,
    proposal: actions::Proposal,
    bounds: Bounds,
) -> bool {
    if !room_for_proposal(conversation) {
        return false;
    }
    conversation.push(
        Entry {
            role: Role::Proposal,
            text: proposal.draft.summary.clone(),
            at: proposal.created_at,
            data: Some(proposal.shown()),
        },
        bounds,
    );
    conversation.proposals.push(proposal);
    true
}

/// What she is told when a proposal does not fit.
const PROPOSALS_FULL: &str = "32 proposals already wait for the person, so nothing was \
                              proposed. Ask them to confirm or cancel some first.";

/// Drops finished proposals until one more fits; `false` when every one kept still waits for
/// the person or is being carried out, which are never dropped.
fn room_for_proposal(conversation: &mut Conversation) -> bool {
    expire_proposals(conversation);
    while conversation.proposals.len() >= actions::MAX_PROPOSALS {
        let Some(finished) = conversation
            .proposals
            .iter()
            .position(|proposal| !matches!(proposal.status, "pending" | "confirming"))
        else {
            return false;
        };
        conversation.proposals.remove(finished);
    }
    true
}

/// A proposal whose confirmation has a Station make `commit`: `request` describes the commit
/// for the person (its message, every file's new text), `needs` what it rests on.
fn commit_proposal(
    tool: &'static str,
    session: String,
    target: String,
    scope: actions::Scope,
    needs: &'static str,
    summary: String,
    commit: AssistantCommit,
) -> actions::Proposal {
    actions::Proposal {
        id: format!("proposal-{}", Uuid::new_v4().simple()),
        tool,
        draft: actions::Draft {
            call: tools::Call {
                method: axum::http::Method::from_bytes(b"COMMIT")
                    .unwrap_or(axum::http::Method::POST),
                path: target,
                body: Some(json!({
                    "message": commit.message,
                    "files": commit.change.files,
                })),
            },
            scope,
            action: needs,
            summary,
        },
        status: "pending",
        created_at: now(),
        session,
        result: None,
        work: Some(commit),
        cable: None,
    }
}

/// Who a valid MCP token speaks for.
pub(crate) struct Granted {
    pub(crate) conversation_id: String,
    pub(crate) person_id: String,
    /// The person's access token, with which the tools run.
    pub(crate) bearer: String,
}

/// Marks proposals older than their lifetime expired.
fn expire_proposals(conversation: &mut Conversation) {
    let now = now();
    let mut expired = Vec::new();
    for proposal in &mut conversation.proposals {
        if proposal.status == "pending" && now - proposal.created_at > actions::PROPOSAL_TTL_SECS {
            proposal.status = "expired";
            expired.push(proposal.clone());
        }
    }
    for proposal in expired {
        conversation.show_proposal(&proposal);
    }
}

/// What the app is shown when it opens a conversation.
///
/// Only the newest entries that fit [`READY_ENTRIES`] and [`READY_BYTES`] are sent; `earlier`
/// counts those left out.
fn ready(conversation: &Conversation) -> Value {
    let mut shown = Vec::new();
    let mut bytes = 0;
    for entry in conversation.entries.iter().rev().take(READY_ENTRIES) {
        bytes += store::entry_bytes(entry);
        if bytes > READY_BYTES && !shown.is_empty() {
            break;
        }
        shown.push(entry);
    }
    shown.reverse();
    json!({
        "type": "ready",
        "conversation": conversation.id,
        "entries": shown,
        "earlier": conversation.entries.len() - shown.len(),
        "busy": conversation.busy,
    })
}

/// The end of the conversation so far, for a harness that starts without its provider
/// Session's own memory of it.
fn recap(conversation: &Conversation) -> Option<String> {
    let mut lines = Vec::new();
    let mut size = 0;
    for entry in conversation.entries.iter().rev() {
        let who = match entry.role {
            Role::Person => "The person",
            Role::Moneypenny => "You",
            _ => continue,
        };
        let line = format!("{who}: {}", entry.text);
        size += line.len();
        if size > MAX_RECAP_BYTES {
            break;
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    let recap = format!(
        "Earlier in this conversation (a recap; the app restarted your session):\n\n{}",
        lines.join("\n\n")
    );
    (recap.len() < MAX_ASSISTANT_CONTEXT_BYTES / 2).then_some(recap)
}

/// The line that tells Moneypenny which app page the person is on, from the page's known
/// keys only, each a short identifier.
fn page_line(page: &Value) -> Option<String> {
    let page = page.as_object()?;
    let mut parts = Vec::new();
    for key in [
        "view",
        "operation_id",
        "mission_id",
        "station_id",
        "session_id",
    ] {
        if let Some(value) = page.get(key).and_then(Value::as_str)
            && value.len() <= 128
            && !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
        {
            parts.push(format!("{key}={value}"));
        }
    }
    (!parts.is_empty()).then(|| format!("[App page: {}]", parts.join(" ")))
}

fn cut(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Records one harness event in the conversation.
fn apply(conversation: &mut Conversation, event: &AssistantEvent, max_entries: Bounds) {
    let at = now();
    let text = event.text.clone().unwrap_or_default();
    let entry = |role, text: String| Entry {
        role,
        text,
        at,
        data: None,
    };
    match event.kind.as_str() {
        EVENT_TEXT if !text.trim().is_empty() => {
            conversation.answered = true;
            conversation.push(entry(Role::Moneypenny, text), max_entries);
        }
        EVENT_TOOL => {
            let tools = event.tools.join(", ");
            conversation.push(entry(Role::Tools, tools), max_entries);
            let reach = conversation.scripts_reach_hosts;
            if let Some(tool) = event.tools.iter().find(|tool| outside_tool(tool, reach))
                && !conversation.exposed
            {
                conversation.exposed = true;
                conversation.push(
                    entry(
                        Role::Notice,
                        format!(
                            "Moneypenny read text from outside AI6 ({}): from now on her memory \
                             changes and Cables wait for your confirmation.",
                            cut(tool, 200)
                        ),
                    ),
                    max_entries,
                );
            }
        }
        EVENT_COMPLETED => {
            if !conversation.answered && !text.trim().is_empty() {
                conversation.push(entry(Role::Moneypenny, text), max_entries);
            }
            conversation.answered = false;
            conversation.set_busy(false);
        }
        EVENT_FAILED | EVENT_LIMIT => {
            let what = if event.kind == EVENT_LIMIT {
                "Moneypenny's model account is out of quota for now".to_owned()
            } else {
                format!("Moneypenny stopped: {}", or_unknown(&text))
            };
            conversation.live = false;
            conversation.push(entry(Role::Error, what), max_entries);
            conversation.set_busy(false);
        }
        // What she and the person should know about her workspace.
        EVENT_NOTICE if !text.trim().is_empty() => {
            conversation.notes.push(format!("[Station: {text}]"));
            conversation.push(entry(Role::Notice, text), max_entries);
        }
        // Only the answer being given can be the one stopped; a later message is not.
        EVENT_INTERRUPTED if conversation.busy && conversation.pending.is_none() => {
            stopped(conversation, max_entries);
        }
        EVENT_CLOSED if !conversation.busy || conversation.pending.is_some() => {
            // An idle harness closed, or the one a new job replaces.
            conversation.live = false;
        }
        EVENT_CLOSED => {
            conversation.live = false;
            conversation.push(
                entry(
                    Role::Error,
                    format!(
                        "Moneypenny's session closed before she answered: {}",
                        or_unknown(&text)
                    ),
                ),
                max_entries,
            );
            conversation.set_busy(false);
        }
        _ => {}
    }
}

/// Records a Station's answer to a commit job: an entry for the person, then the proposal it
/// carried out settled, or a note telling Moneypenny with her next message.
fn committed(
    conversation: &mut Conversation,
    proposal: Option<&str>,
    what: &str,
    report: &AssistantReport,
    bounds: Bounds,
) {
    let answer = report.events.first();
    let done = answer.is_some_and(|event| event.kind == EVENT_ACCEPTED);
    let text = answer
        .and_then(|event| event.text.clone())
        .unwrap_or_default();
    let line = if done {
        format!("{what}: {}.", or_unknown(&text))
    } else {
        format!("{what} failed: {}.", or_unknown(&text))
    };
    conversation.push(
        Entry {
            role: if done { Role::Notice } else { Role::Error },
            text: line.clone(),
            at: now(),
            data: None,
        },
        bounds,
    );
    match proposal {
        Some(id) => settle(conversation, id, done, or_unknown(&text)),
        None => conversation.notes.push(format!("[Station: {line}]")),
    }
}

/// Ends the answer the person stopped, and shows that it was stopped.
fn stopped(conversation: &mut Conversation, bounds: Bounds) {
    conversation.pending = None;
    conversation.answered = false;
    conversation.push(
        Entry {
            role: Role::Notice,
            text: "You stopped Moneypenny's answer.".to_owned(),
            at: now(),
            data: None,
        },
        bounds,
    );
    conversation.set_busy(false);
}

fn or_unknown(text: &str) -> &str {
    if text.trim().is_empty() {
        "no reason given"
    } else {
        text
    }
}

/// Healthy Stations, the Headquarters Station first, then by ID.
async fn healthy_stations(gateway: &Gateway) -> Vec<String> {
    let Some(registry) = gateway.registry.clone() else {
        return Vec::new();
    };
    let found = tokio::task::spawn_blocking(move || {
        let stations = registry.stations(Duration::MAX).ok()?;
        let headquarters = registry.headquarters().ok().flatten();
        Some((stations, headquarters))
    })
    .await
    .ok()
    .flatten();
    let Some((stations, headquarters)) = found else {
        return Vec::new();
    };
    let now = now();
    let dark = ai6::GONE_DARK_AFTER.as_secs_f64();
    let leader = headquarters
        .filter(|lease| lease.expires() > now)
        .map(|lease| lease.station().to_owned());
    let mut healthy: Vec<String> = stations
        .iter()
        .filter(|station| now - station.seen() <= dark)
        .map(|station| station.station().to_owned())
        .collect();
    healthy.sort_by_key(|station| (Some(station) != leader.as_ref(), station.clone()));
    healthy
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum FromApp {
    Hello {
        token: String,
        #[serde(default)]
        conversation: Option<String>,
    },
    Message {
        text: String,
        #[serde(default)]
        page: Option<Value>,
    },
    New {},
    Confirm {
        proposal: String,
    },
    Cancel {
        proposal: String,
    },
    Submit {
        view: String,
        values: serde_json::Map<String, Value>,
    },
    Interrupt {},
}

fn frame(value: &Value) -> Message {
    Message::Text(value.to_string().into())
}

fn problem(code: &str, message: &str) -> Value {
    json!({"type": "error", "code": code, "message": message})
}

/// `GET /v1/user/moneypenny`: the app's conversation with Moneypenny. The first frame is
/// `{"type":"hello","token":ACCESS_TOKEN,"conversation"?:ID}`; the token is the person's
/// access token, sent in the socket rather than a header a browser cannot set.
pub(crate) async fn socket(State(gateway): State<Gateway>, upgrade: WebSocketUpgrade) -> Response {
    if gateway.assistant.is_none() {
        return ApiError::unavailable().into_response();
    }
    upgrade
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| converse(socket, gateway))
}

async fn converse(socket: WebSocket, gateway: Gateway) {
    let Some(assistant) = gateway.assistant.clone() else {
        return;
    };
    let (mut sender, mut receiver) = socket.split();
    let hello = tokio::time::timeout(HELLO_TIMEOUT, receiver.next()).await;
    let Ok(Some(Ok(Message::Text(hello)))) = hello else {
        let _ = sender.send(close(1008, "say hello first")).await;
        return;
    };
    let Ok(FromApp::Hello {
        token,
        conversation,
    }) = serde_json::from_str::<FromApp>(&hello)
    else {
        let _ = sender.send(close(1008, "say hello first")).await;
        return;
    };
    let Ok(person) = resolve_user(&gateway, &token).await else {
        let _ = sender
            .send(frame(&problem(
                "unauthorized",
                "Sign in again to talk to Moneypenny.",
            )))
            .await;
        let _ = sender.send(close(1008, "unauthorized")).await;
        return;
    };
    let (mut conversation_id, shown, mut events) =
        assistant.open(&person.id, &token, conversation.as_deref());
    if sender.send(frame(&shown)).await.is_err() {
        return;
    }
    let mut sweep = tokio::time::interval(SWEEP_EVERY);
    loop {
        tokio::select! {
            _ = sweep.tick() => assistant.sweep(&gateway).await,
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<FromApp>(&text) {
                        Ok(FromApp::Message { text, page }) => {
                            if let Err((code, message)) = assistant
                                .message(&gateway, &conversation_id, &person.id, &text, page.as_ref())
                                .await
                                && sender.send(frame(&problem(code, &message))).await.is_err()
                            {
                                break;
                            }
                        }
                        Ok(FromApp::Confirm { proposal }) => {
                            if let Err((code, message)) = assistant
                                .confirm(&gateway, &conversation_id, &person, &token, &proposal)
                                .await
                                && sender.send(frame(&problem(code, &message))).await.is_err()
                            {
                                break;
                            }
                        }
                        Ok(FromApp::Cancel { proposal }) => {
                            if let Err((code, message)) =
                                assistant.cancel(&conversation_id, &person.id, &proposal)
                                && sender.send(frame(&problem(code, &message))).await.is_err()
                            {
                                break;
                            }
                        }
                        Ok(FromApp::Submit { view, values }) => {
                            if let Err((code, message)) = assistant
                                .submit(&gateway, &conversation_id, &person.id, &token, &view, &values)
                                .await
                                && sender.send(frame(&problem(code, &message))).await.is_err()
                            {
                                break;
                            }
                        }
                        Ok(FromApp::Interrupt {}) => {
                            let answer = match assistant.interrupt(&conversation_id, &person.id) {
                                Ok(stopping) => json!({"type": "interrupt", "stopping": stopping}),
                                Err((code, message)) => problem(code, &message),
                            };
                            if sender.send(frame(&answer)).await.is_err() {
                                break;
                            }
                        }
                        Ok(FromApp::New {}) => {
                            let (id, shown, subscription) = assistant.open(&person.id, &token, None);
                            conversation_id = id;
                            events = subscription;
                            if sender.send(frame(&shown)).await.is_err() {
                                break;
                            }
                        }
                        Ok(FromApp::Hello { .. }) | Err(_) => {
                            if sender
                                .send(frame(&problem("bad_request", "That frame is not understood.")))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    if sender.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Binary(_))) => {
                    let _ = sender.send(close(1003, "text frames only")).await;
                    break;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
            },
            event = events.recv() => match event {
                Ok(event) => {
                    if sender.send(frame(&event)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let _ = sender.send(close(1013, "fell behind; reopen the conversation")).await;
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

fn close(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

#[derive(Deserialize)]
pub(crate) struct NextQuery {
    station: String,
    wait_ms: Option<u64>,
}

/// `GET /v1/station/assistant/next?station=ID&wait_ms=N`: the next message for the
/// Station's assistant desk (`200` and an [`AssistantJob`]), or `204` after `wait_ms`
/// (default 1,000, at most 25,000).
pub(crate) async fn next(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<NextQuery>,
) -> Result<Response, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    if !ai6::directorates::section_00::assistant::valid_identity(&query.station) {
        return Err(ApiError::bad_request());
    }
    let Some(assistant) = gateway.assistant.clone() else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    let wait = Duration::from_millis(query.wait_ms.unwrap_or(1_000).min(MAX_WAIT_MS));
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let notified = assistant.ready.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if let Some(job) = assistant.take(&gateway, &query.station).await {
            return Ok(Json(job).into_response());
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct ExposureQuery {
    station: String,
    conversation: String,
    /// An Operation whose Cable permissions the Station asks about too.
    #[serde(default)]
    operation: Option<String>,
}

/// `GET /v1/station/assistant/exposure?station=ID&conversation=ID[&operation=ID]`:
/// `{"exposed": bool}` for a conversation the Station runs, with an Operation also `"send"`
/// and `"read"`, whether its person holds `cable.send` and `cable.read` there now; `404` for
/// one this gateway does not hold, `403` for one another Station runs.
pub(crate) async fn exposure(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<ExposureQuery>,
) -> Result<Json<Value>, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    let assistant = gateway.assistant.clone().ok_or_else(ApiError::not_found)?;
    let exposed = assistant.run_by(&query.station, &query.conversation, false)?;
    let Some(operation) = query.operation else {
        return Ok(Json(json!({"exposed": exposed})));
    };
    let (send, read) = assistant
        .cable_permissions(&gateway, &query.conversation, &operation)
        .await;
    Ok(Json(
        json!({"exposed": exposed, "send": send, "read": read}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Exposed {
    station: String,
    conversation: String,
    /// Where the outside text came from, such as `personal:notes/a.md`.
    source: String,
}

/// `POST /v1/station/assistant/exposure` with `{"station", "conversation", "source"}`: marks a
/// conversation the Station runs exposed for good, `204`; `404` and `403` as for `GET`. The
/// person is shown a notice naming the source the first time.
pub(crate) async fn expose(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(exposed): Json<Exposed>,
) -> Result<StatusCode, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    if !ai6::directorates::section_00::assistant::valid_identity(&exposed.source) {
        return Err(ApiError::bad_request());
    }
    let assistant = gateway.assistant.clone().ok_or_else(ApiError::not_found)?;
    let was = assistant.run_by(&exposed.station, &exposed.conversation, false)?;
    assistant.run_by(&exposed.station, &exposed.conversation, true)?;
    if !was {
        let bounds = assistant.config.bounds();
        if let Some(conversation) = assistant
            .desk()
            .store
            .conversations
            .get_mut(&exposed.conversation)
        {
            conversation.push(
                Entry {
                    role: Role::Notice,
                    text: format!(
                        "Moneypenny read text written outside AI6's own records ({}): from now \
                         on her memory changes and Cables wait for your confirmation.",
                        cut(&exposed.source, 200)
                    ),
                    at: now(),
                    data: None,
                },
                bounds,
            );
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/station/assistant/reports`: what a Station's assistant harness did. `204`;
/// `404` for a conversation this gateway does not hold, `409` for a job it no longer waits
/// for, `403` for a report from a Station that does not run the conversation.
pub(crate) async fn reports(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(report): Json<AssistantReport>,
) -> Result<StatusCode, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    let Some(assistant) = gateway.assistant.clone() else {
        return Err(ApiError::not_found());
    };
    match assistant.answer_memory(&report) {
        Some(Ok(())) => return Ok(StatusCode::NO_CONTENT),
        Some(Err(_)) => return Err(ApiError::forbidden()),
        None => {}
    }
    match assistant.report(&gateway, report).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(StatusCode::NOT_FOUND) => Err(ApiError::not_found()),
        Err(StatusCode::FORBIDDEN) => Err(ApiError::forbidden()),
        Err(StatusCode::CONFLICT) => Err(ApiError::from(GatewayError::Conflict)),
        Err(_) => Err(ApiError::bad_request()),
    }
}

#[cfg(test)]
mod tests;
