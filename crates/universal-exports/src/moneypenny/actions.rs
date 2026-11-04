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
            Self::Operation(operation) => crate::tree::operation_resource(operation),
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

/// What a proposal rests on when she makes it: the read tools she called this turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Evidence {
    pub(crate) read: Vec<super::store::ReadCall>,
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
    /// For a Cable Moneypenny asked to send: the Station holding it, which sends it once
    /// confirmed. Such a proposal is not a gateway request.
    pub(crate) cable: Option<CableHold>,
    /// What she had read when she made it.
    pub(crate) evidence: Evidence,
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
/// most `[assistant.limits] change_bytes` of text and a one-line subject of 1 to
/// `[assistant.gateway] change_message_chars` characters. Answers group, path, content and
/// subject.
pub(crate) fn shared_change<'a>(
    input: &'a Value,
    limits: &ai6::AssistantLimits,
    gateway: &ai6::AssistantGatewayLimits,
) -> Result<(&'a str, &'a str, &'a str, &'a str), Outcome> {
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
    if !limits.change_bytes.allows(content.len()) {
        return Err(invalid(format!(
            "content is longer than {} bytes",
            limits.change_bytes
        )));
    }
    let chars = gateway.change_message_chars;
    if message.trim().is_empty() || !chars.allows(message.chars().count()) || message.contains('\n')
    {
        return Err(invalid(format!(
            "message is one line of 1 to {chars} characters"
        )));
    }
    Ok((group, path, content, message))
}

