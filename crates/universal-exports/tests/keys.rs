//! Several sign-in providers, and people's own API keys: who is let in by what, what a key may
//! do, and that a key's value is shown once and kept out of every record.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, ConfigProvider, ConfigRecord,
    ConfigScope, Description, Directory, FileRegistry, Lookup, Person, PutOutcome, SqlProvider,
    Verdict, guatiao::library::ProviderError,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use universal_exports::{
    AccessProviders, Gateway, GatewayConfig, GatewayLimits, KeyProvider, LiveAccess,
};

const STATION_TOKEN: &str = "exports-test-credential-with-more-than-32-bytes";
const ORIGIN: &str = "https://exports.example";

/// People: a password per name, one directory.
struct People(Vec<&'static str>);

impl Authenticator for People {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        Ok(match (attempt.name, attempt.secret) {
            (Some(name), Some(secret)) if secret == "secret" && self.0.contains(&name.as_str()) => {
                Verdict::Valid(Lookup {
                    by: "name".to_owned(),
                    value: name,
                })
            }
            _ => Verdict::Invalid,
        })
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            takes: Some(ai6::directorates::m::access::takes::PASSWORD.to_owned()),
            ..Description::default()
        })
    }
}

impl Directory for People {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        let name = lookup.value.trim_end_matches("-id").to_owned();
        Ok(self.0.contains(&name.as_str()).then(|| Person {
            id: format!("{name}-id"),
            name,
            ..Person::default()
        }))
    }
}

/// A second people's provider: it claims only names that start `ext-`, and says `Unknown` of
/// the rest; `ext-bad` it knows and refuses.
struct External;

impl Authenticator for External {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        Ok(match (attempt.name, attempt.secret) {
            (Some(name), Some(_)) if name == "ext-bad" => Verdict::Invalid,
            (Some(name), Some(secret)) if name.starts_with("ext-") && secret == "secret" => {
                Verdict::Valid(Lookup {
                    by: "name".to_owned(),
                    value: name,
                })
            }
            _ => Verdict::Unknown,
        })
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[derive(Clone)]
struct Held {
    id: String,
    name: String,
    secret: String,
    groups: Vec<String>,
    expires_unix: Option<i64>,
}

/// A key provider: keys begin `ck_`, and anything else is not its to judge.
#[derive(Default)]
struct Keys(Mutex<Vec<Held>>);

impl Authenticator for Keys {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let Some(secret) = attempt.secret.filter(|secret| secret.starts_with("ck_")) else {
            return Ok(Verdict::Unknown);
        };
        Ok(
            match self
                .0
                .lock()
                .unwrap()
                .iter()
                .find(|held| held.secret == secret)
            {
                Some(held) if held.expires_unix.is_some_and(|end| end <= now_unix()) => {
                    Verdict::Invalid
                }
                Some(held) => Verdict::Valid(Lookup {
                    by: "id".to_owned(),
                    value: held.id.clone(),
                }),
                None => Verdict::Unknown,
            },
        )
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            takes: Some(ai6::directorates::m::access::takes::KEY.to_owned()),
            ..Description::default()
        })
    }
}

impl Directory for Keys {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|held| {
                if lookup.by == "id" {
                    held.id == lookup.value
                } else {
                    held.name == lookup.value
                }
            })
            .map(|held| Person {
                id: held.id.clone(),
                name: held.name.clone(),
                groups: held.groups.clone(),
                ..Person::default()
            }))
    }
}

