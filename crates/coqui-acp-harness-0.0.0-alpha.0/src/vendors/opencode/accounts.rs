//! OpenCode (`opencode acp`), measured on 1.19.6 (Windows arm64, Linux arm64 and macOS x86_64) and 2.3.2 (Windows arm64).
//!
//! OpenCode keeps what it signs in with and every session in its data folder, `$XDG_DATA_HOME/opencode` (`auth.json`,
//! `opencode.db`), and honours `XDG_DATA_HOME` on every system measured (Windows included, where it ignores
//! `%APPDATA%`). A provider's key is read from that provider's own environment variable, and an inline
//! `OPENCODE_AUTH_CONTENT` replaces the whole of `auth.json` for a run. An account starts OpenCode with a data folder of
//! its own, so no credential in the person's `auth.json` meets the account's, and its sessions are the account's.
//!
//! Not measured: a completed sign-in (`opencode auth login` was run to its prompts, on plain pipes, and a made-up key typed once), the fields of an OAuth entry in
//! `auth.json`, whether it rewrites the file when it refreshes one, and every provider variable beyond
//! the ones listed. A provider the catalog adds is not in the list a start clears.

use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use std::ffi::OsString;
use std::path::PathBuf;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::schema::{FieldBuilder, KindBuilder, SchemaBuilder};
use coqui_acp_api::{
    ASK, Credentials, Environment, LoginForm, LoginMethodInfo, LoginOutcome, LoginPrompter,
    LoginRequest, OPEN_URL, OpenUrl, ProcessSpawner, SignedInCheck, USER_CODE,
};
use serde_json::json;

use coqui_acp_api::standard;

use super::provider::{export_session, import_session};
use crate::access::{Access, Approvals, Part, Sandbox, Target, Translation};
use crate::login::reason;
use crate::process::Launch;
use crate::provider::schema;
use crate::signed_in;
use crate::stdio::accounts::{
    AcceptDef, Credential, HarnessDef, KindDef, Setting, SlotDef, check_variable_name,
};
use crate::stdio::login::{self, Ctx};
use crate::stdio::{SessionIds, StdioDef};

/// The kind of a sign-in kept in `auth.json`.
pub const LOGIN: &str = "opencode-login";
/// The kind a key of another provider's account becomes: an API standard OpenCode has a variable for, read from it.
pub const API_KEY: &str = "opencode-api-key";
/// The kind a sign-in method makes: a key for one provider, read from the variable the person chose.
pub const PROVIDER_KEY: &str = "opencode-provider-key";

/// The file the sign-in is kept in, in the folder `XDG_DATA_HOME` names.
const AUTH_FILE: &str = "opencode/auth.json";

/// The variables a sign-in form suggests: each one seen to make OpenCode list a provider as signed in by the environment.
/// A key account may name any other variable (a provider the catalog adds, or one the person's own `opencode.json` reads).
pub const KEY_VARIABLES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_GENERATIVE_AI_API_KEY",
    "XAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "DEEPSEEK_API_KEY",
    "OPENCODE_API_KEY",
    "TOGETHER_API_KEY",
    "CEREBRAS_API_KEY",
    "FIREWORKS_API_KEY",
    "HF_TOKEN",
];

/// Every variable OpenCode reads for a credential or an identity, or that moves where it keeps them, so a start on an
/// account removes them all. Not here, on purpose: `OPENCODE_PERMISSION`, which is the host's access for the session and
/// holds no credential.
const VARIABLES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_GENERATIVE_AI_API_KEY",
    "XAI_API_KEY",
    "OPENROUTER_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "DEEPSEEK_API_KEY",
    "OPENCODE_API_KEY",
    "TOGETHER_API_KEY",
    "CEREBRAS_API_KEY",
    "FIREWORKS_API_KEY",
    "HF_TOKEN",
    // Other providers the catalog names variables for.
    "302AI_API_KEY",
    "DASHSCOPE_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_PROFILE",
    "AWS_REGION",
    // What carries a whole credential store or a whole configuration, which may hold keys.
    "OPENCODE_AUTH_CONTENT",
    "OPENCODE_CONFIG_CONTENT",
    // Where the data folder is.
    "XDG_DATA_HOME",
];

/// A key for the API `$standard`, read by OpenCode from `$variable`: a key account with that variable fixed.
macro_rules! key_of {
    ($standard:expr, $variable:literal) => {
        AcceptDef {
            standard: $standard,
            slots: &[SlotDef {
                credential: "api_key",
                kind: API_KEY,
                variable: Some($variable),
                file: None,
                base_url_setting: None,
                base_url_variable: None,
                dir_variable: Some("XDG_DATA_HOME"),
                settings: &[("variable", $variable)],
            }],
        }
    };
}