impl Proposal {
    /// How the app shows it: the exact request, the summary and the status; with what she
    /// had read (`read`, when she read anything this turn) and `opens`, the link of the page it
    /// changes.
    pub(crate) fn shown(&self) -> Value {
        let mut shown = json!({
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
        });
        if !self.evidence.read.is_empty() {
            shown["read"] = json!(self.evidence.read);
        }
        if let Some(route) = super::routes::opens(self.tool, &self.draft.call) {
            shown["opens"] = Value::from(route.to_uri());
        }
        shown
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
    let settings = ai6::read_definition::<ai6::AssistantSettings>(&definition)
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
        name: "propose_access_change",
        description: "Propose one change of the access policy: set or remove a role, replace \
                      the whole ordered rule list, or set the delegation cap (global only). \
                      With operation_id it is that Operation's own policy, else the global \
                      one. Name the revision you read with the access tool. The server \
                      refuses what the person could not grant themselves, and a stale \
                      revision. Nothing changes until the person confirms it in the app. \
                      Needs access.manage.",
        schema: || {
            object(
                &[
                    (
                        "operation_id",
                        text("An Operation, for its own policy; omit for the global one."),
                    ),
                    (
                        "change",
                        json!({"type": "string",
                               "enum": ["set_role", "remove_role", "set_rules", "set_delegable"],
                               "description": "What to change."}),
                    ),
                    ("revision", text("The revision the access tool showed.")),
                    ("role", text("The role, for set_role and remove_role.")),
                    (
                        "actions",
                        strings(
                            "The role's actions, or the cap's, for set_role and set_delegable.",
                        ),
                    ),
                    ("description", text("What the role is for, for set_role.")),
                    (
                        "rules",
                        json!({"type": "array", "description": "The whole ordered list.",
                               "items": {"type": "object", "properties": {
                                   "subject": {"type": "string"},
                                   "scope": {"type": "string"},
                                   "role": {"type": "string"},
                                   "propagate": {"type": "boolean"}}}}),
                    ),
                ],
                &["change", "revision"],
            )
        },
        plan: |input, _| {
            let change = string(input, "change").ok_or("change is required")?;
            let revision = id(input, "revision")?;
            let operation = optional_id(input, "operation_id")?;
            let base = match operation {
                Some(operation) => format!("/v1/user/operations/{}/access", encode(operation)),
                None => "/v1/user/access".to_owned(),
            };
            let query = format!("?revision={}", encode(revision));
            let (method, tail, body, summary) = match change {
                "set_role" => {
                    let role = id(input, "role")?;
                    let mut body =
                        json!({"actions": input.get("actions").cloned().unwrap_or(json!([]))});
                    if let Some(description) = string(input, "description") {
                        body["description"] = json!(description);
                    }
                    let summary = format!("Set role {role} to {}", body["actions"]);
                    (
                        Method::PUT,
                        format!("/roles/{}", encode(role)),
                        Some(body),
                        summary,
                    )
                }
                "remove_role" => {
                    let role = id(input, "role")?;
                    (
                        Method::DELETE,
                        format!("/roles/{}", encode(role)),
                        None,
                        format!("Remove role {role}"),
                    )
                }
                "set_rules" => {
                    let rules = input.get("rules").cloned().ok_or("rules is required")?;
                    let summary = format!("Replace the rule list with {rules}");
                    (
                        Method::PUT,
                        "/rules".to_owned(),
                        Some(json!({ "rules": rules })),
                        summary,
                    )
                }
                "set_delegable" if operation.is_none() => {
                    let actions = input.get("actions").cloned().ok_or("actions is required")?;
                    let summary = format!("Set the delegation cap to {actions}");
                    (
                        Method::PUT,
                        "/delegable".to_owned(),
                        Some(json!({ "actions": actions })),
                        summary,
                    )
                }
                _ => return Err("that change is not available here".to_owned()),
            };
            let (scope, summary) = match operation {
                Some(operation) => (
                    Scope::Operation(operation.to_owned()),
                    format!("{summary} for Operation {operation}"),
                ),
                None => (Scope::Resource("config:access".to_owned()), summary),
            };
            Ok(Draft {
                summary,
                call: Call {
                    method,
                    path: format!("{base}{tail}{query}"),
                    body,
                },
                scope,
                action: "access.manage",
            })
        },
    },
    WriteTool {
        name: "propose_account_change",
        description: "Propose an account change: create one, disable or enable it, set its \
                      groups (which also needs access.manage), or reset its password. A \
                      password is never shown to you: the app shows it to the person once \
                      after they confirm. Nothing changes until the person confirms it in \
                      the app. Needs account.manage.",
        schema: || {
            object(
                &[
                    ("name", text("The account name.")),
                    (
                        "change",
                        json!({"type": "string",
                               "enum": ["create", "disable", "enable", "set_groups",
                                        "reset_password"],
                               "description": "What to do."}),
                    ),
                    ("groups", strings("The groups, for set_groups and create.")),
                ],
                &["name", "change"],
            )
        },
        plan: |input, _| {
            let name = id(input, "name")?;
            let change = string(input, "change").ok_or("change is required")?;
            let path = format!("/v1/user/accounts/{}", encode(name));
            let (method, path, body, summary) = match change {
                "create" => {
                    let body = input
                        .get("groups")
                        .map_or(json!({}), |groups| json!({ "groups": groups }));
                    (
                        Method::PUT,
                        path,
                        Some(body),
                        format!("Create account {name}"),
                    )
                }
                "disable" => (
                    Method::PUT,
                    path,
                    Some(json!({"disabled": true})),
                    format!("Disable account {name}"),
                ),
                "enable" => (
                    Method::PUT,
                    path,
                    Some(json!({"disabled": false})),
                    format!("Enable account {name}"),
                ),
                "set_groups" => {
                    let groups = input.get("groups").cloned().ok_or("groups is required")?;
                    let summary = format!("Set the groups of account {name} to {groups}");
                    (
                        Method::PUT,
                        path,
                        Some(json!({ "groups": groups })),
                        summary,
                    )
                }
                "reset_password" => (
                    Method::POST,
                    format!("{path}/secret"),
                    None,
                    format!(
                        "Reset the password of account {name} (shown to the person once, not to me)"
                    ),
                ),
                _ => return Err("unknown change".to_owned()),
            };
            Ok(Draft {
                summary,
                call: Call { method, path, body },
                scope: Scope::Resource("config:accounts".to_owned()),
                action: "account.manage",
            })
        },
    },
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
        name: "propose_issue_comment",
        description: "Propose posting a comment on a linked issue, under the Operation's \
                      identity with the person's name in a footer. On an issue with an open \
                      question it is the answer, and the Mission resumes. Nothing changes \
                      until the person confirms it in the app. Needs issue.comment.",
        schema: || {
            object(
                &[
                    operation(),
                    (
                        "mission_id",
                        text("The Mission: its UID or any handle it had."),
                    ),
                    ("issue", text("The linked issue, as TRACKER:ITEM.")),
                    ("body", text("The exact text to post.")),
                ],
                &["operation_id", "mission_id", "issue", "body"],
            )
        },
        plan: |input, _| {
            let operation = id(input, "operation_id")?;
            let mission = id(input, "mission_id")?;
            let issue = id(input, "issue")?;
            let body = string(input, "body").ok_or("body is required")?;
            Ok(Draft {
                summary: format!("comment on issue {issue}: {body}"),
                call: post(
                    format!(
                        "/v1/user/operations/{}/missions/{}/issues/{}/comments",
                        encode(operation),
                        encode(mission),
                        encode(issue)
                    ),
                    Some(json!({"body": body})),
                ),
                scope: Scope::Operation(operation.to_owned()),
                action: "issue.comment",
            })
        },
    },
    WriteTool {
        name: "propose_issue_create",
        description: "Propose creating an issue in one of the Operation's trackers, as a child \
                      of a linked issue when `parent` is set. The card shows the destination \
                      and the exact text. Nothing changes until the person confirms it in the \
                      app. Needs issue.create; a label that starts work also needs \
                      mission.create.",
        schema: || {
            object(
                &[
                    operation(),
                    ("tracker", text("The tracker instance to create it in.")),
                    ("title", text("The title.")),
                    ("body", text("The body, as Markdown.")),
                    ("labels", strings("Labels to set.")),
                    (
                        "issue_type",
                        text("The issue type, where the tracker has them."),
                    ),
                    ("priority", text("The priority, in the tracker's words.")),
                    (
                        "parent",
                        text("The linked issue it is a child of, as TRACKER:ITEM."),
                    ),
                    ("mission", text("The Mission to link it to.")),
                ],
                &["operation_id", "tracker", "title"],
            )
        },
        plan: |input, _| {
            let operation = id(input, "operation_id")?;
            let tracker = id(input, "tracker")?;
            let title = string(input, "title").ok_or("title is required")?;
            let mut body = Map::new();
            for key in [
                "tracker",
                "title",
                "body",
                "labels",
                "issue_type",
                "priority",
                "parent",
                "mission",
            ] {
                if let Some(value) = input.get(key) {
                    body.insert(key.to_owned(), value.clone());
                }
            }
            Ok(Draft {
                summary: format!("create an issue in {tracker}: {title}"),
                call: post(
                    format!("/v1/user/operations/{}/issues", encode(operation)),
                    Some(Value::Object(body)),
                ),
                scope: Scope::Operation(operation.to_owned()),
                action: "issue.create",
            })
        },
    },
    WriteTool {
        name: "propose_issue_link",
        description: "Propose linking an issue to a Mission, accepting a link an Agent \
                      proposed, or unlinking one. Nothing changes until the person confirms \
                      it in the app. Needs issue.link.",
        schema: || {
            object(
                &[
                    operation(),
                    (
                        "mission_id",
                        text("The Mission: its UID or any handle it had."),
                    ),
                    ("issue", text("The issue, as TRACKER:ITEM.")),
                    (
                        "decision",
                        json!({"type": "string", "enum": ["link", "accept", "unlink"]}),
                    ),
                    (
                        "relation",
                        json!({"type": "string", "enum": ["related", "child", "parent", "mirror"]}),
                    ),
                ],
                &["operation_id", "mission_id", "issue", "decision"],
            )
        },
        plan: |input, _| {
            let operation = id(input, "operation_id")?;
            let mission = id(input, "mission_id")?;
            let issue = id(input, "issue")?;
            let decision = string(input, "decision").ok_or("decision is required")?;
            let base = format!(
                "/v1/user/operations/{}/missions/{}/issues",
                encode(operation),
                encode(mission)
            );
            let call = match decision {
                "link" => {
                    let mut body = json!({"issue": issue});
                    if let Some(relation) = string(input, "relation") {
                        body["relation"] = json!(relation);
                    }
                    post(base, Some(body))
                }
                "accept" => post(format!("{base}/{}/accept", encode(issue)), None),
                "unlink" => Call {
                    method: Method::DELETE,
                    path: format!("{base}/{}", encode(issue)),
                    body: None,
                },
                _ => return Err("decision is link, accept or unlink".to_owned()),
            };
            Ok(Draft {
                summary: format!("{decision} issue {issue} on Mission {mission}"),
                call,
                scope: Scope::Operation(operation.to_owned()),
                action: "issue.link",
            })
        },
    },
    WriteTool {
        name: "propose_issue_proposal_decision",
        description: "Propose approving or rejecting what an Agent asked to create. The card \
                      shows the destination, the exact text, the fields it cannot carry over \
                      and `mark_ready` when set. Nothing changes until the person confirms it \
                      in the app. Needs issue.create; mark_ready also needs mission.create.",
        schema: || {
            object(
                &[
                    operation(),
                    (
                        "proposal",
                        text("The proposal, as issue_proposals lists it."),
                    ),
                    (
                        "decision",
                        json!({"type": "string", "enum": ["approve", "reject"]}),
                    ),
                    ("mark_ready", json!({"type": "boolean"})),
                    ("reason", text("Why, for a rejection.")),
                ],
                &["operation_id", "proposal", "decision"],
            )
        },
        plan: |input, _| {
            let operation = id(input, "operation_id")?;
            let proposal = id(input, "proposal")?;
            let decision = string(input, "decision").ok_or("decision is required")?;
            let mark_ready = input.get("mark_ready").and_then(Value::as_bool) == Some(true);
            if decision != "approve" && decision != "reject" {
                return Err("decision is approve or reject".to_owned());
            }
            let body = if decision == "approve" {
                json!({"mark_ready": mark_ready})
            } else {
                match string(input, "reason") {
                    Some(reason) => json!({"reason": reason}),
                    None => json!({}),
                }
            };
            Ok(Draft {
                summary: format!(
                    "{decision} issue proposal {proposal} in Operation {operation}{}",
                    if mark_ready { " and mark it ready" } else { "" }
                ),
                call: post(
                    format!(
                        "/v1/user/operations/{}/issue-proposals/{}/{decision}",
                        encode(operation),
                        encode(proposal)
                    ),
                    Some(body),
                ),
                scope: Scope::Operation(operation.to_owned()),
                action: "issue.create",
            })
        },
    },
    WriteTool {
        name: "propose_issue_update",
        description: "Propose changing a linked issue: labels, a workflow transition, closing \
                      or reopening it, or its assignees. Nothing changes until the person \
                      confirms it in the app. Needs issue.update.",
        schema: || {
            object(
                &[
                    operation(),
                    (
                        "mission_id",
                        text("The Mission: its UID or any handle it had."),
                    ),
                    ("issue", text("The linked issue, as TRACKER:ITEM.")),
                    ("add_labels", strings("Labels to add.")),
                    ("remove_labels", strings("Labels to remove.")),
                    ("transition", text("A workflow transition by name.")),
                    ("close", json!({"type": "boolean"})),
                    ("reopen", json!({"type": "boolean"})),
                    ("add_assignees", strings("Assignees to add.")),
                    ("remove_assignees", strings("Assignees to remove.")),
                ],
                &["operation_id", "mission_id", "issue"],
            )
        },
        plan: |input, _| {
            let operation = id(input, "operation_id")?;
            let mission = id(input, "mission_id")?;
            let issue = id(input, "issue")?;
            let mut body = Map::new();
            for key in [
                "add_labels",
                "remove_labels",
                "transition",
                "close",
                "reopen",
                "add_assignees",
                "remove_assignees",
            ] {
                if let Some(value) = input.get(key) {
                    body.insert(key.to_owned(), value.clone());
                }
            }
            if body.is_empty() {
                return Err("name something to change".to_owned());
            }
            let body = Value::Object(body);
            Ok(Draft {
                summary: format!("change issue {issue}: {body}"),
                call: post(
                    format!(
                        "/v1/user/operations/{}/missions/{}/issues/{}/update",
                        encode(operation),
                        encode(mission),
                        encode(issue)
                    ),
                    Some(body),
                ),
                scope: Scope::Operation(operation.to_owned()),
                action: "issue.update",
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
                    (
                        "mission_id",
                        text("The Mission: its UID or any handle it had."),
                    ),
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
    fn the_access_tools_name_the_revision_and_the_scope() {
        let find = |name: &str| write_tools().iter().find(|tool| tool.name == name).unwrap();
        let draft = find("propose_access_change")
            .draft(
                &json!({"change": "set_role", "role": "reviewer", "revision": "abc",
                        "actions": ["mission.read"], "operation_id": "P"}),
                "p1",
                None,
            )
            .unwrap();
        assert_eq!(draft.action, "access.manage");
        assert_eq!(draft.scope, Scope::Operation("P".to_owned()));
        assert_eq!(
            draft.call.path,
            "/v1/user/operations/P/access/roles/reviewer?revision=abc"
        );
        let global = find("propose_access_change")
            .draft(
                &json!({"change": "set_delegable", "revision": "0", "actions": []}),
                "p2",
                None,
            )
            .unwrap();
        assert_eq!(global.scope, Scope::Resource("config:access".to_owned()));
        assert!(
            find("propose_access_change")
                .draft(
                    &json!({"change": "set_delegable", "revision": "0", "operation_id": "P",
                            "actions": []}),
                    "p3",
                    None,
                )
                .is_err(),
            "the cap is global"
        );
        let reset = find("propose_account_change")
            .draft(
                &json!({"name": "ann", "change": "reset_password"}),
                "p4",
                None,
            )
            .unwrap();
        assert_eq!(reset.call.path, "/v1/user/accounts/ann/secret");
        assert_eq!(reset.action, "account.manage");
    }

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
    fn every_write_tool_opens_the_page_it_changes_through_a_route_that_round_trips() {
        use super::super::routes::{Limits, Route, opens};
        let limits = Limits::default();
        let cases = [
            (
                "propose_access_change",
                json!({"operation_id": "G", "change": "set_rules", "revision": "r", "rules": []}),
                Some("app:/access?tab=rules&operation_id=G"),
            ),
            (
                "propose_account_change",
                json!({"name": "ann", "change": "disable"}),
                Some("app:/access?tab=accounts"),
            ),
            (
                "propose_class_authorization",
                json!({"operation_id": "G", "kinds": ["push"], "target_pattern": "origin:*",
                       "expires_in_secs": 60}),
                Some("app:/green_ink?tab=classes&operation_id=G"),
            ),
            (
                "propose_held_brief_decision",
                json!({"operation_id": "G", "key": "source/1 x", "decision": "release"}),
                Some("app:/briefings?tab=held&operation_id=G&item_id=source%2F1%20x"),
            ),
            (
                "propose_mission",
                json!({"operation_id": "G", "title": "t", "prompt": "p"}),
                Some("app:/missions?operation_id=G&state=queued"),
            ),
            (
                "propose_mission_control",
                json!({"operation_id": "G", "mission_id": "m-1", "action": "cancel"}),
                Some("app:/missions?operation_id=G&mission_id=m-1"),
            ),
            (
                "propose_mission_proposal_decision",
                json!({"operation_id": "G", "proposal_id": "pr-1", "decision": "approve"}),
                Some("app:/briefings?tab=proposals&operation_id=G&item_id=pr-1"),
            ),
            (
                "propose_roster_edit",
                json!({"kind": "agents", "key": "007", "definition": {"handler": "claude"}}),
                Some("app:/roster?kind=agent&key=007"),
            ),
            (
                "propose_roster_removal",
                json!({"kind": "covers", "key": "tourist"}),
                Some("app:/roster?kind=cover&key=tourist"),
            ),
            (
                "propose_station_settings",
                json!({"station_id": "hq", "max_concurrent_missions": 4}),
                Some("app:/stations?station_id=hq&tab=settings"),
            ),
            (
                "propose_issue_link",
                json!({"operation_id": "G", "mission_id": "m", "issue": "gh:o/r#1", "decision": "link"}),
                Some("app:/missions?operation_id=G&mission_id=m"),
            ),
            (
                "propose_issue_comment",
                json!({"operation_id": "G", "mission_id": "m", "issue": "gh:o/r#1", "body": "x"}),
                Some("app:/missions?operation_id=G&mission_id=m"),
            ),
            (
                "propose_issue_update",
                json!({"operation_id": "G", "mission_id": "m", "issue": "gh:o/r#1", "close": true}),
                Some("app:/missions?operation_id=G&mission_id=m"),
            ),
            (
                "propose_issue_create",
                json!({"operation_id": "G", "tracker": "gh", "title": "x"}),
                Some("app:/briefings?tab=proposals&operation_id=G"),
            ),
            (
                "propose_issue_proposal_decision",
                json!({"operation_id": "G", "proposal": "ip-1", "decision": "approve"}),
                Some("app:/briefings?tab=proposals&operation_id=G&item_id=ip-1"),
            ),
        ];
        for tool in write_tools() {
            assert!(
                cases.iter().any(|(name, _, _)| *name == tool.name),
                "{} has a case",
                tool.name
            );
        }
        for (name, input, expected) in cases {
            let tool = write_tools().iter().find(|tool| tool.name == name).unwrap();
            let draft = tool.draft(&input, "p", None).unwrap();
            let route = opens(name, &draft.call).unwrap_or_else(|| panic!("{name} opens"));
            assert_eq!(Some(route.to_uri().as_str()), expected, "{name}");
            assert_eq!(
                Route::parse_uri(&route.to_uri(), limits),
                Ok(route),
                "{name}"
            );
        }
        // A shared repository, a Cable and a memory file have no page.
        let none = Call {
            method: Method::POST,
            path: "shared/ops/runbook.md".to_owned(),
            body: None,
        };
        assert!(opens(SHARED_CHANGE, &none).is_none());
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
