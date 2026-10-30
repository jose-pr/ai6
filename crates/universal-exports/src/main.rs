//! Universal Exports gateway command.

use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use ai6::guatiao::{ToValue, Value};
use ai6::{
    Authenticator, AuthorizationDecision, AuthorizationRequest, Authorizer, ConfigProvider,
    ConfigRevision, Directory, RoleAuthorizer, UnexRecord,
};
use clap::Parser;
use coqui_access_kit::CacheOptions;
use guatiao::value::Number;
use serde_json::Value as JsonValue;
use universal_exports::{
    AccessProviders, AssistantConfig, Gateway, GatewayConfig, GatewayLimits, LoginLimits,
    ViewLimits,
};

const MAX_PROVIDER_CONFIG_DEPTH: usize = 64;

#[derive(Debug, Parser)]
#[command(
    name = "universal-exports",
    about = "Serve the authenticated AI6 Universal Exports gateway",
    after_help = "Settings needed before the configuration provider is reached come from unex.cfg: --config, else UNEX_CONFIG, else unex.cfg in the user's then the system's configuration directory. Its station_token is the Stations' credential. Sign-in, login limits and the assistant are the provider's unex record; the role policy is its access record."
)]
struct Cli {
    /// The bootstrap file, which must exist (default: UNEX_CONFIG, else unex.cfg in the user's,
    /// then the system's configuration directory).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Where the configuration provider is: a configuration directory (read-only),
    /// `sqlite:PATH` or a `postgres://` URL; its roster is served for administration under
    /// /v1/user/roster (default: `[provider] location`, else the directory holding the
    /// bootstrap file, else the user's configuration directory).
    #[arg(long, value_name = "LOCATION")]
    provider: Option<String>,
    /// Where the shared Registry is (default: `[registry] location`, else the provider's
    /// database when it is SQL, else `registry` in the state directory).
    #[arg(long, value_name = "LOCATION")]
    registry: Option<String>,
    /// The loopback address used behind the trusted HTTPS reverse proxy (default: `listen`,
    /// else 127.0.0.1:7082).
    #[arg(long, value_name = "ADDRESS")]
    listen: Option<std::net::SocketAddr>,
    /// The public HTTPS origin used to validate browser requests (default: `public_origin`;
    /// one of them is required).
    #[arg(long)]
    public_origin: Option<String>,
    /// A directory or library containing Guatiao providers, besides `[plugins] paths`; may be
    /// repeated.
    #[arg(long = "plugin-path", value_name = "PATH")]
    plugin_paths: Vec<PathBuf>,
    /// How provider libraries are checked: strict, report or off (default: `[plugins]
    /// profile`, else strict with `plugin` records and report without).
    #[arg(long, value_name = "PROFILE")]
    plugin_profile: Option<ai6::PluginProfile>,
}

/// What the gateway reads from `unex.cfg` with the command line over it.
struct Settings {
    located: ai6::bootstrap::Located<ai6::bootstrap::UnexBootstrap>,
    state_dir: PathBuf,
}

fn settings(cli: &Cli) -> Result<Settings, String> {
    let mut located = ai6::bootstrap::UnexBootstrap::find(cli.config.as_deref())
        .map_err(|error| error.to_string())?;
    let value = &mut located.value;
    if let Some(provider) = &cli.provider {
        value.provider.location = Some(provider.clone());
    }
    if let Some(registry) = &cli.registry {
        value.registry.location = Some(registry.clone());
    }
    value.listen = cli.listen.or(value.listen);
    value.public_origin = cli.public_origin.clone().or(value.public_origin.take());
    value.plugins.paths.extend(cli.plugin_paths.iter().cloned());
    value.plugins.profile = cli.plugin_profile.or(value.plugins.profile);
    let state_dir = located.state_dir().map_err(|error| error.to_string())?;
    Ok(Settings { located, state_dir })
}

/// SQL options from the Headquarters lease and the password `secret` refers to.
fn sql_options(
    settings: &Settings,
    secret: Option<&ai6::bootstrap::SecretRef>,
    what: &str,
) -> Result<ai6::SqlOptions, String> {
    let options = ai6::SqlOptions::for_lease(settings.located.value.registry.headquarters_lease());
    match secret {
        Some(secret) => {
            let password = secret
                .read()
                .map_err(|error| format!("the {what} password: {error}"))?;
            Ok(options.with_password(password.as_str()))
        }
        None => Ok(options),
    }
}

