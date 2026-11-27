//! Grok (`grok agent stdio`), measured on 1.0.77.
//!
//! `GROK_HOME` places `auth.json`, the sessions and the logs. A sign-in file that the harness cannot refresh is deleted
//! by it with exit code 0, which the watcher reports as a loss. What is not listed is not measured: the keys inside
//! `auth.json`, the defaults of the endpoints, and the `GROK_AUTH_PROVIDER_COMMAND` kind, which runs a program named in
//! settings and awaits the owner's answer.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use std::ffi::OsString;
use std::path::PathBuf;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::{
    ASK, AUTH, Credentials, LoginForm, LoginMethodInfo, LoginOutcome, LoginPrompter, LoginRequest,
    MODEL_API, OPEN_URL, OpenUrl, PROXY, ProcessSpawner, USER_CODE,
};
use coqui_acp_api::{Environment, SignedInCheck, SignedInState};

use coqui_acp_api::standard;

use crate::access::{Access, Approvals, Part, Sandbox, Target, Translation};
use crate::cli::address_in;
use crate::login::reason;
use crate::process::{Exit, Launch};
use crate::signed_in;
use crate::stdio::accounts::{
    AcceptDef, Credential, Endpoint, HarnessDef, KindDef, Setting, SlotDef,
};
use crate::stdio::login::{self, Ctx};
use crate::stdio::{StdioAccountSettings, StdioDef, StdioSettings};

/// The kind of a login made by `grok login`, kept in `auth.json`.
pub const LOGIN: &str = "grok-login";
/// The kind of a key in `XAI_API_KEY`.
pub const API_KEY: &str = "xai-api-key";
/// The kind of a login through the person's own identity provider.
pub const OIDC: &str = "grok-oidc";

const AUTH_FILE: &str = "auth.json";

/// The hosts Grok's sign-in pages are on; an address a command prints elsewhere is not shown to the person.
const SIGN_IN_HOSTS: &[&str] = &["accounts.x.ai", "auth.x.ai"];

const URL_SETTINGS: &[Setting] = &[
    Setting {
        name: "proxy_base_url",
        required: false,
        url: true,
        choices: &[],
        suggestions: &[],
        provides: &[],
    },
    Setting {
        name: "xai_api_base_url",
        required: false,
        url: true,
        choices: &[],
        suggestions: &[],
        provides: &[],
    },
];

const OIDC_SETTINGS: &[Setting] = &[
    Setting {
        name: "issuer",
        required: true,
        url: true,
        choices: &[],
        suggestions: &[],
        provides: &[],
    },
    Setting {
        name: "client_id",
        required: true,
        url: false,
        choices: &[],
        suggestions: &[],
        provides: &[],
    },
    Setting {
        name: "proxy_base_url",
        required: false,
        url: true,
        choices: &[],
        suggestions: &[],
        provides: &[],
    },
];

const PROVIDER_VARIABLES: &[(&str, &str)] = &[
    ("proxy_base_url", "GROK_CLI_CHAT_PROXY_BASE_URL"),
    ("xai_api_base_url", "GROK_XAI_API_BASE_URL"),
];

const API_ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        purpose: PROXY,
        key: Some("proxy_base_url"),
        default: None,
    },
    Endpoint {
        purpose: MODEL_API,
        key: Some("xai_api_base_url"),
        default: None,
    },
];

const OIDC_ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        purpose: AUTH,
        key: Some("issuer"),
        default: None,
    },
    Endpoint {
        purpose: PROXY,
        key: Some("proxy_base_url"),
        default: None,
    },
];

