//! Signing in to Claude Code and renewing a login, by its own commands, through the host's spawner.
//!
//! `claude auth login` prints the page to open and waits for the code the page shows; a key or token is only checked
//! with `claude auth status`, which asks no model. A code, a key or a token goes to the command on its standard input
//! or in its environment, and is in no argument, answer, error or log.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use std::ffi::OsString;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{Status, Wipe};
use coqui_acp_api::{
    AUTH, Credentials, LoginForm, LoginMethodInfo, LoginOutcome, LoginPrompter, LoginRequest,
    OpenUrl, ProcessSpawner, RenewReport, RenewRequest,
};
use coqui_acp_harness::ThroughSpawner;
use coqui_acp_harness::account::{Accounts, Files, SecretText, rfc3339_ms};
use coqui_acp_harness::cli::{Cli, Next, address_in, host_of, run};
use coqui_acp_harness::login::{Job, asked, gone, reason, supervise};
use coqui_acp_harness::process::Launch;
use coqui_acp_harness::provider::{credential, schema, to_json};
use serde_json::{Value, json};

use crate::accounts::{
    self, API_KEY, API_KEY_METHOD, AUTH_TOKEN, AUTH_TOKEN_METHOD, CONSOLE, CONSOLE_METHOD,
    CREDENTIALS_FILE, FILE_KEY, LOGIN, LONG_LIVED_TOKEN, SUBSCRIPTION, VARIABLES,
};

/// How close to its end a login must be for `claude auth status` to refresh it, in milliseconds (Claude Code's own
/// window).
pub(crate) const REFRESH_WINDOW_MS: i64 = 300_000;

/// The most output of a command that is kept, in bytes.
const OUTPUT: usize = 1 << 16;

fn failure(status: Status, why: &str) -> ProviderError {
    ProviderError::new(status, why)
}

fn internal(why: &str) -> ProviderError {
    failure(Status::GUATIAO_ERR_INTERNAL, why)
}

/// A command line of Claude Code for the account commands: every credential variable removed, then `set`.
pub(crate) fn command(executable: &OsString, args: &[&str], set: &[(String, String)]) -> Launch {
    let mut launch = Launch::new(executable.clone()).args(args.iter().copied());
    launch.remove_env = VARIABLES.iter().map(OsString::from).collect();
    for (name, value) in set {
        launch = launch.env(name, value);
    }
    launch
}

/// What `claude auth status` says: whether it ran and was logged in, and the address of the person when it names one.
#[derive(Debug, Default)]
pub(crate) struct Checked {
    pub exited_ok: bool,
    pub logged_in: bool,
    pub label: Option<String>,
}

/// Runs `claude auth status` as `set` says, within `limit`; inside a sign-in, as part of its `job`, so that cancelling
/// ends it.
pub(crate) fn status(
    spawn: &ThroughSpawner,
    executable: &OsString,
    set: &[(String, String)],
    limit: Duration,
    job: Option<&Job>,
) -> Result<Checked, ProviderError> {
    let launch = command(executable, &["auth", "status"], set);
    let done = match job {
        Some(job) => job.run(spawn, &launch, None, limit, OUTPUT)?,
        None => run(spawn, &launch, None, limit, OUTPUT)
            .map_err(|_| internal("Claude Code could not be started"))?,
    };
    if done.timed_out {
        return Err(internal("Claude Code did not answer `auth status` in time"));
    }
    let text = String::from_utf8_lossy(&done.stdout).into_owned();
    let json: Option<Value> = serde_json::from_str(&text).ok();
    let logged_in = json
        .as_ref()
        .and_then(|j| j.get("loggedIn"))
        .and_then(Value::as_bool)
        .unwrap_or(done.succeeded());
    let label = json
        .as_ref()
        .and_then(|j| j.get("email"))
        .and_then(Value::as_str)
        .filter(|email| {
            !email.is_empty() && email.len() <= 254 && !email.chars().any(char::is_control)
        })
        .map(str::to_string);
    Ok(Checked {
        exited_ok: done.succeeded(),
        logged_in,
        label,
    })
}

