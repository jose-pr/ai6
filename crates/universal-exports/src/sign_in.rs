//! Which methods a sign-in may use, from where it is made, and what it still needs.
//!
//! The decision belongs to a `coqui_sign_in_policy` provider the `unex` record names
//! (`sign_in_policy`); with none, each configured method alone is enough, from anywhere.
//! [`LiveSignIn`] holds the policy in force and swaps it when the record changes. Bearer API
//! keys are outside it: they are scoped and revocable, and never open a page's sign-in.
//! The routes read the stored record fresh and write it with a compare-and-swap against the
//! revision the caller read, so two editors never overwrite each other.

use std::net::SocketAddr;
use std::sync::{Arc, PoisonError, RwLock};

use ai6::directorates::m::access::{
    Factor, Person, SignInContext, SignInDecision, SignInPolicy, clients, purposes,
};
use ai6::{
    CONFIG_ACCESS, ConfigProvider, ConfigRecord, ConfigScope, ProviderConfig, RosterErrorKind,
    SignInFactor, SignInPolicyProvider, UnexRecord,
};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::header::ORIGIN;
use axum::http::{HeaderMap, StatusCode};
use coqui_access_signin::SignInRules;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::access_admin::{AdminError, WriteQuery, audit, revision_of, token};
use crate::{
    AccessProviders, ApiError, Gateway, authorize_resource_action, bearer, resolve_user,
    unix_timestamp,
};

/// The key of the `unex` record, which has one.
const KEY: &str = ConfigRecord::SINGLETON_KEY;

/// The word for what is signed in to, in a [`SignInContext`]: the gateway's own routes.
pub(crate) const SERVICE: &str = "unex";

/// The word for what is signed in to when a page is asked for.
pub(crate) const PAGES: &str = "pages";

/// Builds a sign-in policy provider from the `unex` record's `sign_in_policy`: the provider's
/// configuration checked against the provider's own schema, so a policy that does not build is
/// refused before it is stored or put in force.
pub trait SignInLoader: Send + Sync {
    /// The policy provider `provider` configured with `config`, or why it cannot be built.
    ///
    /// # Errors
    ///
    /// The reason, as text, the provider is missing, its configuration is refused or it fails to
    /// build.
    fn load(
        &self,
        provider: &str,
        config: Option<&ProviderConfig>,
    ) -> Result<Arc<dyn SignInPolicy>, String>;
}

struct Held {
    record: Option<SignInPolicyProvider>,
    /// The stored record could not be read at all, so `record` says nothing of it.
    unparsed: bool,
    policy: Option<Arc<dyn SignInPolicy>>,
    invalid: Option<String>,
}

/// The sign-in policy in force, and the way back to the record that stores it.
pub struct LiveSignIn {
    known: Option<std::collections::BTreeSet<String>>,
    store: Option<Arc<dyn ConfigProvider>>,
    loader: Option<Arc<dyn SignInLoader>>,
    held: RwLock<Held>,
    write: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for LiveSignIn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LiveSignIn").finish_non_exhaustive()
    }
}

impl LiveSignIn {
    /// A policy kept in the `unex` record of `store` and built by `loader`; none is in force
    /// until [`apply`](Self::apply) takes the record.
    pub fn new(store: Arc<dyn ConfigProvider>, loader: Arc<dyn SignInLoader>) -> Self {
        Self {
            known: None,
            store: Some(store),
            loader: Some(loader),
            held: RwLock::new(Held {
                record: None,
                unparsed: false,
                policy: None,
                invalid: None,
            }),
            write: tokio::sync::Mutex::new(()),
        }
    }

    /// `policy`, which no route changes.
    pub fn fixed(policy: Arc<dyn SignInPolicy>) -> Self {
        Self {
            known: None,
            store: None,
            loader: None,
            held: RwLock::new(Held {
                record: None,
                unparsed: false,
                policy: Some(policy),
                invalid: None,
            }),
            write: tokio::sync::Mutex::new(()),
        }
    }