/// What OpenCode takes as an account.
pub static DEF: HarnessDef = HarnessDef {
    name: "OpenCode",
    program: "opencode",
    args: &["acp"],
    kinds: &[
        KindDef {
            id: LOGIN,
            provides: &[standard::OPENCODE],
            title: "OpenCode sign-in",
            description: "The `auth.json` OpenCode's own sign-in wrote, kept in a data folder of the account's own with the sessions made on it.",
            credentials: &[Credential {
                key: "auth_json",
                required: true,
            }],
            settings: &[],
            state_dir: true,
            rotates: true,
            history_portable: false,
            renews: false,
            endpoints: &[],
            notices: &[
                "the login is kept in a file that what the agent runs can read",
                "the account has an OpenCode data folder of its own: its sessions are the account's, and the person's own `opencode.json` still applies",
            ],
            credential_variables: &[],
            setting_variables: &[],
            named_variable: None,
            home_variable: Some("XDG_DATA_HOME"),
            file: Some((AUTH_FILE, "auth_json")),
            renew_unsupported: "OpenCode refreshes a sign-in itself when it needs one; it has no command that does so without a model turn",
        },
        KindDef {
            id: API_KEY,
            // Which API the key is for is the `variable` setting's: the account of a standard is made by `accepts`
            // below, which fixes it.
            provides: &[],
            title: "OpenCode provider key",
            description: "A key for one provider, read from the variable `variable` names, with OpenCode started on a data folder of its own so that no key in the person's `auth.json` meets it.",
            credentials: &[Credential {
                key: "api_key",
                required: true,
            }],
            settings: &[Setting {
                name: "variable",
                required: true,
                url: false,
                choices: &[],
                suggestions: KEY_VARIABLES,
                provides: &[],
            }],
            state_dir: true,
            rotates: false,
            history_portable: false,
            renews: false,
            endpoints: &[],
            notices: &[
                "the credential is in the environment of every program the agent runs, in the variable the account names, over the person's own value of it",
                "OpenCode uses it only where it reads that variable: a provider's own key variable, or one the person's own `opencode.json` names",
                "the account has an OpenCode data folder of its own: its sessions are the account's, and the person's own `opencode.json` still applies",
            ],
            credential_variables: &[],
            setting_variables: &[],
            named_variable: Some(("variable", "api_key")),
            home_variable: Some("XDG_DATA_HOME"),
            file: None,
            renew_unsupported: "a key is not renewed",
        },
        KindDef {
            id: PROVIDER_KEY,
            provides: &[standard::OPENCODE_KEY],
            title: "OpenCode provider key",
            description: "A key for one provider, read from the variable the account names, made by the `api-key` sign-in; OpenCode starts on a data folder of its own so that no key in the person's `auth.json` meets it.",
            credentials: &[Credential {
                key: "api_key",
                required: true,
            }],
            settings: &[Setting {
                name: "variable",
                required: true,
                url: false,
                choices: &[],
                suggestions: KEY_VARIABLES,
                provides: &[],
            }],
            state_dir: true,
            rotates: false,
            history_portable: false,
            renews: false,
            endpoints: &[],
            notices: &[
                "the credential is in the environment of every program the agent runs, in the variable the account names, over the person's own value of it",
                "OpenCode uses it only where it reads that variable: a provider's own key variable, or one the person's own `opencode.json` names",
                "the account has an OpenCode data folder of its own: its sessions are the account's, and the person's own `opencode.json` still applies",
            ],
            credential_variables: &[],
            setting_variables: &[],
            named_variable: Some(("variable", "api_key")),
            home_variable: Some("XDG_DATA_HOME"),
            file: None,
            renew_unsupported: "a key is not renewed",
        },
    ],
    own_standard: Some(standard::OPENCODE),
    // A key of each API OpenCode has a variable for. The endpoint of a provider is the person's own configuration
    // (`provider.<id>.options.baseURL`), which no account moves.
    accepts: &[
        key_of!(standard::ANTHROPIC_API, "ANTHROPIC_API_KEY"),
        key_of!(standard::OPENAI_API, "OPENAI_API_KEY"),
        key_of!(standard::GEMINI_API, "GEMINI_API_KEY"),
        key_of!(standard::XAI_API, "XAI_API_KEY"),
        AcceptDef {
            standard: standard::OPENCODE,
            slots: &[SlotDef {
                credential: "auth_json",
                kind: LOGIN,
                variable: None,
                file: Some(AUTH_FILE),
                base_url_setting: None,
                base_url_variable: None,
                dir_variable: Some("XDG_DATA_HOME"),
                settings: &[],
            }],
        },
    ],
    variables: VARIABLES,
    renew_args: None,

    login_unsupported: "OpenCode is signed in with the `provider-key`, `provider-code` and `api-key` methods; its own sign-in that returns the browser to a port on the machine it runs on is not offered",
};

