//! `ai6ctl keys` against a stand-in gateway that records what it was asked and answers as the
//! real routes do.

use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::any;
use serde_json::{Value, json};

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

/// Not a real key: the stand-in's made-up value.
const VALUE: &str = "ck_stand_in_value_0123456789";

async fn stand_in(
    State(seen): State<Seen>,
    method: Method,
    uri: Uri,
    body: String,
) -> impl IntoResponse {
    let json: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    seen.lock()
        .unwrap()
        .push((method.to_string(), uri.to_string(), json.clone()));
    match (method.as_str(), uri.path()) {
        ("POST", "/v1/user/keys") if json["scopes"] == json!(["roster.manage@config:roster"]) => (
            StatusCode::BAD_REQUEST,
            axum::Json(
                json!({"error": "you do not hold roster.manage@config:roster, so a key cannot"}),
            ),
        )
            .into_response(),
        ("POST", "/v1/user/keys") => (
            StatusCode::CREATED,
            axum::Json(json!({"key": {"id": "key-1", "name": json["name"]}, "secret": VALUE})),
        )
            .into_response(),
        ("GET", "/v1/user/keys") => axum::Json(json!([{
            "id": "key-1", "name": "deploy", "scopes": ["config.manage@config:plugin"],
            "created_at": 1000.0, "expires_at": null, "last_used_at": 2000.0,
            "owner_id": "alice-id", "owner_name": "alice"
        }]))
        .into_response(),
        ("DELETE", "/v1/user/keys/key-1") => axum::Json(json!({"revoked": true})).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve() -> (String, Seen) {
    let seen = Seen::default();
    let app = Router::new()
        .fallback(any(stand_in))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, seen)
}

async fn ai6ctl(server: &str, args: &[&str]) -> (bool, String, String) {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let server = server.to_owned();
    tokio::task::spawn_blocking(move || {
        let output = Command::new(env!("CARGO_BIN_EXE_ai6ctl"))
            .arg("--server")
            .arg(server)
            .args(args)
            .env("UNEX_ACCESS_TOKEN", "token")
            .output()
            .unwrap();
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn create_sends_the_scopes_and_prints_the_value_alone_on_standard_output() {
    let (server, seen) = serve().await;
    let (ok, out, err) = ai6ctl(
        &server,
        &[
            "keys",
            "create",
            "--scope",
            "config.manage@config:plugin",
            "--scope",
            "config.read@config:plugin",
            "--expires",
            "30d",
            "--name",
            "deploy",
        ],
    )
    .await;
    assert!(ok, "{err}");
    assert_eq!(
        out,
        format!("{VALUE}\n"),
        "nothing but the value on standard output"
    );
    assert!(
        !err.contains(VALUE),
        "the value is not repeated on standard error"
    );
    let post = seen
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.0 == "POST")
        .cloned()
        .unwrap();
    assert_eq!(
        post.2,
        json!({
            "name": "deploy",
            "scopes": ["config.manage@config:plugin", "config.read@config:plugin"],
            "expires_in": 2_592_000.0
        })
    );
}

#[tokio::test]
async fn create_can_write_the_value_to_a_new_private_file_and_never_replaces_one() {
    let (server, seen) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("token");
    let path = file.to_str().unwrap();
    let args = [
        "keys",
        "create",
        "--scope",
        "config.manage@config:plugin",
        "--output",
        path,
    ];
    let (ok, out, err) = ai6ctl(&server, &args).await;
    assert!(ok, "{err}");
    assert!(!out.contains(VALUE) && !err.contains(VALUE), "{out}{err}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{VALUE}\n")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // Asking again names a file that exists: refused before another key is made.
    let made = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.0 == "POST")
        .count();
    let (ok, _, err) = ai6ctl(&server, &args).await;
    assert!(!ok && err.contains("exists"), "{err}");
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|r| r.0 == "POST")
            .count(),
        made
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{VALUE}\n")
    );
}

#[tokio::test]
async fn a_refusal_prints_the_gateway_s_reason_and_a_bad_duration_asks_nothing() {
    let (server, seen) = serve().await;
    let (ok, _, err) = ai6ctl(
        &server,
        &["keys", "create", "--scope", "roster.manage@config:roster"],
    )
    .await;
    assert!(
        !ok && err.contains("you do not hold roster.manage"),
        "{err}"
    );
    let before = seen.lock().unwrap().len();
    let (ok, _, err) = ai6ctl(
        &server,
        &[
            "keys",
            "create",
            "--scope",
            "config.manage@config:plugin",
            "--expires",
            "soon",
        ],
    )
    .await;
    assert!(!ok && err.contains("duration"), "{err}");
    assert_eq!(seen.lock().unwrap().len(), before);
    let (ok, _, err) = ai6ctl(&server, &["keys", "create"]).await;
    assert!(
        !ok && err.contains("--scope"),
        "a key names at least one scope: {err}"
    );
}

#[tokio::test]
async fn list_shows_name_scope_created_expires_and_last_used_and_all_asks_for_everyone() {
    let (server, seen) = serve().await;
    let (ok, out, err) = ai6ctl(&server, &["keys", "list"]).await;
    assert!(ok, "{err}");
    assert!(out.contains("NAME") && out.contains("LAST USED"));
    assert!(
        out.contains("key-1\tdeploy\talice\tconfig.manage@config:plugin\t1000\tnever\t2000"),
        "{out}"
    );
    let (ok, _, _) = ai6ctl(&server, &["keys", "list", "--all"]).await;
    assert!(ok);
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|r| r.1 == "/v1/user/keys?all=true")
    );
}

#[tokio::test]
async fn revoke_deletes_the_key_by_id() {
    let (server, seen) = serve().await;
    let (ok, out, err) = ai6ctl(&server, &["keys", "revoke", "key-1"]).await;
    assert!(ok, "{err}");
    assert!(out.contains("key key-1 revoked"));
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|r| r.0 == "DELETE" && r.1 == "/v1/user/keys/key-1")
    );
    let (ok, _, err) = ai6ctl(&server, &["keys", "revoke", "nope"]).await;
    assert!(!ok && err.contains("404"), "{err}");
}
