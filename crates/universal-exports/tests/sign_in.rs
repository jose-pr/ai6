//! The sign-in policy through the gateway's real routes: what the page is offered, what a login
//! may present from where, what a renewal decides again, and how the policy is changed.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai6::directorates::m::access::SignInPolicy;
use ai6::directorates::registry::backend::*;
use ai6::{
    AccessTokenRecord, Attempt, Authenticator, ConfigProvider, ConfigRecord, ConfigScope,
    Description, Directory, FileRegistry, Lookup, Person, ProviderConfig, SqlProvider, Verdict,
    guatiao::library::ProviderError,
};
use coqui_access_signin::vectors::FLOWS;
use coqui_access_signin::{RuleDef, SignInRules};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use universal_exports::{
    AccessProviders, Gateway, GatewayConfig, GatewayLimits, LiveAccess, LiveSignIn, LoginLimits,
    SignInLoader,
};

const STATION_TOKEN: &str = "exports-test-credential-with-more-than-32-bytes";
const ORIGIN: &str = "https://exports.example";
const PROXY_SECRET: &str = "a-proxy-secret-that-is-long-enough";

/// What the stand-in providers were asked, to show who was and was not.
#[derive(Default)]
struct Counts {
    /// Authenticators asked a name and a password.
    passwords: AtomicUsize,
    /// Identities the reverse proxy asserted that the directory was asked about.
    proxied: AtomicUsize,
    asked: Mutex<Vec<String>>,
}

/// A stand-in sign-in provider: `local` accepts alice and bob with `pw` and refuses other secrets;
/// `ldap` does not know whom it is asked about.
struct Stand {
    id: &'static str,
    counts: Arc<Counts>,
}

impl Authenticator for Stand {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        self.counts.passwords.fetch_add(1, Ordering::SeqCst);
        self.counts.asked.lock().unwrap().push(self.id.to_owned());
        Ok(
            match (self.id, attempt.name.as_deref(), attempt.secret.as_deref()) {
                ("ldap", ..) => Verdict::Unknown,
                (_, Some(who @ ("alice" | "bob")), Some("pw")) => Verdict::Valid(Lookup {
                    by: "name".to_owned(),
                    value: who.to_owned(),
                    ..Default::default()
                }),
                _ => Verdict::Invalid,
            },
        )
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            takes: Some(ai6::directorates::m::access::takes::PASSWORD.to_owned()),
            ..Description::default()
        })
    }
}

/// Alice, an administrator, and Bob.
struct People(Arc<Counts>);

