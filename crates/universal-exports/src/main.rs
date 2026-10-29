//! Universal Exports gateway command.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::Read;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use ai6::guatiao::{ToValue, Value};
use ai6::{Authenticator, Authorizer, Directory, RoleAuthorizer};
use clap::Parser;
use coqui_access_kit::CacheOptions;
use guatiao::value::Number;
use serde_json::Value as JsonValue;
use universal_exports::{
    AccessProviders, AssistantConfig, Gateway, GatewayConfig, GatewayLimits, LoginLimits,
};

const MAX_PROVIDER_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_PROVIDER_CONFIG_DEPTH: usize = 64;

#[derive(Debug, Parser)]
#[command(
    name = "universal-exports",
    about = "Serve the authenticated AI6 Universal Exports gateway",
    after_help = "Settings needed before the configuration provider is reached come from unex.cfg: --config, else UNEX_CONFIG, else unex.cfg in the user's then the system's configuration directory. Its station_token is the Stations' credential. User access requires a Directory plus an Authenticator or a trusted reverse-proxy identity header."
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
    /// The provider library pin set, as a Station's (default: plugins.toml in the provider's
    /// directory); a file named here must exist.
    #[arg(long, env = "AI6_EXPORTS_PLUGIN_PINS", value_name = "PATH")]
    plugin_pins: Option<PathBuf>,
    /// How provider libraries are checked: strict, report or off (default: `[plugins]
    /// profile`, else the pin set's, else strict with a pin set and report without).
    #[arg(long, value_name = "PROFILE")]
    plugin_profile: Option<ai6::PluginProfile>,
    /// Guatiao Authenticator provider ID for user login.
    #[arg(long, env = "AI6_EXPORTS_AUTHENTICATOR")]
    authenticator: Option<String>,
    /// JSON configuration file for the selected Authenticator.
    #[arg(
        long,
        env = "AI6_EXPORTS_AUTHENTICATOR_CONFIG",
        requires = "authenticator"
    )]
    authenticator_config: Option<PathBuf>,
    /// Guatiao Directory provider ID for user identity lookup.
    #[arg(long, env = "AI6_EXPORTS_DIRECTORY")]
    directory: Option<String>,
    /// JSON configuration file for the selected Directory.
    #[arg(long, env = "AI6_EXPORTS_DIRECTORY_CONFIG", requires = "directory")]
    directory_config: Option<PathBuf>,
    /// Sanitized authenticated-user header set by the trusted reverse proxy.
    #[arg(long, env = "AI6_EXPORTS_PROXY_USER_HEADER")]
    proxy_user_header: Option<String>,
    /// Directory attribute used to resolve the reverse-proxy identity.
    #[arg(long, env = "AI6_EXPORTS_PROXY_LOOKUP_BY", default_value = "name")]
    proxy_lookup_by: String,
    /// Built-in RoleAuthorizer policy TOML; defaults to deny-all when omitted.
    #[arg(long, env = "AI6_EXPORTS_ACCESS_POLICY")]
    access_policy: Option<PathBuf>,
    /// Password or key tries a minute allowed from one client address at the login route.
    #[arg(
        long,
        env = "AI6_EXPORTS_LOGIN_TRIES_PER_ADDRESS",
        value_name = "TRIES",
        default_value_t = LoginLimits::default().per_address
    )]
    login_tries_per_address: NonZeroU32,
    /// Password or key tries in ten minutes allowed for one account name, from anywhere.
    #[arg(
        long,
        env = "AI6_EXPORTS_LOGIN_TRIES_PER_NAME",
        value_name = "TRIES",
        default_value_t = LoginLimits::default().per_name
    )]
    login_tries_per_name: NonZeroU32,
    /// Header the trusted reverse proxy sets to the client address, such as x-forwarded-for
    /// (the last entry counts) or x-real-ip. Without it every login counts as the proxy.
    #[arg(long, env = "AI6_EXPORTS_CLIENT_ADDRESS_HEADER", value_name = "HEADER")]
    client_address_header: Option<String>,
    /// Optional custom Guatiao Authorizer provider ID; conflicts with access-policy.
    #[arg(long, env = "AI6_EXPORTS_AUTHORIZER")]
    authorizer: Option<String>,
    /// JSON configuration file for the selected custom Authorizer.
    #[arg(long, env = "AI6_EXPORTS_AUTHORIZER_CONFIG", requires = "authorizer")]
    authorizer_config: Option<PathBuf>,
    /// Seconds the loaded Directory's person and the loaded Authorizer's grant are kept, to
    /// absorb bursts (a revoked account or grant can pass this long); 0 keeps none.
    #[arg(long, env = "AI6_EXPORTS_ACCESS_CACHE_TTL", value_name = "SECONDS",
          default_value = "5", value_parser = seconds)]
    access_cache_ttl: Duration,
    /// Seconds "no such person" and a denial are kept; 0 keeps none.
    #[arg(long, env = "AI6_EXPORTS_ACCESS_CACHE_NEGATIVE_TTL", value_name = "SECONDS",
          default_value = "1", value_parser = seconds)]
    access_cache_negative_ttl: Duration,
    /// The most lookups, and the most decisions, kept; 0 keeps none.
    #[arg(long, env = "AI6_EXPORTS_ACCESS_CACHE_ENTRIES", value_name = "ENTRIES",
          default_value_t = coqui_access_kit::cache::DEFAULT_MAX_ENTRIES)]
    access_cache_entries: u64,
    /// The roster Agent that is Moneypenny, the operator's assistant; her model runs on a
    /// Station with that Agent's handler, model, effort and credentials.
    #[arg(
        long,
        env = "AI6_EXPORTS_ASSISTANT_AGENT",
        value_name = "AGENT",
        default_value = "moneypenny"
    )]
    assistant_agent: String,
    /// Serve no assistant: the app's Moneypenny socket answers 503.
    #[arg(long, env = "AI6_EXPORTS_NO_ASSISTANT")]
    no_assistant: bool,
}

