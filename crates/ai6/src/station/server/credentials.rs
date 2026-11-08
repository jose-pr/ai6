//! Q's credentials for a Session: the Agent's loadout secrets, leased per Session from the
//! secret provider each names and handed to the harness in its environment only.
//!
//! Nothing is written anywhere: a value lives in the Station's memory and the harness's
//! environment. Leases are revoked when the Session's Mission ends or its Session stops on
//! this Station, and when their time is up. A `green_ink` secret is leased only while the
//! Mission holds a live green ink approval of a kind it names, once per approval, and its lease
//! is revoked as soon as that approval expires or is revoked.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::directorates::m::mission::Mission;
use crate::directorates::m::roster::Agent;
use crate::directorates::medical::exposure::{ExposureGate, Restriction};
use crate::directorates::q::secret_kind::{
    BUILT_IN, REGISTRY_PROVIDER, SecretFields, SecretKeeper,
};
use crate::directorates::q::{
    CREDENTIAL_VARIABLES, Loadout, LoadoutCatalog, SecretBroker, SecretDefinition, SecretLease,
    SecretLeaseRequest,
};
use crate::directorates::registry::approvals::{Approval, ApprovalStatus};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::station::plugins::{PluginBuildError, Plugins};

use super::DaemonState;
use super::accounts::Holder;
use super::routes::OnRoute;

/// The timeline kind of a credential event.
const TIMELINE_KIND: &str = "credential";

/// One built secret provider, with what it was built from.
struct Built {
    provider: String,
    settings: SecretFields,
    keeper: Arc<SecretKeeper>,
}

/// The Station's loadouts and the secret providers it built for them, and the leases it has
/// issued, by Mission.
#[derive(Clone, Default)]
pub(crate) struct CredentialDesk {
    catalog: Option<Arc<LoadoutCatalog>>,
    /// Where the `file` provider finds relative secret files: the Station's secrets directory.
    secrets_dir: Option<std::path::PathBuf>,
    /// One built provider per instance, or per provider used with its defaults.
    built: Arc<Mutex<BTreeMap<String, Built>>>,
    issued: Arc<Mutex<BTreeMap<String, Vec<Issued>>>>,
    /// Pool account logins and the accounts Sessions hold.
    pub(super) accounts: super::accounts::AccountDesk,
}

impl std::fmt::Debug for CredentialDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialDesk")
            .field("catalog", &self.catalog)
            .finish_non_exhaustive()
    }
}

impl PartialEq for CredentialDesk {
    fn eq(&self, other: &Self) -> bool {
        self.catalog == other.catalog
            && self.secrets_dir == other.secrets_dir
            && Arc::ptr_eq(&self.built, &other.built)
            && Arc::ptr_eq(&self.issued, &other.issued)
            && self.accounts == other.accounts
    }
}

impl Eq for CredentialDesk {}

impl CredentialDesk {
    pub(crate) fn with_catalog(mut self, catalog: LoadoutCatalog) -> Self {
        self.catalog = Some(Arc::new(catalog));
        self
    }

    pub(crate) fn has_catalog(&self) -> bool {
        self.catalog.is_some()
    }

    pub(super) fn catalog(&self) -> Option<&LoadoutCatalog> {
        self.catalog.as_deref()
    }

    pub(super) fn shared_catalog(&self) -> Option<Arc<LoadoutCatalog>> {
        self.catalog.clone()
    }

    /// Issues from `catalog` from now on; leases already issued keep their own terms.
    pub(super) fn set_catalog(&mut self, catalog: Option<Arc<LoadoutCatalog>>) {
        self.catalog = catalog;
    }

    pub(crate) fn with_account_login(
        mut self,
        login: Arc<dyn crate::directorates::q::AccountLogin>,
    ) -> Self {
        self.accounts = self.accounts.with_login(login);
        self
    }

    pub(crate) fn with_account_homes(mut self, dir: std::path::PathBuf) -> Self {
        self.accounts = self.accounts.with_homes(dir);
        self
    }

    pub(crate) fn with_default_account_homes(mut self, dir: std::path::PathBuf) -> Self {
        self.accounts = self.accounts.with_default_homes(dir);
        self
    }

    /// Resolves relative secret files against the Station's secrets directory, and
    /// logs each weakening of its secrets.
    pub(crate) fn in_secrets_dir(mut self, dir: &std::path::Path) -> Self {
        self.secrets_dir = Some(dir.to_path_buf());
        for weakening in self.weakenings() {
            eprintln!("ai6: {weakening}");
        }
        self
    }