fn email_of(inputs: &coqui_acp_api::guatiao::Value) -> Result<Option<String>, ProviderError> {
    let inputs = to_json(inputs);
    let Some(object) = inputs.as_object() else {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "the sign-in's inputs are not an object",
        ));
    };
    if object.keys().any(|key| key != "email") {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "the sign-in's inputs name something it does not ask",
        ));
    }
    match object.get("email") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(email))
            if !email.is_empty()
                && email.len() <= 254
                && !email.starts_with('-')
                && !email.chars().any(|c| c.is_control() || c.is_whitespace()) =>
        {
            Ok(Some(email.clone()))
        }
        Some(_) => Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "that is not an address",
        )),
    }
}

/// What a sign-in needs, gathered before the worker starts.
struct Plan {
    method: LoginMethodInfo,
    email: Option<String>,
    dir: String,
    executable: OsString,
    limit: Duration,
}

fn plan(
    executable: &OsString,
    request: &LoginRequest,
    command_limit: Duration,
    files: &Files,
) -> Result<Plan, ProviderError> {
    if request.method == LONG_LIVED_TOKEN {
        return Err(failure(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "`claude setup-token` needs a terminal and its prompts were not recorded, so this provider cannot run it",
        ));
    }
    let catalog = accounts::offer();
    let Some(method) = catalog.logins.iter().find(|m| m.id == request.method) else {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "this provider has no such sign-in method",
        ));
    };
    if request.state_dir.is_empty() {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "a sign-in needs a directory to run in",
        ));
    }
    let email = email_of(&request.inputs)?;
    if !files.available() {
        return Err(failure(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this spawner serves no private files, which a sign-in reads its result from",
        ));
    }
    Ok(Plan {
        method: method.clone(),
        email,
        dir: request.state_dir.clone(),
        executable: executable.clone(),
        limit: command_limit,
    })
}

/// Signs in as `request` says.
///
/// # Errors
///
/// `BAD_VALUE` for a method it does not list or a key Claude Code refuses (`login refused: ...`); `UNSUPPORTED`
/// for a method it cannot run; `GONE` for `login cancelled` and `login timed out`, answered within a moment of
/// either; `INTERNAL` otherwise, with a reason that holds no input and no address.
pub(crate) fn login(
    executable: &OsString,
    command_limit: Duration,
    request: &LoginRequest,
    spawner: Object<dyn ProcessSpawner>,
    prompter: Object<dyn LoginPrompter>,
) -> Result<LoginOutcome, ProviderError> {
    let spawner = Arc::new(spawner);
    let plan = {
        let files = Files::new(Arc::clone(&spawner));
        plan(executable, request, command_limit, &files)?
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
    let dir = ("CLAUDE_CONFIG_DIR".to_string(), plan.dir.clone());
    match plan.method.id.as_str() {
        SUBSCRIPTION | CONSOLE_METHOD => {
            let (flag, produces) = if plan.method.id == SUBSCRIPTION {
                ("--claudeai", LOGIN)
            } else {
                ("--console", CONSOLE)
            };
            let mut args = vec!["auth", "login", flag];
            if let Some(email) = &plan.email {
                args.push("--email");
                args.push(email);
            }
            let launch = command(&plan.executable, &args, core::slice::from_ref(&dir));
            let cli = job.start(&spawn, &launch)?;
            web(plan, &files, &spawn, prompter, job, &cli, produces, &dir)
        }
        API_KEY_METHOD => typed(
            plan,
            &spawn,
            prompter,
            job,
            API_KEY,
            "api_key",
            "ANTHROPIC_API_KEY",
            &dir,
        ),
        AUTH_TOKEN_METHOD => typed(
            plan,
            &spawn,
            prompter,
            job,
            AUTH_TOKEN,
            "auth_token",
            "ANTHROPIC_AUTH_TOKEN",
            &dir,
        ),
        _ => Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "this provider has no such sign-in method",
        )),
    }
}

