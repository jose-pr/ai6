//! The access policy the gateway enforces, and the routes that read and change it.
//!
//! [`LiveAccess`] holds the compiled policy and swaps it when the configuration provider's
//! `access` and `operation_access` records change; a record that does not compile is kept out
//! and the last good one stays in force. The routes read the stored records fresh and write
//! them with a compare-and-swap against the revision the caller read, so two editors never
//! overwrite each other.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::{Arc, PoisonError, RwLock};

use ai6::directorates::registry::backend::RegistryBackend;
use ai6::{
    ACTIONS, AccessPolicy, AccessSet, AuditRecord, AuthorizationDecision, AuthorizationRequest,
    Authorizer, CONFIG_ACCESS, ConfigProvider, ConfigRecord, ConfigScope, ConfigSnapshot, Editor,
    EffectiveAccess, Lineage, Lookup, NOT_DELEGABLE, OperationAccess, Orphan, Person, RoleDef,
    RosterErrorKind, RuleDef, TierKind, check_change, check_operation_change,
    compile_with_operations, seeded_policy,
};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::tree::OperationTree;
use crate::{
    ApiError, Gateway, authorize_resource_action, bearer, operation_tree, resolve_user,
    unix_timestamp,
};

/// The key of the global policy among the `access` family's records.
const KEY: &str = ConfigRecord::SINGLETON_KEY;
/// The longest role name or Operation ID the routes take.
const MAX_NAME_BYTES: usize = 128;

/// An answer that is not a plain [`ApiError`]: the reason is the caller's to read.
pub(crate) enum AdminError {
    Api(ApiError),
    /// 400: the change is refused, with the reason.
    Refused(String),
    /// 409: the state is in the way, with the reason.
    Conflict(String),
    /// 428: the write named no revision.
    Required,
}

impl From<ApiError> for AdminError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        let (status, reason) = match self {
            Self::Api(error) => return error.into_response(),
            Self::Refused(reason) => (StatusCode::BAD_REQUEST, reason),
            Self::Conflict(reason) => (StatusCode::CONFLICT, reason),
            Self::Required => (
                StatusCode::PRECONDITION_REQUIRED,
                "send the revision you read in If-Match".to_owned(),
            ),
        };
        (status, Json(json!({ "error": reason }))).into_response()
    }
}

fn internal() -> AdminError {
    AdminError::Api(ApiError::internal())
}

/// What the gateway holds of the policy.
struct Held {
    lineage: Lineage,
    policy: AccessPolicy,
    operations: BTreeMap<String, OperationAccess>,
    effective: Arc<EffectiveAccess>,
    invalid: Option<String>,
    /// The revision whose refusal was last logged.
    logged: Option<ai6::ConfigRevision>,
}

/// The policy in force, and the way back to the provider that stores it.
pub struct LiveAccess {
    provider: Arc<dyn ConfigProvider>,
    host_chiefs: Vec<String>,
    managed_by: Option<String>,
    state: RwLock<Held>,
    write: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for LiveAccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LiveAccess").finish_non_exhaustive()
    }
}