impl Accounts for Keys {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|held| AccountInfo {
                id: held.id.clone(),
                name: held.name.clone(),
                groups: held.groups.clone(),
                ..AccountInfo::default()
            })
            .collect())
    }

    fn put(&self, put: AccountPut) -> Result<PutOutcome, ProviderError> {
        let mut held = self.0.lock().unwrap();
        if let Some(found) = held.iter_mut().find(|held| held.name == put.name) {
            if let Some(groups) = put.groups {
                found.groups = groups;
            }
            return Ok(PutOutcome::default());
        }
        let secret = format!("ck_{}", uuid::Uuid::new_v4().simple());
        let id = format!("key-{}", held.len() + 1);
        held.push(Held {
            id,
            name: put.name,
            secret: secret.clone(),
            groups: put.groups.unwrap_or_default(),
            expires_unix: put.expires_unix,
        });
        Ok(PutOutcome {
            secret: Some(secret),
        })
    }

    fn remove(&self, name: String) -> Result<(), ProviderError> {
        let mut held = self.0.lock().unwrap();
        let before = held.len();
        held.retain(|held| held.name != name);
        if held.len() == before {
            return Err(ai6::guatiao::library::ProviderError::new(
                ai6::guatiao::Status::GUATIAO_ERR_NOT_FOUND,
                "no such account",
            ));
        }
        Ok(())
    }
}

struct Harness {
    base: String,
    registry: FileRegistry,
    provider: Arc<dyn ConfigProvider>,
    live: Arc<LiveAccess>,
    keys: Arc<Keys>,
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

/// Alice may manage charges and the roster; the admin manages accounts; the group
/// `deployers` (the service account `deploy` is in it) manages charges.
fn policy(alice_roster: bool) -> ai6::AccessPolicy {
    let mut policy = ai6::seeded_policy();
    policy.roles.insert(
        "watcher".into(),
        ai6::RoleDef::new(["config.read", "config.manage"]),
    );
    policy.roles.insert(
        "rostering".into(),
        ai6::RoleDef::new(["roster.read", "roster.manage"]),
    );
    policy
        .roles
        .insert("accountant".into(), ai6::RoleDef::new(["account.manage"]));
    let mut rules = vec![
        ("account:alice", "config:cables", "watcher"),
        ("account:admin", "config:accounts", "accountant"),
        ("group:deployers", "config:cables", "watcher"),
        ("account:robot", "config:cables", "watcher"),
    ];
    if alice_roster {
        rules.push(("account:alice", "config:roster", "rostering"));
    }
    for (subject, scope, role) in rules {
        policy.rules.push(ai6::RuleDef::new(subject, scope, role));
    }
    policy.chief = Vec::new();
    policy
}

async fn serve(with_keys: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", dir.path().join("config.db").display());
    let provider = Arc::new(SqlProvider::open(&location).unwrap());
    provider
        .put(
            ConfigRecord::Access,
            &ConfigScope::Global,
            "settings",
            &policy(true).to_json(),
        )
        .unwrap();
    let registry = FileRegistry::open(dir.path().join("registry"));
    let registry_dyn: Arc<dyn RegistryBackend> = Arc::new(registry.clone());
    let live = Arc::new(LiveAccess::new(provider.clone(), Vec::new(), None).unwrap());
    live.seed(&provider.load("").unwrap(), &registry_dyn)
        .unwrap();
    live.apply(&provider.load("").unwrap());
    let people = Arc::new(People(vec!["alice", "bob", "admin", "ext-1", "ext-bad"]));
    let keys = Arc::new(Keys::default());
    let mut access = AccessProviders::new(Arc::new(External), people.clone(), live.clone())
        .with_authenticator(people);
    if with_keys {
        access = access.with_keys(KeyProvider::new(keys.clone(), keys.clone(), keys.clone()));
    }
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
        access,
    )
    .with_access_admin(live.clone())
    .with_roster(provider.clone())
    .with_accounts(keys.clone());
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
        live,
        keys,
        stop: Some(stop),
        _dir: dir,
    }
}

async fn try_login(base: &str, name: &str, secret: &str) -> StatusCode {
    reqwest::Client::new()
        .post(format!("{base}/v1/auth/login"))
        .header("origin", ORIGIN)
        .header("x-forwarded-proto", "https")
        .json(&json!({"name": name, "secret": secret}))
        .send()
        .await
        .unwrap()
        .status()
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
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = reqwest::Client::new()
        .request(method, format!("{}{path}", h.base))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or_default())
}

fn apply_body(kind: &str, key: &str, definition: Value) -> Value {
    json!({"records": [{"kind": kind, "key": key, "definition": definition}]})
}

