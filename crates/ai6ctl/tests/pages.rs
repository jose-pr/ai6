//! `ai6ctl pages` against a stand-in for Universal Exports' page routes: what it sends and
//! prints, and how it reads a refusal.

use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use axum::{Json, response::IntoResponse};
use serde_json::{Value, json};

/// Every request the stand-in saw: method, path, bearer and body.
type Seen = Arc<Mutex<Vec<(String, String, String, Value)>>>;

fn record(seen: &Seen, method: &str, path: String, headers: &HeaderMap, body: Value) {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    seen.lock()
        .unwrap()
        .push((method.to_owned(), path, bearer, body));
}

async fn start() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let app = Router::new()
        .route(
            "/v1/user/pages",
            get(|State(seen): State<Seen>, headers: HeaderMap| async move {
                record(&seen, "GET", "/v1/user/pages".into(), &headers, Value::Null);
                Json(json!([
                    {"id": "page-a", "kind": "dashboard", "title": "Load\tby station", "bytes": 2048,
                     "expires_at": 1_800_000_000.0},
                    {"id": "page-b", "kind": "file", "title": "Log", "bytes": 10,
                     "expires_at": null},
                    {"id": "page-c", "kind": "report", "title": "Theirs", "bytes": 1,
                     "expires_at": null, "shared_by": "q-007"},
                ]))
            }),
        )
        .route(
            "/v1/user/pages/{id}/open",
            post(
                |State(seen): State<Seen>,
                 headers: HeaderMap,
                 Path(id): Path<String>,
                 Json(body): Json<Value>| async move {
                    record(&seen, "POST", format!("/v1/user/pages/{id}/open"), &headers, body);
                    if id == "gone" {
                        return StatusCode::NOT_FOUND.into_response();
                    }
                    Json(json!({"url": format!("https://pages.example/v/token-for-{id}/"),
                                "expires_at": 1.0}))
                    .into_response()
                },
            ),
        )
        .route(
            "/v1/user/pages/{id}/remember",
            post(
                |State(seen): State<Seen>,
                 headers: HeaderMap,
                 Path(id): Path<String>,
                 Json(body): Json<Value>| async move {
                    record(&seen, "POST", format!("/v1/user/pages/{id}/remember"), &headers, body);
                    Json(json!({"id": id}))
                },
            ),
        )
        .route(
            "/v1/user/pages/{id}/shares",
            put(
                |State(seen): State<Seen>,
                 headers: HeaderMap,
                 Path(id): Path<String>,
                 Json(body): Json<Value>| async move {
                    record(&seen, "PUT", format!("/v1/user/pages/{id}/shares"), &headers, body.clone());
                    if body["subjects"] == json!(["operation:H"]) {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({"error": "you do not hold mission.read in Operation H"})),
                        )
                            .into_response();
                    }
                    Json(json!({"id": id, "shares": body["subjects"]})).into_response()
                },
            ),
        )
        .route(
            "/v1/user/pages/{id}/public",
            put(
                |State(seen): State<Seen>,
                 headers: HeaderMap,
                 Path(id): Path<String>,
                 Json(body): Json<Value>| async move {
                    record(&seen, "PUT", format!("/v1/user/pages/{id}/public"), &headers, body.clone());
                    if body["on"] == true {
                        Json(json!({"id": id, "public": true,
                                    "public_url": format!("https://pages.example/s/public-{id}/")}))
                    } else {
                        Json(json!({"id": id, "public": false}))
                    }
                },
            ),
        )
        .route(
            "/v1/user/pages/{id}",
            delete(
                |State(seen): State<Seen>, headers: HeaderMap, Path(id): Path<String>| async move {
                    record(&seen, "DELETE", format!("/v1/user/pages/{id}"), &headers, Value::Null);
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (server, seen)
}

async fn ai6ctl(server: &str, token: Option<&str>, args: &[&str]) -> std::process::Output {
    let (server, token) = (server.to_owned(), token.map(str::to_owned));
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ai6ctl"));
        command
            .arg("--server")
            .arg(server)
            .args(args)
            .env_remove("UNEX_ACCESS_TOKEN");
        if let Some(token) = token {
            command.env("UNEX_ACCESS_TOKEN", token);
        }
        command.output().unwrap()
    })
    .await
    .unwrap()
}

