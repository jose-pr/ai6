//! The checks, each answering what went wrong.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::path::{Path, PathBuf};
use std::time::Instant;

use guatiao::Secret;
use serde_json::{Value, json};

use super::Contract;
use super::host::{self, Conn, GRACE, PATIENCE, open, status};
use crate::usage::{LimitHit, Refill, Status, UsageQuery, UsageReport};
use crate::{
    AccountGrant, AcpHarness, Environment, ExportPage, FILES_UNDER, HARNESS_LOGIN, ImportReport,
    LIMIT_META_KEY, ModelsRequest, NoticeKind, Requirement, SessionExport, SessionImport,
    SessionQuery, StartRequest, USAGE_CAPABILITY, USAGE_METHOD, USAGE_NOTIFICATION, is_session_id,
};

// `guatiao_status` numbers, as a channel's `close` and `closed` carry them.
const OK: u32 = 0;
const BAD_VALUE: u32 = 7;
const GONE: u32 = 12;
const UNSUPPORTED: u32 = 13;
const NOT_FOUND: u32 = 10;

/// A value that must never come back from a provider: put in an environment, looked for in every
/// answer.
pub(super) const CANARY: &str = "coqui-acp-contract-canary-6b1d";

macro_rules! ensure {
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            return Err(format!($($arg)+));
        }
    };
}

pub(super) type Outcome = Result<(), String>;

pub(super) fn all(contract: &Contract, harness: &dyn AcpHarness) -> Vec<(&'static str, Outcome)> {
    let place = match Place::make() {
        Ok(place) => place,
        Err(why) => return vec![("the suite's directories", Err(why))],
    };
    let mut checks: Vec<(&'static str, Outcome)> = vec![
        ("describes itself", describes_itself(contract, harness)),
        ("requirements only narrow", requirements(contract, harness)),
        ("no environment value comes back", keeps_secrets(harness)),
        ("speaks ACP", speaks_acp(contract, harness, &place)),
        (
            "answers what is not a request",
            malformed(contract, harness, &place),
        ),
        (
            "seeds a session at the host's id",
            sessions(contract, harness, &place),
        ),
        (
            "reopens a session by its id",
            reopens(contract, harness, &place),
        ),
        ("a cancel ends the turn", cancel(contract, harness, &place)),
        (
            "an exit reaches the host as a notice",
            exit_is_noticed(contract, harness, &place),
        ),
        (
            "the host answers permission",
            permission(contract, harness, &place),
        ),
        (
            "asks only what the client offers",
            client_capabilities(contract, harness, &place),
        ),
        ("reports usage as it says", usage(contract, harness, &place)),
        ("holds a sender back", back_pressure(contract, harness)),
        (
            "close wakes a waiting send",
            close_wakes_send(contract, harness),
        ),
        (
            "close reaches an idle provider",
            close_reaches_idle(contract, harness),
        ),
        (
            "close answers what is pending",
            close_answers_pending(contract, harness, &place),
        ),
        (
            "the provider closes once",
            provider_closes(contract, harness, &place),
        ),
        ("a sink that has gone ends it", gone_sink(contract, harness)),
        (
            "working roots hold",
            working_roots(contract, harness, &place),
        ),
        (
            "a message over the limit ends it",
            message_limit(contract, harness),
        ),
        (
            "serves a real client",
            super::transport::serves_the_client(harness, &place.root),
        ),
    ];
    checks.extend(super::accounts::all(contract, harness, &place));
    place.remove();
    checks
}

/// The directories sessions are opened in: `root`, with `root/inside`, a sibling `outside`, and
/// `root/link` leading to `outside` when the platform let the suite make a link.
pub(super) struct Place {
    base: PathBuf,
    pub(super) root: String,
    inside: String,
    outside: String,
    beside: String,
    link: Option<String>,
}

impl Place {
    fn make() -> Result<Self, String> {
        let temp = std::fs::canonicalize(std::env::temp_dir())
            .map_err(|e| format!("no temporary directory: {e}"))?;
        let base = temp.join(format!("acp-harness-contract-{}", fresh_uuid()));
        let root = base.join("root");
        let outside = base.join("outside");
        for dir in [root.join("inside"), outside.clone()] {
            std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {dir:?}: {e}"))?;
        }
        let link = root.join("link");
        let linked = make_link(&outside, &link).then(|| text(&link));
        Ok(Self {
            root: text(&root),
            inside: text(&root.join("inside")),
            outside: text(&outside),
            beside: format!("{}-beside", text(&root)),
            link: linked,
            base,
        })
    }

    /// A new directory under the suite's own, for an account's files.
    pub(super) fn scratch(&self) -> Result<PathBuf, String> {
        let dir = self.base.join("accounts").join(fresh_uuid());
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {dir:?}: {e}"))?;
        Ok(dir)
    }

    fn remove(self) {
        if let Some(link) = &self.link {
            let _ = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link));
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// A path as text, without the `\\?\` prefix a canonical Windows path carries.
pub(super) fn text(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
}

#[cfg(unix)]
pub(super) fn make_link(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

/// A junction, which Windows lets anyone make, unlike a symbolic link.
#[cfg(windows)]
pub(super) fn make_link(target: &Path, link: &Path) -> bool {
    std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|out| out.status.success())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn make_link(_target: &Path, _link: &Path) -> bool {
    false
}

/// A grant of the kind every provider lists, which hands nothing over: for the checks that need an
/// account only to name it.
pub(super) fn grant(id: &str) -> AccountGrant {
    AccountGrant::new(id, HARNESS_LOGIN)
}

/// A version 4 UUID no other call picks: the clock and the process, hashed with keys `RandomState`
/// draws afresh for each half. Unique, not secret.
pub(super) fn fresh_uuid() -> String {
    use core::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut bytes = [0u8; 16];
    for half in bytes.chunks_mut(8) {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        hasher.write_u32(std::process::id());
        half.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

pub(super) fn initialize(id: u64, version: u16, client_capabilities: Value) -> Value {
    request(
        id,
        "initialize",
        json!({"protocolVersion": version, "clientCapabilities": client_capabilities}),
    )
}

pub(super) fn new_session(id: u64, cwd: &str) -> Value {
    request(id, "session/new", json!({"cwd": cwd, "mcpServers": []}))
}

fn prompt(id: u64, session: &str, text: &str) -> Value {
    request(
        id,
        "session/prompt",
        json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]}),
    )
}

fn code(message: &Value) -> Option<i64> {
    message["error"]["code"].as_i64()
}

/// Opens a connection, initializes it and opens one session in `cwd`.
fn open_session(
    contract: &Contract,
    harness: &dyn AcpHarness,
    request: StartRequest,
    cwd: &str,
    capabilities: Value,
) -> Result<(Conn, String), String> {
    open_session_in(contract, harness, request, cwd, capabilities, None)
}

/// As [`open_session`], on a spawner that serves files in `root` when there is one.
pub(super) fn open_session_in(
    contract: &Contract,
    harness: &dyn AcpHarness,
    request: StartRequest,
    cwd: &str,
    capabilities: Value,
    root: Option<&Path>,
) -> Result<(Conn, String), String> {
    let mut conn = match root {
        Some(root) => host::open_in(harness, request, root)?,
        None => open(harness, request)?,
    };
    conn.send(&initialize(0, contract.version, capabilities))?;
    let answer = conn.answer(0)?;
    ensure!(
        answer["result"]["protocolVersion"] == json!(contract.version),
        "initialize was answered {answer}"
    );
    conn.send(&new_session(1, cwd))?;
    let answer = conn.answer(1)?;
    let id = answer["result"]["sessionId"]
        .as_str()
        .ok_or_else(|| format!("session/new was answered {answer}"))?
        .to_owned();
    ensure!(is_session_id(&id), "the session id {id:?} is not a UUID");
    Ok((conn, id))
}

/// The text of every chunk until request `id` is answered, and its answer. An agent's request in
/// between is an error.
fn turn(conn: &mut Conn, id: u64) -> Result<(String, Value), String> {
    let mut text = String::new();
    loop {
        let message = conn.next()?;
        if message.get("method") == Some(&json!("session/update")) {
            text.push_str(&said_by(&message));
            continue;
        }
        if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
            return Ok((text, message));
        }
        return Err(format!("the agent sent {message} during the turn"));
    }
}

