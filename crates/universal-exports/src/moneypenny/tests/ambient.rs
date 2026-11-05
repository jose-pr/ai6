//! A page opened by itself with what a front door or the gateway's own TLS carries, through the
//! real coqui-access providers a deployment configures as its page authenticator:
//! `coqui_access_proxy` behind a front door (a user header or a verified certificate header,
//! trusted from the proxy's address only) and `coqui_access_mtls` on the gateway's own TLS.
//! Neither opens anything under `/v1`.

use std::path::PathBuf;
use std::sync::OnceLock;

use ai6::guatiao::{ToValue, Value as Provided};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};

use super::*;

/// The access plugins Cargo built beside the test binary, loaded from a copy of their own: a
/// library a test loads stays mapped, and locked on Windows, for the process.
fn plugins() -> Arc<ai6::station::plugins::Plugins> {
    static PLUGINS: OnceLock<Arc<ai6::station::plugins::Plugins>> = OnceLock::new();
    PLUGINS
        .get_or_init(|| {
            let deps = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf();
            let scratch = deps
                .join("plugin-scratch")
                .join(format!("ue-access-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&scratch).unwrap();
            for wanted in ["coqui_access_proxy_plugin", "coqui_access_mtls_plugin"] {
                let library: PathBuf = std::fs::read_dir(&deps)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == std::env::consts::DLL_EXTENSION)
                            && path
                                .file_name()
                                .is_some_and(|name| name.to_string_lossy().contains(wanted))
                    })
                    .max_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
                    .unwrap_or_else(|| panic!("{wanted} is built beside the test binary"));
                std::fs::copy(&library, scratch.join(library.file_name().unwrap())).unwrap();
            }
            Arc::new(ai6::station::plugins::Plugins::scan(&[scratch]))
        })
        .clone()
}

/// A JSON value as a provider's configuration.
fn provided(json: &Value) -> Provided {
    match json {
        Value::String(text) => text.clone().to_value().unwrap(),
        Value::Bool(flag) => flag.to_value().unwrap(),
        Value::Number(number) => number.as_u64().unwrap().to_value().unwrap(),
        Value::Array(items) => {
            std::collections::VecDeque::from(items.iter().map(provided).collect::<Vec<_>>())
                .to_value()
                .unwrap()
        }
        Value::Object(entries) => entries
            .iter()
            .map(|(key, value)| (key.clone(), provided(value)))
            .collect::<BTreeMap<String, Provided>>()
            .to_value()
            .unwrap(),
        Value::Null => ().to_value().unwrap(),
    }
}

fn authenticator(id: &str, settings: &Value) -> Arc<dyn Authenticator> {
    Arc::from(
        plugins()
            .authenticator(id, Some(&provided(settings)))
            .unwrap(),
    )
}

/// A gateway on the API's origin whose page authenticator is `authenticator`.
async fn served_ambient(authenticator: Arc<dyn Authenticator>) -> Served {
    served_setup(Setup {
        config: AssistantConfig::default(),
        roster: None,
        own_origin: false,
        page_authenticator: Some(authenticator),
        public_origin: None,
    })
    .await
}

/// The operator's report, published.
async fn report(served: &Served) -> String {
    let (_app, job) = accepted(served, &served.operator, "A report.").await;
    published_id(
        &publish(
            served,
            &job.mcp_token,
            json!({"kind": "report", "title": "Report", "content": "<p>r</p>", "format": "html"}),
        )
        .await,
    )
}

/// A certificate authority for the people's certificates.
struct Authority {
    issuer: Issuer<'static, KeyPair>,
    pem: String,
}

impl Authority {
    fn new() -> Self {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .distinguished_name
            .push(DnType::CommonName, "Test People CA");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let pem = params.self_signed(&key).unwrap().pem();
        Self {
            issuer: Issuer::new(params, key),
            pem,
        }
    }

    /// A certificate naming `name` as its subject's common name, and its key, as PEM.
    fn person(&self, name: &str) -> (String, String) {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(DnType::CommonName, name);
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
        (
            params.signed_by(&key, &self.issuer).unwrap().pem(),
            key.serialize_pem(),
        )
    }
}