/// Whether OpenCode has a sign-in: `auth.json` in its data folder (`XDG_DATA_HOME`, else `.local/share` below the home) that
/// holds something, or one of the variables a provider's key is read from, or `OPENCODE_AUTH_CONTENT`, which replaces the file.
fn signed_in(environment: &Environment, _wait: Duration) -> SignedInCheck {
    let mut variables: Vec<&str> = KEY_VARIABLES.to_vec();
    variables.push("OPENCODE_AUTH_CONTENT");
    let data = signed_in::value(environment, "XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| signed_in::home(environment).map(|home| home.join(".local").join("share")));
    signed_in::answer_for_file(
        environment,
        &variables,
        data.map(|data| data.join(AUTH_FILE)).as_deref(),
        "OpenCode's auth.json",
    )
}

/// How the stdio provider runs it.
pub static STDIO: StdioDef = StdioDef {
    accounts: &DEF,
    signed_in,
    accounts_description: "Makes an OpenCode account from a provider's key, watches the sign-in OpenCode keeps in `auth.json`, and says its accounts report no plan usage.",
    logins,
    sign_in: Some(sign_in),
    session_ids: Some(SessionIds {
        native: super::history::native_id_of,
        host: super::history::host_id_of,
    }),
    import: Some(import_session),
    export: Some(export_session),
    import_unsupported: "this provider does not seed a session in an agent it does not keep the sessions of; set replay_max_bytes to replay the history as context in the first prompt",
    export_unsupported: "this provider does not read a session out of an agent it does not keep the sessions of",
};

/// The ways to sign in: OpenCode's own `auth login` for a provider, which asks for a key (`provider-key`) or shows a code to
/// confirm on a page (`provider-code`), and a key in a variable the account names (`api-key`). A provider's sign-in through a
/// browser that returns to a port of the machine OpenCode runs on is not offered.
#[must_use]
pub fn logins() -> Vec<LoginMethodInfo> {
    let inputs = |providers: &[&str], methods: &[&str]| {
        SchemaBuilder::new()
            .field(
                FieldBuilder::new("provider", KindBuilder::string())
                    .title("Provider")
                    .description("The provider's id or name, as `opencode auth login` takes it.")
                    .examples(providers.iter().copied())
                    .required(),
            )
            .field(
                FieldBuilder::new("method", KindBuilder::string())
                    .title("Login method")
                    .description("The method's label, for a provider that has several.")
                    .examples(methods.iter().copied()),
            )
            .finish()
            .expect("the inputs schema builds")
    };
    let mut own_key =
        LoginMethodInfo::new("provider-key", "A provider's key, kept by OpenCode", LOGIN);
    own_key.description = "Runs `opencode auth login` for a provider and types the key it asks for into its prompt, so that OpenCode keeps the key in a sign-in file of the account's own. It sends nothing to the provider.".to_string();
    own_key.inputs = inputs(
        &["openrouter", "anthropic", "opencode", "google", "openai"],
        &["Manually enter API Key"],
    );
    own_key.steps = vec![ASK.to_string()];
    own_key.remote = true;
    let mut code = LoginMethodInfo::new("provider-code", "Sign in with a code", LOGIN);
    code.description = "Runs `opencode auth login` for a provider that shows a page and a code to confirm there, and waits until the person has. OpenCode keeps the sign-in in a file of the account's own.".to_string();
    code.inputs = inputs(
        &["github-copilot", "openai"],
        &["ChatGPT Pro/Plus (headless)"],
    );
    code.steps = vec![OPEN_URL.to_string(), USER_CODE.to_string()];
    code.remote = true;
    let mut key = LoginMethodInfo::new("api-key", "Provider API key", PROVIDER_KEY);
    key.description = "Asks which environment variable the key goes in (a provider's own is suggested, any other name works) and then the key. It sends nothing to the provider.".to_string();
    key.inputs = SchemaBuilder::new()
        .field(
            FieldBuilder::new("variable", KindBuilder::string())
                .examples(KEY_VARIABLES.iter().copied())
                .required(),
        )
        .finish()
        .expect("the inputs schema builds");
    key.steps = vec![ASK.to_string()];
    key.remote = true;
    vec![own_key, code, key]
}

fn bad(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, why)
}