    /// The IDs of the sign-in providers a policy may list; a policy listing any other is refused.
    /// Without this nothing is checked.
    #[must_use]
    pub fn with_providers<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.known = Some(ids.into_iter().map(Into::into).collect());
        self
    }

    /// Takes the record stored now. It reads the store under the lock a route's change holds, so
    /// a poll that began before a change cannot put an older record over it.
    ///
    /// Call it where blocking is allowed.
    ///
    /// # Errors
    ///
    /// Why the stored record does not stand: the policy offers nothing meanwhile.
    pub fn refresh(&self) -> Result<bool, String> {
        let _turn = self.write.blocking_lock();
        let store = self
            .store
            .clone()
            .ok_or_else(|| "the sign-in policy is not kept in the configuration".to_owned())?;
        let stored = store
            .definitions(ConfigRecord::Unex, &ConfigScope::Global)
            .map_err(|error| error.to_string())?
            .records
            .into_iter()
            .find(|(key, _)| key == KEY)
            .and_then(|(_, definition)| definition.get("sign_in_policy").cloned());
        let record = match stored {
            None => None,
            Some(value) => match ai6::directorates::m::records::from_json(&value) {
                Ok(record) => Some(record),
                Err(reason) => {
                    let reason = format!("sign_in_policy: {reason}");
                    self.deny(None, reason.clone(), true);
                    return Err(reason);
                }
            },
        };
        self.apply(record.as_ref())
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Held> {
        self.held.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The policy in force; `None` when the record names none.
    pub fn policy(&self) -> Option<Arc<dyn SignInPolicy>> {
        self.read().policy.clone()
    }

    /// Why the stored policy is not in force, when it is not.
    pub fn invalid(&self) -> Option<String> {
        self.read().invalid.clone()
    }

    /// Puts the policy `record` names in force, or none when it names none, and says whether
    /// that changed what is in force. A policy that does not build, or names a provider that is
    /// not configured, fails closed: nothing is offered and [`invalid`](Self::invalid) says why,
    /// never the policy that was in force before.
    ///
    /// # Errors
    ///
    /// Why the policy does not stand.
    pub fn apply(&self, record: Option<&SignInPolicyProvider>) -> Result<bool, String> {
        {
            let held = self.read();
            if held.record.as_ref() == record && !held.unparsed {
                return match &held.invalid {
                    Some(reason) => Err(reason.clone()),
                    None => Ok(false),
                };
            }
        }
        match record {
            None => self.install(None, None),
            Some(record) => match self.build(record) {
                Ok(policy) => self.install(Some(record.clone()), Some(policy)),
                Err(reason) => {
                    self.deny(Some(record.clone()), reason.clone(), false);
                    return Err(reason);
                }
            },
        }
        Ok(true)
    }

    /// Offers nothing, and says why.
    fn deny(&self, record: Option<SignInPolicyProvider>, reason: String, unparsed: bool) {
        let mut held = self.held.write().unwrap_or_else(PoisonError::into_inner);
        held.record = record;
        held.policy = Some(Arc::new(SignInRules::default()));
        held.invalid = Some(reason);
        held.unparsed = unparsed;
    }

    fn build(&self, record: &SignInPolicyProvider) -> Result<Arc<dyn SignInPolicy>, String> {
        if let Some(known) = &self.known {
            for id in listed_providers(record) {
                if !known.contains(&id) {
                    return Err(format!(
                        "the policy lists the provider {id}, which no sign-in provider is \
                         configured under"
                    ));
                }
            }
        }
        self.loader
            .as_ref()
            .ok_or_else(|| "no sign-in policy provider can be loaded here".to_owned())?
            .load(&record.provider, record.config.as_ref())
    }

    fn install(&self, record: Option<SignInPolicyProvider>, policy: Option<Arc<dyn SignInPolicy>>) {
        let mut held = self.held.write().unwrap_or_else(PoisonError::into_inner);
        held.record = record;
        held.policy = policy;
        held.invalid = None;
        held.unparsed = false;
    }

    /// Whether the policy in force offers no sign-in to any client from an unknown address or
    /// from loopback. A policy that offers only from other networks is not counted: no list of
    /// addresses is complete.
    pub fn offers_nothing(&self) -> bool {
        let Some(policy) = self.policy() else {
            return false;
        };
        [None, Some("127.0.0.1"), Some("::1")]
            .into_iter()
            .flat_map(|address| {
                [clients::BROWSER, clients::APP, clients::API]
                    .into_iter()
                    .map(move |client| SignInContext {
                        purpose: purposes::SIGN_IN.to_owned(),
                        address: address.map(str::to_owned),
                        client: Some(client.to_owned()),
                        service: Some(SERVICE.to_owned()),
                        ..SignInContext::default()
                    })
            })
            .all(|context| {
                policy
                    .decide(context)
                    .map(|decision| decision.methods.is_empty() && !decision.complete)
                    .unwrap_or(true)
            })
    }
}

/// The provider IDs a rules policy lists in its conditions.
fn listed_providers(record: &SignInPolicyProvider) -> Vec<String> {
    if record.provider != SignInPolicyProvider::RULES {
        return Vec::new();
    }
    record
        .config
        .as_ref()
        .and_then(|config| config.0.get("rule"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|rule| rule.get("providers")?.as_array())
        .flatten()
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect()
}

/// The step a Station's token carries: no sign-in made it, and none decides it again.
pub(crate) const STATION: &str = "station";

/// Whether `factors` say a Station issued the token.
pub(crate) fn from_station(factors: &[SignInFactor]) -> bool {
    matches!(factors, [only] if only.method() == STATION)
}

impl AccessProviders {
    /// The IDs a sign-in policy may list: each named sign-in provider and the page
    /// authenticator. The built-in proxy identity has none, so a `providers` condition never
    /// matches its step.
    pub fn provider_ids(&self) -> Vec<String> {
        self.authenticators
            .iter()
            .filter_map(|signer| signer.id.clone())
            .chain(self.page_authenticator_id.clone())
            .collect()
    }

    /// Whether a key provider is configured, which keeps working whatever the policy offers.
    pub fn has_keys(&self) -> bool {
        self.keys.is_some()
    }

    /// The methods a page's viewer may be named by when no policy says otherwise: those a sign-in
    /// may have used, and the page authenticator's own, whichever it uses.
    pub(crate) fn page_methods(&self) -> Vec<&'static str> {
        let mut methods = self.configured_methods();
        if self.page_authenticator.is_some() {
            for method in ["certificate", "proxy"] {
                if !methods.contains(&method) {
                    methods.push(method);
                }
            }
        }
        methods
    }

    /// The methods each configured sign-in provider and proxy identity offer, as a policy words
    /// them: what stands when no policy is configured.
    pub(crate) fn configured_methods(&self) -> Vec<&'static str> {
        let mut methods = Vec::new();
        for signer in &self.authenticators {
            // An authenticator that says nothing, or cannot, is still asked for a name and a
            // secret by `login`.
            let takes = signer
                .authenticator
                .describe()
                .ok()
                .and_then(|description| description.takes);
            if takes.as_deref() != Some(ai6::directorates::m::access::takes::REQUEST)
                && !methods.contains(&"password")
            {
                methods.push("password");
            }
        }
        if self.proxy_identity.is_some() {
            methods.push("proxy");
        }
        methods
    }
}

