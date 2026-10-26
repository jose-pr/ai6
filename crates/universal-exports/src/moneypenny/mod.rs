//! Moneypenny, the operator's assistant, as Universal Exports serves her.
//!
//! The model runs on a Station, through the same AgentHandler providers as every Agent: the
//! roster's assistant Agent (`moneypenny` unless configured otherwise) with its handler,
//! model, effort and model credentials. Universal Exports keeps the conversations, relays
//! each message to a Station as an [`ai6::AssistantJob`] and what the harness says back to
//! the app, and serves her tools on an MCP endpoint of its own. A tool runs as the person
//! who is chatting, through the same routes and authorization as the app.
//!
//! Routes: `GET /v1/user/moneypenny` (the app's WebSocket), `POST /v1/moneypenny/mcp` (the
//! harness's tools, with a token bound to the person and the conversation),
//! `GET /v1/station/assistant/next` and `POST /v1/station/assistant/reports` (the Stations).

pub(crate) mod actions;
mod guide;
pub(crate) mod mcp;
mod store;
pub(crate) mod tools;
pub(crate) mod views;

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ai6::{
    AssistantEvent, AssistantJob, AssistantReport, EVENT_ACCEPTED, EVENT_CLOSED, EVENT_COMPLETED,
    EVENT_FAILED, EVENT_LIMIT, EVENT_REJECTED, EVENT_TEXT, EVENT_TOOL, MAX_ASSISTANT_CONTEXT_BYTES,
    MAX_ASSISTANT_EVENTS, MAX_ASSISTANT_MESSAGE_BYTES,
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
use store::{Conversation, Entry, Pending, Role, Store};

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
    tool_calls_per_minute: u32,
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
            tool_calls_per_minute: 120,
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

    /// How many tool calls one conversation may make in a minute (default 120).
    #[must_use]
    pub fn with_tool_calls_per_minute(mut self, calls: u32) -> Self {
        self.tool_calls_per_minute = calls.max(1);
        self
    }
}

/// The token a harness presents, and whose conversation it is.
struct Grant {
    conversation_id: String,
    person_id: String,
    expires_at: f64,
}

