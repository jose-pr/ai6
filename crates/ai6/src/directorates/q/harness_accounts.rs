//! Claude Code's and Codex's account kinds, as their account providers describe them, and how a kind is applied to a
//! Run's environment.
//!
//! What a kind reads (its variable, the directory it keeps a login in, the variables a start clears) and what applying
//! one does are the providers' (`coqui-acp-claude` and `coqui-acp-codex`, `accounts::apply`); AI6 keeps the pool, the
//! claim and where a credential comes from. A kind with no provider (a `harness-login`, or a `config-dir` of another
//! handler) is applied by AI6 alone.

// Only the Station applies an account to a Run (`daemon`); what the providers say of their kinds is read everywhere.
#![cfg_attr(not(feature = "daemon"), allow(dead_code))]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use codex::ApplyOptions;
use coqui_acp_api::{AccountCatalog, AccountGrant, Credentials, ProcessSpawner, UsageReport};
use coqui_acp_claude::{ClaudeLaunch, account_usage as claude_usage, accounts as claude};
use coqui_acp_codex::{CodexLaunch, account_usage as codex_usage, accounts as codex};
use coqui_acp_harness::LocalSpawner;
use coqui_acp_harness::account::Files;

use super::AccountKind;

/// A harness whose account provider describes accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Harness {
    Claude,
    Codex,
}

/// What one slot of a catalog says about a kind.
#[derive(Debug, Clone)]
struct Slot {
    kind: String,
    credential: String,
    variable: Option<String>,
    directory: Option<String>,
    file: bool,
}

/// What a harness's catalog says that AI6 reads.
struct Facts {
    slots: Vec<Slot>,
    /// Every variable a start clears.
    cleared: Vec<String>,
    /// The part of them that is itself a secret.
    secret: Vec<String>,
}

impl Facts {
    fn of(catalog: &AccountCatalog) -> Self {
        Self {
            slots: catalog
                .accepts
                .iter()
                .flat_map(|accepted| &accepted.slots)
                .map(|slot| Slot {
                    kind: slot.kind.clone(),
                    credential: slot.credential.clone(),
                    variable: slot.variable.clone(),
                    directory: slot.dir_variable.clone(),
                    file: slot.file.is_some(),
                })
                .collect(),
            cleared: catalog.credential_variables.clone(),
            secret: catalog.secret_variables.clone(),
        }
    }
}

static CLAUDE: LazyLock<Facts> = LazyLock::new(|| Facts::of(&claude::catalog()));
static CODEX: LazyLock<Facts> = LazyLock::new(|| Facts::of(&codex::catalog()));

impl Harness {
    /// The harness a pool's `handler` runs, when its account provider is one of these.
    pub(crate) fn named(handler: &str) -> Option<Self> {
        match handler {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    /// The handler name.
    pub(crate) const fn handler(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn facts(self) -> &'static Facts {
        match self {
            Self::Claude => &CLAUDE,
            Self::Codex => &CODEX,
        }
    }

    /// The kind of a login kept in a directory the harness runs on as it is.
    const fn login_kind(self) -> &'static str {
        match self {
            Self::Claude => claude::LOGIN,
            Self::Codex => codex::CHATGPT,
        }
    }
}

/// The id of `kind` in its provider's catalog; `None` for a kind no provider names.
pub(crate) const fn provider_id(kind: AccountKind) -> Option<&'static str> {
    match kind {
        AccountKind::ClaudeOauthToken => Some(claude::OAUTH_TOKEN),
        AccountKind::AnthropicApiKey => Some(claude::API_KEY),
        AccountKind::AnthropicAuthToken => Some(claude::AUTH_TOKEN),
        AccountKind::CodexApiKey => Some(codex::API_KEY),
        AccountKind::CodexAccessToken => Some(codex::ACCESS_TOKEN),
        AccountKind::ConfigDir | AccountKind::HarnessLogin => None,
    }
}

