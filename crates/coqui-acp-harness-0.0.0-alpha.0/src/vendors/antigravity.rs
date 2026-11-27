//! Antigravity (`agy acp`), measured on 1.5.3 from its help text and binary strings only.
//!
//! Its login is kept in `<gemini-dir>/<app-data-dir>/antigravity-oauth-token` and, on Windows, in the Credential Manager
//! entry `gemini:antigravity`. Which one wins, whether `--gemini-dir` and `--app-data-dir` move the token, whether two
//! logins can exist at once, and what the sign-in `agy acp --login` prints were not measured, so the provider lists no
//! kind but `harness-login`. Its one way to sign in starts `agy acp` and opens a session, which says whether the person
//! is signed in; when they are not, it asks them to start Antigravity and sign in, then tries again.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use std::time::Instant;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::schema::{FieldBuilder, KindBuilder, SchemaBuilder};
use coqui_acp_api::{
    ASK, Credentials, HARNESS_LOGIN, LoginForm, LoginMethodInfo, LoginOutcome, LoginPrompter,
    LoginRequest, ProcessSpawner,
};
use coqui_acp_api::{Environment, SignedInCheck};
use serde_json::{Value, json};

use crate::access::{Access, Approvals, Part, Sandbox, Target, Translation};
use crate::cli::Next;
use crate::process::Launch;
use crate::stdio::accounts::HarnessDef;
use crate::stdio::login::{self, Ctx};
use crate::stdio::{StdioAccountSettings, StdioDef, StdioSettings};

/// The way to sign in: start the program and open a session.
pub const START: &str = "start";

/// How long the program is given to answer each of the two requests that open a session.
const ANSWER_WAIT: Duration = Duration::from_secs(30);

/// What Antigravity takes as an account.
pub static DEF: HarnessDef = HarnessDef {
    name: "Antigravity",
    program: "agy",
    args: &["acp"],
    kinds: &[],
    accepts: &[],
    own_standard: Some(coqui_acp_api::standard::ANTIGRAVITY),
    variables: &[],
    renew_args: None,

    login_unsupported: "agy signs in with `agy acp --login`, whose prompts were not run: it may write the Windows Credential Manager, which a throwaway profile does not isolate, and where it keeps the login and whether two logins can exist at once are unmeasured",
};

/// The ways to sign in.
#[must_use]
pub fn logins() -> Vec<LoginMethodInfo> {
    let mut start = LoginMethodInfo::new(START, "Start Antigravity to sign in", HARNESS_LOGIN);
    start.description = "Starts `agy acp` and opens a session to see whether you are signed in. If you are not, it asks you to start Antigravity, sign in there and continue.".to_string();
    start.steps = vec![ASK.to_string()];
    vec![start]
}

/// Runs the sign-in `request.method` names.
///
/// # Errors
///
/// As [`AccountProvider::login`](coqui_acp_api::AccountProvider::login).
pub fn sign_in(
    program: String,
    request: LoginRequest,
    spawner: Object<dyn ProcessSpawner>,
    prompter: Object<dyn LoginPrompter>,
) -> Result<LoginOutcome, ProviderError> {
    if request.method != START {
        return Err(ProviderError::new(
            Status::GUATIAO_ERR_BAD_VALUE,
            "this harness offers no such way to sign in",
        ));
    }
    let dir = request.state_dir.clone();
    login::run(spawner, prompter, request.timeout_ms, move |ctx| {
        loop {
            ctx.check()?;
            let Err(why) = open_session(ctx, &program, &dir)? else {
                break;
            };
            let form = LoginForm::new(
                "Start Antigravity to sign in",
                SchemaBuilder::new()
                    .field(
                        FieldBuilder::new("started", KindBuilder::bool())
                            .title("Antigravity is started and signed in")
                            .description(&alloc::format!(
                                "Antigravity did not open a session ({why}). Start Antigravity, sign in there, then continue."
                            )),
                    )
                    .finish()
                    .expect("the form schema builds"),
            );
            ctx.ask(form)?;
        }
        let mut outcome = LoginOutcome::new(HARNESS_LOGIN, Credentials::new());
        outcome.checked = true;
        Ok(outcome)
    })
}