#[derive(Default)]
struct Desk {
    store: Store,
    /// Tokens by their SHA-256, so the token itself is never kept.
    tokens: HashMap<String, Grant>,
    /// Tool calls in the current minute, by conversation.
    calls: HashMap<String, (Instant, u32)>,
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

/// The most tokens one conversation holds; a harness uses the newest, older ones age out.
const MAX_TOKENS_PER_CONVERSATION: usize = 8;
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
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let now = now();
        let expires_at = now + self.config.token_ttl.as_secs_f64();
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
            let mut context = guide::context();
            if let Some(recap) = recap(conversation) {
                context.push(recap);
            }
            let mut parts = std::mem::take(&mut conversation.notes);
            parts.push(message.to_owned());
            let job = AssistantJob {
                job_id: Uuid::new_v4().to_string(),
                conversation_id: conversation_id.to_owned(),
                agent_id: self.config.agent_id.clone(),
                message: cut(&parts.join("\n\n"), MAX_ASSISTANT_MESSAGE_BYTES),
                context,
                mcp_token: token.clone(),
                token_expires_at: expires_at,
                session_id: conversation.session_id.clone(),
                close: false,
            };
            if let Some(text) = shown {
                conversation.push(
                    Entry {
                        role: Role::Person,
                        text: text.to_owned(),
                        at: now,
                        data: None,
                    },
                    self.config.max_entries,
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
            let grant = Grant {
                conversation_id: conversation_id.to_owned(),
                person_id: person_id.to_owned(),
                expires_at,
            };
            desk.tokens.retain(|_, grant| grant.expires_at > now);
            let mut held: Vec<(String, f64)> = desk
                .tokens
                .iter()
                .filter(|(_, held)| held.conversation_id == conversation_id)
                .map(|(key, held)| (key.clone(), held.expires_at))
                .collect();
            held.sort_by(|a, b| a.1.total_cmp(&b.1));
            while held.len() >= MAX_TOKENS_PER_CONVERSATION {
                let (oldest, _) = held.remove(0);
                desk.tokens.remove(&oldest);
            }
            desk.tokens.insert(digest(&token), grant);
        }
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
        let max_entries = self.config.max_entries;
        let Some(conversation) = desk.store.conversations.get_mut(conversation_id) else {
            return;
        };
        let Some(pending) = conversation.pending.as_mut() else {
            return;
        };
        match chosen {
            Some(station) => {
                pending.station = Some(station);
                pending.taken_at = None;
                pending.queued_at = now();
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
        }
        for conversation in again {
            self.place(gateway, &conversation).await;
        }
    }

    /// The next job for `station`.
    async fn take(&self, gateway: &Gateway, station: &str) -> Option<AssistantJob> {
        self.sweep(gateway).await;
        let now = now();
        let mut desk = self.desk();
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
        let max_entries = self.config.max_entries;
        let mut refused = false;
        {
            let mut desk = self.desk();
            let conversation = desk
                .store
                .conversations
                .get_mut(&report.conversation_id)
                .ok_or(StatusCode::NOT_FOUND)?;
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
            for event in report
                .events
                .iter()
                .filter(|event| report.job_id.is_none() || event.kind != EVENT_ACCEPTED)
            {
                apply(conversation, event, max_entries);
            }
        }
        if refused {
            self.place(gateway, &report.conversation_id).await;
        }
        Ok(())
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
        };
        let max_entries = self.config.max_entries;
        {
            let mut desk = self.desk();
            let Some(conversation) = desk
                .store
                .owned(&granted.conversation_id, &granted.person_id)
            else {
                return tools::Outcome::error("The conversation is gone.");
            };
            expire_proposals(conversation);
            while conversation.proposals.len() >= actions::MAX_PROPOSALS {
                let finished = conversation
                    .proposals
                    .iter()
                    .position(|proposal| proposal.status != "pending")
                    .unwrap_or(0);
                conversation.proposals.remove(finished);
            }
            conversation.push(
                Entry {
                    role: Role::Proposal,
                    text: summary.clone(),
                    at: proposal.created_at,
                    data: Some(proposal.shown()),
                },
                max_entries,
            );
            conversation.proposals.push(proposal);
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
        let registry = gateway
            .registry
            .clone()
            .ok_or("this gateway keeps no audit log, so it ran nothing")?;
        let (operation, resource) = match &proposal.draft.scope {
            actions::Scope::Operation(operation) => (operation.clone(), None),
            actions::Scope::Resource(resource) => (String::new(), Some(resource.clone())),
        };
        let audit = ai6::AuditRecord {
            id: Uuid::new_v4().to_string(),
            actor: person.id.clone(),
            action: "moneypenny.confirmed".to_owned(),
            operation,
            resource,
            target: cut(
                &format!(
                    "{conversation_id} {} {} {}",
                    proposal.id, proposal.tool, proposal.draft.call.path
                ),
                512,
            ),
            at: now(),
        };
        // Recorded before it runs: a confirmation that cannot be audited is not carried out.
        tokio::task::spawn_blocking(move || registry.record_audit(&audit))
            .await
            .map_err(|_| "it could not be audited, so it did not run".to_owned())?
            .map_err(|_| "it could not be audited, so it did not run".to_owned())?;
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
        let max_entries = self.config.max_entries;
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
        Ok(())
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
fn ready(conversation: &Conversation) -> Value {
    json!({
        "type": "ready",
        "conversation": conversation.id,
        "entries": conversation.entries,
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
fn apply(conversation: &mut Conversation, event: &AssistantEvent, max_entries: usize) {
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
            conversation.push(entry(Role::Error, what), max_entries);
            conversation.set_busy(false);
        }
        EVENT_CLOSED if conversation.busy && conversation.pending.is_none() => {
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
