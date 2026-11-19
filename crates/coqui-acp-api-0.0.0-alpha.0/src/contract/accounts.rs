//! The checks of accounts a harness takes: the catalog, applying a grant and credentials that never
//! come back. Signing in, renewing and watching are the account provider's, held by its own suite;
//! the helpers it shares are here.
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
use guatiao::library::{Object, ProviderError};
use serde_json::{Value, json};

use super::Contract;
use super::checks::{
    CANARY, Outcome, Place, fresh_uuid, initialize, new_session, open_session_in, text,
};
use super::host::{self, PATIENCE, Script, status};
use super::json::{from_json, to_json};
use crate::{
    ACCOUNT_ID_MAX, AccountCatalog, AccountGrant, AccountKindInfo, AccountRef, AcpHarness,
    Credentials, HARNESS_LOGIN, LoginMethodInfo, LoginOutcome, LoginPrompter, LoginRequest,
    ModelsRequest, ProcessSpawner, Requirement, SessionExport, SessionImport, StartRequest,
    UsageQuery, is_account_id,
};

pub(super) const OK: u32 = 0;
pub(super) const BAD_VALUE: u32 = 7;
pub(super) const GONE: u32 = 12;
pub(super) const UNSUPPORTED: u32 = 13;

/// What a rotated copy of a credential reads as.
pub(super) const ROTATED: &str = "coqui-acp-contract-rotated-9e2a";

/// What a second rotation of it reads as.
pub(super) const SECOND: &str = "coqui-acp-contract-second-4c1d";

/// How long past its deadline a cancelled or timed out login may take to answer: a margin for a busy machine, not a promise
/// of promptness; a login that never answered would still be caught.
pub(super) const PROMPTLY: Duration = Duration::from_secs(20);

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
    ]
}

