//! Signing in to Codex, saving a key or token, and renewing a login, by its own commands, through the host's spawner.
//!
//! `codex login` prints the page to open and waits for the browser to come back to this machine; `--device-auth` prints
//! a page and a one-time code instead. A key or token goes to `codex login --with-api-key` / `--with-access-token` on
//! standard input and is in no argument, answer, error or log.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::time::Duration;
use std::ffi::OsString;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{Status, Wipe};
use coqui_acp_api::{
    Credentials, LoginForm, LoginOutcome, LoginPrompter, LoginRequest, OpenUrl, ProcessSpawner,
    RenewReport, RenewRequest,
};
use coqui_acp_harness::ThroughSpawner;
use coqui_acp_harness::account::{Accounts, Files, SecretText};
use coqui_acp_harness::cli::{Next, address_in, plain, run};
use coqui_acp_harness::login::{Job, asked, reason, supervise};
use coqui_acp_harness::process::Launch;
use coqui_acp_harness::provider::{credential, schema, to_json};
use serde_json::{Value, json};

use crate::accounts::{
    self, ACCESS_TOKEN, ACCESS_TOKEN_METHOD, API_KEY, API_KEY_METHOD, AUTH_FILE, CHATGPT,
    CHATGPT_METHOD, CONFIG_FILE, CONFIG_TEXT, DEVICE_METHOD, FILE_KEY, StdinLogin, VARIABLES,
};
use crate::{CodexLaunch, CodexSession, CodexSettings};

/// The most output of a command that is kept, in bytes.
const OUTPUT: usize = 1 << 16;

/// How long a device code stays good, in milliseconds.
const DEVICE_CODE_MS: u64 = 900_000;

fn failure(status: Status, why: &str) -> ProviderError {
    ProviderError::new(status, why)
}

fn internal(why: &str) -> ProviderError {
    failure(Status::GUATIAO_ERR_INTERNAL, why)
}

/// A command line of Codex for the account commands: every credential variable removed, then `set`.
pub(crate) fn command(executable: &OsString, args: &[&str], dir: &str) -> Launch {
    let mut launch = Launch::new(executable.clone()).args(args.iter().copied());
    launch.remove_env = VARIABLES.iter().map(OsString::from).collect();
    launch.env("CODEX_HOME", dir)
}

/// Makes Codex keep its credentials in `auth.json` of its home, as every command on it then agrees.
pub(crate) fn configure(files: &Files) -> Result<(), ProviderError> {
    if files.info(CONFIG_FILE)?.is_none() {
        files.write(CONFIG_FILE, CONFIG_TEXT)?;
    }
    Ok(())
}

/// Saves a key or token with `codex login`, the secret on standard input.
///
/// # Errors
///
/// `BAD_VALUE` `login refused: ...` when Codex refuses it; `INTERNAL` when it could not be run.
pub(crate) fn save(
    spawn: &ThroughSpawner,
    executable: &OsString,
    dir: &str,
    login: &StdinLogin,
    limit: Duration,
    job: Option<&Job>,
) -> Result<(), ProviderError> {
    let launch = command(executable, &["login", login.flag], dir);
    let mut input = format!("{}\n", login.secret);
    let done = match job {
        Some(job) => job.run(spawn, &launch, Some(input.as_bytes()), limit, OUTPUT),
        None => run(spawn, &launch, Some(input.as_bytes()), limit, OUTPUT)
            .map_err(|_| internal("Codex could not be started")),
    };
    input.wipe();
    let done = done?;
    if done.timed_out {
        return Err(internal(
            "Codex did not finish saving the credential in time",
        ));
    }
    if done.succeeded() {
        return Ok(());
    }
    let printed = format!(
        "{}\n{}",
        String::from_utf8_lossy(&done.stdout),
        String::from_utf8_lossy(&done.stderr)
    );
    Err(failure(
        Status::GUATIAO_ERR_BAD_VALUE,
        &format!("login refused: {}", reason(&printed, &[&login.secret])),
    ))
}

/// Whether `codex login status` says a login is configured. It asks no model.
pub(crate) fn configured(
    spawn: &ThroughSpawner,
    executable: &OsString,
    dir: &str,
    limit: Duration,
    job: Option<&Job>,
) -> bool {
    let launch = command(executable, &["login", "status"], dir);
    match job {
        Some(job) => job
            .run(spawn, &launch, None, limit, OUTPUT)
            .is_ok_and(|done| done.succeeded()),
        None => run(spawn, &launch, None, limit, OUTPUT).is_ok_and(|done| done.succeeded()),
    }
}

/// What a sign-in needs, gathered before the worker starts.
struct Plan {
    method: String,
    dir: String,
    executable: OsString,
    limit: Duration,
}

