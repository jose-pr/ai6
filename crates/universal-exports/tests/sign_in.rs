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
    /// Codes the second-factor stand-in was asked to check.
    codes: AtomicUsize,
}

/// The second-factor stand-in of the shared vectors: `123456` for the person already named, and
/// the recovery code `RECOVER-1` once.
struct Second {
    counts: Arc<Counts>,
    recovered: std::sync::atomic::AtomicBool,
}

impl Authenticator for Second {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        self.counts.codes.fetch_add(1, Ordering::SeqCst);
        let named = matches!(attempt.name.as_deref(), Some("id-alice" | "id-bob"));
        let right = match (attempt.method.as_deref(), attempt.secret.as_deref()) {
            (Some("totp"), Some("123456")) => true,
            (Some("recovery_code"), Some("RECOVER-1")) => {
                !self.recovered.swap(true, Ordering::SeqCst)
            }
            _ => false,
        };
        Ok(if named && right {
            Verdict::Valid(Lookup {
                by: "id".to_owned(),
                value: attempt.name.unwrap_or_default(),
                ..Default::default()
            })
        } else {
            Verdict::Invalid
        })
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            takes: Some(ai6::directorates::m::access::takes::OTP.to_owned()),
            ..Description::default()
        })
    }
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

/// The tries the vectors assume: a person's wrong codes end at the third, and are forgotten
/// after a quiet period that the test shortens to a second and a half.
fn flow_limits() -> LoginLimits {
    LoginLimits {
        per_name: std::num::NonZeroU32::new(3).unwrap(),
        name_window: Duration::from_millis(1500),
        ..limits(50)
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

/// A second-factor provider and where its factors are set up.
type SecondFactor = (
    Arc<dyn Authenticator>,
    Option<Arc<dyn ai6::Accounts>>,
    Option<Arc<universal_exports::FactorSealer>>,
    Option<Arc<dyn Authenticator>>,
);

/// A gateway over the stand-in providers; `ldap` is asked first when `with_ldap`.
fn gateway_over(
    counts: &Arc<Counts>,
    with_ldap: bool,
    registry: &FileRegistry,
    login: LoginLimits,
    policy: Option<SignInRules>,
    second: Option<SecondFactor>,
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
    let (second, factors, sealer, page) = second.unwrap_or_else(|| {
        (
            Arc::new(Second {
                counts: counts.clone(),
                recovered: Default::default(),
            }),
            None,
            None,
            None,
        )
    });
    if let Some(page) = page {
        access = access
            .with_page_authenticator(page)
            .with_page_authenticator_id("front");
    }
    access = access.with_second_factor("totp", second, factors);
    if let Some(sealer) = sealer {
        access = access.with_factor_sealer(sealer);
    }
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
    .with_login_limits(login);
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
    serve_with(policy, with_ldap, limits(per_address)).await
}

async fn serve_with(policy: Option<Value>, with_ldap: bool, login: LoginLimits) -> Served {
    let dir = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(dir.path());
    serve_on(registry, dir, policy, with_ldap, login, None).await
}

/// A gateway over `registry`, which another may share.
async fn serve_on(
    registry: FileRegistry,
    dir: tempfile::TempDir,
    policy: Option<Value>,
    with_ldap: bool,
    login: LoginLimits,
    second: Option<SecondFactor>,
) -> Served {
    let counts = Arc::new(Counts::default());
    let policy = policy.map(|policy| rules(&policy).unwrap());
    let gateway = gateway_over(&counts, with_ldap, &registry, login, policy, second);
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

/// A request to `POST /v1/auth/login` as a browser through the proxy.
async fn login_as_browser(
    h: &Served,
    address: Option<&str>,
    extra: &[(&str, &str)],
    body: &Value,
) -> (StatusCode, Value) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/v1/auth/login", h.base))
        .json(body);
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let response = browser(request, address).send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or_default())
}

/// What the offer says while a sign-in waits.
async fn methods_pending(h: &Served, address: Option<&str>, pending: &str) -> Value {
    browser(
        reqwest::Client::new()
            .get(format!("{}/v1/auth/methods", h.base))
            .header("x-ai6-pending", pending),
        address,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap()
}

#[tokio::test]
async fn every_shared_flow_goes_the_same_through_the_routes() {
    let document: Value = serde_json::from_str(FLOWS).unwrap();
    let flows = document["flows"].as_array().unwrap();
    assert!(flows.len() >= 18, "{} flows", flows.len());
    for flow in flows {
        let name = flow["name"].as_str().unwrap();
        // The vectors say `certificate`; this gateway meets a certificate as the identity a
        // trusted proxy asserts.
        let config = proxied(&flow["config"]);
        let ldap = flow["config"].to_string().contains("providers");
        let h = serve_with(Some(config), ldap, flow_limits()).await;
        let mut token: Option<String> = None;
        let mut pending: Option<String> = None;
        for (index, step) in flow["steps"].as_array().unwrap().iter().enumerate() {
            let at = format!("{name}, step {index}");
            let address = step["address"].as_str();
            // The vectors' quiet periods of up to 900 seconds are 1.5 seconds here.
            if let Some(after) = step["after_seconds"].as_f64() {
                tokio::time::sleep(Duration::from_secs_f64(after * 1.5 / 900.0)).await;
            }
            let calls = |h: &Served| {
                h.counts.passwords.load(Ordering::SeqCst)
                    + h.counts.proxied.load(Ordering::SeqCst)
                    + h.counts.codes.load(Ordering::SeqCst)
            };
            let before = calls(&h);
            h.counts.asked.lock().unwrap().clear();
            let expect = &step["expect"];
            let mut expected_calls = expect["authenticator_calls"].as_i64().unwrap();
            let result = match step["action"].as_str().unwrap() {
                "offer" => match &pending {
                    Some(held) => {
                        let offered = methods_pending(&h, address, held).await;
                        assert_eq!(offered["step"], "next", "{at}");
                        assert_eq!(
                            strings(Some(&offered["methods"])),
                            strings(Some(&proxied(&expect["methods"]))),
                            "{at}"
                        );
                        "methods"
                    }
                    None => {
                        let offered = methods(&h, address, true).await;
                        assert_eq!(offered, strings(Some(&proxied(&expect["methods"]))), "{at}");
                        "methods"
                    }
                },
                "present" => {
                    let who = step["name"].as_str().unwrap_or_default();
                    let method = step["method"].as_str().unwrap();
                    let mut body = match method {
                        "password" => {
                            json!({"name": who, "secret": step["secret"].as_str().unwrap()})
                        }
                        "totp" | "recovery_code" => json!({
                            "method": method,
                            "name": who,
                            "secret": step["secret"].as_str().unwrap(),
                        }),
                        _ => json!({}),
                    };
                    let mut extra: Vec<(&str, &str)> = Vec::new();
                    if method == "certificate" {
                        extra.push(("x-forwarded-user", who));
                        extra.push(("x-ai6-proxy-secret", PROXY_SECRET));
                    }
                    if let Some(held) = &pending {
                        // A sign-in that waits is continued, whatever is presented to it.
                        body["pending"] = json!(held);
                        body["method"] = json!(if method == "certificate" {
                            "proxy"
                        } else {
                            method
                        });
                        if method == "password" {
                            body["method"] = json!("password");
                        }
                    }
                    // The gateway only ever checks a code for the person the sign-in named, so
                    // a code naming another person is refused before an authenticator is asked.
                    if ["totp", "recovery_code"].contains(&method)
                        && pending.is_some()
                        && who != "id-alice"
                    {
                        expected_calls = 0;
                    }
                    let (status, answer) = login_as_browser(&h, address, &extra, &body).await;
                    match status {
                        StatusCode::CREATED => {
                            token = answer["token"].as_str().map(str::to_owned);
                            pending = None;
                            "signed_in"
                        }
                        StatusCode::ACCEPTED => {
                            pending = answer["pending"].as_str().map(str::to_owned);
                            assert_eq!(
                                strings(Some(&answer["methods"])),
                                strings(Some(&proxied(&expect["methods"]))),
                                "{at}"
                            );
                            assert!(answer["expires_in"].as_f64().unwrap() > 0.0, "{at}");
                            "next"
                        }
                        StatusCode::FORBIDDEN => "refused_unasked",
                        StatusCode::UNAUTHORIZED | StatusCode::TOO_MANY_REQUESTS => "failed",
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
            assert_eq!(calls(&h) as i64 - before as i64, expected_calls, "{at}");
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

/// The bytes of an unpadded RFC 4648 base32 text.
fn base32(text: &str) -> Vec<u8> {
    let mut bits = 0u32;
    let mut held = 0u32;
    let mut bytes = Vec::new();
    for character in text.bytes() {
        let value = match character {
            b'A'..=b'Z' => character - b'A',
            b'2'..=b'7' => character - b'2' + 26,
            _ => continue,
        };
        bits = (bits << 5) | u32::from(value);
        held += 5;
        if held >= 8 {
            held -= 8;
            bytes.push((bits >> held) as u8);
            bits &= (1 << held) - 1;
        }
    }
    bytes
}

/// The `secret` of a provisioning link.
fn secret_of(link: &str) -> String {
    link.split(['?', '&'])
        .find_map(|part| part.strip_prefix("secret="))
        .unwrap()
        .to_owned()
}

/// The code an app shows at `seconds`, written from RFC 4226 and independent of the provider.
fn code_at(link: &str, seconds: i64) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    let key = base32(&secret_of(link));
    let mut mac = <Hmac<sha1::Sha1> as KeyInit>::new_from_slice(&key).unwrap();
    mac.update(&((seconds / 30) as u64).to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let number = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:06}", number % 1_000_000)
}

/// The one-time code provider over `registry`, reading `clock` as the time.
fn real_second(registry: &FileRegistry, clock: Arc<std::sync::atomic::AtomicI64>) -> SecondFactor {
    second_sealed(registry, clock, Some("a key every gateway shares"))
}

/// As [`real_second`], sealing with `key`.
fn second_sealed(
    registry: &FileRegistry,
    clock: Arc<std::sync::atomic::AtomicI64>,
    key: Option<&str>,
) -> SecondFactor {
    let shared: Arc<dyn ai6::RegistryBackend> = Arc::new(registry.clone());
    let sealer = Arc::new(universal_exports::FactorSealer::new(key).unwrap());
    let totp = Arc::new(
        universal_exports::totp_builder(&shared, None, &sealer)
            .unwrap()
            .issuer("AI6 test")
            .clock(move || clock.load(Ordering::SeqCst))
            .build()
            .unwrap(),
    );
    (totp.clone(), Some(totp), Some(sealer), None)
}

async fn send(
    h: &Served,
    method: Method,
    path: &str,
    address: Option<&str>,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = reqwest::Client::new().request(method, format!("{}{path}", h.base));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = browser(request, address).send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or_default())
}

fn audited(h: &Served, action: &str) -> Vec<ai6::AuditRecord> {
    h.registry
        .list_audit(200)
        .unwrap()
        .into_iter()
        .filter(|record| record.action == action)
        .collect()
}

#[tokio::test]
async fn a_sign_in_sets_up_its_second_factor_where_a_rule_names_enrol_first_and_codes_are_good_once()
 {
    let clock = Arc::new(std::sync::atomic::AtomicI64::new(1_700_000_010));
    let dir = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(dir.path());
    let policy = json!({"rule": [
        {"purposes": ["enrol_first"], "networks": ["10.0.0.0/8"], "allow": [["password"]]},
        {"allow": [["password", "totp"], ["password", "recovery_code"]]}
    ]});
    let second = real_second(&registry, clock.clone());
    let h = serve_on(registry, dir, Some(policy), false, limits(50), Some(second)).await;
    let (status, body) = password(&h, Some("10.1.1.1"), "alice", "pw", true).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["methods"], json!(["totp", "recovery_code"]));
    let pending = body["pending"].as_str().unwrap().to_owned();
    // Only where a rule names `enrol_first`: not from outside the office.
    let (status, _) = send(
        &h,
        Method::POST,
        "/v1/auth/enrol",
        Some("203.0.113.9"),
        None,
        Some(json!({"pending": pending})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(audited(&h, "account.factor.enrol").is_empty());
    let (status, begun) = send(
        &h,
        Method::POST,
        "/v1/auth/enrol",
        Some("10.1.1.1"),
        None,
        Some(json!({"pending": pending})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let uri = begun["uri"].as_str().unwrap().to_owned();
    assert!(uri.starts_with("otpauth://totp/"), "{uri}");
    let codes: Vec<String> = strings(Some(&begun["recovery_codes"]));
    assert_eq!(codes.len(), 10);
    let confirm = |code: String| {
        send(
            &h,
            Method::POST,
            "/v1/auth/enrol/confirm",
            Some("10.1.1.1"),
            None,
            Some(json!({"pending": pending, "code": code})),
        )
    };
    let (status, _) = confirm("000000".to_owned()).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a wrong first code leaves the enrolment"
    );
    let now = clock.load(Ordering::SeqCst);
    let (status, body) = confirm(code_at(&uri, now)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The confirming code is spent; the next step's code signs the waiting sign-in in.
    clock.store(now + 30, Ordering::SeqCst);
    let (status, body) = login_as_browser(
        &h,
        Some("10.1.1.1"),
        &[],
        &json!({"pending": pending, "method": "totp", "secret": code_at(&uri, now + 30)}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // A recovery code signs in once.
    for (round, expected) in [(0, StatusCode::CREATED), (1, StatusCode::UNAUTHORIZED)] {
        let (status, body) = password(&h, Some("10.1.1.1"), "alice", "pw", true).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        let (status, _) = login_as_browser(
            &h,
            Some("10.1.1.1"),
            &[],
            &json!({"pending": body["pending"], "method": "recovery_code", "secret": codes[0]}),
        )
        .await;
        assert_eq!(status, expected, "round {round}");
    }
    // Enrolment is audited, and nothing that proves the factor is in any audit record.
    assert!(!audited(&h, "account.factor.enrol").is_empty());
    assert!(!audited(&h, "account.factor.confirm").is_empty());
    let dump = serde_json::to_string(&h.registry.list_audit(200).unwrap()).unwrap();
    for secret in [
        secret_of(&uri),
        uri.clone(),
        codes[0].clone(),
        codes[1].clone(),
    ] {
        assert!(!dump.contains(&secret), "an audit record holds a secret");
    }
}

#[tokio::test]
async fn a_signed_in_person_enrols_under_an_enrol_rule_and_an_administrator_resets() {
    let clock = Arc::new(std::sync::atomic::AtomicI64::new(1_700_000_010));
    let dir = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(dir.path());
    let policy = json!({"rule": [
        {"purposes": ["enrol"], "allow": [["password"]]},
        {"allow": [["password"]]}
    ]});
    let second = real_second(&registry, clock.clone());
    let h = serve_on(registry, dir, Some(policy), false, limits(50), Some(second)).await;
    let (_, alice) = password(&h, None, "alice", "pw", true).await;
    let alice = alice["token"].as_str().unwrap().to_owned();
    let (_, bob) = password(&h, None, "bob", "pw", true).await;
    let bob = bob["token"].as_str().unwrap().to_owned();
    let (status, state) = send(&h, Method::GET, "/v1/user/factors", None, Some(&alice), None).await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(
        state,
        json!({"configured": true, "enrolled": false, "enrolling": false, "may_enrol": true})
    );
    let (status, begun) = send(
        &h,
        Method::POST,
        "/v1/user/factors/totp",
        None,
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let uri = begun["uri"].as_str().unwrap();
    let (_, state) = send(&h, Method::GET, "/v1/user/factors", None, Some(&alice), None).await;
    assert_eq!(state["enrolling"], true);
    // The Registry holds the waiting enrolment and, below, the factor with the secret sealed.
    let in_clear = |family: &str| {
        let documents = h.registry.factor_documents(family).unwrap();
        assert_eq!(documents.len(), 1, "{family}");
        assert!(documents[0].1.contains("aes256gcm1:"), "{family}");
        !documents[0].1.contains(&secret_of(uri))
    };
    assert!(in_clear("totp_enrolling"), "the enrolment keeps its secret in the clear");
    let (status, _) = send(
        &h,
        Method::POST,
        "/v1/user/factors/totp/confirm",
        None,
        Some(&alice),
        Some(json!({"code": code_at(uri, clock.load(Ordering::SeqCst))})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, state) = send(&h, Method::GET, "/v1/user/factors", None, Some(&alice), None).await;
    assert_eq!(
        (state["enrolled"].clone(), state["enrolling"].clone()),
        (json!(true), json!(false))
    );
    assert!(in_clear("totp"), "the factor keeps its secret in the clear");
    // Only someone holding account.manage resets.
    let reset = "/v1/user/access/factors/id-alice";
    let (status, _) = send(&h, Method::DELETE, reset, None, Some(&bob), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(audited(&h, "account.factor.reset").is_empty());
    let (status, _) = send(&h, Method::DELETE, reset, None, Some(&alice), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, state) = send(&h, Method::GET, "/v1/user/factors", None, Some(&alice), None).await;
    assert_eq!(state["enrolled"], false, "the person enrols again");
    let (status, _) = send(&h, Method::DELETE, reset, None, Some(&alice), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let resets = audited(&h, "account.factor.reset");
    assert_eq!(resets.len(), 2, "each reset asked for is recorded first");
    assert!(resets.iter().all(|record| record.target == "id-alice"));
    let dump = serde_json::to_string(&h.registry.list_audit(200).unwrap()).unwrap();
    for secret in [secret_of(uri), uri.to_owned()] {
        assert!(!dump.contains(&secret), "an audit record holds a secret");
    }
}

#[tokio::test]
async fn nobody_enrols_under_a_policy_with_no_enrol_rule() {
    let clock = Arc::new(std::sync::atomic::AtomicI64::new(1_700_000_010));
    let dir = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(dir.path());
    let second = real_second(&registry, clock);
    let h = serve_on(
        registry,
        dir,
        Some(json!({"rule": [{"allow": [["password"]]}]})),
        false,
        limits(50),
        Some(second),
    )
    .await;
    let (_, alice) = password(&h, None, "alice", "pw", true).await;
    let alice = alice["token"].as_str().unwrap();
    let (_, state) = send(&h, Method::GET, "/v1/user/factors", None, Some(alice), None).await;
    assert_eq!(state["may_enrol"], false);
    let (status, _) = send(
        &h,
        Method::POST,
        "/v1/user/factors/totp",
        None,
        Some(alice),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(audited(&h, "account.factor.enrol").is_empty());
}

fn two_step() -> Value {
    json!({"rule": [{"allow": [["password", "totp"]]}]})
}

#[tokio::test]
async fn a_pending_sign_in_is_shared_by_two_gateways_on_one_registry_and_taken_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let first = serve_on(
        FileRegistry::open(&path),
        dir,
        Some(two_step()),
        false,
        limits(50),
        None,
    )
    .await;
    let other = serve_on(
        FileRegistry::open(&path),
        tempfile::tempdir().unwrap(),
        Some(two_step()),
        false,
        limits(50),
        None,
    )
    .await;
    let (status, body) = password(&first, None, "alice", "pw", true).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let pending = body["pending"].as_str().unwrap().to_owned();
    // Only a digest of what the person holds is kept.
    let kept = first.registry.factor_documents("pending").unwrap();
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].0.contains(&pending) && !kept[0].1.contains(&pending));
    let code = json!({"pending": pending, "method": "totp", "secret": "123456"});
    let (status, body) = login_as_browser(&other, None, &[], &code).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(other.counts.codes.load(Ordering::SeqCst), 1);
    assert_eq!(first.counts.codes.load(Ordering::SeqCst), 0);
    // The sign-in was taken: neither gateway completes it again.
    for h in [&first, &other] {
        let (status, _) = login_as_browser(h, None, &[], &code).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    assert!(
        first
            .registry
            .factor_documents("pending")
            .unwrap()
            .is_empty()
    );
    // A sign-in completed by step-up stores every step it passed.
    let record = first
        .registry
        .login_by_id(body["token_id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let steps: Vec<(&str, Option<&str>)> = record
        .factors()
        .iter()
        .map(|factor| (factor.method(), factor.provider()))
        .collect();
    assert_eq!(steps, [("password", Some("local")), ("totp", Some("totp"))]);
}

#[tokio::test]
async fn a_pending_sign_in_ends_at_its_try_cap_and_at_its_time() {
    let capped = serve_with(
        Some(two_step()),
        false,
        LoginLimits {
            pending_tries: 2,
            ..limits(50)
        },
    )
    .await;
    let (_, body) = password(&capped, None, "alice", "pw", true).await;
    let pending = body["pending"].clone();
    for _ in 0..2 {
        let (status, _) = login_as_browser(
            &capped,
            None,
            &[],
            &json!({"pending": pending, "method": "totp", "secret": "000000"}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = login_as_browser(
        &capped,
        None,
        &[],
        &json!({"pending": pending, "method": "totp", "secret": "123456"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the right code, past the cap"
    );
    assert_eq!(
        capped.counts.codes.load(Ordering::SeqCst),
        2,
        "the third asked nobody"
    );
    assert!(
        capped
            .registry
            .factor_documents("pending")
            .unwrap()
            .is_empty()
    );

    let brief = serve_with(
        Some(two_step()),
        false,
        LoginLimits {
            pending_seconds: Duration::from_millis(800),
            ..limits(50)
        },
    )
    .await;
    let (_, body) = password(&brief, None, "alice", "pw", true).await;
    assert!(body["expires_in"].as_f64().unwrap() <= 0.8);
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let (status, _) = login_as_browser(
        &brief,
        None,
        &[],
        &json!({"pending": body["pending"], "method": "totp", "secret": "123456"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(brief.counts.codes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn enrolment_is_refused_with_no_factor_key_and_a_wrong_key_fails_the_sign_in() {
    let clock = Arc::new(std::sync::atomic::AtomicI64::new(1_700_000_010));
    let policy = json!({"rule": [
        {"purposes": ["enrol"], "allow": [["password"]]},
        {"allow": [["password"]]}
    ]});
    // No key: nothing is set up, and the refusal says why.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let registry = FileRegistry::open(&path);
    let keyless = second_sealed(&registry, clock.clone(), None);
    let h = serve_on(
        registry,
        dir,
        Some(policy),
        false,
        limits(50),
        Some(keyless),
    )
    .await;
    let (_, alice) = password(&h, None, "alice", "pw", true).await;
    let alice = alice["token"].as_str().unwrap().to_owned();
    let (status, refused) = send(
        &h,
        Method::POST,
        "/v1/user/factors/totp",
        None,
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("factor_key"));
    assert!(audited(&h, "account.factor.enrol").is_empty());
    assert!(h.registry.factor_documents("totp_enrolling").unwrap().is_empty());

    // A factor enrolled under one key, read by a gateway holding another: the step fails, and
    // is never skipped.
    let sealed = serve_on(
        FileRegistry::open(&path),
        tempfile::tempdir().unwrap(),
        Some(json!({"rule": [
            {"purposes": ["enrol"], "allow": [["password"]]},
            {"allow": [["password", "totp"]]}
        ]})),
        false,
        limits(50),
        Some(second_sealed(
            &FileRegistry::open(&path),
            clock.clone(),
            Some("the first key"),
        )),
    )
    .await;
    // Enrol through a policy that lets a password open a session, then read under another key.
    let open = serve_on(
        FileRegistry::open(&path),
        tempfile::tempdir().unwrap(),
        Some(json!({"rule": [{"purposes": ["enrol"], "allow": [["password"]]}, {"allow": [["password"]]}]})),
        false,
        limits(50),
        Some(second_sealed(
            &FileRegistry::open(&path),
            clock.clone(),
            Some("the first key"),
        )),
    )
    .await;
    let (_, token) = password(&open, None, "alice", "pw", true).await;
    let token = token["token"].as_str().unwrap().to_owned();
    let (status, begun) = send(
        &open,
        Method::POST,
        "/v1/user/factors/totp",
        None,
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{begun}");
    let uri = begun["uri"].as_str().unwrap().to_owned();
    let now = clock.load(Ordering::SeqCst);
    let (status, _) = send(
        &open,
        Method::POST,
        "/v1/user/factors/totp/confirm",
        None,
        Some(&token),
        Some(json!({"code": code_at(&uri, now)})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    clock.store(now + 30, Ordering::SeqCst);
    let (_, body) = password(&sealed, None, "alice", "pw", true).await;
    let right = json!({"pending": body["pending"], "method": "totp", "secret": code_at(&uri, now + 30)});
    let (status, _) = login_as_browser(&sealed, None, &[], &right).await;
    assert_eq!(status, StatusCode::CREATED, "its own key opens it");
    // Another gateway whose key does not open the stored factors does not start: no sign-in
    // can skip the step.
    let shared: Arc<dyn ai6::RegistryBackend> = Arc::new(FileRegistry::open(&path));
    let other = Arc::new(universal_exports::FactorSealer::new(Some("another key")).unwrap());
    let refused = universal_exports::totp_builder(&shared, None, &other)
        .unwrap()
        .build()
        .map(|_| ())
        .unwrap_err();
    assert!(refused.to_string().contains("factor_key"), "{refused}");
}

/// A page authenticator that names alice by any certificate the gateway's own TLS proved.
struct Certificates;

impl Authenticator for Certificates {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        Ok(
            if attempt.certificates.as_ref().is_some_and(|chain| !chain.is_empty()) {
                Verdict::Valid(Lookup {
                    by: "name".to_owned(),
                    value: "alice".to_owned(),
                    ..Default::default()
                })
            } else {
                Verdict::Unknown
            },
        )
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description::default())
    }
}

#[tokio::test]
async fn a_certificate_the_gateways_own_tls_proved_is_the_next_step_of_a_waiting_sign_in() {
    use rcgen::{CertificateParams, KeyPair};
    let counts = Arc::new(Counts::default());
    let dir = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(dir.path());
    let policy = rules(&json!({"rule": [{"allow": [["password", "certificate"]]}]})).unwrap();
    let second: SecondFactor = (
        Arc::new(Second {
            counts: counts.clone(),
            recovered: Default::default(),
        }),
        None,
        None,
        Some(Arc::new(Certificates)),
    );
    let gateway = gateway_over(
        &counts,
        false,
        &registry,
        limits(50),
        Some(policy),
        Some(second),
    );
    let server_key = KeyPair::generate().unwrap();
    let server = CertificateParams::new(vec!["localhost".to_owned()])
        .unwrap()
        .self_signed(&server_key)
        .unwrap();
    let config = Arc::new(
        universal_exports::tls_server_config(
            server.pem().as_bytes(),
            server_key.serialize_pem().as_bytes(),
        )
        .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let tls = tokio::spawn(async move {
        let _ = gateway
            .serve_tls(listener, config, std::future::pending())
            .await;
    });
    let client = |identity: Option<reqwest::Identity>| {
        let builder = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .resolve("localhost", ([127, 0, 0, 1], port).into());
        match identity {
            Some(identity) => builder.identity(identity),
            None => builder,
        }
        .build()
        .unwrap()
    };
    let key = KeyPair::generate().unwrap();
    let certificate = CertificateParams::new(vec!["person".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let held = client(Some(
        reqwest::Identity::from_pem(
            format!("{}{}", certificate.pem(), key.serialize_pem()).as_bytes(),
        )
        .unwrap(),
    ));
    let bare = client(None);
    let url = format!("https://localhost:{port}/v1/auth/login");
    let begin = async |who: &reqwest::Client| -> Value {
        who.post(&url)
            .json(&json!({"name": "alice", "secret": "pw"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    };
    let body = begin(&bare).await;
    assert_eq!(body["methods"], json!(["certificate"]), "{body}");
    // Without a certificate the TLS proved there is nothing to name the person.
    let refused = bare
        .post(&url)
        .json(&json!({"pending": body["pending"], "method": "certificate"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    let signed = held
        .post(&url)
        .json(&json!({"pending": body["pending"], "method": "certificate"}))
        .send()
        .await
        .unwrap();
    assert_eq!(signed.status(), StatusCode::CREATED);
    let answer: Value = signed.json().await.unwrap();
    let record = registry.login_by_id(answer["token_id"].as_str().unwrap()).unwrap().unwrap();
    let steps: Vec<(&str, Option<&str>)> = record
        .factors()
        .iter()
        .map(|factor| (factor.method(), factor.provider()))
        .collect();
    assert_eq!(
        steps,
        [("password", Some("local")), ("certificate", Some("front"))]
    );
    tls.abort();
}
