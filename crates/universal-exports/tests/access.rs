//! Access administration through the gateway: seeding, the guarded routes, Operation policies
//! managed by delegates, accounts and the audit trail.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, AuthorizationDecision,
    AuthorizationRequest, Authorizer, ConfigProvider, ConfigRecord, ConfigScope, Directory,
    FileProvider, FileRegistry, Lookup, Person, PutOutcome, SqlProvider, Verdict,
    guatiao::library::ProviderError,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use universal_exports::{
    AccessProviders, EitherAuthorizer, Gateway, GatewayConfig, GatewayLimits, LiveAccess,
};

const STATION_TOKEN: &str = "exports-test-credential-with-more-than-32-bytes";
const ORIGIN: &str = "https://exports.example";

#[derive(Clone)]
struct Account {
    groups: Vec<String>,
    disabled: bool,
    secret: String,
}

/// Accounts, sign-in and the directory over one table.
#[derive(Default)]
struct Table(Mutex<BTreeMap<String, Account>>);

impl Table {
    fn with(accounts: &[(&str, &[&str])]) -> Arc<Self> {
        let table = Table::default();
        for (name, groups) in accounts {
            table.0.lock().unwrap().insert(
                (*name).to_owned(),
                Account {
                    groups: groups.iter().map(|g| (*g).to_owned()).collect(),
                    disabled: false,
                    secret: "secret".to_owned(),
                },
            );
        }
        Arc::new(table)
    }
}

impl Authenticator for Table {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let accounts = self.0.lock().unwrap();
        Ok(match (attempt.name, attempt.secret) {
            (Some(name), Some(secret))
                if accounts
                    .get(&name)
                    .is_some_and(|a| !a.disabled && a.secret == secret) =>
            {
                Verdict::Valid(Lookup {
                    by: "name".to_owned(),
                    value: name,
                })
            }
            _ => Verdict::Invalid,
        })
    }
}

impl Directory for Table {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        let name = if lookup.by == "id" {
            lookup.value.trim_end_matches("-id").to_owned()
        } else {
            lookup.value
        };
        Ok(self.0.lock().unwrap().get(&name).map(|a| Person {
            id: format!("{name}-id"),
            name: name.clone(),
            groups: a.groups.clone(),
            ..Person::default()
        }))
    }
}

impl Accounts for Table {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(name, a)| AccountInfo {
                id: format!("{name}-id"),
                name: name.clone(),
                groups: a.groups.clone(),
                disabled: Some(a.disabled),
                ..AccountInfo::default()
            })
            .collect())
    }

    fn put(&self, put: AccountPut) -> Result<PutOutcome, ProviderError> {
        let mut accounts = self.0.lock().unwrap();
        let entry = accounts.entry(put.name).or_insert(Account {
            groups: vec![],
            disabled: false,
            secret: String::new(),
        });
        if let Some(groups) = put.groups {
            entry.groups = groups;
        }
        if let Some(disabled) = put.disabled {
            entry.disabled = disabled;
        }
        if let Some(secret) = put.secret {
            entry.secret = secret;
        }
        Ok(PutOutcome::default())
    }

    fn remove(&self, name: String) -> Result<(), ProviderError> {
        self.0.lock().unwrap().remove(&name);
        Ok(())
    }
}

struct Plugin;

impl Authorizer for Plugin {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        Ok(AuthorizationDecision {
            allowed: request.person.name == "guest",
            ..AuthorizationDecision::default()
        })
    }
}

