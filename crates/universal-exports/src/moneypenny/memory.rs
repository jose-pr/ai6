//! The person's memory as the app reads and changes it: `GET /v1/user/assistant/memory` lists
//! the files of `memory/` in the person's personal repository, and `GET`, `PUT` and `DELETE
//! /v1/user/assistant/memory/{path}` read, write and forget one of them (`{path}` is relative
//! to `memory/`).
//!
//! Universal Exports has no checkout of any repository: each request becomes a memory job
//! ([`ai6::MemoryOp`]) for the Headquarters Station, else the first healthy one, which makes it
//! in a clone of its own and answers within [`MEMORY_TIMEOUT`], or the request fails with
//! `504`. A person reads and changes their own memory only. `?person=<id>` reads another
//! person's with [`MEMORY_ADMIN`] on `config:assistant`, audited `assistant.memory.read`; a
//! change to another person's is refused even then. A change is audited
//! (`assistant.memory.write`, `assistant.memory.forget`) before it is made, and committed to
//! `main` authored by the person, with `Via: Universal Exports` in its message.
//!
//! Errors are `{"error": "<what happened>"}`, worded for the person.

use std::sync::Arc;
use std::time::Duration;

use ai6::{
    AssistantEvent, AssistantJob, AssistantReport, EVENT_ACCEPTED, MemoryAction, MemoryAnswer,
    MemoryCommit, MemoryOp, Person,
};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{ASSISTANT_AUDIT_RESOURCE, Assistant, audit, author_of, healthy_stations, now};
use crate::{ApiError, Gateway, authorize_resource_action, bearer, resolve_user};

/// How long a Station has to answer a memory request.
const MEMORY_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(5)
} else {
    Duration::from_secs(30)
};

/// The action that reads another person's memory, on [`ASSISTANT_AUDIT_RESOURCE`].
pub(crate) const MEMORY_ADMIN: &str = "assistant.memory.admin";

/// The trailer of every commit the person makes from the app.
const TRAILER: &str = "Via: Universal Exports";

/// The longest commit message a person may give, in bytes.
const MAX_MESSAGE_BYTES: usize = 1_024;

/// `?person=<id>`: whose memory, when it is not the signed-in person's.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Whose {
    #[serde(default)]
    person: Option<String>,
}

/// The body of a `PUT`: the file's new text, a commit message, and the digest of the text it
/// was made from (a read's `digest`), so a file changed meanwhile is not replaced.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Written {
    content: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    base: Option<String>,
}

/// A refusal as the app shows it: `{"error": TEXT}` with its status.
#[derive(Debug)]
pub(crate) struct Refused {
    status: StatusCode,
    error: String,
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": self.error}))).into_response()
    }
}

impl From<ApiError> for Refused {
    fn from(error: ApiError) -> Self {
        Self {
            status: error.status,
            error: error.message.to_owned(),
        }
    }
}

fn refused(status: StatusCode, error: impl Into<String>) -> Refused {
    Refused {
        status,
        error: error.into(),
    }
}

/// Whose memory a request is about, and where it lives.
struct Memory {
    assistant: Arc<Assistant>,
    /// The signed-in person.
    person: Person,
    /// The person whose memory it is.
    owner: String,
    /// Their personal repository.
    repo: String,
}

