//! Sessions seeded by replaying a history as context, for an agent with no way to import one.
//!
//! `import_session` records, at the host's UUID, the context its history makes
//! ([`coqui_acp_history::replay`]). The first `session/resume` or `session/load` of that UUID then
//! starts a fresh session in the agent (`session/new`), answers the host as if the session had
//! been opened, and speaks of it by the UUID from then on: the UUID the host sends is the agent's
//! own id on the way in, and the agent's id is the UUID on the way out. The first prompt of the
//! session carries the context as its first block, once.
//!
//! An import is bound to nothing but its UUID, so it is for a UUID the host minted: an id the
//! agent has already reported (any connection of this provider) is refused, and an import nobody
//! opens expires.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use core::time::Duration;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use serde_json::{Value, json};

/// How many imports may wait to be opened by default, so a host that imports and never opens cannot make the
/// provider hold histories without limit.
pub(crate) const WAITING_MAX: u64 = 64;
/// How long an import waits to be opened by default, in seconds.
pub(crate) const WAITING_SECS: u64 = 60 * 60;
/// How many agent session ids are remembered as live by default.
pub(crate) const REMEMBERED_IDS: u64 = 4096;

/// The id as it is compared: a UUID in any spelling is lower-case and hyphenated, anything else as it is.
fn normal(id: &str) -> String {
    uuid::Uuid::parse_str(id).map_or_else(|_| id.to_owned(), |uuid| uuid.hyphenated().to_string())
}

/// A session imported by replay that no connection has opened yet.
#[derive(Debug)]
struct Waiting {
    /// The replay block that goes first in the session's first prompt; none for an empty history.
    context: Option<Value>,
    since: Instant,
}

#[derive(Debug, Default)]
struct Inner {
    waiting: BTreeMap<String, Waiting>,
    /// Session ids the agent has reported, oldest first.
    seen: BTreeSet<String>,
    order: VecDeque<String>,
}

/// Imported sessions waiting for a connection to open them, shared by the provider's connections.
#[derive(Debug)]
pub(crate) struct Replays {
    inner: Mutex<Inner>,
    waits: Duration,
    max_seen: usize,
    max_waiting: usize,
}

impl Default for Replays {
    fn default() -> Self {
        Self::expiring(Duration::from_secs(WAITING_SECS))
    }
}

impl Replays {
    /// Imports that expire after `waits`.
    pub(crate) fn expiring(waits: Duration) -> Self {
        Self::limited(
            waits,
            usize::try_from(REMEMBERED_IDS).unwrap_or(usize::MAX),
            usize::try_from(WAITING_MAX).unwrap_or(usize::MAX),
        )
    }

    /// Imports that expire after `waits`, and the last `max_seen` agent session ids remembered.
    pub(crate) fn limited(waits: Duration, max_seen: usize, max_waiting: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            waits,
            max_seen,
            max_waiting,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn purge(&self, inner: &mut Inner) {
        let waits = self.waits;
        inner
            .waiting
            .retain(|_, waiting| waiting.since.elapsed() < waits);
    }

    /// Keeps `context` for `session`.
    pub(crate) fn add(&self, session: &str, context: Option<Value>) -> Result<(), Refusal> {
        let session = normal(session);
        let session = session.as_str();
        let mut inner = self.lock();
        self.purge(&mut inner);
        if inner.seen.contains(session) {
            return Err(Refusal::Live);
        }
        if inner.waiting.contains_key(session) {
            return Err(Refusal::Named);
        }
        if inner.waiting.len() >= self.max_waiting {
            return Err(Refusal::Full);
        }
        inner.waiting.insert(
            session.to_owned(),
            Waiting {
                context,
                since: Instant::now(),
            },
        );
        Ok(())
    }

    /// Gives back an import that was taken but not used, whether or not its id has opened since:
    /// it waits again unless the id was imported anew meanwhile or too many wait.
    fn restore(&self, session: &str, context: Option<Value>) {
        let session = normal(session);
        let mut inner = self.lock();
        self.purge(&mut inner);
        if inner.waiting.contains_key(&session) || inner.waiting.len() >= self.max_waiting {
            tracing::warn!(
                "an imported session could not be put back: it was imported again or too many wait"
            );
            return;
        }
        inner.waiting.insert(
            session,
            Waiting {
                context,
                since: Instant::now(),
            },
        );
    }

    fn take(&self, session: &str) -> Option<Waiting> {
        let session = normal(session);
        let mut inner = self.lock();
        self.purge(&mut inner);
        inner.waiting.remove(&session)
    }

    /// Notes that the agent speaks of `session`: no import may take its id, and one that waits at
    /// it is dropped, as it would meet a session that is not the import's.
    fn see(&self, session: &str) {
        let session = normal(session);
        let session = session.as_str();
        let mut inner = self.lock();
        inner.waiting.remove(session);
        if inner.seen.insert(session.to_owned()) {
            inner.order.push_back(session.to_owned());
            while inner.order.len() > self.max_seen {
                if let Some(oldest) = inner.order.pop_front() {
                    inner.seen.remove(&oldest);
                }
            }
        }
    }
}

/// Why a session could not be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The id already names an imported session.
    Named,
    /// The agent has reported the id as a session of its own.
    Live,
    /// Too many imported sessions wait.
    Full,
}

