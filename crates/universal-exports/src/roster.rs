//! Roster administration: the Operations, Agents, Agent roles, cover policies and Moneypenny's
//! settings every Station reads, edited through a roster source that takes changes (a SQL
//! roster).
//!
//! Every Station polls the same source, so an accepted change reaches each without a restart.
//! The source checks the whole roster before it keeps a change: a refused edit changes
//! nothing, and is audited as an accepted one is. The routes need `roster.read` or
//! `roster.manage` on the resource `config:roster`. A write may name the revision of the record
//! it read in `If-Match`, so an edit made meanwhile is not overwritten.

use ai6::{AssistantSettings, AuditRecord, ConfigRecord, Person, RosterError, RosterErrorKind};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::access_admin::token;
use crate::{
    ApiError, Gateway, GatewayError, bearer, resolve_user, resource_action_allowed, unix_timestamp,
};

/// The authorization resource of the roster.
pub(crate) const ROSTER_RESOURCE: &str = "config:roster";
/// One roster record as its source holds it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterRecord {
    /// The Operation key, Agent codename, role ID, cover name, or `assistant`.
    pub key: String,
    /// A JSON object with the keys of the record's table in the configuration files.
    pub definition: serde_json::Value,
    /// A digest of the stored record, for `If-Match` on a write.
    pub revision: String,
}

/// The records of one kind.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterRecords {
    /// `operations`, `agents`, `roles`, `covers` or `assistant` (at most one record, keyed
    /// `settings`).
    pub kind: String,
    /// The roster revision they were read at, in decimal; Stations report the revision they
    /// run at check-in.
    pub revision: String,
    /// Whether the source takes edits.
    pub writable: bool,
    /// The records, by key.
    pub records: Vec<RosterRecord>,
}

/// One Operation as its Stations resolve it: its definition with its ancestors' keys layered
/// in, and which Operation wrote each key.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectiveOperation {
    /// Operation codename.
    pub operation_id: String,
    /// The Operations it is subordinate to, nearest first.
    pub lineage: Vec<String>,
    /// Whether it takes policy from its ancestors.
    pub inherit: bool,
    /// The keys of its `[operation.<id>]` table with its ancestors' layered in, before
    /// presets and defaults; a key no Operation wrote has its default and is absent.
    pub definition: serde_json::Map<String, serde_json::Value>,
    /// Each written key, dotted (`green_ink.kinds`), and the Operation that wrote it.
    pub origins: std::collections::BTreeMap<String, String>,
    /// The roster revision it was read at, in decimal.
    pub revision: String,
}

/// An accepted edit.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterChange {
    /// The roster revision the edit moved to, in decimal.
    pub revision: String,
    /// What the stored `assistant` record loosens or redirects against the settings in force
    /// before it ([`AssistantSettings::changes_from`]); omitted when there is nothing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub weakened: Vec<String>,
}

/// Why the source refused an edit (HTTP 422).
#[derive(Serialize)]
struct Refused {
    refused: String,
}

/// An edit named a revision the record no longer has (HTTP 409).
#[derive(Serialize)]
struct Changed {
    /// The record's revision now; `0` when it is gone.
    changed: String,
}

/// A kind of record the routes serve, and what a person needs to read or change it.
#[derive(Clone)]
struct Family {
    record: ConfigRecord,
    /// The resource the actions are held on.
    resource: String,
    read: &'static str,
    manage: &'static str,
}

/// The route kinds gated by `config.read` and `config.manage` on `config:<family>`, and the
/// family each names.
pub(crate) const CONFIG_KINDS: [(&str, ConfigRecord); 13] = [
    ("plugins", ConfigRecord::Plugin),
    ("tls", ConfigRecord::Tls),
    ("cables", ConfigRecord::Cables),
    ("unex", ConfigRecord::Unex),
    ("watches", ConfigRecord::Watch),
    ("theaters", ConfigRecord::Theater),
    ("loadouts", ConfigRecord::Loadout),
    ("sandboxes", ConfigRecord::Sandbox),
    ("mcps", ConfigRecord::Mcp),
    ("overlays", ConfigRecord::Overlay),
    ("secrets", ConfigRecord::Secret),
    ("secret_providers", ConfigRecord::SecretProvider),
    ("pools", ConfigRecord::Pool),
];

/// The route kinds gated by `roster.read` and `roster.manage` on `config:roster`.
const ROSTER_KINDS: [(&str, ConfigRecord); 7] = [
    ("operations", ConfigRecord::Operation),
    ("agents", ConfigRecord::Agent),
    ("roles", ConfigRecord::Role),
    ("covers", ConfigRecord::Cover),
    ("reviews", ConfigRecord::Review),
    ("assistant", ConfigRecord::Assistant),
    ("roster", ConfigRecord::Roster),
];

/// The family a route kind names, in the family's name or its plural (`watch`, `watches`).
fn family_of(kind: &str) -> Result<Family, ApiError> {
    let record =
        ai6::directorates::m::config::family_of_kind(kind).ok_or_else(ApiError::not_found)?;
    if CONFIG_KINDS.iter().any(|(_, served)| *served == record) {
        return Ok(Family {
            record,
            resource: format!("config:{}", record.name()),
            read: "config.read",
            manage: "config.manage",
        });
    }
    if ROSTER_KINDS.iter().any(|(_, served)| *served == record) {
        return Ok(Family {
            record,
            resource: ROSTER_RESOURCE.to_owned(),
            read: "roster.read",
            manage: "roster.manage",
        });
    }
    Err(ApiError::not_found())
}

/// Refuses a person who does not hold `action` on the family's resource, naming the family.
async fn require(
    gateway: &Gateway,
    person: &Person,
    family: &Family,
    action: &'static str,
) -> Result<(), ApiError> {
    if resource_action_allowed(gateway, person.clone(), family.resource.clone(), action).await? {
        Ok(())
    } else {
        Err(ApiError::refused(
            StatusCode::FORBIDDEN,
            format!(
                "the {} family needs {action} on {}",
                family.record.name(),
                family.resource
            ),
        ))
    }
}

/// The audit target of a record: `watch operation:G`, or with a Station's own `loadout 007 (station:x)`.
fn target_of(record: ConfigRecord, scope: &ai6::ConfigScope, key: &str) -> String {
    match scope {
        ai6::ConfigScope::Global => format!("{} {key}", record.name()),
        scope => format!("{} {key} ({scope})", record.name()),
    }
}

/// The Station a request names, for a family that may have records of its own per Station.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Scoped {
    station: Option<String>,
    /// For a removal: go ahead though another record names this one.
    #[serde(default)]
    force: bool,
    /// For a write: store a record of a trust-root family though it loosens what is in force.
    #[serde(default)]
    confirm: bool,
}

fn scope_of(record: ConfigRecord, scoped: &Scoped) -> Result<ai6::ConfigScope, ApiError> {
    match scoped.station.as_deref() {
        None => Ok(ai6::ConfigScope::Global),
        Some(id) if record.station_scoped() && ai6::ConfigScope::is_plain(id) => {
            Ok(ai6::ConfigScope::Station(id.to_owned()))
        }
        Some(_) => Err(ApiError::bad_request()),
    }
}

