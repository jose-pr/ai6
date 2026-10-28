//! Conversations with Moneypenny, kept per person in the gateway's memory.
//!
//! A conversation remembers what was said (for the app to show again when the person comes
//! back), which Station runs its harness and which provider Session that harness is, so a
//! later message resumes it. It holds the person's access token while the person uses it:
//! the tools run with it, so they can do no more than the person could, and stop when the
//! person's sign-in ends.

use std::collections::{HashMap, VecDeque};
use std::fmt;

use ai6::{AssistantJob, AssistantSharedBase, AssistantWorkspace};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use super::actions::Proposal;

/// Who said an entry, or what it records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
    /// The MCP token the conversation's harness was given, the Station it was given for, and
    /// when it expires; reused for later messages to that Station while it lasts.
    pub(crate) token: Option<(String, String, f64)>,
    /// The size of `entries`, as the app is sent them, in bytes.
    pub(crate) bytes: usize,
    /// A tool answered with text others wrote; for the conversation's life, a memory change
    /// waits for the person.
    pub(crate) exposed: bool,
    /// The workspace the last job named, for the commits its reports lead to.
    pub(crate) workspace: Option<AssistantWorkspace>,
    /// Whether any job of the conversation let her scripts reach a host, so a script brings in
    /// outside text; raised for good, never lowered by a narrower later job.
    pub(crate) scripts_reach_hosts: bool,
    /// What the Station running the harness reported each shared change call was made from,
    /// oldest first, until the call takes it; at most [`ai6::MAX_ASSISTANT_SHARED_BASES`].
    pub(crate) shared_bases: VecDeque<AssistantSharedBase>,
    /// The Station that took a message the person then stopped, until it takes another: its
    /// harness is cut off from her Cables, and the only reports taken from it are the Cables
    /// it sent meanwhile, for the audit log.
    pub(crate) cut_off: Option<String>,
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
            .field("exposed", &self.exposed)
            .field("bearer", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl Conversation {
    pub(crate) fn new(id: String, person_id: String, bearer: String, now: f64) -> Self {
        Self {
            id,
            person_id,
            updated_at: now,
            entries: VecDeque::new(),
            station: None,
            session_id: None,
            busy: false,
            answered: false,
            pending: None,
            bearer,
            events: broadcast::channel(EVENT_BUFFER).0,
            notes: Vec::new(),
            proposals: Vec::new(),
            live: false,
            heard_at: now,
            token: None,
            bytes: 0,
            exposed: false,
            workspace: None,
            scripts_reach_hosts: false,
            shared_bases: VecDeque::new(),
            cut_off: None,
        }
    }

    /// Keeps the shared bases a Station reported, dropping the oldest past
    /// [`ai6::MAX_ASSISTANT_SHARED_BASES`] and any [`AssistantSharedBase::validate`] refuses.
    pub(crate) fn keep_shared_bases(&mut self, reported: &[AssistantSharedBase]) {
        for base in reported.iter().take(ai6::MAX_ASSISTANT_SHARED_BASES) {
            if base.validate().is_ok() {
                if self.shared_bases.len() == ai6::MAX_ASSISTANT_SHARED_BASES {
                    self.shared_bases.pop_front();
                }
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
        let _ = self
            .events
            .send(serde_json::json!({"type": "proposal", "proposal": shown}));
    }

    /// Records whether a message is being answered and tells the app.
    pub(crate) fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        let _ = self
            .events
            .send(serde_json::json!({"type": "busy", "busy": busy}));
    }
}

/// Events buffered for a slow app before it is told it fell behind.
const EVENT_BUFFER: usize = 256;

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
                Conversation::new((*id).to_owned(), (*person).to_owned(), "t".into(), *at),
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
        let mut conversation = Conversation::new("c".into(), "p".into(), "secret".into(), 1.0);
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
        let mut conversation = Conversation::new("c".into(), "p".into(), "t".into(), 1.0);
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
            let mut conversation = Conversation::new(id.into(), "p".into(), "t".into(), first);
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