/// One connection's sessions opened from an import.
#[derive(Debug, Default)]
pub(crate) struct Mapping {
    /// `session/new` requests standing in for an open, by request id.
    opening: BTreeMap<String, (String, Option<Value>)>,
    /// Opens of sessions that are not imports, by request id: the session, live if the open works.
    loading: BTreeMap<String, String>,
    /// The normal UUID to the UUID as the host spells it, the agent's id and the context not yet
    /// delivered.
    live: BTreeMap<String, (String, String, Option<Value>)>,
    /// The agent's id to the UUID as the host spells it.
    by_agent: BTreeMap<String, String>,
}

impl Mapping {
    /// Rewrites a message of the host for the agent: an open of an imported session becomes
    /// `session/new`, and the UUID becomes the agent's id, the first prompt carrying the context.
    /// Answers the request id of an open it turned into `session/new`, for [`Mapping::undo`].
    pub(crate) fn outbound(&mut self, replays: &Replays, message: &mut Value) -> Option<String> {
        let object = message.as_object_mut()?;
        let method = object.get("method").and_then(Value::as_str).unwrap_or("");
        let id = object
            .get("id")
            .filter(|id| id.is_string() || id.is_number());
        let id = id.map(Value::to_string);
        let session = object
            .get("params")
            .and_then(|params| params.get("sessionId"))
            .and_then(Value::as_str)
            .map(str::to_owned)?;
        let opens = matches!(method, "session/load" | "session/resume");
        // An id already standing in for an open is not used for a second.
        let free = id
            .as_ref()
            .is_some_and(|id| !self.opening.contains_key(id) && !self.loading.contains_key(id));
        if opens
            && free
            && let Some(id) = id.clone()
            && let Some(waiting) = replays.take(&session)
        {
            let params = object.get("params").cloned().unwrap_or(Value::Null);
            let mut fresh = json!({"cwd": params.get("cwd").cloned().unwrap_or(Value::Null)});
            fresh["mcpServers"] = params
                .get("mcpServers")
                .cloned()
                .unwrap_or_else(|| json!([]));
            if let Some(directories) = params.get("additionalDirectories") {
                fresh["additionalDirectories"] = directories.clone();
            }
            object.insert("method".into(), json!("session/new"));
            object.insert("params".into(), fresh);
            self.opening.insert(id.clone(), (session, waiting.context));
            return Some(id);
        }
        let Some((_, agent, context)) = self.live.get_mut(&normal(&session)) else {
            // An open of a session that is no import: it is live if the agent opens it.
            if opens
                && free
                && let Some(id) = id
            {
                self.loading.insert(id, session);
            }
            return None;
        };
        let agent = agent.clone();
        if method == "session/prompt"
            && let Some(context) = context.take()
            && let Some(prompt) = object
                .get_mut("params")
                .and_then(|params| params.get_mut("prompt"))
                .and_then(Value::as_array_mut)
        {
            prompt.insert(0, context);
        }
        if let Some(params) = object.get_mut("params").and_then(Value::as_object_mut) {
            params.insert("sessionId".into(), Value::String(agent));
        }
        None
    }

