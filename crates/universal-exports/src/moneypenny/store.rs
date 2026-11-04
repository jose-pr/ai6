//! Conversations with Moneypenny, kept per person in the gateway's memory.
//!
//! A conversation remembers what was said (for the app to show again when the person comes
//! back), which Station runs its harness and which provider Session that harness is, so a
//! later message resumes it. It holds the person's access token while the person uses it:
//! the tools run with it, so they can do no more than the person could, and stop when the
//! person's sign-in ends.
//!
//! What it holds is bounded; every entry also goes to the durable log ([`super::log`]), with
//! the turn that ran it, and a conversation the gateway no longer holds is rebuilt from there.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;

use ai6::{AccountRef, AssistantJob, AssistantSharedBase, AssistantTurn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

use super::actions::{Evidence, Proposal};

/// Who said an entry, or what it records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    /// The person.
    Person,
    /// Moneypenny's text.
    Moneypenny,
    /// Tools Moneypenny called, by name.
    Tools,
    /// A failure the person should see.
    Error,
    /// A change Moneypenny proposed; `data` is the proposal, kept current.
    Proposal,
    /// A view Moneypenny showed; `data` is `{id, view}`.
    View,
    /// A page Moneypenny offered to open; `data` is `{id, route, label}`, the route in its
    /// JSON form. The person opens it; following it only reads.
    Navigation,
    /// A page Moneypenny published; `data` is its summary (`id`, `kind`, `title`, `bytes`,
    /// `expires_at`, `feeds`). The person opens it in a browser tab.
    Page,
    /// What happened to the conversation itself, such as an answer the person stopped.
    Notice,
}

/// One item of a conversation as the app shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Entry {
    pub(crate) role: Role,
    pub(crate) text: String,
    /// Unix seconds.
    pub(crate) at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<Value>,
}

/// A message on its way to a Station.
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    pub(crate) job: AssistantJob,
    /// The Station the job is queued for or was taken by.
    pub(crate) station: Option<String>,
    /// When a Station took it from its queue, Unix seconds.
    pub(crate) taken_at: Option<f64>,
    /// When it was queued for its Station, Unix seconds.
    pub(crate) queued_at: f64,
    /// Stations that refused it, and the last reason.
    pub(crate) refused_by: Vec<String>,
    pub(crate) reason: Option<String>,
}

pub(crate) struct Conversation {
    pub(crate) id: String,
    pub(crate) person_id: String,
    pub(crate) updated_at: f64,
    pub(crate) entries: VecDeque<Entry>,
    /// The Station that runs (or last ran) the harness.
    pub(crate) station: Option<String>,
    /// The provider Session of the harness, for a resume.
    pub(crate) session_id: Option<String>,
    /// The route that Session runs on, when the assistant Agent has routes.
    pub(crate) route: Option<ai6::RouteRef>,
    /// A message is being answered.
    pub(crate) busy: bool,
    /// Moneypenny wrote text since the person's last message.
    pub(crate) answered: bool,
    pub(crate) pending: Option<Pending>,
    /// The person's access token, as last presented; the tools run with it.
    pub(crate) bearer: String,
    pub(crate) events: broadcast::Sender<Value>,
    /// What Moneypenny is told with the next message (proposals confirmed or cancelled).
    pub(crate) notes: Vec<String>,
    pub(crate) proposals: Vec<Proposal>,
    /// A harness runs for the conversation on `station`, as far as the gateway knows.
    pub(crate) live: bool,
    /// When a Station last reported on the conversation, Unix seconds.
    pub(crate) heard_at: f64,
    /// The person stopped the answer and it has not ended yet.
    pub(crate) stopping: bool,
    /// The MCP token the conversation's harness was given, the Station it was given for, and
    /// when it expires; reused for later messages to that Station while it lasts.
    pub(crate) token: Option<(String, String, f64)>,
    /// The size of `entries`, as the app is sent them, in bytes.
    pub(crate) bytes: usize,
    /// The read tools called since the person's last message, oldest first, at most
    /// `[assistant.gateway] proposal_reads`: what a proposal made now is based on.
    pub(crate) turn_reads: Vec<ReadCall>,
    /// How the memory entries she was shown here were marked: each `memory:<id>` of one an exposed
    /// Session wrote. What she writes from this conversation carries them.
    pub(crate) memory_marks: Vec<String>,
    /// What the Station running the harness reported each shared change call was made from,
    /// oldest first, until the call takes it; at most `[assistant.limits] shared_bases`.
    pub(crate) shared_bases: VecDeque<AssistantSharedBase>,
    /// The Station that took a message the person then stopped, until it takes another: its
    /// harness is cut off from her Cables, and the only reports taken from it are the Cables
    /// it sent meanwhile, for the audit log.
    pub(crate) cut_off: Option<String>,
    /// Where each entry is kept for good; `None` without a Registry.
    pub(crate) log: Option<super::log::Sink>,
    /// The roster Agent that is Moneypenny.
    pub(crate) agent: Option<String>,
    /// The pool account and model the harness last reported running on.
    pub(crate) account: Option<AccountRef>,
    pub(crate) model: Option<String>,
    /// While a Station's report is applied: that Station, and the tokens it reported, which
    /// the first entry it adds carries.
    pub(crate) reporting: Option<(String, Option<u64>)>,
}