fn slot_of(kind: AccountKind) -> Option<(Harness, &'static Slot)> {
    let id = provider_id(kind)?;
    [Harness::Claude, Harness::Codex]
        .into_iter()
        .find_map(|harness| {
            harness
                .facts()
                .slots
                .iter()
                .find(|slot| slot.kind == id)
                .map(|slot| (harness, slot))
        })
}

/// The harness whose provider lists `kind`.
pub(crate) fn owner(kind: AccountKind) -> Option<Harness> {
    slot_of(kind).map(|(harness, _)| harness)
}

/// The variable the credential of `kind` is read from; `None` for a kind that hands over a directory.
pub(crate) fn variable(kind: AccountKind) -> Option<String> {
    slot_of(kind).and_then(|(_, slot)| slot.variable.clone())
}

/// The variable that names the directory `kind` is logged in to, or `handler`'s directory for a `config-dir`.
pub(crate) fn directory_variable(kind: AccountKind, handler: &str) -> Option<String> {
    if kind == AccountKind::ConfigDir {
        let harness = Harness::named(handler)?;
        return harness
            .facts()
            .slots
            .iter()
            .find(|slot| slot.kind == harness.login_kind())
            .and_then(|slot| slot.directory.clone());
    }
    slot_of(kind).and_then(|(_, slot)| slot.directory.clone())
}

/// Whether `kind` is saved by the harness's own login command into a directory made for the Mission: a kind whose
/// credential is neither a variable nor a file the account provider places.
pub(crate) fn logs_in_to_a_home(kind: AccountKind) -> bool {
    slot_of(kind)
        .is_some_and(|(_, slot)| slot.directory.is_some() && slot.variable.is_none() && !slot.file)
}

/// The variables that carry a credential to Claude Code or Codex: each provider's secret variables, and the
/// directories their logins are kept in. An endpoint, a cloud-provider switch or a region is not among them.
pub static CREDENTIAL_VARIABLES: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut names: Vec<String> = [Harness::Claude, Harness::Codex]
        .into_iter()
        .flat_map(|harness| {
            let facts = harness.facts();
            facts
                .secret
                .iter()
                .cloned()
                .chain(facts.slots.iter().filter_map(|slot| slot.directory.clone()))
        })
        .collect();
    names.sort();
    names.dedup();
    names
});

/// What a start on an account clears that is not a secret: endpoints, cloud-provider switches, regions, profiles. A
/// person's own choice of these is theirs, so it is put back after the provider cleared it.
fn kept_variables() -> Vec<&'static String> {
    [Harness::Claude, Harness::Codex]
        .into_iter()
        .flat_map(|harness| {
            let facts = harness.facts();
            facts
                .cleared
                .iter()
                .filter(|name| !facts.secret.contains(name))
        })
        .collect()
}

fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn no_files() -> Files {
    Files::new(Arc::new(LocalSpawner::new().into_object()))
}

fn files_in(dir: &Path) -> Files {
    Files::new(Arc::new(LocalSpawner::new().files(dir).into_object()))
}

fn id_of(pool: &str, account: &str) -> String {
    format!("{pool}/{account}")
}

/// Applies `grant` through `harness`'s provider to `environment`, which is the whole environment of the Run: what the
/// provider clears is gone from it, what the Run's own settings put there that is not a secret (an endpoint, a
/// cloud-provider switch) is put back, and what the kind sets is in it.
fn through(
    harness: Harness,
    grant: &AccountGrant,
    files: &Files,
    options: &ApplyOptions,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    // Every credential variable of both harnesses goes, so the Run sees one account.
    environment.retain(|name, _| {
        !CREDENTIAL_VARIABLES
            .iter()
            .any(|variable| same_name(name, variable))
    });
    let kept = kept_variables();
    let before: Vec<(String, String)> = environment
        .iter()
        .filter(|(name, _)| kept.iter().any(|kept| same_name(name, kept)))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let as_os = |environment: &BTreeMap<String, String>| -> BTreeMap<OsString, OsString> {
        environment
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    };
    let after = match harness {
        Harness::Claude => {
            let mut launch = ClaudeLaunch::new(".");
            launch.env = as_os(environment);
            let applied =
                claude::apply(grant, &mut launch, files).map_err(|e| e.message().to_owned())?;
            drop(applied);
            launch.env
        }
        Harness::Codex => {
            let mut launch = CodexLaunch::new(".");
            launch.env = as_os(environment);
            let applied = codex::apply_with(grant, &mut launch, files, options)
                .map_err(|e| e.message().to_owned())?;
            drop(applied);
            launch.env
        }
    };
    *environment = after
        .into_iter()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    for (name, value) in before {
        environment.entry(name).or_insert(value);
    }
    Ok(())
}