impl LiveAccess {
    /// Policy over `provider`'s records, with `host_chiefs` (`unex.cfg [access] chief`) always
    /// chief. With `managed_by` an authorizer plugin decides and only the chief tier is the
    /// gateway's own: the routes that change the policy answer 409.
    pub fn new(
        provider: Arc<dyn ConfigProvider>,
        host_chiefs: Vec<String>,
        managed_by: Option<String>,
    ) -> Result<Self, String> {
        let policy = AccessPolicy::default();
        let effective = ai6::compile(&policy, &host_chiefs).map_err(|error| error.to_string())?;
        Ok(Self {
            provider,
            host_chiefs,
            managed_by,
            state: RwLock::new(Held {
                lineage: Lineage::default(),
                policy,
                operations: BTreeMap::new(),
                effective: Arc::new(effective),
                invalid: None,
                logged: None,
            }),
            write: tokio::sync::Mutex::new(()),
        })
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Held> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The policy now in force.
    pub fn effective(&self) -> Arc<EffectiveAccess> {
        self.read().effective.clone()
    }

    /// Whether the policy has no chief anywhere: nobody could change access.
    pub fn has_no_chief(&self) -> bool {
        self.managed_by.is_none()
            && self.host_chiefs.is_empty()
            && self.read().policy.chief.is_empty()
    }

    /// Takes the records of `snapshot`. A policy that does not compile is kept out, once per
    /// revision logged, and shown by [`LiveAccess::invalid`]; with none stored the built-in
    /// roles stand in memory.
    pub fn apply(&self, snapshot: &ConfigSnapshot) {
        let stored = if self.managed_by.is_some() {
            Ok(None)
        } else {
            snapshot.settings.access_policy()
        };
        let operations = if self.managed_by.is_some() {
            BTreeMap::new()
        } else {
            snapshot.settings.operation_access.clone()
        };
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let compiled = stored.and_then(|policy| {
            let policy = policy.unwrap_or_else(|| {
                if self.managed_by.is_some() {
                    AccessPolicy::default()
                } else {
                    seeded_policy()
                }
            });
            compile_with_operations(&policy, &self.host_chiefs, &operations, &state.lineage)
                .map(|effective| (policy, effective))
                .map_err(|error| error.to_string())
        });
        match compiled {
            Ok((policy, effective)) => {
                for (operation, orphans) in effective.orphaned() {
                    for Orphan { item, reason } in orphans {
                        if state.logged != Some(snapshot.revision) {
                            eprintln!(
                                "universal-exports: access: {operation}: {item} is not applied: {reason}"
                            );
                        }
                    }
                }
                state.policy = policy;
                state.operations = operations;
                state.effective = Arc::new(effective);
                state.invalid = None;
            }
            Err(reason) => {
                if state.logged != Some(snapshot.revision) {
                    eprintln!(
                        "universal-exports: access: revision {} is invalid, keeping the policy in force: {reason}",
                        snapshot.revision.get()
                    );
                }
                state.invalid = Some(reason);
            }
        }
        state.logged = Some(snapshot.revision);
    }

    /// Reads the provider and takes what it holds.
    ///
    /// # Errors
    ///
    /// The provider's error, as text.
    pub fn reload(&self) -> Result<(), String> {
        let snapshot = self.provider.load("").map_err(|error| error.to_string())?;
        self.apply(&snapshot);
        Ok(())
    }

    /// Notes that the provider refused its configuration, so the policy in force is not the
    /// stored one.
    pub fn mark_invalid(&self, reason: &str) {
        self.state
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .invalid = Some(reason.to_owned());
    }

    /// Why the stored policy is not in force, when it is not.
    pub fn invalid(&self) -> Option<String> {
        self.read().invalid.clone()
    }

    /// Takes the tree of Operations the Stations last published; the Operation policies are
    /// recompiled against it when it changed.
    pub fn observe_lineage(&self, lineage: Lineage) {
        if self.read().lineage == lineage {
            return;
        }
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.lineage = lineage;
        if let Ok(effective) = compile_with_operations(
            &state.policy,
            &self.host_chiefs,
            &state.operations,
            &state.lineage,
        ) {
            state.effective = Arc::new(effective);
        }
    }

    /// Writes the built-in roles as the `access` record when there is none, and says whether
    /// it did. A record another gateway wrote meanwhile wins and is read.
    ///
    /// # Errors
    ///
    /// The provider's error, as text, other than "read-only" (the roles stay in memory) and a
    /// record that appeared meanwhile.
    pub fn seed(
        &self,
        snapshot: &ConfigSnapshot,
        registry: &Arc<dyn RegistryBackend>,
    ) -> Result<bool, String> {
        if self.managed_by.is_some() || snapshot.settings.access.is_some() {
            return Ok(false);
        }
        match self.provider.put_if(
            ConfigRecord::Access,
            &ConfigScope::Global,
            KEY,
            None,
            &seeded_policy().to_json(),
        ) {
            Ok(_) => {
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|error| error.to_string())?
                    .as_secs_f64();
                registry
                    .record_audit(&AuditRecord {
                        id: Uuid::new_v4().to_string(),
                        actor: "gateway".to_owned(),
                        action: "access.seeded".to_owned(),
                        operation: String::new(),
                        resource: Some(CONFIG_ACCESS.to_owned()),
                        target: "roles controller operator auditor".to_owned(),
                        at,
                    })
                    .map_err(|error| format!("auditing the seeded policy failed: {error}"))?;
                Ok(true)
            }
            Err(error) if error.kind() == RosterErrorKind::ReadOnly => {
                eprintln!(
                    "universal-exports: access: no stored policy; seeded roles kept in memory (provider is read-only)"
                );
                Ok(false)
            }
            Err(error) if error.kind() == RosterErrorKind::Conflict => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }
}

impl Authorizer for LiveAccess {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ai6::guatiao::library::ProviderError> {
        self.effective().authorize(request)
    }
}

/// Allows what either allows: the chief and Operation tiers of the gateway, or an authorizer
/// plugin.
pub struct EitherAuthorizer {
    /// The gateway's own tiers.
    pub live: Arc<LiveAccess>,
    /// The plugin.
    pub plugin: Arc<dyn Authorizer>,
}

impl std::fmt::Debug for EitherAuthorizer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EitherAuthorizer")
            .finish_non_exhaustive()
    }
}

impl Authorizer for EitherAuthorizer {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ai6::guatiao::library::ProviderError> {
        if self.live.authorize(request.clone())?.allowed {
            return Ok(AuthorizationDecision {
                allowed: true,
                ..AuthorizationDecision::default()
            });
        }
        self.plugin.authorize(request)
    }
}

