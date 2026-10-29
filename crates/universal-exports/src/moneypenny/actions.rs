//! Moneypenny's write tools: she proposes, the person confirms.
//!
//! A write tool never changes anything. It checks the input, checks that the person may do
//! the thing at all (so a person without the permission gets no proposal), and records a
//! proposal: the exact request, a summary and what it changes. The app shows it with
//! Confirm and Cancel. Only a confirm from the signed-in session the proposal was made in
//! runs it, through the same route and authorization as the app's own request, after an
//! audit record (`moneypenny.confirmed`) names the person, the conversation and the
//! proposal. Moneypenny reads untrusted text, so nothing she reads can act on its own.
//!
//! Irreversible steps keep green ink: there is no tool to sign it; she can only point the
//! person to the Green ink page.

use axum::http::Method;
use serde_json::{Map, Value, json};

use super::tools::{
    Call, Outcome, check, encode, id, object, operation, optional_id, string, text,
};

/// How long a proposal waits for the person, in seconds.
pub(crate) const PROPOSAL_TTL_SECS: f64 = 3_600.0;
/// The most proposals one conversation keeps; finished ones go first.
pub(crate) const MAX_PROPOSALS: usize = 32;

/// Where the permission a write needs is checked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Scope {
    /// `operation:<id>`, recorded as the audit record's Operation.
    Operation(String),
    /// Another resource (`config:roster`, `station:<id>`), recorded as the audit resource.
    Resource(String),
}

impl Scope {
    pub(crate) fn resource(&self) -> String {
        match self {
            Self::Operation(operation) => format!("operation:{operation}"),
            Self::Resource(resource) => resource.clone(),
        }
    }
}

/// What a write tool would do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Draft {
    pub(crate) call: Call,
    pub(crate) scope: Scope,
    /// The action the person needs on the scope.
    pub(crate) action: &'static str,
    pub(crate) summary: String,
}

/// A proposal and where it stands.
#[derive(Debug, Clone)]
pub(crate) struct Proposal {
    pub(crate) id: String,
    pub(crate) tool: &'static str,
    pub(crate) draft: Draft,
    /// `pending`, `confirming`, `done`, `failed`, `cancelled` or `expired`.
    pub(crate) status: &'static str,
    pub(crate) created_at: f64,
    /// SHA-256 of the access token of the session it was proposed in.
    pub(crate) session: String,
    pub(crate) result: Option<String>,
    /// A commit a Station makes once the person confirms, in place of `draft.call`, which then
    /// only describes it.
    pub(crate) work: Option<ai6::AssistantCommit>,
    /// For a Cable Moneypenny asked to send after reading outside text: the Station holding
    /// it, which sends it once confirmed. Such a proposal is not a gateway request.
    pub(crate) cable: Option<CableHold>,
}

/// Where a Cable waiting for the person's confirmation is held.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CableHold {
    /// The Station desk's ID for it.
    pub(crate) held_id: String,
    /// SHA-256 of the request, as the Station reported it.
    pub(crate) digest: String,
    /// The Station whose desk holds it.
    pub(crate) station: String,
}

/// The proposal of a memory change her conversation set aside: confirmed, a Station commits
/// it (`memory_commit`); cancelled, it is discarded (`memory_discard`).
pub(crate) const MEMORY_COMMIT: &str = "memory_commit";

/// The write tool that proposes writing one file of a shared repository.
pub(crate) const SHARED_CHANGE: &str = ai6::ASSISTANT_SHARED_CHANGE_TOOL;

pub(crate) const SHARED_CHANGE_DESCRIPTION: &str = "Propose writing one file of a repository the person's directory group shares (shared/<group>/ \
     in your workspace, or shared/org/), committed under the person's name. Nothing changes \
     until the person confirms it in the app, unless the deployment lets that group's changes \
     through at once. Refused when the person is not in the group.";

/// The input of [`SHARED_CHANGE`].
pub(crate) fn shared_change_schema() -> Value {
    object(
        &[
            (
                "group",
                text("The directory group whose repository it is, or \"org\"."),
            ),
            (
                "path",
                text("The file, relative to the repository's root (docs/on-call.md)."),
            ),
            ("content", text("The file's whole new text.")),
            (
                "message",
                text("The commit's subject: what changes and why."),
            ),
        ],
        &["group", "path", "content", "message"],
    )
}