/// A read tool she called: its name and the identifier arguments it was given, never free
/// text.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ReadCall {
    pub(crate) tool: String,
    pub(crate) ids: BTreeMap<String, String>,
}

/// How much one conversation keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Bounds {
    /// Entries kept, newest first.
    pub(crate) entries: usize,
    /// Bytes of entries kept, newest first.
    pub(crate) bytes: usize,
}

/// The size an entry adds to a conversation, as sent to the app.
pub(crate) fn entry_bytes(entry: &Entry) -> usize {
    serde_json::to_vec(entry).map_or(0, |bytes| bytes.len())
}

impl fmt::Debug for Conversation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conversation")
            .field("id", &self.id)
            .field("person_id", &self.person_id)
            .field("entries", &self.entries.len())
            .field("station", &self.station)
            .field("busy", &self.busy)
            .field("bearer", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl Conversation {
    /// Keeps `marks` with those already on it, at most `kept` in all.
    pub(crate) fn note_memory_marks(&mut self, marks: Vec<String>, kept: ai6::Limit) {
        for mark in marks {
            if !self.memory_marks.contains(&mark) && kept.allows(self.memory_marks.len() + 1) {
                self.memory_marks.push(mark);
            }
        }
    }

    /// A new conversation whose open sockets may fall `buffer` events behind.
    pub(crate) fn new(
        id: String,
        person_id: String,
        bearer: String,
        now: f64,
        buffer: usize,
    ) -> Self {
        Self {
            id,
            person_id,
            updated_at: now,
            entries: VecDeque::new(),
            station: None,
            session_id: None,
            route: None,
            busy: false,
            answered: false,
            pending: None,
            bearer,
            events: broadcast::channel(buffer).0,
            notes: Vec::new(),
            proposals: Vec::new(),
            live: false,
            heard_at: now,
            stopping: false,
            token: None,
            bytes: 0,
            turn_reads: Vec::new(),
            memory_marks: Vec::new(),
            shared_bases: VecDeque::new(),
            cut_off: None,
            log: None,
            agent: None,
            account: None,
            model: None,
            reporting: None,
        }
    }

    /// Takes what a Station's `report` says its harness runs on, and has the entries it adds
    /// carry that Station and its tokens until [`Conversation::reporting`] is cleared.
    pub(crate) fn hear_run(&mut self, report: &ai6::AssistantReport) {
        if let Some(run) = &report.run {
            if run.account.is_some() {
                self.account.clone_from(&run.account);
            }
            if run.model.is_some() {
                self.model.clone_from(&run.model);
            }
        }
        self.reporting = Some((
            report.station_id.clone(),
            report.run.as_ref().and_then(|run| run.tokens),
        ));
    }

    /// The turn the harness is on, as far as the gateway knows: the Station reporting now or
    /// the one running it, its provider Session, route, account and model.
    fn turn(&mut self) -> AssistantTurn {
        let (station, tokens) = match &mut self.reporting {
            Some((station, tokens)) => (Some(station.clone()), tokens.take()),
            None => (self.station.clone(), None),
        };
        let mut turn = AssistantTurn::default();
        turn.station = station;
        turn.session.clone_from(&self.session_id);
        turn.agent.clone_from(&self.agent);
        turn.route.clone_from(&self.route);
        turn.account.clone_from(&self.account);
        turn.model.clone_from(&self.model);
        turn.tokens = tokens;
        turn
    }

    /// Keeps a `kind` entry in the durable log, with the turn when `harness` (it came from her
    /// harness or her tools). The person's access token and the tool token are taken out of it
    /// first, wherever they appear.
    pub(crate) fn record(&mut self, kind: &str, text: &str, data: Option<&Value>, harness: bool) {
        if self.log.is_none() {
            return;
        }
        let turn = harness.then(|| self.turn());
        let secrets: Vec<String> = [
            Some(self.bearer.clone()),
            self.token.as_ref().map(|token| token.0.clone()),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut entry = ai6::AssistantLogEntry::new(
            self.id.as_str(),
            self.person_id.as_str(),
            kind,
            super::log::redact(text, &secrets),
            self.updated_at.max(super::now()),
        );
        if let Some(data) = data {
            entry = entry.with_data(super::log::redact_value(data, &secrets));
        }
        if let Some(turn) = turn {
            entry = entry.with_turn(turn);
        }
        if let Some(log) = &self.log {
            log.send(entry);
        }
    }

    /// Remembers that read tool `tool` was called with these identifier arguments, until the
    /// person's next message.
    pub(crate) fn note_read(
        &mut self,
        tool: &str,
        ids: BTreeMap<String, String>,
        gateway: &ai6::AssistantGatewayLimits,
    ) {
        if gateway.proposal_reads.allows(self.turn_reads.len() + 1) {
            self.turn_reads.push(ReadCall {
                tool: tool.to_owned(),
                ids,
            });
        }
    }

    /// What a proposal made now rests on: the reads of this turn.
    pub(crate) fn evidence(&self) -> Evidence {
        Evidence {
            read: self.turn_reads.clone(),
        }
    }

    /// Keeps the shared bases a Station reported, dropping the oldest past `most` and any
    /// [`AssistantSharedBase::validate`] refuses.
    pub(crate) fn keep_shared_bases(&mut self, reported: &[AssistantSharedBase], most: ai6::Limit) {
        for base in reported.iter().take(most.or_max()) {
            if base.validate().is_ok() {
                while !most.allows(self.shared_bases.len() + 1)
                    && self.shared_bases.pop_front().is_some()
                {}
                self.shared_bases.push_back(base.clone());
            }
        }
    }

    /// Takes the base reported for the call that changes `path` of `group`'s repository to
    /// the text whose hex SHA-256 is `content`.
    pub(crate) fn take_shared_base(
        &mut self,
        group: &str,
        path: &str,
        content: &str,
    ) -> Option<AssistantSharedBase> {
        let index = self
            .shared_bases
            .iter()
            .position(|base| base.group == group && base.path == path && base.content == content)?;
        self.shared_bases.remove(index)
    }

    /// Adds an entry, keeping the newest that fit `bounds`, and tells the app.
    pub(crate) fn push(&mut self, entry: Entry, bounds: Bounds) {
        self.updated_at = entry.at;
        if self.log.is_some() {
            let kind = serde_json::to_value(entry.role)
                .ok()
                .and_then(|kind| kind.as_str().map(str::to_owned))
                .unwrap_or_default();
            // The person's own words, and what the gateway says of itself, ran on no turn.
            let harness = self.reporting.is_some()
                || !matches!(entry.role, Role::Person | Role::Error | Role::Notice);
            self.record(&kind, &entry.text, entry.data.as_ref(), harness);
        }
        let _ = self
            .events
            .send(serde_json::json!({"type": "entry", "entry": &entry}));
        self.bytes += entry_bytes(&entry);
        self.entries.push_back(entry);
        while self.entries.len() > bounds.entries
            || (self.bytes > bounds.bytes && self.entries.len() > 1)
        {
            if let Some(dropped) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(entry_bytes(&dropped));
            }
        }
    }

    /// Shows a proposal's new status in its entry and tells the app.
    pub(crate) fn show_proposal(&mut self, proposal: &Proposal) {
        let shown = proposal.shown();
        let id = Value::String(proposal.id.clone());
        if let Some(entry) = self.entries.iter_mut().rev().find(|entry| {
            entry.role == Role::Proposal
                && entry.data.as_ref().and_then(|data| data.get("id")) == Some(&id)
        }) {
            let before = entry_bytes(entry);
            entry.data = Some(shown.clone());
            self.bytes = (self.bytes + entry_bytes(entry)).saturating_sub(before);
        }
        self.record(super::log::PROPOSAL_STATUS, "", Some(&shown), false);
        let _ = self
            .events
            .send(serde_json::json!({"type": "proposal", "proposal": shown}));
    }

    /// Records whether a message is being answered and tells the app.
    pub(crate) fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if !busy {
            self.stopping = false;
        }
        let _ = self
            .events
            .send(serde_json::json!({"type": "busy", "busy": busy}));
    }

    /// Whether the tool token with this digest still serves past its time: it is the one the
    /// conversation holds, and its answer runs with the Station heard from within `deadline`
    /// seconds. A turn may outlast its token, and `answer_deadline` bounds only its silence.
    pub(crate) fn outlasts_its_token(&self, token_digest: &str, now: f64, deadline: f64) -> bool {
        self.busy
            && !self.stopping
            && now - self.heard_at <= deadline
            && self
                .token
                .as_ref()
                .is_some_and(|(token, _, _)| super::digest(token) == token_digest)
    }
}