fn launch(program: &str, args: &[&str], dir: &str) -> Launch {
    let mut launch = Launch::new(program)
        .args(args.iter().copied())
        .env("XDG_DATA_HOME", dir);
    launch.remove_env = VARIABLES.iter().map(OsString::from).collect();
    launch
}

fn key_form() -> LoginForm {
    LoginForm::secret("OpenCode provider key", "api_key")
}

/// The key the person typed into `form`, when it is one the program can be given on a line.
fn typed_key(ctx: &Ctx<'_>, form: LoginForm) -> Result<alloc::string::String, ProviderError> {
    let answer = ctx.ask(form)?;
    let key = answer
        .secrets
        .expose()
        .get("api_key")
        .unwrap_or_default()
        .to_string();
    if key.trim().is_empty() || key.chars().any(char::is_control) {
        return Err(bad("login refused: no usable key was given"));
    }
    Ok(key)
}

fn api_key(ctx: &Ctx<'_>, variable: &str) -> Result<LoginOutcome, ProviderError> {
    let key = typed_key(ctx, key_form())?;
    let mut credentials = Credentials::new();
    credentials.insert("api_key", key);
    let mut outcome = LoginOutcome::new(PROVIDER_KEY, credentials);
    outcome.settings = schema(&json!({"variable": variable}));
    // Whether the provider accepts the key is not known without a model turn.
    outcome.checked = false;
    Ok(outcome)
}

/// What `opencode auth login` has shown so far, read from its lines as they come.
#[derive(Default)]
struct Shown {
    url: Option<alloc::string::String>,
    code: Option<alloc::string::String>,
    opened: bool,
    keyed: bool,
    /// The title of a selection being listed, and the end of its list not yet seen.
    selecting: Option<alloc::string::String>,
    /// The last thing the program said it could not do.
    error: Option<alloc::string::String>,
}

/// A line of the program's prompts without the frame it draws them in: the bars, the marker and the bullet.
fn spoken(line: &str) -> &str {
    line.trim_start_matches(['|', '*', '>', '\u{2022}', ' '])
        .trim()
}

/// Whether `url` sends the browser back to a port of the machine the program runs on: its `redirect_uri` is a loopback
/// address, which the program then listens on.
fn returns_to_this_machine(url: &str) -> bool {
    let Some((_, query)) = url.split_once('?') else {
        return false;
    };
    query
        .split('&')
        .filter_map(|pair| pair.strip_prefix("redirect_uri="))
        .any(|target| {
            let target = percent_decoded(target);
            crate::cli::host_of(&target).is_some_and(|host| crate::cli::is_loopback(&host))
        })
}

fn percent_decoded(text: &str) -> alloc::string::String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|pair| core::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    alloc::string::String::from_utf8_lossy(&out).into_owned()
}

fn internal(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, why)
}

/// What one line of `opencode auth login` says, as far as a sign-in answers it.
#[derive(Debug, PartialEq, Eq)]
enum Said {
    /// The program refused: its reason.
    Error(alloc::string::String),
    /// The rule that ends a prompt's frame.
    Rule,
    /// A prompt's title: a selection when no key is asked.
    Title(alloc::string::String),
    /// The page to open.
    Page(alloc::string::String),
    /// The code to confirm there.
    Code(alloc::string::String),
    /// The prompt for a key.
    Key,
    /// Nothing to answer.
    Nothing,
}

fn classify(text: &str) -> Said {
    let text = text.trim();
    if let Some(error) = text.strip_prefix("Error:") {
        return Said::Error(error.trim().to_string());
    }
    if text.starts_with('\u{2014}') {
        return Said::Rule;
    }
    let said = spoken(text);
    if let Some(url) = said.strip_prefix("Go to:") {
        Said::Page(url.trim().to_string())
    } else if let Some(code) = said.strip_prefix("Enter code:") {
        Said::Code(code.trim().to_string())
    } else if said.contains("Enter your API key") {
        Said::Key
    } else if text.starts_with('*') {
        Said::Title(said.to_string())
    } else {
        Said::Nothing
    }
}