/// Checks [`SHARED_CHANGE`]'s input: the schema, no hidden character, a repository path, at
/// most [`ai6::MAX_ASSISTANT_CHANGE_BYTES`] of text and a one-line subject of 1 to 200
/// characters. Answers group, path, content and subject.
pub(crate) fn shared_change(input: &Value) -> Result<(&str, &str, &str, &str), Outcome> {
    let invalid = |problem: String| Outcome::error(format!("INVALID_INPUT: {problem}"));
    check(&shared_change_schema(), input, "input").map_err(invalid)?;
    hidden_characters(input, "input").map_err(invalid)?;
    let field = |name| input[name].as_str().unwrap_or_default();
    let (group, path, content, message) = (
        field("group"),
        field("path"),
        field("content"),
        field("message"),
    );
    if !ai6::directorates::m::assistant::file_safe(group) {
        return Err(invalid(format!("{group:?} is not a group")));
    }
    if !ai6::directorates::section_00::assistant::repository_path(path) {
        return Err(invalid(format!(
            "{path:?} is not a file path inside a repository"
        )));
    }
    if content.len() > ai6::MAX_ASSISTANT_CHANGE_BYTES {
        return Err(invalid(format!(
            "content is longer than {} bytes",
            ai6::MAX_ASSISTANT_CHANGE_BYTES
        )));
    }
    if message.trim().is_empty() || message.chars().count() > 200 || message.contains('\n') {
        return Err(invalid(
            "message is one line of 1 to 200 characters".to_owned(),
        ));
    }
    Ok((group, path, content, message))
}

impl Proposal {
    /// How the app shows it: the exact request, the summary and the status.
    pub(crate) fn shown(&self) -> Value {
        json!({
            "id": self.id,
            "tool": self.tool,
            "summary": self.draft.summary,
            "request": {
                "method": self.draft.call.method.as_str(),
                "path": self.draft.call.path,
                "body": self.draft.call.body,
            },
            "needs": {"action": self.draft.action, "on": self.draft.scope.resource()},
            "status": self.status,
            "result": self.result,
        })
    }
}

/// A write tool as Moneypenny sees it.
pub(crate) struct WriteTool {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    schema: fn() -> Value,
    /// The draft for the checked input; the second argument is the proposal's ID, which a
    /// route that takes an idempotency key gets.
    plan: fn(&Map<String, Value>, &str) -> Result<Draft, String>,
}

impl WriteTool {
    pub(crate) fn schema(&self) -> Value {
        (self.schema)()
    }

    /// Checks `input` and drafts the request. An edit of her own settings (`assistant`) needs
    /// `current`, the settings in force, and its summary ends with what it loosens or
    /// redirects against them ([`ai6::AssistantSettings::changes_from`]).
    pub(crate) fn draft(
        &self,
        input: &Value,
        proposal_id: &str,
        current: Option<&ai6::AssistantSettings>,
    ) -> Result<Draft, Outcome> {
        let invalid = |problem: String| Outcome::error(format!("INVALID_INPUT: {problem}"));
        check(&self.schema(), input, "input").map_err(invalid)?;
        hidden_characters(input, "input").map_err(invalid)?;
        let empty = Map::new();
        let input = input.as_object().unwrap_or(&empty);
        let mut draft = (self.plan)(input, proposal_id).map_err(invalid)?;
        if edits_assistant(self.name, input) {
            let settings = assistant_record(input).map_err(invalid)?;
            let current = current.ok_or_else(|| {
                Outcome::error(
                    "Nothing was proposed: the settings in force could not be read, so what \
                     this changes could not be shown.",
                )
            })?;
            let changes = settings.changes_from(current);
            if !changes.is_empty() {
                draft
                    .summary
                    .push_str(&format!(". Weakens: {}", changes.join("; ")));
            }
        }
        Ok(draft)
    }
}

/// Whether `tool` with `input` proposes an edit of her own settings, the `assistant` record.
pub(crate) fn edits_assistant(tool: &str, input: &Map<String, Value>) -> bool {
    tool == "propose_roster_edit" && input.get("kind").and_then(Value::as_str) == Some("assistant")
}