struct Harness {
    base: String,
    registry: FileRegistry,
    provider: Arc<dyn ConfigProvider>,
    table: Arc<Table>,
    stop: Option<oneshot::Sender<()>>,
    _dir: tempfile::TempDir,
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// ROOT > P > X, ROOT > S; every name below is an account in `table`.
async fn serve(
    provider: Arc<dyn ConfigProvider>,
    dir: tempfile::TempDir,
    chiefs: &[&str],
    plugin: bool,
) -> Harness {
    let registry = FileRegistry::open(dir.path().join("registry"));
    registry
        .check_in(
            "station-a",
            BTreeMap::from([
                ("operations".to_owned(), json!(["X", "S"])),
                (
                    "operation_lineage".to_owned(),
                    json!({"ROOT": [], "P": ["ROOT"], "X": ["P", "ROOT"], "S": ["ROOT"]}),
                ),
            ]),
        )
        .unwrap();
    let table = Table::with(&[
        ("boss", &[]),
        ("ctl", &[]),
        ("aud", &[]),
        ("dep", &[]),
        ("qa1", &["qa"]),
        ("guest", &[]),
        ("nobody", &[]),
        ("acc", &[]),
    ]);
    let registry_dyn: Arc<dyn RegistryBackend> = Arc::new(registry.clone());
    let live = Arc::new(
        LiveAccess::new(
            provider.clone(),
            chiefs.iter().map(|c| (*c).to_owned()).collect(),
            plugin.then(|| "test".to_owned()),
        )
        .unwrap(),
    );
    let snapshot = provider.load("").unwrap();
    live.seed(&snapshot, &registry_dyn).unwrap();
    live.apply(&provider.load("").unwrap());
    let authorizer: Arc<dyn Authorizer> = if plugin {
        Arc::new(EitherAuthorizer {
            live: live.clone(),
            plugin: Arc::new(Plugin),
        })
    } else {
        live.clone()
    };
    let gateway = Gateway::with_access(
        GatewayConfig::new(
            ORIGIN,
            STATION_TOKEN,
            GatewayLimits::default(),
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap(),
        registry_dyn,
        AccessProviders::new(table.clone(), table.clone(), authorizer),
    )
    .with_access_admin(live)
    .with_accounts(table.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, gateway.router())
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    Harness {
        base,
        registry,
        provider,
        table,
        stop: Some(stop),
        _dir: dir,
    }
}

async fn sql(chiefs: &[&str], plugin: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", dir.path().join("config.db").display());
    let provider = Arc::new(SqlProvider::open(&location).unwrap());
    serve(provider, dir, chiefs, plugin).await
}

async fn login(base: &str, name: &str) -> String {
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/auth/login"))
        .header("origin", ORIGIN)
        .header("x-forwarded-proto", "https")
        .json(&json!({"name": name, "secret": "secret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED, "{name}");
    response.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn call(
    h: &Harness,
    token: &str,
    method: Method,
    path: &str,
    revision: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = reqwest::Client::new()
        .request(method, format!("{}{path}", h.base))
        .bearer_auth(token);
    if let Some(revision) = revision {
        request = request.header("if-match", format!("\"{revision}\""));
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or_default())
}

async fn revision(h: &Harness, token: &str, path: &str) -> String {
    let (status, view) = call(h, token, Method::GET, path, None, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view["revision"].as_str().unwrap().to_owned()
}

fn audits(h: &Harness, action: &str) -> Vec<ai6::AuditRecord> {
    h.registry
        .list_audit(200)
        .unwrap()
        .into_iter()
        .filter(|record| record.action == action)
        .collect()
}

#[tokio::test]
async fn the_first_start_seeds_the_built_in_roles_once() {
    let h = sql(&["account:boss"], false).await;
    assert_eq!(audits(&h, "access.seeded").len(), 1);
    let boss = login(&h.base, "boss").await;
    let (_, view) = call(&h, &boss, Method::GET, "/v1/user/access", None, None).await;
    let names: Vec<&str> = view["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|role| role["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["chief", "auditor", "controller", "operator"]);
    assert_eq!(view["roles"][0]["editable"], false);
    assert_eq!(view["chief"]["host"], json!(["account:boss"]));

    let revision = view["revision"].as_str().unwrap();
    let (status, _) = call(
        &h,
        &boss,
        Method::DELETE,
        "/v1/user/access/roles/operator",
        Some(revision),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A second gateway over the same provider does not bring `operator` back.
    let registry: Arc<dyn RegistryBackend> = Arc::new(h.registry.clone());
    let again = LiveAccess::new(h.provider.clone(), vec![], None).unwrap();
    assert!(
        !again
            .seed(&h.provider.load("").unwrap(), &registry)
            .unwrap()
    );
    let stored = h
        .provider
        .definitions(ConfigRecord::Access, &ConfigScope::Global)
        .unwrap();
    assert!(stored.records[0].1["role"].get("operator").is_none());
    assert_eq!(audits(&h, "access.seeded").len(), 1);
}

#[tokio::test]
async fn a_read_only_provider_serves_the_seeded_roles_from_memory() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    let h = serve(
        Arc::new(FileProvider::new(&config)),
        dir,
        &["account:boss"],
        false,
    )
    .await;
    let boss = login(&h.base, "boss").await;
    let (status, view) = call(&h, &boss, Method::GET, "/v1/user/access", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["writable"], false);
    assert!(
        view["roles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|role| role["name"] == "controller")
    );
    let (status, _) = call(
        &h,
        &boss,
        Method::PUT,
        "/v1/user/access/delegable",
        Some("0"),
        Some(json!({"actions": null})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(audits(&h, "access.seeded").is_empty());
}

#[tokio::test]
async fn the_chief_guard_and_the_revision_precondition() {
    let h = staffed_sql(&["account:boss"], false).await;
    let boss = login(&h.base, "boss").await;
    let ctl = login(&h.base, "ctl").await;
    let revision = revision(&h, &ctl, "/v1/user/access").await;

    // No If-Match: 428. A stale one: 409.
    let delegable = json!({"actions": ["mission.read"]});
    let (status, _) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/access/delegable",
        None,
        Some(delegable.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    let (status, _) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/access/delegable",
        Some("stale"),
        Some(delegable),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // A controller may not change the chief list, nor add assistant.memory.admin.
    let (status, body) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/access/chief",
        Some(&revision),
        Some(json!({"chief": ["account:ctl"]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("chief"), "{body}");
    let (status, body) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/access/roles/controller",
        Some(&revision),
        Some(json!({"actions": ["access.manage", "assistant.memory.admin"]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!audits(&h, "access.refused").is_empty());

    // A role no rule binds is checked at *, where this controller holds nothing; removing passes.
    let (status, body) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/access/roles/operator",
        Some(&revision),
        Some(json!({"actions": ["mission.read"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!audits(&h, "access.update").is_empty());

    // The host chief may clear the stored list; with nobody left, the stored last chief may not go.
    let revision = self::revision(&h, &boss, "/v1/user/access").await;
    let (status, _) = call(
        &h,
        &boss,
        Method::PUT,
        "/v1/user/access/chief",
        Some(&revision),
        Some(json!({"chief": ["account:ctl"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, view) = call(&h, &boss, Method::GET, "/v1/user/access", None, None).await;
    assert_eq!(view["chief"]["stored"], json!(["account:ctl"]));
}

/// The seeded roles, `ctl` as Controller for P (and of the policy and accounts), `aud` as
/// auditor everywhere, `dep` as auditor at X, `acc` managing accounts only, `boss` chief.
fn staffed() -> ai6::AccessPolicy {
    let mut policy = ai6::seeded_policy();
    policy
        .roles
        .insert("accountant".into(), ai6::RoleDef::new(["account.manage"]));
    for (subject, scope, role) in [
        ("account:ctl", "operation:P", "controller"),
        ("account:ctl", "config:access", "controller"),
        ("account:ctl", "config:accounts", "controller"),
        ("account:aud", "*", "auditor"),
        ("account:dep", "operation:X", "auditor"),
        ("account:acc", "config:accounts", "accountant"),
    ] {
        policy.rules.push(ai6::RuleDef::new(subject, scope, role));
    }
    policy.chief = vec!["account:boss".to_owned()];
    policy
}

async fn staffed_sql(chiefs: &[&str], plugin: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", dir.path().join("config.db").display());
    let provider = Arc::new(SqlProvider::open(&location).unwrap());
    provider
        .put(
            ConfigRecord::Access,
            &ConfigScope::Global,
            "settings",
            &staffed().to_json(),
        )
        .unwrap();
    serve(provider, dir, chiefs, plugin).await
}

#[tokio::test]
async fn removing_the_last_chief_without_a_host_chief_is_refused() {
    let h = staffed_sql(&[], false).await;
    let boss = login(&h.base, "boss").await;
    let revision = revision(&h, &boss, "/v1/user/access").await;
    let (status, body) = call(
        &h,
        &boss,
        Method::PUT,
        "/v1/user/access/chief",
        Some(&revision),
        Some(json!({"chief": []})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("no chief"));
}

#[tokio::test]
async fn a_controller_for_p_manages_p_and_nothing_beside_it() {
    let h = staffed_sql(&["account:boss"], false).await;
    let ctl = login(&h.base, "ctl").await;
    let aud = login(&h.base, "aud").await;
    let dep = login(&h.base, "dep").await;

    let revision = revision(&h, &ctl, "/v1/user/operations/P/access").await;
    let (status, changed) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/operations/P/access/roles/reviewer",
        Some(&revision),
        Some(json!({"actions": ["mission.read", "approval.decide"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let revision = changed["revision"].as_str().unwrap().to_owned();
    let (status, changed) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/operations/P/access/rules",
        Some(&revision),
        Some(json!({"rules": [
            {"subject": "group:qa", "scope": "operation:X", "role": "reviewer"},
            {"subject": "group:other", "scope": "operation:P", "role": "reviewer",
             "propagate": false}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    let updates = audits(&h, "access.update");
    assert!(!updates.is_empty());
    assert_eq!(updates[0].operation, "P");

    let check = |subject: &str, resource: &str| {
        format!(
            "/v1/user/access/check?subject={subject}&action=approval.decide&resource={resource}"
        )
    };
    let (status, answer) = call(
        &h,
        &ctl,
        Method::GET,
        &check("account:qa1", "operation:X"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["allowed"], true);
    assert_eq!(answer["tier"], "operation:P");
    assert_eq!(answer["role"], "reviewer");
    let (status, _) = call(
        &h,
        &ctl,
        Method::GET,
        &check("account:qa1", "operation:S"),
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a sibling of P is not for ctl to read"
    );

    // The same change at a sibling is refused.
    let (status, _) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/operations/S/access/rules",
        Some("0"),
        Some(json!({"rules": []})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // What a Controller may not hand out.
    let revision = self::revision(&h, &ctl, "/v1/user/operations/P/access").await;
    for actions in [["station.configure"], ["approval.class"], ["access.manage"]] {
        let (status, body) = call(
            &h,
            &ctl,
            Method::PUT,
            "/v1/user/operations/P/access/roles/bad",
            Some(&revision),
            Some(json!({ "actions": actions })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{actions:?}: {body}");
    }
    let (status, body) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/operations/P/access/rules",
        Some(&revision),
        Some(json!({"rules": [{"subject": "group:g", "scope": "*", "role": "operator"}]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // A person with access.read at X sees the bindings that reach X, not ones bound elsewhere.
    let (status, view) = call(
        &h,
        &dep,
        Method::GET,
        "/v1/user/operations/X/access",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let bound = view["bindings"].to_string();
    assert!(bound.contains("group:qa"), "{bound}");
    assert!(!bound.contains("group:other"), "{bound}");
    let (status, _) = call(
        &h,
        &aud,
        Method::PUT,
        "/v1/user/operations/X/access/rules",
        Some("0"),
        Some(json!({"rules": []})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an auditor only reads");
}

#[tokio::test]
async fn accounts_are_made_disabled_and_reset_with_the_secret_shown_once() {
    let h = staffed_sql(&["account:boss"], false).await;
    let ctl = login(&h.base, "ctl").await;
    let acc = login(&h.base, "acc").await;
    let aud = login(&h.base, "aud").await;

    let (status, made) = call(
        &h,
        &acc,
        Method::PUT,
        "/v1/user/accounts/newbie",
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let first = made["secret"].as_str().unwrap().to_owned();
    assert_eq!(first.len(), 48);
    let (status, _) = call(&h, &aud, Method::GET, "/v1/user/accounts", None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, listed) = call(&h, &acc, Method::GET, "/v1/user/accounts", None, None).await;
    assert!(listed.to_string().contains("newbie"));
    assert!(
        !listed.to_string().contains(&first),
        "a list never shows a secret"
    );

    // Groups confer roles, so they also need access.manage.
    let groups = Some(json!({"groups": ["staff"]}));
    let (status, body) = call(
        &h,
        &acc,
        Method::PUT,
        "/v1/user/accounts/newbie",
        None,
        groups.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/accounts/newbie",
        None,
        groups,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &h,
        &acc,
        Method::PUT,
        "/v1/user/accounts/newbie",
        None,
        Some(json!({"disabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(h.table.0.lock().unwrap()["newbie"].disabled);

    // A chief's account needs a chief.
    let (status, body) = call(
        &h,
        &ctl,
        Method::PUT,
        "/v1/user/accounts/boss",
        None,
        Some(json!({"disabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!h.table.0.lock().unwrap()["boss"].disabled);

    let (status, reset) = call(
        &h,
        &acc,
        Method::POST,
        "/v1/user/accounts/newbie/secret",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    let secret = reset["secret"].as_str().unwrap().to_owned();
    assert_ne!(secret, first);
    for record in h.registry.list_audit(200).unwrap() {
        assert!(
            !record.target.contains(&secret) && !record.target.contains(&first),
            "{record:?}"
        );
    }
    assert!(!audits(&h, "account.secret").is_empty());
    assert!(!audits(&h, "account.refused").is_empty());
}

#[tokio::test]
async fn with_a_plugin_authorizer_the_chief_still_applies_and_the_routes_decline() {
    let h = staffed_sql(&["account:boss"], true).await;
    let boss = login(&h.base, "boss").await;
    let (status, view) = call(&h, &boss, Method::GET, "/v1/user/access", None, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["managed_by"], "test");
    let (status, body) = call(
        &h,
        &boss,
        Method::PUT,
        "/v1/user/access/rules",
        Some("0"),
        Some(json!({"rules": []})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("managed by"));
    let nobody = login(&h.base, "nobody").await;
    let (status, _) = call(&h, &nobody, Method::GET, "/v1/user/access", None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