async fn apply_charge(h: &Harness, token: &str) -> StatusCode {
    call(
        h,
        token,
        Method::POST,
        "/v1/user/resources/apply",
        Some(apply_body("cables", "settings", json!({}))),
    )
    .await
    .0
}

async fn apply_agent(h: &Harness, token: &str) -> StatusCode {
    call(
        h,
        token,
        Method::POST,
        "/v1/user/resources/apply",
        Some(apply_body(
            "agent",
            "007",
            json!({"handler": "mock-agent", "clearance": "00"}),
        )),
    )
    .await
    .0
}

async fn make_key(h: &Harness, token: &str, scopes: &[&str]) -> (String, String) {
    let (status, made) = call(
        h,
        token,
        Method::POST,
        "/v1/user/keys",
        Some(json!({"name": "deploy", "scopes": scopes})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    (
        made["key"]["id"].as_str().unwrap().to_owned(),
        made["secret"].as_str().unwrap().to_owned(),
    )
}

#[tokio::test]
async fn people_sign_in_by_password_and_accounts_by_key_each_refused_by_the_other() {
    let h = serve(true).await;
    let admin = login(&h.base, "admin").await;
    // A service account, made under account.manage, with a key.
    let (status, made) = call(
        &h,
        &admin,
        Method::PUT,
        "/v1/user/accounts/robot",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let key = made["secret"].as_str().unwrap().to_owned();
    assert!(key.starts_with("ck_"));

    // The key is the bearer of a request and acts as its account.
    assert_eq!(apply_charge(&h, &key).await, StatusCode::OK);
    // A password is no bearer, and a key is no password.
    assert_eq!(apply_charge(&h, "secret").await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        try_login(&h.base, "robot", &key).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        try_login(&h.base, "alice", &key).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        try_login(&h.base, "alice", "secret").await,
        StatusCode::CREATED
    );
    let (status, methods) = call(&h, "", Method::GET, "/v1/auth/methods", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        methods["methods"],
        json!(["password"]),
        "keys are not a sign-in"
    );
}

#[tokio::test]
async fn removing_the_key_provider_refuses_keys_and_leaves_sign_in_working() {
    let with = serve(true).await;
    let admin = login(&with.base, "admin").await;
    let (_, made) = call(
        &with,
        &admin,
        Method::PUT,
        "/v1/user/accounts/robot",
        Some(json!({})),
    )
    .await;
    let key = made["secret"].as_str().unwrap().to_owned();
    assert_eq!(apply_charge(&with, &key).await, StatusCode::OK);

    let without = serve(false).await;
    // The same value, with no provider to claim it.
    without
        .keys
        .0
        .lock()
        .unwrap()
        .push(with.keys.0.lock().unwrap()[0].clone());
    assert_eq!(apply_charge(&without, &key).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        try_login(&without.base, "alice", "secret").await,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn each_credential_goes_to_the_provider_that_claims_it_in_list_order() {
    let h = serve(false).await;
    // `ext-1` is the first provider's; `alice` falls through it to the second; `ext-bad` is
    // refused by the first, which claimed it, though the second would have let it in.
    assert_eq!(
        try_login(&h.base, "ext-1", "secret").await,
        StatusCode::CREATED
    );
    assert_eq!(
        try_login(&h.base, "alice", "secret").await,
        StatusCode::CREATED
    );
    assert_eq!(
        try_login(&h.base, "ext-bad", "secret").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        try_login(&h.base, "alice", "wrong").await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_personal_key_does_what_its_scope_names_and_no_more_than_its_person_may() {
    let h = serve(true).await;
    let alice = login(&h.base, "alice").await;
    assert_eq!(
        apply_agent(&h, &alice).await,
        StatusCode::OK,
        "alice may edit the roster"
    );

    let (_, key) = make_key(&h, &alice, &["config.manage@config:cables"]).await;
    assert_eq!(apply_charge(&h, &key).await, StatusCode::OK);
    assert_eq!(
        apply_agent(&h, &key).await,
        StatusCode::FORBIDDEN,
        "not in its scope"
    );
    // A key does not make keys, nor read the list of them.
    let (status, _) = call(
        &h,
        &key,
        Method::POST,
        "/v1/user/keys",
        Some(json!({"name": "x", "scopes": ["config.manage@config:cables"]})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        call(&h, &key, Method::GET, "/v1/user/keys", None).await.0,
        StatusCode::FORBIDDEN
    );

    // What the person does not hold, or does not name, makes no key.
    let bob = login(&h.base, "bob").await;
    for (token, scopes) in [
        (&bob, json!(["config.manage@config:cables"])),
        (&alice, json!(["account.manage@config:accounts"])),
        (&alice, json!([])),
        (&alice, json!(["config.manage"])),
        (&alice, json!(["nothing@config:cables"])),
        (&alice, json!(["config.manage@*"])),
    ] {
        let (status, body) = call(
            &h,
            token,
            Method::POST,
            "/v1/user/keys",
            Some(json!({"name": "x", "scopes": scopes})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{scopes}: {body}");
    }
}

#[tokio::test]
async fn a_key_loses_what_its_person_loses() {
    let h = serve(true).await;
    let alice = login(&h.base, "alice").await;
    let (_, key) = make_key(&h, &alice, &["config.manage@config:cables"]).await;
    assert_eq!(apply_charge(&h, &key).await, StatusCode::OK);

    h.provider
        .put(ConfigRecord::Access, &ConfigScope::Global, "settings", &{
            let mut without = policy(false);
            without.rules.retain(|rule| rule.subject != "account:alice");
            without.to_json()
        })
        .unwrap();
    h.live.apply(&h.provider.load("").unwrap());
    assert_eq!(apply_charge(&h, &key).await, StatusCode::FORBIDDEN);
    assert_eq!(apply_charge(&h, &alice).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn revoking_and_expiry_refuse_a_key_at_once() {
    let h = serve(true).await;
    let alice = login(&h.base, "alice").await;
    let (revoked, first) = make_key(&h, &alice, &["config.manage@config:cables"]).await;
    let (expiring, second) = make_key(&h, &alice, &["config.manage@config:cables"]).await;
    assert_eq!(apply_charge(&h, &first).await, StatusCode::OK);
    assert_eq!(apply_charge(&h, &second).await, StatusCode::OK);

    let (status, _) = call(
        &h,
        &alice,
        Method::DELETE,
        &format!("/v1/user/keys/{revoked}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(apply_charge(&h, &first).await, StatusCode::UNAUTHORIZED);

    let mut record = h.registry.api_key(&expiring).unwrap().unwrap();
    record.expires_at = Some(1.0);
    h.registry.put_api_key(&record).unwrap();
    assert_eq!(apply_charge(&h, &second).await, StatusCode::UNAUTHORIZED);
    // A key's life defaults from the setting, and an explicit 0 never ends.
    let (_, made) = call(
        &h,
        &alice,
        Method::POST,
        "/v1/user/keys",
        Some(
            json!({"name": "forever", "scopes": ["config.manage@config:cables"], "expires_in": 0}),
        ),
    )
    .await;
    assert_eq!(made["key"]["expires_at"], Value::Null, "{made}");
    let (_, made) = call(
        &h,
        &alice,
        Method::POST,
        "/v1/user/keys",
        Some(json!({"name": "default", "scopes": ["config.manage@config:cables"]})),
    )
    .await;
    assert!(
        made["key"]["expires_at"].as_f64().unwrap() > made["key"]["created_at"].as_f64().unwrap()
    );
}

#[tokio::test]
async fn keys_are_a_persons_own_and_an_administrator_sees_every_one() {
    let h = serve(true).await;
    let alice = login(&h.base, "alice").await;
    let bob = login(&h.base, "bob").await;
    let admin = login(&h.base, "admin").await;
    let (id, key) = make_key(&h, &alice, &["config.manage@config:cables"]).await;

    let (_, mine) = call(&h, &alice, Method::GET, "/v1/user/keys", None).await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    assert_eq!(mine[0]["name"], "deploy");
    assert_eq!(mine[0]["scopes"], json!(["config.manage@config:cables"]));
    assert_eq!(mine[0]["last_used_at"], Value::Null);
    let (_, theirs) = call(&h, &bob, Method::GET, "/v1/user/keys", None).await;
    assert_eq!(theirs, json!([]));
    assert_eq!(
        call(&h, &bob, Method::GET, "/v1/user/keys?all=true", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, _) = call(
        &h,
        &bob,
        Method::DELETE,
        &format!("/v1/user/keys/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another person's key is not found"
    );
    assert_eq!(apply_charge(&h, &key).await, StatusCode::OK, "still works");

    // The key's use is written down, not its value.
    let (_, used) = call(&h, &alice, Method::GET, "/v1/user/keys", None).await;
    assert!(used[0]["last_used_at"].as_f64().is_some(), "{used}");
    assert!(!used.to_string().contains(&key));

    let (_, everyone) = call(&h, &admin, Method::GET, "/v1/user/keys?all=true", None).await;
    assert_eq!(everyone.as_array().unwrap().len(), 1);
    assert_eq!(everyone[0]["owner_id"], "alice-id");
    let (status, _) = call(
        &h,
        &admin,
        Method::DELETE,
        &format!("/v1/user/keys/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(apply_charge(&h, &key).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_keys_value_is_in_no_audit_record_or_error() {
    let h = serve(true).await;
    let alice = login(&h.base, "alice").await;
    let (id, key) = make_key(&h, &alice, &["config.manage@config:cables"]).await;
    assert_eq!(apply_charge(&h, &key).await, StatusCode::OK);
    assert_eq!(apply_agent(&h, &key).await, StatusCode::FORBIDDEN);
    let (_, refused) = call(&h, &key, Method::GET, "/v1/user/keys", None).await;
    call(
        &h,
        &alice,
        Method::DELETE,
        &format!("/v1/user/keys/{id}"),
        None,
    )
    .await;
    let (_, after) = call(&h, &key, Method::GET, "/v1/user/keys", None).await;

    let audit = serde_json::to_string(&h.registry.list_audit(200).unwrap()).unwrap();
    assert!(
        !audit.contains(&key) && !audit.contains(&key[3..]),
        "{audit}"
    );
    assert!(!refused.to_string().contains(&key) && !after.to_string().contains(&key));
    assert!(
        audit.contains("key.create") && audit.contains("key.revoke"),
        "{audit}"
    );
    // The apply made under the key names the key beside its person.
    assert!(audit.contains(&format!("alice-id via key {id}")), "{audit}");
}

#[tokio::test]
async fn a_service_accounts_key_acts_with_the_accounts_own_grants() {
    let h = serve(true).await;
    let admin = login(&h.base, "admin").await;
    let (_, made) = call(
        &h,
        &admin,
        Method::PUT,
        "/v1/user/accounts/deploy",
        Some(json!({})),
    )
    .await;
    let key = made["secret"].as_str().unwrap().to_owned();
    // Groups confer roles; the account gets them from an administrator's change.
    h.keys.0.lock().unwrap()[0].groups = vec!["deployers".to_owned()];
    assert_eq!(apply_charge(&h, &key).await, StatusCode::OK);
    assert_eq!(apply_agent(&h, &key).await, StatusCode::FORBIDDEN);
    // An account may not take a personal key's name.
    let (status, _) = call(
        &h,
        &admin,
        Method::PUT,
        "/v1/user/accounts/uk-sneaky",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_personal_keys_account_is_not_listed_among_the_accounts() {
    let h = serve(true).await;
    let alice = login(&h.base, "alice").await;
    let admin = login(&h.base, "admin").await;
    make_key(&h, &alice, &["config.manage@config:cables"]).await;
    let (_, accounts) = call(&h, &admin, Method::GET, "/v1/user/accounts", None).await;
    assert_eq!(accounts, json!([]));
}