/// The revision of one stored record: `0` when there is none, else a digest of it.
pub(crate) fn token(record: Option<&Value>) -> String {
    let Some(record) = record else {
        return "0".to_owned();
    };
    let mut hex = String::new();
    for byte in &Sha256::digest(record.to_string().as_bytes())[..12] {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The records as the provider holds them now.
struct Current {
    policy: Option<AccessPolicy>,
    stored: Option<Value>,
    operations: BTreeMap<String, OperationAccess>,
    operation_stored: BTreeMap<String, Value>,
    writable: bool,
}

impl Current {
    /// The policy a write starts from: what is stored, else the built-in roles that stand.
    fn policy(&self) -> AccessPolicy {
        self.policy.clone().unwrap_or_else(seeded_policy)
    }
}

async fn read_current(live: &Arc<LiveAccess>) -> Result<Current, AdminError> {
    let live = live.clone();
    tokio::task::spawn_blocking(move || {
        let global = live
            .provider
            .definitions(ConfigRecord::Access, &ConfigScope::Global);
        let operations = live
            .provider
            .definitions(ConfigRecord::OperationAccess, &ConfigScope::Global);
        match (global, operations) {
            (Ok(global), Ok(operations)) => {
                let stored = global
                    .records
                    .into_iter()
                    .find(|(key, _)| key == KEY)
                    .map(|(_, definition)| definition);
                let policy = stored
                    .as_ref()
                    .map(AccessPolicy::from_json)
                    .transpose()
                    .map_err(|error| AdminError::Conflict(error.to_string()))?;
                let mut records = BTreeMap::new();
                let mut raw = BTreeMap::new();
                for (key, definition) in operations.records {
                    let record = OperationAccess::from_json(&definition)
                        .map_err(|error| AdminError::Conflict(error.to_string()))?;
                    records.insert(key.clone(), record);
                    raw.insert(key, definition);
                }
                Ok(Current {
                    policy,
                    stored,
                    operations: records,
                    operation_stored: raw,
                    writable: global.writable && operations.writable,
                })
            }
            _ => {
                let state = live.read();
                Ok(Current {
                    policy: Some(state.policy.clone()),
                    stored: None,
                    operations: state.operations.clone(),
                    operation_stored: BTreeMap::new(),
                    writable: false,
                })
            }
        }
    })
    .await
    .map_err(|_| internal())?
}

/// The revision a write names: `If-Match`, else `?revision=` (what a proposal carries).
fn revision_of(headers: &HeaderMap, query: &WriteQuery) -> Result<String, AdminError> {
    headers
        .get("if-match")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().trim_matches('"').to_owned())
        .or_else(|| query.revision.clone())
        .ok_or(AdminError::Required)
}

/// `?revision=`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct WriteQuery {
    revision: Option<String>,
}

fn checked_name(name: &str) -> Result<(), AdminError> {
    if name.trim().is_empty()
        || name.len() > MAX_NAME_BYTES
        || name.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        Err(AdminError::Api(ApiError::bad_request()))
    } else {
        Ok(())
    }
}

async fn audit(
    gateway: &Gateway,
    person: &Person,
    action: &str,
    operation: Option<&str>,
    resource: &str,
    target: &str,
) -> Result<(), AdminError> {
    let registry = gateway
        .registry
        .clone()
        .ok_or_else(|| AdminError::Api(ApiError::unavailable()))?;
    let record = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: action.to_owned(),
        operation: operation.unwrap_or_default().to_owned(),
        resource: Some(resource.to_owned()),
        target: crate::roster::bounded(target),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| internal())?
        .map_err(|_| internal())
}

pub(crate) fn live(gateway: &Gateway) -> Result<Arc<LiveAccess>, AdminError> {
    gateway
        .access_admin
        .clone()
        .ok_or(AdminError::Api(ApiError::unavailable()))
}

fn writable(live: &LiveAccess) -> Result<(), AdminError> {
    match &live.managed_by {
        Some(id) => Err(AdminError::Conflict(format!(
            "access is managed by authorizer {id}"
        ))),
        None => Ok(()),
    }
}

/// What an accepted change came to.
#[derive(Serialize)]
pub(crate) struct Changed {
    revision: String,
}

#[derive(Serialize)]
struct RoleView {
    name: String,
    /// `chief`, `controller`, `operator` and `auditor`: the names an Operation may not reuse.
    built_in: bool,
    /// Whether the role can be edited or deleted (`chief` cannot).
    editable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    actions: Vec<String>,
}

#[derive(Serialize)]
struct DelegableView {
    actions: Vec<String>,
    /// Whether this is the default cap rather than a stored one.
    default: bool,
    /// What the cap allows that the default withholds.
    weakenings: Vec<String>,
}

