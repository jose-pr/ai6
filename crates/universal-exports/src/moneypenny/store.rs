//! Conversations with Moneypenny, kept per person in the gateway's memory.
//!
//! A conversation remembers what was said (for the app to show again when the person comes
//! back), which Station runs its harness and which provider Session that harness is, so a
//! later message resumes it. It holds the person's access token while the person uses it:
//! the tools run with it, so they can do no more than the person could, and stop when the
//! person's sign-in ends.

use std::collections::{HashMap, VecDeque};
use std::fmt;

use ai6::AssistantJob;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

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
        }
    }

    /// Adds an entry, keeping the newest `max_entries`, and tells the app.
    pub(crate) fn push(&mut self, entry: Entry, max_entries: usize) {
        self.updated_at = entry.at;
        let _ = self
            .events
            .send(serde_json::json!({"type": "entry", "entry": &entry}));
        self.entries.push_back(entry);
        while self.entries.len() > max_entries {
            self.entries.pop_front();
        }
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
                3,
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
}
