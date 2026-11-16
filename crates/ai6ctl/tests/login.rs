//! `ai6ctl login` and `ai6ctl factors` against a stand-in gateway that answers as the real
//! routes do: a password then a one-time code, and a second factor set up and reset.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::any;
use serde_json::{Value, json};

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

async fn stand_in(
    State(seen): State<Seen>,
    method: Method,
    uri: Uri,
    _headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    let json: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    seen.lock()
        .unwrap()
        .push((method.to_string(), uri.path().to_owned(), json.clone()));
    match (method.as_str(), uri.path()) {
        ("POST", "/v1/auth/login") => match (json["pending"].as_str(), json["secret"].as_str()) {
            (None, Some("pw")) if json["name"] == "alice" => (
                StatusCode::ACCEPTED,
                axum::Json(json!({
                    "pending": "waiting-1",
                    "methods": ["totp", "recovery_code"],
                    "expires_in": 300.0
                })),
            )
                .into_response(),
            (Some("waiting-1"), Some("123456")) if json["method"] == "totp" => (
                StatusCode::CREATED,
                axum::Json(json!({"token": "token-1", "token_id": "t"})),
            )
                .into_response(),
            (Some("waiting-1"), Some("RECOVER-1")) if json["method"] == "recovery_code" => (
                StatusCode::CREATED,
                axum::Json(json!({"token": "token-2", "token_id": "t"})),
            )
                .into_response(),
            _ => StatusCode::UNAUTHORIZED.into_response(),
        },
        ("GET", "/v1/user/factors") => axum::Json(json!({
            "configured": true, "enrolled": false, "enrolling": false, "may_enrol": true
        }))
        .into_response(),
        ("POST", "/v1/user/factors/totp") => axum::Json(json!({
            "uri": "otpauth://totp/Example:alice?secret=ABC",
            "recovery_codes": ["AAAAA-BBBBB", "CCCCC-DDDDD"]
        }))
        .into_response(),
        ("POST", "/v1/user/factors/totp/confirm") => {
            if json["code"] == "654321" {
                axum::Json(json!({"enrolled": true})).into_response()
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"error": "that code is not right"})),
                )
                    .into_response()
            }
        }
        ("DELETE", "/v1/user/access/factors/id-alice") => StatusCode::NO_CONTENT.into_response(),
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

/// Runs `ai6ctl` with `stdin` on its standard input and `env` set.
async fn ai6ctl(
    server: &str,
    args: &[&str],
    env: &[(&str, &str)],
    stdin: &str,
) -> (bool, String, String) {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    let (server, stdin) = (server.to_owned(), stdin.to_owned());
    tokio::task::spawn_blocking(move || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ai6ctl"));
        command
            .arg("--server")
            .arg(server)
            .args(args)
            .env_remove("UNEX_ACCESS_TOKEN")
            .env_remove("UNEX_PASSWORD")
            .env_remove("UNEX_CODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
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
async fn login_asks_for_a_code_after_the_password_and_prints_only_the_token() {
    let (server, seen) = serve().await;
    let (ok, out, err) = ai6ctl(
        &server,
        &["login", "--name", "alice"],
        &[("UNEX_PASSWORD", "pw")],
        "123456\n",
    )
    .await;
    assert!(ok, "{err}");
    assert_eq!(
        out, "token-1\n",
        "the token is the only thing on standard output"
    );
    assert!(err.contains("Code: "), "{err}");
    assert!(!err.contains("pw") && !err.contains("123456"));
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].2, json!({"name": "alice", "secret": "pw"}));
    assert_eq!(
        seen[1].2,
        json!({"pending": "waiting-1", "method": "totp", "secret": "123456"})
    );
}

#[tokio::test]
async fn login_takes_the_password_and_the_first_code_from_standard_input_or_a_recovery_code() {
    let (server, seen) = serve().await;
    let (ok, out, err) = ai6ctl(
        &server,
        &["login", "--name", "alice", "--recovery"],
        &[],
        "pw\nRECOVER-1\n",
    )
    .await;
    assert!(ok, "{err}");
    assert_eq!(out, "token-2\n");
    assert_eq!(seen.lock().unwrap()[1].2["method"], "recovery_code");
}

#[tokio::test]
async fn a_wrong_code_is_refused_without_a_token() {
    let (server, _) = serve().await;
    let (ok, out, err) = ai6ctl(
        &server,
        &["login", "--name", "alice"],
        &[("UNEX_PASSWORD", "pw"), ("UNEX_CODE", "000000")],
        "",
    )
    .await;
    assert!(!ok);
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("401"), "{err}");
}

#[tokio::test]
async fn factors_set_up_show_codes_once_confirm_and_reset() {
    let (server, seen) = serve().await;
    let token = [("UNEX_ACCESS_TOKEN", "token")];
    let (ok, out, err) = ai6ctl(&server, &["factors", "status"], &token, "").await;
    assert!(ok, "{err}");
    assert!(out.contains("may enrol\ttrue"), "{out}");
    let (ok, out, err) = ai6ctl(&server, &["factors", "enrol"], &token, "").await;
    assert!(ok, "{err}");
    assert!(out.starts_with("otpauth://totp/"), "{out}");
    assert!(out.contains("recovery\tAAAAA-BBBBB"), "{out}");
    let (ok, _, err) = ai6ctl(&server, &["factors", "confirm", "000000"], &token, "").await;
    assert!(!ok && err.contains("that code is not right"), "{err}");
    let (ok, out, err) = ai6ctl(&server, &["factors", "confirm", "654321"], &token, "").await;
    assert!(ok, "{err}");
    assert!(out.contains("enrolled"));
    // Without the code on the command line it is read from standard input.
    let (ok, out, err) = ai6ctl(&server, &["factors", "confirm"], &token, "654321\n").await;
    assert!(ok, "{err}");
    assert!(
        out.contains("enrolled") && err.contains("Code: "),
        "{out} {err}"
    );
    let (ok, out, err) = ai6ctl(&server, &["factors", "reset", "id-alice"], &token, "").await;
    assert!(ok, "{err}");
    assert!(out.contains("second factor removed"));
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|(method, path, _)| method == "DELETE" && path.ends_with("/factors/id-alice"))
    );
}