    /// The ways these secrets are looser than they could be: a source every local harness
    /// inherits or a value written in the configuration, a file outside the Station's secrets
    /// directory, a green ink secret any target unlocks, and a green ink secret served by a
    /// built-in provider, whose value revoking a lease cannot take back.
    pub(crate) fn weakenings(&self) -> Vec<String> {
        let Some(catalog) = &self.catalog else {
            return Vec::new();
        };
        let mut weakened = Vec::new();
        for (name, instance) in catalog.secret_providers() {
            if instance.provider() == "file"
                && let Some(dir) = instance.settings().get("dir").and_then(|d| d.as_str())
                && self
                    .secrets_dir
                    .as_deref()
                    .is_none_or(|own| std::path::Path::new(dir) != own)
            {
                weakened.push(format!(
                    "secret provider {name:?} reads files from {dir}, outside the Station's \
                     secrets directory"
                ));
            }
        }
        for definition in catalog.secrets() {
            let name = definition.name();
            let source = definition.source();
            let provider = catalog.provider_of(&source.provider);
            let reference = crate::bootstrap::SecretRef::Provider(
                crate::directorates::q::SecretSource::new(provider, source.key.clone()),
            );
            if let Some(weakening) = reference.weakening() {
                weakened.push(format!("secret {name:?} is weakened: {weakening}"));
            }
            let key = std::path::Path::new(&source.key);
            if provider == "file"
                && (key.is_absolute()
                    || key
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir)))
            {
                weakened.push(format!(
                    "secret {name:?} is read from {}, outside the Station's secrets directory",
                    source.key
                ));
            }
            if let Some(weakening) = definition.green_ink_weakening() {
                weakened.push(weakening);
            }
            if definition.green_ink()
                && (BUILT_IN.contains(&provider) || provider == REGISTRY_PROVIDER)
            {
                weakened.push(format!(
                    "green ink secret {name:?} is served from the Station's own files, \
                     environment or configuration, so revoking its lease cannot take the value \
                     back; use a provider that issues credentials of its own"
                ));
            }
        }
        weakened
    }

    /// Builds each secret provider the catalog names, or keeps the one already built, and says
    /// why each it cannot build is refused. Nothing a provider said is repeated.
    pub(crate) fn check_providers(&self, plugins: &Arc<Plugins>) -> Vec<String> {
        let Some(catalog) = &self.catalog else {
            return Vec::new();
        };
        let names: BTreeSet<String> = catalog
            .secret_providers()
            .keys()
            .cloned()
            .chain(
                catalog
                    .secrets()
                    .into_iter()
                    .map(|definition| definition.source().provider.clone()),
            )
            .collect();
        let finder = self.finder(plugins.clone(), catalog.clone());
        let mut refused = Vec::new();
        for name in names {
            match finder.keeper(&name, &mut Vec::new()) {
                Ok(keeper) => {
                    for definition in catalog.secrets() {
                        if definition.source().provider == name
                            && let Err(reason) = keeper.check_fields(&definition.source().fields)
                        {
                            refused.push(format!(
                                "secret {:?} is refused: {reason}",
                                definition.name()
                            ));
                        }
                    }
                }
                Err(reason) => refused.push(format!("secret provider {name:?}: {reason}")),
            }
        }
        refused
    }

    /// The names of the secrets leased to `mission_id`'s Session now.
    #[cfg(test)]
    pub(crate) fn issued_for(&self, mission_id: &str) -> Vec<String> {
        self.issued
            .lock()
            .unwrap()
            .get(mission_id)
            .into_iter()
            .flatten()
            .flat_map(|issued| issued.lease.names())
            .collect()
    }

    fn finder(&self, plugins: Arc<Plugins>, catalog: Arc<LoadoutCatalog>) -> Finder {
        Finder {
            plugins,
            catalog,
            secrets_dir: self.secrets_dir.clone(),
            built: self.built.clone(),
            vault: self.accounts.logins.vault().cloned(),
        }
    }

    /// A broker over the catalog's secrets and the providers they name, leasing for at most
    /// `max_ttl`.
    fn broker(
        &self,
        plugins: &Arc<Plugins>,
        catalog: &Arc<LoadoutCatalog>,
        max_ttl: crate::bounds::TimeLimit,
    ) -> SecretBroker {
        let finder = self.finder(plugins.clone(), catalog.clone());
        SecretBroker::new(catalog.clone(), move |name| {
            finder.keeper(name, &mut Vec::new())
        })
        .with_max_ttl(max_ttl)
    }
}