/// Every variable Grok reads for a credential, an identity provider or a program that makes a token, so a start on an
/// account removes them all.
const VARIABLES: &[&str] = &[
    "XAI_API_KEY",
    "GROK_CODE_XAI_API_KEY",
    "GROK_AUTH",
    "GROK_OIDC_ISSUER",
    "GROK_OIDC_CLIENT_ID",
    "GROK_AUTH_PROVIDER_COMMAND",
    "GROK_AUTH_PROVIDER_COMMAND_LABEL",
    "GROK_AUTH_TOKEN_TTL",
    "GROK_CLI_CHAT_PROXY_BASE_URL",
    "GROK_XAI_API_BASE_URL",
    "GROK_HOME",
    // Where the harness gets models and tokens, and what it reports to.
    "GROK_MODELS_BASE_URL",
    "GROK_MODELS_LIST_URL",
    "GROK_AUTH_PROVIDER_ACCESS_TOKEN",
    "GROK_AUTH_PROVIDER_REFRESH_TOKEN",
    "GROK_AUTH_PROVIDER_EXPIRES_AT",
    "GROK_DEPLOYMENT_KEY",
    "GROK_TELEMETRY_EVENTS_URL",
    "GROK_TELEMETRY_EVENTS_API_KEY",
    "GROK_TELEMETRY_BUILD_EVENTS_URL",
    "GROK_TELEMETRY_BUILD_EVENTS_API_KEY",
    "GROK_TELEMETRY_MIXPANEL_TOKEN",
    "GROK_TELEMETRY_BUILD_MIXPANEL_TOKEN",
    "GROK_WEB_FETCH_PROXY",
];

/// What Grok takes as an account.
pub static DEF: HarnessDef = HarnessDef {
    name: "Grok",
    program: "grok",
    args: &["agent", "stdio"],
    kinds: &[
        KindDef {
            id: LOGIN,
            provides: &[standard::GROK],
            title: "Grok login",
            description: "A login made by `grok login`, kept in `auth.json` of Grok's home directory.",
            credentials: &[Credential {
                key: "auth_json",
                required: true,
            }],
            settings: URL_SETTINGS,
            state_dir: true,
            rotates: true,
            history_portable: false,
            renews: true,
            endpoints: API_ENDPOINTS,
            notices: &["the login is kept in a file that what the agent runs can read"],
            credential_variables: &[],
            setting_variables: PROVIDER_VARIABLES,
            named_variable: None,
            home_variable: Some("GROK_HOME"),
            file: Some((AUTH_FILE, "auth_json")),
            renew_unsupported: "",
        },
        KindDef {
            id: API_KEY,
            provides: &[standard::XAI_API],
            title: "xAI API key",
            description: "A key read from `XAI_API_KEY`, with Grok started in a directory of its own so that no login in `auth.json` and no endpoint in the person's own `config.toml` overrides it.",
            credentials: &[Credential {
                key: "api_key",
                required: true,
            }],
            settings: URL_SETTINGS,
            state_dir: true,
            rotates: false,
            history_portable: false,
            renews: false,
            endpoints: API_ENDPOINTS,
            notices: &[
                "the credential is in the environment of every program the agent runs",
                "the account has a Grok home of its own, without the person's settings",
            ],
            credential_variables: &[("api_key", "XAI_API_KEY")],
            setting_variables: PROVIDER_VARIABLES,
            named_variable: None,
            home_variable: Some("GROK_HOME"),
            file: None,
            renew_unsupported: "a key is not renewed",
        },
        KindDef {
            id: OIDC,
            provides: &[standard::GROK],
            title: "Grok through an identity provider",
            description: "A login through the person's own OpenID Connect provider; Grok signs in on its first start and keeps the tokens in `auth.json`.",
            credentials: &[Credential {
                key: "auth_json",
                required: false,
            }],
            settings: OIDC_SETTINGS,
            state_dir: true,
            rotates: true,
            history_portable: false,
            renews: true,
            endpoints: OIDC_ENDPOINTS,
            notices: &["the login is kept in a file that what the agent runs can read"],
            credential_variables: &[],
            setting_variables: &[
                ("issuer", "GROK_OIDC_ISSUER"),
                ("client_id", "GROK_OIDC_CLIENT_ID"),
                ("proxy_base_url", "GROK_CLI_CHAT_PROXY_BASE_URL"),
            ],
            named_variable: None,
            home_variable: Some("GROK_HOME"),
            file: Some((AUTH_FILE, "auth_json")),
            renew_unsupported: "",
        },
    ],
    own_standard: Some(standard::GROK),
    accepts: &[
        AcceptDef {
            standard: standard::GROK,
            slots: &[SlotDef {
                credential: "auth_json",
                kind: LOGIN,
                variable: None,
                file: Some(AUTH_FILE),
                base_url_setting: None,
                base_url_variable: None,
                dir_variable: Some("GROK_HOME"),
                settings: &[],
            }],
        },
        AcceptDef {
            standard: standard::XAI_API,
            slots: &[SlotDef {
                credential: "api_key",
                kind: API_KEY,
                variable: Some("XAI_API_KEY"),
                file: None,
                base_url_setting: Some("xai_api_base_url"),
                base_url_variable: Some("GROK_XAI_API_BASE_URL"),
                dir_variable: Some("GROK_HOME"),
                settings: &[],
            }],
        },
    ],
    variables: VARIABLES,
    // `grok models` renews a login within `GROK_AUTH_EARLY_INVALIDATION_SECS` (300) of its end.
    renew_args: Some(&["models"]),

    login_unsupported: "",
};