fn out(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn err(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[tokio::test]
async fn pages_list_prints_one_line_each_with_the_expiry_or_remembered() {
    let (server, seen) = start().await;
    let output = ai6ctl(&server, Some("tok"), &["pages", "list"]).await;
    assert!(output.status.success(), "{}", err(&output));
    let lines: Vec<String> = out(&output).lines().map(str::to_owned).collect();
    // A control character in a title is shown as one, never as a column break.
    assert_eq!(
        lines[0],
        "page-a\tdashboard\t2048\t1800000000\tLoad\u{fffd}by station\t-"
    );
    assert_eq!(lines[1], "page-b\tfile\t10\tremembered\tLog\t-");
    // A page shared with you names whose it is.
    assert_eq!(lines[2], "page-c\treport\t1\tremembered\tTheirs\tq-007");
    assert_eq!(seen.lock().unwrap()[0].2, "Bearer tok");
}

#[tokio::test]
async fn pages_share_puts_the_subjects_and_prints_the_gateways_reason_for_a_refusal() {
    let (server, seen) = start().await;
    let shared = ai6ctl(
        &server,
        Some("tok"),
        &["pages", "share", "page-a", "group:ops", "operation:G"],
    )
    .await;
    assert!(shared.status.success(), "{}", err(&shared));
    assert_eq!(out(&shared), "page-a: shared with group:ops operation:G\n");
    let nobody = ai6ctl(&server, Some("tok"), &["pages", "share", "page-a"]).await;
    assert_eq!(out(&nobody), "page-a: shared with nobody\n");
    let refused = ai6ctl(
        &server,
        Some("tok"),
        &["pages", "share", "page-a", "operation:H"],
    )
    .await;
    assert!(!refused.status.success());
    assert!(
        err(&refused).contains("you do not hold mission.read in Operation H"),
        "{}",
        err(&refused)
    );
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.iter()
            .map(|(method, path, _, body)| (method.as_str(), path.as_str(), body.clone()))
            .collect::<Vec<_>>(),
        [
            (
                "PUT",
                "/v1/user/pages/page-a/shares",
                json!({"subjects": ["group:ops", "operation:G"]})
            ),
            (
                "PUT",
                "/v1/user/pages/page-a/shares",
                json!({"subjects": []})
            ),
            (
                "PUT",
                "/v1/user/pages/page-a/shares",
                json!({"subjects": ["operation:H"]})
            ),
        ]
    );
}

#[tokio::test]
async fn pages_open_prints_the_link_and_asks_for_the_network_when_told() {
    let (server, seen) = start().await;
    let plainly = ai6ctl(&server, Some("tok"), &["pages", "open", "page-a"]).await;
    assert_eq!(out(&plainly), "https://pages.example/v/token-for-page-a/\n");
    let off = ai6ctl(
        &server,
        Some("tok"),
        &["pages", "open", "page-a", "--network", "none"],
    )
    .await;
    assert!(off.status.success(), "{}", err(&off));
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].3, json!({"network": null}));
        assert_eq!(seen[1].3, json!({"network": "none"}));
    }
    let bad = ai6ctl(
        &server,
        Some("tok"),
        &["pages", "open", "page-a", "--network", "wide"],
    )
    .await;
    assert!(!bad.status.success());
}

#[tokio::test]
async fn pages_remember_forget_and_rm_send_what_they_say() {
    let (server, seen) = start().await;
    let remember = ai6ctl(&server, Some("tok"), &["pages", "remember", "page-a"]).await;
    assert_eq!(out(&remember), "page-a: remembered\n");
    let forget = ai6ctl(&server, Some("tok"), &["pages", "forget", "page-a"]).await;
    assert_eq!(out(&forget), "page-a: will expire\n");
    let rm = ai6ctl(&server, Some("tok"), &["pages", "rm", "page-a"]).await;
    assert_eq!(out(&rm), "page-a: deleted\n");
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.iter()
            .map(|(method, path, _, body)| (method.as_str(), path.as_str(), body.clone()))
            .collect::<Vec<_>>(),
        [
            (
                "POST",
                "/v1/user/pages/page-a/remember",
                json!({"remember": true})
            ),
            (
                "POST",
                "/v1/user/pages/page-a/remember",
                json!({"remember": false})
            ),
            ("DELETE", "/v1/user/pages/page-a", Value::Null),
        ]
    );
}

#[tokio::test]
async fn pages_encode_the_id_as_one_segment_and_read_a_refusal() {
    let (server, seen) = start().await;
    let output = ai6ctl(&server, Some("tok"), &["pages", "open", "gone"]).await;
    assert!(!output.status.success());
    assert!(
        err(&output).contains("no such page of yours"),
        "{}",
        err(&output)
    );
    let odd = ai6ctl(&server, Some("tok"), &["pages", "rm", "a/b ?x"]).await;
    assert!(odd.status.success(), "{}", err(&odd));
    let paths: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|seen| seen.1.clone())
        .collect();
    assert_eq!(
        paths.last().unwrap(),
        "/v1/user/pages/a/b ?x",
        "one path segment, decoded by the route"
    );
    let no_token = ai6ctl(&server, None, &["pages", "list"]).await;
    assert!(!no_token.status.success());
    assert!(
        err(&no_token).contains("UNEX_ACCESS_TOKEN"),
        "{}",
        err(&no_token)
    );
}

#[tokio::test]
async fn pages_public_turns_the_public_link_on_and_off_and_prints_it() {
    let (server, seen) = start().await;
    let on = ai6ctl(&server, Some("tok"), &["pages", "public", "page-a", "on"]).await;
    assert!(on.status.success(), "{}", err(&on));
    assert_eq!(
        out(&on),
        "page-a: public at https://pages.example/s/public-page-a/\n"
    );
    let off = ai6ctl(&server, Some("tok"), &["pages", "public", "page-a", "off"]).await;
    assert_eq!(out(&off), "page-a: no public link\n");
    let bad = ai6ctl(&server, Some("tok"), &["pages", "public", "page-a", "yes"]).await;
    assert!(!bad.status.success());
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.iter()
            .map(|(method, path, _, body)| (method.as_str(), path.as_str(), body.clone()))
            .collect::<Vec<_>>(),
        [
            ("PUT", "/v1/user/pages/page-a/public", json!({"on": true})),
            ("PUT", "/v1/user/pages/page-a/public", json!({"on": false})),
        ]
    );
}
