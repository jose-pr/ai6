//! The checks of accounts: the catalog, applying a grant, credentials that never come back, a login
//! the harness renewed or lost, `renew`, and the sign-in methods.
//!
//! The suite hands a provider a spawner that is virtual (it starts nothing: it records the specs it
//! is asked to start and hands back idle processes) and serves files in a directory of the suite's.
//! A credential it grants is a canary, so any place it turns up in is seen.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use std::path::Path;
use std::time::Instant;

use guatiao::Secret;
use serde_json::{Value, json};

use super::Contract;
use super::checks::{
    CANARY, Outcome, Place, fresh_uuid, initialize, new_session, open_session_in, text,
};
use super::host::{self, GRACE, PATIENCE, Script, status};
use super::json::{from_json, to_json};
use crate::{
    ACCOUNT_ID_MAX, AccountCatalog, AccountGrant, AccountKindInfo, AccountRef, AcpHarness,
    Credentials, HARNESS_LOGIN, LoginMethodInfo, LoginRequest, ModelsRequest, RenewRequest,
    Requirement, SessionExport, SessionImport, StartRequest, UsageQuery, is_account_id,
};

const OK: u32 = 0;
const BAD_VALUE: u32 = 7;
const GONE: u32 = 12;
const UNSUPPORTED: u32 = 13;

/// What a rotated copy of a credential reads as.
const ROTATED: &str = "coqui-acp-contract-rotated-9e2a";

/// What a second rotation of it reads as.
const SECOND: &str = "coqui-acp-contract-second-4c1d";

/// How long a cancelled or timed out login may take to answer.
const PROMPTLY: Duration = Duration::from_secs(2);

macro_rules! ensure {
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            return Err(format!($($arg)+));
        }
    };
}

pub(super) fn all(
    contract: &Contract,
    harness: &dyn AcpHarness,
    place: &Place,
) -> Vec<(&'static str, Outcome)> {
    vec![
        ("lists the accounts it takes", catalog(contract, harness)),
        (
            "applies an account and clears the credential variables",
            applies(contract, harness, place),
        ),
        (
            "refuses an account it cannot take, saying nothing of its credentials",
            refuses(contract, harness, place),
        ),
        (
            "reports a login it saw renewed or lost",
            keeps_watch(contract, harness, place),
        ),
        (
            "renews as its catalog says",
            renews(contract, harness, place),
        ),
        (
            "signs in as its catalog says",
            signs_in(contract, harness, place),
        ),
    ]
}

fn why(error: &crate::guatiao::library::ProviderError) -> String {
    format!("{} ({})", error.message(), status(error))
}

fn catalog_of(harness: &dyn AcpHarness) -> Result<AccountCatalog, String> {
    harness.accounts().map_err(|e| why(&e))
}

/// The kinds the suite checks: all, or the one `--account` names.
fn kinds(contract: &Contract, catalog: &AccountCatalog) -> Result<Vec<AccountKindInfo>, String> {
    match &contract.account {
        None => Ok(catalog.kinds.clone()),
        Some(wanted) => {
            let kind = catalog
                .kind(wanted)
                .ok_or_else(|| format!("the catalog lists no kind {wanted:?}"))?;
            Ok(vec![kind.clone()])
        }
    }
}

fn properties(schema: &Value) -> serde_json::Map<String, Value> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn required(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn write_only(property: &Value) -> bool {
    property.get("writeOnly") == Some(&json!(true))
}

fn x_sensitive(property: &Value) -> bool {
    property.get("x-sensitive") == Some(&json!(true))
}

/// The keys of a schema's credentials: its properties marked sensitive.
fn credential_keys(schema: &Value) -> Vec<String> {
    properties(schema)
        .into_iter()
        .filter(|(_, property)| write_only(property) && x_sensitive(property))
        .map(|(key, _)| key)
        .collect()
}

/// A value a property accepts, from what the schema itself offers.
fn sample(property: &Value) -> Option<Value> {
    if let Some(value) = property.get("default").or_else(|| property.get("const")) {
        return Some(value.clone());
    }
    if let Some(value) = property
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return Some(value.clone());
    }
    if let Some(value) = property
        .get("examples")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return Some(value.clone());
    }
    Some(match property.get("type").and_then(Value::as_str)? {
        "string" => match property.get("format").and_then(Value::as_str) {
            Some("uri" | "url") => json!("https://example.invalid"),
            _ => json!("contract"),
        },
        "integer" | "number" => property.get("minimum").cloned().unwrap_or(json!(1)),
        "boolean" => json!(false),
        "array" => json!([]),
        "object" => json!({}),
        _ => return None,
    })
}

/// Settings the kind's schema accepts, made of its required plain properties.
fn settings_for(info: &AccountKindInfo) -> Result<crate::guatiao::Value, String> {
    let schema = to_json(&info.settings);
    let properties = properties(&schema);
    let mut settings = serde_json::Map::new();
    for name in required(&schema) {
        let Some(property) = properties.get(&name) else {
            continue;
        };
        if write_only(property) {
            continue;
        }
        let value = sample(property).ok_or_else(|| {
            format!(
                "cannot make settings for kind {}: the required property {name} has no default, \
                 enum, example or plain type",
                info.id
            )
        })?;
        settings.insert(name, value);
    }
    from_json(&Value::Object(settings))
}