/// The variables Grok signs in by itself with: a key of xAI's API.
const SIGN_IN_VARIABLES: &[&str] = &["XAI_API_KEY", "GROK_CODE_XAI_API_KEY"];

/// Whether Grok has a login: `auth.json` in its home (`GROK_HOME`) that holds something, or a key in one of
/// [`SIGN_IN_VARIABLES`]. Without `GROK_HOME` the home is assumed to be `.grok` below the user's home, which was not measured,
/// so a file missing there is unknown, not signed out.
fn signed_in(environment: &Environment, _wait: Duration) -> SignedInCheck {
    let named = signed_in::value(environment, "GROK_HOME").map(PathBuf::from);
    let assumed = named.is_none();
    let home = named.or_else(|| signed_in::home(environment).map(|home| home.join(".grok")));
    let path = home.map(|home| home.join(AUTH_FILE));
    let answer = signed_in::answer_for_file(
        environment,
        SIGN_IN_VARIABLES,
        path.as_deref(),
        "Grok's auth.json",
    );
    if assumed && answer.state == SignedInState::SignedOut {
        return SignedInCheck::unknown(
            "Grok's auth.json is not in the folder Grok is assumed to keep it in (.grok below the home), which was not measured, and GROK_HOME is not set",
        );
    }
    answer
}

/// How the stdio provider runs it.
pub static STDIO: StdioDef = StdioDef {
    accounts: &DEF,
    signed_in,
    accounts_description: "Signs in to Grok, renews and watches its logins, and says its accounts report no plan usage.",
    logins,
    sign_in: Some(sign_in),
    session_ids: None,
    import: None,
    export: None,
    import_unsupported: "this provider does not seed a session in an agent it does not keep the sessions of; set replay_max_bytes to replay the history as context in the first prompt",
    export_unsupported: "this provider does not read a session out of an agent it does not keep the sessions of",
};

crate::stdio_vendor! {
    /// Grok, as a provider of `acp_harness` under the id `grok`: `grok agent stdio`.
    harness: GrokProvider, id: "grok",
    /// Grok's accounts, as a provider of `account_provider` under the id `grok_accounts`.
    accounts: GrokAccounts, id: "grok_accounts",
    def: STDIO,
    settings: StdioSettings, account_settings: StdioAccountSettings,
}

/// Grok's access settings in its words, under the AOS harness id `grok`.
pub const TARGET: Target = Target::new("grok", translate);

const fn translate(access: Access) -> Translation {
    let sandbox = match access.sandbox {
        Sandbox::Workspace => Part::env(
            &[("GROK_SANDBOX", "workspace")],
            "Grok's sandbox profile workspace (GROK_SANDBOX)",
        ),
        Sandbox::ReadOnly => Part::env(
            &[("GROK_SANDBOX", "read-only")],
            "Grok's sandbox profile read-only (GROK_SANDBOX)",
        ),
        _ => Part::env(
            &[("GROK_SANDBOX", "off")],
            "Grok's sandbox profile off (GROK_SANDBOX)",
        ),
    };
    let network = match (access.network, access.sandbox) {
        (true, Sandbox::ReadOnly) => Part::cannot(
            "Grok's read-only profile blocks its commands' network, on Linux by its documentation",
        ),
        (true, _) => Part::said("the profile's own: its commands may use the network"),
        (false, Sandbox::ReadOnly) => Part::said(
            "Grok's read-only profile blocks its commands' network, on Linux by its documentation",
        ),
        (false, _) => Part::cannot(
            "Grok's built-in sandbox profiles have no network switch: a custom profile's restrict_network has",
        ),
    };
    let approvals = match access.approvals {
        Approvals::Never => Part::json(
            r#"{"yoloMode":true}"#,
            "Grok's always-approve for the session (_meta yoloMode)",
        ),
        Approvals::Edits => Part::cannot("Grok's agent mode has no accept-edits setting"),
        _ => Part::said("Grok asks before it runs a tool, as it does by default"),
    };
    Translation::new(sandbox, network, approvals)
}