/// Finds and builds the provider a secret names.
struct Finder {
    plugins: Arc<Plugins>,
    catalog: Arc<LoadoutCatalog>,
    secrets_dir: Option<std::path::PathBuf>,
    built: Arc<Mutex<BTreeMap<String, Built>>>,
    vault: Option<Arc<crate::directorates::q::sealing::Vault>>,
}

impl Finder {
    /// The provider `name` names: its instance, or the loaded provider of that ID with its
    /// default settings (the `file` provider's directory is the Station's secrets directory).
    /// `within` holds the instances whose settings are being read, so one whose credential
    /// leads back to itself is refused. Its error is the Station's own text, never a provider's.
    fn keeper(&self, name: &str, within: &mut Vec<String>) -> Result<Arc<SecretKeeper>, String> {
        if name == REGISTRY_PROVIDER {
            let vault = self
                .vault
                .clone()
                .ok_or("this Station has no sealing key")?;
            return Ok(Arc::new(SecretKeeper::new(Box::new(
                crate::directorates::q::sealing::RegistrySecrets::new(vault),
            ))?));
        }
        let instance = self.catalog.secret_providers().get(name);
        let (provider, settings) = match instance {
            Some(instance) => {
                if self.plugins.has_provider(name) {
                    return Err(format!(
                        "instance {name:?} takes the ID of a loaded provider; rename it"
                    ));
                }
                (instance.provider().to_owned(), instance.settings().clone())
            }
            None => {
                let mut settings = serde_json::Map::new();
                if name == "file"
                    && let Some(dir) = &self.secrets_dir
                {
                    settings.insert(
                        "dir".to_owned(),
                        serde_json::Value::String(dir.to_string_lossy().into_owned()),
                    );
                }
                (name.to_owned(), SecretFields::new(settings))
            }
        };
        if let Some(built) = self
            .built
            .lock()
            .map_err(|_| "the secret provider lock was poisoned".to_owned())?
            .get(name)
            .filter(|built| built.provider == provider && built.settings == settings)
        {
            return Ok(built.keeper.clone());
        }
        if within.iter().any(|seen| seen == name) {
            within.push(name.to_owned());
            return Err(format!(
                "its credential leads back to itself: {}",
                within.join(" -> ")
            ));
        }
        within.push(name.to_owned());
        let config = self.config(&provider, &settings, within)?;
        within.pop();
        let built = if instance.is_none() && name != "file" {
            self.plugins.secret_provider(&provider, None)
        } else {
            self.plugins.secret_provider(&provider, Some(&config))
        }
        .map_err(|error| built_error(&provider, &error))?;
        let keeper = Arc::new(SecretKeeper::new(built)?);
        self.built
            .lock()
            .map_err(|_| "the secret provider lock was poisoned".to_owned())?
            .insert(
                name.to_owned(),
                Built {
                    provider,
                    settings,
                    keeper: keeper.clone(),
                },
            );
        Ok(keeper)
    }

    /// `settings` as `provider`'s configuration, each field its schema marks sensitive read
    /// through the secret reference it holds.
    fn config(
        &self,
        provider: &str,
        settings: &SecretFields,
        within: &mut Vec<String>,
    ) -> Result<guatiao::Value, String> {
        let mut settings = settings.as_json().clone();
        if let Some(schema) = self.plugins.config_schema(provider) {
            self.resolve(&schema, &mut settings, within)?;
        }
        let value = SecretFields::new(settings)
            .to_map_in(guatiao::Alloc::wiping())
            .map_err(|_| format!("the settings of {provider:?} cannot be handed to it"))?;
        Ok(value.into())
    }