/// What the gateway reads from `unex.cfg` with the command line over it.
struct Settings {
    located: ai6::bootstrap::Located<ai6::bootstrap::UnexBootstrap>,
    state_dir: PathBuf,
    /// The provider's directory, where `plugins.toml` is read; `None` for a SQL provider.
    provider_dir: Option<PathBuf>,
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
    let provider = located
        .provider_location()
        .map_err(|error| error.to_string())?;
    let provider_dir =
        (!ai6::bootstrap::is_sql_location(&provider)).then(|| PathBuf::from(&provider));
    Ok(Settings {
        located,
        state_dir,
        provider_dir,
    })
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

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let settings = match settings(&cli) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("universal-exports: {error}");
            return ExitCode::FAILURE;
        }
    };
    for weakening in settings.located.weakenings() {
        eprintln!("universal-exports: warning: {weakening}");
    }
    let listen = settings.located.listen();
    if listen.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
        eprintln!(
            "universal-exports: listen must be a 127.0.0.1 address behind the reverse proxy, not {listen}"
        );
        return ExitCode::FAILURE;
    }
    let Some(public_origin) = settings.located.value.public_origin.clone() else {
        eprintln!("universal-exports: public_origin is required (unex.cfg or --public-origin)");
        return ExitCode::FAILURE;
    };
    let station_token = match settings
        .located
        .value
        .station_token
        .as_ref()
        .map(|token| token.read())
    {
        Some(Ok(token)) => token,
        Some(Err(error)) => {
            eprintln!("universal-exports: station_token: {error}");
            return ExitCode::FAILURE;
        }
        None => {
            eprintln!("universal-exports: station_token is required in unex.cfg");
            return ExitCode::FAILURE;
        }
    };
    let login_limits = match login_limits(&cli) {
        Ok(limits) => limits,
        Err(error) => {
            eprintln!("universal-exports: {error}");
            return ExitCode::FAILURE;
        }
    };
    let config = match GatewayConfig::new(
        public_origin,
        station_token.as_str().to_owned(),
        GatewayLimits::default(),
        Duration::from_secs(300),
        Duration::from_secs(30),
    ) {
        Ok(config) => config.with_login_limits(login_limits),
        Err(error) => {
            eprintln!("universal-exports: gateway configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let registry_location = match settings.located.registry_location() {
        Ok(location) => location,
        Err(error) => {
            eprintln!("universal-exports: {error}");
            return ExitCode::FAILURE;
        }
    };
    let registry = match sql_options(&settings, settings.located.registry_password(), "Registry")
        .and_then(|options| {
            ai6::directorates::registry::backend::open_with(&registry_location, &options)
                .map_err(|error| error.to_string())
        }) {
        Ok(registry) => {
            for weakening in ai6::directorates::registry::sql::weakenings(&registry_location) {
                eprintln!(
                    "universal-exports: warning: the Registry at {}: {weakening}",
                    ai6::directorates::registry::sql::redact(&registry_location)
                );
            }
            registry
        }
        Err(error) => {
            eprintln!("universal-exports: shared access-token Registry is unavailable: {error}");
            return ExitCode::FAILURE;
        }
    };
    let access = match configured_access(&cli, &settings) {
        Ok(access) => access,
        Err(error) => {
            eprintln!("universal-exports: user access configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut gateway = match access {
        Some(access) => Gateway::with_access(config, registry, access),
        None => Gateway::with_registry(config, registry),
    };
    let location = match settings.located.provider_location() {
        Ok(location) => location,
        Err(error) => {
            eprintln!("universal-exports: {error}");
            return ExitCode::FAILURE;
        }
    };
    let shown = ai6::directorates::registry::sql::redact(&location);
    let opened = sql_options(
        &settings,
        settings.located.value.provider.password.as_ref(),
        "provider",
    )
    .and_then(|options| {
        ai6::directorates::m::config::open_with(&location, &options)
            .map_err(|error| error.to_string())
    });
    match opened {
        Ok(source) => {
            for weakening in ai6::directorates::registry::sql::weakenings(&location) {
                eprintln!(
                    "universal-exports: warning: the configuration provider at {shown}: {weakening}"
                );
            }
            gateway = gateway.with_roster(source);
        }
        Err(error) => {
            eprintln!(
                "universal-exports: the configuration provider at {shown} cannot be opened: {error}"
            );
            return ExitCode::FAILURE;
        }
    }
    if !cli.no_assistant {
        match AssistantConfig::new(cli.assistant_agent.clone()) {
            Ok(assistant) => gateway = gateway.with_assistant(assistant),
            Err(error) => {
                eprintln!("universal-exports: the assistant Agent is not valid: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    match gateway
        .serve_loopback(listen.port(), shutdown_signal())
        .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("universal-exports: gateway listener failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn login_limits(cli: &Cli) -> Result<LoginLimits, String> {
    let address_header = cli
        .client_address_header
        .as_deref()
        .map(|header| {
            axum::http::HeaderName::from_bytes(header.as_bytes()).map_err(|_| {
                "AI6_EXPORTS_CLIENT_ADDRESS_HEADER is not a valid HTTP header name".to_owned()
            })
        })
        .transpose()?;
    Ok(LoginLimits {
        per_address: cli.login_tries_per_address,
        per_name: cli.login_tries_per_name,
        address_header,
    })
}

/// The provider library policy, chosen as a Station chooses its own: the pin set
/// `--plugin-pins` names, else `plugins.toml` in the provider's directory; verified copies
/// under the state directory's `plugins/verified`.
fn plugin_policy(
    cli: &Cli,
    settings: &Settings,
) -> Result<ai6::station::plugins::PluginPolicy, String> {
    let default_pins = settings
        .provider_dir
        .as_ref()
        .map(|dir| dir.join("plugins.toml"));
    ai6::station::plugins::PluginPolicy::load(
        cli.plugin_pins.as_deref(),
        default_pins.as_deref(),
        settings.located.value.plugins.profile,
        settings.state_dir.join("plugins").join("verified"),
    )
    .map_err(|error| format!("provider library pins: {error}"))
}

/// Seconds as a duration: a whole or fractional number, not negative.
fn seconds(text: &str) -> Result<Duration, String> {
    text.trim()
        .parse::<f64>()
        .ok()
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .ok_or_else(|| "expected a number of seconds, 0 or more".to_owned())
}

/// How the loaded Directory and Authorizer keep their answers; a lifetime past the default is
/// reported, since it widens the window in which a revocation is not yet seen.
fn access_cache(cli: &Cli) -> CacheOptions {
    let cache = CacheOptions::default()
        .ttl(cli.access_cache_ttl)
        .negative_ttl(cli.access_cache_negative_ttl)
        .max_entries(cli.access_cache_entries);
    if cache.exceeds_default() {
        eprintln!(
            "universal-exports: warning: access answers are kept {:?} ({:?} for a refusal),              longer than the default: a revoked account or grant passes that long",
            cache.ttl, cache.negative_ttl
        );
    }
    cache
}

fn configured_access(cli: &Cli, settings: &Settings) -> Result<Option<AccessProviders>, String> {
    let needs_directory = cli.authenticator.is_some()
        || cli.proxy_user_header.is_some()
        || cli.directory.is_some()
        || cli.access_policy.is_some()
        || cli.authorizer.is_some();
    if !needs_directory {
        return Ok(None);
    }
    let directory_id = cli.directory.as_deref().ok_or_else(|| {
        "AI6_EXPORTS_DIRECTORY is required when user access is configured".to_owned()
    })?;
    if cli.access_policy.is_some() && cli.authorizer.is_some() {
        return Err(
            "configure either AI6_EXPORTS_ACCESS_POLICY or AI6_EXPORTS_AUTHORIZER".to_owned(),
        );
    }

    let plugins = ai6::station::plugins::Plugins::scan_with(
        &settings.located.value.plugins.paths,
        &plugin_policy(cli, settings)?,
    );
    ai6::station::plugins::log_scan(plugins.scan_summary());
    let authenticator_config = cli
        .authenticator_config
        .as_deref()
        .map(load_provider_config)
        .transpose()?;
    let directory_config = cli
        .directory_config
        .as_deref()
        .map(load_provider_config)
        .transpose()?;
    let authorizer_config = cli
        .authorizer_config
        .as_deref()
        .map(load_provider_config)
        .transpose()?;
    let authenticator: Option<Arc<dyn Authenticator>> = cli
        .authenticator
        .as_deref()
        .map(|authenticator_id| {
            plugins
                .authenticator(authenticator_id, authenticator_config.as_ref())
                .map(Arc::from)
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    let cache = access_cache(cli);
    let directory: Arc<dyn Directory> = plugins
        .cached_directory(directory_id, directory_config.as_ref(), cache)
        .map_err(|error| error.to_string())?;
    let authorizer: Arc<dyn Authorizer> = if let Some(provider) = &cli.authorizer {
        plugins
            .cached_authorizer(provider, authorizer_config.as_ref(), cache)
            .map_err(|error| error.to_string())?
    } else if let Some(path) = &cli.access_policy {
        Arc::new(ai6::load_role_authorizer(path).map_err(|error| error.to_string())?)
    } else {
        Arc::new(RoleAuthorizer::default())
    };
    let access = match (authenticator, cli.proxy_user_header.as_deref()) {
        (Some(authenticator), Some(header)) => {
            let header = axum::http::HeaderName::from_bytes(header.as_bytes()).map_err(|_| {
                "AI6_EXPORTS_PROXY_USER_HEADER is not a valid HTTP header name".to_owned()
            })?;
            AccessProviders::new(authenticator, directory, authorizer)
                .with_proxy_identity(header, cli.proxy_lookup_by.clone())
                .with_proxy_secret(proxy_secret(settings)?)?
        }
        (Some(authenticator), None) => AccessProviders::new(authenticator, directory, authorizer),
        (None, Some(header)) => {
            let header = axum::http::HeaderName::from_bytes(header.as_bytes()).map_err(|_| {
                "AI6_EXPORTS_PROXY_USER_HEADER is not a valid HTTP header name".to_owned()
            })?;
            AccessProviders::from_reverse_proxy(
                directory,
                authorizer,
                header,
                cli.proxy_lookup_by.clone(),
            )
            .with_proxy_secret(proxy_secret(settings)?)?
        }
        (None, None) => {
            return Err(
                "configure AI6_EXPORTS_AUTHENTICATOR or AI6_EXPORTS_PROXY_USER_HEADER".to_owned(),
            );
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

fn load_provider_config(path: &Path) -> Result<Value, String> {
    let mut file = File::open(path).map_err(|_| {
        format!(
            "provider configuration {} could not be opened",
            path.display()
        )
    })?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_PROVIDER_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            format!(
                "provider configuration {} could not be read",
                path.display()
            )
        })?;
    if bytes.len() > MAX_PROVIDER_CONFIG_BYTES {
        return Err(format!(
            "provider configuration {} exceeds {MAX_PROVIDER_CONFIG_BYTES} bytes",
            path.display()
        ));
    }
    let source = std::str::from_utf8(&bytes).map_err(|error| {
        format!(
            "provider configuration {} is not UTF-8 at byte {}",
            path.display(),
            error.valid_up_to()
        )
    })?;
    let config: JsonValue = serde_json::from_str(source).map_err(|error| {
        format!(
            "provider configuration {} is invalid JSON at line {} column {}",
            path.display(),
            error.line(),
            error.column()
        )
    })?;
    if !config.is_object() {
        return Err(format!(
            "provider configuration {} must contain a JSON object",
            path.display()
        ));
    }
    json_to_provider_value(config, 0)
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
    fn the_gateway_checks_its_access_plugins_with_a_pin_set_like_a_station() {
        let directory = tempfile::tempdir().unwrap();
        let bootstrap = directory.path().join("unex.cfg");
        std::fs::write(&bootstrap, "state_dir = \"state\"\n").unwrap();
        let cli = |pins: &std::path::Path, profile: Option<&str>| {
            let mut arguments = vec![
                "universal-exports".to_owned(),
                "--config".to_owned(),
                bootstrap.display().to_string(),
                "--plugin-pins".to_owned(),
                pins.display().to_string(),
            ];
            if let Some(profile) = profile {
                arguments.extend(["--plugin-profile".to_owned(), profile.to_owned()]);
            }
            Cli::try_parse_from(arguments).unwrap()
        };
        let policy = |cli: &Cli| plugin_policy(cli, &settings(cli).unwrap());
        let missing = directory.path().join("plugins.toml");
        let error = policy(&cli(&missing, None)).unwrap_err();
        assert!(error.contains("does not exist"), "{error}");

        std::fs::write(&missing, "").unwrap();
        assert_eq!(
            policy(&cli(&missing, None)).unwrap().profile(),
            ai6::PluginProfile::Strict
        );
        assert_eq!(
            policy(&cli(&missing, Some("report"))).unwrap().profile(),
            ai6::PluginProfile::Report
        );
    }

    #[test]
    fn the_access_cache_defaults_to_seconds_and_reads_fractions_or_none() {
        let cli = |extra: &[&str]| {
            let mut arguments = vec!["universal-exports"];
            arguments.extend_from_slice(extra);
            Cli::try_parse_from(arguments).unwrap()
        };
        assert_eq!(access_cache(&cli(&[])), CacheOptions::default());
        let tuned = access_cache(&cli(&[
            "--access-cache-ttl",
            "0.5",
            "--access-cache-negative-ttl",
            "0",
            "--access-cache-entries",
            "10",
        ]));
        assert_eq!(
            (tuned.ttl, tuned.negative_ttl, tuned.max_entries),
            (Duration::from_millis(500), Duration::ZERO, 10)
        );
        assert!(access_cache(&cli(&["--access-cache-ttl", "60"])).exceeds_default());
        assert!(Cli::try_parse_from(["universal-exports", "--access-cache-ttl", "-1",]).is_err());
    }

    #[test]
    fn provider_config_requires_its_provider_id() {
        assert!(
            Cli::try_parse_from([
                "universal-exports",
                "--authenticator-config",
                "provider.json",
            ])
            .is_err()
        );
    }

    #[test]
    fn provider_config_file_converts_json_objects_to_guatiao_values() {
        use ai6::guatiao::FromValue;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider.json");
        std::fs::write(
            &path,
            r#"{"path":"accounts.toml","enabled":true,"nested":[1,true,null]}"#,
        )
        .unwrap();

        let value = load_provider_config(&path).unwrap();
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
    fn provider_config_file_rejects_non_object_roots_and_oversized_input() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider.json");
        std::fs::write(&path, "[]").unwrap();
        assert!(
            load_provider_config(&path)
                .unwrap_err()
                .contains("JSON object")
        );

        let oversized = " ".repeat(MAX_PROVIDER_CONFIG_BYTES + 1);
        std::fs::write(&path, oversized).unwrap();
        assert!(load_provider_config(&path).unwrap_err().contains("exceeds"));
    }

    #[test]
    fn provider_config_rejects_invalid_utf8_and_does_not_echo_malformed_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider.json");
        std::fs::write(&path, [0xff]).unwrap();
        assert!(
            load_provider_config(&path)
                .unwrap_err()
                .contains("not UTF-8")
        );

        std::fs::write(&path, r#"{"secret":"private-marker", invalid}"#).unwrap();
        let error = load_provider_config(&path).unwrap_err();
        assert!(error.contains("invalid JSON"));
        assert!(!error.contains("private-marker"));
    }

    #[test]
    fn provider_config_rejects_more_than_sixty_four_nested_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider.json");
        let arrays = "[".repeat(MAX_PROVIDER_CONFIG_DEPTH + 1);
        let closing = "]".repeat(MAX_PROVIDER_CONFIG_DEPTH + 1);
        std::fs::write(&path, format!("{{\"nested\":{arrays}null{closing}}}")).unwrap();

        assert!(
            load_provider_config(&path)
                .unwrap_err()
                .contains("64 nested values")
        );
    }
}