/// Refuses text holding characters that render as nothing (direction controls, zero-width
/// characters, tags, selectors, fillers, as AI6's vetting counts them): a person confirming
/// a proposal must see every character of what runs.
pub(crate) fn hidden_characters(value: &Value, at: &str) -> Result<(), String> {
    match value {
        Value::String(text) => match text
            .chars()
            .find(|character| ai6::directorates::m::vetting::is_invisible(*character))
        {
            Some(character) => Err(format!(
                "{at} holds a hidden character (U+{:04X}); write it without one",
                u32::from(character)
            )),
            None => Ok(()),
        },
        Value::Array(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(index, item)| hidden_characters(item, &format!("{at}[{index}]"))),
        Value::Object(object) => object.iter().try_for_each(|(key, item)| {
            hidden_characters(&Value::String(key.clone()), &format!("{at} key"))?;
            hidden_characters(item, &format!("{at}.{key}"))
        }),
        _ => Ok(()),
    }
}

fn post(path: String, body: Option<Value>) -> Call {
    Call {
        method: Method::POST,
        path,
        body,
    }
}

fn strings(description: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": description})
}

fn string_list(input: &Map<String, Value>, name: &str) -> Vec<Value> {
    input
        .get(name)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn roster_kind() -> Value {
    json!({
        "type": "string",
        "enum": ["operations", "agents", "roles", "covers", "assistant"],
        "description": "Which records; assistant is your own settings, one record keyed settings."
    })
}

/// The settings a `propose_roster_edit` of the `assistant` record sets, or why they are not
/// acceptable settings under the key `settings`.
fn assistant_record(input: &Map<String, Value>) -> Result<ai6::AssistantSettings, String> {
    let key = input.get("key").and_then(Value::as_str).unwrap_or_default();
    if key != ai6::ConfigRecord::SINGLETON_KEY {
        return Err(format!(
            "the assistant record is keyed {:?}",
            ai6::ConfigRecord::SINGLETON_KEY
        ));
    }
    let definition = input.get("definition").cloned().unwrap_or(json!({}));
    let settings = serde_json::from_value::<ai6::AssistantSettings>(definition)
        .map_err(|reason| format!("assistant: {reason}"))?;
    settings.validate()?;
    Ok(settings)
}

/// Every write tool, sorted by name.
pub(crate) fn write_tools() -> &'static [WriteTool] {
    WRITE_TOOLS
}