/// Opens the configuration provider `unex.cfg` names.
fn open_provider(settings: &Settings) -> Result<Arc<dyn ConfigProvider>, String> {
    let location = settings
        .located
        .provider_location()
        .map_err(|error| error.to_string())?;
    let shown = ai6::directorates::registry::sql::redact(&location);
    let options = sql_options(
        settings,
        settings.located.value.provider.password.as_ref(),
        "provider",
    )?;
    let provider =
        ai6::directorates::m::config::open_with(&location, &options).map_err(|error| {
            format!("the configuration provider at {shown} cannot be opened: {error}")
        })?;
    for weakening in ai6::directorates::registry::sql::weakenings(&location) {
        eprintln!("universal-exports: warning: the configuration provider at {shown}: {weakening}");
    }
    Ok(provider)
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("universal-exports: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let settings = settings(&cli)?;
    for weakening in settings.located.weakenings() {
        eprintln!("universal-exports: warning: {weakening}");
    }
    let listen = settings.located.listen();
    if listen.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
        return Err(format!(
            "listen must be a 127.0.0.1 address behind the reverse proxy, not {listen}"
        ));
    }
    let public_origin = settings
        .located
        .value
        .public_origin
        .clone()
        .ok_or("public_origin is required (unex.cfg or --public-origin)")?;
    let station_token = settings
        .located
        .value
        .station_token
        .as_ref()
        .ok_or("station_token is required in unex.cfg")?
        .read()
        .map_err(|error| format!("station_token: {error}"))?;
    let provider = open_provider(&settings)?;
    let snapshot = provider
        .load("")
        .map_err(|error| format!("the configuration provider: {error}"))?;
    let record = &snapshot.settings.unex;
    for weakening in record.weakenings() {
        eprintln!("universal-exports: warning: {weakening}");
    }
    let config = GatewayConfig::new(
        public_origin,
        station_token.as_str().to_owned(),
        GatewayLimits::default(),
        Duration::from_secs(300),
        Duration::from_secs(30),
    )
    .map_err(|error| format!("gateway configuration failed: {error}"))?
    .with_login_limits(login_limits(record)?);
    let registry_location = settings
        .located
        .registry_location()
        .map_err(|error| error.to_string())?;
    let registry = sql_options(&settings, settings.located.registry_password(), "Registry")
        .and_then(|options| {
            ai6::directorates::registry::backend::open_with(&registry_location, &options)
                .map_err(|error| error.to_string())
        })
        .map_err(|error| format!("shared access-token Registry is unavailable: {error}"))?;
    for weakening in ai6::directorates::registry::sql::weakenings(&registry_location) {
        eprintln!(
            "universal-exports: warning: the Registry at {}: {weakening}",
            ai6::directorates::registry::sql::redact(&registry_location)
        );
    }
    let policy = Arc::new(LivePolicy::new(
        snapshot.revision,
        snapshot.settings.access_authorizer()?,
    ));
    let access = configured_access(&settings, &snapshot, policy.clone())
        .map_err(|error| format!("user access configuration failed: {error}"))?;
    let mut gateway = match access {
        Some(access) => Gateway::with_access(config, registry, access),
        None => Gateway::with_registry(config, registry),
    };
    gateway = gateway.with_roster(provider.clone());
    if record.assistant {
        let mut assistant = AssistantConfig::new(record.assistant_agent.clone())
            .map_err(|error| format!("the assistant Agent is not valid: {error}"))?
            .with_view_limits(view_limits(snapshot.roster.assistant().views())?);
        if let Some(bytes) = record.route_text_bytes {
            assistant =
                assistant.with_route_text_bytes(usize::try_from(bytes).unwrap_or(usize::MAX));
        }
        gateway = gateway.with_assistant(assistant);
    }
    let poll = settings.located.value.provider.poll();
    if !poll.is_zero() {
        tokio::spawn(follow_policy(provider, policy, snapshot.clone(), poll));
    }
    gateway
        .serve_loopback(listen.port(), shutdown_signal())
        .await
        .map_err(|error| format!("gateway listener failed: {error}"))
}

/// The role policy of the `access` record, replaced while the gateway runs when the record
/// changes; a policy the provider does not accept is refused and the last good one kept.
struct LivePolicy {
    current: RwLock<(ConfigRevision, Arc<RoleAuthorizer>)>,
}