/// Every conversation, bounded per person, in total and by age.
#[derive(Debug, Default)]
pub(crate) struct Store {
    pub(crate) conversations: HashMap<String, Conversation>,
}

impl Store {
    /// Drops conversations untouched for `max_age` seconds, then the oldest of `person`'s past
    /// `per_person - 1` and the oldest of all past `total - 1`, so one more fits.
    pub(crate) fn make_room(
        &mut self,
        person: &str,
        now: f64,
        max_age: f64,
        per_person: usize,
        total: usize,
    ) {
        self.conversations
            .retain(|_, conversation| now - conversation.updated_at <= max_age);
        let oldest = |conversations: &HashMap<String, Conversation>, person: Option<&str>| {
            conversations
                .values()
                .filter(|conversation| person.is_none_or(|p| conversation.person_id == p))
                .filter(|conversation| !conversation.busy)
                .min_by(|a, b| a.updated_at.total_cmp(&b.updated_at))
                .map(|conversation| conversation.id.clone())
        };
        while self
            .conversations
            .values()
            .filter(|conversation| conversation.person_id == person)
            .count()
            >= per_person
        {
            let Some(id) = oldest(&self.conversations, Some(person)) else {
                break;
            };
            self.conversations.remove(&id);
        }
        while self.conversations.len() >= total {
            let Some(id) = oldest(&self.conversations, None) else {
                break;
            };
            self.conversations.remove(&id);
        }
    }