/// The text of an `agent_message_chunk` update; any other update says nothing the suite reads.
fn said_by(update: &Value) -> String {
    let update = &update["params"]["update"];
    if update["sessionUpdate"] == "agent_message_chunk" {
        update["content"]["text"].as_str().unwrap_or("").to_owned()
    } else {
        String::new()
    }
}

fn describes_itself(contract: &Contract, harness: &dyn AcpHarness) -> Outcome {
    let info = harness.describe().map_err(|e| e.message().to_owned())?;
    ensure!(!info.title.is_empty(), "no title");
    ensure!(
        info.versions.contains(&contract.version),
        "describe lists ACP versions {:?}, not {}",
        info.versions,
        contract.version
    );
    harness.program().map_err(|e| e.message().to_owned())?;
    let (_, spawner, keeper) = host::spawner();
    harness
        .models(ModelsRequest::new(), spawner, keeper)
        .map_err(|e| e.message().to_owned())?;
    harness
        .session_record(SessionQuery::new(fresh_uuid(), "/"))
        .map_err(|e| e.message().to_owned())?;
    harness.non_defaults().map_err(|e| e.message().to_owned())?;
    let (watch, spawner, keeper) = host::spawner();
    let report = harness
        .usage(UsageQuery::new(), spawner, keeper)
        .map_err(|e| e.message().to_owned())?;
    ensure!(
        watch.destroyed(),
        "usage answered with the spawner it was handed alive"
    );
    ensure!(
        report.supported == info.usage.reports(),
        "describe says it reports {} usage, and `usage` answers `supported: {}`",
        if info.usage.reports() { "some" } else { "no" },
        report.supported
    );
    if !report.supported {
        ensure!(
            report
                .unsupported_because
                .as_deref()
                .is_some_and(|why| !why.is_empty())
                && report.buckets.is_empty()
                && report.limit.is_none()
                && report.spend.is_none(),
            "an unsupported report carries no reason, or carries figures: {report:?}"
        );
    }
    Ok(())
}

/// Answers whether `start` refused `request` with `expected`, both objects destroyed.
fn refused_with(harness: &dyn AcpHarness, request: StartRequest, expected: u32) -> Outcome {
    match host::start(harness, request) {
        Ok(conn) => {
            conn.channel().close(OK, "");
            Err(format!("started where it had to refuse with {expected}"))
        }
        Err((error, conn)) => {
            ensure!(
                status(&error) == expected,
                "refused with {} ({}), not {expected}",
                status(&error),
                error.message()
            );
            conn.destroyed()
        }
    }
}

fn requirements(contract: &Contract, harness: &dyn AcpHarness) -> Outcome {
    let refusal = |requirements: Vec<Requirement>| {
        harness
            .refusal(requirements, Secret::default(), None)
            .map_err(|e| status(&e))
    };
    let with = |requirements: &[Requirement]| {
        let mut request = StartRequest::new();
        request.requirements = requirements.to_vec();
        request
    };
    for held in &contract.holds {
        ensure!(
            refusal(vec![held.clone()]) == Ok(None),
            "refusal of {} with no environment was not None",
            held.name
        );
        open(harness, with(core::slice::from_ref(held)))?.close_with(OK)?;
    }
    ensure!(
        refusal(contract.holds.clone()) == Ok(None),
        "the requirements it holds were refused together"
    );
    open(harness, with(&contract.holds))?.close_with(OK)?;

    let unknown = Requirement::new("no_such_requirement");
    ensure!(
        matches!(refusal(vec![unknown.clone()]), Ok(Some(_))),
        "a requirement it does not know was not refused"
    );
    let mut asked = contract.holds.clone();
    asked.push(unknown);
    ensure!(
        matches!(refusal(asked.clone()), Ok(Some(_))),
        "a requirement it does not know, beside ones it holds, was not refused"
    );
    refused_with(harness, with(&asked), UNSUPPORTED)?;

    let unreadable: Vec<(&str, Vec<Requirement>)> = vec![
        (
            "a path that is not absolute",
            vec![Requirement::from(crate::Narrowing::FilesUnder(vec![
                "relative".into(),
            ]))],
        ),
        ("a list given no value", vec![Requirement::new(FILES_UNDER)]),
        (
            "a value given to a name that takes none",
            vec![Requirement::with_value(crate::READ_ONLY, "yes")],
        ),
        (
            "a name given twice",
            vec![
                Requirement::new(crate::NO_COMMANDS),
                Requirement::new(crate::NO_COMMANDS),
            ],
        ),
    ];
    for (what, requirements) in unreadable {
        ensure!(
            refusal(requirements.clone()) == Err(BAD_VALUE),
            "refusal of {what} was not GUATIAO_ERR_BAD_VALUE"
        );
        refused_with(harness, with(&requirements), BAD_VALUE)
            .map_err(|why| format!("{what}: {why}"))?;
    }
    Ok(())
}

fn keeps_secrets(harness: &dyn AcpHarness) -> Outcome {
    let environment: Environment = [("COQUI_ACP_CONTRACT_SECRET", CANARY)]
        .into_iter()
        .collect();
    let secret = || Secret::new(environment.clone());
    let unknown = vec![Requirement::new("no_such_requirement")];
    let answered = format!(
        "{:?} {:?} {:?} {:?}",
        harness.refusal(unknown.clone(), secret(), None),
        harness.models(
            ModelsRequest::new().environment(environment.clone()),
            host::spawner().1,
            host::spawner().2
        ),
        harness
            .session_record(SessionQuery::new(fresh_uuid(), "/").environment(environment.clone())),
        harness.usage(
            UsageQuery::new().environment(environment.clone()),
            host::spawner().1,
            host::spawner().2
        ),
    );
    ensure!(
        !answered.contains(CANARY),
        "an answer carried an environment value"
    );
    let request = StartRequest::builder()
        .environment(environment.clone())
        .requirement(Requirement::new("no_such_requirement"))
        .build();
    match host::start(harness, request) {
        Ok(conn) => {
            conn.channel().close(OK, "");
            Err("started with a requirement it does not know".to_owned())
        }
        Err((error, conn)) => {
            ensure!(
                !error.message().contains(CANARY),
                "a refusal carried an environment value"
            );
            conn.destroyed()
        }
    }
}