impl Memory {
    /// Checks the request: a signed-in person asking about their own memory, or reading
    /// (`change` false) someone else's with [`MEMORY_ADMIN`]; a deployment that keeps memory.
    async fn of(
        gateway: &Gateway,
        headers: &HeaderMap,
        whose: Option<String>,
        change: bool,
    ) -> Result<Self, Refused> {
        let assistant = gateway.assistant.clone().ok_or_else(|| {
            refused(
                StatusCode::SERVICE_UNAVAILABLE,
                "Universal Exports does not serve Moneypenny here.",
            )
        })?;
        let person = resolve_user(gateway, bearer(headers)?).await?;
        let owner = whose.unwrap_or_else(|| person.id.clone());
        if owner != person.id {
            if change {
                return Err(refused(
                    StatusCode::FORBIDDEN,
                    "Only its person changes their memory.",
                ));
            }
            if !ai6::directorates::section_00::assistant::valid_identity(&owner) {
                return Err(refused(StatusCode::BAD_REQUEST, "That is not a person."));
            }
            authorize_resource_action(
                gateway,
                person.clone(),
                ASSISTANT_AUDIT_RESOURCE.to_owned(),
                MEMORY_ADMIN,
            )
            .await
            .map_err(|_| {
                refused(
                    StatusCode::FORBIDDEN,
                    format!(
                        "Reading another person's memory needs {MEMORY_ADMIN} on \
                         {ASSISTANT_AUDIT_RESOURCE}."
                    ),
                )
            })?;
        }
        let settings = super::assistant_settings(gateway)
            .await
            .map_err(|reason| refused(StatusCode::SERVICE_UNAVAILABLE, reason))?;
        let repo = match settings.personal_repo_for(&owner) {
            Ok(Some(repo)) => repo,
            Ok(None) => {
                return Err(refused(
                    StatusCode::NOT_FOUND,
                    "This deployment gives nobody a personal repository, so Moneypenny keeps \
                     no memory.",
                ));
            }
            Err(reason) => return Err(refused(StatusCode::BAD_REQUEST, reason)),
        };
        Ok(Self {
            assistant,
            person,
            owner,
            repo,
        })
    }

    /// Records `action` by the signed-in person on the owner's `what`: called for every read of
    /// another person's memory and every change, so nothing runs that could not be recorded.
    async fn audit(&self, gateway: &Gateway, action: &str, what: &str) -> Result<(), Refused> {
        audit(
            gateway,
            &self.person,
            action,
            String::new(),
            Some(ASSISTANT_AUDIT_RESOURCE.to_owned()),
            &format!("{} {what}", self.owner),
        )
        .await
        .map_err(|reason| {
            refused(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Nothing was done: {reason}."),
            )
        })
    }

    /// Audits a read of another person's memory; a person's own reads are not audited.
    async fn audit_read(&self, gateway: &Gateway, what: &str) -> Result<(), Refused> {
        if self.owner == self.person.id {
            return Ok(());
        }
        self.audit(gateway, "assistant.memory.read", what).await
    }

    /// Has a Station do `action`; its answer, or why there is none.
    async fn ask(&self, gateway: &Gateway, action: MemoryAction) -> Result<MemoryAnswer, Refused> {
        let op = MemoryOp {
            repo: self.repo.clone(),
            action,
        };
        let event = self.assistant.memory(gateway, &self.owner, op).await?;
        let answer = event.memory.unwrap_or_default();
        if event.kind == EVENT_ACCEPTED {
            return Ok(answer);
        }
        let status = if answer.missing {
            StatusCode::NOT_FOUND
        } else if answer.changed {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_GATEWAY
        };
        Err(refused(
            status,
            sentence(event.text.as_deref().unwrap_or_default()),
        ))
    }

    /// The commit of a change the signed-in person makes: their `message`, else `subject`,
    /// then the trailer.
    fn commit(&self, message: Option<String>, subject: String) -> Result<MemoryCommit, Refused> {
        let subject = match message.map(|message| message.trim().to_owned()) {
            None => subject,
            Some(message)
                if !message.is_empty()
                    && message.len() <= MAX_MESSAGE_BYTES
                    && !message.chars().any(|c| c.is_control() && c != '\n') =>
            {
                message
            }
            Some(_) => {
                return Err(refused(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "A commit message is 1 to {MAX_MESSAGE_BYTES} bytes, without control \
                         characters."
                    ),
                ));
            }
        };
        let (author_name, author_email) = author_of(&self.person);
        Ok(MemoryCommit {
            message: format!("{subject}\n\n{TRAILER}"),
            author_name,
            author_email,
        })
    }
}