/// Starts the program, opens a session in `dir` and ends it. The inner error is why no session opened.
fn open_session(
    ctx: &Ctx<'_>,
    program: &str,
    dir: &str,
) -> Result<Result<(), String>, ProviderError> {
    let child = ctx.spawn(&Launch::new(program).args(DEF.args.iter().copied()).cwd(dir))?;
    let cli = child.input();
    let mut heard = Heard::default();
    let opened = (|| {
        for (id, method, params) in [
            (
                1,
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {}}),
            ),
            (2, "session/new", json!({"cwd": dir, "mcpServers": []})),
        ] {
            let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            cli.write(alloc::format!("{line}\n").as_bytes())
                .map_err(|error| alloc::format!("it does not read its input ({error})"))?;
            let answer = heard.answer(ctx, &cli, id)?;
            if let Some(error) = answer.get("error") {
                return Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("it refused")
                    .to_string());
            }
        }
        Ok(())
    })();
    cli.kill();
    Ok(opened)
}

/// What a program has said that is not yet a whole line.
#[derive(Default)]
struct Heard {
    pending: Vec<u8>,
}

impl Heard {
    /// The answer to request `id`, waiting at most [`ANSWER_WAIT`]; the other lines are not for this.
    fn answer(&mut self, ctx: &Ctx<'_>, cli: &crate::cli::Cli, id: u64) -> Result<Value, String> {
        let until = Instant::now() + ANSWER_WAIT;
        loop {
            ctx.check().map_err(|_| "the sign-in ended".to_string())?;
            while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                if let Ok(value) = serde_json::from_slice::<Value>(&line)
                    && value.get("id").and_then(Value::as_u64) == Some(id)
                    && value.get("method").is_none()
                {
                    return Ok(value);
                }
            }
            if Instant::now() >= until {
                return Err("it did not answer".to_string());
            }
            match cli.next(Duration::from_millis(40)) {
                Next::Piece(piece) if !piece.error => self.pending.extend_from_slice(&piece.bytes),
                Next::Piece(_) | Next::Idle => {}
                Next::Ended => return Err("it ended before it answered".to_string()),
            }
        }
    }
}

/// Whether Antigravity has a login: an entry in the keyring (`gemini`, `antigravity`) or a file under `.gemini` below the home that
/// holds a whole sign-in, as [`AgyLogin`](crate::antigravity::AgyLogin) reads it, and nothing is refreshed or requested.
#[cfg(feature = "antigravity")]
fn signed_in(environment: &Environment, wait: Duration) -> SignedInCheck {
    signed_in_with(environment, wait, crate::antigravity::Keyring::Os)
}

/// The signed-in check with the keyring `keyring` names, so that a test need not read the operating system's.
#[cfg(feature = "antigravity")]
#[must_use]
pub fn signed_in_with(
    environment: &Environment,
    wait: Duration,
    keyring: crate::antigravity::Keyring,
) -> SignedInCheck {
    use crate::antigravity::{Cause, LoginSettings, Step, has_stored_login};
    let Some(home) = crate::signed_in::home(environment) else {
        return SignedInCheck::unknown(
            "the home folder Antigravity keeps its login under could not be worked out",
        );
    };
    match has_stored_login(
        &LoginSettings::new()
            .home(home)
            .keyring(keyring)
            .keyring_timeout(wait),
    ) {
        Ok(true) => SignedInCheck::signed_in(),
        Ok(false) => SignedInCheck::signed_out(
            "Antigravity's keyring entry and its login files under .gemini hold no whole sign-in",
        ),
        Err(error) => SignedInCheck::unknown(match (error.step, error.cause) {
            (Step::Keyring, Cause::Io(std::io::ErrorKind::TimedOut)) => {
                String::from("the keyring did not answer in time")
            }
            (Step::Keyring, _) => String::from("the keyring could not be read"),
            _ => String::from("a login file could not be read"),
        }),
    }
}

/// Without the `antigravity` feature nothing here can read Antigravity's keyring.
#[cfg(not(feature = "antigravity"))]
fn signed_in(_environment: &Environment, _wait: Duration) -> SignedInCheck {
    SignedInCheck::unknown(
        "this build cannot read Antigravity's login (the `antigravity` feature is off)",
    )
}