/// Signs in as `request` says.
///
/// # Errors
///
/// `BAD_VALUE` for a method it does not list or a credential Codex refuses (`login refused: ...`); `UNSUPPORTED`
/// when the spawner serves no files; `GONE` for `login cancelled` and `login timed out`, answered within a moment of
/// either; `INTERNAL` otherwise, with a reason that holds no input and no address.
pub(crate) fn login(
    executable: &OsString,
    command_limit: Duration,
    request: &LoginRequest,
    spawner: Object<dyn ProcessSpawner>,
    prompter: Object<dyn LoginPrompter>,
) -> Result<LoginOutcome, ProviderError> {
    let spawner = Arc::new(spawner);
    if !accounts::catalog()
        .logins
        .iter()
        .any(|method| method.id == request.method)
    {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "this provider has no such sign-in method",
        ));
    }
    if request.state_dir.is_empty() {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "a sign-in needs a directory to run in",
        ));
    }
    if !to_json(&request.inputs)
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "the sign-in asks no inputs",
        ));
    }
    if !Files::new(Arc::clone(&spawner)).available() {
        return Err(failure(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this spawner serves no private files, which a sign-in reads its result from",
        ));
    }
    let plan = Plan {
        method: request.method.clone(),
        dir: request.state_dir.clone(),
        executable: executable.clone(),
        limit: command_limit,
    };
    supervise(
        spawner,
        prompter,
        request.timeout_ms,
        Box::new(move |spawner, prompter, job| work(&plan, spawner, prompter, job)),
    )
}

fn work(
    plan: &Plan,
    spawner: &Arc<Object<dyn ProcessSpawner>>,
    prompter: &Object<dyn LoginPrompter>,
    job: &Job,
) -> Result<LoginOutcome, ProviderError> {
    let files = Files::new(Arc::clone(spawner));
    let spawn = ThroughSpawner::shared(Arc::clone(spawner));
    configure(&files)?;
    match plan.method.as_str() {
        CHATGPT_METHOD | DEVICE_METHOD => web(plan, &files, &spawn, prompter, job),
        API_KEY_METHOD => typed(
            plan,
            &spawn,
            prompter,
            job,
            API_KEY,
            "api_key",
            "--with-api-key",
        ),
        ACCESS_TOKEN_METHOD => typed(
            plan,
            &spawn,
            prompter,
            job,
            ACCESS_TOKEN,
            "access_token",
            "--with-access-token",
        ),
        _ => Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "this provider has no such sign-in method",
        )),
    }
}

/// The one-time code a device sign-in prints: the first line after the one that asks for it.
fn device_code(shown: &str) -> Option<String> {
    let mut lines = shown.lines();
    lines.find(|line| line.contains("one-time code"))?;
    let code = lines.map(str::trim).find(|line| !line.is_empty())?;
    // The line ends with a newline when the code is whole.
    shown.ends_with('\n').then(|| code.to_string())
}

fn web(
    plan: &Plan,
    files: &Files,
    spawn: &ThroughSpawner,
    prompter: &Object<dyn LoginPrompter>,
    job: &Job,
) -> Result<LoginOutcome, ProviderError> {
    let device = plan.method == DEVICE_METHOD;
    let args: &[&str] = if device {
        &["login", "--device-auth"]
    } else {
        &["login"]
    };
    let cli = job.start(spawn, &command(&plan.executable, args, &plan.dir))?;
    let mut seen: Vec<u8> = Vec::new();
    let (url, code) = loop {
        match cli.next(Duration::from_millis(100)) {
            Next::Piece(piece) => {
                seen.extend_from_slice(&piece.bytes);
                seen.truncate(OUTPUT);
                let shown = plain(&String::from_utf8_lossy(&seen));
                let url = address_in(&shown, &["auth.openai.com"]);
                let code = device_code(&shown);
                if url.is_some() && (!device || code.is_some()) {
                    break (url.unwrap_or_default(), code);
                }
            }
            Next::Idle => job.check()?,
            Next::Ended => {
                return Err(internal(
                    "Codex ended without printing a page to sign in at",
                ));
            }
        }
    };
    job.check()?;
    let mut open = OpenUrl::new(url);
    if device {
        open.user_code = code;
        open.expires_ms = Some(DEVICE_CODE_MS);
    }
    asked(prompter.open_url(open))?;
    // The person signs in on the page; Codex ends when it has the login.
    let mut printed = String::new();
    loop {
        match cli.next(Duration::from_millis(100)) {
            Next::Piece(piece) => {
                printed.push_str(&String::from_utf8_lossy(&piece.bytes));
                if printed.len() > OUTPUT {
                    printed.drain(..printed.len() - OUTPUT);
                }
            }
            Next::Idle => job.check()?,
            Next::Ended => break,
        }
    }
    job.check()?;
    let exit = cli.wait(Duration::from_secs(5));
    if !exit.is_some_and(|exit| exit.code == Some(0)) {
        return Err(internal(&format!(
            "Codex's sign-in did not finish: {}",
            reason(&printed, &[])
        )));
    }
    let text = files
        .read(AUTH_FILE)?
        .unwrap_or_else(|| SecretText::new(String::new()));
    if !accounts::usable(&text) {
        return Err(internal("Codex signed in but wrote no login"));
    }
    let mut credentials = Credentials::new();
    credentials.insert(FILE_KEY, text.to_string());
    let mut outcome = LoginOutcome::new(CHATGPT, credentials);
    // `login status` asks no model.
    outcome.checked = configured(spawn, &plan.executable, &plan.dir, plan.limit, Some(job));
    Ok(outcome)
}