impl Shown {
    /// Takes one line, answering what it asks: a key is asked of the person and typed, a selection takes its first (the
    /// program's own default), a page and code are shown to the person.
    fn line(
        &mut self,
        ctx: &Ctx<'_>,
        input: &crate::cli::Cli,
        line: &str,
    ) -> Result<(), ProviderError> {
        match classify(line) {
            Said::Error(error) => self.error = Some(error),
            // The rule that ends a frame: a selection has been listed whole.
            Said::Rule => {
                if self.selecting.take().is_some() {
                    let _ = input.write(b"\r");
                }
            }
            Said::Title(title) => self.selecting = Some(title),
            Said::Page(url) => {
                if returns_to_this_machine(&url) {
                    return Err(ProviderError::new(
                        Status::GUATIAO_ERR_UNSUPPORTED,
                        "this sign-in returns the browser to a port on the machine OpenCode runs on, which this provider does not hand over; name a method that shows a code, or one that asks for a key",
                    ));
                }
                if !url.starts_with("https://")
                    || url.chars().any(|c| c.is_whitespace() || c.is_control())
                {
                    return Err(internal(
                        "OpenCode printed a page that is not an https address",
                    ));
                }
                self.url = Some(url);
            }
            Said::Code(code) => self.code = Some(code),
            Said::Key if !self.keyed => {
                self.keyed = true;
                let key = typed_key(ctx, key_form())?;
                input
                    .write(alloc::format!("{key}\r").as_bytes())
                    .map_err(|_| internal("OpenCode no longer reads its input"))?;
            }
            Said::Key | Said::Nothing => {}
        }
        if let Some(url) = &self.url
            && self.code.is_some()
            && !self.opened
        {
            self.opened = true;
            let mut open = OpenUrl::new(url.clone());
            open.user_code.clone_from(&self.code);
            ctx.open_url(open)?;
        }
        Ok(())
    }
}

/// `opencode auth login -p provider [-m method]` on the sign-in's data folder: what it asks is answered, and what it writes
/// is the account's sign-in.
fn provider_login(
    ctx: &Ctx<'_>,
    program: &str,
    dir: &str,
    provider: &str,
    method: Option<&str>,
) -> Result<LoginOutcome, ProviderError> {
    let mut args = vec!["auth", "login", "-p", provider];
    if let Some(method) = method {
        args.extend(["-m", method]);
    }
    let mut child = ctx.spawn(&launch(program, &args, dir))?;
    let input = child.input();
    let mut shown = Shown::default();
    let exit = child.run(ctx, |line| shown.line(ctx, &input, line))?;
    if exit.code != Some(0) {
        let why = shown
            .error
            .map(|error| alloc::format!(": {}", reason(&error, &[])))
            .unwrap_or_default();
        return Err(bad(&alloc::format!(
            "login refused: `opencode auth login` ended without a sign-in{why}"
        )));
    }
    let text = ctx
        .read_settled(AUTH_FILE)
        .map_err(|_| internal("`opencode auth login` wrote no sign-in file"))?;
    let mut outcome = LoginOutcome::new(LOGIN, login::one("auth_json", &text));
    // Whether the provider accepts it is not known without a model turn.
    outcome.checked = false;
    Ok(outcome)
}

/// Whether `text` can be one argument of `opencode auth login` that is no option: at most `limit` characters, no control
/// character, not starting with `-`.
fn argument(text: &str, limit: usize) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= limit
        && !text.starts_with('-')
        && !text.chars().any(char::is_control)
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
    let inputs = crate::provider::to_json(&request.inputs);
    let text = |name: &str| {
        inputs
            .get(name)
            .and_then(|value| value.as_str().map(ToString::to_string))
    };
    let dir = request.state_dir.clone();
    match request.method.as_str() {
        "api-key" => {
            let variable = text("variable")
                .ok_or_else(|| bad("login refused: the variable the key is for is missing"))?;
            check_variable_name(&variable)?;
            login::run(spawner, prompter, request.timeout_ms, move |ctx| {
                api_key(ctx, &variable)
            })
        }
        "provider-key" | "provider-code" => {
            let provider = text("provider")
                .filter(|provider| argument(provider, 64))
                .ok_or_else(|| {
                    bad("login refused: the provider is missing or not an id or a name")
                })?;
            let method = text("method").filter(|method| !method.trim().is_empty());
            if method
                .as_deref()
                .is_some_and(|method| !argument(method, 80))
            {
                return Err(bad("login refused: the login method is not a label"));
            }
            login::run(spawner, prompter, request.timeout_ms, move |ctx| {
                provider_login(ctx, &program, &dir, &provider, method.as_deref())
            })
        }
        _ => Err(ProviderError::new(
            Status::GUATIAO_ERR_BAD_VALUE,
            "this harness offers no such way to sign in",
        )),
    }
}