#[derive(Serialize)]
struct ChiefView {
    stored: Vec<String>,
    /// From `unex.cfg`: always chief, never changed here.
    host: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct AccessView {
    revision: String,
    writable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    managed_by: Option<String>,
    roles: Vec<RoleView>,
    rules: Vec<RuleDef>,
    delegable: DelegableView,
    chief: ChiefView,
    #[serde(skip_serializing_if = "Option::is_none")]
    invalid: Option<String>,
}

fn role_views(roles: &BTreeMap<String, RoleDef>) -> Vec<RoleView> {
    let mut views = vec![RoleView {
        name: ai6::CHIEF_ROLE.to_owned(),
        built_in: true,
        editable: false,
        description: Some("Holds every action; named only in the chief list".to_owned()),
        actions: ACTIONS.iter().map(|action| (*action).to_owned()).collect(),
    }];
    views.extend(roles.iter().map(|(name, role)| RoleView {
        name: name.clone(),
        built_in: ai6::RESERVED_ROLES.contains(&name.as_str()),
        editable: true,
        description: role.description.clone(),
        actions: role.actions.clone(),
    }));
    views
}

fn delegable_view(policy: &AccessPolicy) -> DelegableView {
    let set = policy.delegable_set();
    DelegableView {
        weakenings: NOT_DELEGABLE
            .iter()
            .filter(|action| set.contains(**action))
            .map(|action| (*action).to_owned())
            .collect(),
        actions: set.into_iter().collect(),
        default: policy.delegable.is_none(),
    }
}

/// `GET /v1/user/access`: needs `access.read` at `config:access`.
pub(crate) async fn read_global_policy(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<AccessView>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(&gateway, person, CONFIG_ACCESS.to_owned(), "access.read").await?;
    let live = live(&gateway)?;
    let current = read_current(&live).await?;
    let policy = current.policy();
    let (host, stored) = (live.host_chiefs.clone(), policy.chief.clone());
    Ok(Json(AccessView {
        revision: token(current.stored.as_ref()),
        writable: current.writable && live.managed_by.is_none(),
        managed_by: live.managed_by.clone(),
        roles: role_views(&policy.roles),
        delegable: delegable_view(&policy),
        rules: policy.rules,
        chief: ChiefView { stored, host },
        invalid: live.invalid(),
    }))
}

/// A role as a body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoleBody {
    actions: Vec<String>,
    #[serde(default)]
    description: Option<String>,
}

/// The whole ordered rule list.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RulesBody {
    rules: Vec<RuleBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuleBody {
    subject: String,
    scope: String,
    role: String,
    #[serde(default = "yes")]
    propagate: bool,
}

const fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DelegableBody {
    /// `null` puts the cap back to the default.
    actions: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChiefBody {
    chief: Vec<String>,
}

/// Marks the rules of `after` that are not in `before` as written now by `person`; one that
/// stays keeps its editor.
fn stamp(before: &[RuleDef], after: &mut [RuleDef], person: &Person, now: f64) {
    for rule in after {
        let kept = before.iter().find(|old| {
            old.subject == rule.subject
                && old.scope == rule.scope
                && old.role == rule.role
                && old.propagate == rule.propagate
        });
        match kept {
            Some(old) => {
                rule.editor.clone_from(&old.editor);
                rule.changed_unix = old.changed_unix;
            }
            None => {
                rule.editor = Some(person.name.clone());
                rule.changed_unix = Some(now as u64);
            }
        }
    }
}

type Mutation = Box<dyn FnOnce(&mut AccessPolicy) -> Result<String, AdminError> + Send>;

/// Changes the global policy: `access.manage` on it, the named revision, the guard, a
/// compare-and-swap write, the audit record, then the policy in force.
async fn write_global(
    gateway: Gateway,
    headers: HeaderMap,
    query: WriteQuery,
    action: &'static str,
    mutation: Mutation,
) -> Result<Json<Changed>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let live = live(&gateway)?;
    writable(&live)?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        CONFIG_ACCESS.to_owned(),
        "access.manage",
    )
    .await?;
    let expected = revision_of(&headers, &query)?;
    let _turn = live.write.lock().await;
    let current = read_current(&live).await?;
    if !current.writable {
        return Err(AdminError::Conflict(
            "the configuration provider is read-only".to_owned(),
        ));
    }
    if token(current.stored.as_ref()) != expected {
        return Err(AdminError::Conflict(
            "the access policy changed since you read it; reload it".to_owned(),
        ));
    }
    let before = current.policy();
    let mut after = before.clone();
    let target = mutation(&mut after)?;
    let now = unix_timestamp()?;
    stamp(&before.rules, &mut after.rules, &person, now);
    let effective = live.effective();
    let lineage = live.read().lineage.clone();
    let editor = Editor::new(person.clone(), &effective);
    if let Err(refusal) = check_change(&before, &after, &editor, &effective, &lineage) {
        audit(
            &gateway,
            &person,
            "access.refused",
            None,
            CONFIG_ACCESS,
            &format!("{target}: {}", refusal.reason),
        )
        .await?;
        return Err(AdminError::Refused(refusal.reason));
    }
    let definition = after.to_json();
    let swapped = {
        let (live, stored, definition) = (live.clone(), current.stored.clone(), definition.clone());
        tokio::task::spawn_blocking(move || {
            live.provider.put_if(
                ConfigRecord::Access,
                &ConfigScope::Global,
                KEY,
                stored.as_ref(),
                &definition,
            )
        })
        .await
        .map_err(|_| internal())?
    };
    match swapped {
        Ok(_) => {}
        Err(error) => {
            return Err(match error.kind() {
                RosterErrorKind::Conflict => AdminError::Conflict(
                    "the access policy changed since you read it; reload it".to_owned(),
                ),
                RosterErrorKind::ReadOnly => {
                    AdminError::Conflict("the configuration provider is read-only".to_owned())
                }
                RosterErrorKind::Invalid => {
                    audit(
                        &gateway,
                        &person,
                        "access.refused",
                        None,
                        CONFIG_ACCESS,
                        &format!("{target}: {error}"),
                    )
                    .await?;
                    AdminError::Refused(error.to_string())
                }
                _ => AdminError::Api(ApiError::unavailable()),
            });
        }
    }
    audit(&gateway, &person, action, None, CONFIG_ACCESS, &target).await?;
    let _ = live.reload();
    Ok(Json(Changed {
        revision: token(Some(&definition)),
    }))
}