fn speaks_acp(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let (mut conn, session) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    conn.send(&prompt(2, &session, "hello"))?;
    // A harness may tell more than its words (a title, its commands, a usage figure), before or after them.
    let update = loop {
        let update = conn.next()?;
        ensure!(
            update["method"] == "session/update" && update["params"]["sessionId"] == json!(session),
            "a prompt's first message was {update}"
        );
        if update["params"]["update"]["sessionUpdate"] == "agent_message_chunk" {
            break update;
        }
    };
    ensure!(
        update["params"]["update"]["content"]["text"] == "echo: hello",
        "the prompt was echoed as {update}"
    );
    let answer = conn.answer(2)?;
    ensure!(
        answer["result"]["stopReason"] == "end_turn",
        "the turn ended {answer}"
    );
    conn.close_with(OK)
}

fn malformed(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let (mut conn, _) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    let cases: [(&str, Vec<u8>, i64, Value); 8] = [
        ("text that is not JSON", b"not JSON".to_vec(), -32700, Value::Null),
        (
            "a batch",
            br#"[{"jsonrpc":"2.0","id":3,"method":"initialize","params":{}}]"#.to_vec(),
            -32600,
            Value::Null,
        ),
        ("a number", b"5".to_vec(), -32600, Value::Null),
        (
            "a method that is not text",
            br#"{"jsonrpc":"2.0","id":4,"method":5}"#.to_vec(),
            -32600,
            json!(4),
        ),
        (
            "params that are a list",
            br#"{"jsonrpc":"2.0","id":5,"method":"session/new","params":[]}"#.to_vec(),
            -32602,
            json!(5),
        ),
        (
            "a cwd that is not text",
            br#"{"jsonrpc":"2.0","id":6,"method":"session/new","params":{"cwd":5,"mcpServers":[]}}"#
                .to_vec(),
            -32602,
            json!(6),
        ),
        (
            "a prompt that is not a list",
            br#"{"jsonrpc":"2.0","id":7,"method":"session/prompt","params":{"sessionId":"x","prompt":"hi"}}"#
                .to_vec(),
            -32602,
            json!(7),
        ),
        (
            "a method nobody has",
            br#"{"jsonrpc":"2.0","id":8,"method":"no/such","params":{}}"#.to_vec(),
            -32601,
            json!(8),
        ),
    ];
    for (what, bytes, expected, id) in cases {
        conn.send_bytes(&bytes)?;
        // A harness may tell a session's commands, its title or its options right after `session/new`: not an answer.
        let answer = loop {
            let message = conn.next()?;
            if message.get("method") != Some(&json!("session/update")) {
                break message;
            }
        };
        ensure!(
            code(&answer) == Some(expected) && answer["id"] == id,
            "{what} was answered {answer}, not {expected} for id {id}"
        );
    }
    conn.send(&notification("no/such", json!({})))?;
    conn.send(&initialize(9, contract.version, json!({})))?;
    let answer = conn.next()?;
    ensure!(
        answer["id"] == 9,
        "a notification nobody has was answered: {answer}"
    );
    conn.close_with(OK)
}

/// A history in the standard format's shape whose header names `version`.
fn history_of_version(version: u64) -> String {
    format!("{{\"format\":\"coqui-acp-history\",\"version\":{version}}}\n")
}

/// Seeds `session` from `history`, checking the spawner it was handed is gone when it answers.
fn import(
    harness: &dyn AcpHarness,
    session: &str,
    cwd: &str,
    history: &str,
) -> Result<Result<ImportReport, (u32, String)>, String> {
    let (watch, spawner, keeper) = host::spawner();
    let answer = harness.import_session(SessionImport::new(session, cwd, history), spawner, keeper);
    ensure!(
        watch.destroyed(),
        "import_session answered with the spawner it was handed alive"
    );
    Ok(answer.map_err(|e| (status(&e), e.message().to_owned())))
}

/// The history of `session`, checking the spawner it was handed is gone when it answers.
fn export(
    harness: &dyn AcpHarness,
    session: &str,
    cwd: &str,
) -> Result<Result<String, u32>, String> {
    Ok(match export_page(harness, session, cwd, None, u64::MAX)? {
        Ok(page) => {
            ensure!(
                page.next.is_none(),
                "an export with no bound on its page came in more than one"
            );
            Ok(page.text)
        }
        Err((code, _)) => Err(code),
    })
}

/// One page of the history of `session`, checking the spawner it was handed is gone when it answers.
fn export_page(
    harness: &dyn AcpHarness,
    session: &str,
    cwd: &str,
    after: Option<&str>,
    max_bytes: u64,
) -> Result<Result<ExportPage, (u32, String)>, String> {
    let (watch, spawner, keeper) = host::spawner();
    let mut asked = SessionExport::new(session, cwd).max_bytes(max_bytes);
    if let Some(after) = after {
        asked = asked.after(after);
    }
    let answer = harness.export_session(asked, spawner, keeper);
    ensure!(
        watch.destroyed(),
        "export_session answered with the spawner it was handed alive"
    );
    Ok(answer.map_err(|e| (status(&e), e.message().to_owned())))
}

/// Whether `line` is a history's header: an object naming the format and its version.
fn is_header(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .is_ok_and(|value| value.get("format").is_some() && value["version"].is_u64())
}

/// Pages `session`'s history at `max_bytes`, checking each page is whole lines.
fn pages(
    harness: &dyn AcpHarness,
    session: &str,
    cwd: &str,
    max_bytes: u64,
) -> Result<Vec<ExportPage>, String> {
    let mut pages: Vec<ExportPage> = Vec::new();
    loop {
        let after = pages.last().and_then(|page| page.next.clone());
        let page = export_page(harness, session, cwd, after.as_deref(), max_bytes)?.map_err(
            |(code, why)| format!("page {} was refused with {code}: {why}", pages.len() + 1),
        )?;
        ensure!(
            !page.text.is_empty() && page.text.ends_with('\n'),
            "page {} is not whole lines",
            pages.len() + 1
        );
        let done = page.next.is_none();
        pages.push(page);
        if done {
            return Ok(pages);
        }
        ensure!(pages.len() < 10_000, "the pages never end");
    }
}

/// The text a reopened session replays, and the answer to the reopening request `id`.
fn reopened(conn: &mut Conn, id: u64) -> Result<(String, Value), String> {
    turn(conn, id)
}