/// OpenCode's access settings in its words, under the AOS harness id `opencode`.
pub const TARGET: Target = Target::new("opencode", translate);

/// OpenCode's `OPENCODE_PERMISSION`, a JSON object of tool -> `allow`, `ask` or `deny` it reads when it starts. A tool set to
/// `deny` is not offered to the model at all.
const fn translate(access: Access) -> Translation {
    let sandbox = match access.sandbox {
        Sandbox::Unrestricted => Part::said("OpenCode has no sandbox"),
        Sandbox::ReadOnly => Part::env(
            &[("OPENCODE_PERMISSION", r#"{"edit":"deny"}"#)],
            "OpenCode's edit permission set to deny (OPENCODE_PERMISSION), which takes its edit and write tools away; its commands are not restricted",
        ),
        _ => Part::cannot(
            "OpenCode has no sandbox: its external_directory rule asks about paths outside the session's directory unless approvals are never",
        ),
    };
    let network = if access.network {
        Part::said("OpenCode has no network setting; it is on")
    } else {
        Part::cannot("OpenCode has no network setting")
    };
    let approvals = match (access.sandbox, access.approvals) {
        (Sandbox::ReadOnly, _) => {
            Part::cannot("a read-only sandbox denies every edit, so nothing is asked about one")
        }
        (_, Approvals::Edits) => Part::env(
            &[("OPENCODE_PERMISSION", r#"{"edit":"allow","bash":"ask"}"#)],
            "OpenCode's edit permission allow and bash permission ask (OPENCODE_PERMISSION)",
        ),
        (_, Approvals::Never) => Part::env(
            &[(
                "OPENCODE_PERMISSION",
                r#"{"edit":"allow","bash":"allow","read":"allow","external_directory":"allow","doom_loop":"allow"}"#,
            )],
            "OpenCode's permissions all allow, none asks (OPENCODE_PERMISSION)",
        ),
        _ => Part::env(
            &[("OPENCODE_PERMISSION", r#"{"edit":"ask","bash":"ask"}"#)],
            "OpenCode's edit and bash permissions ask (OPENCODE_PERMISSION)",
        ),
    };
    Translation::new(sandbox, network, approvals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_access_table_is_well_formed() {
        assert_eq!(TARGET.well_formed(), Ok(()));
    }

    #[test]
    fn the_standard_access_asks_about_edits_and_commands() {
        assert_eq!(
            Access::STANDARD.translate(&TARGET).env(),
            vec![("OPENCODE_PERMISSION", r#"{"edit":"ask","bash":"ask"}"#)]
        );
    }

    #[test]
    fn permissions_are_one_variable() {
        let permission = |sandbox, approvals| {
            let mut access = Access::STANDARD;
            access.sandbox = sandbox;
            access.approvals = approvals;
            let translation = access.translate(&TARGET);
            let env = translation.env();
            assert_eq!(env.len(), 1, "{access:?}: {env:?}");
            assert_eq!(env[0].0, "OPENCODE_PERMISSION");
            serde_json::from_str::<serde_json::Value>(env[0].1).expect("a JSON object")
        };
        assert_eq!(
            permission(Sandbox::Workspace, Approvals::Edits),
            serde_json::json!({"edit": "allow", "bash": "ask"})
        );
        assert_eq!(
            permission(Sandbox::Unrestricted, Approvals::Never)["external_directory"],
            "allow"
        );
        // A read-only sandbox denies edits whatever the approvals are, and says nothing else about them.
        for approvals in [Approvals::Ask, Approvals::Edits, Approvals::Never] {
            assert_eq!(
                permission(Sandbox::ReadOnly, approvals),
                serde_json::json!({"edit": "deny"})
            );
        }
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::ReadOnly;
        assert!(!access.translate(&TARGET).approvals.applies());
        assert_eq!(access.translate(&TARGET).mode(), None);
    }

    use crate::stdio::accounts::{apply, catalog};
    use coqui_acp_api::AccountGrant;

    const CANARY: &str = "sk-canary-91c0";

    fn foreign(standards: &[&str], key: &str) -> AccountGrant {
        AccountGrant::new("a", "someone-elses-kind")
            .provides(standards.iter().copied())
            .credentials(Credentials::from_iter([(key, CANARY)]))
            .state_dir("/some/dir")
    }

    fn names(applied: &crate::stdio::accounts::Applied) -> Vec<alloc::string::String> {
        applied
            .environment
            .expose()
            .names()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn opencode_lists_what_it_provides_and_accepts() {
        let catalog = catalog(&DEF);
        assert_eq!(
            catalog.kind(LOGIN).expect("login").provides,
            [standard::OPENCODE]
        );
        assert_eq!(
            catalog.kind(PROVIDER_KEY).expect("key").provides,
            [standard::OPENCODE_KEY]
        );
        assert!(catalog.kind(API_KEY).expect("applier").provides.is_empty());
        let accepted: Vec<&str> = catalog
            .accepts
            .iter()
            .map(|accepted| accepted.standard.as_str())
            .collect();
        assert_eq!(
            accepted,
            [
                standard::ANTHROPIC_API,
                standard::OPENAI_API,
                standard::GEMINI_API,
                standard::XAI_API,
                standard::OPENCODE
            ]
        );
    }

    #[test]
    fn every_variable_a_key_may_use_is_one_a_start_clears_and_the_access_is_not() {
        for variable in KEY_VARIABLES {
            assert!(VARIABLES.contains(variable), "{variable}");
        }
        assert!(!VARIABLES.contains(&"OPENCODE_PERMISSION"));
        assert!(VARIABLES.contains(&"XDG_DATA_HOME"));
    }

    #[test]
    fn a_key_of_each_accepted_api_is_read_from_that_apis_variable_in_a_data_folder_of_its_own() {
        for (standard, variable) in [
            (standard::ANTHROPIC_API, "ANTHROPIC_API_KEY"),
            (standard::OPENAI_API, "OPENAI_API_KEY"),
            (standard::GEMINI_API, "GEMINI_API_KEY"),
            (standard::XAI_API, "XAI_API_KEY"),
        ] {
            let applied = apply(&DEF, &foreign(&[standard], "api_key"))
                .expect("accepted")
                .expect("a kind");
            let seen = names(&applied);
            assert!(seen.contains(&variable.to_string()), "{standard}: {seen:?}");
            assert!(seen.contains(&"XDG_DATA_HOME".to_string()), "{standard}");
            assert!(!alloc::format!("{applied:?}").contains(CANARY));
        }
    }

    #[test]
    fn a_sign_in_from_elsewhere_is_the_file_in_the_data_folder_it_watches() {
        let applied = apply(&DEF, &foreign(&[standard::OPENCODE], "auth_json"))
            .expect("accepted")
            .expect("a kind");
        assert_eq!(applied.watched.expect("watched").path, "opencode/auth.json");
    }

    #[test]
    fn what_opencode_has_no_variable_for_or_cannot_move_is_refused_with_no_value_in_the_reason() {
        for (standards, key) in [
            (&[standard::CLAUDE_CODE][..], "token"),
            (&[standard::CODEX][..], "auth_file"),
            (&[][..], "api_key"),
            (&[standard::ANTHROPIC_API][..], "auth_token"),
        ] {
            let Err(error) = apply(&DEF, &foreign(standards, key)) else {
                panic!("{standards:?} {key} was taken");
            };
            assert!(!error.message().contains(CANARY));
        }
        let moved = foreign(&[standard::OPENAI_API], "api_key")
            .settings(schema(&json!({"base_url": "https://gateway.example/v1"})));
        assert!(apply(&DEF, &moved).is_err());
    }

    #[test]
    fn a_key_account_names_any_variable_and_the_form_only_suggests() {
        let key = |kind: &str, variable: &str| {
            AccountGrant::new("a", kind)
                .settings(schema(&json!({"variable": variable})))
                .credentials(Credentials::from_iter([("api_key", CANARY)]))
                .state_dir("/some/dir")
        };
        for kind in [API_KEY, PROVIDER_KEY] {
            let info = catalog(&DEF).kind(kind).expect("kind").settings.clone();
            let variable = &crate::provider::to_json(&info)["properties"]["variable"];
            assert_eq!(variable["examples"], json!(KEY_VARIABLES), "{kind}");
            assert!(variable.get("enum").is_none(), "{kind}: {variable}");
            // A provider's own variable, one the catalog may add, and ones that move programs or the person's own: all taken.
            for name in [
                "GROQ_API_KEY",
                "ACME_GATEWAY_KEY",
                "lower_case_1",
                "_X",
                "LD_PRELOAD",
                "PATH",
                "OPENCODE_PERMISSION",
            ] {
                let applied = apply(&DEF, &key(kind, name))
                    .unwrap_or_else(|error| panic!("{name} refused: {}", error.message()))
                    .expect("a kind");
                assert!(names(&applied).contains(&name.to_string()), "{name}");
                assert!(!alloc::format!("{applied:?}").contains(CANARY));
            }
            // Only what is no variable name, or what the host owns, is refused.
            for bad in [
                "",
                "1ST_KEY",
                "A=B",
                "TWO WORDS",
                "KEY-NAME",
                "AOS_OVERLAY_NAME",
                "aos_x",
                "TEAM_OVERLAY_ROOT",
                "team_overlay_state",
            ] {
                let Err(error) = apply(&DEF, &key(kind, bad)) else {
                    panic!("{bad:?} was taken");
                };
                assert!(!error.message().contains(CANARY));
            }
        }
    }

    fn recorded() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/opencode_auth_login.json"
        ))
        .expect("the recorded prompts are JSON")
    }

    /// What a sign-in answers in `output`, in order.
    fn said_in(output: &str) -> Vec<Said> {
        output
            .lines()
            .map(|line| classify(&crate::cli::plain(line)))
            .filter(|said| *said != Said::Nothing)
            .collect()
    }

    fn said_of(name: &str, field: &str) -> Vec<Said> {
        said_in(
            recorded()["scenarios"][name][field]
                .as_str()
                .expect("recorded"),
        )
    }

    #[test]
    fn the_prompts_the_program_printed_are_read_as_what_they_ask() {
        let title = |text: &str| Said::Title(text.to_string());
        assert!(said_of("picker", "output").contains(&title("Select provider")));
        assert!(said_of("openai_methods", "output").contains(&title("Login method")));
        assert!(
            said_of("copilot_type", "output").contains(&title("Select GitHub deployment type"))
        );
        for (name, field) in [
            ("openai_key", "output"),
            ("anthropic_key", "output"),
            ("opencode_key", "output"),
            ("openrouter_key", "before_input"),
        ] {
            let said = said_of(name, field);
            assert!(said.contains(&Said::Key), "{name}: {said:?}");
            // The key's own prompt is no selection.
            assert!(
                !said.iter().any(|said| matches!(said, Said::Title(_))),
                "{name}"
            );
        }
        for (name, field, page) in [
            (
                "openai_headless",
                "output",
                "https://auth.openai.com/codex/device",
            ),
            (
                "copilot_device",
                "after_input",
                "https://github.com/login/device",
            ),
        ] {
            let said = said_of(name, field);
            assert!(
                said.contains(&Said::Page(page.to_string())),
                "{name}: {said:?}"
            );
            assert!(
                said.contains(&Said::Code("ABCD-12345".to_string())),
                "{name}: {said:?}"
            );
            assert!(!returns_to_this_machine(page));
        }
        let browser = said_of("openai_browser", "output");
        let Some(Said::Page(page)) = browser.iter().find(|said| matches!(said, Said::Page(_)))
        else {
            panic!("no page in {browser:?}");
        };
        assert!(returns_to_this_machine(page), "{page}");
        assert!(!browser.iter().any(|said| matches!(said, Said::Code(_))));
        assert!(
            said_of("openai_bad_method", "output")
                .iter()
                .any(|said| matches!(said, Said::Error(why) if why.starts_with("Unknown method \"nonsense\" for openai")))
        );
        assert!(said_of("bad_provider", "output").contains(&Said::Error(
            "Unknown provider \"nosuchprovider\"".to_string()
        )));
    }

    #[test]
    fn a_page_that_sends_the_browser_back_to_this_machine_is_known_by_its_redirect() {
        let back = "https://auth.example.com/authorize?client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fcb&state=s";
        assert!(returns_to_this_machine(back));
        assert!(returns_to_this_machine(
            "https://a.example/o?redirect_uri=http%3A%2F%2F127.0.0.1%3A40000%2Fcallback"
        ));
        assert!(!returns_to_this_machine(
            "https://a.example/o?redirect_uri=https%3A%2F%2Fapp.example.com%2Fcb"
        ));
        assert!(!returns_to_this_machine("https://a.example/device"));
    }

    #[test]
    fn what_the_program_is_given_is_a_label_and_never_an_option() {
        assert!(argument("openai", 64) && argument("ChatGPT Pro/Plus (headless)", 80));
        for bad in ["", "  ", "-p", "--help", "a\nb", "a\u{1b}b"] {
            assert!(!argument(bad, 80), "{bad:?}");
        }
        assert!(!argument(&"x".repeat(65), 64));
    }
}