/// The ways to sign in.
///
/// The identity-provider kind has no method: its sign-in needs a provider to answer and was not run.
#[must_use]
pub fn logins() -> Vec<LoginMethodInfo> {
    let mut device = LoginMethodInfo::new("device", "Sign in with a code", LOGIN);
    device.description =
        "`grok login`: open a page on any machine and confirm the code it shows.".to_string();
    device.steps = vec![OPEN_URL.to_string(), USER_CODE.to_string()];
    device.remote = true;
    let mut browser = LoginMethodInfo::new("browser", "Sign in with a browser", LOGIN);
    browser.description =
        "`grok login --oauth`: the browser returns to a port on the machine where Grok runs."
            .to_string();
    browser.steps = vec![OPEN_URL.to_string()];
    let mut key = LoginMethodInfo::new("api-key", "xAI API key", API_KEY);
    key.description = "Asks for the key and checks it with `grok models`.".to_string();
    key.steps = vec![ASK.to_string()];
    key.remote = true;
    vec![device, browser, key]
}

fn launch(program: &str, args: &[&str], dir: &str, extra: &[(&str, &str)]) -> Launch {
    let mut launch = Launch::new(program)
        .args(args.iter().copied())
        .env("GROK_HOME", dir);
    for (name, value) in extra {
        launch = launch.env(*name, *value);
    }
    launch.remove_env = VARIABLES.iter().map(OsString::from).collect();
    launch
}

#[derive(Default)]
struct Seen {
    url: Option<alloc::string::String>,
    code: Option<alloc::string::String>,
    after_confirm: bool,
    waiting: bool,
    opened: bool,
}

impl Seen {
    fn line(&mut self, ctx: &Ctx<'_>, line: &str, device: bool) -> Result<(), ProviderError> {
        let text = line.trim();
        if self.url.is_none()
            && let Some(url) = address_in(text, SIGN_IN_HOSTS)
        {
            self.url = Some(url);
        }
        if text.starts_with("Confirm this code") {
            self.after_confirm = true;
        } else if self.after_confirm && !text.is_empty() && self.code.is_none() {
            self.code = Some(text.to_string());
            self.after_confirm = false;
        }
        if text.starts_with("Waiting for authorization") {
            self.waiting = true;
        }
        let ready = self.url.is_some() && (!device || self.code.is_some() || self.waiting);
        if ready && !self.opened {
            self.opened = true;
            let mut open = OpenUrl::new(self.url.clone().unwrap_or_default());
            open.user_code.clone_from(&self.code);
            ctx.open_url(open)?;
        }
        Ok(())
    }
}

fn internal(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, why)
}

fn code_text(exit: &Exit) -> alloc::string::String {
    exit.code.map_or_else(
        || "was ended".to_string(),
        |code| alloc::format!("ended with code {code}"),
    )
}

/// `grok models` on the sign-in's directory, which asks nothing of a model and renews a login near its end.
fn check(ctx: &Ctx<'_>, program: &str, dir: &str) -> Result<bool, ProviderError> {
    let mut child = ctx.spawn(&launch(program, &["models"], dir, &[]))?;
    let exit = child.run(ctx, |_| Ok(()))?;
    Ok(exit.code == Some(0))
}

fn browser_or_device(
    ctx: &Ctx<'_>,
    program: &str,
    dir: &str,
    device: bool,
) -> Result<LoginOutcome, ProviderError> {
    let args: &[&str] = if device {
        &["login"]
    } else {
        &["login", "--oauth"]
    };
    let mut child = ctx.spawn(&launch(program, args, dir, &[]))?;
    let mut seen = Seen::default();
    let exit = child.run(ctx, |line| seen.line(ctx, line, device))?;
    if exit.code != Some(0) {
        return Err(internal(&alloc::format!(
            "`grok login` {}",
            code_text(&exit)
        )));
    }
    let text = ctx
        .read_settled(AUTH_FILE)
        .map_err(|_| internal("`grok login` wrote no login file"))?;
    let checked = check(ctx, program, dir)?;
    // `grok models` may have renewed the login: the newer copy is the one to keep.
    let text = ctx.read_settled(AUTH_FILE).unwrap_or(text);
    let mut outcome = LoginOutcome::new(LOGIN, login::one("auth_json", &text));
    outcome.checked = checked;
    Ok(outcome)
}