fn sessions(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    if !contract.imports {
        let refused = import(harness, &fresh_uuid(), &place.root, "")?;
        return match refused {
            Err((UNSUPPORTED, why)) if !why.trim().is_empty() => Ok(()),
            Err((code, why)) => Err(format!(
                "a harness that cannot import refused with {code} ({why:?}), not UNSUPPORTED with a reason"
            )),
            Ok(_) => Err("a harness said to import nothing seeded a session".to_owned()),
        };
    }
    let takes = harness
        .describe()
        .map_err(|e| e.message().to_owned())?
        .takes;
    let method = match (takes.resume, takes.load) {
        (true, _) => "session/resume",
        (false, true) => "session/load",
        (false, false) => {
            return Err(
                "a harness that imports takes neither session/resume nor session/load".into(),
            );
        }
    };
    let reopen = |id: u64, method: &str, session: &str| {
        request(
            id,
            method,
            json!({"sessionId": session, "cwd": place.root, "mcpServers": []}),
        )
    };

    // Seeded empty, then resumed by the host's UUID.
    let seeded = fresh_uuid();
    let report = import(harness, &seeded, &place.root, "")?
        .map_err(|(code, why)| format!("an empty seed was refused with {code}: {why}"))?;
    ensure!(
        report.seeded_from.is_none(),
        "an empty seed said it started at {:?}",
        report.seeded_from
    );
    let mut conn = open(harness, StartRequest::new())?;
    conn.send(&initialize(0, contract.version, json!({})))?;
    conn.answer(0)?;
    conn.send(&reopen(1, method, &seeded))?;
    let (_, answer) = reopened(&mut conn, 1)?;
    ensure!(
        code(&answer).is_none(),
        "{method} of a seeded session was answered {answer}"
    );
    conn.send(&prompt(2, &seeded, "seeded"))?;
    let (text, _) = turn(&mut conn, 2)?;
    ensure!(
        text == "echo: seeded",
        "the seeded session answered {text:?}"
    );
    conn.close_with(OK)?;

    // Turns, exported, imported at another UUID, resumed there with the turns in it.
    let (mut conn, origin) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    conn.send(&prompt(2, &origin, "first"))?;
    turn(&mut conn, 2)?;
    conn.close_with(OK)?;
    let history = export(harness, &origin, &place.root)?
        .map_err(|code| format!("the history of a session it reported was refused with {code}"))?;
    let header: Value = history
        .lines()
        .next()
        .and_then(|line| serde_json::from_str(line).ok())
        .unwrap_or(Value::Null);
    ensure!(
        header["version"].is_u64(),
        "an exported history's first line is no header with a version: {header}"
    );
    ensure!(
        history.contains("echo: first"),
        "the exported history lacks the session's turn"
    );
    let moved = fresh_uuid();
    let report = import(harness, &moved, &place.root, &history)?
        .map_err(|(code, why)| format!("its own export was refused with {code}: {why}"))?;
    ensure!(
        report.seeded_from.is_some(),
        "a seed from a history said it started nowhere"
    );
    let mut conn = open(harness, StartRequest::new())?;
    conn.send(&initialize(0, contract.version, json!({})))?;
    conn.answer(0)?;
    if takes.load {
        conn.send(&reopen(1, "session/load", &moved))?;
        let (replayed, answer) = reopened(&mut conn, 1)?;
        ensure!(
            code(&answer).is_none(),
            "session/load of an imported session was answered {answer}"
        );
        ensure!(
            replayed.contains("echo: first"),
            "session/load of an imported session replayed {replayed:?}"
        );
    } else {
        conn.send(&reopen(1, method, &moved))?;
        let (_, answer) = reopened(&mut conn, 1)?;
        ensure!(
            code(&answer).is_none(),
            "{method} of an imported session was answered {answer}"
        );
    }
    conn.send(&prompt(2, &moved, "second"))?;
    let (text, _) = turn(&mut conn, 2)?;
    ensure!(
        text == "echo: second",
        "the imported session answered {text:?}"
    );
    conn.close_with(OK)?;
    let again = export(harness, &moved, &place.root)?
        .map_err(|code| format!("the history of an imported session was refused with {code}"))?;
    ensure!(
        again.contains("echo: first") && again.contains("echo: second"),
        "an imported session's history lacks a turn"
    );

    // Paged: the pages join to the whole, only the first starts with the header, which carries a
    // digest of the whole; a page too small for its next line is refused, saying the line's size.
    // Pages as small as the longest line allows, so the history comes in several.
    let bound = again
        .lines()
        .map(|line| line.len() as u64 + 1)
        .max()
        .unwrap_or(1);
    let paged = pages(harness, &moved, &place.root, bound)?;
    ensure!(
        paged.len() > 1,
        "pages of {bound} bytes, its longest line, of a history of two turns came as one"
    );
    let joined: String = paged.iter().map(|page| page.text.as_str()).collect();
    ensure!(joined == again, "the pages do not join to the whole export");
    let header: Value = paged[0]
        .text
        .lines()
        .next()
        .and_then(|line| serde_json::from_str(line).ok())
        .unwrap_or(Value::Null);
    ensure!(
        is_header(&header.to_string()) && header["digest"].is_string(),
        "the first page does not start with a header carrying a digest: {header}"
    );
    ensure!(
        paged[1..]
            .iter()
            .all(|page| !page.text.lines().any(is_header)),
        "a page after the first carries a header"
    );
    let header_bytes = again.lines().next().map_or(0, |line| line.len() as u64 + 1);
    match export_page(harness, &moved, &place.root, None, header_bytes - 1)? {
        Err((BAD_VALUE, why)) => ensure!(
            crate::line_too_large(&why) == Some(header_bytes),
            "a page too small for its first line ({header_bytes} bytes) was refused saying {why:?}"
        ),
        Err((code, why)) => {
            return Err(format!(
                "a page too small for its first line was refused with {code} ({why}), not BAD_VALUE"
            ));
        }
        Ok(page) => {
            return Err(format!(
                "a page of {} bytes came, over a bound of {}",
                page.text.len(),
                header_bytes - 1
            ));
        }
    }
    for page in &paged {
        ensure!(
            page.text.len() as u64 <= bound,
            "a page bound to {bound} bytes held {}",
            page.text.len()
        );
    }

    // An `after` the provider never gave, or one the session has moved past, is refused.
    let unknown = export_page(
        harness,
        &moved,
        &place.root,
        Some("not a position it gave"),
        bound,
    )?;
    ensure!(
        unknown.as_ref().err().map(|(code, _)| *code) == Some(BAD_VALUE),
        "an unknown `after` was answered {:?}",
        unknown.map(|page| page.next)
    );
    let stale = paged[0].next.clone().unwrap_or_default();
    let mut conn = open(harness, StartRequest::new())?;
    conn.send(&initialize(0, contract.version, json!({})))?;
    conn.answer(0)?;
    conn.send(&reopen(1, method, &moved))?;
    reopened(&mut conn, 1)?;
    conn.send(&prompt(2, &moved, "third"))?;
    turn(&mut conn, 2)?;
    conn.close_with(OK)?;
    let changed = export_page(harness, &moved, &place.root, Some(&stale), bound)?;
    ensure!(
        changed.as_ref().err().map(|(code, _)| *code) == Some(BAD_VALUE),
        "an `after` from before the session changed was answered {:?}",
        changed.map(|page| page.next)
    );

    // What an import and an export refuse.
    let refusals: [(&str, String, String, u32); 4] = [
        (
            "an id that names a session",
            moved.clone(),
            String::new(),
            BAD_VALUE,
        ),
        (
            "an id that is not a UUID",
            "not-a-uuid".into(),
            String::new(),
            BAD_VALUE,
        ),
        (
            "a history version it does not read",
            fresh_uuid(),
            history_of_version(999),
            UNSUPPORTED,
        ),
        (
            "a history that is not JSON lines",
            fresh_uuid(),
            "not JSON\n".into(),
            BAD_VALUE,
        ),
    ];
    for (what, session, history, expected) in refusals {
        match import(harness, &session, &place.root, &history)? {
            Err((code, _)) if code == expected => {}
            Err((code, why)) => {
                return Err(format!(
                    "{what} was refused with {code} ({why}), not {expected}"
                ));
            }
            Ok(_) => return Err(format!("{what} was seeded")),
        }
    }
    let unknown = export(harness, &fresh_uuid(), &place.root)?;
    ensure!(
        unknown == Err(NOT_FOUND),
        "the history of a session nobody made was answered {unknown:?}"
    );
    Ok(())
}