/// Everything the command printed so far, as text.
fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[expect(
    clippy::too_many_arguments,
    reason = "one sign-in's parts, handed down once"
)]
fn web(
    plan: &Plan,
    files: &Files,
    spawn: &ThroughSpawner,
    prompter: &Object<dyn LoginPrompter>,
    job: &Job,
    cli: &Arc<Cli>,
    produces: &str,
    dir: &(String, String),
) -> Result<LoginOutcome, ProviderError> {
    // The page is on the host the kind signs in at: what the command prints cannot send the person elsewhere.
    let host = accounts::offer()
        .kind(produces)
        .and_then(|kind| kind.endpoints.iter().find(|slot| slot.purpose == AUTH))
        .and_then(|slot| slot.default.as_deref().and_then(host_of));
    let hosts: Vec<&str> = host.iter().map(String::as_str).collect();
    let mut seen: Vec<u8> = Vec::new();
    let mut url = None;
    while url.is_none() {
        match cli.next(Duration::from_millis(100)) {
            Next::Piece(piece) => {
                seen.extend_from_slice(&piece.bytes);
                seen.truncate(OUTPUT);
                let shown = text_of(&seen);
                // The address is whole once something follows it.
                if shown.contains("Paste") || shown.contains('\n') {
                    url = address_in(&shown, &hosts);
                }
            }
            Next::Idle => job.check()?,
            Next::Ended => {
                return Err(internal(
                    "Claude Code ended without printing a page to sign in at",
                ));
            }
        }
    }
    job.check()?;
    let url = url.unwrap_or_default();
    asked(prompter.open_url(OpenUrl::new(url)))?;
    job.check()?;
    let form = LoginForm::new(
        "Paste the code the page shows",
        schema(&json!({
            "type": "object",
            "properties": {"code": credential()},
            "required": ["code"],
        })),
    );
    let answer = asked(prompter.ask(form))?;
    job.check()?;
    let code = SecretText::new(
        answer
            .secrets
            .expose()
            .get("code")
            .unwrap_or_default()
            .trim()
            .to_string(),
    );
    if code.is_empty() {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "login refused: no code was given",
        ));
    }
    let mut line = format!("{}\n", &*code);
    let written = cli.write(line.as_bytes());
    line.wipe();
    written.map_err(|_| internal("Claude Code stopped reading before the code was given"))?;
    cli.close_input();
    let mut printed = String::new();
    loop {
        match cli.next(Duration::from_millis(100)) {
            Next::Piece(piece) => {
                printed.push_str(&text_of(&piece.bytes));
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
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            &format!("login refused: {}", reason(&printed, &[&*code])),
        ));
    }
    let text = files
        .read(CREDENTIALS_FILE)?
        .unwrap_or_else(|| SecretText::new(String::new()));
    if !accounts::usable(&text) {
        return Err(internal("Claude Code signed in but wrote no login"));
    }
    let expires = accounts::expires(&text);
    let mut credentials = Credentials::new();
    credentials.insert(FILE_KEY, text.to_string());
    let mut outcome = LoginOutcome::new(produces, credentials);
    outcome.expires_at = expires;
    // `auth status` asks no model: it checks the login Claude Code has just written.
    if let Ok(checked) = status(
        spawn,
        &plan.executable,
        core::slice::from_ref(dir),
        plan.limit,
        Some(job),
    ) {
        outcome.checked = checked.exited_ok && checked.logged_in;
        outcome.label = checked.label;
    }
    Ok(outcome)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one sign-in's parts, handed down once"
)]
fn typed(
    plan: &Plan,
    spawn: &ThroughSpawner,
    prompter: &Object<dyn LoginPrompter>,
    job: &Job,
    produces: &str,
    key: &str,
    variable: &str,
    dir: &(String, String),
) -> Result<LoginOutcome, ProviderError> {
    let form = LoginForm::new(
        format!("Enter the {}", plan.method.title.to_lowercase()),
        schema(&json!({
            "type": "object",
            "properties": {key: credential()},
            "required": [key],
        })),
    );
    let answer = asked(prompter.ask(form))?;
    job.check()?;
    let value = SecretText::new(
        answer
            .secrets
            .expose()
            .get(key)
            .unwrap_or_default()
            .trim()
            .to_string(),
    );
    if value.is_empty() {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "login refused: nothing was entered",
        ));
    }
    let mut set = vec![dir.clone(), (variable.to_string(), value.to_string())];
    let checked = status(spawn, &plan.executable, &set, plan.limit, Some(job));
    for (_, secret) in &mut set {
        secret.wipe();
    }
    let checked = checked?;
    job.check()?;
    if !checked.exited_ok || !checked.logged_in {
        return Err(failure(
            Status::GUATIAO_ERR_BAD_VALUE,
            "login refused: Claude Code does not accept it",
        ));
    }
    let mut credentials = Credentials::new();
    credentials.insert(key, value.to_string());
    let mut outcome = LoginOutcome::new(produces, credentials);
    outcome.checked = true;
    outcome.label = checked.label;
    Ok(outcome)
}