    /// Replaces each value in `settings` that `schema` marks `x-sensitive` with what its secret
    /// reference reads, at any depth of `properties`.
    fn resolve(
        &self,
        schema: &serde_json::Value,
        settings: &mut serde_json::Map<String, serde_json::Value>,
        within: &mut Vec<String>,
    ) -> Result<(), String> {
        let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
            return Ok(());
        };
        for (field, property) in properties {
            let Some(value) = settings.get_mut(field) else {
                continue;
            };
            if property
                .get("x-sensitive")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                let reference: crate::bootstrap::SecretRef =
                    crate::directorates::m::records::from_json(value).map_err(|_| {
                        format!("its setting `{field}` is a credential: a secret reference")
                    })?;
                let read = self.read(&reference, within)?;
                *value = serde_json::Value::String(read.as_str().to_owned());
            } else if let Some(inner) = value.as_object_mut() {
                self.resolve(property, inner, within)?;
            }
        }
        Ok(())
    }

    /// What `reference` reads, for an instance's own credential.
    fn read(
        &self,
        reference: &crate::bootstrap::SecretRef,
        within: &mut Vec<String>,
    ) -> Result<zeroize::Zeroizing<String>, String> {
        let source = match reference {
            crate::bootstrap::SecretRef::Provider(source) => source.clone(),
            crate::bootstrap::SecretRef::Named(name) => self.catalog.secret(name).source().clone(),
        };
        let keeper = self.keeper(&source.provider, within)?;
        keeper.check_fields(&source.fields)?;
        keeper
            .read(&source.key, &source.fields)
            .map(|read| zeroize::Zeroizing::new(read.value.expose().clone()))
            .map_err(|_| {
                format!(
                    "its credential could not be read from {:?}; the provider's reason is withheld",
                    source.provider
                )
            })
    }
}

/// Why a provider could not be built, without the provider's own words where they could quote
/// a credential it was given.
fn built_error(provider: &str, error: &PluginBuildError) -> String {
    match error {
        PluginBuildError::InvalidConfiguration { .. } => format!(
            "provider {provider:?} refused its settings; its reason is withheld, since it may \
             quote a credential"
        ),
        PluginBuildError::Refused { .. } => format!(
            "provider {provider:?} could not be built; its reason is withheld, since it may \
             quote a credential"
        ),
        other => other.to_string(),
    }
}

/// The value of the `[secret.<name>]` a private overlay source is fetched with, read for this
/// fetch alone: no Session sees it.
pub(super) fn read_for_overlay(
    state: &DaemonState,
    name: &str,
) -> Result<zeroize::Zeroizing<String>, String> {
    let desk = &state.station.credentials;
    let catalog = desk
        .catalog
        .as_ref()
        .ok_or_else(|| format!("secret {name:?} is defined by no loadout document"))?;
    desk.finder(state.plugins.clone(), catalog.clone())
        .read(
            &crate::bootstrap::SecretRef::Named(name.to_owned()),
            &mut Vec::new(),
        )
        .map_err(|why| format!("secret {name:?}: {why}"))
}