impl Directory for People {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        let name = match lookup.by.as_str() {
            "proxy" => {
                self.0.proxied.fetch_add(1, Ordering::SeqCst);
                lookup.value
            }
            "id" => lookup.value.trim_start_matches("id-").to_owned(),
            _ => lookup.value,
        };
        Ok(match name.as_str() {
            "alice" | "bob" => Some(Person {
                id: format!("id-{name}"),
                groups: if name == "alice" {
                    vec!["admins".to_owned()]
                } else {
                    Vec::new()
                },
                name,
                ..Person::default()
            }),
            _ => None,
        })
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The rules provider's configuration as the policy the stand-in loader builds.
fn rules(config: &Value) -> Result<SignInRules, String> {
    let mut builder = SignInRules::builder();
    for rule in config["rule"].as_array().cloned().unwrap_or_default() {
        let chains: Vec<Vec<String>> = rule["allow"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|chain| strings(Some(chain)))
            .collect();
        let mut def = RuleDef::new(chains)
            .networks(strings(rule.get("networks")))
            .clients(strings(rule.get("clients")))
            .subjects(strings(rule.get("subjects")))
            .providers(strings(rule.get("providers")));
        if rule.get("purposes").is_some() {
            def = def.purposes(strings(rule.get("purposes")));
        }
        builder = builder.rule(def);
    }
    builder.build().map_err(|error| error.to_string())
}

/// Builds the rules provider from a stored configuration, refusing any other provider.
struct Loader;

impl SignInLoader for Loader {
    fn load(
        &self,
        provider: &str,
        config: Option<&ProviderConfig>,
    ) -> Result<Arc<dyn SignInPolicy>, String> {
        if provider != "coqui_signin_rules" {
            return Err(format!("{provider} is not a sign-in policy provider here"));
        }
        let config = config.map_or_else(|| json!({}), |config| Value::Object(config.0.clone()));
        rules(&config).map(|policy| Arc::new(policy) as Arc<dyn SignInPolicy>)
    }
}

/// `certificate` as the gateway names the method a trusted reverse proxy gives it.
fn proxied(value: &Value) -> Value {
    match value {
        Value::String(text) if text == "certificate" => json!("proxy"),
        Value::Array(items) => Value::Array(items.iter().map(proxied).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), proxied(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn limits(per_address: u32) -> LoginLimits {
    LoginLimits {
        per_address: std::num::NonZeroU32::new(per_address).unwrap(),
        address_header: Some(axum::http::HeaderName::from_static("x-forwarded-for")),
        ..LoginLimits::default()
    }
}

struct Served {
    base: String,
    counts: Arc<Counts>,
    registry: FileRegistry,
    _dir: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// A gateway over the stand-in providers; `ldap` is asked first when `with_ldap`.
fn gateway_over(
    counts: &Arc<Counts>,
    with_ldap: bool,
    registry: &FileRegistry,
    per_address: u32,
    policy: Option<SignInRules>,
) -> Gateway {
    let people = Arc::new(People(counts.clone()));
    let local = Arc::new(Stand {
        id: "local",
        counts: counts.clone(),
    });
    let dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn ConfigProvider> = Arc::new(
        SqlProvider::open(&format!("sqlite:{}", dir.path().join("c.db").display())).unwrap(),
    );
    std::mem::forget(dir);
    let live = Arc::new(LiveAccess::new(provider, vec!["account:alice".to_owned()], None).unwrap());
    let mut access = if with_ldap {
        AccessProviders::new(
            Arc::new(Stand {
                id: "ldap",
                counts: counts.clone(),
            }),
            people.clone(),
            live.clone(),
        )
        .named("ldap")
        .with_authenticator(local)
        .named("local")
    } else {
        AccessProviders::new(local, people.clone(), live.clone()).named("local")
    };
    if let Some(policy) = policy {
        access = access.with_sign_in_policy(Arc::new(policy));
    }
    access = access
        .with_proxy_identity(
            axum::http::HeaderName::from_static("x-forwarded-user"),
            "proxy",
        )
        .with_proxy_secret(PROXY_SECRET, 8)
        .unwrap();
    let config = GatewayConfig::new(
        ORIGIN,
        STATION_TOKEN,
        GatewayLimits::default(),
        Duration::from_secs(60),
        Duration::from_secs(30),
    )
    .unwrap()
    .with_login_limits(limits(per_address));
    let pages = tempfile::tempdir().unwrap();
    let pages_dir = pages.path().to_path_buf();
    std::mem::forget(pages);
    Gateway::with_access(config, Arc::new(registry.clone()), access)
        .with_access_admin(live)
        .with_pages(universal_exports::PagesConfig::new(pages_dir))
        .unwrap()
}

async fn listen(
    gateway: Gateway,
    counts: Arc<Counts>,
    registry: FileRegistry,
    dir: tempfile::TempDir,
) -> Served {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            gateway
                .router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    Served {
        base,
        counts,
        registry,
        _dir: dir,
        server,
    }
}

/// A gateway under a policy that stays as it is.
async fn serve(policy: Option<Value>, with_ldap: bool, per_address: u32) -> Served {
    let counts = Arc::new(Counts::default());
    let dir = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(dir.path());
    let policy = policy.map(|policy| rules(&policy).unwrap());
    let gateway = gateway_over(&counts, with_ldap, &registry, per_address, policy);
    listen(gateway, counts, registry, dir).await
}

/// What a request carries to be a browser's through the trusted proxy.
fn browser(request: reqwest::RequestBuilder, address: Option<&str>) -> reqwest::RequestBuilder {
    let request = request
        .header("origin", ORIGIN)
        .header("x-forwarded-proto", "https");
    match address {
        Some(address) => request.header("x-forwarded-for", address),
        None => request,
    }
}

async fn methods(h: &Served, address: Option<&str>, origin: bool) -> Vec<String> {
    let mut request = reqwest::Client::new().get(format!("{}/v1/auth/methods", h.base));
    if origin {
        request = browser(request, address);
    } else if let Some(address) = address {
        request = request.header("x-forwarded-for", address);
    }
    let answer: Value = request.send().await.unwrap().json().await.unwrap();
    strings(Some(&answer["methods"]))
}

/// A password sign-in, as a browser when `origin`.
async fn password(
    h: &Served,
    address: Option<&str>,
    name: &str,
    secret: &str,
    origin: bool,
) -> (StatusCode, Value) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/v1/auth/login", h.base))
        .json(&json!({"name": name, "secret": secret}));
    if origin {
        request = browser(request, address);
    } else if let Some(address) = address {
        request = request.header("x-forwarded-for", address);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or_default())
}

/// A sign-in by the identity the reverse proxy asserts.
async fn proxy(h: &Served, address: Option<&str>, name: &str) -> (StatusCode, Value) {
    let request = reqwest::Client::new()
        .post(format!("{}/v1/auth/login", h.base))
        .header("x-forwarded-user", name)
        .header("x-ai6-proxy-secret", PROXY_SECRET)
        .json(&json!({}));
    let response = browser(request, address).send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or_default())
}

async fn renew(h: &Served, address: Option<&str>, token: &str) -> StatusCode {
    browser(
        reqwest::Client::new()
            .post(format!("{}/v1/auth/renew", h.base))
            .bearer_auth(token),
        address,
    )
    .send()
    .await
    .unwrap()
    .status()
}

fn office_rule() -> Value {
    json!({"rule": [
        {"networks": ["10.0.0.0/8"], "allow": [["password"], ["certificate"]]},
        {"allow": [["certificate"]]}
    ]})
}

/// The flows that need no second factor and no pending sign-in.
fn single_step_flows() -> Vec<Value> {
    let document: Value = serde_json::from_str(FLOWS).unwrap();
    document["flows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|flow| {
            flow["steps"].as_array().unwrap().iter().all(|step| {
                let method = step["method"].as_str().unwrap_or_default();
                step["expect"]["result"] != "next"
                    && !["totp", "recovery_code"].contains(&method)
                    && !strings(step["expect"].get("methods")).contains(&"totp".to_owned())
            })
        })
        .cloned()
        .collect()
}

#[tokio::test]
async fn every_shared_flow_without_a_second_factor_goes_the_same_through_the_routes() {
    let flows = single_step_flows();
    assert!(flows.len() >= 8, "{} flows", flows.len());
    for flow in &flows {
        let name = flow["name"].as_str().unwrap();
        // The vectors say `certificate`; this gateway meets a certificate as the identity a
        // trusted proxy asserts.
        let config = proxied(&flow["config"]);
        let ldap = flow["config"].to_string().contains("providers");
        let h = serve(Some(config), ldap, 50).await;
        let mut token: Option<String> = None;
        for (index, step) in flow["steps"].as_array().unwrap().iter().enumerate() {
            let at = format!("{name}, step {index}");
            let address = step["address"].as_str();
            let (passwords, proxied_before) = (
                h.counts.passwords.load(Ordering::SeqCst),
                h.counts.proxied.load(Ordering::SeqCst),
            );
            h.counts.asked.lock().unwrap().clear();
            let expect = &step["expect"];
            let result = match step["action"].as_str().unwrap() {
                "offer" => {
                    let offered = methods(&h, address, true).await;
                    assert_eq!(offered, strings(Some(&proxied(&expect["methods"]))), "{at}");
                    "methods"
                }
                "present" => {
                    let who = step["name"].as_str().unwrap();
                    let (status, body) = match step["method"].as_str().unwrap() {
                        "password" => {
                            password(&h, address, who, step["secret"].as_str().unwrap(), true).await
                        }
                        _ => proxy(&h, address, who).await,
                    };
                    match status {
                        StatusCode::CREATED => {
                            token = body["token"].as_str().map(str::to_owned);
                            "signed_in"
                        }
                        StatusCode::FORBIDDEN => "refused_unasked",
                        StatusCode::UNAUTHORIZED => "failed",
                        other => panic!("{at}: {other}"),
                    }
                }
                "resume" => match renew(&h, address, token.as_deref().expect("signed in")).await {
                    StatusCode::OK => "signed_in",
                    StatusCode::UNAUTHORIZED => "failed",
                    other => panic!("{at}: {other}"),
                },
                other => panic!("{at}: an action {other}"),
            };
            assert_eq!(result, expect["result"].as_str().unwrap(), "{at}");
            let asked = (h.counts.passwords.load(Ordering::SeqCst) - passwords)
                + (h.counts.proxied.load(Ordering::SeqCst) - proxied_before);
            assert_eq!(asked as i64, expect["authenticator_calls"], "{at}");
            if let Some(providers) = expect.get("asked") {
                assert_eq!(
                    *h.counts.asked.lock().unwrap(),
                    strings(Some(providers)),
                    "{at}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_page_is_offered_what_the_office_rule_says_for_its_own_address() {
    let h = serve(Some(office_rule()), false, 50).await;
    assert_eq!(
        methods(&h, Some("10.1.2.3"), true).await,
        ["password", "certificate"]
    );
    assert_eq!(
        methods(&h, Some("203.0.113.7"), true).await,
        ["certificate"]
    );
    assert_eq!(methods(&h, None, true).await, ["certificate"]);
    // A policy that names a client tells a browser from any other by the origin it names.
    let h = serve(
        Some(json!({"rule": [{"clients": ["browser"], "allow": [["password"]]}]})),
        false,
        50,
    )
    .await;
    assert_eq!(methods(&h, None, true).await, ["password"]);
    assert!(methods(&h, None, false).await.is_empty());
    // A browser sends no origin on a same-origin GET; what it does send is a hint, and only that.
    let same_origin: Value = reqwest::Client::new()
        .get(format!("{}/v1/auth/methods", h.base))
        .header("sec-fetch-site", "same-origin")
        .header("sec-fetch-mode", "cors")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(same_origin["methods"], json!(["password"]));
}

#[tokio::test]
async fn the_policy_sees_the_address_the_proxy_said_and_no_other() {
    let h = serve(Some(office_rule()), false, 50).await;
    let ask = |lines: &'static [&'static str]| {
        let h = &h;
        async move {
            let mut request = reqwest::Client::new().get(format!("{}/v1/auth/methods", h.base));
            for line in lines {
                request = request.header("x-forwarded-for", *line);
            }
            let answer: Value = request.send().await.unwrap().json().await.unwrap();
            strings(Some(&answer["methods"]))
        }
    };
    let inside = ["password", "certificate"];
    // The client's own earlier line is ignored; the proxy's, last, counts.
    assert_eq!(ask(&["10.1.2.3", "203.0.113.7"]).await, ["certificate"]);
    assert_eq!(ask(&["6.6.6.6", "10.1.2.3"]).await, inside);
    // An address the proxy did not give is no address, though the peer is loopback.
    assert_eq!(ask(&[]).await, ["certificate"]);
    assert_eq!(ask(&["unknown"]).await, ["certificate"]);
    // A mapped address is the IPv4 address.
    assert_eq!(ask(&["::ffff:10.1.2.3"]).await, inside);
}

#[tokio::test]
async fn an_unset_policy_offers_each_configured_method_from_anywhere() {
    let h = serve(None, false, 50).await;
    for address in [Some("10.1.2.3"), Some("203.0.113.7"), None] {
        assert_eq!(methods(&h, address, true).await, ["password", "proxy"]);
    }
    let (status, _) = password(&h, Some("203.0.113.7"), "alice", "pw", false).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = proxy(&h, Some("203.0.113.7"), "bob").await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn a_refused_password_reaches_no_authenticator_and_costs_its_address_no_try() {
    // Two tries a minute from an address: ten refusals would have used them up if counted.
    let h = serve(
        Some(json!({"rule": [{"clients": ["browser"], "allow": [["password"]]}]})),
        false,
        2,
    )
    .await;
    for _ in 0..10 {
        let (status, _) = password(&h, Some("203.0.113.7"), "alice", "wrong", false).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    assert_eq!(h.counts.passwords.load(Ordering::SeqCst), 0);
    let (status, _) = password(&h, Some("203.0.113.7"), "alice", "pw", true).await;
    assert_eq!(status, StatusCode::CREATED, "its tries were untouched");
    // The same address is held off once it spends them on wrong passwords.
    let (first, _) = password(&h, Some("203.0.113.8"), "alice", "wrong", true).await;
    let (second, _) = password(&h, Some("203.0.113.8"), "alice", "wrong", true).await;
    let (third, _) = password(&h, Some("203.0.113.8"), "alice", "pw", true).await;
    assert_eq!(
        [first, second, third],
        [
            StatusCode::UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS
        ]
    );
}

#[tokio::test]
async fn a_sign_in_is_decided_again_by_the_steps_it_was_opened_with_when_it_renews() {
    let h = serve(Some(office_rule()), false, 50).await;
    let (status, body) = password(&h, Some("10.1.2.3"), "alice", "pw", true).await;
    assert_eq!(status, StatusCode::CREATED);
    let token = body["token"].as_str().unwrap().to_owned();
    assert_eq!(renew(&h, Some("10.1.2.3"), &token).await, StatusCode::OK);
    assert_eq!(
        renew(&h, Some("203.0.113.7"), &token).await,
        StatusCode::UNAUTHORIZED,
        "a password does not carry outside"
    );
    assert_eq!(
        renew(&h, None, &token).await,
        StatusCode::UNAUTHORIZED,
        "an address that is not known matches no network"
    );
    // The steps are in the record the Registry keeps.
    let record: AccessTokenRecord = h
        .registry
        .login_by_id(body["token_id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(record.factors().len(), 1);
    assert_eq!(record.factors()[0].method(), "password");
    assert_eq!(record.factors()[0].provider(), Some("local"));
    assert_eq!(record.factors()[0].person(), Some("id-alice"));
}

#[tokio::test]
async fn a_browsers_session_cookie_is_decided_again_at_its_address_when_it_resumes() {
    let h = serve(Some(office_rule()), false, 50).await;
    let signed_in = browser(
        reqwest::Client::new()
            .post(format!("{}/v1/auth/login", h.base))
            .json(&json!({"name": "alice", "secret": "pw"})),
        Some("10.1.2.3"),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(signed_in.status(), StatusCode::CREATED);
    let cookie = signed_in
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|value| value.to_str().unwrap().strip_prefix("ai6_session="))
        .map(|rest| rest.split(';').next().unwrap().to_owned())
        .expect("a session cookie");
    let resume = |address: &str| {
        browser(
            reqwest::Client::new()
                .post(format!("{}/v1/auth/resume", h.base))
                .header("cookie", format!("ai6_session={cookie}")),
            Some(address),
        )
        .send()
    };
    assert_eq!(
        resume("10.1.2.3").await.unwrap().status(),
        StatusCode::CREATED
    );
    assert_eq!(
        resume("203.0.113.7").await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_policy_that_offers_nothing_leaves_the_gateway_up_and_refuses_every_sign_in() {
    let nothing = LiveSignIn::fixed(Arc::new(rules(&json!({"rule": []})).unwrap()));
    assert!(nothing.offers_nothing());
    assert!(!LiveSignIn::fixed(Arc::new(rules(&office_rule()).unwrap())).offers_nothing());
    let h = serve(Some(json!({"rule": []})), false, 50).await;
    assert!(methods(&h, None, true).await.is_empty());
    let (status, _) = password(&h, None, "alice", "pw", true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = proxy(&h, None, "alice").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(h.counts.passwords.load(Ordering::SeqCst), 0);
    assert_eq!(h.counts.proxied.load(Ordering::SeqCst), 0);
}

/// A gateway whose policy is kept in the `unex` record of a database and edited by route.
struct Editable {
    served: Served,
    provider: Arc<SqlProvider>,
    live: Arc<LiveSignIn>,
    token: String,
}

async fn editable() -> Editable {
    let counts = Arc::new(Counts::default());
    let dir = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(dir.path());
    let provider = Arc::new(
        SqlProvider::open(&format!(
            "sqlite:{}",
            dir.path().join("config.db").display()
        ))
        .unwrap(),
    );
    let access = Arc::new(
        LiveAccess::new(provider.clone(), vec!["account:alice".to_owned()], None).unwrap(),
    );
    let live =
        Arc::new(LiveSignIn::new(provider.clone(), Arc::new(Loader)).with_providers(["local"]));
    let people = Arc::new(People(counts.clone()));
    let providers = AccessProviders::new(
        Arc::new(Stand {
            id: "local",
            counts: counts.clone(),
        }),
        people,
        access.clone(),
    )
    .named("local")
    .with_sign_in(live.clone());
    let config = GatewayConfig::new(
        ORIGIN,
        STATION_TOKEN,
        GatewayLimits::default(),
        Duration::from_secs(60),
        Duration::from_secs(30),
    )
    .unwrap()
    .with_login_limits(limits(50));
    let gateway = Gateway::with_access(config, Arc::new(registry.clone()), providers)
        .with_access_admin(access);
    let served = listen(gateway, counts, registry, dir).await;
    let (_, body) = password(&served, None, "alice", "pw", true).await;
    let token = body["token"].as_str().unwrap().to_owned();
    Editable {
        served,
        provider,
        live,
        token,
    }
}

async fn call(
    h: &Editable,
    token: &str,
    method: Method,
    revision: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = reqwest::Client::new()
        .request(method, format!("{}/v1/user/access/sign-in", h.served.base))
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

fn audits(h: &Editable, action: &str) -> Vec<ai6::AuditRecord> {
    h.served
        .registry
        .list_audit(200)
        .unwrap()
        .into_iter()
        .filter(|record| record.action == action)
        .collect()
}

fn stored(h: &Editable) -> Option<Value> {
    h.provider
        .definitions(ConfigRecord::Unex, &ConfigScope::Global)
        .unwrap()
        .records
        .into_iter()
        .next()
        .and_then(|(_, record)| record.get("sign_in_policy").cloned())
}

#[tokio::test]
async fn the_policy_is_read_and_changed_through_a_route_guarded_by_access_manage_and_audited() {
    let h = editable().await;
    let (status, view) = call(&h, &h.token, Method::GET, None, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["policy"], Value::Null);
    assert_eq!(view["in_force"], false);
    let revision = view["revision"].as_str().unwrap().to_owned();

    // No `If-Match`: 428. A stale one: 409.
    let owners = json!({"config": office_rule()});
    let (status, _) = call(&h, &h.token, Method::PUT, None, Some(owners.clone())).await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    let (status, _) = call(
        &h,
        &h.token,
        Method::PUT,
        Some("stale"),
        Some(owners.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // A policy that does not build is refused, audited, and stored nowhere.
    let (status, _) = call(
        &h,
        &h.token,
        Method::PUT,
        Some(&revision),
        Some(json!({"config": {"rule": [{"allow": []}]}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &h,
        &h.token,
        Method::PUT,
        Some(&revision),
        Some(json!({"provider": "nobody", "config": {}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(audits(&h, "access.sign_in.refused").len(), 2);
    assert_eq!(stored(&h), None);
    assert!(h.live.policy().is_none());

    // A good one is stored in the `unex` record, in force at once, and audited.
    let (status, changed) = call(&h, &h.token, Method::PUT, Some(&revision), Some(owners)).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["offers_nothing"], false);
    assert_eq!(stored(&h).unwrap()["config"], office_rule());
    assert_eq!(
        methods(&h.served, Some("203.0.113.7"), true).await,
        ["certificate"]
    );
    let (status, _) = password(&h.served, Some("203.0.113.7"), "alice", "pw", true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let updates = audits(&h, "access.sign_in.update");
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].resource.as_deref(), Some("config:access"));
    assert_eq!(updates[0].actor, "id-alice");
    assert_eq!(updates[0].target, "sign_in_policy coqui_signin_rules");

    // The record is read by another gateway on the same database, as a poll would.
    let other = LiveSignIn::new(h.provider.clone(), Arc::new(Loader));
    let snapshot = h.provider.load("").unwrap();
    assert_eq!(
        other.apply(snapshot.settings.unex.sign_in_policy.as_ref()),
        Ok(true)
    );
    assert!(other.policy().is_some());
    assert_eq!(
        other.apply(snapshot.settings.unex.sign_in_policy.as_ref()),
        Ok(false)
    );

    // A policy that offers no sign-in is accepted: the person who is in stays in, and puts it back.
    let (_, view) = call(&h, &h.token, Method::GET, None, None).await;
    let revision = view["revision"].as_str().unwrap().to_owned();
    let (status, changed) = call(
        &h,
        &h.token,
        Method::PUT,
        Some(&revision),
        Some(json!({"config": {"rule": []}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["offers_nothing"], true);
    assert!(methods(&h.served, None, true).await.is_empty());
    let (status, _) = password(&h.served, Some("10.1.2.3"), "alice", "pw", true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, view) = call(&h, &h.token, Method::GET, None, None).await;
    assert_eq!(view["offers_nothing"], true);
    let revision = view["revision"].as_str().unwrap().to_owned();
    let (status, _) = call(&h, &h.token, Method::DELETE, Some(&revision), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored(&h), None);
    assert_eq!(methods(&h.served, None, true).await, ["password"]);
    assert_eq!(audits(&h, "access.sign_in.remove").len(), 1);
    let (status, _) = call(&h, &h.token, Method::DELETE, Some("0"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_person_without_access_manage_reads_and_changes_no_policy() {
    let h = editable().await;
    let (status, body) = password(&h.served, None, "bob", "pw", true).await;
    assert_eq!(status, StatusCode::CREATED);
    let bob = body["token"].as_str().unwrap().to_owned();
    let (status, _) = call(&h, &bob, Method::GET, None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &h,
        &bob,
        Method::PUT,
        Some("0"),
        Some(json!({"config": {"rule": []}})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(stored(&h), None);
}

#[tokio::test]
async fn a_policy_that_does_not_build_offers_nothing_and_is_shown_invalid() {
    let h = editable().await;
    let read = |value: Value| -> ai6::SignInPolicyProvider {
        ai6::directorates::m::records::from_json(&value).unwrap()
    };
    let good = read(json!({"config": office_rule()}));
    assert_eq!(h.live.apply(Some(&good)), Ok(true));
    let bad = read(json!({"config": {"rule": [{"allow": []}]}}));
    assert!(h.live.apply(Some(&bad)).is_err());
    assert!(h.live.invalid().is_some());
    assert!(
        h.live.offers_nothing(),
        "a policy this gateway cannot build offers nothing, not the one before"
    );
    let (_, view) = call(&h, &h.token, Method::GET, None, None).await;
    assert!(view["invalid"].is_string(), "{view}");
    assert_eq!(h.live.apply(Some(&good)), Ok(true));
    assert!(h.live.invalid().is_none());
    assert!(!h.live.offers_nothing());
    assert_eq!(h.live.apply(None), Ok(true));
    assert!(h.live.policy().is_none());
}

/// The digest the gateway keeps of a token it issued, so a test can write a record of its own.
fn digest_of(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"ai6-access-token\0");
    hasher.update(token.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn renewal(h: &Served, token: &str) -> reqwest::Response {
    browser(
        reqwest::Client::new()
            .post(format!("{}/v1/auth/renew", h.base))
            .bearer_auth(token),
        Some("10.1.2.3"),
    )
    .send()
    .await
    .unwrap()
}

#[tokio::test]
async fn a_sign_in_with_no_steps_is_not_complete_and_only_a_stations_token_is_exempt() {
    let h = serve(None, false, 50).await;
    // A record no sign-in made and no Station issued signs in once more.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    h.registry
        .store_access_token(
            &AccessTokenRecord::try_new(
                "6c2e8d3c-2f31-4c54-9d57-7c8f1f2b9a02",
                digest_of("old-token"),
                "id-alice",
                now - 1.0,
                now + 3_600.0,
                None,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        renewal(&h, "old-token").await.status(),
        StatusCode::UNAUTHORIZED
    );
    // A Station's token renews, and a browser's renewal of it gets no page cookie.
    let issued: Value = reqwest::Client::new()
        .post(format!("{}/v1/station/access-tokens", h.base))
        .bearer_auth(STATION_TOKEN)
        .json(&json!({"person_id": "id-alice", "ttl_seconds": 3600}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let station = renewal(&h, issued["token"].as_str().unwrap()).await;
    assert_eq!(station.status(), StatusCode::OK);
    assert!(station.headers().get("set-cookie").is_none());
    // The same renewal of a password sign-in does mint one.
    let (_, body) = password(&h, Some("10.1.2.3"), "alice", "pw", true).await;
    let signed_in = renewal(&h, body["token"].as_str().unwrap()).await;
    assert_eq!(signed_in.status(), StatusCode::OK);
    assert!(signed_in.headers().get("set-cookie").is_some());
}

#[tokio::test]
async fn a_policy_naming_a_provider_nobody_has_is_refused() {
    let h = editable().await;
    let (_, view) = call(&h, &h.token, Method::GET, None, None).await;
    let revision = view["revision"].as_str().unwrap().to_owned();
    let (status, body) = call(
        &h,
        &h.token,
        Method::PUT,
        Some(&revision),
        Some(json!({"config": {"rule": [{"providers": ["nobody"], "allow": [["password"]]}]}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("nobody"), "{body}");
    assert_eq!(stored(&h), None);
    let (status, _) = call(
        &h,
        &h.token,
        Method::PUT,
        Some(&revision),
        Some(json!({"config": {"rule": [{"providers": ["local"], "allow": [["password"]]}]}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_policy_that_offers_nothing_says_when_only_a_database_edit_restores_sign_in() {
    let h = editable().await;
    let (_, view) = call(&h, &h.token, Method::GET, None, None).await;
    let revision = view["revision"].as_str().unwrap().to_owned();
    let (status, changed) = call(
        &h,
        &h.token,
        Method::PUT,
        Some(&revision),
        Some(json!({"config": {"rule": []}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        changed["note"]
            .as_str()
            .unwrap_or_default()
            .contains("database"),
        "{changed}"
    );
    let (_, view) = call(&h, &h.token, Method::GET, None, None).await;
    assert!(
        view["note"]
            .as_str()
            .unwrap_or_default()
            .contains("database")
    );
}

#[tokio::test]
async fn a_refresh_takes_the_stored_record_and_never_an_older_one() {
    let h = editable().await;
    let (_, view) = call(&h, &h.token, Method::GET, None, None).await;
    let revision = view["revision"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &h,
        &h.token,
        Method::PUT,
        Some(&revision),
        Some(json!({"config": office_rule()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let refresh = |live: Arc<LiveSignIn>| async move {
        tokio::task::spawn_blocking(move || live.refresh())
            .await
            .unwrap()
    };
    assert_eq!(
        refresh(h.live.clone()).await,
        Ok(false),
        "what the route stored"
    );
    // A record changed behind the gateway's back is taken on the next refresh.
    let mut record = h
        .provider
        .definitions(ConfigRecord::Unex, &ConfigScope::Global)
        .unwrap()
        .records
        .remove(0);
    let before = record.1.clone();
    record.1["sign_in_policy"] = json!({"config": {"rule": []}});
    h.provider
        .put_if(
            ConfigRecord::Unex,
            &ConfigScope::Global,
            &record.0,
            Some(&before),
            &record.1,
        )
        .unwrap();
    assert_eq!(refresh(h.live.clone()).await, Ok(true));
    assert!(h.live.offers_nothing());
}