/// The Station's words as a sentence: capitalised, ending with a full stop.
fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return "The Station could not do it, and gave no reason.".to_owned();
    };
    let mut said: String = first.to_uppercase().chain(chars).collect();
    if !said.ends_with(['.', '!', '?']) {
        said.push('.');
    }
    said
}

/// `memory/{path}`, when the route's `{path}` names a file of the person's memory.
fn memory_path(path: &str) -> Result<String, Refused> {
    let full = format!("memory/{path}");
    if ai6::memory_path(&full) {
        Ok(full)
    } else {
        Err(refused(
            StatusCode::BAD_REQUEST,
            format!(
                "{path:?} is not a file of Moneypenny's memory: up to seven names separated by \
                 `/`, each of letters, digits, `.`, `-` and `_`, none starting with `.`."
            ),
        ))
    }
}

/// `path` without its `memory/`, as the app names files.
fn shown(path: &str) -> &str {
    path.strip_prefix("memory/").unwrap_or(path)
}

/// `GET /v1/user/assistant/memory[?person=ID]`: `{"person", "commit", "files": [{"path",
/// "bytes"}], "more"}`, each path relative to `memory/`.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(whose): Query<Whose>,
) -> Result<Json<Value>, Refused> {
    let memory = Memory::of(&gateway, &headers, whose.person, false).await?;
    memory.audit_read(&gateway, "memory/").await?;
    let answer = memory.ask(&gateway, MemoryAction::List {}).await?;
    Ok(Json(json!({
        "person": memory.owner,
        "commit": answer.commit,
        "files": answer
            .files
            .iter()
            .map(|file| json!({"path": shown(&file.path), "bytes": file.bytes}))
            .collect::<Vec<Value>>(),
        "more": answer.more,
    })))
}

/// `GET /v1/user/assistant/memory/{path}[?person=ID]`: `{"path", "text", "digest",
/// "commit"}`; `404` for a file that is not there.
pub(crate) async fn read(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(path): Path<String>,
    Query(whose): Query<Whose>,
) -> Result<Json<Value>, Refused> {
    let memory = Memory::of(&gateway, &headers, whose.person, false).await?;
    let path = memory_path(&path)?;
    memory.audit_read(&gateway, &path).await?;
    let answer = memory
        .ask(&gateway, MemoryAction::Read { path: path.clone() })
        .await?;
    Ok(Json(json!({
        "path": shown(&path),
        "text": answer.text,
        "digest": answer.digest,
        "commit": answer.commit,
    })))
}

/// `PUT /v1/user/assistant/memory/{path}` with `{"content", "message"?, "base"?}`: writes the
/// signed-in person's file and commits it; `{"path", "digest", "commit"}`. `409` when `base`
/// is given and the file no longer holds it.
pub(crate) async fn write(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(path): Path<String>,
    Query(whose): Query<Whose>,
    Json(written): Json<Written>,
) -> Result<Json<Value>, Refused> {
    let memory = Memory::of(&gateway, &headers, whose.person, true).await?;
    let path = memory_path(&path)?;
    let commit = memory.commit(written.message, format!("Edit {path}"))?;
    let action = MemoryAction::Write {
        path: path.clone(),
        text: written.content,
        base: written.base,
        commit,
    };
    memory
        .assistant
        .check(&memory.owner, &memory.repo, &action)?;
    memory
        .audit(&gateway, "assistant.memory.write", &path)
        .await?;
    let answer = memory.ask(&gateway, action).await?;
    Ok(Json(json!({
        "path": shown(&path),
        "digest": answer.digest,
        "commit": answer.commit,
    })))
}

/// `DELETE /v1/user/assistant/memory/{path}`: "forget", a commit that removes the signed-in
/// person's file; `{"path", "commit"}`, `404` for a file that is not there.
pub(crate) async fn forget(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(path): Path<String>,
    Query(whose): Query<Whose>,
) -> Result<Json<Value>, Refused> {
    let memory = Memory::of(&gateway, &headers, whose.person, true).await?;
    let path = memory_path(&path)?;
    let commit = memory.commit(None, format!("Forget {path}"))?;
    let action = MemoryAction::Forget {
        path: path.clone(),
        commit,
    };
    memory
        .audit(&gateway, "assistant.memory.forget", &path)
        .await?;
    let answer = memory.ask(&gateway, action).await?;
    Ok(Json(json!({"path": shown(&path), "commit": answer.commit})))
}