/// What the changed records of a trust-root family give up that the family's stored records
/// do not already give up.
fn loosened(
    source: &dyn ai6::ConfigProvider,
    record: ConfigRecord,
    scope: &ai6::ConfigScope,
    changes: &[(String, serde_json::Value)],
) -> Result<Vec<String>, RosterError> {
    if !ai6::directorates::m::config::TRUST_ROOT.contains(&record) {
        return Ok(Vec::new());
    }
    let current = source.definitions(record, scope)?.records;
    let mut after = current.clone();
    for (key, definition) in changes {
        match after.iter_mut().find(|(found, _)| found == key) {
            Some(held) => held.1 = definition.clone(),
            None => after.push((key.clone(), definition.clone())),
        }
    }
    let before = ai6::directorates::m::config::record_weakenings(record, &current);
    Ok(
        ai6::directorates::m::config::record_weakenings(record, &after)
            .into_iter()
            .filter(|line| !before.contains(line))
            .collect(),
    )
}

fn checked_key(key: &str, bytes: ai6::Limit) -> Result<(), ApiError> {
    if key.trim().is_empty() || !bytes.allows(key.len()) || key.chars().any(char::is_control) {
        Err(ApiError::bad_request())
    } else {
        Ok(())
    }
}

fn source(gateway: &Gateway) -> Result<std::sync::Arc<dyn ai6::ConfigProvider>, ApiError> {
    gateway.roster.clone().ok_or_else(ApiError::unavailable)
}

/// `text` on one line, cut to the audit target's bound at a character boundary.
pub(crate) fn bounded(text: &str, bytes: ai6::Limit) -> String {
    let mut line = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if !bytes.allows(line.len()) {
        let mut end = bytes.or_max().saturating_sub('…'.len_utf8());
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    line
}

async fn audit(
    gateway: &Gateway,
    person: &Person,
    resource: &str,
    action: &str,
    target: &str,
) -> Result<(), ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let record = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: crate::actor_of(person),
        action: action.to_owned(),
        operation: String::new(),
        resource: Some(resource.to_owned()),
        target: bounded(target, gateway.config.unex().text.roster_target_bytes),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())
}

/// `GET /v1/user/roster/{kind}`: needs `roster.read`.
pub(crate) async fn records(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    Query(scoped): Query<Scoped>,
) -> Result<Json<RosterRecords>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let family = family_of(&kind)?;
    let scope = scope_of(family.record, &scoped)?;
    require(&gateway, &person, &family, family.read).await?;
    let source = source(&gateway)?;
    let record = family.record;
    let definitions = tokio::task::spawn_blocking(move || source.definitions(record, &scope))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|error| match error.kind() {
            RosterErrorKind::ReadOnly => ApiError::from(GatewayError::Conflict),
            _ => ApiError::unavailable(),
        })?;
    Ok(Json(RosterRecords {
        kind,
        revision: definitions.revision.get().to_string(),
        writable: definitions.writable,
        records: definitions
            .records
            .into_iter()
            .map(|(key, definition)| RosterRecord {
                revision: token(Some(&definition)),
                key,
                definition,
            })
            .collect(),
    }))
}

/// `GET /v1/user/roster/{kind}/schema`: needs `roster.read`. The record's schema, the form that
/// shows it and how its key is asked for, as its type declares them.
pub(crate) async fn form(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Result<Json<ai6::RecordForm>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let family = family_of(&kind)?;
    require(&gateway, &person, &family, family.read).await?;
    match family.record.form() {
        Ok(Some(form)) => Ok(Json(form)),
        Ok(None) => Err(ApiError::not_found()),
        Err(_) => Err(ApiError::internal()),
    }
}

/// The revision a write names in `If-Match`, when it names one.
fn expected_revision(headers: &HeaderMap) -> Option<String> {
    headers
        .get("if-match")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().trim_matches('"').to_owned())
}

/// Stores `definition` under `key`. With `expected`, only while the stored record has that
/// revision (`0`: there is none); otherwise whatever is there is replaced.
fn store(
    source: &dyn ai6::ConfigProvider,
    record: ConfigRecord,
    scope: &ai6::ConfigScope,
    key: &str,
    expected: Option<&str>,
    definition: &serde_json::Value,
) -> Result<Result<ai6::ConfigRevision, Changed>, ai6::RosterError> {
    let Some(expected) = expected else {
        return source.put(record, scope, key, definition).map(Ok);
    };
    let stored = stored_record(source, record, scope, key)?;
    let now = token(stored.as_ref());
    if now != expected {
        return Ok(Err(Changed { changed: now }));
    }
    match source.put_if(record, scope, key, stored.as_ref(), definition) {
        Err(error) if error.kind() == RosterErrorKind::Conflict => {
            let now = stored_record(source, record, scope, key)?;
            Ok(Err(Changed {
                changed: token(now.as_ref()),
            }))
        }
        outcome => outcome.map(Ok),
    }
}

fn stored_record(
    source: &dyn ai6::ConfigProvider,
    record: ConfigRecord,
    scope: &ai6::ConfigScope,
    key: &str,
) -> Result<Option<serde_json::Value>, ai6::RosterError> {
    Ok(source
        .definitions(record, scope)?
        .records
        .into_iter()
        .find(|(stored, _)| stored == key)
        .map(|(_, definition)| definition))
}

fn changed_response(changed: Changed) -> Response {
    (StatusCode::CONFLICT, Json(changed)).into_response()
}

/// `PUT` on the path the schema route holds: a record may be keyed `schema`.
pub(crate) async fn put_record_keyed_schema(
    state: State<Gateway>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    scoped: Query<Scoped>,
    definition: Json<serde_json::Value>,
) -> Result<Response, ApiError> {
    put_record(
        state,
        headers,
        Path((kind, "schema".to_owned())),
        scoped,
        definition,
    )
    .await
}

/// `DELETE` on the path the schema route holds: a record may be keyed `schema`.
pub(crate) async fn remove_record_keyed_schema(
    state: State<Gateway>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    scoped: Query<Scoped>,
) -> Result<Response, ApiError> {
    remove_record(state, headers, Path((kind, "schema".to_owned())), scoped).await
}

/// `GET /v1/user/roster/operation/{id}/effective`: needs `roster.read`.
pub(crate) async fn effective(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<EffectiveOperation>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    checked_key(&id, gateway.config.unex().text.roster_key_bytes)?;
    if !resource_action_allowed(&gateway, person, ROSTER_RESOURCE.to_owned(), "roster.read").await?
    {
        return Err(ApiError::forbidden());
    }
    let source = source(&gateway)?;
    let snapshot = tokio::task::spawn_blocking(move || source.load(""))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::unavailable())?;
    let operation = snapshot
        .roster
        .operation(&id)
        .map_err(|_| ApiError::not_found())?;
    Ok(Json(EffectiveOperation {
        operation_id: operation.id.clone(),
        lineage: snapshot
            .roster
            .lineage(&operation.id)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        inherit: operation.inherit,
        definition: operation.definition().clone(),
        origins: operation.origins().clone(),
        revision: snapshot.revision.get().to_string(),
    }))
}