fn api_key(ctx: &Ctx<'_>, program: &str, dir: &str) -> Result<LoginOutcome, ProviderError> {
    let form = LoginForm::secret("xAI API key", "api_key");
    let answer = ctx.ask(form)?;
    let key = answer
        .secrets
        .expose()
        .get("api_key")
        .unwrap_or_default()
        .to_string();
    if key.trim().is_empty() || key.chars().any(char::is_control) {
        return Err(ProviderError::new(
            Status::GUATIAO_ERR_BAD_VALUE,
            "login refused: no usable key was given",
        ));
    }
    let mut child = ctx.spawn(&launch(
        program,
        &["models"],
        dir,
        &[("XAI_API_KEY", key.as_str())],
    ))?;
    let mut last = alloc::string::String::new();
    let exit = child.run(ctx, |line| {
        if !line.trim().is_empty() {
            last = line.to_string();
        }
        Ok(())
    })?;
    if exit.code != Some(0) {
        return Err(ProviderError::new(
            Status::GUATIAO_ERR_BAD_VALUE,
            &alloc::format!("login refused: {}", reason(&last, &[key.as_str()])),
        ));
    }
    let mut credentials = Credentials::new();
    credentials.insert("api_key", key);
    let mut outcome = LoginOutcome::new(API_KEY, credentials);
    outcome.checked = true;
    Ok(outcome)
}