pub(super) fn why(error: &crate::guatiao::library::ProviderError) -> String {
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

pub(super) fn properties(schema: &Value) -> serde_json::Map<String, Value> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn required(schema: &Value) -> Vec<String> {
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
pub(super) fn credential_keys(schema: &Value) -> Vec<String> {
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
pub(super) fn grant_of(
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

pub(super) fn is_url(text: &str) -> bool {
    (text.starts_with("https://") || text.starts_with("http://"))
        && text.len() > 8
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// What every kind an account provider or a harness lists is held to.
pub(super) fn kind_rules(kinds: &[AccountKindInfo]) -> Outcome {
    let mut seen = Vec::new();
    for info in kinds {
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
    for info in kinds {
        for standard in &info.provides {
            ensure!(
                crate::standard::is_standard(standard),
                "{} provides {standard:?}, which is not a standard id",
                info.id
            );
        }
    }
    Ok(())
}

/// What every login method is held to.
pub(super) fn login_rules(kinds: &[AccountKindInfo], logins: &[LoginMethodInfo]) -> Outcome {
    let mut methods = Vec::new();
    for login in logins {
        ensure!(!login.id.is_empty(), "a login method has no id");
        ensure!(
            !methods.contains(&&login.id),
            "the method {} is listed twice",
            login.id
        );
        methods.push(&login.id);
        ensure!(
            kinds.iter().any(|kind| kind.id == login.produces),
            "{} produces {}, which no listed kind is",
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
    Ok(())
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
    kind_rules(&catalog.kinds)?;
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
    let mut secrets: Vec<&String> = Vec::new();
    for name in &catalog.secret_variables {
        ensure!(
            catalog.credential_variables.contains(name),
            "secret_variables names {name}, which credential_variables does not list, so a start would not clear it"
        );
        ensure!(
            !secrets.contains(&name),
            "{name} is listed twice as a secret"
        );
        secrets.push(name);
    }
    let mut standards = Vec::new();
    for accepted in &catalog.accepts {
        ensure!(
            crate::standard::is_standard(&accepted.standard),
            "accepts {:?}, which is not a standard id",
            accepted.standard
        );
        ensure!(
            !standards.contains(&&accepted.standard),
            "{} is accepted twice",
            accepted.standard
        );
        standards.push(&accepted.standard);
        ensure!(
            !accepted.slots.is_empty(),
            "{} is accepted with no way to apply it",
            accepted.standard
        );
        for slot in &accepted.slots {
            let Some(kind) = catalog.kind(&slot.kind) else {
                return Err(format!(
                    "{} is applied by {}, which the catalog does not list",
                    accepted.standard, slot.kind
                ));
            };
            ensure!(
                kind.provides.is_empty() || kind.provides.contains(&accepted.standard),
                "{} applies {} but does not provide it",
                slot.kind,
                accepted.standard
            );
            for variable in [&slot.variable, &slot.base_url_variable]
                .into_iter()
                .flatten()
            {
                ensure!(
                    catalog.credential_variables.contains(variable),
                    "{} is set for {} but is not in credential_variables, so a start would not clear it",
                    variable,
                    accepted.standard
                );
            }
        }
    }
    ensure!(
        catalog.logins.is_empty(),
        "the catalog lists {} login method(s); signing in is an account provider's",
        catalog.logins.len()
    );
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
        let (watch, spawner, _) = host::spawner_in(Some(&dir));
        let listing = harness
            .models(ModelsRequest::new().account(grant.clone()), spawner)
            .map_err(|e| format!("{}: models: {}", info.id, why(&e)))?;
        ensure!(
            watch.destroyed() && !format!("{listing:?}").contains(CANARY),
            "{}: models kept an object it was handed, or answered a credential",
            info.id
        );
        let (watch, spawner, _) = host::spawner_in(Some(&dir));
        let report = harness
            .usage(UsageQuery::new().account(grant.clone()), spawner)
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
            let (watch, spawner, _) = host::spawner_in(Some(&dir));
            let imported = harness
                .import_session(
                    SessionImport::new(session.as_str(), &place.root, "").account(grant.clone()),
                    spawner,
                )
                .map_err(|e| format!("{}: import_session: {}", info.id, why(&e)))?;
            ensure!(
                watch.destroyed() && !format!("{imported:?}").contains(CANARY),
                "{}: import_session kept an object it was handed, or answered a credential",
                info.id
            );
            let (watch, spawner, _) = host::spawner_in(Some(&dir));
            let exported = harness
                .export_session(
                    SessionExport::new(session.as_str(), &place.root).account(grant.clone()),
                    spawner,
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
    let (watch, spawner, _) = host::spawner();
    let answer = harness.models(ModelsRequest::new().account(strange.clone()), spawner);
    ensure!(
        watch.destroyed(),
        "models refused with an object it was handed alive"
    );
    refused("models", answer.err())?;
    let (watch, spawner, _) = host::spawner();
    let answer = harness.usage(UsageQuery::new().account(strange.clone()), spawner);
    ensure!(
        watch.destroyed(),
        "usage refused with an object it was handed alive"
    );
    refused("usage", answer.err())?;
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

pub(super) fn distinct<T: Clone>(
    items: impl Iterator<Item = T>,
    key: impl Fn(&T) -> (String, u64),
) -> Vec<T> {
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

/// Inputs a method's schema accepts.
pub(super) fn inputs_for(method: &LoginMethodInfo) -> Result<crate::guatiao::Value, String> {
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

/// A call of `login`.
pub(super) type LoginCall<'a> = dyn Fn(
        LoginRequest,
        Object<dyn ProcessSpawner>,
        Object<dyn LoginPrompter>,
    ) -> Result<LoginOutcome, ProviderError>
    + 'a;

/// What an account provider's sign-in is held to.
pub(super) struct SignIn<'a> {
    /// Whether the provider offers any login method; when not, `login` must refuse.
    pub offers: bool,
    /// The methods it lists.
    pub methods: &'a [LoginMethodInfo],
    /// Checks the methods that produce this kind alone.
    pub only: Option<&'a str>,
    /// Calls `login`.
    pub login: &'a LoginCall<'a>,
    /// Holds what a sign-in that completed made to the provider's own further rules.
    pub made: &'a dyn Fn(&LoginMethodInfo, &LoginOutcome, &host::Watch) -> Outcome,
}

pub(super) fn sign_in(place: &Place, held: &SignIn<'_>) -> Outcome {
    let dir = place.scratch()?;
    if !held.offers {
        let (watch, spawner, prompter) = host::login_in(&dir, Script::default());
        let error = (held.login)(LoginRequest::new("any", text(&dir)), spawner, prompter)
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
        let error = (held.login)(
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
    for method in held.methods {
        if held.only.is_some_and(|kind| kind != method.produces) {
            continue;
        }
        let label = method.id.as_str();
        let request = |dir: &Path, timeout_ms: u64| -> Result<LoginRequest, String> {
            Ok(LoginRequest::new(label, text(dir))
                .inputs(inputs_for(method)?)
                .timeout_ms(timeout_ms))
        };

        // It completes, and what it made is held to the provider's own rules.
        let dir = place.scratch()?;
        let (watch, spawner, prompter) = host::login_in(&dir, Script::default());
        let outcome = (held.login)(request(&dir, 300_000)?, spawner, prompter)
            .map_err(|e| format!("{label}: login: {}", why(&e)))?;
        ensure!(
            watch.spawner_destroyed() && watch.prompter_destroyed(),
            "{label}: login answered with an object it was handed alive"
        );
        ensure!(
            !watch.said().contains(CANARY),
            "{label}: a secret the person typed was shown back to them"
        );
        (held.made)(method, &outcome, &watch)?;

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
        let answer = (held.login)(request(&dir, 300_000)?, spawner, prompter);
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
        let answer = (held.login)(request(&dir, 300_000)?, spawner, prompter);
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
        let answer = (held.login)(request(&dir, 300)?, spawner, prompter);
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