/// What an edit came to: accepted at a revision, refused with a reason, or a failure.
/// `weakened` is what an accepted edit loosens.
async fn settle(
    gateway: &Gateway,
    person: &Person,
    resource: &str,
    target: String,
    outcome: Result<Option<ai6::ConfigRevision>, ai6::RosterError>,
    accepted: &str,
    weakened: Vec<String>,
) -> Result<Response, ApiError> {
    match outcome {
        Ok(Some(revision)) => {
            audit(gateway, person, resource, accepted, &target).await?;
            Ok(Json(RosterChange {
                revision: revision.get().to_string(),
                weakened,
            })
            .into_response())
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(error) => match error.kind() {
            RosterErrorKind::Invalid => {
                let reason = error.to_string();
                audit(
                    gateway,
                    person,
                    resource,
                    "roster.refused",
                    &format!("{target}: {reason}"),
                )
                .await?;
                Ok((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(Refused { refused: reason }),
                )
                    .into_response())
            }
            RosterErrorKind::ReadOnly => Err(ApiError::from(GatewayError::Conflict)),
            _ => Err(ApiError::unavailable()),
        },
    }
}

/// `PUT /v1/user/roster/{kind}/{key}` with the record's definition as a JSON object: needs
/// `roster.manage`. 200 with the new revision, and for the `assistant` record what it loosens
/// or redirects against the settings in force (`weakened`);
/// 422 with the reason when the roster it would leave is not acceptable; 409 when the source
/// takes no edits, or with `changed` when `If-Match` names a revision the record no longer has.
pub(crate) async fn put_record(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((kind, key)): Path<(String, String)>,
    Query(scoped): Query<Scoped>,
    Json(definition): Json<serde_json::Value>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let family = family_of(&kind)?;
    let record = family.record;
    let scope = scope_of(record, &scoped)?;
    checked_key(&key, gateway.config.unex().text.roster_key_bytes)?;
    if !definition.is_object() {
        return Err(ApiError::bad_request());
    }
    require(&gateway, &person, &family, family.manage).await?;
    let source = source(&gateway)?;
    let target = target_of(record, &scope, &key);
    let expected = expected_revision(&headers);
    let confirm = scoped.confirm;
    let outcome = tokio::task::spawn_blocking(move || {
        if !confirm {
            let held = loosened(
                source.as_ref(),
                record,
                &scope,
                &[(key.clone(), definition.clone())],
            )?;
            if !held.is_empty() {
                return Ok(Ok((None, held)));
            }
        }
        // What an `assistant` record changes is told against the settings in force just
        // before it, and only for an accepted edit, whose definition the source parsed the
        // same way.
        let current = (record == ConfigRecord::Assistant)
            .then(|| source.load(""))
            .transpose()?
            .map(|snapshot| snapshot.roster.assistant().clone());
        let revision = match store(
            source.as_ref(),
            record,
            &scope,
            &key,
            expected.as_deref(),
            &definition,
        )? {
            Ok(revision) => revision,
            Err(changed) => return Ok(Err(changed)),
        };
        let weakened = current
            .zip(ai6::read_definition::<AssistantSettings>(&definition).ok())
            .map(|(current, settings)| settings.changes_from(&current))
            .unwrap_or_default();
        Ok(Ok((Some(revision), weakened)))
    })
    .await
    .map_err(|_| ApiError::internal())?;
    let (outcome, weakened) = match outcome {
        Ok(Ok((None, weakened))) => {
            return Ok(Json(Held {
                applied: false,
                weakened,
            })
            .into_response());
        }
        Ok(Ok((Some(revision), weakened))) => (Ok(Some(revision)), weakened),
        Ok(Err(changed)) => return Ok(changed_response(changed)),
        Err(error) => (Err(error), Vec::new()),
    };
    settle(
        &gateway,
        &person,
        &family.resource,
        target,
        outcome,
        "roster.update",
        weakened,
    )
    .await
}

/// `DELETE /v1/user/roster/{kind}/{key}`: needs `roster.manage`. 200 with the new revision;
/// 404 when there is no such record; 422 when the roster left would not be acceptable (an
/// Agent another extends, say); 409 with `changed` as for `PUT`.
pub(crate) async fn remove_record(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((kind, key)): Path<(String, String)>,
    Query(scoped): Query<Scoped>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let family = family_of(&kind)?;
    let record = family.record;
    let scope = scope_of(record, &scoped)?;
    checked_key(&key, gateway.config.unex().text.roster_key_bytes)?;
    require(&gateway, &person, &family, family.manage).await?;
    let source = source(&gateway)?;
    let target = target_of(record, &scope, &key);
    let expected = expected_revision(&headers);
    let force = scoped.force;
    let outcome = tokio::task::spawn_blocking(move || -> Result<Removal, RosterError> {
        if let Some(expected) = expected {
            let now = token(stored_record(source.as_ref(), record, &scope, &key)?.as_ref());
            if now != expected {
                return Ok(Removal::Stale(Changed { changed: now }));
            }
        }
        let names = match ai6::directorates::m::config::references_to(
            source.as_ref(),
            record,
            &scope,
            &key,
        ) {
            Ok(names) => names,
            Err(error) if error.kind() == RosterErrorKind::Invalid => return Ok(Removal::Gone),
            Err(error) => return Err(error),
        };
        let names: Vec<String> = names.iter().map(ToString::to_string).collect();
        if !names.is_empty() && !force {
            return Ok(Removal::Named(names));
        }
        Ok(match source.remove(record, &scope, &key)? {
            Some(revision) => Removal::Removed(revision, names),
            None => Removal::Gone,
        })
    })
    .await
    .map_err(|_| ApiError::internal())?;
    match outcome {
        Ok(Removal::Stale(changed)) => Ok(changed_response(changed)),
        Ok(Removal::Gone) => Err(ApiError::not_found()),
        Ok(Removal::Named(names)) => {
            let reason = format!("still named by {}; change those first", names.join(", "));
            audit(
                &gateway,
                &person,
                &family.resource,
                "roster.refused",
                &format!("{target}: {reason}"),
            )
            .await?;
            Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(Refused { refused: reason }),
            )
                .into_response())
        }
        Ok(Removal::Removed(revision, names)) => {
            let target = if names.is_empty() {
                target
            } else {
                format!("{target}, forced: still named by {}", names.join(", "))
            };
            settle(
                &gateway,
                &person,
                &family.resource,
                target,
                Ok(Some(revision)),
                "roster.remove",
                Vec::new(),
            )
            .await
        }
        Err(error) => {
            settle(
                &gateway,
                &person,
                &family.resource,
                target,
                Err(error),
                "roster.remove",
                Vec::new(),
            )
            .await
        }
    }
}