    /// Takes back an open that was turned into `session/new` but never sent, because the request
    /// was refused: the import waits again.
    pub(crate) fn undo(&mut self, replays: &Replays, request: &str) {
        if let Some((uuid, context)) = self.opening.remove(request) {
            replays.restore(&uuid, context);
        }
    }

    /// The connection ended: an open still out, and an opened session whose first prompt never
    /// went, give their imports back.
    pub(crate) fn restore_all(&mut self, replays: &Replays) {
        for (uuid, context) in core::mem::take(&mut self.opening).into_values() {
            replays.restore(&uuid, context);
        }
        for (uuid, _, context) in core::mem::take(&mut self.live).into_values() {
            if context.is_some() {
                replays.restore(&uuid, context);
            }
        }
        self.loading.clear();
        self.by_agent.clear();
    }

    /// Rewrites a message of the agent for the host: the answer to a stand-in open becomes the
    /// answer to the resume or load, and the agent's id becomes the UUID. Whether it changed the
    /// message.
    pub(crate) fn inbound(&mut self, replays: &Replays, message: &mut Value) -> bool {
        let Some(object) = message.as_object_mut() else {
            return false;
        };
        // Every id the agent speaks of is live, whichever connection it is on.
        if let Some(session) = object
            .get("result")
            .and_then(|result| result.get("sessionId"))
            .and_then(Value::as_str)
        {
            replays.see(session);
        }
        if let Some(session) = object
            .get("params")
            .and_then(|params| params.get("sessionId"))
            .and_then(Value::as_str)
        {
            replays.see(session);
        }
        if let Some(sessions) = object
            .get("result")
            .and_then(|result| result.get("sessions"))
            .and_then(Value::as_array)
        {
            for session in sessions
                .iter()
                .filter_map(|s| s.get("sessionId").and_then(Value::as_str))
            {
                replays.see(session);
            }
        }
        if !object.contains_key("method")
            && let Some(id) = object.get("id").map(Value::to_string)
            && let Some(session) = self.loading.remove(&id)
            && object.contains_key("result")
        {
            replays.see(&session);
        }
        if !object.contains_key("method")
            && let Some(id) = object.get("id").map(Value::to_string)
            && let Some((uuid, context)) = self.opening.remove(&id)
        {
            let opened = object
                .get_mut("result")
                .and_then(Value::as_object_mut)
                .and_then(|result| result.remove("sessionId"));
            match opened {
                Some(Value::String(agent)) => {
                    // The id is the agent's session from now on, not an import to take.
                    replays.see(&uuid);
                    self.by_agent.insert(agent.clone(), uuid.clone());
                    self.live.insert(normal(&uuid), (uuid, agent, context));
                }
                _ => {
                    // The agent did not open a session: the import waits for the next try, and the
                    // host is told so, not that the open worked.
                    replays.restore(&uuid, context);
                    if object.contains_key("result") {
                        let id = object.get("id").cloned().unwrap_or(Value::Null);
                        *message = json!({"jsonrpc": "2.0", "id": id, "error": {
                            "code": -32603, "message": "the agent did not open a session"}});
                    }
                }
            }
            return true;
        }
        let Some(params) = object.get_mut("params").and_then(Value::as_object_mut) else {
            return false;
        };
        if let Some(Value::String(session)) = params.get("sessionId")
            && let Some(uuid) = self.by_agent.get(session)
        {
            let uuid = uuid.clone();
            params.insert("sessionId".into(), Value::String(uuid));
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resume(id: u64, session: &str) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": "session/resume",
            "params": {"sessionId": session, "cwd": "/w", "mcpServers": []}})
    }

    #[test]
    fn an_agent_answer_without_a_session_id_is_a_failure_and_the_import_waits_again() {
        for answer in [
            json!({"jsonrpc": "2.0", "id": 7, "result": {}}),
            json!({"jsonrpc": "2.0", "id": 7, "result": {"sessionId": 5}}),
        ] {
            let replays = Replays::default();
            replays.add("u1", None).unwrap();
            let mut mapping = Mapping::default();
            let mut request = resume(7, "u1");
            assert_eq!(mapping.outbound(&replays, &mut request), Some("7".into()));
            assert_eq!(request["method"], "session/new");
            let mut answer = answer;
            assert!(mapping.inbound(&replays, &mut answer));
            assert!(answer.get("result").is_none(), "{answer}");
            assert_eq!(answer["error"]["code"], -32603);
            assert_eq!(
                replays.add("u1", None),
                Err(Refusal::Named),
                "it waits again"
            );
        }
    }

    #[test]
    fn a_refused_request_gives_the_import_back() {
        let replays = Replays::default();
        replays.add("u1", None).unwrap();
        let mut mapping = Mapping::default();
        let mut request = resume(3, "u1");
        let key = mapping.outbound(&replays, &mut request).unwrap();
        assert!(replays.take("u1").is_none(), "taken while the open is out");
        mapping.undo(&replays, &key);
        assert!(replays.take("u1").is_some());
        assert!(mapping.opening.is_empty());
    }

    #[test]
    fn an_id_the_agent_reported_cannot_be_imported() {
        let replays = Replays::default();
        let mut mapping = Mapping::default();
        let mut news = json!({"jsonrpc": "2.0", "id": 1, "result": {"sessionId": "live-1"}});
        mapping.inbound(&replays, &mut news);
        let mut update = json!({"jsonrpc": "2.0", "method": "session/update",
            "params": {"sessionId": "live-2", "update": {}}});
        mapping.inbound(&replays, &mut update);
        assert_eq!(replays.add("live-1", None), Err(Refusal::Live));
        assert_eq!(replays.add("live-2", None), Err(Refusal::Live));
        assert!(replays.add("minted", None).is_ok());
    }

    #[test]
    fn only_the_last_remembered_ids_are_held() {
        let replays = Replays::limited(Duration::from_secs(60), 2, 8);
        for id in ["a", "b", "c"] {
            replays.see(id);
        }
        assert!(replays.add("a", None).is_ok(), "the oldest was forgotten");
        assert_eq!(replays.add("c", None), Err(Refusal::Live));
    }

    #[test]
    fn no_more_imports_wait_than_the_cap_and_zero_refuses_all() {
        let replays = Replays::limited(Duration::from_secs(60), 8, 2);
        assert!(replays.add("a", None).is_ok());
        assert!(replays.add("b", None).is_ok());
        assert_eq!(replays.add("c", None), Err(Refusal::Full));
        let none = Replays::limited(Duration::from_secs(60), 8, 0);
        assert_eq!(none.add("a", None), Err(Refusal::Full));
    }

    #[test]
    fn an_import_nobody_opens_expires() {
        let replays = Replays::expiring(Duration::from_millis(10));
        replays.add("u1", None).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        assert!(replays.take("u1").is_none());
        assert!(replays.add("u1", None).is_ok(), "the id is free again");
    }

    #[test]
    fn an_id_is_the_same_in_any_spelling() {
        let upper = "5F0F6D6E-3F3C-4F1A-8A52-1C1E6F0B9A77";
        let lower = "5f0f6d6e-3f3c-4f1a-8a52-1c1e6f0b9a77";
        let replays = Replays::default();
        replays.add(upper, None).unwrap();
        assert_eq!(replays.add(lower, None), Err(Refusal::Named));
        assert!(replays.take(lower).is_some(), "found by another spelling");
        let braced = format!("{{{lower}}}");
        replays.see(&braced);
        assert_eq!(replays.add(upper, None), Err(Refusal::Live));
    }

    #[test]
    fn an_id_the_agent_reports_drops_the_import_waiting_at_it() {
        let replays = Replays::default();
        replays.add("u1", None).unwrap();
        let mut mapping = Mapping::default();
        let mut news = json!({"jsonrpc": "2.0", "id": 1, "result": {"sessionId": "u1"}});
        mapping.inbound(&replays, &mut news);
        assert!(
            replays.take("u1").is_none(),
            "the agent's session is not the import's"
        );
    }

    #[test]
    fn an_import_that_opened_is_live_and_cannot_be_imported_again() {
        let replays = Replays::default();
        replays.add("u1", None).unwrap();
        let mut mapping = Mapping::default();
        let mut request = resume(7, "u1");
        mapping.outbound(&replays, &mut request).unwrap();
        let mut answer = json!({"jsonrpc": "2.0", "id": 7, "result": {"sessionId": "agent-1"}});
        mapping.inbound(&replays, &mut answer);
        assert_eq!(replays.add("u1", None), Err(Refusal::Live));
    }

    #[test]
    fn sessions_a_load_or_a_list_reports_are_live() {
        let replays = Replays::default();
        let mut mapping = Mapping::default();
        let mut load = resume(5, "x1");
        assert!(mapping.outbound(&replays, &mut load).is_none());
        let mut answer = json!({"jsonrpc": "2.0", "id": 5, "result": {}});
        mapping.inbound(&replays, &mut answer);
        assert_eq!(replays.add("x1", None), Err(Refusal::Live));

        let mut refused = resume(6, "x2");
        mapping.outbound(&replays, &mut refused);
        let mut error = json!({"jsonrpc": "2.0", "id": 6, "error": {"code": -1, "message": "no"}});
        mapping.inbound(&replays, &mut error);
        assert!(
            replays.add("x2", None).is_ok(),
            "a refused load names no live session"
        );

        let mut list = json!({"jsonrpc": "2.0", "id": 9, "result": {"sessions": [
            {"sessionId": "l1", "cwd": "/w"}, {"sessionId": "l2", "cwd": "/w"}]}});
        mapping.inbound(&replays, &mut list);
        assert_eq!(replays.add("l1", None), Err(Refusal::Live));
        assert_eq!(replays.add("l2", None), Err(Refusal::Live));
    }

    #[test]
    fn an_open_lost_with_the_connection_gives_the_import_back() {
        let replays = Replays::default();
        replays
            .add("u1", Some(json!({"type": "text", "text": "ctx"})))
            .unwrap();
        replays
            .add("u2", Some(json!({"type": "text", "text": "ctx2"})))
            .unwrap();
        let mut mapping = Mapping::default();
        // One open that was never answered, one that opened and never got its first prompt.
        let mut first = resume(1, "u1");
        mapping.outbound(&replays, &mut first).unwrap();
        let mut second = resume(2, "u2");
        mapping.outbound(&replays, &mut second).unwrap();
        let mut answer = json!({"jsonrpc": "2.0", "id": 2, "result": {"sessionId": "agent-2"}});
        mapping.inbound(&replays, &mut answer);
        mapping.restore_all(&replays);
        assert!(replays.take("u1").is_some());
        assert!(replays.take("u2").is_some());
    }

    #[test]
    fn an_import_whose_context_was_delivered_is_not_given_back() {
        let replays = Replays::default();
        replays
            .add("u1", Some(json!({"type": "text", "text": "ctx"})))
            .unwrap();
        let mut mapping = Mapping::default();
        let mut open = resume(1, "u1");
        mapping.outbound(&replays, &mut open).unwrap();
        let mut answer = json!({"jsonrpc": "2.0", "id": 1, "result": {"sessionId": "agent-1"}});
        mapping.inbound(&replays, &mut answer);
        let mut prompt = json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
            "params": {"sessionId": "u1", "prompt": [{"type": "text", "text": "go"}]}});
        mapping.outbound(&replays, &mut prompt);
        mapping.restore_all(&replays);
        assert!(replays.take("u1").is_none());
    }

    #[test]
    fn a_second_open_with_the_same_request_id_is_not_converted() {
        let replays = Replays::default();
        replays.add("u1", None).unwrap();
        replays.add("u2", None).unwrap();
        let mut mapping = Mapping::default();
        let mut first = resume(7, "u1");
        assert!(mapping.outbound(&replays, &mut first).is_some());
        let mut second = resume(7, "u2");
        assert!(mapping.outbound(&replays, &mut second).is_none());
        assert_eq!(
            second["method"], "session/resume",
            "it goes to the agent as it is"
        );
        assert!(replays.take("u2").is_some(), "u2 was not used up");
    }

    #[test]
    fn an_import_that_cannot_be_put_back_is_dropped_without_a_trace_in_the_mapping() {
        let replays = Replays::default();
        replays.add("u1", None).unwrap();
        let mut mapping = Mapping::default();
        let mut request = resume(3, "u1");
        let key = mapping.outbound(&replays, &mut request).unwrap();
        // The host imported the id again meanwhile.
        replays.add("u1", None).unwrap();
        mapping.undo(&replays, &key);
        assert!(mapping.opening.is_empty());
        assert!(replays.take("u1").is_some());
        assert!(replays.take("u1").is_none());
    }
}