fn reopens(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let takes = harness
        .describe()
        .map_err(|e| e.message().to_owned())?
        .takes;
    let methods: Vec<&str> = [
        ("session/load", takes.load),
        ("session/resume", takes.resume),
    ]
    .into_iter()
    .filter_map(|(method, taken)| taken.then_some(method))
    .collect();
    for method in methods {
        let (mut conn, chosen) = open_session(
            contract,
            harness,
            StartRequest::new(),
            &place.root,
            json!({}),
        )?;
        if contract.program_agent {
            // A harness's program keeps a session from its first turn on.
            conn.send(&prompt(2, &chosen, "first"))?;
            turn(&mut conn, 2)?;
        }
        conn.close_with(OK)?;

        let mut conn = open(harness, StartRequest::new())?;
        conn.send(&initialize(0, contract.version, json!({})))?;
        conn.answer(0)?;
        let reopen = |id: u64, session: &str, cwd: &str| {
            request(
                id,
                method,
                json!({"sessionId": session, "cwd": cwd, "mcpServers": []}),
            )
        };
        conn.send(&reopen(1, &chosen, &place.root))?;
        let answer = conn.answer(1)?;
        ensure!(
            code(&answer).is_none(),
            "{method} of a known session was answered {answer}"
        );
        conn.send(&prompt(2, &chosen, "again"))?;
        let (text, _) = turn(&mut conn, 2)?;
        ensure!(
            text == "echo: again",
            "the reopened session answered {text:?}"
        );
        conn.send(&reopen(3, &fresh_uuid(), &place.root))?;
        let answer = conn.answer(3)?;
        ensure!(
            code(&answer).is_some(),
            "{method} of an unknown session was answered {answer}"
        );
        conn.close_with(OK)?;

        let mut conn = open(
            harness,
            StartRequest::builder()
                .working_root(place.inside.clone())
                .build(),
        )?;
        conn.send(&initialize(0, contract.version, json!({})))?;
        conn.answer(0)?;
        conn.send(&reopen(1, &chosen, &place.root))?;
        let answer = conn.answer(1)?;
        ensure!(
            code(&answer) == Some(-32602),
            "{method} outside the working roots was answered {answer}"
        );
        conn.close_with(OK)?;
    }
    Ok(())
}

fn cancel(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let (mut conn, session) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    conn.send(&prompt(2, &session, "wait"))?;
    ensure!(conn.quiet()?, "a waiting turn answered before any cancel");
    conn.send(&notification(
        "session/cancel",
        json!({"sessionId": session}),
    ))?;
    let (_, answer) = turn(&mut conn, 2)?;
    ensure!(
        answer["result"]["stopReason"] == "cancelled",
        "a cancelled turn ended {answer}"
    );
    conn.close_with(OK)
}

/// A program that ends mid-session is told to the host: through the spawner's `notice`, in one line that says how it
/// ended and holds nothing it wrote, or, for a provider that relays it, by ending the connection with a failure. A
/// provider with no program of its own has none to end.
fn exit_is_noticed(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    if !contract.program_agent {
        return Ok(());
    }
    let (conn, session) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    conn.send(&prompt(2, &session, "die"))?;
    let Some(text) = conn.told_of_exit(NoticeKind::Exited)? else {
        // The provider relays the program, so its end is the connection's: told by `closed` with a failure.
        conn.channel().close(OK, "");
        return conn.destroyed();
    };
    ensure!(
        !text.contains('\n') && !text.contains("its last output"),
        "the exit notice was not one line of how it ended: {text:?}"
    );
    conn.close_with(OK)
}