/// Sets `kind`'s credential in `environment` as its provider does, with every other credential variable of the
/// harness gone. `kind` is one of the environment kinds.
///
/// # Errors
///
/// Why the provider refused the account, which names no credential.
pub(crate) fn apply_secret(
    pool: &str,
    account: &str,
    kind: AccountKind,
    secret: &str,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let (harness, slot) =
        slot_of(kind).ok_or_else(|| format!("a {kind} account has no account provider"))?;
    let id = provider_id(kind).unwrap_or_default();
    let credentials: Credentials = [(slot.credential.as_str(), secret)].into_iter().collect();
    let grant = AccountGrant::new(id_of(pool, account), id).credentials(credentials);
    through(
        harness,
        &grant,
        &no_files(),
        &ApplyOptions::default(),
        environment,
    )
}

/// The plan limits of the account `account` of `pool`, read by its harness's account provider with the credential
/// the account hands a Run, which `environment` holds as `hand_over` left it: a request to the provider's own
/// endpoint (Claude's OAuth API, the ChatGPT backend), no program started and no model turn. `endpoint` replaces
/// that address, for a test's stand-in.
///
/// `None` for an account the provider cannot be asked from here (its kind has no provider, or a Codex key or token
/// saved to a home the harness keeps, which the harness itself reads): the caller asks the handler. An account with
/// no plan limits is a report that says so, not an error; the message of an error names no credential.
pub(crate) fn read_limits(
    handler: &str,
    pool: &str,
    account: &str,
    kind: AccountKind,
    environment: &BTreeMap<String, String>,
    endpoint: Option<&str>,
) -> Option<Result<UsageReport, String>> {
    if !reads_limits(handler, kind) {
        return None;
    }
    let harness = Harness::named(handler)?;
    let from = |name: &str| {
        environment
            .iter()
            .find(|(variable, _)| same_name(variable, name))
            .map(|(_, value)| value.as_str())
    };
    let id = id_of(pool, account);
    let grant = match slot_of(kind) {
        Some((_, slot)) if slot.variable.is_some() => {
            let secret = from(slot.variable.as_deref()?)?;
            let credentials: Credentials =
                [(slot.credential.as_str(), secret)].into_iter().collect();
            AccountGrant::new(id, provider_id(kind)?).credentials(credentials)
        }
        // A key or token the harness logs in to a home of its own.
        Some(_) => return None,
        None => {
            let dir = Path::new(from(&directory_variable(AccountKind::ConfigDir, handler)?)?);
            let (file, key) = match harness {
                Harness::Claude => (claude::CREDENTIALS_FILE, claude::FILE_KEY),
                Harness::Codex => (codex::AUTH_FILE, codex::FILE_KEY),
            };
            let Ok(text) = std::fs::read_to_string(dir.join(file)) else {
                return Some(Err(format!(
                    "the account's directory holds no readable {file}"
                )));
            };
            let credentials: Credentials = [(key, text.as_str())].into_iter().collect();
            AccountGrant::new(id, harness.login_kind())
                .credentials(credentials)
                .state_dir(dir.to_string_lossy())
        }
    };
    let read = match harness {
        Harness::Claude => {
            let mut options = claude_usage::ReadOptions::new();
            if let Some(endpoint) = endpoint {
                options = options.endpoint(endpoint);
            }
            claude_usage::read_limits(&grant, &options)
        }
        Harness::Codex => {
            let mut options = codex_usage::ReadOptions::new();
            if let Some(endpoint) = endpoint {
                options = options.endpoint(endpoint);
            }
            codex_usage::read_limits(&grant, &options)
        }
    };
    Some(read.map_err(|error| error.message().to_owned()))
}