impl LivePolicy {
    fn new(revision: ConfigRevision, policy: RoleAuthorizer) -> Self {
        Self {
            current: RwLock::new((revision, Arc::new(policy))),
        }
    }

    fn revision(&self) -> ConfigRevision {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }

    fn replace(&self, revision: ConfigRevision, policy: RoleAuthorizer) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) =
            (revision, Arc::new(policy));
    }
}

impl Authorizer for LivePolicy {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ai6::guatiao::library::ProviderError> {
        let policy = self
            .current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .clone();
        policy.authorize(request)
    }
}

/// Reads the provider every `every` and applies a changed `access` record; a changed record
/// of a family only a restart applies (`unex`, `plugin`) is logged as needing one.
async fn follow_policy(
    provider: Arc<dyn ConfigProvider>,
    policy: Arc<LivePolicy>,
    started: ai6::ConfigSnapshot,
    every: Duration,
) {
    let started = Arc::new(started);
    let mut refused = None;
    let mut restart: Vec<ai6::ConfigRecord> = Vec::new();
    loop {
        tokio::time::sleep(every).await;
        let (provider, policy, base) = (provider.clone(), policy.clone(), started.clone());
        let since = policy.revision();
        let outcome = tokio::task::spawn_blocking(move || {
            let snapshot = match provider.changes_since("", since) {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => return Ok(None),
                Err(error) => return Err((None, error.to_string())),
            };
            let changed: Vec<ai6::ConfigRecord> = base
                .changed_families(&snapshot)
                .into_iter()
                .filter(|record| {
                    matches!(record, ai6::ConfigRecord::Unex | ai6::ConfigRecord::Plugin)
                })
                .collect();
            match snapshot.settings.access_authorizer() {
                Ok(authorizer) => {
                    policy.replace(snapshot.revision, authorizer);
                    Ok(Some((snapshot.revision, changed)))
                }
                Err(error) => Err((Some(snapshot.revision), error)),
            }
        })
        .await;
        match outcome {
            Ok(Ok(Some((revision, changed)))) => {
                refused = None;
                eprintln!(
                    "universal-exports: configuration revision {} applied",
                    revision.get()
                );
                for record in &changed {
                    if !restart.contains(record) {
                        eprintln!(
                            "universal-exports: {record} changed; restart universal-exports to \
                             apply it"
                        );
                    }
                }
                restart = changed;
            }
            Ok(Ok(None)) => {}
            Ok(Err((revision, error))) if refused != Some(revision) => {
                eprintln!(
                    "universal-exports: configuration refused, keeping the access policy it runs: {error}"
                );
                refused = Some(revision);
            }
            Ok(Err(_)) => {}
            Err(error) => eprintln!("universal-exports: configuration poll failed: {error}"),
        }
    }
}

/// Moneypenny's view bounds from `[assistant.views]`, each absent one at its default.
fn view_limits(record: &ai6::AssistantViewLimits) -> Result<ViewLimits, String> {
    let mut limits = ViewLimits::default();
    for (key, value, limit) in [
        ("blocks", record.blocks, &mut limits.blocks),
        ("view_bytes", record.view_bytes, &mut limits.view_bytes),
        ("text_bytes", record.text_bytes, &mut limits.text_bytes),
        ("short_bytes", record.short_bytes, &mut limits.short_bytes),
        ("columns", record.columns, &mut limits.columns),
        ("rows", record.rows, &mut limits.rows),
        ("items", record.items, &mut limits.items),
        ("series", record.series, &mut limits.series),
        ("points", record.points, &mut limits.points),
        ("pie_slices", record.pie_slices, &mut limits.pie_slices),
        ("diff_bytes", record.diff_bytes, &mut limits.diff_bytes),
        ("fields", record.fields, &mut limits.fields),
        (
            "schedule_rows",
            record.schedule_rows,
            &mut limits.schedule_rows,
        ),
        ("graph_nodes", record.graph_nodes, &mut limits.graph_nodes),
        ("graph_edges", record.graph_edges, &mut limits.graph_edges),
        ("svg_bytes", record.svg_bytes, &mut limits.svg_bytes),
        (
            "svg_elements",
            record.svg_elements,
            &mut limits.svg_elements,
        ),
        ("svg_depth", record.svg_depth, &mut limits.svg_depth),
        (
            "navigate_label_bytes",
            record.navigate_label_bytes,
            &mut limits.navigate_label_bytes,
        ),
    ] {
        if let Some(value) = value {
            *limit = usize::try_from(value)
                .ok()
                .filter(|value| *value >= 1)
                .ok_or_else(|| format!("assistant.views.{key} must be at least 1"))?;
        }
    }
    Ok(limits)
}