impl Assistant {
    /// The memory job of `op` for `person`, checked as a Station will check it.
    fn memory_job(&self, person: &str, op: MemoryOp) -> Result<AssistantJob, Refused> {
        let job = AssistantJob {
            job_id: Uuid::new_v4().to_string(),
            conversation_id: format!("memory-{}", Uuid::new_v4().simple()),
            agent_id: self.config.agent_id.clone(),
            message: String::new(),
            context: Vec::new(),
            recap: None,
            // Never accepted anywhere: a memory job starts no harness.
            mcp_token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            token_expires_at: now() + MEMORY_TIMEOUT.as_secs_f64(),
            session_id: None,
            close: false,
            interrupt: false,
            exposed: false,
            person_id: Some(person.to_owned()),
            workspace: None,
            commit: None,
            cables: Vec::new(),
            cable_confirm: None,
            cable_discard: None,
            memory: Some(op),
            // Runs nothing.
            reach: Default::default(),
            skills: Vec::new(),
            groups: None,
        };
        job.validate()
            .map_err(|reason| refused(StatusCode::BAD_REQUEST, sentence(&reason)))?;
        Ok(job)
    }

    /// Refuses `action` on `person`'s `repo` as its job would be refused, before anything is
    /// audited.
    fn check(&self, person: &str, repo: &str, action: &MemoryAction) -> Result<(), Refused> {
        let op = MemoryOp {
            repo: repo.to_owned(),
            action: action.clone(),
        };
        self.memory_job(person, op).map(|_| ())
    }

    /// Queues `op` on `person`'s memory for the Headquarters Station, else the first healthy
    /// one, and waits for its answer: the event that settles the job.
    async fn memory(
        &self,
        gateway: &Gateway,
        person: &str,
        op: MemoryOp,
    ) -> Result<AssistantEvent, Refused> {
        let job = self.memory_job(person, op)?;
        let job_id = job.job_id.clone();
        let station = healthy_stations(gateway)
            .await
            .into_iter()
            .next()
            .ok_or_else(|| {
                refused(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "No Station is checking in to reach the repository.",
                )
            })?;
        let (answered, answer) = tokio::sync::oneshot::channel();
        {
            let mut desk = self.desk();
            desk.memory
                .insert(job_id.clone(), (station.clone(), answered));
            desk.controls.push((station, job));
        }
        self.ready.notify_waiters();
        if let Ok(Ok(report)) = tokio::time::timeout(MEMORY_TIMEOUT, answer).await
            && let Some(event) = report.events.into_iter().next()
        {
            return Ok(event);
        }
        let mut desk = self.desk();
        desk.memory.remove(&job_id);
        let taken = !desk.controls.iter().any(|(_, job)| job.job_id == job_id);
        desk.controls.retain(|(_, job)| job.job_id != job_id);
        Err(refused(
            StatusCode::GATEWAY_TIMEOUT,
            if taken {
                "The Station did not answer in time; a change may still be made. Look again \
                 shortly."
            } else {
                "No Station took the request in time, so nothing was done."
            },
        ))
    }

    /// Hands a Station's answer to the memory request waiting for it: `None` when `report`
    /// answers no memory job, `Err(403)` when it comes from a Station the job was not given to.
    pub(crate) fn answer_memory(&self, report: &AssistantReport) -> Option<Result<(), StatusCode>> {
        let job_id = report.job_id.as_ref()?;
        let mut desk = self.desk();
        let (station, _) = desk.memory.get(job_id)?;
        if *station != report.station_id {
            return Some(Err(StatusCode::FORBIDDEN));
        }
        let (_, waiting) = desk.memory.remove(job_id)?;
        let _ = waiting.send(report.clone());
        Some(Ok(()))
    }
}