fn permission(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let (mut conn, session) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    let selected =
        |option: &str| json!({"result": {"outcome": {"outcome": "selected", "optionId": option}}});
    // Which option allows once and which rejects once is the option's kind; its id is the harness's own. `None` for the
    // replies that choose no offered option.
    let cases = [
        ("allow", Some("allow_once"), "echo: allowed", "end_turn"),
        ("reject", Some("reject_once"), "echo: refused", "end_turn"),
        ("an error", None, "echo: refused", "end_turn"),
        (
            "an outcome it does not know",
            None,
            "echo: refused",
            "end_turn",
        ),
        (
            "a selected option it never offered",
            None,
            "echo: refused",
            "end_turn",
        ),
        (
            "an allow option it never offered",
            None,
            "echo: refused",
            "end_turn",
        ),
        ("cancelled", None, "", "cancelled"),
    ];
    for (id, (what, kind, text, stop)) in (10u64..).zip(cases) {
        conn.send(&prompt(id, &session, "ask"))?;
        let asked = loop {
            let message = conn.next()?;
            if message.get("method") == Some(&json!("session/update")) {
                continue;
            }
            break message;
        };
        ensure!(
            asked["method"] == "session/request_permission"
                && asked["params"]["sessionId"] == json!(session)
                && asked.get("id").is_some(),
            "a prompt of `ask` sent {asked}"
        );
        let options = asked["params"]["options"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let option_of = |kind: &str| {
            options
                .iter()
                .find(|o| o["kind"] == kind)
                .and_then(|o| o["optionId"].as_str())
                .map(str::to_owned)
        };
        ensure!(
            option_of("allow_once").is_some() && option_of("reject_once").is_some(),
            "the permission offered {options:?}, with no option of kind allow_once and one of kind reject_once"
        );
        let reply = match (what, kind) {
            (_, Some(kind)) => selected(&option_of(kind).unwrap_or_default()),
            ("an error", _) => json!({"error": {"code": -32603, "message": "the host failed"}}),
            ("an outcome it does not know", _) => {
                json!({"result": {"outcome": {"outcome": "maybe"}}})
            }
            ("a selected option it never offered", _) => selected("never-offered-option"),
            ("an allow option it never offered", _) => {
                // The id of an option that allows, as harnesses spell them, which this question did not put.
                let put: Vec<&str> = options
                    .iter()
                    .filter_map(|o| o["optionId"].as_str())
                    .collect();
                let absent = [
                    "allow",
                    "allow_always",
                    "accept",
                    "accept_always",
                    "accept_for_session",
                ]
                .into_iter()
                .find(|id| !put.contains(id))
                .unwrap_or("allow-never-offered");
                selected(absent)
            }
            _ => json!({"result": {"outcome": {"outcome": "cancelled"}}}),
        };
        let mut answer = reply;
        answer["jsonrpc"] = json!("2.0");
        answer["id"] = asked["id"].clone();
        conn.send(&answer)?;
        let (said, end) = turn(&mut conn, id)?;
        let stopped = end["result"]["stopReason"].as_str().unwrap_or("");
        ensure!(
            said == text && stopped == stop,
            "answered {what}, the turn said {said:?} and ended {end}"
        );
    }
    conn.close_with(OK)
}

/// The agent's capabilities in an `initialize` answer: version 1's `agentCapabilities`, version 2's
/// `capabilities`.
fn capabilities_of(answer: &Value, version: u16) -> &Value {
    if version >= 2 {
        &answer["result"]["capabilities"]
    } else {
        &answer["result"]["agentCapabilities"]
    }
}

/// Every message until request `id` is answered: the usage notifications it saw, the text said and
/// the answer. Anything else from the agent is an error.
fn turn_with_usage(conn: &mut Conn, id: u64) -> Result<(String, Value, Vec<Value>), String> {
    let mut text = String::new();
    let mut told = Vec::new();
    loop {
        let message = conn.next()?;
        match message.get("method").and_then(Value::as_str) {
            Some("session/update") => text.push_str(&said_by(&message)),
            Some(USAGE_NOTIFICATION) => told.push(message),
            Some(_) => return Err(format!("the agent sent {message} during the turn")),
            None if message.get("id") == Some(&json!(id)) => return Ok((text, message, told)),
            None => return Err(format!("the agent sent {message} during the turn")),
        }
    }
}

/// Whether two reports hold the same figures, whenever each was read.
fn same_figures(a: &UsageReport, b: &UsageReport) -> bool {
    let plain = |report: &UsageReport| {
        let mut report = report.clone();
        report.observed_at = 0;
        for bucket in &mut report.buckets {
            bucket.observed_at = 0;
        }
        report
    };
    plain(a) == plain(b)
}

/// Whether two reports hold the same buckets and the same limit, whatever their revisions.
fn same_buckets(a: &UsageReport, b: &UsageReport) -> bool {
    let plain = |report: &UsageReport| {
        let mut buckets = report.buckets.clone();
        for bucket in &mut buckets {
            bucket.observed_at = 0;
        }
        buckets.sort_by(|x, y| x.id.cmp(&y.id));
        (
            buckets,
            report.limit.clone().map(|hit| (hit.bucket, hit.resets_at)),
        )
    };
    plain(a) == plain(b)
}

/// Asks `_coqui_acp/usage` and reads the report.
fn usage_report(conn: &mut Conn, id: u64, session: Option<&str>) -> Result<UsageReport, String> {
    let params = session.map_or_else(|| json!({}), |session| json!({"sessionId": session}));
    conn.send(&request(id, USAGE_METHOD, params))?;
    let answer = conn.answer(id)?;
    ensure!(
        answer.get("error").is_none(),
        "{USAGE_METHOD} was answered {answer}"
    );
    serde_json::from_value(answer["result"].clone())
        .map_err(|e| format!("{USAGE_METHOD} answered no report ({e}): {answer}"))
}

/// What `usage` says a harness reports is what it does: nothing advertised and `-32601` for one that
/// reports none; for one that does (the contract agent scripts it: `quota P`, `limit`, fixed spend
/// each turn), the figures, the revisions, the notification only a client that asked gets, the
/// account, and the kind's answer agreeing with the connection's.
fn usage(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let info = harness.describe().map_err(|e| e.message().to_owned())?;
    let optin =
        json!({"_meta": {USAGE_CAPABILITY: {"notify": true, "account": "from-the-client"}}});
    if !info.usage.reports() {
        // Nothing advertised, `-32601`, and the scripted prompts are plain text.
        let mut conn = open(
            harness,
            StartRequest::builder().account(grant("acct-1")).build(),
        )?;
        conn.send(&initialize(0, contract.version, optin))?;
        let answer = conn.answer(0)?;
        ensure!(
            capabilities_of(&answer, contract.version)["_meta"]
                .get(USAGE_CAPABILITY)
                .is_none(),
            "a harness that reports no usage advertised it: {answer}"
        );
        conn.send(&request(1, USAGE_METHOD, json!({})))?;
        let answer = conn.answer(1)?;
        ensure!(
            code(&answer) == Some(-32601),
            "{USAGE_METHOD} of a harness that reports none was answered {answer}"
        );
        conn.send(&new_session(2, &place.root))?;
        let session = conn.answer(2)?["result"]["sessionId"]
            .as_str()
            .ok_or("session/new gave no id")?
            .to_owned();
        conn.send(&prompt(3, &session, "quota 620"))?;
        let (said, _) = turn(&mut conn, 3)?;
        ensure!(
            said == "echo: quota 620",
            "a harness that reports none scripted `quota`: {said:?}"
        );
        return conn.close_with(OK);
    }

    // A harness that reports: the capability agrees with `describe`.
    let account = "acct-1";
    let mut conn = open(
        harness,
        StartRequest::builder().account(grant(account)).build(),
    )?;
    conn.send(&initialize(
        0,
        contract.version,
        json!({"_meta": {USAGE_CAPABILITY: {"notify": true, "account": "from-the-client"}}}),
    ))?;
    let answer = conn.answer(0)?;
    let advertised = &capabilities_of(&answer, contract.version)["_meta"][USAGE_CAPABILITY];
    ensure!(
        advertised["version"] == json!(1),
        "the usage capability is {advertised}, not version 1"
    );
    let sources: Vec<&str> = advertised["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let described: Vec<&str> = info
        .usage
        .sources
        .iter()
        .map(crate::Source::as_str)
        .collect();
    ensure!(
        sources == described
            && advertised["spend"] == json!(info.usage.spend)
            && advertised["needs_turn"].as_bool().unwrap_or(false) == info.usage.needs_turn
            && advertised["needs_session"].as_bool().unwrap_or(false) == info.usage.needs_session,
        "initialize advertises {advertised}, describe says {:?}",
        info.usage
    );

    // Answerable with no session and no turn, and an unchanged account costs nothing to read. A harness that can read
    // its account with no turn may have figures before one.
    let scripted = !contract.program_agent;
    let waits_for_a_turn = scripted || info.usage.needs_turn;
    let first = usage_report(&mut conn, 1, None)?;
    ensure!(
        first.supported
            && (first.buckets.is_empty() || !waits_for_a_turn)
            && first.spend.is_none()
            && first.account.as_deref() == Some(account),
        "the report before any turn is {first:?}, not an empty one for `{account}`"
    );
    let again = usage_report(&mut conn, 2, None)?;
    ensure!(
        same_figures(&again, &first),
        "polling an unchanged account changed the report: {first:?} then {again:?}"
    );
    conn.send(&request(
        3,
        USAGE_METHOD,
        json!({"sessionId": fresh_uuid()}),
    ))?;
    let refused = conn.answer(3)?;
    ensure!(
        code(&refused) == Some(-32602),
        "a session that is not open was answered {refused}"
    );

    conn.send(&new_session(4, &place.root))?;
    let session = conn.answer(4)?["result"]["sessionId"]
        .as_str()
        .ok_or("session/new gave no id")?
        .to_owned();

    // `quota 620`: the five-hour window used 620 of 1000, a token bucket beside it.
    conn.send(&prompt(5, &session, "quota 620"))?;
    let (said, end, told) = turn_with_usage(&mut conn, 5)?;
    ensure!(
        said == "echo: quota 620" && end["result"]["stopReason"] == "end_turn",
        "`quota 620` said {said:?} and ended {end}"
    );
    ensure!(
        !told.is_empty(),
        "a client that asked was told nothing of a report that changed"
    );
    let quota = usage_report(&mut conn, 6, None)?;
    ensure!(
        quota.revision > first.revision,
        "the revision did not grow: {} then {}",
        first.revision,
        quota.revision
    );
    if scripted {
        let five = quota
            .buckets
            .iter()
            .find(|bucket| bucket.id == "five_hour")
            .ok_or_else(|| format!("no `five_hour` bucket in {quota:?}"))?;
        ensure!(
            (five.used, five.limit) == (Some(620), Some(1000))
                && UsageReport::fraction_permille(five) == Some(620)
                && five.status == Status::Ok
                && matches!(five.refill, Refill::FixedWindow { resets_at: Some(at), .. } if at > five.observed_at),
            "the five-hour bucket is {five:?}"
        );
        ensure!(
            quota.buckets.iter().any(|bucket| bucket.id == "rpm"
                && matches!(
                    bucket.refill,
                    Refill::TokenBucket {
                        amount: 60,
                        max: 60,
                        ..
                    }
                )),
            "no token bucket in {quota:?}"
        );
    } else {
        ensure!(
            !quota.buckets.is_empty(),
            "a turn that scripted a quota left no bucket in {quota:?}"
        );
    }
    ensure!(
        quota.account.as_deref() == Some(account) && quota.spend.is_none(),
        "the account or the spend of a report with no session: {quota:?}"
    );
    let with_session = usage_report(&mut conn, 7, Some(&session))?;
    ensure!(
        !info.usage.spend
            || with_session
                .spend
                .as_ref()
                .is_some_and(|spend| spend.session_id == session
                    && spend.input > 0
                    && spend.output > 0),
        "the session's spend is {:?}",
        with_session.spend
    );
    let last = told
        .last()
        .map(|message| message["params"].clone())
        .unwrap_or(Value::Null);
    ensure!(
        last["sessionId"] == json!(session)
            && serde_json::from_value::<UsageReport>(last["report"].clone())
                .is_ok_and(|told| told == with_session),
        "the last notification {last} is not the report {with_session:?}"
    );

    // `limit`: the turn is refused, and the report keeps the hit.
    conn.send(&prompt(8, &session, "limit"))?;
    let (_, end, _) = turn_with_usage(&mut conn, 8)?;
    ensure!(
        end["result"]["stopReason"] == "refusal",
        "a refused turn ended {end}"
    );
    let hit: LimitHit = serde_json::from_value(end["result"]["_meta"][LIMIT_META_KEY].clone())
        .map_err(|e| format!("the refusal's `_meta` holds no limit hit ({e}): {end}"))?;
    ensure!(
        (!scripted || (hit.bucket.as_deref() == Some("five_hour") && hit.retry_after.is_some()))
            && hit.resets_at.is_some()
            && hit.text.is_some(),
        "the limit hit is {hit:?}"
    );
    let limited = usage_report(&mut conn, 9, None)?;
    ensure!(
        limited.limit.as_ref() == Some(&hit)
            && limited.revision > quota.revision
            && limited.buckets.iter().any(|bucket| {
                (!scripted || bucket.id == "five_hour") && bucket.status == Status::Exhausted
            }),
        "after the refusal the report is {limited:?}"
    );

    // The kind answers what the connection last reported for the account, and an empty report for
    // one it never saw.
    let (watch, spawner, keeper) = host::spawner();
    let kind = harness
        .usage(UsageQuery::new().account(grant(account)), spawner, keeper)
        .map_err(|e| e.message().to_owned())?;
    ensure!(watch.destroyed(), "usage kept the spawner it was handed");
    // A harness that reads its account itself (no turn needed) answers from that reading, not from a connection's.
    ensure!(
        if waits_for_a_turn && scripted {
            kind == limited
        } else if waits_for_a_turn {
            kind.supported && same_buckets(&kind, &limited)
        } else {
            kind.supported && kind.account.as_deref() == Some(account) && !kind.buckets.is_empty()
        },
        "the kind answers {kind:?}, the connection's last report was {limited:?}"
    );
    let (_, spawner, keeper) = host::spawner();
    let unseen = harness
        .usage(UsageQuery::new().account(grant("nobody")), spawner, keeper)
        .map_err(|e| e.message().to_owned())?;
    ensure!(
        unseen.supported
            && (unseen.buckets.is_empty() || !waits_for_a_turn)
            && unseen.account.as_deref() == Some("nobody"),
        "an account no connection ran on is {unseen:?}"
    );
    conn.close_with(OK)?;

    // A client that did not ask is told nothing, and the account is the client's when the host named none.
    let (mut quiet, session) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    quiet.send(&prompt(10, &session, "quota 100"))?;
    let (said, _) = turn(&mut quiet, 10)?;
    ensure!(
        said == "echo: quota 100",
        "a client that did not ask: {said:?}"
    );
    let report = usage_report(&mut quiet, 11, None)?;
    ensure!(
        if scripted {
            report.buckets.iter().any(|bucket| bucket.used == Some(100))
        } else {
            !report.buckets.is_empty()
        },
        "a client that did not ask reads {report:?}"
    );
    quiet.close_with(OK)
}

fn client_capabilities(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    if contract.program_agent {
        // The harness reads its own files; it has no way to ask the client for one.
        return Ok(());
    }
    let file = format!("{}/notes.txt", place.root);
    let (mut conn, session) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    conn.send(&prompt(2, &session, &format!("read {file}")))?;
    let (text, _) = turn(&mut conn, 2)?;
    ensure!(
        text == "echo: no fs",
        "a read with no fs offered said {text:?}"
    );
    ensure!(
        !conn.asked("fs/read_text_file") && !conn.asked("terminal/create"),
        "the agent asked what the client never offered"
    );
    conn.close_with(OK)?;

    let offered = json!({"fs": {"readTextFile": true, "writeTextFile": false}, "terminal": false});
    let (mut conn, session) =
        open_session(contract, harness, StartRequest::new(), &place.root, offered)?;
    conn.send(&prompt(2, &session, &format!("read {file}")))?;
    let asked = loop {
        let message = conn.next()?;
        if message.get("method") != Some(&json!("session/update")) {
            break message;
        }
    };
    ensure!(
        asked["method"] == "fs/read_text_file" && asked["params"]["path"] == json!(file),
        "a read with fs offered sent {asked}"
    );
    conn.send(&json!({"jsonrpc": "2.0", "id": asked["id"], "result": {"content": "hi"}}))?;
    let (text, _) = turn(&mut conn, 2)?;
    ensure!(
        text == "echo: hi",
        "the file's content was echoed as {text:?}"
    );
    conn.close_with(OK)
}

/// Sends `count` messages on a thread of its own, counting each one taken, until one is refused.
fn sender(
    channel: Arc<guatiao::library::Object<dyn guatiao_channel::Channel>>,
    version: u16,
    count: u64,
) -> (Arc<AtomicUsize>, std::thread::JoinHandle<Option<u32>>) {
    let taken = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&taken);
    let thread = std::thread::spawn(move || {
        for id in 0..count {
            let message = initialize(id, version, json!({})).to_string();
            if let Err(e) = channel.send(message.as_bytes()) {
                return Some(status(&e));
            }
            counted.fetch_add(1, Ordering::SeqCst);
        }
        None
    });
    (taken, thread)
}

/// Waits until `taken` stops growing, and answers where it stopped.
fn settled(taken: &AtomicUsize) -> usize {
    let mut last = taken.load(Ordering::SeqCst);
    loop {
        std::thread::sleep(GRACE);
        let now = taken.load(Ordering::SeqCst);
        if now == last {
            return now;
        }
        last = now;
    }
}

fn back_pressure(contract: &Contract, harness: &dyn AcpHarness) -> Outcome {
    const MANY: u64 = 64;
    let mut conn = open(harness, StartRequest::new())?;
    conn.hold();
    let (taken, thread) = sender(conn.shared_channel(), contract.version, MANY);
    let held = settled(&taken);
    ensure!(held < MANY as usize, "a held sink held no sender back");
    ensure!(conn.received() == 0, "a held sink took a message");
    conn.release();
    ensure!(
        thread
            .join()
            .map_err(|_| "the sender panicked".to_owned())?
            .is_none(),
        "a send was refused once the sink took again"
    );
    // Requests are answered as their work ends, so in no order the suite may rely on; each is answered once.
    let mut answered = Vec::new();
    for _ in 0..MANY {
        answered.push(conn.next()?["id"].as_u64());
    }
    answered.sort_unstable();
    ensure!(
        answered == (0..MANY).map(Some).collect::<Vec<_>>(),
        "the {MANY} requests were answered as {answered:?}"
    );
    conn.close_with(OK)
}

fn close_wakes_send(contract: &Contract, harness: &dyn AcpHarness) -> Outcome {
    let conn = open(harness, StartRequest::new())?;
    conn.hold();
    let (taken, thread) = sender(conn.shared_channel(), contract.version, 64);
    settled(&taken);
    let closing = Instant::now();
    conn.channel().close(OK, "");
    let woken = thread
        .join()
        .map_err(|_| "the sender panicked".to_owned())?;
    ensure!(
        woken == Some(GONE),
        "a send waiting for room was answered {woken:?}, not GONE, by close"
    );
    ensure!(
        closing.elapsed() < PATIENCE,
        "close took too long to wake a send"
    );
    let after = conn.channel().send(b"{}").map_err(|e| status(&e));
    ensure!(
        after == Err(GONE),
        "a send after close was answered {after:?}"
    );
    conn.release();
    ensure!(
        conn.closed()? == OK,
        "closed did not carry the host's status"
    );
    conn.destroyed()
}

fn close_reaches_idle(contract: &Contract, harness: &dyn AcpHarness) -> Outcome {
    for code in [OK, UNSUPPORTED] {
        let mut conn = open(harness, StartRequest::new())?;
        conn.send(&initialize(0, contract.version, json!({})))?;
        conn.answer(0)?;
        // The provider now waits for a message; close has to reach it there.
        std::thread::sleep(GRACE);
        conn.close_with(code)?;
    }
    Ok(())
}

fn close_answers_pending(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let (mut conn, session) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    conn.send(&prompt(2, &session, "wait"))?;
    ensure!(conn.quiet()?, "a waiting turn answered before the close");
    conn.channel().close(OK, "");
    let (_, answer) = turn(&mut conn, 2)?;
    ensure!(
        answer["result"]["stopReason"] == "cancelled",
        "a turn the host closed on ended {answer}"
    );
    ensure!(
        conn.closed()? == OK,
        "closed did not carry the host's status"
    );
    conn.destroyed()
}

fn provider_closes(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    if contract.program_agent {
        // A program that ends ends one session; the connection is the host's to close.
        return Ok(());
    }
    let (mut conn, session) = open_session(
        contract,
        harness,
        StartRequest::new(),
        &place.root,
        json!({}),
    )?;
    conn.send(&prompt(2, &session, "close"))?;
    let (_, answer) = turn(&mut conn, 2)?;
    ensure!(
        answer["result"]["stopReason"] == "end_turn",
        "the prompt was not answered before closing: {answer}"
    );
    ensure!(
        conn.closed()? == OK,
        "the provider's own end was not closed(OK)"
    );
    ensure!(
        conn.spawner_gone_at_close()?,
        "the spawner was still alive when the provider called closed"
    );
    let after = conn.channel().send(b"{}").map_err(|e| status(&e));
    ensure!(
        after == Err(GONE),
        "the first send after closed was answered {after:?}"
    );
    conn.channel().close(OK, "");
    conn.destroyed()
}

fn gone_sink(contract: &Contract, harness: &dyn AcpHarness) -> Outcome {
    let conn = open(harness, StartRequest::new())?;
    conn.go_away();
    conn.send(&initialize(0, contract.version, json!({})))?;
    let closed = conn.closed()?;
    ensure!(
        closed == GONE,
        "a sink answering GONE ended it with {closed}"
    );
    conn.destroyed()
}

fn working_roots(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let rooted = StartRequest::builder()
        .working_root(place.root.clone())
        .build();
    let (mut conn, _) = open_session(contract, harness, rooted, &place.root, json!({}))?;
    let separator = if cfg!(windows) { '\\' } else { '/' };
    let mut cases: Vec<(String, Value, bool)> = vec![
        ("the root".into(), json!({"cwd": place.root}), true),
        (
            "a directory inside".into(),
            json!({"cwd": place.inside}),
            true,
        ),
        (
            "a sibling sharing its prefix".into(),
            json!({"cwd": place.beside}),
            false,
        ),
        (
            "a path climbing out with `..`".into(),
            json!({"cwd": format!("{}{separator}..{separator}outside", place.root)}),
            false,
        ),
        ("a relative path".into(), json!({"cwd": "inside"}), false),
        (
            "an additional directory outside".into(),
            json!({"cwd": place.root, "additionalDirectories": [place.outside]}),
            false,
        ),
        (
            "an additional directory inside".into(),
            json!({"cwd": place.root, "additionalDirectories": [place.inside]}),
            true,
        ),
    ];
    if cfg!(windows) {
        cases.push((
            "the root in other case".into(),
            json!({"cwd": place.root.to_uppercase()}),
            true,
        ));
    }
    if let Some(link) = &place.link {
        cases.push((
            "a link inside leading outside".into(),
            json!({"cwd": link}),
            false,
        ));
    }
    for (id, (what, mut params, admitted)) in (10u64..).zip(cases) {
        params["mcpServers"] = json!([]);
        conn.send(&request(id, "session/new", params))?;
        let answer = conn.answer(id)?;
        if admitted {
            ensure!(
                answer["result"]["sessionId"].is_string(),
                "{what} was refused: {answer}"
            );
        } else {
            ensure!(
                code(&answer) == Some(-32602),
                "{what} was answered {answer}"
            );
        }
    }
    conn.close_with(OK)
}

fn message_limit(contract: &Contract, harness: &dyn AcpHarness) -> Outcome {
    let Some(limit) = contract.message_limit else {
        return Ok(());
    };
    let conn = open(harness, StartRequest::new())?;
    let head = r#"{"jsonrpc":"2.0","method":"x","params":{"pad":""#;
    let tail = r#""}}"#;
    let pad = (limit + 1).saturating_sub(head.len() + tail.len());
    let message = format!("{head}{}{tail}", "a".repeat(pad));
    let sent = conn
        .channel()
        .send(message.as_bytes())
        .map_err(|e| status(&e));
    ensure!(
        sent == Err(BAD_VALUE),
        "a message over the limit was answered {sent:?}"
    );
    let closed = conn.closed()?;
    ensure!(
        !conn.reason()?.trim().is_empty(),
        "a provider failing on a message over its limit gave no reason in closed"
    );
    ensure!(
        closed == BAD_VALUE,
        "a message over the limit ended it with {closed}"
    );
    conn.channel().close(OK, "");
    conn.destroyed()
}