impl Gateway {
    /// The context of a request for `purpose`, with what the gateway knows of where it comes
    /// from. The client is a browser when the request names an origin, which no other client of
    /// the gateway does, and an API client otherwise: Outer Office's desktop apps arrive as
    /// the second kind, so a rule for `app` never matches here.
    pub(crate) fn sign_in_context(
        &self,
        purpose: &str,
        headers: &HeaderMap,
        connection: Option<SocketAddr>,
        service: &str,
    ) -> SignInContext {
        // Only a hint about what to offer: the caller says all of this. A browser sends no origin
        // on a same-origin GET, but it does say how it is fetching.
        let browser = service == PAGES
            || headers.contains_key(ORIGIN)
            || headers.contains_key("sec-fetch-mode")
            || headers.contains_key("sec-fetch-site");
        let client = if browser {
            clients::BROWSER
        } else {
            clients::API
        };
        SignInContext {
            purpose: purpose.to_owned(),
            address: self
                .login_guard
                .address(headers, connection)
                .map(|address| address.to_string()),
            client: Some(client.to_owned()),
            service: Some(service.to_owned()),
            time_unix: unix_timestamp().ok().map(|now| now as i64),
            ..SignInContext::default()
        }
    }

    /// What the sign-in policy decides for `context`; `None` when it could not find out, which
    /// offers nothing and refuses.
    pub(crate) async fn decide_sign_in(
        &self,
        providers: &AccessProviders,
        context: SignInContext,
    ) -> Option<SignInDecision> {
        let providers = providers.clone();
        let decided = tokio::task::spawn_blocking(move || {
            let policy: Arc<dyn SignInPolicy> =
                match providers.sign_in.as_ref().and_then(|live| live.policy()) {
                    Some(policy) => policy,
                    None => {
                        let pages = context.service.as_deref() == Some(PAGES);
                        let methods = if pages {
                            providers.page_methods()
                        } else {
                            providers.configured_methods()
                        };
                        // Nothing configured to sign in with offers nothing.
                        let rules = if methods.is_empty() {
                            SignInRules::default()
                        } else {
                            SignInRules::single_factor(methods)
                                .map_err(|error| error.to_string())?
                        };
                        Arc::new(rules)
                    }
                };
            policy.decide(context).map_err(|error| error.to_string())
        })
        .await
        .ok()?;
        match decided {
            Ok(decision) => Some(decision),
            Err(reason) => {
                eprintln!("universal-exports: the sign-in policy could not decide: {reason}");
                None
            }
        }
    }