/// A grant of `info`, with a canary for each credential, and `dir` as its directory when the kind
/// needs one.
fn grant_of(
    info: &AccountKindInfo,
    dir: Option<&Path>,
    revision: u64,
) -> Result<AccountGrant, String> {
    let schema = to_json(&info.settings);
    let credentials: Credentials = credential_keys(&schema)
        .into_iter()
        .map(|key| (key.clone(), format!("{CANARY}-{}-{key}", info.id)))
        .collect();
    let mut grant = AccountGrant::new("contract-account", &info.id)
        .settings(settings_for(info)?)
        .credentials(credentials)
        .revision(revision);
    if info.state_dir {
        let dir = dir.ok_or("the suite made no directory for a kind that needs one")?;
        grant = grant.state_dir(text(dir));
    }
    Ok(grant)
}

fn is_url(text: &str) -> bool {
    (text.starts_with("https://") || text.starts_with("http://"))
        && text.len() > 8
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn catalog(contract: &Contract, harness: &dyn AcpHarness) -> Outcome {
    let catalog = catalog_of(harness)?;
    ensure!(
        catalog == catalog_of(harness)?,
        "accounts() answered differently on a second call"
    );
    ensure!(
        catalog.kind(HARNESS_LOGIN).is_some(),
        "the catalog does not list {HARNESS_LOGIN}"
    );
    let mut seen = Vec::new();
    for info in &catalog.kinds {
        ensure!(
            !info.id.is_empty() && !info.title.is_empty(),
            "a kind has no id or title"
        );
        ensure!(
            !seen.contains(&&info.id),
            "the kind {} is listed twice",
            info.id
        );
        seen.push(&info.id);
        let schema = to_json(&info.settings);
        ensure!(
            schema.is_object() && schema.get("type").is_none_or(|t| t == "object"),
            "the settings schema of {} is not an object schema: {schema}",
            info.id
        );
        for (key, property) in properties(&schema) {
            ensure!(
                write_only(&property) == x_sensitive(&property),
                "{}'s property {key} is marked writeOnly or x-sensitive but not both",
                info.id
            );
            if write_only(&property) {
                ensure!(
                    property.get("type").is_none_or(|t| t == "string"),
                    "{}'s credential {key} is not text",
                    info.id
                );
                ensure!(
                    property.get("default").is_none() && property.get("examples").is_none(),
                    "{}'s credential {key} carries a default or an example",
                    info.id
                );
            }
        }
        for slot in &info.endpoints {
            ensure!(
                matches!(slot.purpose.as_str(), "model_api" | "auth" | "proxy"),
                "{} has an endpoint for {:?}",
                info.id,
                slot.purpose
            );
            if let Some(key) = &slot.key {
                let property = properties(&schema).get(key).cloned();
                ensure!(
                    property.as_ref().is_some_and(|p| {
                        !write_only(p) && p.get("type").is_none_or(|t| t == "string")
                    }),
                    "{}'s endpoint names {key}, which is not a plain text setting",
                    info.id
                );
            }
            if let Some(default) = &slot.default {
                ensure!(
                    is_url(default),
                    "{}'s endpoint default is not a URL",
                    info.id
                );
            }
        }
    }
    let mut variables = Vec::new();
    for name in &catalog.credential_variables {
        ensure!(
            !name.is_empty()
                && !name.contains(['=', '\0'])
                && !name.chars().any(char::is_whitespace),
            "credential_variables names {name:?}, which cannot be a variable"
        );
        ensure!(!variables.contains(&name), "{name} is listed twice");
        variables.push(name);
    }
    let mut methods = Vec::new();
    for login in &catalog.logins {
        ensure!(!login.id.is_empty(), "a login method has no id");
        ensure!(
            !methods.contains(&&login.id),
            "the method {} is listed twice",
            login.id
        );
        methods.push(&login.id);
        ensure!(
            catalog.kind(&login.produces).is_some(),
            "{} produces {}, which the catalog does not list",
            login.id,
            login.produces
        );
        for step in &login.steps {
            ensure!(
                matches!(
                    step.as_str(),
                    "open_url" | "user_code" | "ask" | "paste_code"
                ),
                "{} has the step {step:?}",
                login.id
            );
        }
        let inputs = to_json(&login.inputs);
        ensure!(
            inputs.is_object() && inputs.get("type").is_none_or(|t| t == "object"),
            "{}'s inputs are not an object schema",
            login.id
        );
    }
    if contract.logins {
        ensure!(
            !catalog.logins.is_empty(),
            "the catalog lists no login method; a provider with none is run with logins off"
        );
    } else {
        ensure!(
            catalog.logins.is_empty(),
            "logins are off, and the catalog lists {} method(s)",
            catalog.logins.len()
        );
    }
    kinds(contract, &catalog)?;
    Ok(())
}

/// Opens a connection on `grant` in `dir`, initializes it and opens a session, so a provider that
/// starts its program late has done so.
fn running(
    contract: &Contract,
    harness: &dyn AcpHarness,
    place: &Place,
    grant: AccountGrant,
    dir: &Path,
) -> Result<(super::host::Conn, String), String> {
    open_session_in(
        contract,
        harness,
        StartRequest::builder().account(grant).build(),
        &place.root,
        json!({}),
        Some(dir),
    )
}

/// Checks the specs the spawner was asked to start while an account was applied: that there are
/// some (a provider that starts nothing through the spawner shows nothing), that no credential is
/// in a command line, that a kind clears every credential variable and that `harness-login`
/// clears none.
fn applied_to(
    specs: &[crate::SpawnSpec],
    catalog: &AccountCatalog,
    info: &AccountKindInfo,
) -> Outcome {
    ensure!(
        !specs.is_empty(),
        "the provider started no program through the spawner on a {} account, so nothing shows \
         that it clears the credential variables",
        info.id
    );
    for spec in specs {
        ensure!(
            !spec.args.iter().any(|arg| arg.contains(CANARY))
                && !spec.program.contains(CANARY)
                && !spec.cwd.as_deref().unwrap_or_default().contains(CANARY),
            "a program was started with a credential in its command line"
        );
        if info.id == HARNESS_LOGIN {
            ensure!(
                !spec.clear_environment
                    && !catalog
                        .credential_variables
                        .iter()
                        .any(|name| spec.remove_environment.contains(name)),
                "a program was started on a harness-login account with variables cleared"
            );
            continue;
        }
        let missed: Vec<&String> = catalog
            .credential_variables
            .iter()
            .filter(|name| !spec.clear_environment && !spec.remove_environment.contains(name))
            .collect();
        ensure!(
            missed.is_empty(),
            "a program was started on a {} account without clearing {missed:?}",
            info.id
        );
    }
    Ok(())
}

/// Checks that a program the provider started to sign in cleared every credential variable, and
/// that at least one was started.
fn clears_all(specs: &[crate::SpawnSpec], catalog: &AccountCatalog) -> Outcome {
    ensure!(
        !specs.is_empty(),
        "no program was started through the spawner, so nothing shows that it clears the credential variables"
    );
    for spec in specs {
        ensure!(
            !spec.args.iter().any(|arg| arg.contains(CANARY)),
            "a program was started with a credential in its command line"
        );
        let missed: Vec<&String> = catalog
            .credential_variables
            .iter()
            .filter(|name| !spec.clear_environment && !spec.remove_environment.contains(name))
            .collect();
        ensure!(
            missed.is_empty(),
            "a program was started without clearing {missed:?}"
        );
    }
    Ok(())
}

fn applies(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let catalog = catalog_of(harness)?;
    // With no account nothing is cleared.
    {
        let mut conn = host::open(harness, StartRequest::new())?;
        conn.send(&initialize(0, contract.version, json!({})))?;
        conn.answer(0)?;
        conn.send(&new_session(1, &place.root))?;
        conn.answer(1)?;
        for spec in conn.watch().specs() {
            ensure!(
                !catalog
                    .credential_variables
                    .iter()
                    .any(|name| spec.remove_environment.contains(name)),
                "a program was started with credential variables cleared though no account was given"
            );
        }
        conn.close_with(OK)?;
    }
    for info in kinds(contract, &catalog)? {
        let dir = place.scratch()?;
        let grant = grant_of(&info, Some(&dir), 1)?;
        let (conn, _) = running(contract, harness, place, grant.clone(), &dir)
            .map_err(|e| format!("{}: {e}", info.id))?;
        let watch = conn.watch();
        applied_to(&watch.specs(), &catalog, &info).map_err(|e| format!("{}: {e}", info.id))?;
        ensure!(
            !watch.traffic().contains(CANARY),
            "{}: a credential came back in the connection's messages",
            info.id
        );
        conn.close_with(OK)
            .map_err(|e| format!("{}: {e}", info.id))?;
        ensure!(
            watch.all_killed(),
            "{}: closed with a program it started still running",
            info.id
        );
        ensure!(
            !watch.traffic().contains(CANARY),
            "{}: a credential came back in the close reason",
            info.id
        );

        // import, export, models and usage on the account.
        let (watch, spawner, keeper) = host::spawner_in(Some(&dir));
        let listing = harness
            .models(ModelsRequest::new().account(grant.clone()), spawner, keeper)
            .map_err(|e| format!("{}: models: {}", info.id, why(&e)))?;
        ensure!(
            watch.destroyed() && !format!("{listing:?}").contains(CANARY),
            "{}: models kept an object it was handed, or answered a credential",
            info.id
        );
        let (watch, spawner, keeper) = host::spawner_in(Some(&dir));
        let report = harness
            .usage(UsageQuery::new().account(grant.clone()), spawner, keeper)
            .map_err(|e| format!("{}: usage: {}", info.id, why(&e)))?;
        ensure!(
            watch.destroyed() && !format!("{report:?}").contains(CANARY),
            "{}: usage kept an object it was handed, or answered a credential",
            info.id
        );
        ensure!(
            report.account.as_deref() == Some(grant.id.as_str()) || !report.supported,
            "{}: usage reported for {:?}, not the granted account",
            info.id,
            report.account
        );
        if contract.imports {
            let session = fresh_uuid();
            let (watch, spawner, keeper) = host::spawner_in(Some(&dir));
            let imported = harness
                .import_session(
                    SessionImport::new(session.as_str(), &place.root, "").account(grant.clone()),
                    spawner,
                    keeper,
                )
                .map_err(|e| format!("{}: import_session: {}", info.id, why(&e)))?;
            ensure!(
                watch.destroyed() && !format!("{imported:?}").contains(CANARY),
                "{}: import_session kept an object it was handed, or answered a credential",
                info.id
            );
            let (watch, spawner, keeper) = host::spawner_in(Some(&dir));
            let exported = harness
                .export_session(
                    SessionExport::new(session.as_str(), &place.root).account(grant.clone()),
                    spawner,
                    keeper,
                )
                .map_err(|e| format!("{}: export_session: {}", info.id, why(&e)))?;
            ensure!(
                watch.destroyed() && !format!("{exported:?}").contains(CANARY),
                "{}: export_session kept an object it was handed, or answered a credential",
                info.id
            );
        }
    }
    Ok(())
}

fn refuses(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let catalog = catalog_of(harness)?;
    let canary = || Credentials::from_iter([("api_key", CANARY)]);
    let strange = AccountGrant::new("contract-account", "no-such-kind").credentials(canary());

    // An unknown kind, in every call that takes an account.
    let refused = |what: &str, error: Option<crate::guatiao::library::ProviderError>| -> Outcome {
        let error = error.ok_or_else(|| format!("{what} took a kind the catalog does not list"))?;
        ensure!(
            status(&error) == BAD_VALUE,
            "{what} refused an unknown kind with {}, not {BAD_VALUE}",
            why(&error)
        );
        ensure!(
            !error.message().contains(CANARY),
            "{what}'s refusal carried a credential"
        );
        Ok(())
    };
    match super::host::start(
        harness,
        StartRequest::builder().account(strange.clone()).build(),
    ) {
        Ok(conn) => {
            conn.channel().close(OK, "");
            refused("start", None)?;
        }
        Err((error, conn)) => {
            refused("start", Some(error))?;
            conn.destroyed()?;
        }
    }
    let answer = harness.refusal(
        Vec::<Requirement>::new(),
        Secret::default(),
        Some(strange.reference()),
    );
    refused("refusal", answer.err())?;
    let (watch, spawner, keeper) = host::spawner();
    let answer = harness.models(
        ModelsRequest::new().account(strange.clone()),
        spawner,
        keeper,
    );
    ensure!(
        watch.destroyed(),
        "models refused with an object it was handed alive"
    );
    refused("models", answer.err())?;
    let (watch, spawner, keeper) = host::spawner();
    let answer = harness.usage(UsageQuery::new().account(strange.clone()), spawner, keeper);
    ensure!(
        watch.destroyed(),
        "usage refused with an object it was handed alive"
    );
    refused("usage", answer.err())?;
    let (watch, spawner, _keeper) = host::spawner();
    let answer = harness.renew(RenewRequest::new(strange.clone()), spawner);
    ensure!(
        watch.spawner_destroyed(),
        "renew refused with the spawner it was handed alive"
    );
    refused("renew", answer.err())?;

    // An id that is not an id.
    for id in [
        String::new(),
        "x".repeat(ACCOUNT_ID_MAX + 1),
        "a\nb".to_owned(),
    ] {
        ensure!(!is_account_id(&id), "the suite's bad id is a good one");
        let grant = AccountGrant::new(id, HARNESS_LOGIN);
        match super::host::start(harness, StartRequest::builder().account(grant).build()) {
            Ok(conn) => {
                conn.channel().close(OK, "");
                return Err("start took an account id that is not one".to_owned());
            }
            Err((error, conn)) => {
                ensure!(
                    status(&error) == BAD_VALUE,
                    "start refused a bad account id with {}, not {BAD_VALUE}",
                    why(&error)
                );
                conn.destroyed()?;
            }
        }
    }

    for info in kinds(contract, &catalog)? {
        if info.id == HARNESS_LOGIN {
            continue;
        }
        let schema = to_json(&info.settings);
        let dir = place.scratch()?;
        // A credential the kind requires, left out.
        let needed: Vec<String> = required(&schema)
            .into_iter()
            .filter(|name| credential_keys(&schema).contains(name))
            .collect();
        if !needed.is_empty() {
            let grant = grant_of(&info, Some(&dir), 1)?.credentials(Credentials::new());
            match super::host::start_in(
                harness,
                StartRequest::builder().account(grant).build(),
                Some(&dir),
            ) {
                Ok(conn) => {
                    conn.channel().close(OK, "");
                    return Err(format!(
                        "{}: started with a required credential missing",
                        info.id
                    ));
                }
                Err((error, conn)) => {
                    ensure!(
                        status(&error) == BAD_VALUE,
                        "{}: a missing credential was refused with {}, not {BAD_VALUE}",
                        info.id,
                        why(&error)
                    );
                    conn.destroyed()?;
                }
            }
        }
        if info.state_dir {
            // A directory the kind needs: none granted, and a spawner without files.
            let grant = grant_of(&info, Some(&dir), 1)?;
            let mut bare = grant.clone();
            bare.state_dir = None;
            match super::host::start_in(
                harness,
                StartRequest::builder().account(bare).build(),
                Some(&dir),
            ) {
                Ok(conn) => {
                    conn.channel().close(OK, "");
                    return Err(format!("{}: started with no directory it needs", info.id));
                }
                Err((error, conn)) => {
                    ensure!(
                        status(&error) == BAD_VALUE,
                        "{}: a missing directory was refused with {}, not {BAD_VALUE}",
                        info.id,
                        why(&error)
                    );
                    conn.destroyed()?;
                }
            }
            match super::host::start(harness, StartRequest::builder().account(grant).build()) {
                Ok(conn) => {
                    conn.channel().close(OK, "");
                    return Err(format!(
                        "{}: started on a spawner with no files, which it needs",
                        info.id
                    ));
                }
                Err((error, conn)) => {
                    ensure!(
                        status(&error) == UNSUPPORTED && !error.message().is_empty(),
                        "{}: a spawner without files was refused with {}, not {UNSUPPORTED} and a reason",
                        info.id,
                        why(&error)
                    );
                    ensure!(
                        !error.message().contains(CANARY),
                        "{}: the refusal carried a credential",
                        info.id
                    );
                    conn.destroyed()?;
                }
            }
        }
        // refusal answers for a kind it lists.
        let reference: AccountRef = grant_of(&info, Some(&dir), 1)?.reference();
        let answer = harness
            .refusal(Vec::new(), Secret::default(), Some(reference))
            .map_err(|e| format!("{}: refusal: {}", info.id, why(&e)))?;
        ensure!(
            !format!("{answer:?}").contains(CANARY),
            "{}: refusal carried a credential",
            info.id
        );
    }
    Ok(())
}

/// The relative path of the file a provider wrote with a canary in it, and the file's directory.
fn credential_file(writes: &[(String, Vec<u8>)]) -> Option<String> {
    writes
        .iter()
        .find(|(_, bytes)| String::from_utf8_lossy(bytes).contains(CANARY))
        .map(|(path, _)| path.clone())
}

/// What the keepers of the `models` calls made so far have heard; the connection's own keeper is
/// read in full each time.
#[derive(Default)]
struct Heard {
    renewed: Vec<crate::AccountRenewal>,
    lost: Vec<crate::AccountLoss>,
}

/// What every keeper heard, each event once by (account, sequence), as a host deduplicates.
struct Events {
    renewed: Vec<crate::AccountRenewal>,
    lost: Vec<crate::AccountLoss>,
}

fn distinct<T: Clone>(items: impl Iterator<Item = T>, key: impl Fn(&T) -> (String, u64)) -> Vec<T> {
    let mut seen = Vec::new();
    let mut kept = Vec::new();
    for item in items {
        let k = key(&item);
        if !seen.contains(&k) {
            seen.push(k);
            kept.push(item);
        }
    }
    kept
}

/// Asks the provider, with `models` calls on the same account, until `done` holds of how many
/// distinct renewals and losses every keeper heard in all, or `patience` passes.
fn polling(
    harness: &dyn AcpHarness,
    grant: &AccountGrant,
    dir: &Path,
    conn: &super::host::Conn,
    heard: &mut Heard,
    patience: Duration,
    mut done: impl FnMut(usize, usize) -> bool,
) -> Result<Events, String> {
    let deadline = Instant::now() + patience;
    loop {
        let (watch, spawner, keeper) = host::spawner_in(Some(dir));
        harness
            .models(ModelsRequest::new().account(grant.clone()), spawner, keeper)
            .map_err(|e| format!("models: {}", why(&e)))?;
        heard.renewed.extend(watch.renewed());
        heard.lost.extend(watch.lost());
        let held = conn.watch();
        let events = Events {
            renewed: distinct(heard.renewed.iter().cloned().chain(held.renewed()), |e| {
                (e.id.clone(), e.sequence)
            }),
            lost: distinct(heard.lost.iter().cloned().chain(held.lost()), |e| {
                (e.id.clone(), e.sequence)
            }),
        };
        if done(events.renewed.len(), events.lost.len()) || Instant::now() >= deadline {
            return Ok(events);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// How many times the connection's own keeper heard the renewal with `sequence`.
fn conn_renewals(conn: &super::host::Conn, sequence: u64) -> usize {
    conn.watch()
        .renewed()
        .iter()
        .filter(|e| e.sequence == sequence)
        .count()
}

fn conn_losses(conn: &super::host::Conn, sequence: u64) -> usize {
    conn.watch()
        .lost()
        .iter()
        .filter(|e| e.sequence == sequence)
        .count()
}

fn keeps_watch(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let catalog = catalog_of(harness)?;
    for info in kinds(contract, &catalog)? {
        // A kind that keeps a rotating login in a directory keeps it in a file the suite can see.
        if !(info.state_dir && info.rotates) {
            continue;
        }
        for scenario in ["rotated twice then blanked", "deleted"] {
            let label = format!("{} ({scenario})", info.id);
            let dir = place.scratch()?;
            let grant = grant_of(&info, Some(&dir), 1)?;
            let (conn, _) = running(contract, harness, place, grant.clone(), &dir)
                .map_err(|e| format!("{label}: {e}"))?;
            let writes = conn.watch().writes();
            let Some(path) = credential_file(&writes) else {
                return Err(format!(
                    "{label}: a rotating kind that needs a directory wrote no file holding its \
                     credential there, so nothing shows that it watches the login"
                ));
            };
            let file = dir.join(&path);
            let mut heard = Heard::default();
            let original = std::fs::read_to_string(&file)
                .map_err(|e| format!("{label}: the file {path} cannot be read: {e}"))?;
            if scenario == "deleted" {
                std::fs::remove_file(&file).map_err(|e| format!("{label}: {e}"))?;
                let events = polling(
                    harness,
                    &grant,
                    &dir,
                    &conn,
                    &mut heard,
                    PATIENCE,
                    |_, lost| lost >= 1,
                )?;
                ensure!(
                    events.renewed.is_empty(),
                    "{label}: a deleted file was reported renewed"
                );
                ensure!(
                    events.lost.len() == 1,
                    "{label}: {} losses reported, not one: {:?}",
                    events.lost.len(),
                    events.lost
                );
                let loss = &events.lost[0];
                ensure!(
                    loss.id == grant.id
                        && loss.sequence >= 1
                        && !loss.reason.is_empty()
                        && !loss.reason.contains(CANARY),
                    "{label}: the loss is {loss:?}"
                );
                std::thread::sleep(GRACE);
                let again = polling(harness, &grant, &dir, &conn, &mut heard, GRACE, |_, _| true)?;
                ensure!(
                    again.renewed.is_empty() && again.lost.len() == 1,
                    "{label}: a deleted file was reported again ({} renewed, {} lost)",
                    again.renewed.len(),
                    again.lost.len()
                );
                ensure!(
                    conn_losses(&conn, loss.sequence) == 1,
                    "{label}: the connection's own keeper did not hear the loss once"
                );
            } else {
                let rotated = original.replace(CANARY, ROTATED);
                ensure!(
                    rotated != original,
                    "{label}: the file does not hold the credential"
                );
                std::fs::write(&file, &rotated).map_err(|e| format!("{label}: {e}"))?;
                let first = polling(
                    harness,
                    &grant,
                    &dir,
                    &conn,
                    &mut heard,
                    PATIENCE,
                    |renewed, _| renewed >= 1,
                )?;
                ensure!(
                    first.lost.is_empty(),
                    "{label}: a rotated file was reported lost"
                );
                ensure!(
                    first.renewed.len() == 1,
                    "{label}: {} renewals reported, not one",
                    first.renewed.len()
                );
                let one = &first.renewed[0];
                ensure!(
                    one.id == grant.id && one.from_revision == grant.revision && one.sequence >= 1,
                    "{label}: the renewal is for {} at revision {}, sequence {}",
                    one.id,
                    one.from_revision,
                    one.sequence
                );
                let held = one.credentials.expose();
                ensure!(
                    held.iter().any(|(_, value)| value.contains(ROTATED))
                        && !held.iter().any(|(_, value)| value.contains(CANARY)),
                    "{label}: the renewal does not hold the rotated credential alone"
                );
                std::thread::sleep(GRACE);
                let settled =
                    polling(harness, &grant, &dir, &conn, &mut heard, GRACE, |_, _| true)?;
                ensure!(
                    settled.renewed.len() == 1,
                    "{label}: the rotation was reported as {} events",
                    settled.renewed.len()
                );
                // Every live keeper hears it, the connection's whichever call noticed it, and once.
                ensure!(
                    conn_renewals(&conn, one.sequence) == 1,
                    "{label}: the connection's own keeper heard the rotation {} times, not once",
                    conn_renewals(&conn, one.sequence)
                );

                // A second rotation, chained on the first: a later sequence.
                let again = rotated.replace(ROTATED, SECOND);
                std::fs::write(&file, &again).map_err(|e| format!("{label}: {e}"))?;
                let second = polling(
                    harness,
                    &grant,
                    &dir,
                    &conn,
                    &mut heard,
                    PATIENCE,
                    |renewed, _| renewed >= 2,
                )?;
                ensure!(
                    second.renewed.len() == 2 && second.lost.is_empty(),
                    "{label}: {} renewals and {} losses after the second rotation",
                    second.renewed.len(),
                    second.lost.len()
                );
                let two = second
                    .renewed
                    .iter()
                    .find(|e| e.sequence != one.sequence)
                    .ok_or("the second rotation has the first one's sequence")?;
                ensure!(
                    two.sequence > one.sequence
                        && two
                            .credentials
                            .expose()
                            .iter()
                            .any(|(_, value)| value.contains(SECOND)),
                    "{label}: the second rotation has sequence {} after {} and holds {:?}",
                    two.sequence,
                    one.sequence,
                    two.credentials
                );

                // Blanked: lost, never renewed, and later than both.
                std::fs::write(&file, "").map_err(|e| format!("{label}: {e}"))?;
                let blank = polling(
                    harness,
                    &grant,
                    &dir,
                    &conn,
                    &mut heard,
                    PATIENCE,
                    |_, lost| lost >= 1,
                )?;
                ensure!(
                    blank.renewed.len() == 2,
                    "{label}: a blanked file was reported renewed"
                );
                ensure!(
                    blank.lost.len() == 1,
                    "{label}: {} losses reported, not one: {:?}",
                    blank.lost.len(),
                    blank.lost
                );
                let loss = &blank.lost[0];
                ensure!(
                    loss.id == grant.id
                        && !loss.reason.contains(CANARY)
                        && loss.sequence > two.sequence,
                    "{label}: the loss is {loss:?}, after sequence {}",
                    two.sequence
                );
                ensure!(
                    conn_losses(&conn, loss.sequence) == 1,
                    "{label}: the connection's own keeper did not hear the loss once"
                );
            }
            ensure!(
                !conn.watch().traffic().contains(ROTATED),
                "{label}: a credential came back in the connection's messages"
            );
            conn.close_with(OK).map_err(|e| format!("{label}: {e}"))?;
        }
    }
    Ok(())
}

fn renews(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let catalog = catalog_of(harness)?;
    for info in kinds(contract, &catalog)? {
        let dir = place.scratch()?;
        let grant = grant_of(&info, Some(&dir), 1)?;
        // A connection on the account first, so the files it keeps are where renew looks.
        let held = if info.renews && info.state_dir {
            Some(
                running(contract, harness, place, grant.clone(), &dir)
                    .map_err(|e| format!("{}: {e}", info.id))?
                    .0,
            )
        } else {
            None
        };
        let renew = |grant: &AccountGrant| {
            let (watch, spawner, keeper) = host::spawner_in(Some(&dir));
            drop(keeper);
            let answer = harness.renew(RenewRequest::new(grant.clone()), spawner);
            (watch.spawner_destroyed(), answer)
        };
        let (gone, answer) = renew(&grant);
        ensure!(
            gone,
            "{}: renew answered with the spawner it was handed alive",
            info.id
        );
        if info.renews {
            let report = answer.map_err(|e| format!("{}: renew: {}", info.id, why(&e)))?;
            ensure!(
                report.credentials.is_some()
                    || report
                        .not_renewed_because
                        .as_deref()
                        .is_some_and(|reason| !reason.is_empty()),
                "{}: renew changed nothing and gave no reason",
                info.id
            );
            ensure!(
                !report
                    .not_renewed_because
                    .as_deref()
                    .unwrap_or_default()
                    .contains(CANARY)
                    && !report
                        .expires_at
                        .as_deref()
                        .unwrap_or_default()
                        .contains(CANARY),
                "{}: renew's report carried a credential",
                info.id
            );
            if let Some(conn) = held {
                // A login the harness lost is GONE, whatever is left of its file: never a renewal
                // made of nothing.
                if let Some(path) = credential_file(&conn.watch().writes()) {
                    let file = dir.join(&path);
                    for (how, make) in [("blank", true), ("missing", false)] {
                        if make {
                            std::fs::write(&file, "").map_err(|e| format!("{}: {e}", info.id))?;
                        } else {
                            std::fs::remove_file(&file).map_err(|e| format!("{}: {e}", info.id))?;
                        }
                        let (_, lost) = renew(&grant);
                        let error = lost.err().ok_or_else(|| {
                            format!("{}: renew of a {how} login file answered a report", info.id)
                        })?;
                        ensure!(
                            status(&error) == GONE
                                && !error.message().is_empty()
                                && !error.message().contains(CANARY),
                            "{}: renew of a {how} login file refused with {}, not {GONE} and a reason",
                            info.id,
                            why(&error)
                        );
                    }
                }
                conn.close_with(OK)?;
            }
        } else {
            let error = answer.err().ok_or_else(|| {
                format!(
                    "{}: renew answered where the catalog says it cannot",
                    info.id
                )
            })?;
            ensure!(
                status(&error) == UNSUPPORTED
                    && !error.message().is_empty()
                    && !error.message().contains(CANARY),
                "{}: renew refused with {}, not {UNSUPPORTED} and a reason",
                info.id,
                why(&error)
            );
        }
    }
    Ok(())
}

/// Inputs a method's schema accepts.
fn inputs_for(method: &LoginMethodInfo) -> Result<crate::guatiao::Value, String> {
    let schema = to_json(&method.inputs);
    let properties = properties(&schema);
    let mut inputs = serde_json::Map::new();
    for name in required(&schema) {
        let Some(property) = properties.get(&name) else {
            continue;
        };
        let value = if write_only(property) {
            json!(format!("{CANARY}-input-{name}"))
        } else {
            sample(property).ok_or_else(|| {
                format!(
                    "cannot make inputs for {}: {name} has no default, enum, example or plain type",
                    method.id
                )
            })?
        };
        inputs.insert(name, value);
    }
    from_json(&Value::Object(inputs))
}

fn signs_in(contract: &Contract, harness: &dyn AcpHarness, place: &Place) -> Outcome {
    let catalog = catalog_of(harness)?;
    let dir = place.scratch()?;
    if !contract.logins {
        let (watch, spawner, prompter) = host::login_in(&dir, Script::default());
        let error = harness
            .login(LoginRequest::new("any", text(&dir)), spawner, prompter)
            .err()
            .ok_or("login answered where the provider offers no method")?;
        ensure!(
            watch.spawner_destroyed() && watch.prompter_destroyed(),
            "login answered with an object it was handed alive"
        );
        ensure!(
            status(&error) == UNSUPPORTED && !error.message().is_empty(),
            "a provider with no login method refused with {}, not {UNSUPPORTED} and a reason",
            why(&error)
        );
        return Ok(());
    }
    {
        let (watch, spawner, prompter) = host::login_in(&dir, Script::default());
        let error = harness
            .login(
                LoginRequest::new("no-such-method", text(&dir)),
                spawner,
                prompter,
            )
            .err()
            .ok_or("login took a method the catalog does not list")?;
        ensure!(
            watch.spawner_destroyed() && watch.prompter_destroyed(),
            "login refused an unknown method with an object it was handed alive"
        );
        ensure!(
            status(&error) == BAD_VALUE,
            "an unknown login method was refused with {}, not {BAD_VALUE}",
            why(&error)
        );
    }
    let mut waits_on_a_person = false;
    let mut cancelled_while_waiting = false;
    for method in &catalog.logins {
        if contract
            .account
            .as_ref()
            .is_some_and(|kind| *kind != method.produces)
        {
            continue;
        }
        let label = method.id.as_str();
        let request = |dir: &Path, timeout_ms: u64| -> Result<LoginRequest, String> {
            Ok(LoginRequest::new(label, text(dir))
                .inputs(inputs_for(method)?)
                .timeout_ms(timeout_ms))
        };

        // It completes, and what it made starts.
        let dir = place.scratch()?;
        let (watch, spawner, prompter) = host::login_in(&dir, Script::default());
        let outcome = harness
            .login(request(&dir, 5_000)?, spawner, prompter)
            .map_err(|e| format!("{label}: login: {}", why(&e)))?;
        ensure!(
            watch.spawner_destroyed() && watch.prompter_destroyed(),
            "{label}: login answered with an object it was handed alive"
        );
        ensure!(
            !watch.said().contains(CANARY),
            "{label}: a secret the person typed was shown back to them"
        );
        clears_all(&watch.specs(), &catalog)
            .map_err(|e| format!("{label}: the sign-in's program: {e}"))?;
        let made = catalog
            .kind(&outcome.kind)
            .ok_or_else(|| format!("{label}: it made a kind the catalog does not list"))?;
        ensure!(
            outcome.kind == method.produces,
            "{label}: it produces {}, and made {}",
            method.produces,
            outcome.kind
        );
        ensure!(
            made.id == HARNESS_LOGIN || !outcome.credentials.expose().is_empty(),
            "{label}: it made an account with no credentials"
        );
        let shown = format!(
            "{} {:?}",
            to_json(&outcome.settings),
            (&outcome.label, &outcome.expires_at)
        );
        ensure!(
            !shown.contains(CANARY),
            "{label}: the outcome's settings or label carry a secret"
        );
        let kept = place.scratch()?;
        let mut account = AccountGrant::new("contract-signed-in", &outcome.kind)
            .settings(outcome.settings.clone())
            .credentials(outcome.credentials.expose().clone())
            .revision(1);
        if made.state_dir {
            account = account.state_dir(text(&kept));
        }
        let (conn, _) = running(contract, harness, place, account, &kept)
            .map_err(|e| format!("{label}: what it made does not start: {e}"))?;
        conn.close_with(OK)?;

        // The person cancels at the first thing shown.
        let dir = place.scratch()?;
        let (watch, spawner, prompter) = host::login_in(
            &dir,
            Script {
                cancel_at_call: Some(0),
                ..Script::default()
            },
        );
        let begun = Instant::now();
        let answer = harness.login(request(&dir, 5_000)?, spawner, prompter);
        let took = begun.elapsed();
        ensure!(
            watch.spawner_destroyed() && watch.prompter_destroyed(),
            "{label}: a cancelled login answered with an object it was handed alive"
        );
        match answer {
            Ok(_) => ensure!(
                watch.prompts() == 0,
                "{label}: the person cancelled and the login went on to an outcome"
            ),
            Err(error) => {
                ensure!(
                    status(&error) == GONE && error.message().contains("cancelled"),
                    "{label}: a cancelled login answered {}, not GONE `login cancelled`",
                    why(&error)
                );
                ensure!(
                    !error.message().contains(CANARY),
                    "{label}: the refusal carried a secret"
                );
                ensure!(took <= PROMPTLY, "{label}: a cancelled login took {took:?}");
                ensure!(
                    watch.all_killed(),
                    "{label}: a cancelled login left a program it started running"
                );
            }
        }

        // The person cancels while the login waits.
        waits_on_a_person |= method
            .steps
            .iter()
            .any(|step| matches!(step.as_str(), "open_url" | "user_code" | "paste_code"));
        let dir = place.scratch()?;
        let (watch, spawner, prompter) = host::login_in(
            &dir,
            Script {
                cancel_after: Some(Duration::from_millis(250)),
                ..Script::default()
            },
        );
        let begun = Instant::now();
        let answer = harness.login(request(&dir, 5_000)?, spawner, prompter);
        let took = begun.elapsed();
        if let Err(error) = answer {
            cancelled_while_waiting = true;
            ensure!(
                status(&error) == GONE && error.message().contains("cancelled"),
                "{label}: a login cancelled while it waited answered {}",
                why(&error)
            );
            ensure!(
                took <= Duration::from_millis(250) + PROMPTLY,
                "{label}: a login cancelled while it waited took {took:?}"
            );
            ensure!(
                watch.all_killed(),
                "{label}: a login cancelled while it waited left a program running"
            );
        }

        // It runs out of time.
        let dir = place.scratch()?;
        let delay = Duration::from_millis(800);
        let (watch, spawner, prompter) = host::login_in(
            &dir,
            Script {
                ask_delay: delay,
                ..Script::default()
            },
        );
        let begun = Instant::now();
        let answer = harness.login(request(&dir, 300)?, spawner, prompter);
        let took = begun.elapsed();
        match answer {
            Ok(_) => {
                ensure!(
                    watch.spawner_destroyed() && watch.prompter_destroyed(),
                    "{label}: a login answered with an object it was handed alive"
                );
                ensure!(
                    watch.asked() == 0 && took < Duration::from_millis(300),
                    "{label}: a login went past its 300 ms timeout and made an account"
                );
            }
            Err(error) => {
                ensure!(
                    status(&error) == GONE && error.message().contains("timed out"),
                    "{label}: a login that ran out of time answered {}, not GONE `login timed out`",
                    why(&error)
                );
                // The provider answers at the deadline, even with a prompter call still out.
                ensure!(
                    took <= Duration::from_millis(300) + PROMPTLY,
                    "{label}: a login that ran out of time took {took:?}"
                );
                ensure!(
                    watch.all_killed(),
                    "{label}: a login that ran out of time left a program running"
                );
                ensure!(
                    watch.wait_destroyed(PATIENCE),
                    "{label}: a login that ran out of time never let go of what it was handed"
                );
            }
        }
    }
    ensure!(
        !waits_on_a_person || cancelled_while_waiting,
        "no login method that waits for a person was cancelled while it waited: none of them \
         answered GONE within the 250 ms the suite waits before cancelling"
    );
    Ok(())
}