/// `text` percent-encoded as nginx forwards a certificate (`$ssl_client_escaped_cert`).
fn escaped(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

async fn status(url: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut request = reqwest::Client::new().get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.send().await.unwrap().status()
}

#[tokio::test]
async fn behind_a_front_door_its_user_or_certificate_header_opens_a_page_and_never_v1() {
    let served = served_ambient(authenticator(
        "coqui_access_proxy",
        &json!({
            "trusted": ["127.0.0.1", "::1"],
            "user_header": "x-remote-user",
            "certificate_header": "x-client-cert",
            "certificate_encoding": "pem-url",
            "account": [{"field": "subject.cn", "by": "id"}],
        }),
    ))
    .await;
    let id = report(&served).await;
    let frame = format!("{}/pages/p/{id}/", served.base);
    assert_eq!(
        status(&frame, &[("x-remote-user", "operator-007")]).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&frame, &[("x-remote-user", "viewer-007")]).await,
        StatusCode::NOT_FOUND,
        "the page is not the viewer's"
    );
    assert_eq!(status(&frame, &[]).await, StatusCode::NOT_FOUND);
    let (certificate, _) = Authority::new().person("operator-007");
    let forwarded = escaped(&certificate);
    assert_eq!(
        status(&frame, &[("x-client-cert", &forwarded)]).await,
        StatusCode::OK
    );
    // /v1 takes neither header as anyone.
    for (name, value) in [
        ("x-remote-user", "operator-007"),
        ("x-client-cert", forwarded.as_str()),
    ] {
        assert_eq!(
            status(
                &format!("{}/v1/user/profile", served.base),
                &[(name, value)]
            )
            .await,
            StatusCode::UNAUTHORIZED,
            "{name}"
        );
    }
}

#[tokio::test]
async fn on_the_gateway_s_own_tls_a_client_certificate_opens_a_page_and_never_v1() {
    let authority = Authority::new();
    let ca_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ca_file.path(), &authority.pem).unwrap();
    let served = served_ambient(authenticator(
        "coqui_access_mtls",
        &json!({
            "trust": "ca",
            "ca_file": ca_file.path().to_str().unwrap(),
            "account": [{"field": "subject.cn", "by": "id"}],
        }),
    ))
    .await;
    let id = report(&served).await;
    // The same gateway on TLS of its own.
    let server_key = KeyPair::generate().unwrap();
    let server = CertificateParams::new(vec!["localhost".to_owned()])
        .unwrap()
        .self_signed(&server_key)
        .unwrap();
    let config = Arc::new(
        crate::tls_server_config(
            server.pem().as_bytes(),
            server_key.serialize_pem().as_bytes(),
        )
        .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let gateway = served.gateway.clone();
    let tls = tokio::spawn(async move {
        let _ = gateway
            .serve_tls(listener, config, std::future::pending())
            .await;
    });
    let base = format!("https://localhost:{port}");
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
    let (certificate, key) = authority.person("operator-007");
    let operator = client(Some(
        reqwest::Identity::from_pem(format!("{certificate}{key}").as_bytes()).unwrap(),
    ));
    let frame = format!("{base}/pages/p/{id}/");
    assert_eq!(
        operator.get(&frame).send().await.unwrap().status(),
        StatusCode::OK
    );
    let (certificate, key) = authority.person("viewer-007");
    let viewer = client(Some(
        reqwest::Identity::from_pem(format!("{certificate}{key}").as_bytes()).unwrap(),
    ));
    assert_eq!(
        viewer.get(&frame).send().await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "the page is not the viewer's"
    );
    // A certificate of another authority names nobody.
    let (certificate, key) = Authority::new().person("operator-007");
    let stranger = client(Some(
        reqwest::Identity::from_pem(format!("{certificate}{key}").as_bytes()).unwrap(),
    ));
    assert_eq!(
        stranger.get(&frame).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client(None).get(&frame).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    // /v1 takes the certificate as nobody, and the bearer as before.
    let profile = format!("{base}/v1/user/profile");
    assert_eq!(
        operator.get(&profile).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        operator
            .get(&profile)
            .bearer_auth(&served.operator)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    tls.abort();
}