    /// The steps `factors` record, as a policy reads them.
    pub(crate) fn passed(factors: &[SignInFactor]) -> Vec<Factor> {
        factors
            .iter()
            .map(|factor| Factor {
                method: factor.method().to_owned(),
                provider: factor.provider().map(str::to_owned),
                person: factor.person().map(str::to_owned),
            })
            .collect()
    }
}

/// A step that passed, as the sign-in keeps it.
pub(crate) fn step(
    method: &str,
    provider: Option<&str>,
    person: &Person,
) -> Result<SignInFactor, ApiError> {
    SignInFactor::try_new(method, provider.map(str::to_owned), Some(person.id.clone()))
        .map_err(|_| ApiError::internal())
}

/// The refusal of a method the policy does not offer from here: no authenticator is asked and
/// no try is counted.
pub(crate) fn not_accepted() -> ApiError {
    ApiError::refused(
        StatusCode::FORBIDDEN,
        "this sign-in method is not accepted from here".to_owned(),
    )
}

fn live(gateway: &Gateway) -> Result<Arc<LiveSignIn>, AdminError> {
    gateway
        .access
        .as_ref()
        .and_then(|access| access.sign_in.clone())
        .ok_or(AdminError::Api(ApiError::unavailable()))
}

fn store(live: &LiveSignIn) -> Result<Arc<dyn ConfigProvider>, AdminError> {
    live.store.clone().ok_or_else(|| {
        AdminError::Conflict("the sign-in policy is not kept in the configuration".to_owned())
    })
}

/// The `unex` record as stored, and the part of it that is the policy.
struct Stored {
    unex: Option<Value>,
    writable: bool,
}

impl Stored {
    fn policy(&self) -> Option<&Value> {
        self.unex.as_ref()?.get("sign_in_policy")
    }
}

async fn read_stored(store: Arc<dyn ConfigProvider>) -> Result<Stored, AdminError> {
    tokio::task::spawn_blocking(move || {
        store
            .definitions(ConfigRecord::Unex, &ConfigScope::Global)
            .map(|definitions| Stored {
                writable: definitions.writable,
                unex: definitions
                    .records
                    .into_iter()
                    .find(|(key, _)| key == KEY)
                    .map(|(_, definition)| definition),
            })
    })
    .await
    .map_err(|_| AdminError::Api(ApiError::internal()))?
    .map_err(|_| AdminError::Api(ApiError::unavailable()))
}