/// `PUT /v1/user/access/roles/{role}`: sets one role's actions (and description).
pub(crate) async fn put_role(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
    Path(role): Path<String>,
    Json(body): Json<RoleBody>,
) -> Result<Json<Changed>, AdminError> {
    checked_name(&role)?;
    write_global(
        gateway,
        headers,
        query,
        "access.update",
        Box::new(move |policy| {
            policy.roles.insert(
                role.clone(),
                RoleDef {
                    actions: body.actions,
                    description: body.description,
                },
            );
            Ok(format!("role {role}"))
        }),
    )
    .await
}

/// `DELETE /v1/user/access/roles/{role}`: a role a rule names cannot be deleted (409).
pub(crate) async fn delete_role(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
    Path(role): Path<String>,
) -> Result<Json<Changed>, AdminError> {
    checked_name(&role)?;
    write_global(
        gateway,
        headers,
        query,
        "access.remove",
        Box::new(move |policy| {
            if policy.roles.remove(&role).is_none() {
                return Err(AdminError::Api(ApiError::not_found()));
            }
            let used: Vec<String> = policy
                .rules
                .iter()
                .enumerate()
                .filter(|(_, rule)| rule.role == role)
                .map(|(index, _)| format!("rule {index}"))
                .collect();
            if used.is_empty() {
                Ok(format!("role {role}"))
            } else {
                Err(AdminError::Conflict(format!(
                    "role {role} is named by {}",
                    used.join(", ")
                )))
            }
        }),
    )
    .await
}

/// `PUT /v1/user/access/rules`: replaces the whole ordered list.
pub(crate) async fn put_rules(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
    Json(body): Json<RulesBody>,
) -> Result<Json<Changed>, AdminError> {
    write_global(
        gateway,
        headers,
        query,
        "access.update",
        Box::new(move |policy| {
            policy.rules = body
                .rules
                .into_iter()
                .map(|rule| {
                    let mut def = RuleDef::new(rule.subject, rule.scope, rule.role);
                    def.propagate = rule.propagate;
                    def
                })
                .collect();
            Ok("rules".to_owned())
        }),
    )
    .await
}

/// `PUT /v1/user/access/delegable`: sets the cap (`null` for the default).
pub(crate) async fn put_delegable(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
    Json(body): Json<DelegableBody>,
) -> Result<Json<Changed>, AdminError> {
    write_global(
        gateway,
        headers,
        query,
        "access.update",
        Box::new(move |policy| {
            policy.delegable = body.actions;
            Ok("delegable".to_owned())
        }),
    )
    .await
}

/// `PUT /v1/user/access/chief`: sets the stored chief list.
pub(crate) async fn put_chief(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
    Json(body): Json<ChiefBody>,
) -> Result<Json<Changed>, AdminError> {
    write_global(
        gateway,
        headers,
        query,
        "access.update",
        Box::new(move |policy| {
            policy.chief = body.chief;
            Ok("chief".to_owned())
        }),
    )
    .await
}

// ---- Operation policies -------------------------------------------------------------------

