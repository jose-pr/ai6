//! Universal Exports gateway command.

use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use ai6::guatiao::{ToValue, Value};
use ai6::{Authenticator, Authorizer, ConfigProvider, Directory, UnexRecord};
use clap::Parser;
use coqui_access_kit::CacheOptions;
use guatiao::value::Number;
use serde_json::Value as JsonValue;
use universal_exports::{
    AccessProviders, AssistantConfig, EitherAuthorizer, Gateway, GatewayConfig, GatewayLimits,
    LiveAccess, LoginLimits, ViewLimits,
};

#[derive(Debug, Parser)]
#[command(
    name = "universal-exports",
    about = "Serve the authenticated AI6 Universal Exports gateway",
    after_help = "Settings needed before the configuration provider is reached come from ai6.cfg: --config, else AI6_CONFIG, else ai6.cfg in the user's then the system's configuration directory. Its [unex] station_token is the Stations' credential. Sign-in, login limits and the assistant are the provider's unex record; the role policy is its access record."
)]
struct Cli {
    /// The bootstrap file, which must exist (default: AI6_CONFIG, else ai6.cfg in the user's,
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
    /// The loopback address used behind the trusted HTTPS reverse proxy (default: `[unex]
    /// listen`, else 127.0.0.1:7082).
    #[arg(long, value_name = "ADDRESS")]
    listen: Option<std::net::SocketAddr>,
    /// The public HTTPS origin used to validate browser requests (default: `[unex]
    /// public_origin`; one of them is required).
    #[arg(long)]
    public_origin: Option<String>,
    /// A directory or library containing Guatiao providers, besides `[unex.plugins] paths`; may
    /// be repeated.
    #[arg(long = "plugin-path", value_name = "PATH")]
    plugin_paths: Vec<PathBuf>,
    /// How provider libraries are checked: strict, report or off (default: `[unex.plugins]
    /// profile`, else strict with `plugin` records and report without).
    #[arg(long, value_name = "PROFILE")]
    plugin_profile: Option<ai6::PluginProfile>,
}

/// What the gateway reads from `ai6.cfg` with the command line over it.
struct Settings {
    located: ai6::bootstrap::Located<ai6::bootstrap::Bootstrap>,
    state_dir: PathBuf,
}

fn settings(cli: &Cli) -> Result<Settings, String> {
    let mut located = ai6::bootstrap::Bootstrap::find(cli.config.as_deref())
        .map_err(|error| error.to_string())?;
    let value = &mut located.value;
    if let Some(provider) = &cli.provider {
        value.provider.location = Some(provider.clone());
    }
    if let Some(registry) = &cli.registry {
        value.registry.location = Some(registry.clone());
    }
    let unex = &mut value.unex;
    unex.listen = cli.listen.or(unex.listen);
    unex.public_origin = cli.public_origin.clone().or(unex.public_origin.take());
    unex.plugins.paths.extend(cli.plugin_paths.iter().cloned());
    unex.plugins.profile = cli.plugin_profile.or(unex.plugins.profile);
    let state_dir = located
        .unex_state_dir()
        .map_err(|error| error.to_string())?;
    Ok(Settings { located, state_dir })
}