#[derive(Serialize)]
pub(crate) struct PolicyView {
    /// What a write names in `If-Match`.
    revision: String,
    writable: bool,
    /// The stored `sign_in_policy`; `null` when none is: each configured method alone is enough.
    policy: Option<Value>,
    /// Whether a stored policy is in force rather than that default.
    in_force: bool,
    /// Whether no sign-in is offered, so that only API keys work.
    offers_nothing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    invalid: Option<String>,
    /// Said when nothing is offered and no key provider is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'static str>,
}

/// `GET /v1/user/access/sign-in`: needs `access.read` at `config:access`.
pub(crate) async fn read_policy(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<PolicyView>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(&gateway, person, CONFIG_ACCESS.to_owned(), "access.read").await?;
    let live = live(&gateway)?;
    let stored = read_stored(store(&live)?).await?;
    let nothing = offers_nothing(&live).await;
    Ok(Json(PolicyView {
        revision: token(stored.policy()),
        writable: stored.writable,
        policy: stored.policy().cloned(),
        in_force: live.policy().is_some(),
        offers_nothing: nothing,
        invalid: live.invalid(),
        note: restore_note(&gateway, nothing),
    }))
}

/// The body of `PUT /v1/user/access/sign-in`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyBody {
    /// The policy provider; default `coqui_signin_rules`.
    #[serde(default)]
    provider: Option<String>,
    /// The provider's configuration.
    #[serde(default)]
    config: Option<Map<String, Value>>,
}

/// What a change came to.
#[derive(Serialize)]
pub(crate) struct PolicyChanged {
    revision: String,
    /// Whether the policy now in force offers no sign-in, so that only API keys work.
    offers_nothing: bool,
    /// Said when nothing is offered and no key provider is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'static str>,
}

enum Change {
    Set(Box<SignInPolicyProvider>, Value),
    Remove,
}

/// `PUT /v1/user/access/sign-in`: stores the policy and puts it in force; needs
/// `access.manage` at `config:access` and the revision read in `If-Match`. A policy that does
/// not build is refused (400). One that offers no sign-in is accepted: API keys still work.
pub(crate) async fn put_policy(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
    Json(body): Json<PolicyBody>,
) -> Result<Json<PolicyChanged>, AdminError> {
    let mut value = Map::new();
    if let Some(provider) = body.provider {
        value.insert("provider".to_owned(), Value::String(provider));
    }
    if let Some(config) = body.config {
        value.insert("config".to_owned(), Value::Object(config));
    }
    let value = Value::Object(value);
    let record: SignInPolicyProvider = ai6::directorates::m::records::from_json(&value)
        .map_err(|reason| AdminError::Refused(format!("sign_in_policy: {reason}")))?;
    write(
        gateway,
        headers,
        query,
        "access.sign_in.update",
        Change::Set(Box::new(record), value),
    )
    .await
}

/// `DELETE /v1/user/access/sign-in`: back to each configured method alone being enough.
pub(crate) async fn remove_policy(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<WriteQuery>,
) -> Result<Json<PolicyChanged>, AdminError> {
    write(
        gateway,
        headers,
        query,
        "access.sign_in.remove",
        Change::Remove,
    )
    .await
}