    /// Keeps `person`'s conversations within `budget` bytes in all: their oldest views give
    /// way first (the entry stays, saying it was removed), then their oldest entries.
    pub(crate) fn trim_person(&mut self, person: &str, budget: usize) {
        let total = |store: &Self| -> usize {
            store
                .conversations
                .values()
                .filter(|conversation| conversation.person_id == person)
                .map(|conversation| conversation.bytes)
                .sum()
        };
        while total(self) > budget {
            let oldest_view = self
                .conversations
                .values()
                .filter(|conversation| conversation.person_id == person)
                .flat_map(|conversation| {
                    conversation
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| entry.role == Role::View && entry.data.is_some())
                        .map(|(index, entry)| (entry.at, conversation.id.clone(), index))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, id, index)) = oldest_view
                && let Some(conversation) = self.conversations.get_mut(&id)
                && let Some(entry) = conversation.entries.get_mut(index)
            {
                let before = entry_bytes(entry);
                entry.data = None;
                entry.text = "(an older view was removed to save space)".to_owned();
                conversation.bytes =
                    (conversation.bytes + entry_bytes(entry)).saturating_sub(before);
                continue;
            }
            let oldest = self
                .conversations
                .values_mut()
                .filter(|conversation| {
                    conversation.person_id == person && !conversation.entries.is_empty()
                })
                .min_by(|a, b| a.updated_at.total_cmp(&b.updated_at));
            let Some(conversation) = oldest else {
                return;
            };
            if let Some(dropped) = conversation.entries.pop_front() {
                conversation.bytes = conversation.bytes.saturating_sub(entry_bytes(&dropped));
            }
        }
    }

    /// The conversation `id` when it is `person`'s.
    pub(crate) fn owned(&mut self, id: &str, person: &str) -> Option<&mut Conversation> {
        self.conversations
            .get_mut(id)
            .filter(|conversation| conversation.person_id == person)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(conversations: &[(&str, &str, f64)]) -> Store {
        let mut store = Store::default();
        for (id, person, at) in conversations {
            store.conversations.insert(
                (*id).to_owned(),
                Conversation::new((*id).to_owned(), (*person).to_owned(), "t".into(), *at, 256),
            );
        }
        store
    }

    #[test]
    fn room_is_made_by_age_then_per_person_then_in_total() {
        let mut store = store_with(&[
            ("old", "a", 1.0),
            ("a1", "a", 100.0),
            ("a2", "a", 200.0),
            ("b1", "b", 150.0),
        ]);
        store.make_room("a", 300.0, 250.0, 2, 10);
        let mut kept: Vec<_> = store.conversations.keys().cloned().collect();
        kept.sort();
        assert_eq!(kept, ["a2", "b1"], "too old, then the oldest of a's");
        store.make_room("c", 300.0, 250.0, 2, 2);
        let mut kept: Vec<_> = store.conversations.keys().cloned().collect();
        kept.sort();
        assert_eq!(kept, ["a2"], "the oldest of all");
        assert!(store.owned("a2", "b").is_none(), "another person's");
        assert!(store.owned("a2", "a").is_some());
    }

    #[test]
    fn entries_are_bounded_and_the_bearer_is_never_printed() {
        let mut conversation = Conversation::new("c".into(), "p".into(), "secret".into(), 1.0, 256);
        for at in 0..5 {
            conversation.push(
                Entry {
                    role: Role::Person,
                    text: at.to_string(),
                    at: f64::from(at),
                    data: None,
                },
                Bounds {
                    entries: 3,
                    bytes: usize::MAX,
                },
            );
        }
        let texts: Vec<_> = conversation
            .entries
            .iter()
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(texts, ["2", "3", "4"]);
        assert!(!format!("{conversation:?}").contains("secret"));
    }

    fn view(at: f64, size: usize) -> Entry {
        Entry {
            role: Role::View,
            text: String::new(),
            at,
            data: Some(serde_json::json!({"id": format!("view-{at}"), "view": "x".repeat(size)})),
        }
    }

    #[test]
    fn bytes_are_bounded_per_conversation_and_per_person_views_first() {
        let unbounded = Bounds {
            entries: usize::MAX,
            bytes: usize::MAX,
        };
        let mut conversation = Conversation::new("c".into(), "p".into(), "t".into(), 1.0, 256);
        for at in 0..4 {
            conversation.push(
                view(f64::from(at), 1_000),
                Bounds {
                    entries: usize::MAX,
                    bytes: 2_500,
                },
            );
        }
        assert_eq!(conversation.entries.len(), 2, "the oldest views went");
        assert!(conversation.bytes <= 2_500);

        let mut store = Store::default();
        for (id, first) in [("a", 0.0), ("b", 10.0)] {
            let mut conversation = Conversation::new(id.into(), "p".into(), "t".into(), first, 256);
            conversation.push(view(first, 1_000), unbounded);
            conversation.push(
                Entry {
                    role: Role::Person,
                    text: "hello".into(),
                    at: first + 1.0,
                    data: None,
                },
                unbounded,
            );
            store.conversations.insert(id.into(), conversation);
        }
        store.trim_person("p", 1_500);
        let a = &store.conversations["a"];
        assert!(
            a.entries[0].data.is_none(),
            "the oldest view of all went first"
        );
        assert!(a.entries[0].text.contains("removed"));
        assert!(store.conversations["b"].entries[0].data.is_some());
        let total: usize = store.conversations.values().map(|c| c.bytes).sum();
        assert!(total <= 1_500, "{total}");
    }
}