fn login_limits(record: &UnexRecord) -> Result<LoginLimits, String> {
    let defaults = LoginLimits::default();
    let tries = |value: Option<u32>, default: NonZeroU32, key: &str| match value {
        Some(value) => {
            NonZeroU32::new(value).ok_or_else(|| format!("unex.{key} must be at least 1"))
        }
        None => Ok(default),
    };
    let address_header = record
        .client_address_header
        .as_deref()
        .map(|header| {
            axum::http::HeaderName::from_bytes(header.as_bytes()).map_err(|_| {
                "unex.client_address_header is not a valid HTTP header name".to_owned()
            })
        })
        .transpose()?;
    Ok(LoginLimits {
        per_address: tries(
            record.login_tries_per_address,
            defaults.per_address,
            "login_tries_per_address",
        )?,
        per_name: tries(
            record.login_tries_per_name,
            defaults.per_name,
            "login_tries_per_name",
        )?,
        address_header,
    })
}

/// The provider library policy, chosen as a Station chooses its own: the provider's `plugin`
/// records under `[plugins] profile`; verified copies under the state directory's
/// `plugins/verified`.
fn plugin_policy(
    settings: &Settings,
    snapshot: &ai6::ConfigSnapshot,
) -> ai6::station::plugins::PluginPolicy {
    let policy = ai6::station::plugins::PluginPolicy::new(
        snapshot.settings.pins.clone(),
        settings.state_dir.join("plugins").join("verified"),
    );
    match settings.located.value.plugins.profile {
        Some(profile) => policy.with_profile(profile),
        None => policy,
    }
}

/// How the loaded Directory and Authorizer keep their answers; a lifetime past the default is
/// reported, since it widens the window in which a revocation is not yet seen.
fn access_cache(record: &UnexRecord) -> Result<CacheOptions, String> {
    let seconds = |value: Option<f64>, key: &str| {
        value
            .map(|seconds| {
                Duration::try_from_secs_f64(seconds)
                    .map_err(|_| format!("unex.{key}: seconds must be finite and not negative"))
            })
            .transpose()
    };
    let mut cache = CacheOptions::default();
    if let Some(ttl) = seconds(record.access_cache_ttl, "access_cache_ttl")? {
        cache = cache.ttl(ttl);
    }
    if let Some(ttl) = seconds(
        record.access_cache_negative_ttl,
        "access_cache_negative_ttl",
    )? {
        cache = cache.negative_ttl(ttl);
    }
    if let Some(entries) = record.access_cache_entries {
        cache = cache.max_entries(entries);
    }
    if cache.exceeds_default() {
        eprintln!(
            "universal-exports: warning: access answers are kept {:?} ({:?} for a refusal), \
             longer than the default: a revoked account or grant passes that long",
            cache.ttl, cache.negative_ttl
        );
    }
    Ok(cache)
}