#[derive(Serialize)]
struct Binding {
    subject: String,
    role: String,
    /// `global`, `operation` (this Operation's own record) or `ancestor`.
    tier: &'static str,
    /// The scope the rule is written at: `*`, or an Operation the person may read.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

#[derive(Serialize)]
struct Inherited {
    role: String,
    from: String,
    actions: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct OperationView {
    operation_id: String,
    revision: String,
    writable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    managed_by: Option<String>,
    roles: Vec<RoleView>,
    rules: Vec<RuleDef>,
    inherited: Vec<Inherited>,
    orphaned: Vec<OrphanView>,
    bindings: Vec<Binding>,
    /// What an Operation's roles may hold, from the global cap.
    delegable: Vec<String>,
}

#[derive(Serialize)]
struct OrphanView {
    item: String,
    reason: String,
}

async fn known_operation(gateway: &Gateway, operation: &str) -> Result<OperationTree, AdminError> {
    checked_name(operation)?;
    let tree = operation_tree(gateway).await?;
    if tree.operations().any(|known| known == operation) {
        Ok(tree)
    } else {
        Err(AdminError::Api(ApiError::not_found()))
    }
}

/// `GET /v1/user/operations/{op}/access`: needs `access.read` at the Operation.
pub(crate) async fn read_operation_policy(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation): Path<String>,
) -> Result<Json<OperationView>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let tree = known_operation(&gateway, &operation).await?;
    let resource = crate::tree::operation_resource(&operation);
    authorize_resource_action(&gateway, person.clone(), resource, "access.read").await?;
    let live = live(&gateway)?;
    let current = read_current(&live).await?;
    let policy = current.policy();
    let record = current
        .operations
        .get(&operation)
        .cloned()
        .unwrap_or_default();
    // An ancestor is named only to someone who may read its Missions.
    let mut visible = BTreeSet::new();
    for ancestor in tree.ancestors(&operation) {
        let allowed = authorize_resource_action(
            &gateway,
            person.clone(),
            crate::tree::operation_resource(ancestor),
            "mission.read",
        )
        .await
        .is_ok();
        if allowed {
            visible.insert(ancestor.clone());
        }
    }
    let mut inherited = Vec::new();
    let mut bindings = Vec::new();
    for ancestor in tree.ancestors(&operation) {
        if let Some(parent) = current.operations.get(ancestor) {
            let shown = visible.contains(ancestor);
            for (name, role) in &parent.roles {
                if shown {
                    inherited.push(Inherited {
                        role: name.clone(),
                        from: ancestor.clone(),
                        actions: role.actions.clone(),
                    });
                }
            }
            for rule in &parent.rules {
                if rule.scope == format!("operation:{operation}")
                    || (rule.propagate
                        && tree.in_subtree(&operation, rule.scope.trim_start_matches("operation:")))
                {
                    bindings.push(Binding {
                        subject: rule.subject.clone(),
                        role: rule.role.clone(),
                        tier: "ancestor",
                        scope: shown.then(|| rule.scope.clone()),
                    });
                }
            }
        }
    }
    for rule in &record.rules {
        if rule.scope == format!("operation:{operation}")
            || (rule.propagate
                && tree.in_subtree(&operation, rule.scope.trim_start_matches("operation:")))
        {
            bindings.push(Binding {
                subject: rule.subject.clone(),
                role: rule.role.clone(),
                tier: "operation",
                scope: Some(rule.scope.clone()),
            });
        }
    }
    for rule in &policy.rules {
        let at = rule.scope.strip_prefix("operation:");
        let reaches = rule.scope == "*"
            || at == Some(operation.as_str())
            || (rule.propagate
                && at.is_some_and(|id| tree.ancestors(&operation).iter().any(|a| a == id)));
        if reaches {
            let hidden_ancestor = at.is_some_and(|id| id != operation && !visible.contains(id));
            bindings.push(Binding {
                subject: rule.subject.clone(),
                role: rule.role.clone(),
                tier: "global",
                scope: (!hidden_ancestor).then(|| rule.scope.clone()),
            });
        }
    }
    let effective = live.effective();
    let orphaned = effective
        .orphaned()
        .get(&operation)
        .into_iter()
        .flatten()
        .map(|orphan| OrphanView {
            item: orphan.item.clone(),
            reason: orphan.reason.clone(),
        })
        .collect();
    Ok(Json(OperationView {
        operation_id: operation.clone(),
        revision: token(current.operation_stored.get(&operation)),
        writable: current.writable && live.managed_by.is_none(),
        managed_by: live.managed_by.clone(),
        roles: role_views_operation(&record.roles),
        rules: record.rules,
        inherited,
        orphaned,
        bindings,
        delegable: policy.delegable_set().into_iter().collect(),
    }))
}

fn role_views_operation(roles: &BTreeMap<String, RoleDef>) -> Vec<RoleView> {
    roles
        .iter()
        .map(|(name, role)| RoleView {
            name: name.clone(),
            built_in: false,
            editable: true,
            description: role.description.clone(),
            actions: role.actions.clone(),
        })
        .collect()
}

type OperationMutation = Box<dyn FnOnce(&mut OperationAccess) -> Result<String, AdminError> + Send>;