/// SQL options from `[registry]` (timeouts, limits) and the password `secret` refers to.
fn sql_options(
    settings: &Settings,
    secret: Option<&ai6::bootstrap::SecretRef>,
    what: &str,
) -> Result<ai6::SqlOptions, String> {
    let registry = &settings.located.value.registry;
    let options = ai6::SqlOptions::for_lease(registry.headquarters_lease())
        .with_busy_timeout(registry.sqlite_busy_timeout())
        .with_limits(registry.limits.clone());
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

/// Opens the configuration provider `ai6.cfg` names.
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
    let provider = ai6::directorates::m::config::open_with(
        &location,
        &options,
        settings.located.value.provider.limits(),
    )
    .map_err(|error| format!("the configuration provider at {shown} cannot be opened: {error}"))?;
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
    for weakening in settings.located.unex_weakenings() {
        eprintln!("universal-exports: warning: {weakening}");
    }
    let listen = settings.located.unex_listen();
    let tls = own_tls(&settings)?;
    if tls.is_none() && listen.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
        return Err(format!(
            "[unex] listen must be a 127.0.0.1 address behind the reverse proxy, not {listen}, \
             unless [unex.tls] gives the gateway TLS of its own"
        ));
    }
    let public_origin = settings
        .located
        .value
        .unex
        .public_origin
        .clone()
        .ok_or("[unex] public_origin is required (ai6.cfg or --public-origin)")?;
    let station_token = settings
        .located
        .value
        .unex
        .station_token
        .as_ref()
        .ok_or("[unex] station_token is required in ai6.cfg")?
        .read()
        .map_err(|error| format!("[unex] station_token: {error}"))?;
    let provider = open_provider(&settings)?;
    let snapshot = provider
        .load("")
        .map_err(|error| format!("the configuration provider: {error}"))?;
    let record = snapshot.settings.unex.clone();
    for weakening in record.weakenings() {
        eprintln!("universal-exports: warning: {weakening}");
    }
    record
        .limits()
        .validate()
        .map_err(|error| format!("unex.{error}"))?;
    let config = GatewayConfig::new(
        public_origin,
        station_token.as_str().to_owned(),
        GatewayLimits::from(&record.gateway),
        record.gateway.attach_token_ttl.0,
        record.gateway.write_lease_ttl.0,
    )
    .map_err(|error| format!("gateway configuration failed: {error}"))?
    .with_unex_limits(record.limits())
    .with_login_limits(login_limits(&record)?);
    let registry_location = settings
        .located
        .unex_registry_location()
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
    let host_chiefs = settings.located.value.unex.access.chief.clone();
    for subject in &host_chiefs {
        subject.parse::<ai6::Subject>().map_err(|_| {
            "[unex.access] chief: an entry is not account:<name>, person:<id> or group:<name>"
                .to_owned()
        })?;
    }
    let live = Arc::new(
        LiveAccess::new(provider.clone(), host_chiefs, record.authorizer.clone())
            .map_err(|error| format!("user access configuration failed: {error}"))?,
    );
    let mut snapshot = snapshot;
    if user_access_configured(&record) {
        let seeded = {
            let (live, snapshot) = (live.clone(), snapshot.clone());
            let registry = registry.clone();
            tokio::task::spawn_blocking(move || live.seed(&snapshot, &registry))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| format!("seeding the access policy failed: {error}"))?
        };
        if seeded {
            snapshot = provider
                .load("")
                .map_err(|error| format!("the configuration provider: {error}"))?;
        }
    }
    live.apply(&snapshot);
    let access = configured_access(&settings, &snapshot, &live)
        .map_err(|error| format!("user access configuration failed: {error}"))?;
    if access.is_some() && live.has_no_chief() {
        eprintln!(
            "universal-exports: access: no chief; nobody can change access; set [unex.access] chief in ai6.cfg"
        );
    }
    let mut gateway = match access {
        Some((access, accounts)) => {
            let gateway =
                Gateway::with_access(config, registry, access).with_access_admin(live.clone());
            match accounts {
                Some(accounts) => gateway.with_accounts(accounts),
                None => gateway,
            }
        }
        None => Gateway::with_registry(config, registry),
    };
    gateway = gateway
        .with_roster(provider.clone())
        .with_job_poll_max_wait(snapshot.roster.assistant().gateway().max_wait.0);
    if record.assistant {
        let assistant_record = snapshot.roster.assistant();
        let (body_bytes, _, default_ttl) = snapshot.settings.cables.message_limits();
        let mut assistant = AssistantConfig::new(record.assistant_agent.clone())
            .map_err(|error| format!("the assistant Agent is not valid: {error}"))?
            .with_view_limits(view_limits(assistant_record.views())?)
            .with_limits(assistant_record.limits().clone())
            .and_then(|assistant| assistant.with_gateway_limits(assistant_record.gateway().clone()))
            .map_err(|error| format!("assistant: {error}"))?
            .with_cable_limits(body_bytes, default_ttl);
        if let Some(bytes) = record.route_text_bytes {
            assistant =
                assistant.with_route_text_bytes(usize::try_from(bytes).unwrap_or(usize::MAX));
        }
        gateway = gateway.with_assistant(assistant);
        let pages = &settings.located.value.unex.pages;
        let pages_dir = settings
            .located
            .pages_dir()
            .map_err(|error| format!("[unex.pages] dir: {error}"))?;
        eprintln!(
            "universal-exports: pages are kept in {}",
            pages_dir.display()
        );
        let mut config = universal_exports::PagesConfig::new(pages_dir);
        if let Some(origin) = &pages.origin {
            let listen = settings.located.pages_listen();
            if listen.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
                return Err(format!(
                    "[unex.pages] listen must be a 127.0.0.1 address behind the reverse proxy, not \
                     {listen}"
                ));
            }
            config = config.with_origin(origin.clone(), listen);
        }
        gateway = gateway
            .with_pages(config)
            .map_err(|error| format!("pages: {error}"))?;
        if let Some(weakening) =
            public_pages_weakening(pages.origin.is_some(), assistant_record.pages().public)
        {
            eprintln!("universal-exports: warning: {weakening}");
        }
        if let Some(origin) = &pages.origin {
            let listen = settings.located.pages_listen();
            let own = gateway.clone();
            eprintln!("universal-exports: pages are served at {origin} (listening on {listen})");
            tokio::spawn(async move {
                if let Err(error) = own.serve_pages(listen, shutdown_signal()).await {
                    eprintln!("universal-exports: the pages listener failed: {error}");
                }
            });
        }
    }
    let liaison_settings = &settings.located.value.unex.liaison;
    if liaison_settings.listen.is_some() || !liaison_settings.peer.is_empty() {
        gateway = gateway.with_liaison(
            universal_exports::liaison::LiaisonConfig::from_settings(liaison_settings)
                .map_err(|error| error.to_string())?,
        );
    }
    // Held until the gateway stops; dropping it closes the link listener.
    let _liaison = match liaison_settings.listen {
        Some(listen) => Some(start_liaison(&settings, &gateway, listen, tls.clone())?),
        None => None,
    };
    let poll = settings.located.value.provider.poll();
    if !poll.is_zero() {
        tokio::spawn(follow_policy(provider, live, snapshot.clone(), poll));
    }
    match tls {
        Some(config) => {
            let listener = tokio::net::TcpListener::bind(listen)
                .await
                .map_err(|error| format!("gateway listener failed: {error}"))?;
            eprintln!("universal-exports: serving TLS of its own on {listen}");
            gateway.serve_tls(listener, config, shutdown_signal()).await
        }
        None => {
            gateway
                .serve_loopback(listen.port(), shutdown_signal())
                .await
        }
    }
    .map_err(|error| format!("gateway listener failed: {error}"))
}