/// Whether the account provider of `handler`'s harness may read an account of `kind`: the pool is served by Claude
/// Code or Codex and the kind is one of that harness's, so [`read_limits`] can answer.
pub(crate) fn reads_limits(handler: &str, kind: AccountKind) -> bool {
    let named = Harness::named(handler);
    match kind {
        AccountKind::ConfigDir | AccountKind::HarnessLogin => named.is_some(),
        other => named.is_some() && owner(other) == named,
    }
}

/// Runs the harness of `handler` on the directory `dir`, a login it already holds, as it is: nothing is written there.
/// Every credential variable of the harness is gone from `environment`.
///
/// # Errors
///
/// Why the provider refused the account, or that the handler has no provider.
pub(crate) fn apply_directory(
    handler: &str,
    pool: &str,
    account: &str,
    dir: &Path,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let harness = Harness::named(handler)
        .ok_or_else(|| format!("handler {handler:?} has no account provider"))?;
    let grant = AccountGrant::new(id_of(pool, account), harness.login_kind())
        .state_dir(dir.to_string_lossy());
    through(
        harness,
        &grant,
        &no_files(),
        &ApplyOptions::default(),
        environment,
    )
}

/// Saves `credential`, a Codex key or token, in the home `home` with `program login`, which takes it on standard input;
/// `program` is the program and its leading arguments. Codex's provider runs it, and its refusal says why without the
/// credential.
///
/// # Errors
///
/// Why Codex did not save it.
pub(crate) fn log_in_to_home(
    program: &[String],
    pool: &str,
    account: &str,
    kind: AccountKind,
    home: &Path,
    credential: &str,
    timeout: Duration,
) -> Result<(), String> {
    let (_, slot) = slot_of(kind)
        .filter(|(harness, slot)| {
            *harness == Harness::Codex && slot.variable.is_none() && !slot.file
        })
        .ok_or_else(|| format!("a {kind} account is not logged in"))?;
    let (executable, leading) = program
        .split_first()
        .ok_or("the pool has no login command")?;
    let id = provider_id(kind).unwrap_or_default();
    let credentials: Credentials = [(slot.credential.as_str(), credential)]
        .into_iter()
        .collect();
    let grant = AccountGrant::new(id_of(pool, account), id)
        .credentials(credentials)
        .state_dir(home.to_string_lossy());
    let mut launch = CodexLaunch::new(home);
    launch.executable = OsString::from(executable);
    let options = ApplyOptions::new()
        .timeout(timeout)
        .leading(leading.iter().map(OsString::from));
    let applied = codex::apply_with(&grant, &mut launch, &files_in(home), &options)
        .map_err(|error| format!("{executable} login failed: {}", error.message()))?;
    drop(applied);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANARY: &str = "sk-canary-5d2f1c77e0";

    fn environment(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn example(name: &str) -> String {
        let tests = std::env::current_exe().expect("the test knows where it is");
        let profile = tests
            .parent()
            .and_then(Path::parent)
            .expect("a test binary is in target/<profile>/deps");
        let program = profile
            .join("examples")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        assert!(
            program.exists(),
            "build the stand-in first: cargo build -p ai6 --example {name}"
        );
        program.to_string_lossy().into_owned()
    }

    #[test]
    fn the_credential_variables_are_the_providers_secrets_and_their_directories_and_no_endpoint() {
        let names: Vec<&str> = CREDENTIAL_VARIABLES.iter().map(String::as_str).collect();
        for credential in [
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "OPENAI_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "CODEX_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
        ] {
            assert!(names.contains(&credential), "{credential}");
        }
        for allowed in [
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_BEDROCK_BASE_URL",
            "ANTHROPIC_VERTEX_BASE_URL",
            "OPENAI_BASE_URL",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLOUD_ML_REGION",
            "AWS_PROFILE",
            "CODEX_SQLITE_HOME",
        ] {
            assert!(
                !names.contains(&allowed),
                "{allowed} is a choice a person makes, not a credential"
            );
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "each once");
    }

    #[test]
    fn each_kind_with_a_provider_is_spelled_as_the_provider_does_and_belongs_to_its_harness() {
        for (kind, harness) in [
            (AccountKind::ClaudeOauthToken, Harness::Claude),
            (AccountKind::AnthropicApiKey, Harness::Claude),
            (AccountKind::AnthropicAuthToken, Harness::Claude),
            (AccountKind::CodexApiKey, Harness::Codex),
            (AccountKind::CodexAccessToken, Harness::Codex),
        ] {
            let id = provider_id(kind).expect("a provider kind");
            assert_eq!(kind.as_str(), id);
            assert_eq!(owner(kind), Some(harness), "{kind}");
            assert!(harness.facts().slots.iter().any(|slot| slot.kind == id));
        }
        assert_eq!(owner(AccountKind::ConfigDir), None);
        assert_eq!(owner(AccountKind::HarnessLogin), None);
        assert_eq!(AccountKind::HarnessLogin.as_str(), "harness-login");
        assert_eq!(
            AccountKind::parse("openai-api-key"),
            None,
            "no alias is kept"
        );
        assert!(logs_in_to_a_home(AccountKind::CodexApiKey));
        assert!(logs_in_to_a_home(AccountKind::CodexAccessToken));
        assert!(!logs_in_to_a_home(AccountKind::ClaudeOauthToken));
        assert!(!logs_in_to_a_home(AccountKind::ConfigDir));
    }

    #[test]
    fn an_environment_kind_leaves_one_credential_and_keeps_the_agents_own_endpoint() {
        let mut env = environment(&[
            ("PATH", "/bin"),
            ("ANTHROPIC_BASE_URL", "https://gateway.example"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("AWS_PROFILE", "team"),
            ("ANTHROPIC_AUTH_TOKEN", "stray"),
            ("CODEX_API_KEY", "stray"),
        ]);
        apply_secret(
            "team",
            "alpha",
            AccountKind::AnthropicApiKey,
            CANARY,
            &mut env,
        )
        .expect("applied");
        assert_eq!(env["ANTHROPIC_API_KEY"], CANARY);
        assert_eq!(env["PATH"], "/bin");
        assert_eq!(env["ANTHROPIC_BASE_URL"], "https://gateway.example");
        assert_eq!(env["CLAUDE_CODE_USE_BEDROCK"], "1");
        assert_eq!(env["AWS_PROFILE"], "team");
        assert!(!env.contains_key("ANTHROPIC_AUTH_TOKEN"), "{env:?}");
        assert!(!env.contains_key("CODEX_API_KEY"), "{env:?}");
        let mut other = BTreeMap::new();
        apply_secret(
            "team",
            "alpha",
            AccountKind::ClaudeOauthToken,
            CANARY,
            &mut other,
        )
        .expect("applied");
        assert_eq!(other["CLAUDE_CODE_OAUTH_TOKEN"], CANARY);
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn a_credential_the_provider_refuses_is_refused_without_being_shown() {
        let mut env = BTreeMap::new();
        let error = apply_secret("team", "alpha", AccountKind::AnthropicApiKey, "", &mut env)
            .expect_err("an empty key");
        assert!(!error.contains(CANARY));
        assert!(env.is_empty(), "{env:?}");
        let error = apply_secret("team", "alpha", AccountKind::ConfigDir, CANARY, &mut env)
            .expect_err("no provider names it");
        assert!(!error.contains(CANARY));
    }

    #[test]
    fn a_login_directory_is_used_as_it_is_and_nothing_is_written_in_it() {
        for (handler, variable) in [("claude", "CLAUDE_CONFIG_DIR"), ("codex", "CODEX_HOME")] {
            let dir = tempfile::tempdir().unwrap();
            let mut env = environment(&[
                ("CLAUDE_CODE_OAUTH_TOKEN", "stray"),
                ("OPENAI_API_KEY", "stray"),
                ("CLAUDE_CONFIG_DIR", "/stray"),
                ("ANTHROPIC_BASE_URL", "https://gateway.example"),
            ]);
            apply_directory(handler, "team", "bob", dir.path(), &mut env).expect("applied");
            assert_eq!(env[variable], dir.path().to_string_lossy());
            assert!(!env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"), "{env:?}");
            assert!(!env.contains_key("OPENAI_API_KEY"), "{env:?}");
            assert_eq!(env["ANTHROPIC_BASE_URL"], "https://gateway.example");
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                0,
                "the directory of a login is left as it is"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let error = apply_directory("grok", "team", "bob", dir.path(), &mut BTreeMap::new())
            .expect_err("no provider");
        assert!(error.contains("grok"), "{error}");
    }

    #[test]
    fn a_codex_key_is_saved_by_the_login_command_on_standard_input_and_not_in_any_argument() {
        let home = tempfile::tempdir().unwrap();
        let program = [example("fake_codex_login")];
        log_in_to_home(
            &program,
            "team",
            "carol",
            AccountKind::CodexApiKey,
            home.path(),
            CANARY,
            Duration::from_secs(30),
        )
        .expect("logged in");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.path().join("auth.json")).unwrap())
                .unwrap();
        assert_eq!(saved["OPENAI_API_KEY"], CANARY);
        assert!(
            home.path().join("config.toml").exists(),
            "the login stays in the home"
        );
        let log = std::fs::read_to_string(home.path().join("fake-codex.log")).unwrap();
        assert!(
            !log.contains(CANARY),
            "the key is in no argument or variable"
        );
        let entry: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
        assert_eq!(
            entry["args"],
            serde_json::json!(["login", "--with-api-key"])
        );
        for variable in ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"] {
            assert!(
                !entry["env_names"].to_string().contains(variable),
                "{variable} is cleared from the command"
            );
        }
        let token = tempfile::tempdir().unwrap();
        log_in_to_home(
            &program,
            "team",
            "dave",
            AccountKind::CodexAccessToken,
            token.path(),
            CANARY,
            Duration::from_secs(30),
        )
        .expect("logged in");
        let log = std::fs::read_to_string(token.path().join("fake-codex.log")).unwrap();
        assert!(log.contains("--with-access-token"), "{log}");
    }

    #[test]
    fn a_credential_codex_refuses_and_a_kind_that_is_not_logged_in_are_refused_without_the_credential()
     {
        let home = tempfile::tempdir().unwrap();
        let program = [example("fake_codex_login")];
        let error = log_in_to_home(
            &program,
            "team",
            "carol",
            AccountKind::CodexApiKey,
            home.path(),
            "refused-canary-0042",
            Duration::from_secs(30),
        )
        .expect_err("refused");
        assert!(error.contains("login"), "{error}");
        assert!(!error.contains("refused-canary-0042"), "{error}");
        let error = log_in_to_home(
            &program,
            "team",
            "alice",
            AccountKind::ClaudeOauthToken,
            home.path(),
            CANARY,
            Duration::from_secs(30),
        )
        .expect_err("not a Codex login");
        assert!(error.contains("not logged in"), "{error}");
        let error = log_in_to_home(
            &[],
            "team",
            "carol",
            AccountKind::CodexApiKey,
            home.path(),
            CANARY,
            Duration::from_secs(30),
        )
        .expect_err("no program");
        assert!(!error.contains(CANARY), "{error}");
    }

    /// A server on a loopback port that answers one request with `status` and `body`, and says what it was
    /// asked: the request's lines.
    fn answers_once(
        status: u16,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let served = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end().to_owned();
                if line.is_empty() {
                    break;
                }
                lines.push(line);
            }
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            lines
        });
        (address, served)
    }

    #[test]
    fn a_claude_token_s_plan_limits_are_read_by_its_account_provider_with_its_own_credential() {
        let (endpoint, served) = answers_once(
            200,
            r#"{"five_hour": {"utilization": 42.5, "resets_at": "2026-11-19T12:00:00Z"}}"#,
        );
        let env = environment(&[("CLAUDE_CODE_OAUTH_TOKEN", CANARY), ("HOME", "/h")]);
        let report = read_limits(
            "claude",
            "team",
            "alice",
            AccountKind::ClaudeOauthToken,
            &env,
            Some(&endpoint),
        )
        .expect("the provider reads this kind")
        .expect("and it answered");
        assert!(report.supported);
        let window = report
            .buckets
            .iter()
            .find(|bucket| bucket.id == "five_hour")
            .expect("the window the document names");
        assert_eq!(UsageReport::fraction_permille(window), Some(425));
        assert_eq!(report.account.as_deref(), Some("team/alice"));
        let asked = served.join().unwrap();
        assert!(asked[0].starts_with("GET /api/oauth/usage"), "{asked:?}");
        assert!(
            asked
                .iter()
                .any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {CANARY}"))),
            "the credential goes to the provider's own endpoint: {asked:?}"
        );
        assert!(
            !format!("{report:?}").contains(CANARY),
            "no credential in the report"
        );
    }

    #[test]
    fn a_refused_token_is_an_error_that_names_no_credential() {
        let (endpoint, served) = answers_once(401, "{}");
        let env = environment(&[("CLAUDE_CODE_OAUTH_TOKEN", CANARY)]);
        let error = read_limits(
            "claude",
            "team",
            "alice",
            AccountKind::ClaudeOauthToken,
            &env,
            Some(&endpoint),
        )
        .unwrap()
        .expect_err("a 401 is a refusal");
        served.join().unwrap();
        assert!(error.contains("renew"), "{error}");
        assert!(!error.contains(CANARY), "{error}");
    }

    #[test]
    fn a_login_in_a_directory_is_read_from_its_file_and_one_without_the_profile_scope_has_no_limits()
     {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".credentials.json"),
            r#"{"claudeAiOauth": {"accessToken": "login-canary-77", "scopes": ["user:inference"], "expiresAt": 99999999999999}}"#,
        )
        .unwrap();
        let env = environment(&[("CLAUDE_CONFIG_DIR", dir.path().to_str().unwrap())]);
        let report = read_limits("claude", "team", "bob", AccountKind::ConfigDir, &env, None)
            .expect("a login in a directory")
            .expect("the provider answered with no request");
        assert!(!report.supported, "no network was needed to say so");
        assert!(
            report
                .unsupported_because
                .as_deref()
                .is_some_and(|why| why.contains("profile scope")),
            "{report:?}"
        );
        assert!(!format!("{report:?}").contains("login-canary-77"));
        let empty = tempfile::tempdir().unwrap();
        let env = environment(&[("CLAUDE_CONFIG_DIR", empty.path().to_str().unwrap())]);
        let error = read_limits("claude", "team", "bob", AccountKind::ConfigDir, &env, None)
            .unwrap()
            .expect_err("no file");
        assert!(error.contains(".credentials.json"), "{error}");
    }

    #[test]
    fn only_what_the_account_providers_can_read_is_asked_of_them() {
        let env = environment(&[("CODEX_HOME", "/somewhere")]);
        assert!(reads_limits("claude", AccountKind::ClaudeOauthToken));
        assert!(reads_limits("claude", AccountKind::ConfigDir));
        assert!(reads_limits("codex", AccountKind::CodexApiKey));
        assert!(!reads_limits("acp", AccountKind::ConfigDir));
        assert!(
            !reads_limits("mock-pooled", AccountKind::ClaudeOauthToken),
            "a key is read only for the harness it belongs to, whatever the pool is called"
        );
        assert!(!reads_limits("codex", AccountKind::ClaudeOauthToken));
        assert!(!reads_limits("antigravity", AccountKind::HarnessLogin));
        // A Codex key saved to a home the harness keeps is read by the harness.
        assert!(
            read_limits(
                "codex",
                "team",
                "carol",
                AccountKind::CodexApiKey,
                &env,
                None
            )
            .is_none()
        );
        assert!(read_limits("acp", "team", "dan", AccountKind::ConfigDir, &env, None).is_none());
        // A credential the environment does not hold is no read either.
        assert!(
            read_limits(
                "claude",
                "team",
                "eve",
                AccountKind::AnthropicApiKey,
                &environment(&[]),
                None
            )
            .is_none()
        );
    }
}