/// Changes one Operation's record: `access.manage` there, the named revision, the delegation
/// guard, a compare-and-swap write and the audit record.
async fn write_operation(
    gateway: Gateway,
    headers: HeaderMap,
    query: WriteQuery,
    operation: String,
    action: &'static str,
    mutation: OperationMutation,
) -> Result<Json<Changed>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let live = live(&gateway)?;
    writable(&live)?;
    let tree = known_operation(&gateway, &operation).await?;
    let resource = crate::tree::operation_resource(&operation);
    authorize_resource_action(&gateway, person.clone(), resource.clone(), "access.manage").await?;
    let expected = revision_of(&headers, &query)?;
    let _turn = live.write.lock().await;
    let current = read_current(&live).await?;
    if !current.writable {
        return Err(AdminError::Conflict(
            "the configuration provider is read-only".to_owned(),
        ));
    }
    let stored = current.operation_stored.get(&operation).cloned();
    if token(stored.as_ref()) != expected {
        return Err(AdminError::Conflict(
            "the Operation's access changed since you read it; reload it".to_owned(),
        ));
    }
    let before = current
        .operations
        .get(&operation)
        .cloned()
        .unwrap_or_default();
    let mut after = before.clone();
    let target = mutation(&mut after)?;
    let now = unix_timestamp()?;
    stamp(&before.rules, &mut after.rules, &person, now);
    let policy = current.policy();
    let lineage = tree_lineage(&tree);
    let effective = live.effective();
    let editor = Editor::new(person.clone(), &effective);
    let set = AccessSet {
        global: &policy,
        operations: &current.operations,
        lineage: &lineage,
    };
    if let Err(refusal) =
        check_operation_change(&operation, &before, &after, &editor, &effective, &set)
    {
        audit(
            &gateway,
            &person,
            "access.refused",
            Some(&operation),
            CONFIG_ACCESS,
            &format!("{target}: {}", refusal.reason),
        )
        .await?;
        return Err(AdminError::Refused(refusal.reason));
    }
    let definition = after.to_json();
    let swapped = {
        let (live, operation, stored, definition) = (
            live.clone(),
            operation.clone(),
            stored.clone(),
            definition.clone(),
        );
        tokio::task::spawn_blocking(move || {
            live.provider.put_if(
                ConfigRecord::OperationAccess,
                &ConfigScope::Global,
                &operation,
                stored.as_ref(),
                &definition,
            )
        })
        .await
        .map_err(|_| internal())?
    };
    if let Err(error) = swapped {
        return Err(match error.kind() {
            RosterErrorKind::Conflict => AdminError::Conflict(
                "the Operation's access changed since you read it; reload it".to_owned(),
            ),
            RosterErrorKind::ReadOnly => {
                AdminError::Conflict("the configuration provider is read-only".to_owned())
            }
            RosterErrorKind::Invalid => AdminError::Refused(error.to_string()),
            _ => AdminError::Api(ApiError::unavailable()),
        });
    }
    audit(
        &gateway,
        &person,
        action,
        Some(&operation),
        CONFIG_ACCESS,
        &target,
    )
    .await?;
    let _ = live.reload();
    Ok(Json(Changed {
        revision: token(Some(&definition)),
    }))
}

/// The tree as the policy sees it.
pub(crate) fn tree_lineage(tree: &OperationTree) -> Lineage {
    Lineage::new(
        tree.operations()
            .map(|operation| (operation.to_owned(), tree.ancestors(operation).to_vec())),
    )
}

/// `PUT /v1/user/operations/{op}/access/roles/{role}`.
pub(crate) async fn put_operation_role(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
    Path((operation, role)): Path<(String, String)>,
    Json(body): Json<RoleBody>,
) -> Result<Json<Changed>, AdminError> {
    checked_name(&role)?;
    write_operation(
        gateway,
        headers,
        query,
        operation,
        "access.update",
        Box::new(move |record| {
            record.roles.insert(
                role.clone(),
                RoleDef {
                    actions: body.actions,
                    description: body.description,
                },
            );
            Ok(format!("role {role}"))
        }),
    )
    .await
}

/// `DELETE /v1/user/operations/{op}/access/roles/{role}`.
pub(crate) async fn delete_operation_role(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
    Path((operation, role)): Path<(String, String)>,
) -> Result<Json<Changed>, AdminError> {
    checked_name(&role)?;
    write_operation(
        gateway,
        headers,
        query,
        operation,
        "access.remove",
        Box::new(move |record| {
            if record.roles.remove(&role).is_none() {
                return Err(AdminError::Api(ApiError::not_found()));
            }
            let used: Vec<String> = record
                .rules
                .iter()
                .enumerate()
                .filter(|(_, rule)| rule.role == role)
                .map(|(index, _)| format!("rule {index}"))
                .collect();
            if used.is_empty() {
                Ok(format!("role {role}"))
            } else {
                Err(AdminError::Conflict(format!(
                    "role {role} is named by {}",
                    used.join(", ")
                )))
            }
        }),
    )
    .await
}