/// What a removal came to.
enum Removal {
    /// The record has another revision than `If-Match` named.
    Stale(Changed),
    /// There is no such record.
    Gone,
    /// Other records name it, and the request did not force.
    Named(Vec<String>),
    /// Removed, and what still named it.
    Removed(ai6::ConfigRevision, Vec<String>),
}

/// An edit held back because it loosens what is in force; repeat it with `confirm=true`.
#[derive(Serialize)]
struct Held {
    applied: bool,
    weakened: Vec<String>,
}

/// A request to store records of any served families together.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyRequest {
    /// A Station's own records, for the families that have them; absent: the global ones.
    station: Option<String>,
    /// Check and report; store nothing.
    #[serde(default)]
    dry_run: bool,
    /// Store though the records loosen what is in force.
    #[serde(default)]
    confirm: bool,
    records: Vec<ApplyRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyRecord {
    /// The family's name or its plural.
    kind: String,
    key: String,
    definition: serde_json::Value,
}

/// One record of an apply, and what it came to.
#[derive(Serialize)]
struct AppliedRecord {
    kind: String,
    key: String,
    /// `created`, `changed` or `unchanged`.
    change: &'static str,
    /// The record's revision once stored.
    revision: String,
}

/// What an apply came to.
#[derive(Serialize)]
struct Applied {
    /// Whether the records are stored: false for a dry run, and for records that loosen what is
    /// in force until the request is repeated with `confirm`.
    applied: bool,
    /// The configuration revision stored at; absent when nothing was stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    records: Vec<AppliedRecord>,
    /// What the records loosen or redirect against what is in force.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    weakened: Vec<String>,
}

/// `POST /v1/user/resources/apply`: stores records of any served families in one transaction,
/// all or none. Needs the family's manage action for each family named, and audits each record
/// stored or refused. 200 with what each came to; 422 with the reason when the configuration it
/// would leave is not acceptable; 409 when the source takes no edits.
pub(crate) async fn apply(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(request): Json<ApplyRequest>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    if request.records.is_empty() {
        return Err(ApiError::invalid("the request holds no records".to_owned()));
    }
    let key_bytes = gateway.config.unex().text.roster_key_bytes;
    let mut records: Vec<(Family, String, serde_json::Value)> = Vec::new();
    for entry in request.records {
        let family = family_of(&entry.kind)
            .map_err(|_| ApiError::invalid(format!("no kind {:?} is managed here", entry.kind)))?;
        checked_key(&entry.key, key_bytes)?;
        if !entry.definition.is_object() {
            return Err(ApiError::invalid(format!(
                "{} {}: a record is a table",
                family.record, entry.key
            )));
        }
        if records
            .iter()
            .any(|(other, key, _)| other.record == family.record && *key == entry.key)
        {
            return Err(ApiError::invalid(format!(
                "{} {} is given twice",
                family.record, entry.key
            )));
        }
        records.push((family, entry.key, entry.definition));
    }
    let scoped = Scoped {
        station: request.station,
        force: false,
        confirm: false,
    };
    let mut seen = Vec::new();
    for (family, _, _) in &records {
        if !seen.contains(&family.record) {
            seen.push(family.record);
            require(&gateway, &person, family, family.manage).await?;
            scope_of(family.record, &scoped).map_err(|_| {
                ApiError::invalid(format!(
                    "{} has no records of a Station's own",
                    family.record
                ))
            })?;
        }
    }
    let scope = match scoped.station.as_deref() {
        Some(id) => ai6::ConfigScope::Station(id.to_owned()),
        None => ai6::ConfigScope::Global,
    };
    let source = source(&gateway)?;
    let held: Vec<(ConfigRecord, String, serde_json::Value)> = records
        .iter()
        .map(|(family, key, definition)| (family.record, key.clone(), definition.clone()))
        .collect();
    let (dry_run, confirm) = (request.dry_run, request.confirm);
    let read_scope = scope.clone();
    let outcome = tokio::task::spawn_blocking(move || -> Result<Applying, RosterError> {
        let mut changes = Vec::new();
        let mut stored = Vec::new();
        for (record, key, definition) in &held {
            let now = stored_record(source.as_ref(), *record, &read_scope, key)?;
            let change = match &now {
                None => "created",
                Some(now) if now == definition => "unchanged",
                Some(_) => "changed",
            };
            changes.push((*record, key.clone(), change, token(Some(definition))));
            if change != "unchanged" {
                stored.push((*record, key.clone(), definition.clone()));
            }
        }
        // What an `assistant` record changes is told against the settings in force just before it.
        let mut weakened = Vec::new();
        for (record, _, definition) in &stored {
            if *record == ConfigRecord::Assistant
                && let Ok(settings) = ai6::read_definition::<AssistantSettings>(definition)
            {
                let current = source.load("")?.roster.assistant().clone();
                weakened.extend(settings.changes_from(&current));
            }
        }
        for trusted in ai6::directorates::m::config::TRUST_ROOT {
            let changes: Vec<(String, serde_json::Value)> = stored
                .iter()
                .filter(|(record, _, _)| *record == trusted)
                .map(|(_, key, definition)| (key.clone(), definition.clone()))
                .collect();
            if !changes.is_empty() {
                weakened.extend(loosened(source.as_ref(), trusted, &read_scope, &changes)?);
            }
        }
        if stored.is_empty() {
            return Ok(Applying::Done(changes, None, weakened));
        }
        if dry_run {
            source.check_all(&read_scope, &stored)?;
            return Ok(Applying::Held(changes, weakened));
        }
        if !weakened.is_empty() && !confirm {
            source.check_all(&read_scope, &stored)?;
            return Ok(Applying::Held(changes, weakened));
        }
        let revision = source.put_all(&read_scope, &stored)?;
        Ok(Applying::Done(changes, revision, weakened))
    })
    .await
    .map_err(|_| ApiError::internal())?;
    let targets: Vec<(String, String)> = records
        .iter()
        .map(|(family, key, _)| {
            (
                family.resource.clone(),
                target_of(family.record, &scope, key),
            )
        })
        .collect();
    let view = |changes: Vec<(ConfigRecord, String, &'static str, String)>| {
        changes
            .into_iter()
            .map(|(record, key, change, revision)| AppliedRecord {
                kind: record.name().to_owned(),
                key,
                change,
                revision,
            })
            .collect::<Vec<_>>()
    };
    match outcome {
        Ok(Applying::Held(changes, weakened)) => Ok(Json(Applied {
            applied: false,
            revision: None,
            records: view(changes),
            weakened,
        })
        .into_response()),
        Ok(Applying::Done(changes, revision, weakened)) => {
            for ((resource, target), (_, _, change, _)) in targets.iter().zip(&changes) {
                if *change != "unchanged" {
                    audit(&gateway, &person, resource, "roster.update", target).await?;
                }
            }
            Ok(Json(Applied {
                applied: true,
                revision: revision.map(|revision| revision.get().to_string()),
                records: view(changes),
                weakened,
            })
            .into_response())
        }
        Err(error) => match error.kind() {
            RosterErrorKind::Invalid => {
                let reason = error.to_string();
                for (resource, target) in &targets {
                    audit(
                        &gateway,
                        &person,
                        resource,
                        "roster.refused",
                        &format!("{target}: {reason}"),
                    )
                    .await?;
                }
                Ok((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(Refused { refused: reason }),
                )
                    .into_response())
            }
            RosterErrorKind::ReadOnly => Err(ApiError::from(GatewayError::Conflict)),
            _ => Err(ApiError::unavailable()),
        },
    }
}