fn typed(
    plan: &Plan,
    spawn: &ThroughSpawner,
    prompter: &Object<dyn LoginPrompter>,
    job: &Job,
    produces: &str,
    key: &'static str,
    flag: &'static str,
) -> Result<LoginOutcome, ProviderError> {
    let form = LoginForm::new(
        if key == "api_key" {
            "Enter the OpenAI API key"
        } else {
            "Enter the access token"
        },
        schema(&json!({
            "type": "object",
            "properties": {key: credential()},
            "required": [key],
        })),
    );
    let answer = asked(prompter.ask(form))?;
    job.check()?;
    let secret = answer
        .secrets
        .expose()
        .get(key)
        .unwrap_or_default()
        .trim()
        .to_string();
    if secret.is_empty() {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "login refused: nothing was entered",
        ));
    }
    let mut login = StdinLogin { flag, key, secret };
    let saved = save(
        spawn,
        &plan.executable,
        &plan.dir,
        &login,
        plan.limit,
        Some(job),
    );
    let mut credentials = Credentials::new();
    credentials.insert(key, login.secret.clone());
    login.secret.wipe();
    saved?;
    job.check()?;
    let mut outcome = LoginOutcome::new(produces, credentials);
    // Codex checks a key only when a request is sent, and `login status` says it is configured, not that it works.
    outcome.checked = key == "access_token"
        && configured(spawn, &plan.executable, &plan.dir, plan.limit, Some(job));
    Ok(outcome)
}

/// Renews the ChatGPT login in `request` with no model turn: the app server is asked for the account with a refresh
/// of the token ahead of its end, and the file it keeps is read afterwards.
///
/// # Errors
///
/// `BAD_VALUE` for an account it does not take; `UNSUPPORTED` for a kind with no renewal; `GONE` when the login is
/// lost (a file missing or blank, or Codex says nobody is signed in); `INTERNAL` when Codex could not be asked.
pub(crate) fn renew(
    accounts: &Arc<Accounts>,
    executable: &OsString,
    request: &RenewRequest,
    spawner: Object<dyn ProcessSpawner>,
) -> Result<RenewReport, ProviderError> {
    let grant = &request.account;
    let applied = accounts::plan(grant)?;
    let Some(applied) = applied.filter(|_| grant.kind == CHATGPT) else {
        return Err(failure(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "Codex renews only a ChatGPT login; this kind holds a credential that is not refreshed",
        ));
    };
    let spawner = Arc::new(spawner);
    let files = Files::new(Arc::clone(&spawner));
    if !files.available() {
        return Err(failure(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this spawner serves no private files, which a login is kept in",
        ));
    }
    configure(&files)?;
    let hold = accounts.hold(
        files,
        None,
        &grant.id,
        &applied.dir,
        grant.revision,
        grant.credentials.expose(),
        applied.watched.clone(),
    )?;
    let lost = |hold: &coqui_acp_harness::account::Hold| {
        hold.look()?;
        match hold.lost() {
            Some(why) => Err(failure(
                Status::GUATIAO_ERR_GONE,
                &format!("the login is lost: {why}"),
            )),
            None => Ok(()),
        }
    };
    lost(&hold)?;
    let mut launch = CodexLaunch::new(".");
    launch.executable.clone_from(executable);
    launch.remove_env = applied.remove.iter().map(OsString::from).collect();
    for (name, value) in &applied.set {
        launch.env.insert(name.into(), value.into());
    }
    launch.config_overrides.clone_from(&applied.config);
    let spawn = ThroughSpawner::shared(spawner);
    let answer = CodexSession::refresh_account(&launch, &spawn, CodexSettings::new())
        .map_err(|_| internal("Codex could not be asked to refresh the login"))?;
    lost(&hold)?;
    if answer.get("account").is_none_or(Value::is_null) {
        return Err(failure(
            Status::GUATIAO_ERR_GONE,
            "the login is lost: Codex reports nobody signed in",
        ));
    }
    let after = hold.credentials()?;
    let expires_at = hold.expires_at();
    hold.finish();
    if after.expose().get(FILE_KEY) == grant.credentials.expose().get(FILE_KEY) {
        return Ok(RenewReport::unchanged(
            "Codex kept the login as it was: it was not due for a refresh",
        ));
    }
    let mut report = RenewReport::renewed(after.expose().clone());
    report.expires_at = expires_at;
    Ok(report)
}