/// `PUT /v1/user/operations/{op}/access/rules`.
pub(crate) async fn put_operation_rules(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
    Path(operation): Path<String>,
    Json(body): Json<RulesBody>,
) -> Result<Json<Changed>, AdminError> {
    write_operation(
        gateway,
        headers,
        query,
        operation,
        "access.update",
        Box::new(move |record| {
            record.rules = body
                .rules
                .into_iter()
                .map(|rule| {
                    let mut def = RuleDef::new(rule.subject, rule.scope, rule.role);
                    def.propagate = rule.propagate;
                    def
                })
                .collect();
            Ok("rules".to_owned())
        }),
    )
    .await
}

// ---- The check ----------------------------------------------------------------------------

/// The query of the check route.
#[derive(Deserialize)]
pub(crate) struct CheckQuery {
    subject: String,
    action: String,
    resource: String,
}

#[derive(Serialize)]
pub(crate) struct Checked {
    allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

/// `GET /v1/user/access/check?subject=&action=&resource=`: needs `access.read` at the
/// resource (`config:access` for one that is not an Operation). The subject is read through
/// the Directory (`account:` by name, `person:` by ID); a `group:` is a person in that group.
pub(crate) async fn check(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<CheckQuery>,
) -> Result<Json<Checked>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let scope = if query.resource.starts_with("operation:") {
        query.resource.clone()
    } else {
        CONFIG_ACCESS.to_owned()
    };
    authorize_resource_action(&gateway, person, scope, "access.read").await?;
    let live = live(&gateway)?;
    let subject: ai6::Subject = query
        .subject
        .parse()
        .map_err(|_| AdminError::Api(ApiError::bad_request()))?;
    let directory = gateway
        .access
        .as_ref()
        .ok_or_else(|| AdminError::Api(ApiError::unavailable()))?
        .directory
        .clone();
    let looked = {
        let (by, value) = match subject.kind() {
            "account" => ("name", subject.value().to_owned()),
            "person" => ("id", subject.value().to_owned()),
            _ => ("", String::new()),
        };
        if by.is_empty() {
            None
        } else {
            let by = by.to_owned();
            tokio::task::spawn_blocking(move || directory.lookup(Lookup { by, value }))
                .await
                .map_err(|_| internal())?
                .map_err(|_| internal())?
        }
    };
    let subject_person = looked.unwrap_or_else(|| match subject.kind() {
        "account" => Person {
            name: subject.value().to_owned(),
            ..Person::default()
        },
        "person" => Person {
            id: subject.value().to_owned(),
            ..Person::default()
        },
        _ => Person {
            groups: vec![subject.value().to_owned()],
            ..Person::default()
        },
    });
    let tree = operation_tree(&gateway).await?;
    let request =
        crate::tree::resource_request(subject_person, &query.action, query.resource, &tree);
    let explained = live.effective().explain(&request).map_err(|_| internal())?;
    Ok(Json(Checked {
        allowed: explained.allowed,
        tier: explained.tier.map(|tier| match tier {
            TierKind::Chief => "chief".to_owned(),
            TierKind::Global => "global".to_owned(),
            TierKind::Operation(id) => format!("operation:{id}"),
            _ => "unknown".to_owned(),
        }),
        rule_index: explained.rule_index,
        role: explained.role,
    }))
}

#[cfg(test)]
mod tests {
    use ai6::{MemoryProvider, Roster, Settings};

    use super::*;

    #[test]
    fn an_invalid_stored_policy_keeps_the_previous_one_in_force() {
        let provider = Arc::new(MemoryProvider::new(Roster::default()));
        let live = LiveAccess::new(provider.clone(), vec!["account:boss".into()], None).unwrap();
        let mut good = Settings::default();
        good.access = Some(json!({"role": {"v": {"actions": ["mission.read"]}},
            "rule": [{"subject": "account:ann", "scope": "operation:G", "role": "v"}]}));
        provider.replace_all(Roster::default(), good);
        live.reload().unwrap();
        let ann = Person {
            name: "ann".into(),
            ..Person::default()
        };
        let allowed = |live: &LiveAccess| {
            live.authorize(AuthorizationRequest {
                person: ann.clone(),
                action: "mission.read".into(),
                resource: "operation:G".into(),
                ancestors: Vec::new(),
            })
            .unwrap()
            .allowed
        };
        assert!(allowed(&live) && live.invalid().is_none());

        let mut bad = Settings::default();
        bad.access =
            Some(json!({"rule": [{"subject": "account:ann", "scope": "*", "role": "chief"}]}));
        provider.replace_all(Roster::default(), bad);
        live.reload().unwrap();
        assert!(allowed(&live), "the last good policy stays in force");
        assert!(live.invalid().unwrap().contains("chief"));
    }
}