/// What an apply did before it answered.
enum Applying {
    /// Checked, and not stored: a dry run, or records that loosen something.
    Held(
        Vec<(ConfigRecord, String, &'static str, String)>,
        Vec<String>,
    ),
    /// Stored at a revision (none when nothing differed).
    Done(
        Vec<(ConfigRecord, String, &'static str, String)>,
        Option<ai6::ConfigRevision>,
        Vec<String>,
    ),
}

/// One kind the routes serve.
#[derive(Serialize)]
pub(crate) struct Resource {
    /// The family's name: what a document's table and a command's kind are called.
    kind: &'static str,
    /// The kind in a route (`/v1/user/roster/{route}`).
    route: &'static str,
    /// The resource the actions are held on.
    resource: String,
    read: &'static str,
    manage: &'static str,
    /// Which of the two the person holds.
    allowed: Vec<&'static str>,
    /// Whether a Station may have records of its own (`?station=`).
    station_scoped: bool,
    /// Whether the family has one record, keyed `settings`.
    singleton: bool,
    /// Whether a running Station applies a change without a restart.
    live: bool,
}

/// `GET /v1/user/resources`: the kinds of record this gateway manages, the action each needs
/// and which the person holds.
pub(crate) async fn resources(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Vec<Resource>>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let mut listed = Vec::new();
    for (route, record) in ROSTER_KINDS.into_iter().chain(CONFIG_KINDS) {
        let family = family_of(route)?;
        let mut allowed = Vec::new();
        for action in [family.read, family.manage] {
            if resource_action_allowed(&gateway, person.clone(), family.resource.clone(), action)
                .await?
            {
                allowed.push(action);
            }
        }
        listed.push(Resource {
            kind: record.name(),
            route,
            resource: family.resource,
            read: family.read,
            manage: family.manage,
            allowed,
            station_scoped: record.station_scoped(),
            singleton: record.is_singleton(),
            live: record.applies_live(),
        });
    }
    Ok(Json(listed))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ai6::directorates::registry::backend::*;
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::controls::tests::{Served, served_with_roster};

    const ROLES: &str = r#"
        [role.roster-readers]
        actions = ["roster.read"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "config:roster"
        role = "roster-readers"

        [role.roster-managers]
        actions = ["roster.read", "roster.manage", "audit.read"]

        [[rule]]
        subject = "person:operator-007"
        scope = "config:roster"
        role = "roster-managers"
    "#;

    async fn call(
        served: &Served,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> reqwest::Response {
        let request = reqwest::Client::new()
            .request(method, format!("{}/v1/user/{path}", served.base))
            .bearer_auth(token);
        match body {
            Some(body) => request.json(&body),
            None => request,
        }
        .send()
        .await
        .unwrap()
    }

    fn audits(served: &Served, action: &str) -> Vec<ai6::AuditRecord> {
        served
            .registry
            .list_audit(100)
            .unwrap()
            .into_iter()
            .filter(|record| record.action == action)
            .collect()
    }

    #[tokio::test]
    async fn without_a_roster_source_the_routes_are_unavailable() {
        let served = served_with_roster(ROLES, None).await;
        let response = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "roster/agents",
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_configuration_directory_is_listed_but_takes_no_edits() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("agents.toml"),
            "[agent.007]\nhandler = \"mock-agent\"\n",
        )
        .unwrap();
        let source = Arc::new(ai6::FileProvider::new(directory.path()));
        let served = served_with_roster(ROLES, Some(source)).await;
        let listed: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/agents",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listed["writable"], false);
        assert_eq!(listed["records"][0]["key"], "007");
        assert_eq!(listed["records"][0]["definition"]["handler"], "mock-agent");
        let refused = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/agents/007",
            Some(json!({"handler": "mock-agent", "priority": 2})),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert!(audits(&served, "roster.update").is_empty());
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn an_edit_writes_through_the_source_and_a_bad_one_is_refused_and_audited() {
        use ai6::{ConfigProvider, ConfigRecord, SqlProvider};

        let directory = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", directory.path().join("roster.db").display());
        let source = Arc::new(SqlProvider::open(&location).unwrap());
        source
            .put(
                ConfigRecord::Operation,
                &ai6::ConfigScope::Global,
                "G",
                &json!({"roster": ["007"]}),
            )
            .unwrap();
        source
            .put(
                ConfigRecord::Agent,
                &ai6::ConfigScope::Global,
                "007",
                &json!({"handler": "mock-agent", "priority": 1}),
            )
            .unwrap();
        let served = served_with_roster(ROLES, Some(source.clone())).await;

        let profile: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "profile",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(
            profile["roster"],
            json!(["roster.read", "roster.manage", "audit.read"])
        );

        let listed: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/agents",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listed["kind"], "agents");
        assert_eq!(listed["writable"], true);
        assert_eq!(
            listed["records"],
            json!([{
                "key": "007",
                "definition": {"handler": "mock-agent", "priority": 1},
                "revision": listed["records"][0]["revision"].clone(),
            }])
        );
        let viewer_edit = call(
            &served,
            &served.viewer,
            reqwest::Method::PUT,
            "roster/agents/007",
            Some(json!({"handler": "mock-agent", "priority": 9})),
        )
        .await;
        assert_eq!(viewer_edit.status(), StatusCode::FORBIDDEN);

        let edited = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/agents/007",
            Some(json!({"handler": "mock-agent", "priority": 9})),
        )
        .await;
        assert_eq!(edited.status(), StatusCode::OK);
        let edited: serde_json::Value = edited.json().await.unwrap();
        let revision = source.revision("").unwrap();
        assert_eq!(edited["revision"], revision.get().to_string());
        assert_eq!(
            source
                .load("")
                .unwrap()
                .roster
                .agent(Some("G"), "007")
                .unwrap()
                .priority,
            9
        );
        let updates = audits(&served, "roster.update");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].actor, "operator-007");
        assert_eq!(updates[0].resource.as_deref(), Some("config:roster"));
        assert_eq!(updates[0].target, "agent 007");

        let refused = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/agents/009",
            Some(json!({"clearance": "root"})),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let refused: serde_json::Value = refused.json().await.unwrap();
        assert!(
            refused["refused"].as_str().unwrap().contains("clearance"),
            "{refused}"
        );
        assert_eq!(source.revision("").unwrap(), revision, "nothing changed");
        let refusals = audits(&served, "roster.refused");
        assert_eq!(refusals.len(), 1);
        assert!(
            refusals[0].target.starts_with("agent 009: "),
            "{refusals:?}"
        );

        let audit: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "audit",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        let actions = audit
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["action"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(actions.contains(&"roster.refused"), "{actions:?}");

        let missing = call(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/roles/nobody",
            None,
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let unknown = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "roster/compartments",
            None,
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_form_of_each_kind_is_served_to_a_person_who_may_read_the_roster() {
        let directory = tempfile::tempdir().unwrap();
        let source = Arc::new(ai6::FileProvider::new(directory.path()));
        let served = served_with_roster(ROLES, Some(source)).await;
        for kind in [
            "operations",
            "agents",
            "roles",
            "covers",
            "reviews",
            "assistant",
            "roster",
        ] {
            let path = format!("roster/{kind}/schema");
            let answer = call(&served, &served.viewer, reqwest::Method::GET, &path, None).await;
            assert_eq!(answer.status(), StatusCode::OK, "{kind}");
            let answer: serde_json::Value = answer.json().await.unwrap();
            assert_eq!(answer["schema"]["type"], "object", "{kind}: {answer}");
            assert!(answer["form"].is_object(), "{kind}: {answer}");
            assert!(answer["key"]["title"].is_string(), "{kind}: {answer}");
        }
        let agent: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/agents/schema",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(agent["schema"]["properties"]["handler"]["title"], "Handler");

        let unknown = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/compartments/schema",
            None,
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_form_is_refused_without_roster_read() {
        let directory = tempfile::tempdir().unwrap();
        let source = Arc::new(ai6::FileProvider::new(directory.path()));
        let served = served_with_roster("", Some(source)).await;
        let refused = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/agents/schema",
            None,
        )
        .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn a_write_that_names_a_stale_revision_changes_nothing() {
        use ai6::{ConfigProvider, SqlProvider};

        let directory = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", directory.path().join("roster.db").display());
        let source = Arc::new(SqlProvider::open(&location).unwrap());
        let served = served_with_roster(ROLES, Some(source.clone())).await;
        let put = |path: &'static str, revision: Option<String>, body: serde_json::Value| {
            let served = &served;
            async move {
                let mut request = reqwest::Client::new()
                    .put(format!("{}/v1/user/{path}", served.base))
                    .bearer_auth(&served.operator)
                    .json(&body);
                if let Some(revision) = revision {
                    request = request.header("if-match", format!("\"{revision}\""));
                }
                request.send().await.unwrap()
            }
        };

        // `0` adds: it refuses a record that is there.
        let added = put(
            "roster/agents/007",
            Some("0".to_owned()),
            json!({"handler": "mock-agent", "priority": 1}),
        )
        .await;
        assert_eq!(added.status(), StatusCode::OK);
        let again = put(
            "roster/agents/007",
            Some("0".to_owned()),
            json!({"handler": "mock-agent", "priority": 9}),
        )
        .await;
        assert_eq!(again.status(), StatusCode::CONFLICT);

        let listed: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/agents",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        let read = listed["records"][0]["revision"]
            .as_str()
            .unwrap()
            .to_owned();
        let first = put(
            "roster/agents/007",
            Some(read.clone()),
            json!({"handler": "mock-agent", "priority": 2}),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let stale = put(
            "roster/agents/007",
            Some(read.clone()),
            json!({"handler": "mock-agent", "priority": 3}),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let stale: serde_json::Value = stale.json().await.unwrap();
        assert_ne!(stale["changed"], read, "{stale}");
        assert_eq!(
            source
                .load("")
                .unwrap()
                .roster
                .agent(None, "007")
                .unwrap()
                .priority,
            2
        );

        let stale_remove = reqwest::Client::new()
            .delete(format!("{}/v1/user/roster/agents/007", served.base))
            .bearer_auth(&served.operator)
            .header("if-match", format!("\"{read}\""))
            .send()
            .await
            .unwrap();
        assert_eq!(stale_remove.status(), StatusCode::CONFLICT);
        assert!(source.load("").unwrap().roster.agent(None, "007").is_ok());

        // Named or not, a person who holds it can still write without one.
        let unconditional = put(
            "roster/agents/007",
            None,
            json!({"handler": "mock-agent", "priority": 4}),
        )
        .await;
        assert_eq!(unconditional.status(), StatusCode::OK);
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn a_record_keyed_schema_is_edited_and_removed_like_any_other() {
        use ai6::SqlProvider;

        let directory = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", directory.path().join("roster.db").display());
        let source = Arc::new(SqlProvider::open(&location).unwrap());
        let served = served_with_roster(ROLES, Some(source)).await;
        let edited = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/agents/schema",
            Some(json!({"handler": "mock-agent"})),
        )
        .await;
        assert_eq!(edited.status(), StatusCode::OK);
        let removed = call(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/agents/schema",
            None,
        )
        .await;
        assert_eq!(removed.status(), StatusCode::OK);
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn the_assistant_record_is_edited_like_any_other_and_reports_its_weakenings() {
        use ai6::{ConfigProvider, SqlProvider};

        let directory = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", directory.path().join("roster.db").display());
        let source = Arc::new(SqlProvider::open(&location).unwrap());
        let served = served_with_roster(ROLES, Some(source.clone())).await;

        let listed: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/assistant",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listed["kind"], "assistant");
        assert_eq!(listed["writable"], true);
        assert_eq!(listed["records"], json!([]));

        let viewer_edit = call(
            &served,
            &served.viewer,
            reqwest::Method::PUT,
            "roster/assistant/settings",
            Some(json!({"allow_elevated": true})),
        )
        .await;
        assert_eq!(viewer_edit.status(), StatusCode::FORBIDDEN);

        let strict = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/assistant/settings",
            Some(json!({"personal_repo": "/srv/mp/{person_digest}.git"})),
        )
        .await;
        assert_eq!(strict.status(), StatusCode::OK);
        let strict: serde_json::Value = strict.json().await.unwrap();
        // Looser than nothing in force, but it moves where people's memory goes.
        assert_eq!(
            strict["weakened"].as_array().map(Vec::len),
            Some(1),
            "{strict}"
        );
        assert!(
            strict["weakened"][0]
                .as_str()
                .unwrap()
                .starts_with("assistant.personal_repo changes"),
            "{strict}"
        );

        let loose = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/assistant/settings",
            Some(json!({"allow_elevated": true, "org": {"repo": "/srv/org.git", "write": true}})),
        )
        .await;
        assert_eq!(loose.status(), StatusCode::OK);
        let loose: serde_json::Value = loose.json().await.unwrap();
        let weakened = loose["weakened"].as_array().unwrap();
        assert_eq!(weakened.len(), 4, "{loose}");
        assert!(
            weakened[0]
                .as_str()
                .unwrap()
                .contains("assistant.allow_elevated"),
            "{loose}"
        );
        // The same record again changes nothing in force, so nothing is reported.
        let again = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/assistant/settings",
            Some(json!({"allow_elevated": true, "org": {"repo": "/srv/org.git", "write": true}})),
        )
        .await;
        assert_eq!(again.status(), StatusCode::OK);
        let again: serde_json::Value = again.json().await.unwrap();
        assert!(again.get("weakened").is_none(), "{again}");
        let settings = source.load("").unwrap().roster.assistant().clone();
        assert!(settings.allow_elevated());
        assert_eq!(
            settings.personal_repo(),
            None,
            "the record is replaced whole"
        );
        let revision = source.revision("").unwrap();

        for (key, definition, reason) in [
            ("moneypenny", json!({}), "\"settings\""),
            (
                "settings",
                json!({"personal_repo": "ext::sh"}),
                "assistant.personal_repo",
            ),
        ] {
            let refused = call(
                &served,
                &served.operator,
                reqwest::Method::PUT,
                &format!("roster/assistant/{key}"),
                Some(definition),
            )
            .await;
            assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let refused: serde_json::Value = refused.json().await.unwrap();
            assert!(
                refused["refused"].as_str().unwrap().contains(reason),
                "{refused}"
            );
        }
        assert_eq!(source.revision("").unwrap(), revision, "nothing changed");
        assert_eq!(audits(&served, "roster.refused").len(), 2);
        assert_eq!(
            audits(&served, "roster.update")[0].target,
            "assistant settings"
        );

        let removed = call(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/assistant/settings",
            None,
        )
        .await;
        assert_eq!(removed.status(), StatusCode::OK);
        assert_eq!(
            source.load("").unwrap().roster.assistant(),
            &ai6::AssistantSettings::default()
        );
    }

    const CONFIG_ROLES: &str = r#"
        [role.config-readers]
        actions = ["config.read"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "config:watch"
        role = "config-readers"

        [role.config-managers]
        actions = ["config.read", "config.manage"]

        [[rule]]
        subject = "person:operator-007"
        scope = "config:watch"
        role = "config-managers"
    "#;

    async fn with_header(
        served: &Served,
        token: &str,
        method: reqwest::Method,
        path: &str,
        revision: &str,
    ) -> reqwest::Response {
        reqwest::Client::new()
            .request(method, format!("{}/v1/user/{path}", served.base))
            .bearer_auth(token)
            .header("if-match", format!("\"{revision}\""))
            .send()
            .await
            .unwrap()
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn a_watch_is_listed_set_and_removed_under_its_own_action_and_audited() {
        use ai6::{ConfigProvider, ConfigRecord, SqlProvider};

        let directory = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", directory.path().join("roster.db").display());
        let source = Arc::new(SqlProvider::open(&location).unwrap());
        let global = ai6::ConfigScope::Global;
        source
            .put(
                ConfigRecord::Operation,
                &global,
                "G",
                &json!({"roster": ["007"]}),
            )
            .unwrap();
        source
            .put(
                ConfigRecord::Agent,
                &global,
                "007",
                &json!({"handler": "mock-agent", "clearance": "00"}),
            )
            .unwrap();
        // A Station reads the same database through a handle of its own.
        let station = SqlProvider::open(&location).unwrap();
        let served = served_with_roster(CONFIG_ROLES, Some(source.clone())).await;

        for (token, held) in [
            (&served.operator, json!(["config.read", "config.manage"])),
            (&served.viewer, json!(["config.read"])),
        ] {
            let profile: serde_json::Value =
                call(&served, token, reqwest::Method::GET, "profile", None)
                    .await
                    .json()
                    .await
                    .unwrap();
            assert_eq!(profile["config"]["watches"], held);
        }

        let form = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/watches/schema",
            None,
        )
        .await;
        assert_eq!(form.status(), StatusCode::OK);
        let form: serde_json::Value = form.json().await.unwrap();
        assert!(form["schema"]["properties"]["agent"].is_object(), "{form}");
        assert_eq!(form["key"]["title"], "Scope");

        let before = station.revision("").unwrap();
        let denied = call(
            &served,
            &served.viewer,
            reqwest::Method::PUT,
            "roster/watches/operation:G",
            Some(json!({"agent": "007"})),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let other_family = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "roster/agents",
            None,
        )
        .await;
        assert_eq!(
            other_family.status(),
            StatusCode::FORBIDDEN,
            "config.manage on config:watch opens no other family"
        );
        assert_eq!(station.revision("").unwrap(), before);

        let set = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/watches/operation:G",
            Some(json!({"agent": "007", "max_turns_per_hour": 4})),
        )
        .await;
        assert_eq!(set.status(), StatusCode::OK);
        let set: serde_json::Value = set.json().await.unwrap();
        assert_ne!(station.revision("").unwrap(), before);
        assert_eq!(
            set["revision"],
            station.revision("").unwrap().get().to_string()
        );
        let held = station.load("").unwrap();
        let watch = &held.roster.watches()["operation:G"];
        assert_eq!(watch.settings.agent, "007");
        assert_eq!(watch.settings.max_turns_per_hour, ai6::Limit::at(4));

        let listed: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/watches",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listed["kind"], "watches");
        assert_eq!(listed["writable"], true);
        assert_eq!(listed["records"][0]["key"], "operation:G");
        let revision = listed["records"][0]["revision"]
            .as_str()
            .unwrap()
            .to_owned();
        let updates = audits(&served, "roster.update");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].resource.as_deref(), Some("config:watch"));
        assert_eq!(updates[0].target, "watch operation:G");

        let refused = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/watches/operation:G",
            Some(json!({"agent": "no-such-agent"})),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(audits(&served, "roster.refused").len(), 1);
        assert_eq!(
            station.load("").unwrap().roster.watches()["operation:G"]
                .settings
                .agent,
            "007"
        );

        let stale = with_header(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/watches/operation:G",
            "0",
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let removed = with_header(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/watches/operation:G",
            &revision,
        )
        .await;
        assert_eq!(removed.status(), StatusCode::OK);
        assert!(station.load("").unwrap().roster.watches().is_empty());
        let removals = audits(&served, "roster.remove");
        assert_eq!(removals[0].resource.as_deref(), Some("config:watch"));
        assert_eq!(removals[0].target, "watch operation:G");
    }

    #[cfg(feature = "cli")]
    fn sql_source(directory: &tempfile::TempDir) -> (Arc<ai6::SqlProvider>, String) {
        let location = format!("sqlite:{}", directory.path().join("roster.db").display());
        (
            Arc::new(ai6::SqlProvider::open(&location).unwrap()),
            location,
        )
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn an_apply_stores_records_of_several_families_together_or_not_at_all() {
        use ai6::{ConfigProvider, SqlProvider};

        let directory = tempfile::tempdir().unwrap();
        let (source, location) = sql_source(&directory);
        let station = SqlProvider::open(&location).unwrap();
        let roles = format!("{ROLES}{CONFIG_ROLES}");
        let served = served_with_roster(&roles, Some(source.clone())).await;
        let apply = |token: String, body: serde_json::Value| {
            let served = &served;
            async move {
                call(
                    served,
                    &token,
                    reqwest::Method::POST,
                    "resources/apply",
                    Some(body),
                )
                .await
            }
        };
        let documents = json!({"records": [
            {"kind": "watches", "key": "operation:G", "definition": {"agent": "007"}},
            {"kind": "agent", "key": "007", "definition": {"handler": "mock-agent", "clearance": "00"}},
            {"kind": "operations", "key": "G", "definition": {"roster": ["007"]}},
        ]});

        let before = station.revision("").unwrap();
        let denied = apply(served.viewer.clone(), documents.clone()).await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let reason = denied.text().await.unwrap();
        assert!(
            reason.contains("family needs") && reason.contains("on config:"),
            "the family is named: {reason}"
        );
        assert_eq!(station.revision("").unwrap(), before);

        let dry = apply(
            served.operator.clone(),
            json!({"dry_run": true, "records": documents["records"]}),
        )
        .await;
        assert_eq!(dry.status(), StatusCode::OK);
        let dry: serde_json::Value = dry.json().await.unwrap();
        assert_eq!(dry["applied"], false);
        assert_eq!(dry["records"][0]["change"], "created");
        assert_eq!(station.revision("").unwrap(), before);
        assert!(audits(&served, "roster.update").is_empty());

        // One record the roster cannot hold keeps the others out, and each is audited refused.
        let bad = json!({"records": [
            {"kind": "operations", "key": "G", "definition": {"roster": ["007"]}},
            {"kind": "agents", "key": "007", "definition": {"handler": "mock-agent", "extends": "absent"}},
        ]});
        let refused = apply(served.operator.clone(), bad).await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(refused.json::<serde_json::Value>().await.unwrap()["refused"].is_string());
        assert_eq!(station.revision("").unwrap(), before);
        assert_eq!(audits(&served, "roster.refused").len(), 2);

        let applied = apply(served.operator.clone(), documents.clone()).await;
        assert_eq!(applied.status(), StatusCode::OK);
        let applied: serde_json::Value = applied.json().await.unwrap();
        assert_eq!(applied["applied"], true);
        assert_eq!(
            applied["revision"],
            station.revision("").unwrap().get().to_string()
        );
        assert_eq!(
            station.load("").unwrap().roster.watches()["operation:G"]
                .settings
                .agent,
            "007"
        );
        let updates = audits(&served, "roster.update");
        assert_eq!(updates.len(), 3);
        let resources: Vec<_> = updates
            .iter()
            .map(|record| record.resource.clone().unwrap())
            .collect();
        assert!(resources.contains(&"config:watch".to_owned()));
        assert!(resources.contains(&"config:roster".to_owned()));

        let same = apply(served.operator.clone(), documents).await;
        let same: serde_json::Value = same.json().await.unwrap();
        assert_eq!(same["records"][0]["change"], "unchanged");
        assert!(same.get("revision").is_none(), "nothing stored: {same}");
        assert_eq!(audits(&served, "roster.update").len(), 3);

        for body in [
            json!({"records": []}),
            json!({"records": [{"kind": "access", "key": "settings", "definition": {}}]}),
            json!({"station": "x", "records": [{"kind": "agent", "key": "007", "definition": {}}]}),
            json!({"records": [
                {"kind": "agent", "key": "007", "definition": {}},
                {"kind": "agents", "key": "007", "definition": {}},
            ]}),
        ] {
            let bad = apply(served.operator.clone(), body.clone()).await;
            assert_eq!(bad.status(), StatusCode::BAD_REQUEST, "{body}");
        }
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn an_apply_of_a_looser_assistant_waits_for_confirmation() {
        use ai6::{ConfigProvider, SqlProvider};

        let directory = tempfile::tempdir().unwrap();
        let (source, location) = sql_source(&directory);
        let station = SqlProvider::open(&location).unwrap();
        let served = served_with_roster(ROLES, Some(source)).await;
        let body = |confirm: bool| {
            json!({"confirm": confirm, "records": [
                {"kind": "assistant", "key": "settings", "definition": {"allow_elevated": true}},
            ]})
        };
        let before = station.revision("").unwrap();
        let held: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            "resources/apply",
            Some(body(false)),
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(held["applied"], false);
        assert!(
            held["weakened"][0]
                .as_str()
                .unwrap()
                .contains("assistant.allow_elevated")
        );
        assert_eq!(station.revision("").unwrap(), before);
        assert!(audits(&served, "roster.update").is_empty());

        let done: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            "resources/apply",
            Some(body(true)),
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(done["applied"], true);
        assert!(
            done["weakened"]
                .as_array()
                .is_some_and(|weakened| !weakened.is_empty())
        );
        assert!(
            station
                .load("")
                .unwrap()
                .roster
                .assistant()
                .allow_elevated()
        );
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn a_record_another_names_is_not_removed_and_the_kinds_are_listed_with_what_is_held() {
        use ai6::{ConfigProvider, ConfigRecord, SqlProvider};

        let directory = tempfile::tempdir().unwrap();
        let (source, location) = sql_source(&directory);
        let station = SqlProvider::open(&location).unwrap();
        let global = ai6::ConfigScope::Global;
        source
            .put(
                ConfigRecord::Operation,
                &global,
                "G",
                &json!({"roster": ["007"]}),
            )
            .unwrap();
        source
            .put(
                ConfigRecord::Agent,
                &global,
                "007",
                &json!({"handler": "mock-agent", "clearance": "00"}),
            )
            .unwrap();
        source
            .put(
                ConfigRecord::Cover,
                &global,
                "house",
                &json!({"description": "House style."}),
            )
            .unwrap();
        let roles = format!("{ROLES}{CONFIG_ROLES}");
        let served = served_with_roster(&roles, Some(source)).await;

        let listed: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "resources",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        let kind = |name: &str| {
            listed
                .as_array()
                .unwrap()
                .iter()
                .find(|kind| kind["kind"] == name)
                .unwrap()
                .clone()
        };
        assert_eq!(kind("watch")["route"], "watches");
        assert_eq!(kind("watch")["allowed"], json!(["config.read"]));
        assert_eq!(kind("watch")["resource"], "config:watch");
        assert_eq!(kind("agent")["allowed"], json!(["roster.read"]));

        // The singular name routes like the plural.
        let by_name = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/agent",
            None,
        )
        .await;
        assert_eq!(by_name.status(), StatusCode::OK);

        let refused = call(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/agents/007",
            None,
        )
        .await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let reason = refused.json::<serde_json::Value>().await.unwrap()["refused"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(reason.contains("operation G"), "{reason}");
        assert_eq!(audits(&served, "roster.refused").len(), 1);
        assert!(
            station
                .load("")
                .unwrap()
                .roster
                .agent_ids()
                .any(|id| id == "007")
        );

        let forced = call(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/covers/house",
            None,
        )
        .await;
        assert_eq!(forced.status(), StatusCode::OK);
        let gone = call(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/covers/house",
            None,
        )
        .await;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }
}