async fn write(
    gateway: Gateway,
    headers: HeaderMap,
    query: WriteQuery,
    action: &'static str,
    change: Change,
) -> Result<Json<PolicyChanged>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let live = live(&gateway)?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        CONFIG_ACCESS.to_owned(),
        "access.manage",
    )
    .await?;
    let expected = revision_of(&headers, &query)?;
    let _turn = live.write.lock().await;
    let provider = store(&live)?;
    let stored = read_stored(provider.clone()).await?;
    if !stored.writable {
        return Err(AdminError::Conflict(
            "the configuration provider is read-only".to_owned(),
        ));
    }
    if token(stored.policy()) != expected {
        return Err(AdminError::Conflict(
            "the sign-in policy changed since you read it; reload it".to_owned(),
        ));
    }
    let target = match &change {
        Change::Set(record, _) => format!("sign_in_policy {}", record.provider),
        Change::Remove => "sign_in_policy".to_owned(),
    };
    // Built before it is stored, so a policy that cannot stand is never written.
    let built = match &change {
        Change::Set(record, _) => {
            let (building, record) = (live.clone(), (**record).clone());
            let result = tokio::task::spawn_blocking(move || building.build(&record))
                .await
                .map_err(|_| AdminError::Api(ApiError::internal()))?;
            match result {
                Ok(policy) => Some(policy),
                Err(reason) => {
                    audit(
                        &gateway,
                        &person,
                        "access.sign_in.refused",
                        None,
                        CONFIG_ACCESS,
                        &format!("{target}: {reason}"),
                    )
                    .await?;
                    return Err(AdminError::Refused(reason));
                }
            }
        }
        Change::Remove => {
            if stored.policy().is_none() {
                return Err(AdminError::Api(ApiError::not_found()));
            }
            None
        }
    };
    let mut definition = match stored.unex.clone() {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    match &change {
        Change::Set(_, value) => {
            definition.insert("sign_in_policy".to_owned(), value.clone());
        }
        Change::Remove => {
            definition.remove("sign_in_policy");
        }
    }
    let definition = Value::Object(definition);
    ai6::directorates::m::records::from_json::<UnexRecord>(&definition)
        .map_err(|reason| AdminError::Refused(format!("unex: {reason}")))?;
    // The audit record comes first: a change that cannot be recorded is not stored.
    audit(&gateway, &person, action, None, CONFIG_ACCESS, &target).await?;
    let swapped = {
        let (provider, expected, definition) =
            (provider.clone(), stored.unex.clone(), definition.clone());
        tokio::task::spawn_blocking(move || {
            provider.put_if(
                ConfigRecord::Unex,
                &ConfigScope::Global,
                KEY,
                expected.as_ref(),
                &definition,
            )
        })
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
    };
    if let Err(error) = swapped {
        return Err(match error.kind() {
            RosterErrorKind::Conflict => AdminError::Conflict(
                "the sign-in policy changed since you read it; reload it".to_owned(),
            ),
            RosterErrorKind::ReadOnly => {
                AdminError::Conflict("the configuration provider is read-only".to_owned())
            }
            RosterErrorKind::Invalid => AdminError::Refused(error.to_string()),
            _ => AdminError::Api(ApiError::unavailable()),
        });
    }
    match change {
        Change::Set(record, _) => live.install(Some(*record), built),
        Change::Remove => live.install(None, None),
    }
    let nothing = offers_nothing(&live).await;
    let keys = gateway
        .access
        .as_ref()
        .is_some_and(AccessProviders::has_keys);
    if nothing {
        eprintln!("{}", nothing_offered(keys));
    }
    Ok(Json(PolicyChanged {
        revision: token(definition.get("sign_in_policy")),
        offers_nothing: nothing,
        note: restore_note(&gateway, nothing),
    }))
}

/// What the gateway says when the policy in force offers no sign-in; with no key provider
/// configured, there is nothing left that works but an edit to the database.
pub fn nothing_offered(keys: bool) -> &'static str {
    if keys {
        "universal-exports: sign-in policy: no sign-in is offered; API keys still work (ai6ctl \
         with a key holding access.manage changes the policy)"
    } else {
        "universal-exports: sign-in policy: no sign-in is offered and no key provider is \
         configured; only an edit to the database's unex record restores sign-in"
    }
}

fn restore_note(gateway: &Gateway, offers_nothing: bool) -> Option<&'static str> {
    let keys = gateway
        .access
        .as_ref()
        .is_some_and(AccessProviders::has_keys);
    (offers_nothing && !keys).then(|| nothing_offered(false))
}

/// [`LiveSignIn::offers_nothing`], where blocking is allowed: the policy may be a plugin.
async fn offers_nothing(live: &Arc<LiveSignIn>) -> bool {
    let live = live.clone();
    tokio::task::spawn_blocking(move || live.offers_nothing())
        .await
        .unwrap_or(true)
}