fn configured_access(
    settings: &Settings,
    snapshot: &ai6::ConfigSnapshot,
    policy: Arc<LivePolicy>,
) -> Result<Option<AccessProviders>, String> {
    let record = &snapshot.settings.unex;
    let needs_directory = record.authenticator.is_some()
        || record.proxy_user_header.is_some()
        || record.directory.is_some()
        || snapshot.settings.access.is_some()
        || record.authorizer.is_some();
    if !needs_directory {
        return Ok(None);
    }
    let directory_id = record
        .directory
        .as_deref()
        .ok_or("unex.directory is required when user access is configured")?;
    if snapshot.settings.access.is_some() && record.authorizer.is_some() {
        return Err("configure either the access record or unex.authorizer".to_owned());
    }
    let plugins = ai6::station::plugins::Plugins::scan_with(
        &settings.located.value.plugins.paths,
        &plugin_policy(settings, snapshot),
    );
    ai6::station::plugins::log_scan(plugins.scan_summary());
    let config = |value: &Option<serde_json::Map<String, JsonValue>>| {
        value
            .clone()
            .map(|object| json_to_provider_value(JsonValue::Object(object), 0))
            .transpose()
    };
    let authenticator_config = config(&record.authenticator_config)?;
    let directory_config = config(&record.directory_config)?;
    let authorizer_config = config(&record.authorizer_config)?;
    let authenticator: Option<Arc<dyn Authenticator>> = record
        .authenticator
        .as_deref()
        .map(|authenticator_id| {
            plugins
                .authenticator(authenticator_id, authenticator_config.as_ref())
                .map(Arc::from)
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let cache = access_cache(record)?;
    let directory: Arc<dyn Directory> = plugins
        .cached_directory(directory_id, directory_config.as_ref(), cache)
        .map_err(|error| error.to_string())?;
    let authorizer: Arc<dyn Authorizer> = match &record.authorizer {
        Some(provider) => plugins
            .cached_authorizer(provider, authorizer_config.as_ref(), cache)
            .map_err(|error| error.to_string())?,
        None => policy,
    };
    let header = |header: &str| {
        axum::http::HeaderName::from_bytes(header.as_bytes())
            .map_err(|_| "unex.proxy_user_header is not a valid HTTP header name".to_owned())
    };
    let access = match (authenticator, record.proxy_user_header.as_deref()) {
        (Some(authenticator), Some(name)) => {
            AccessProviders::new(authenticator, directory, authorizer)
                .with_proxy_identity(header(name)?, record.proxy_lookup_by.clone())
                .with_proxy_secret(proxy_secret(settings)?)?
        }
        (Some(authenticator), None) => AccessProviders::new(authenticator, directory, authorizer),
        (None, Some(name)) => AccessProviders::from_reverse_proxy(
            directory,
            authorizer,
            header(name)?,
            record.proxy_lookup_by.clone(),
        )
        .with_proxy_secret(proxy_secret(settings)?)?,
        (None, None) => {
            return Err("configure unex.authenticator or unex.proxy_user_header".to_owned());
        }
    };
    Ok(Some(access))
}

/// `unex.cfg`'s `proxy_secret`, which the reverse proxy sends in `X-AI6-Proxy-Secret`.
fn proxy_secret(settings: &Settings) -> Result<String, String> {
    let secret = settings
        .located
        .value
        .proxy_secret
        .as_ref()
        .ok_or_else(|| {
            "a reverse-proxy identity needs unex.cfg's proxy_secret, the secret the reverse proxy \
         sends in X-AI6-Proxy-Secret, so a client reaching the gateway's port directly cannot \
         assert an identity"
                .to_owned()
        })?;
    secret
        .read()
        .map(|value| value.as_str().to_owned())
        .map_err(|error| format!("proxy_secret: {error}"))
}

fn json_to_provider_value(value: JsonValue, depth: usize) -> Result<Value, String> {
    if depth > MAX_PROVIDER_CONFIG_DEPTH {
        return Err(format!(
            "provider configuration exceeds {MAX_PROVIDER_CONFIG_DEPTH} nested values"
        ));
    }
    match value {
        JsonValue::Null => encode_provider_value(&()),
        JsonValue::Bool(value) => encode_provider_value(&value),
        JsonValue::Number(value) => {
            let number = Number::new(&value.to_string())
                .map_err(|_| "provider configuration contains an invalid number".to_owned())?;
            encode_provider_value(&number)
        }
        JsonValue::String(value) => encode_provider_value(&value),
        JsonValue::Array(values) => {
            let mut converted = VecDeque::with_capacity(values.len());
            for value in values {
                converted.push_back(json_to_provider_value(value, depth + 1)?);
            }
            encode_provider_value(&converted)
        }
        JsonValue::Object(values) => {
            let mut converted = BTreeMap::new();
            for (key, value) in values {
                converted.insert(key, json_to_provider_value(value, depth + 1)?);
            }
            encode_provider_value(&converted)
        }
    }
}

fn encode_provider_value(value: &impl ToValue) -> Result<Value, String> {
    value
        .to_value()
        .map_err(|_| "provider configuration could not be represented".to_owned())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("universal-exports: could not install the Ctrl-C handler: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_access_cache_defaults_to_seconds_and_reads_fractions_or_none() {
        let record =
            |json: serde_json::Value| -> UnexRecord { serde_json::from_value(json).unwrap() };
        assert_eq!(
            access_cache(&record(serde_json::json!({}))).unwrap(),
            CacheOptions::default()
        );
        let tuned = access_cache(&record(serde_json::json!({
            "access_cache_ttl": 0.5,
            "access_cache_negative_ttl": 0,
            "access_cache_entries": 10,
        })))
        .unwrap();
        assert_eq!(
            (tuned.ttl, tuned.negative_ttl, tuned.max_entries),
            (Duration::from_millis(500), Duration::ZERO, 10)
        );
        assert!(
            access_cache(&record(serde_json::json!({"access_cache_ttl": 60})))
                .unwrap()
                .exceeds_default()
        );
        assert!(access_cache(&record(serde_json::json!({"access_cache_ttl": -1}))).is_err());
        assert!(login_limits(&record(serde_json::json!({"login_tries_per_name": 0}))).is_err());
    }

    #[test]
    fn view_limits_default_and_read_each_setting_of_the_record() {
        let record = |json: serde_json::Value| -> ai6::AssistantSettings {
            serde_json::from_value(json).unwrap()
        };
        assert_eq!(
            view_limits(record(serde_json::json!({})).views()).unwrap(),
            ViewLimits::default()
        );
        let tuned = record(serde_json::json!({"views": {
            "svg_bytes": 65536, "graph_nodes": 100, "navigate_label_bytes": 200
        }}));
        let tuned = view_limits(tuned.views()).unwrap();
        assert_eq!(
            (
                tuned.svg_bytes,
                tuned.graph_nodes,
                tuned.navigate_label_bytes
            ),
            (65536, 100, 200)
        );
        assert_eq!(ViewLimits::default().navigate_label_bytes, 120);
        assert_eq!(tuned.blocks, ViewLimits::default().blocks);
        let zero = record(serde_json::json!({"views": {"rows": 0}}));
        assert!(
            view_limits(zero.views())
                .unwrap_err()
                .contains("assistant.views.rows")
        );
        let refused = zero.validate().unwrap_err();
        assert!(refused.contains("assistant.views.rows"), "{refused}");
        assert!(
            serde_json::from_value::<ai6::AssistantSettings>(
                serde_json::json!({"views": {"pixels": 1}})
            )
            .is_err(),
            "an unknown bound is refused"
        );
    }

    #[test]
    fn provider_configs_convert_json_objects_to_guatiao_values() {
        use ai6::guatiao::FromValue;

        let value = json_to_provider_value(
            serde_json::json!({"path": "accounts.toml", "enabled": true, "nested": [1, true, null]}),
            0,
        )
        .unwrap();
        let fields = BTreeMap::<String, Value>::from_value(&value).unwrap();
        assert_eq!(
            String::from_value(&fields["path"]).unwrap(),
            "accounts.toml"
        );
        assert!(bool::from_value(&fields["enabled"]).unwrap());
        let nested = VecDeque::<Value>::from_value(&fields["nested"]).unwrap();
        assert_eq!(nested.len(), 3);
        assert_eq!(i64::from_value(&nested[0]).unwrap(), 1);
    }

    #[test]
    fn provider_config_rejects_more_than_sixty_four_nested_values() {
        let mut nested = serde_json::Value::Null;
        for _ in 0..=MAX_PROVIDER_CONFIG_DEPTH {
            nested = serde_json::Value::Array(vec![nested]);
        }
        let error = json_to_provider_value(serde_json::json!({ "nested": nested }), 0).unwrap_err();
        assert!(error.contains("64 nested values"), "{error}");
    }

    #[test]
    fn a_changed_access_record_replaces_the_policy_and_a_bad_one_is_kept_out() {
        let policy = LivePolicy::new(ConfigRevision::new(1), RoleAuthorizer::default());
        let allowed = ai6::role_authorizer_from_toml(
            "[role.viewer]\nactions = [\"mission.read\"]\n\
             [[rule]]\nsubject = \"group:ops\"\nscope = \"operation:G\"\nrole = \"viewer\"\n",
        )
        .unwrap();
        policy.replace(ConfigRevision::new(2), allowed);
        assert_eq!(policy.revision(), ConfigRevision::new(2));
    }

    #[test]
    fn an_inline_credential_in_a_provider_config_is_a_weakening() {
        let record: UnexRecord = serde_json::from_value(serde_json::json!({
            "directory_config": {"url": "ldaps://dc", "bind_password": "hunter2",
                                 "ca_file": "ca.pem", "password_file": "/run/pw"},
        }))
        .unwrap();
        let weakenings = record.weakenings();
        assert_eq!(weakenings.len(), 1, "{weakenings:?}");
        assert!(weakenings[0].contains("directory_config.bind_password"));
        assert!(!weakenings[0].contains("hunter2"));
    }
}