static WRITE_TOOLS: &[WriteTool] = &[
    WriteTool {
        name: "propose_class_authorization",
        description: "Propose a class authorisation (a standing approval of a kind of \
                      irreversible step) in an Operation. Nothing changes until the person \
                      confirms it in the app. Needs approval.class.",
        schema: || {
            object(
                &[
                    operation(),
                    ("kinds", strings("The kinds of step it approves.")),
                    ("target_pattern", text("Which targets, as a pattern.")),
                    (
                        "agents",
                        strings("Agents it covers (\"*\" for every Agent)."),
                    ),
                    ("roles", strings("Roles it covers.")),
                    (
                        "expires_in_secs",
                        json!({"type": "integer", "description": "How long it lasts."}),
                    ),
                    (
                        "max_uses",
                        json!({"type": "integer", "description": "How often it may be used."}),
                    ),
                ],
                &["operation_id", "kinds", "target_pattern", "expires_in_secs"],
            )
        },
        plan: |input, _| {
            let operation = id(input, "operation_id")?;
            let pattern = string(input, "target_pattern").ok_or("target_pattern is required")?;
            let mut body = json!({
                "kinds": string_list(input, "kinds"),
                "target_pattern": pattern,
                "agents": string_list(input, "agents"),
                "roles": string_list(input, "roles"),
                "expires_in_secs": input.get("expires_in_secs").cloned().unwrap_or(json!(0)),
            });
            if let Some(uses) = input.get("max_uses") {
                body["max_uses"] = uses.clone();
            }
            Ok(Draft {
                summary: format!(
                    "Authorise {} steps on {pattern:?} in Operation {operation} for {} seconds",
                    Value::Array(string_list(input, "kinds")),
                    body["expires_in_secs"]
                ),
                call: post(
                    format!(
                        "/v1/user/operations/{}/class-authorizations",
                        encode(operation)
                    ),
                    Some(body),
                ),
                scope: Scope::Operation(operation.to_owned()),
                action: "approval.class",
            })
        },
    },
    WriteTool {
        name: "propose_held_brief_decision",
        description: "Propose releasing a held brief back to the queue, or refusing it. \
                      Nothing changes until the person confirms it in the app. Needs \
                      mission.create.",
        schema: || {
            object(
                &[
                    operation(),
                    (
                        "key",
                        text("The held brief's key, as held_briefs lists it."),
                    ),
                    (
                        "decision",
                        json!({"type": "string", "enum": ["release", "refuse"]}),
                    ),
                    ("reason", text("Why, for a refusal.")),
                ],
                &["operation_id", "key", "decision"],
            )
        },
        plan: |input, _| {
            let operation = id(input, "operation_id")?;
            let key = id(input, "key")?;
            let decision = string(input, "decision").ok_or("decision is required")?;
            let body = (decision == "refuse").then(|| match string(input, "reason") {
                Some(reason) => json!({"reason": reason}),
                None => json!({}),
            });
            Ok(Draft {
                summary: format!("{decision} held brief {key} in Operation {operation}"),
                call: post(
                    format!(
                        "/v1/user/operations/{}/held-briefs/{}/{decision}",
                        encode(operation),
                        encode(key)
                    ),
                    body,
                ),
                scope: Scope::Operation(operation.to_owned()),
                action: "mission.create",
            })
        },
    },
    WriteTool {
        name: "propose_mission",
        description: "Propose queuing a new Mission in an Operation, with its title and \
                      prompt. Nothing changes until the person confirms it in the app. \
                      Needs mission.create.",
        schema: || {
            object(
                &[
                    operation(),
                    ("title", text("A short title.")),
                    ("prompt", text("The brief: what the Agent should do.")),
                    ("agent_id", text("Only this Agent.")),
                    ("role", text("The role the Mission needs.")),
                    ("stations", strings("Only these Stations.")),
                ],
                &["operation_id", "title", "prompt"],
            )
        },
        plan: |input, _| {
            let operation = id(input, "operation_id")?;
            let title = string(input, "title").ok_or("title is required")?;
            let prompt = string(input, "prompt").ok_or("prompt is required")?;
            let mut body = json!({"title": title, "prompt": prompt});
            if let Some(agent) = optional_id(input, "agent_id")? {
                body["agent_id"] = json!(agent);
            }
            if let Some(role) = optional_id(input, "role")? {
                body["role"] = json!(role);
            }
            if input.contains_key("stations") {
                body["stations"] = Value::Array(string_list(input, "stations"));
            }
            Ok(Draft {
                summary: format!("Queue a Mission in Operation {operation}: {title}"),
                call: post(
                    format!("/v1/user/operations/{}/missions", encode(operation)),
                    Some(body),
                ),
                scope: Scope::Operation(operation.to_owned()),
                action: "mission.create",
            })
        },
    },
    WriteTool {
        name: "propose_mission_control",
        description: "Propose cancelling, retrying, resuming (with a message) or \
                      interrupting a Mission. Nothing changes until the person confirms it \
                      in the app. Needs mission.control.",
        schema: || {
            object(
                &[
                    operation(),
                    ("mission_id", text("The Mission.")),
                    (
                        "action",
                        json!({"type": "string",
                               "enum": ["cancel", "retry", "resume", "interrupt"]}),
                    ),
                    ("agent", text("Retry with this Agent.")),
                    ("message", text("Resume with this message.")),
                ],
                &["operation_id", "mission_id", "action"],
            )
        },
        plan: |input, proposal_id| {
            let operation = id(input, "operation_id")?;
            let mission = id(input, "mission_id")?;
            let action = string(input, "action").ok_or("action is required")?;
            let mut body = json!({"action": action, "idempotency_key": proposal_id});
            if let Some(agent) = optional_id(input, "agent")? {
                body["agent"] = json!(agent);
            }
            if let Some(message) = string(input, "message") {
                body["message"] = json!(message);
            }
            Ok(Draft {
                summary: format!("{action} Mission {mission} in Operation {operation}"),
                call: post(
                    format!(
                        "/v1/user/operations/{}/missions/{}/actions",
                        encode(operation),
                        encode(mission)
                    ),
                    Some(body),
                ),
                scope: Scope::Operation(operation.to_owned()),
                action: "mission.control",
            })
        },
    },
    WriteTool {
        name: "propose_mission_proposal_decision",
        description: "Propose approving (queuing) or rejecting a Mission an Agent proposed. \
                      Nothing changes until the person confirms it in the app. Needs \
                      mission.create.",
        schema: || {
            object(
                &[
                    operation(),
                    ("proposal_id", text("As mission_proposals lists it.")),
                    (
                        "decision",
                        json!({"type": "string", "enum": ["approve", "reject"]}),
                    ),
                ],
                &["operation_id", "proposal_id", "decision"],
            )
        },
        plan: |input, _| {
            let operation = id(input, "operation_id")?;
            let proposal = id(input, "proposal_id")?;
            let decision = string(input, "decision").ok_or("decision is required")?;
            Ok(Draft {
                summary: format!("{decision} Mission proposal {proposal} in Operation {operation}"),
                call: post(
                    format!(
                        "/v1/user/operations/{}/mission-proposals/{}/{decision}",
                        encode(operation),
                        encode(proposal)
                    ),
                    None,
                ),
                scope: Scope::Operation(operation.to_owned()),
                action: "mission.create",
            })
        },
    },
    WriteTool {
        name: "propose_roster_edit",
        description: "Propose adding or replacing one roster record (an Operation, Agent, \
                      role, cover, or your own [assistant] settings) with a definition: the \
                      keys of its table in the configuration files. The roster checks the \
                      whole roster when it is confirmed. Nothing changes until the person \
                      confirms it in the app; a setting looser than its default is named in \
                      the proposal. Needs roster.manage.",
        schema: || {
            object(
                &[
                    ("kind", roster_kind()),
                    (
                        "key",
                        text("The record's key: Operation, codename, role, cover or assistant."),
                    ),
                    (
                        "definition",
                        json!({"type": "object", "description": "The record's settings."}),
                    ),
                ],
                &["kind", "key", "definition"],
            )
        },
        plan: |input, _| {
            let kind = string(input, "kind").ok_or("kind is required")?;
            let key = id(input, "key")?;
            let definition = input.get("definition").cloned().unwrap_or(json!({}));
            // What an `assistant` record changes is added by `WriteTool::draft`.
            let summary = format!("Set roster {kind}/{key} to {definition}");
            Ok(Draft {
                summary,
                call: Call {
                    method: Method::PUT,
                    path: format!("/v1/user/roster/{}/{}", encode(kind), encode(key)),
                    body: Some(definition),
                },
                scope: Scope::Resource("config:roster".to_owned()),
                action: "roster.manage",
            })
        },
    },
    WriteTool {
        name: "propose_roster_removal",
        description: "Propose removing one roster record. Nothing changes until the person \
                      confirms it in the app. Needs roster.manage.",
        schema: || {
            object(
                &[("kind", roster_kind()), ("key", text("The record's key."))],
                &["kind", "key"],
            )
        },
        plan: |input, _| {
            let kind = string(input, "kind").ok_or("kind is required")?;
            let key = id(input, "key")?;
            Ok(Draft {
                summary: format!("Remove roster {kind}/{key}"),
                call: Call {
                    method: Method::DELETE,
                    path: format!("/v1/user/roster/{}/{}", encode(kind), encode(key)),
                    body: None,
                },
                scope: Scope::Resource("config:roster".to_owned()),
                action: "roster.manage",
            })
        },
    },
    WriteTool {
        name: "propose_station_settings",
        description: "Propose a Station's limits: concurrent Missions, and optionally the \
                      free memory (MiB) and CPU (percent) past which it takes no new \
                      Missions. Nothing changes until the person confirms it in the app. \
                      Needs station.configure.",
        schema: || {
            object(
                &[
                    ("station_id", text("The Station.")),
                    (
                        "max_concurrent_missions",
                        json!({"type": "integer", "description": "1 to 1,024."}),
                    ),
                    ("min_free_memory_mb", json!({"type": "integer"})),
                    ("max_cpu_percent", json!({"type": "integer"})),
                ],
                &["station_id", "max_concurrent_missions"],
            )
        },
        plan: |input, _| {
            let station = id(input, "station_id")?;
            let mut body = json!({"max_concurrent_missions": input["max_concurrent_missions"]});
            for key in ["min_free_memory_mb", "max_cpu_percent"] {
                if let Some(value) = input.get(key) {
                    body[key] = value.clone();
                }
            }
            Ok(Draft {
                summary: format!("Set Station {station}'s limits to {body}"),
                call: Call {
                    method: Method::PUT,
                    path: format!("/v1/user/stations/{}/settings", encode(station)),
                    body: Some(body),
                },
                scope: Scope::Resource(format!("station:{station}")),
                action: "station.configure",
            })
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_tools_are_sorted_strict_and_never_send_unchecked_input() {
        let names: Vec<_> = write_tools().iter().map(|tool| tool.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert!(names.iter().all(|name| name.starts_with("propose_")));
        for tool in write_tools() {
            assert_eq!(
                tool.schema()["additionalProperties"],
                false,
                "{}",
                tool.name
            );
        }
        let edit = write_tools()
            .iter()
            .find(|tool| tool.name == "propose_roster_edit")
            .unwrap();
        let draft = edit
            .draft(
                &json!({"kind": "agents", "key": "007/../x", "definition": {"handler": "claude"}}),
                "p-1",
                None,
            )
            .unwrap();
        assert_eq!(draft.call.path, "/v1/user/roster/agents/007%2F..%2Fx");
        assert_eq!(draft.scope, Scope::Resource("config:roster".to_owned()));
        assert!(
            edit.draft(
                &json!({"kind": "secrets", "key": "k", "definition": {}}),
                "p",
                None,
            )
            .is_err()
        );
        let strict = edit
            .draft(
                &json!({"kind": "assistant", "key": "settings",
                        "definition": {"personal_repo": "/srv/mp/{person_digest}.git"}}),
                "p-2",
                Some(
                    &ai6::AssistantSettings::new()
                        .with_personal_repo("/srv/mp/{person_digest}.git"),
                ),
            )
            .unwrap();
        assert_eq!(strict.call.path, "/v1/user/roster/assistant/settings");
        assert!(!strict.summary.contains("Weakens"), "{}", strict.summary);
        let loose = edit
            .draft(
                &json!({"kind": "assistant", "key": "settings",
                        "definition": {"allow_elevated": true}}),
                "p-3",
                Some(&ai6::AssistantSettings::default()),
            )
            .unwrap();
        assert!(
            loose
                .summary
                .contains("Weakens: assistant.allow_elevated = true"),
            "{}",
            loose.summary
        );
        for refused in [
            json!({"kind": "assistant", "key": "moneypenny", "definition": {}}),
            json!({"kind": "assistant", "key": "settings", "definition": {"unknown": 1}}),
            json!({"kind": "assistant", "key": "settings",
                   "definition": {"personal_repo": "ext::sh"}}),
        ] {
            let error = edit
                .draft(&refused, "p", Some(&ai6::AssistantSettings::default()))
                .unwrap_err();
            assert!(error.text.starts_with("INVALID_INPUT"), "{}", error.text);
        }
        // Compared with the settings in force, not the defaults: a record that drops a group's
        // narrowing and moves the personal repository weakens nothing against the defaults.
        let current = ai6::AssistantSettings::new()
            .with_personal_repo("/srv/mp/{person_digest}.git")
            .with_group(
                "contractors",
                ai6::AssistantNarrowing::new().with_interpreters(Vec::new()),
            );
        let quiet = json!({"kind": "assistant", "key": "settings",
                           "definition": {"personal_repo": "/srv/other/{person_digest}.git"}});
        let moved = edit.draft(&quiet, "p-4", Some(&current)).unwrap();
        for named in [
            "assistant.group.contractors is removed",
            "assistant.personal_repo changes",
        ] {
            assert!(moved.summary.contains(named), "{}", moved.summary);
        }
        let unknown = edit.draft(&quiet, "p-5", None).unwrap_err();
        assert!(
            unknown.text.starts_with("Nothing was proposed"),
            "{}",
            unknown.text
        );
        let control = write_tools()
            .iter()
            .find(|tool| tool.name == "propose_mission_control")
            .unwrap()
            .draft(
                &json!({"operation_id": "G", "mission_id": "m", "action": "cancel"}),
                "p-7",
                None,
            )
            .unwrap();
        assert_eq!(
            control.call.body,
            Some(json!({"action": "cancel", "idempotency_key": "p-7"}))
        );
    }

    #[test]
    fn a_proposal_with_hidden_characters_is_refused() {
        let mission = write_tools()
            .iter()
            .find(|tool| tool.name == "propose_mission")
            .unwrap();
        for hidden in ["\u{202E}", "\u{200B}", "\u{E0041}", "\u{FE0F}"] {
            let refused = mission
                .draft(
                    &json!({"operation_id": "G", "title": "Tidy up",
                            "prompt": format!("Delete the cache{hidden}")}),
                    "p",
                    None,
                )
                .unwrap_err();
            assert!(
                refused.text.contains("hidden character"),
                "{}",
                refused.text
            );
        }
        let edit = write_tools()
            .iter()
            .find(|tool| tool.name == "propose_roster_edit")
            .unwrap();
        let nested = json!({"kind": "agents", "key": "008",
                            "definition": {"context": ["fine", "sneaky\u{2066}"]}});
        assert!(edit.draft(&nested, "p", None).is_err());
        let plain = json!({"operation_id": "G", "title": "Tidy up", "prompt": "Delete the cache"});
        assert!(mission.draft(&plain, "p", None).is_ok());
    }
}