/// What a Session of `agent` starts from: its roster `env`, with its loadout's `env` over it.
/// The issue of secrets after this takes only secrets' and pool accounts' variables away, which
/// a loadout's `env` cannot name, so what is here reaches an exposed Session too.
pub(super) fn session_environment(
    state: &DaemonState,
    agent: &str,
    roster: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut environment = roster.clone();
    if let Some(loadout) = state
        .station
        .credentials
        .catalog
        .as_ref()
        .and_then(|catalog| catalog.for_agent(agent).ok())
    {
        environment.extend(
            loadout
                .env()
                .values()
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
    }
    environment
}

/// Claims a pool account for a Session of `mission` when its Agent draws from a pool, and
/// leases the Agent's loadout secrets; puts each value in `environment` under its variable,
/// replacing what the Mission held before. An Agent without a loadout or pool gets nothing;
/// an exposed Session whose Operation restricts `secrets` gets only what its harness reaches its
/// model with (a pool account, or a loadout secret in a provider credential variable): without
/// it the harness could not run, and a Session that read untrusted text is the common case.
///
/// Fails when the backend refuses an ordinary secret or no pool account can be had, so a
/// Session never starts without the credentials its loadout and pools promise. Blocks on the
/// backend and the Registry.
///
/// `agent` is as its Session works, on `on` when it has routes: the route's own secrets are what
/// its harness reaches its model with, so they are issued whatever the Session has read.
pub(super) fn issue(
    state: &DaemonState,
    mission: &Mission,
    agent: &Agent,
    on: Option<&OnRoute>,
    exposure: &ExposureGate,
    site: super::accounts::Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    issue_for(
        state,
        mission.into(),
        agent,
        on,
        Some((mission, exposure)),
        site,
        environment,
    )
}

/// What an assistant Session of `agent` reaches its model with, under the name `holder` gives it:
/// its route's account and secrets. She is given no loadout secret: nothing she reads is trusted,
/// and a tool's credential would reach whatever she reads. Blocks on the backend and the Registry.
#[cfg(feature = "publisher")]
pub(super) fn issue_for_assistant(
    state: &DaemonState,
    holder: Holder<'_>,
    agent: &Agent,
    on: Option<&OnRoute>,
    site: super::accounts::Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    issue_for(state, holder, agent, on, None, site, environment)
}

/// What a watch's Session of `agent` reaches its model with, under the name `holder` gives it: its
/// route's account and secrets, and no loadout secret, since a watch reads untrusted text. Blocks
/// on the backend and the Registry.
pub(super) fn issue_for_watch(
    state: &DaemonState,
    holder: Holder<'_>,
    agent: &Agent,
    on: Option<&OnRoute>,
    site: super::accounts::Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    issue_for(state, holder, agent, on, None, site, environment)
}

fn issue_for(
    state: &DaemonState,
    holder: Holder<'_>,
    agent: &Agent,
    on: Option<&OnRoute>,
    loadout_of: Option<(&Mission, &ExposureGate)>,
    site: super::accounts::Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let desk = &state.station.credentials;
    let mission_id = holder.id;
    release(state, mission_id);
    if let Some(pool) = on.and_then(|on| on.implicit.as_ref()) {
        super::accounts::claim_implicit(state, pool, holder)?;
    }
    let Some(catalog) = &desk.catalog else {
        return Ok(());
    };
    let broker = desk.broker(
        &state.plugins,
        catalog,
        state.station.limits().secrets.max_lease_ttl,
    );
    // A pooled Agent's loadout may not set a provider credential variable.
    let reserved = super::accounts::credential_variables(state, catalog, holder.operation, agent);
    let mut leases = Leases::default();
    if let Some((mission, exposure)) = loadout_of {
        issue_loadout(
            state,
            (catalog, &broker),
            mission,
            &agent.id,
            exposure,
            reserved.as_ref(),
            environment,
            &mut leases,
        )?;
    }
    if let Some(on) = on {
        issue_route_secrets(
            state,
            (catalog, &broker),
            holder,
            &agent.id,
            on,
            reserved.as_ref(),
            environment,
            &mut leases,
        )?;
    }
    // The pool account goes last, so nothing issued after it can stand in for its credential.
    if let Some(lease) =
        super::accounts::issue(state, catalog, &broker, holder, agent, site, environment)?
    {
        leases.0.push(Issued {
            lease,
            approval: None,
        });
    }
    keep(state, mission_id, leases)
}

/// The catalog and a broker over it, for a look at the models accounts and routes may use.
pub(super) fn listing_broker(state: &DaemonState) -> Option<(&LoadoutCatalog, SecretBroker)> {
    let desk = &state.station.credentials;
    let catalog = desk.catalog.as_ref()?;
    Some((
        catalog,
        desk.broker(
            &state.plugins,
            catalog,
            state.station.limits().secrets.max_lease_ttl,
        ),
    ))
}

/// Leases the secrets `route` names into `environment` for a look at its models, kept under `id`
/// until [`release`]. The route claims no account: it only reads.
pub(super) fn issue_for_listing(
    state: &DaemonState,
    id: &str,
    agent: &Agent,
    route: &crate::directorates::m::roster::AgentRoute,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let Some((catalog, broker)) = listing_broker(state) else {
        return Ok(());
    };
    let holder = Holder {
        id,
        operation: None,
    };
    let on = OnRoute {
        route: route.clone(),
        implicit: None,
    };
    let mut leases = Leases::default();
    issue_route_secrets(
        state,
        (catalog, &broker),
        holder,
        &agent.id,
        &on,
        None,
        environment,
        &mut leases,
    )?;
    keep(state, id, leases)
}

/// Leases the secrets `on`'s route names into `environment`. One whose variable is in `reserved`
/// (the route draws an account from a pool, which supplies that variable) is not issued.
#[allow(clippy::too_many_arguments)]
fn issue_route_secrets(
    state: &DaemonState,
    (catalog, broker): (&LoadoutCatalog, &SecretBroker),
    holder: Holder<'_>,
    agent: &str,
    on: &OnRoute,
    reserved: Option<&BTreeSet<String>>,
    environment: &mut BTreeMap<String, String>,
    leases: &mut Leases,
) -> Result<(), String> {
    let definitions: Vec<SecretDefinition> = on
        .route
        .secrets
        .iter()
        .map(|name| catalog.secret(name))
        .filter(|definition| {
            let kept = reserved.is_none_or(|reserved| !reserved.contains(definition.env()));
            if !kept {
                eprintln!(
                    "ai6: secret {:?} of a route of Agent {agent:?} is left out: the route draws \
                     from an account pool, which supplies {}",
                    definition.name(),
                    definition.env()
                );
            }
            kept
        })
        .collect();
    if definitions.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = definitions.iter().map(|d| d.name().to_owned()).collect();
    let request = SecretLeaseRequest::with_ttl(
        state.station_id.clone(),
        holder.id,
        state.station.limits().secrets.lease_ttl.0,
    )
    .map_err(|error| error.to_string())?;
    let lease = broker
        .lease_names(&Loadout::for_route(agent, &names), &names, request)
        .map_err(|error| format!("Agent {agent:?}'s route could not lease its secrets: {error}"))?;
    leases.hand_over(lease, None, &definitions, environment)
}

/// The loadout half of [`issue`]: the Agent's own secrets, ordinary and green ink, added to
/// `leases`. A secret whose variable is in `reserved` (a pooled Agent's credential
/// variables) is not issued, and that is logged.
#[allow(clippy::too_many_arguments)]
fn issue_loadout(
    state: &DaemonState,
    (catalog, broker): (&LoadoutCatalog, &SecretBroker),
    mission: &Mission,
    agent: &str,
    exposure: &ExposureGate,
    reserved: Option<&BTreeSet<String>>,
    environment: &mut BTreeMap<String, String>,
    leases: &mut Leases,
) -> Result<(), String> {
    let mission_id = mission.mission_id.to_string();
    let loadout = catalog
        .for_agent(agent)
        .ok()
        .filter(|loadout| !loadout.secret_names().is_empty());
    let Some(loadout) = loadout else {
        return Ok(());
    };
    // What an earlier Session of the Mission held goes, so only this issue's leases remain.
    for name in loadout.secret_names() {
        environment.remove(catalog.secret(name).env());
    }
    let restricted = exposure.restricts(Restriction::Secrets);
    if restricted {
        eprintln!(
            "ai6: Mission {mission_id:?} read untrusted input and its Operation restricts \
             secrets, so its Session gets only its harness's model credential"
        );
    }
    let (ordinary, protected): (Vec<_>, Vec<_>) = loadout
        .secret_names()
        .iter()
        .map(|name| catalog.secret(name))
        .filter(|definition| {
            !restricted
                || (!definition.green_ink() && CREDENTIAL_VARIABLES.contains(&definition.env()))
        })
        .filter(|definition| {
            let kept = reserved.is_none_or(|reserved| !reserved.contains(definition.env()));
            if !kept {
                eprintln!(
                    "ai6: secret {:?} is weakened for Mission {mission_id:?}: Agent {agent:?} \
                     draws from an account pool, so its variable {} is left to the pool account",
                    definition.name(),
                    definition.env()
                );
            }
            kept
        })
        .partition(|definition| !definition.green_ink());
    // Whatever was acquired is revoked if this issue fails part way.
    let mut leased = Vec::new();

    if !ordinary.is_empty() {
        let names: Vec<String> = ordinary.iter().map(|d| d.name().to_owned()).collect();
        let request = SecretLeaseRequest::with_ttl(
            state.station_id.clone(),
            mission_id.clone(),
            state.station.limits().secrets.lease_ttl.0,
        )
        .map_err(|error| error.to_string())?;
        let lease = broker
            .lease_names(loadout, &names, request)
            .map_err(|error| {
                format!("Agent {agent:?}'s credentials could not be leased: {error}")
            })?;
        leases.hand_over(lease, None, &ordinary, environment)?;
        leased.extend(names);
    }

    let mut approvals = Vec::new();
    if !protected.is_empty() {
        let mut approved = live_approvals(state, mission);
        for definition in &protected {
            let Some(position) = approved.iter().position(|approval| {
                approval.spent_at.is_none()
                    && definition
                        .unlocked_by(&approval.request.scope.kind, &approval.request.scope.target)
            }) else {
                continue;
            };
            let approval = approved.remove(position);
            let now = state.clock.now() as f64;
            let remaining = approval.expires_at.unwrap_or(now) - now;
            // Clamped before converting: an approval may expire centuries from now. A lapsed
            // one gives no time at all, which the lease request refuses.
            let ceiling = state.station.limits().secrets.max_lease_ttl.as_secs_f64();
            let ttl =
                Duration::try_from_secs_f64(remaining.clamp(0.0, ceiling)).unwrap_or(Duration::MAX);
            let names = vec![definition.name().to_owned()];
            let lease =
                SecretLeaseRequest::with_ttl(state.station_id.clone(), mission_id.clone(), ttl)
                    .and_then(|request| broker.lease_names(loadout, &names, request));
            let lease = match lease {
                Ok(lease) => lease,
                Err(error) => {
                    eprintln!(
                        "ai6: green ink credential {:?} for Mission {mission_id:?} was not leased: {error}",
                        definition.name()
                    );
                    continue;
                }
            };
            // Single use is held in the Registry, so another Station, or this one after a
            // restart, does not lease against the same approval again.
            let request = &approval.request;
            match state
                .intake
                .spend_approval(&request.operation, &request.id, now)
            {
                Ok(true) => {}
                Ok(false) => {
                    let _ = lease.revoke();
                    continue;
                }
                Err(error) => {
                    let _ = lease.revoke();
                    eprintln!(
                        "ai6: green ink approval {:?} could not be marked spent, so {:?} was not leased: {error}",
                        request.id,
                        definition.name()
                    );
                    continue;
                }
            }
            leases.hand_over(
                lease,
                Some(request.id.clone()),
                std::slice::from_ref(definition),
                environment,
            )?;
            approvals.push(request.id.clone());
            leased.push(definition.name().to_owned());
        }
    }

    if !leased.is_empty() {
        record(state, mission, agent, &leased, &approvals);
    }
    Ok(())
}

/// Keeps `leases` for `mission_id` until its Session stops here.
fn keep(state: &DaemonState, mission_id: &str, mut leases: Leases) -> Result<(), String> {
    if leases.0.is_empty() {
        return Ok(());
    }
    let replaced = state
        .station
        .credentials
        .issued
        .lock()
        .map_err(|_| "credential lock was poisoned")?
        .insert(mission_id.to_owned(), std::mem::take(&mut leases.0));
    // A concurrent issue for the same Mission lost the race; its leases go too.
    drop(Leases(replaced.unwrap_or_default()));
    Ok(())
}

/// A lease the Station issued, with the green ink approval that unlocked it, if any.
struct Issued {
    lease: SecretLease,
    approval: Option<String>,
}

/// Leases revoked when dropped.
#[derive(Default)]
struct Leases(Vec<Issued>);

impl Leases {
    /// Keeps `lease` and puts each of `definitions`' values in `environment` under its
    /// variable.
    fn hand_over(
        &mut self,
        lease: SecretLease,
        approval: Option<String>,
        definitions: &[SecretDefinition],
        environment: &mut BTreeMap<String, String>,
    ) -> Result<(), String> {
        self.0.push(Issued { lease, approval });
        let lease = &self.0.last().ok_or("no lease to hand over")?.lease;
        for definition in definitions {
            lease
                .with_secret(definition.name(), |value| {
                    environment.insert(definition.env().to_owned(), value.to_owned())
                })
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

impl Drop for Leases {
    fn drop(&mut self) {
        for Issued { lease, .. } in self.0.drain(..) {
            if let Err(error) = lease.revoke() {
                eprintln!("ai6: a credential lease was not revoked: {error}");
            }
        }
    }
}

/// Revokes every lease issued for `mission_id` and frees its pool account, once its Session
/// stops on this Station or the Mission ends.
pub(super) fn release(state: &DaemonState, mission_id: &str) {
    super::accounts::release(state, mission_id);
    let leases = state
        .station
        .credentials
        .issued
        .lock()
        .ok()
        .and_then(|mut issued| issued.remove(mission_id));
    for Issued { lease, .. } in leases.into_iter().flatten() {
        if let Err(error) = lease.revoke() {
            eprintln!("ai6: a credential lease of Mission {mission_id:?} was not revoked: {error}");
        }
    }
}

/// Revokes the leases whose time is up, and each green ink lease whose approval expired or was
/// revoked, so a protected credential lasts no longer than the approval that unlocked it. A
/// value already in a harness environment stays there; a backend that issues credentials of its
/// own stops honouring them. An approval the Registry cannot read keeps its lease until the
/// next pass.
pub(super) fn revoke_lapsed(state: &DaemonState) {
    let issued = &state.station.credentials.issued;
    let protected: Vec<(String, String)> = match issued.lock() {
        Ok(issued) => issued
            .iter()
            .flat_map(|(mission, leases)| {
                leases
                    .iter()
                    .filter_map(|issued| Some((mission.clone(), issued.approval.clone()?)))
            })
            .collect(),
        Err(_) => return,
    };
    let now = state.clock.now() as f64;
    let mut lapsed = BTreeSet::new();
    for (mission, approval) in protected {
        let operation = state
            .missions
            .lock()
            .ok()
            .and_then(|missions| missions.get(&mission).and_then(|m| m.operation_id.clone()));
        if operation.is_some_and(|operation| {
            !approval_live(state.intake.as_ref(), &operation, &approval, now)
        }) {
            lapsed.insert(approval);
        }
    }
    let mut revoked = Vec::new();
    if let Ok(mut issued) = issued.lock() {
        for (mission, leases) in issued.iter_mut() {
            let (keep, gone): (Vec<_>, Vec<_>) =
                std::mem::take(leases).into_iter().partition(|issued| {
                    issued.lease.is_active()
                        && issued
                            .approval
                            .as_ref()
                            .is_none_or(|approval| !lapsed.contains(approval))
                });
            *leases = keep;
            revoked.extend(gone.into_iter().map(|issued| (mission.clone(), issued)));
        }
        issued.retain(|_, leases| !leases.is_empty());
    }
    for (mission, issued) in revoked {
        let names = issued.lease.names();
        if let Err(error) = issued.lease.revoke() {
            eprintln!("ai6: a credential lease of Mission {mission:?} was not revoked: {error}");
        }
        if let Some(approval) = &issued.approval
            && let Some(operation) =
                state.missions.lock().ok().and_then(|missions| {
                    missions.get(&mission).and_then(|m| m.operation_id.clone())
                })
        {
            let event = TimelineEvent::new(
                TIMELINE_KIND,
                "revoked",
                operation,
                state.clock.now() as f64,
            )
            .station(state.station_id.clone())
            .mission(mission.clone())
            .detail("secrets", names.join(","))
            .detail("approval", approval.clone());
            if let Err(error) = state.intake.record_event(&event) {
                eprintln!("ai6: credential revocation was not recorded on the timeline: {error}");
            }
        }
    }
}

/// Whether approval `id` of `operation` is decided, approved and live at `now`. An unreadable
/// record counts as live.
fn approval_live(registry: &dyn RegistryBackend, operation: &str, id: &str, now: f64) -> bool {
    match registry.approval(operation, id, now) {
        Ok(Some(ApprovalStatus::Decided(approval))) => approval.is_live(now),
        Ok(_) => false,
        Err(error) => {
            eprintln!("ai6: green ink approval {id:?} unreadable: {error}");
            true
        }
    }
}

/// The Mission's approvals that are live now.
fn live_approvals(state: &DaemonState, mission: &Mission) -> Vec<Approval> {
    let Some(operation) = mission.operation_id.as_deref() else {
        return Vec::new();
    };
    let now = state.clock.now() as f64;
    match state
        .intake
        .mission_approvals(operation, mission.mission_id.as_str(), now)
    {
        Ok(statuses) => statuses
            .into_iter()
            .filter_map(|status| match status {
                ApprovalStatus::Decided(approval) if approval.is_live(now) => Some(approval),
                _ => None,
            })
            .collect(),
        Err(error) => {
            eprintln!(
                "ai6: green ink approvals of Mission {:?} unreadable: {error}",
                mission.mission_id.as_str()
            );
            Vec::new()
        }
    }
}

/// Records which credentials a Mission's Session was issued: names, never values.
fn record(
    state: &DaemonState,
    mission: &Mission,
    agent: &str,
    names: &[String],
    approvals: &[String],
) {
    let Some(operation) = mission.operation_id.as_deref() else {
        return;
    };
    let mut listed = String::new();
    for name in names {
        if !state
            .station
            .limits()
            .audit
            .names_detail_chars
            .allows(listed.len() + name.len() + 1)
        {
            break;
        }
        if !listed.is_empty() {
            listed.push(',');
        }
        listed.push_str(name);
    }
    let mut event =
        TimelineEvent::new(TIMELINE_KIND, "leased", operation, state.clock.now() as f64)
            .station(state.station_id.clone())
            .mission(mission.mission_id.to_string())
            .actor(agent)
            .detail("secrets", listed);
    if let Some(approval) = approvals.first() {
        event = event.detail("approval", approval.clone());
    }
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: credential lease was not recorded on the timeline: {error}");
    }
}

#[cfg(test)]
mod tests;
