//! Pages in a real browser, as `AI6_TEST_BROWSER` names one (a Chromium: Chrome or Edge), on the
//! API's own origin, which the gateway serves on TLS of its own here: a sign-in in the browser
//! sets the page cookie, and the page then opens by itself in a new tab, runs as an opaque origin
//! and reads its own files and feeds; a reload sent from another site opens it; another browser
//! profile, given the same address, gets an unknown page's answer; a public link opens anywhere;
//! a fresh profile with no cookie opens a handed-off address, whose copy in another gets nothing.
//! Skipped, and ignored by default, without it.

use rcgen::{CertificateParams, KeyPair};

use super::*;

/// The gateway's own TLS for the browser: a certificate of its own making, and no client
/// certificate asked for, so the browser never offers one of the machine's.
fn browser_tls() -> Arc<rustls::ServerConfig> {
    let key = KeyPair::generate().unwrap();
    let certificate = CertificateParams::new(vec!["localhost".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .unwrap(),
    )
}

#[tokio::test]
#[ignore = "needs AI6_TEST_BROWSER to name a Chromium binary"]
async fn pages_browser_a_signed_in_browser_opens_a_page_by_itself_and_another_gets_nothing() {
    let Ok(binary) = std::env::var("AI6_TEST_BROWSER") else {
        eprintln!("skipped: AI6_TEST_BROWSER names no browser");
        return;
    };
    // Somewhere outside that answers anyone, and a page there that sends its visitor on: on
    // `127.0.0.1`, another site than the gateway's `localhost`.
    let outside = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let outside_port = outside.local_addr().unwrap().port();
    let outside_url = format!("http://127.0.0.1:{outside_port}");
    tokio::spawn(async move {
        let app = axum::Router::new()
            .route(
                "/",
                axum::routing::get(|| async {
                    ([("access-control-allow-origin", "*")], "from outside")
                }),
            )
            .route(
                "/opener",
                axum::routing::get(|| async {
                    axum::response::Html(
                        "<a id=go>go</a><script>const go = document.getElementById('go');\
                         go.href = new URLSearchParams(location.search).get('to'); go.click();\
                         </script>",
                    )
                }),
            );
        let _ = axum::serve(outside, app).await;
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let origin = format!("https://localhost:{port}");
    let served = served_setup(Setup {
        config: AssistantConfig::default(),
        roster: None,
        own_origin: false,
        page_authenticator: None,
        public_origin: Some(origin.clone()),
    })
    .await;
    let gateway = served.gateway.clone();
    tokio::spawn(async move {
        let _ = gateway
            .serve_tls(listener, browser_tls(), std::future::pending())
            .await;
    });
    let (_app, job) = accepted(&served, &served.operator, "Probe.").await;
    station_bundle(
        &served,
        &job.conversation_id,
        "scratch/probe",
        &[
            ("index.html", probe_page(&origin, &outside_url).as_bytes()),
            ("own.json", b"{\"own\":true}"),
            (
                "dot.png",
                b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\
                  \0\0\0\rIDATx\x9cc\xf8\xff\xff?\x00\x05\xfe\x02\xfe\xa7\x35\x81\x84\0\0\0\0IEND\xaeB`\x82",
            ),
        ],
    )
    .await;
    let id = published_id(
        &publish(
            &served,
            &job.mcp_token,
            json!({"kind": "dashboard", "title": "probe", "path": "scratch/probe",
                   "feeds": [{"name": "board", "tool": "board", "input": {}}]}),
        )
        .await,
    );
    let page = format!("{origin}/pages/p/{id}/");
    let mut mine = Browser::start(&binary).await;
    let mut other = Browser::start(&binary).await;

    // Signed in to the app: its token refresh, made from the gateway's origin, sets the cookie.
    let app = mine.open(&format!("{origin}/v1/auth/methods")).await;
    mine.top(&app).await;
    let renewed = mine
        .evaluate(
            &app,
            &format!(
                "fetch('/v1/auth/renew', {{method: 'POST', headers: {{authorization: \
                 'Bearer {}'}}}}).then((r) => r.status)",
                served.operator
            ),
        )
        .await;
    assert_eq!(renewed, json!(200), "the renewal");

    // The page opens by itself in a new tab, as a bookmark would: under the bar, as an opaque
    // origin, its own files and feeds read with its credentials.
    let tab = mine.open(&page).await;
    let top = mine.top(&tab).await;
    assert_eq!(
        (top["iframe"].clone(), top["bar"].clone()),
        (json!(true), json!(true)),
        "{top}"
    );
    let record = mine.probe(&tab).await;
    assert_probe(&record, true, "status 200 at,data", "opened by itself");
    // A fetch of its own files without its credentials sends no cookie, and gets nothing.
    assert_eq!(record["plain"], "blocked TypeError", "{record}");

    // A reload a site of its own sends there opens it too.
    let sent = format!(
        "{outside_url}/opener?to={}",
        page.replace(':', "%3A").replace('/', "%2F")
    );
    let tab = mine.open(&sent).await;
    assert_probe(
        &mine.probe(&tab).await,
        true,
        "status 200 at,data",
        "sent from another site",
    );
    mine.evaluate(&tab, "window.before = 1").await;
    mine.call(Some(&tab), "Page.reload", json!({}))
        .await
        .unwrap();
    for _ in 0..100 {
        if mine.evaluate(&tab, "window.before === undefined").await == json!(true) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_probe(
        &mine.probe(&tab).await,
        true,
        "status 200 at,data",
        "sent from another site, reloaded",
    );

    // With the network off the page reaches nothing outside.
    let tab = mine.open(&format!("{page}?network=none")).await;
    assert_probe(&mine.probe(&tab).await, false, "status 403", "none");

    // Another browser profile, given the copied address, gets nothing.
    let copied = other.open(&page).await;
    let top = other.top(&copied).await;
    assert_eq!(
        (top["iframe"].clone(), top["bar"].clone()),
        (json!(false), json!(false)),
        "{top}"
    );

    // A browser that never signed in opens the address the app hands it, which leaves the address
    // bar; a copy of that address in another browser gets nothing.
    let handed = reqwest::Client::new()
        .post(format!("{}/v1/user/pages/{id}/open", served.base))
        .bearer_auth(&served.operator)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["url"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(handed.contains("handoff="), "{handed}");
    let mut fresh = Browser::start(&binary).await;
    let tab = fresh.open(&handed).await;
    assert_probe(
        &fresh.probe(&tab).await,
        true,
        "status 200 at,data",
        "handed off",
    );
    let top = fresh.top(&tab).await;
    assert_eq!(top["url"], page.as_str(), "the token left the address bar");
    let copied = other.open(&handed).await;
    let top = other.top(&copied).await;
    assert_eq!(
        (top["iframe"].clone(), top["bar"].clone()),
        (json!(false), json!(false)),
        "{top}"
    );

    // A public link opens in any browser, with no sign-in, and serves no feed.
    let public = reqwest::Client::new()
        .put(format!("{}/v1/user/pages/{id}/public", served.base))
        .bearer_auth(&served.operator)
        .json(&json!({"on": true}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["public_url"]
        .as_str()
        .unwrap()
        .to_owned();
    let tab = other.open(&public).await;
    assert_eq!(other.top(&tab).await["bar"], true);
    assert_probe(
        &other.probe(&tab).await,
        true,
        "blocked TypeError",
        "public",
    );
}