/// Renews the login in `request` with no model turn: `claude auth status` refreshes a login within five minutes of its
/// end, and the file it keeps is read afterwards. A login in the directory that is newer than the grant's is answered
/// as the renewal it is, whatever the window says.
///
/// # Errors
///
/// `BAD_VALUE` for an account it does not take; `UNSUPPORTED` for a kind with no renewal; `GONE` when the login is
/// lost (a file missing, blank or blanked by a refused refresh); `INTERNAL` when Claude Code could not be asked.
pub(crate) fn renew(
    accounts: &Arc<Accounts>,
    executable: &OsString,
    command_limit: Duration,
    request: &RenewRequest,
    spawner: Object<dyn ProcessSpawner>,
) -> Result<RenewReport, ProviderError> {
    accounts::own(&request.account)?;
    let grant = &request.account;
    if !accounts::offer()
        .kind(&grant.kind)
        .is_some_and(|kind| kind.renews)
    {
        return Err(failure(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "Claude Code renews only a login it keeps in a configuration directory; this kind holds a credential that does not expire in a way that can be renewed",
        ));
    }
    let applied = accounts::plan(grant)?;
    let spawner = Arc::new(spawner);
    let files = Files::new(Arc::clone(&spawner));
    if !files.available() {
        return Err(failure(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this spawner serves no private files, which a login is kept in",
        ));
    }
    let dir = applied.dir.clone().unwrap_or_default();
    // The login is looked at as it is: a file the person or Claude Code lost is lost, and the grant's copy is not put
    // back in its place.
    let hold = accounts.observe(
        files,
        None,
        &grant.id,
        &dir,
        grant.revision,
        grant.credentials.expose(),
        applied.watched.clone(),
    )?;
    let lost = |hold: &coqui_acp_harness::account::Hold| {
        hold.look()?;
        match hold.lost() {
            Some(why) => Err(gone(&format!("the login is lost: {why}"))),
            None => Ok(()),
        }
    };
    lost(&hold)?;
    let before = hold.credentials()?;
    // A login newer than the grant (renewed by the harness since the host stored its copy) is what the host needs now,
    // whatever the window says: the copy it holds may have a spent refresh token.
    if before.expose().get(FILE_KEY) != grant.credentials.expose().get(FILE_KEY) {
        let expires_at = hold.expires_at();
        hold.finish();
        let mut report = RenewReport::renewed(before.expose().clone());
        report.expires_at = expires_at;
        return Ok(report);
    }
    let text = SecretText::new(
        before
            .expose()
            .get(FILE_KEY)
            .unwrap_or_default()
            .to_string(),
    );
    if let Some(ends) = accounts::expires_ms(&text) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|since| i64::try_from(since.as_millis()).ok())
            .unwrap_or(0);
        if ends - now > REFRESH_WINDOW_MS {
            return Ok(unchanged(
                grant,
                &format!(
                    "the login runs until {}, and Claude Code refreshes one only in its last five minutes",
                    rfc3339_ms(ends)
                ),
            ));
        }
    }
    let spawn = ThroughSpawner::shared(spawner);
    let checked = status(&spawn, executable, &applied.set, command_limit, None)?;
    lost(&hold)?;
    if !checked.exited_ok {
        return Err(internal("Claude Code could not check the login"));
    }
    let after = hold.credentials()?;
    let expires_at = hold.expires_at();
    hold.finish();
    if after.expose().get(FILE_KEY) == grant.credentials.expose().get(FILE_KEY) {
        return Ok(unchanged(grant, "Claude Code kept the login as it was"));
    }
    let mut report = RenewReport::renewed(after.expose().clone());
    report.expires_at = expires_at;
    Ok(report)
}

fn unchanged(grant: &coqui_acp_api::AccountGrant, why: &str) -> RenewReport {
    let mut report = RenewReport::unchanged(why);
    // The expiry of the copy the host holds, when its text says.
    report.expires_at = grant
        .credentials
        .expose()
        .get(FILE_KEY)
        .and_then(accounts::expires);
    report
}
