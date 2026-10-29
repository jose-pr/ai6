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

const DEFAULT_PORT: u16 = 7082;
const STATION_TOKEN_ENV: &str = "AI6_EXPORTS_STATION_TOKEN";
const MAX_PROVIDER_CONFIG_BYTES: usize = 1024 * 1024;
const MAX_PROVIDER_CONFIG_DEPTH: usize = 64;

#[derive(Debug, Parser)]
#[command(
    name = "universal-exports",
    about = "Serve the authenticated AI6 Universal Exports gateway",
    after_help = "The Station channel credential is read only from AI6_EXPORTS_STATION_TOKEN. User access requires a Directory plus an Authenticator or a trusted reverse-proxy identity header."
)]
struct Cli {
    /// The public HTTPS origin used to validate browser requests.
    #[arg(long, env = "AI6_EXPORTS_PUBLIC_ORIGIN")]
    public_origin: String,
    /// The loopback port used behind the trusted HTTPS reverse proxy.
    #[arg(long, env = "AI6_EXPORTS_PORT", default_value_t = DEFAULT_PORT)]
    port: u16,
    /// A directory or library containing Guatiao providers; may be repeated.
    #[arg(long = "plugin-path", value_name = "PATH")]
    plugin_paths: Vec<PathBuf>,
    /// The provider library pin set, as a Station's (default: plugins.toml in the AI6
    /// configuration directory); a file named here must exist.
    #[arg(long, env = "AI6_EXPORTS_PLUGIN_PINS", value_name = "PATH")]
    plugin_pins: Option<PathBuf>,
    /// How provider libraries are checked: strict, report or off (default: the pin set's
    /// profile, else strict with a pin set and report without).
    #[arg(long, env = "AI6_EXPORTS_PLUGIN_PROFILE", value_name = "PROFILE")]
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
    /// The roster the Stations read (the location they take with `ai6 serve --config`): a
    /// configuration directory (read-only), `sqlite:PATH` or a `postgres://` URL, served for
    /// administration under /v1/user/roster. The password may come from AI6_CONFIG_PASSWORD.
    #[arg(long, env = "AI6_EXPORTS_ROSTER", value_name = "LOCATION")]
    roster: Option<String>,
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

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let station_token = match std::env::var(STATION_TOKEN_ENV) {
        Ok(token) => token,
        Err(_) => {
            eprintln!(
                "universal-exports: {STATION_TOKEN_ENV} must be set by the credential provider"
            );
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
        cli.public_origin.clone(),
        station_token,
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
    let registry = match ai6::directorates::registry::backend::from_environment() {
        Ok(registry) => {
            let location = std::env::var("AI6_REGISTRY").unwrap_or_default();
            for weakening in ai6::directorates::registry::sql::weakenings(&location) {
                eprintln!(
                    "universal-exports: warning: the Registry at {}: {weakening}",
                    ai6::directorates::registry::sql::redact(&location)
                );
            }
            registry
        }
        Err(error) => {
            eprintln!("universal-exports: shared access-token Registry is unavailable: {error}");
            return ExitCode::FAILURE;
        }
    };
    let access = match configured_access(&cli) {
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
    if let Some(location) = &cli.roster {
        let mut options = ai6::directorates::registry::sql::SqlOptions::default();
        if let Some(password) = std::env::var("AI6_CONFIG_PASSWORD")
            .ok()
            .filter(|password| !password.is_empty())
        {
            options = options.with_password(password);
        }
        let shown = ai6::directorates::registry::sql::redact(location);
        match ai6::directorates::m::config::open_with(location, &options) {
            Ok(source) => {
                for weakening in ai6::directorates::registry::sql::weakenings(location) {
                    eprintln!("universal-exports: warning: the roster at {shown}: {weakening}");
                }
                gateway = gateway.with_roster(source);
            }
            Err(error) => {
                eprintln!("universal-exports: the roster at {shown} cannot be opened: {error}");
                return ExitCode::FAILURE;
            }
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
    match gateway.serve_loopback(cli.port, shutdown_signal()).await {
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
/// `--plugin-pins` names, else `plugins.toml` in the AI6 configuration directory; verified
/// copies under the state directory's `plugins/verified`.
fn plugin_policy(cli: &Cli) -> Result<ai6::station::plugins::PluginPolicy, String> {
    let default_pins = ai6::directorates::m::roster::config_dir()
        .ok()
        .map(|dir| dir.join("plugins.toml"));
    let state = ai6::station::safehouse::state_dir().map_err(|error| error.to_string())?;
    ai6::station::plugins::PluginPolicy::load(
        cli.plugin_pins.as_deref(),
        default_pins.as_deref(),
        cli.plugin_profile,
        state.join("plugins").join("verified"),
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

fn configured_access(cli: &Cli) -> Result<Option<AccessProviders>, String> {
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

    let plugins =
        ai6::station::plugins::Plugins::scan_with(&cli.plugin_paths, &plugin_policy(cli)?);
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
                .with_proxy_secret(proxy_secret()?)?
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
            .with_proxy_secret(proxy_secret()?)?
        }
        (None, None) => {
            return Err(
                "configure AI6_EXPORTS_AUTHENTICATOR or AI6_EXPORTS_PROXY_USER_HEADER".to_owned(),
            );
        }
    };
    Ok(Some(access))
}

/// `AI6_EXPORTS_PROXY_SECRET`, which the reverse proxy sends in `X-AI6-Proxy-Secret`; read from
/// the environment only, so it never appears in a command line.
fn proxy_secret() -> Result<String, String> {
    std::env::var("AI6_EXPORTS_PROXY_SECRET").map_err(|_| {
        "AI6_EXPORTS_PROXY_USER_HEADER needs AI6_EXPORTS_PROXY_SECRET, the secret the reverse \
         proxy sends in X-AI6-Proxy-Secret, so a client reaching the gateway's port directly \
         cannot assert an identity"
            .to_owned()
    })
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
        let cli = |pins: &std::path::Path, profile: Option<&str>| {
            let mut arguments = vec![
                "universal-exports".to_owned(),
                "--public-origin".to_owned(),
                "https://localhost".to_owned(),
                "--plugin-pins".to_owned(),
                pins.display().to_string(),
            ];
            if let Some(profile) = profile {
                arguments.extend(["--plugin-profile".to_owned(), profile.to_owned()]);
            }
            Cli::try_parse_from(arguments).unwrap()
        };
        let missing = directory.path().join("plugins.toml");
        let error = plugin_policy(&cli(&missing, None)).unwrap_err();
        assert!(error.contains("does not exist"), "{error}");

        std::fs::write(&missing, "").unwrap();
        assert_eq!(
            plugin_policy(&cli(&missing, None)).unwrap().profile(),
            ai6::PluginProfile::Strict
        );
        assert_eq!(
            plugin_policy(&cli(&missing, Some("report")))
                .unwrap()
                .profile(),
            ai6::PluginProfile::Report
        );
    }

    #[test]
    fn the_access_cache_defaults_to_seconds_and_reads_fractions_or_none() {
        let cli = |extra: &[&str]| {
            let mut arguments = vec!["universal-exports", "--public-origin", "https://localhost"];
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
        assert!(
            Cli::try_parse_from([
                "universal-exports",
                "--public-origin",
                "https://localhost",
                "--access-cache-ttl",
                "-1",
            ])
            .is_err()
        );
    }

    #[test]
    fn provider_config_requires_its_provider_id() {
        assert!(
            Cli::try_parse_from([
                "universal-exports",
                "--public-origin",
                "https://localhost",
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