/// `[unex.liaison]`: serves A2A to the paired peers on links at `listen`, over TLS when the
/// gateway has its own, and carries Cables sent to them.
fn start_liaison(
    settings: &Settings,
    gateway: &universal_exports::Gateway,
    listen: std::net::SocketAddr,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> Result<universal_exports::liaison::LinkListener, String> {
    let located = &settings.located;
    let liaison = gateway.liaison().ok_or("[unex.liaison] is not set up")?;
    let peers = located.value.unex.liaison.peer.len();
    let identity = located
        .liaison_identity()
        .map_err(|error| format!("[unex.liaison] identity: {error}"))?;
    let listener =
        universal_exports::liaison::LinkListener::start(&liaison, listen, tls, &identity)
            .map_err(|error| format!("[unex.liaison] {error}"))?;
    eprintln!(
        "universal-exports: Liaison serves A2A links on {} as {} to {peers} paired peer(s)",
        listener.local_addr(),
        listener.peer_id()
    );
    Ok(listener)
}

/// `[unex.tls]`: the TLS configuration of a gateway that serves TLS itself; `None` without one.
fn own_tls(settings: &Settings) -> Result<Option<std::sync::Arc<rustls::ServerConfig>>, String> {
    let tls = &settings.located.value.unex.tls;
    let (certificate, key) = match (&tls.certificate, &tls.key) {
        (None, None) => return Ok(None),
        (Some(certificate), Some(key)) => (certificate, key),
        _ => return Err("[unex.tls] needs both certificate and key".to_owned()),
    };
    let read = |path: &std::path::Path| {
        std::fs::read(path).map_err(|error| format!("[unex.tls] {}: {error}", path.display()))
    };
    universal_exports::tls_server_config(&read(certificate)?, &read(key)?)
        .map(|config| Some(std::sync::Arc::new(config)))
        .map_err(|error| format!("[unex.tls]: {error}"))
}

/// Reads the provider every `every` and applies a changed `access` or `operation_access`
/// record; a changed record of a family only a restart applies (`unex`, `plugin`) is logged as
/// needing one. A configuration the provider refuses keeps the policy in force, and the access
/// admin shows why.
async fn follow_policy(
    provider: Arc<dyn ConfigProvider>,
    live: Arc<LiveAccess>,
    started: ai6::ConfigSnapshot,
    every: Duration,
) {
    let mut since = started.revision;
    let started = Arc::new(started);
    let mut refused = None;
    let mut restart: Vec<ai6::ConfigRecord> = Vec::new();
    loop {
        tokio::time::sleep(every).await;
        let (provider, live, base) = (provider.clone(), live.clone(), started.clone());
        let outcome = {
            let live = live.clone();
            tokio::task::spawn_blocking(move || {
                let snapshot = match provider.changes_since("", since) {
                    Ok(Some(snapshot)) => snapshot,
                    Ok(None) => return Ok(None),
                    Err(error) => return Err(error.to_string()),
                };
                let changed: Vec<ai6::ConfigRecord> = base
                    .changed_families(&snapshot)
                    .into_iter()
                    .filter(|record| {
                        matches!(record, ai6::ConfigRecord::Unex | ai6::ConfigRecord::Plugin)
                    })
                    .collect();
                live.apply(&snapshot);
                Ok(Some((snapshot.revision, changed)))
            })
            .await
        };
        match outcome {
            Ok(Ok(Some((revision, changed)))) => {
                since = revision;
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
            Ok(Err(error)) => {
                if refused.as_ref() != Some(&error) {
                    eprintln!(
                        "universal-exports: configuration refused, keeping the access policy it runs: {error}"
                    );
                    live.mark_invalid(&error);
                    refused = Some(error);
                }
            }
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
        (
            "svg_id_bytes",
            record.svg_id_bytes,
            &mut limits.svg_id_bytes,
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
        name_bytes: record.login.name_bytes.or_max(),
        forget_every: u32::try_from(record.login.forget_every).unwrap_or(u32::MAX),
        address_window: record.login.address_window.0,
        name_window: record.login.name_window.0,
    })
}

/// The provider library policy, chosen as a Station chooses its own: the provider's `plugin`
/// records under `[unex.plugins] profile`; verified copies under the state directory's
/// `plugins/verified`.
fn plugin_policy(
    settings: &Settings,
    snapshot: &ai6::ConfigSnapshot,
) -> ai6::station::plugins::PluginPolicy {
    let policy = ai6::station::plugins::PluginPolicy::new(
        snapshot.settings.pins.clone(),
        settings.state_dir.join("plugins").join("verified"),
    )
    .with_keep_verified(settings.located.value.unex.plugins.keep_verified());
    match settings.located.value.unex.plugins.profile {
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

/// Whether the `unex` record asks for user sign-in and access.
fn user_access_configured(record: &UnexRecord) -> bool {
    record.authenticator.is_some()
        || record.proxy_user_header.is_some()
        || record.directory.is_some()
        || record.authorizer.is_some()
}

/// The user access providers and the accounts provider, when there is one.
type Configured = (AccessProviders, Option<Arc<dyn ai6::Accounts>>);

/// The user access providers `snapshot` configures, with the accounts provider when there is
/// one; `None` without user access.
fn configured_access(
    settings: &Settings,
    snapshot: &ai6::ConfigSnapshot,
    live: &Arc<LiveAccess>,
) -> Result<Option<Configured>, String> {
    let record = &snapshot.settings.unex;
    let needs_directory = user_access_configured(record) || snapshot.settings.access.is_some();
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
        &settings.located.value.unex.plugins.paths,
        &plugin_policy(settings, snapshot),
    );
    ai6::station::plugins::log_scan(plugins.scan_summary());
    let config = |value: &Option<ai6::ProviderConfig>| {
        value
            .clone()
            .map(|object| {
                json_to_provider_value(
                    JsonValue::Object(object.into_inner()),
                    0,
                    record.provider_config_depth,
                )
            })
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
        Some(provider) => Arc::new(EitherAuthorizer {
            live: live.clone(),
            plugin: plugins
                .cached_authorizer(provider, authorizer_config.as_ref(), cache)
                .map_err(|error| error.to_string())?,
        }),
        None => live.clone(),
    };
    let accounts_id = record.accounts.as_deref().or_else(|| {
        let offered = plugins.account_managers();
        record
            .authenticator
            .as_deref()
            .filter(|id| offered.iter().any(|info| info.id == *id))
    });
    let accounts: Option<Arc<dyn ai6::Accounts>> = accounts_id
        .map(|id| {
            let wanted = if record.accounts.is_some() {
                config(&record.accounts_config)?
            } else {
                authenticator_config.clone()
            };
            plugins
                .accounts(id, wanted.as_ref())
                .map(Arc::from)
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let header = |header: &str| {
        axum::http::HeaderName::from_bytes(header.as_bytes())
            .map_err(|_| "unex.proxy_user_header is not a valid HTTP header name".to_owned())
    };
    let access = match (authenticator, record.proxy_user_header.as_deref()) {
        (Some(authenticator), Some(name)) => {
            AccessProviders::new(authenticator, directory, authorizer)
                .with_proxy_identity(header(name)?, record.proxy_lookup_by.clone())
                .with_proxy_secret(proxy_secret(settings)?, proxy_secret_min(settings))?
        }
        (Some(authenticator), None) => AccessProviders::new(authenticator, directory, authorizer),
        (None, Some(name)) => AccessProviders::from_reverse_proxy(
            directory,
            authorizer,
            header(name)?,
            record.proxy_lookup_by.clone(),
        )
        .with_proxy_secret(proxy_secret(settings)?, proxy_secret_min(settings))?,
        (None, None) => {
            return Err("configure unex.authenticator or unex.proxy_user_header".to_owned());
        }
    };
    let access = match record.page_authenticator.as_deref() {
        Some(id) => {
            let page_config = config(&record.page_authenticator_config)?;
            let page_authenticator = plugins
                .authenticator(id, page_config.as_ref())
                .map_err(|error| format!("unex.page_authenticator: {error}"))?;
            access.with_page_authenticator(Arc::from(page_authenticator))
        }
        None => access,
    };
    Ok(Some((access, accounts)))
}

/// `[unex] proxy_secret_min_bytes`: the fewest bytes the proxy secret may hold.
fn proxy_secret_min(settings: &Settings) -> usize {
    usize::try_from(settings.located.proxy_secret_min_bytes()).unwrap_or(usize::MAX)
}

/// `[unex] proxy_secret`, which the reverse proxy sends in `X-AI6-Proxy-Secret`.
fn proxy_secret(settings: &Settings) -> Result<String, String> {
    let secret = settings
        .located
        .value
        .unex
        .proxy_secret
        .as_ref()
        .ok_or_else(|| {
            "a reverse-proxy identity needs [unex] proxy_secret, the secret the reverse proxy \
         sends in X-AI6-Proxy-Secret, so a client reaching the gateway's port directly cannot \
         assert an identity"
                .to_owned()
        })?;
    secret
        .read()
        .map(|value| value.as_str().to_owned())
        .map_err(|error| format!("[unex] proxy_secret: {error}"))
}

fn json_to_provider_value(
    value: JsonValue,
    depth: usize,
    most: ai6::Limit,
) -> Result<Value, String> {
    if !most.allows(depth) {
        return Err(format!(
            "provider configuration exceeds {most} nested values"
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
                converted.push_back(json_to_provider_value(value, depth + 1, most)?);
            }
            encode_provider_value(&converted)
        }
        JsonValue::Object(values) => {
            let mut converted = BTreeMap::new();
            for (key, value) in values {
                converted.insert(key, json_to_provider_value(value, depth + 1, most)?);
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

/// Resolves at Ctrl-C, or on unix at SIGTERM, which a service manager stops the gateway with.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    interrupted = tokio::signal::ctrl_c() => {
                        if let Err(error) = interrupted {
                            eprintln!(
                                "universal-exports: could not install the Ctrl-C handler: {error}"
                            );
                            terminate.recv().await;
                        }
                    }
                    _ = terminate.recv() => {}
                }
                return;
            }
            Err(error) => {
                eprintln!("universal-exports: could not install the SIGTERM handler: {error}");
            }
        }
    }
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("universal-exports: could not install the Ctrl-C handler: {error}");
    }
}

/// What a public link gives up when pages share the API's origin: an outsider then sees a page
/// she made under the app's own address.
fn public_pages_weakening(own_origin: bool, public: bool) -> Option<&'static str> {
    (public && !own_origin).then_some(
        "[assistant.pages] public = true with no [unex.pages] origin: a public link shows a page \
         Moneypenny made, to anyone holding it, under the app's own address",
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_public_link_on_the_apis_own_origin_is_a_weakening() {
        assert!(super::public_pages_weakening(false, true).is_some());
        assert!(super::public_pages_weakening(true, true).is_none());
        assert!(super::public_pages_weakening(false, false).is_none());
    }

    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_access_cache_defaults_to_seconds_and_reads_fractions_or_none() {
        let record = |json: serde_json::Value| -> UnexRecord {
            ai6::directorates::m::records::from_json(&json).unwrap()
        };
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
            ai6::read_definition::<ai6::AssistantSettings>(&json).unwrap()
        };
        assert_eq!(
            view_limits(record(serde_json::json!({})).views()).unwrap(),
            ViewLimits::default()
        );
        let tuned = record(serde_json::json!({"views": {
            "svg_bytes": 65536, "graph_nodes": 100, "svg_id_bytes": 16, "navigate_label_bytes": 200
        }}));
        let tuned = view_limits(tuned.views()).unwrap();
        assert_eq!(
            (
                tuned.svg_bytes,
                tuned.graph_nodes,
                tuned.svg_id_bytes,
                tuned.navigate_label_bytes
            ),
            (65536, 100, 16, 200)
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
            ai6::read_definition::<ai6::AssistantSettings>(
                &serde_json::json!({"views": {"pixels": 1}})
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
            ai6::Limit::at(64),
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
        for _ in 0..=64 {
            nested = serde_json::Value::Array(vec![nested]);
        }
        let value = serde_json::json!({ "nested": nested });
        let error = json_to_provider_value(value.clone(), 0, ai6::Limit::at(64)).unwrap_err();
        assert!(error.contains("64 nested values"), "{error}");
        assert!(json_to_provider_value(value, 0, ai6::Limit::UNBOUNDED).is_ok());
    }

    #[test]
    fn an_inline_credential_in_a_provider_config_is_a_weakening() {
        let record: UnexRecord = ai6::directorates::m::records::from_json(&serde_json::json!({
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