/// Runs the sign-in `request.method` names.
///
/// # Errors
///
/// As [`AccountProvider::login`](coqui_acp_api::AccountProvider::login).
pub fn sign_in(
    program: alloc::string::String,
    request: LoginRequest,
    spawner: Object<dyn ProcessSpawner>,
    prompter: Object<dyn LoginPrompter>,
) -> Result<LoginOutcome, ProviderError> {
    let method = request.method.clone();
    if !matches!(method.as_str(), "device" | "browser" | "api-key") {
        return Err(ProviderError::new(
            Status::GUATIAO_ERR_BAD_VALUE,
            "this harness offers no such way to sign in",
        ));
    }
    let dir = request.state_dir.clone();
    let timeout = request.timeout_ms;
    login::run(spawner, prompter, timeout, move |ctx| {
        match method.as_str() {
            "device" => browser_or_device(ctx, &program, &dir, true),
            "browser" => browser_or_device(ctx, &program, &dir, false),
            _ => api_key(ctx, &program, &dir),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdio::accounts::{apply, catalog};
    use coqui_acp_api::standard;
    use coqui_acp_api::{AccountGrant, Credentials, HARNESS_LOGIN};
    use serde_json::json;

    const CANARY: &str = "sk-canary-3b7c";

    fn foreign(standards: &[&str], key: &str) -> AccountGrant {
        AccountGrant::new("a", "someone-elses-kind")
            .provides(standards.iter().copied())
            .credentials(Credentials::from_iter([(key, CANARY)]))
            .state_dir("/some/dir")
    }

    fn names(applied: &crate::stdio::accounts::Applied) -> Vec<String> {
        applied
            .environment
            .expose()
            .names()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn the_table_is_well_formed() {
        assert_eq!(TARGET.well_formed(), Ok(()));
    }

    #[test]
    fn the_standard_access_turns_the_sandbox_off_and_adds_no_metadata() {
        let standard = Access::STANDARD.translate(&TARGET);
        assert_eq!(standard.env(), vec![("GROK_SANDBOX", "off")]);
        assert_eq!(standard.json(), None);
    }

    #[test]
    fn the_sandbox_is_a_variable_and_never_approvals_are_session_metadata() {
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::Workspace;
        access.approvals = Approvals::Never;
        let grok = access.translate(&TARGET);
        assert_eq!(grok.env(), vec![("GROK_SANDBOX", "workspace")]);
        assert_eq!(
            grok.json().and_then(|json| json.get("yoloMode").cloned()),
            Some(serde_json::Value::Bool(true))
        );
        assert!(grok.args().is_empty());
    }

    #[test]
    fn what_is_applied_shows_names_and_no_value() {
        let grant = AccountGrant::new("a", "xai-api-key")
            .credentials(Credentials::from_iter([("api_key", "secret-value-1f3a")]))
            .state_dir("/some/dir");
        let applied = apply(&DEF, &grant).expect("it applies").expect("a kind");
        let shown = alloc::format!("{applied:?}");
        assert!(shown.contains("XAI_API_KEY"), "{shown}");
        assert!(!shown.contains("secret-value-1f3a"), "{shown}");
    }

    #[test]
    fn grok_lists_what_it_provides_and_accepts() {
        let catalog = catalog(&DEF);
        assert_eq!(
            catalog.kind(LOGIN).expect("login").provides,
            [standard::GROK]
        );
        assert_eq!(
            catalog.kind(API_KEY).expect("key").provides,
            [standard::XAI_API]
        );
        assert_eq!(
            catalog.kind(HARNESS_LOGIN).expect("own").provides,
            [standard::GROK]
        );
        let accepted: Vec<&str> = catalog
            .accepts
            .iter()
            .map(|a| a.standard.as_str())
            .collect();
        assert_eq!(accepted, [standard::GROK, standard::XAI_API]);
    }

    #[test]
    fn an_xai_api_key_from_elsewhere_is_applied_with_its_endpoint_in_a_directory_of_groks() {
        let grant = foreign(&[standard::XAI_API], "api_key").settings(crate::provider::schema(
            &json!({"base_url": "https://gateway.example/v1"}),
        ));
        let applied = apply(&DEF, &grant).expect("accepted").expect("a kind");
        let seen = names(&applied);
        for variable in ["XAI_API_KEY", "GROK_XAI_API_BASE_URL", "GROK_HOME"] {
            assert!(
                seen.contains(&variable.to_string()),
                "{variable} in {seen:?}"
            );
        }
        assert!(!alloc::format!("{applied:?}").contains(CANARY));
    }

    #[test]
    fn a_grok_login_from_elsewhere_is_the_file_it_watches() {
        let applied = apply(&DEF, &foreign(&[standard::GROK], "auth_json"))
            .expect("accepted")
            .expect("a kind");
        assert_eq!(applied.watched.expect("watched").path, "auth.json");
    }

    #[test]
    fn what_grok_does_not_accept_is_refused_with_its_list() {
        for standards in [&[standard::ANTHROPIC_API][..], &[][..]] {
            let grant = foreign(standards, "api_key");
            let Err(error) = apply(&DEF, &grant) else {
                panic!("{standards:?} was taken");
            };
            let said = error.message();
            assert!(said.contains("grok, xai-api"), "{said}");
            assert!(!said.contains(CANARY), "{said}");
            assert!(crate::stdio::accounts::known(&DEF, &grant.reference()).is_err());
        }
        // The right standard with a credential or an endpoint the slot does not take.
        assert!(apply(&DEF, &foreign(&[standard::XAI_API], "token")).is_err());
        assert!(
            apply(
                &DEF,
                &foreign(&[standard::GROK], "auth_json").settings(crate::provider::schema(
                    &json!({"base_url": "https://x.example"})
                ))
            )
            .is_err()
        );
    }

    /// An address setting is any length an address can be: a gateway's path or query is the person's own.
    #[test]
    fn an_address_setting_has_no_length_ceiling() {
        let long = format!("https://gateway.example/{}", "a".repeat(5000));
        let grant = AccountGrant::new("a", API_KEY)
            .settings(crate::provider::schema(&json!({"proxy_base_url": long})))
            .credentials(Credentials::from_iter([("api_key", CANARY)]))
            .state_dir("/some/dir");
        assert!(apply(&DEF, &grant).is_ok());
        let bad = AccountGrant::new("a", API_KEY)
            .settings(crate::provider::schema(
                &json!({"proxy_base_url": "gateway.example"}),
            ))
            .credentials(Credentials::from_iter([("api_key", CANARY)]))
            .state_dir("/some/dir");
        assert!(apply(&DEF, &bad).is_err());
    }
}