/// How the stdio provider runs it.
pub static STDIO: StdioDef = StdioDef {
    accounts: &DEF,
    signed_in,
    accounts_description: "Antigravity keeps its own login; this provider starts it to see who is signed in.",
    logins,
    sign_in: Some(sign_in),
    session_ids: None,
    import: None,
    export: None,
    import_unsupported: "Antigravity keeps a conversation as a database of its own protobuf steps, which it does not document, so it cannot be seeded with a history it did not make; set replay_max_bytes to replay the history as context in the first prompt",
    export_unsupported: "Antigravity keeps each conversation as a SQLite database of protobuf steps in its own program's folder (conversations/<id>.db), a schema it does not publish and changes between releases, so this provider does not read one; a host that records the sessions it serves exports them from its own record",
};

crate::stdio_vendor! {
    /// Antigravity, as a provider of `acp_harness` under the id `antigravity`: `agy acp`.
    harness: AntigravityProvider, id: "antigravity",
    /// Antigravity's accounts, as a provider of `account_provider` under the id `antigravity_accounts`.
    accounts: AntigravityAccounts, id: "antigravity_accounts",
    def: STDIO,
    settings: StdioSettings, account_settings: StdioAccountSettings,
}

/// Antigravity's access settings in its words, under the AOS harness id `antigravity`.
pub const TARGET: Target = Target::new("antigravity", translate);

const fn translate(access: Access) -> Translation {
    let sandbox = match access.sandbox {
        Sandbox::Workspace => Part::args(
            &["--sandbox"],
            "`agy acp --sandbox`, which restricts its terminal",
        ),
        Sandbox::ReadOnly => Part::args(
            &["--sandbox"],
            "`agy acp --sandbox`, which restricts its terminal; Antigravity does not say it forbids every write",
        ),
        _ => Part::said("`agy acp` without --sandbox: its terminal is not restricted"),
    };
    let network = if access.network {
        Part::cannot("Antigravity has no network setting: its own applies")
    } else {
        Part::cannot("Antigravity has no network setting")
    };
    let approvals = match access.approvals {
        Approvals::Never => Part::args(
            &["--dangerously-skip-permissions"],
            "`agy acp --dangerously-skip-permissions`",
        ),
        Approvals::Edits => Part::cannot("Antigravity has no accept-edits setting"),
        _ => Part::said("Antigravity asks before it runs a tool, as it does by default"),
    };
    Translation::new(sandbox, network, approvals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdio::accounts::{apply, catalog};
    use coqui_acp_api::standard;
    use coqui_acp_api::{AccountGrant, Credentials, HARNESS_LOGIN};

    #[test]
    fn the_table_is_well_formed() {
        assert_eq!(TARGET.well_formed(), Ok(()));
    }

    #[test]
    fn it_offers_one_way_to_sign_in_that_starts_the_program() {
        let methods = logins();
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].id, START);
        assert_eq!(methods[0].produces, HARNESS_LOGIN);
        assert_eq!(methods[0].title, "Start Antigravity to sign in");
        assert_eq!(
            catalog(&DEF).kinds.len(),
            1,
            "no kind but the program's own login"
        );
    }

    #[test]
    fn the_standard_access_adds_no_argument() {
        assert!(Access::STANDARD.translate(&TARGET).args().is_empty());
    }

    #[test]
    fn the_sandbox_and_never_approvals_are_arguments() {
        let mut access = Access::STANDARD;
        access.sandbox = Sandbox::Workspace;
        access.approvals = Approvals::Never;
        let agy = access.translate(&TARGET);
        assert_eq!(
            agy.args(),
            vec!["--sandbox", "--dangerously-skip-permissions"]
        );
        assert!(agy.env().is_empty());
        assert_eq!(agy.json(), None);
    }

    #[test]
    fn its_own_login_provides_its_standard_and_nothing_else_is_taken() {
        let catalog = catalog(&DEF);
        assert_eq!(catalog.kinds.len(), 1);
        assert_eq!(
            catalog.kind(HARNESS_LOGIN).expect("own").provides,
            [standard::ANTIGRAVITY]
        );
        assert!(catalog.accepts.is_empty());
        let grant = AccountGrant::new("a", "someone-elses-kind")
            .provides([standard::XAI_API])
            .credentials(Credentials::from_iter([("api_key", "sk-canary-3b7c")]))
            .state_dir("/some/dir");
        let Err(error) = apply(&DEF, &grant) else {
            panic!("a key was taken");
        };
        assert!(error.message().contains("none"), "{}", error.message());
    }
}
