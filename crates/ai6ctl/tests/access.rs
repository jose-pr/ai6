//! `ai6ctl access` and `ai6ctl accounts` against a stand-in gateway that records what it was
//! asked and answers as the real routes do.

use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::any;
use serde_json::{Value, json};

type Seen = Arc<Mutex<Vec<(String, String, Option<String>, Value)>>>;

const VIEW: &str = r#"{
  "revision": "r1", "writable": true,
  "roles": [
    {"name": "chief", "built_in": true, "editable": false, "actions": ["mission.read"]},
    {"name": "operator", "built_in": true, "editable": true, "actions": ["mission.read", "mission.create"]}
  ],
  "rules": [{"subject": "group:ops", "scope": "operation:G", "role": "operator", "propagate": false}],
  "delegable": {"actions": ["mission.read"], "default": true, "weakenings": []},
  "chief": {"stored": ["account:boss"], "host": ["account:root"]}
}"#;

async fn stand_in(
    State(seen): State<Seen>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    let revision = headers
        .get("if-match")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim_matches('"').to_owned());
    let json: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    seen.lock()
        .unwrap()
        .push((method.to_string(), uri.to_string(), revision.clone(), json));
    let path = uri.path();
    let refused = |reason: &str| {
        (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": reason })),
        )
            .into_response()
    };
    match (method.as_str(), path) {
        ("GET", "/v1/user/access" | "/v1/user/operations/P/access") => {
            (StatusCode::OK, VIEW.to_owned()).into_response()
        }
        ("GET", "/v1/user/access/check") => axum::Json(
            json!({"allowed": true, "tier": "global", "rule_index": 0, "role": "operator"}),
        )
        .into_response(),
        ("GET", "/v1/user/accounts") => {
            axum::Json(json!([{"id": "a-id", "name": "ann", "groups": ["ops"], "disabled": false}]))
                .into_response()
        }
        ("PUT", "/v1/user/accounts/ann") => {
            axum::Json(json!({"account": {}, "secret": "s3cret"})).into_response()
        }
        ("POST", "/v1/user/accounts/ann/secret") => {
            axum::Json(json!({"account": {}, "secret": "n3w-s3cret"})).into_response()
        }
        ("PUT", "/v1/user/accounts/boss") => {
            refused("only a chief changes an account that is chief")
        }
        (_, p) if p.contains("/roles/bad") => refused("bad is not allowed"),
        (_, p)
            if p.starts_with("/v1/user/access")
                || p.starts_with("/v1/user/operations/P/access") =>
        {
            if revision.as_deref() == Some("r1") {
                axum::Json(json!({"revision": "r2"})).into_response()
            } else {
                (
                    StatusCode::CONFLICT,
                    axum::Json(json!({"error": "changed since you read it"})),
                )
                    .into_response()
            }
        }
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
async fn show_prints_the_policy_and_an_operation_s_own() {
    let (server, _) = serve().await;
    let (ok, out, err) = ai6ctl(&server, &["access", "show"]).await;
    assert!(ok, "{err}");
    assert!(out.contains("revision\tr1"));
    assert!(out.contains("role\tchief (built in, fixed)"));
    assert!(out.contains("rule 0\tgroup:ops\toperation:G\toperator (this scope only)"));
    assert!(out.contains("chief host\taccount:root"));
    let (ok, out, _) = ai6ctl(&server, &["access", "show", "--operation", "P"]).await;
    assert!(ok && out.contains("operation\tP"));
    let (ok, _, err) = ai6ctl(&server, &["access", "show", "--operation", "Z"]).await;
    assert!(!ok && err.contains("404"), "{err}");
}

#[tokio::test]
async fn a_role_is_set_with_the_revision_read_and_a_refusal_prints_the_reason() {
    let (server, seen) = serve().await;
    let (ok, out, err) = ai6ctl(
        &server,
        &[
            "access",
            "role",
            "set",
            "reviewer",
            "--actions",
            "mission.read,session.read",
        ],
    )
    .await;
    assert!(ok, "{err}");
    assert!(out.contains("role reviewer changed"));
    let put = seen
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.0 == "PUT")
        .cloned()
        .unwrap();
    assert_eq!(put.2.as_deref(), Some("r1"));
    assert_eq!(put.3["actions"], json!(["mission.read", "session.read"]));
    let (ok, _, err) = ai6ctl(
        &server,
        &["access", "role", "set", "bad", "--actions", "x.y"],
    )
    .await;
    assert!(!ok && err.contains("bad is not allowed"), "{err}");
    let (ok, out, _) = ai6ctl(
        &server,
        &["access", "role", "remove", "reviewer", "--operation", "P"],
    )
    .await;
    assert!(ok && out.contains("changed"));
}

#[tokio::test]
async fn rules_export_and_import_carry_the_revision_and_a_stale_one_fails() {
    let (server, seen) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("rules.toml");
    let path = file.to_str().unwrap();
    let (ok, _, err) = ai6ctl(&server, &["access", "rules", "export", path]).await;
    assert!(ok, "{err}");
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("revision = \"r1\"") && written.contains("propagate = false"));
    let (ok, out, err) = ai6ctl(&server, &["access", "rules", "import", path]).await;
    assert!(ok, "{err}");
    assert!(out.contains("1 rules imported"));
    let put = seen
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|r| r.0 == "PUT")
        .cloned()
        .unwrap();
    assert_eq!(put.2.as_deref(), Some("r1"));
    assert_eq!(put.3["rules"][0]["subject"], "group:ops");
    std::fs::write(&file, written.replace("r1", "old")).unwrap();
    let (ok, _, err) = ai6ctl(&server, &["access", "rules", "import", path]).await;
    assert!(!ok && err.contains("changed since you read it"), "{err}");
}

#[tokio::test]
async fn delegable_chief_and_check() {
    let (server, seen) = serve().await;
    let (ok, _, err) = ai6ctl(
        &server,
        &["access", "delegable", "set", "mission.read,pool.read"],
    )
    .await;
    assert!(ok, "{err}");
    let (ok, _, err) = ai6ctl(
        &server,
        &["access", "chief", "set", "account:boss", "group:staff"],
    )
    .await;
    assert!(ok, "{err}");
    let puts: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.0 == "PUT")
        .cloned()
        .collect();
    assert_eq!(puts[0].3["actions"], json!(["mission.read", "pool.read"]));
    assert_eq!(puts[1].3["chief"], json!(["account:boss", "group:staff"]));
    let (ok, out, err) = ai6ctl(
        &server,
        &[
            "access",
            "check",
            "account:ann",
            "mission.create",
            "operation:G",
        ],
    )
    .await;
    assert!(ok, "{err}");
    assert_eq!(out.trim(), "allowed\ttier global\trule 0\trole operator");
    let (ok, _, _) = ai6ctl(&server, &["access", "delegable", "set"]).await;
    assert!(!ok, "no actions is a usage error");
}

#[tokio::test]
async fn accounts_list_create_groups_and_reset_print_the_secret_once() {
    let (server, _) = serve().await;
    let (ok, out, err) = ai6ctl(&server, &["accounts"]).await;
    assert!(ok, "{err}");
    assert!(out.contains("ann\tops\tfalse"));
    let (ok, out, _) = ai6ctl(
        &server,
        &["accounts", "create", "ann", "--groups", "ops,qa"],
    )
    .await;
    assert!(ok && out.contains("password\ts3cret"));
    let (ok, out, err) = ai6ctl(&server, &["accounts", "reset-password", "ann"]).await;
    assert!(ok, "{err}");
    assert!(out.contains("password\tn3w-s3cret"));
    assert!(
        !err.contains("n3w-s3cret"),
        "the secret goes to stdout only"
    );
    let (ok, out, _) = ai6ctl(&server, &["accounts", "disable", "ann"]).await;
    assert!(ok && out.contains("disabled"));
    let (ok, out, _) = ai6ctl(&server, &["accounts", "enable", "ann"]).await;
    assert!(ok && out.contains("enabled"));
    let (ok, out, _) = ai6ctl(&server, &["accounts", "groups", "ann", "a,b"]).await;
    assert!(ok && out.contains("groups of ann set"));
    let (ok, _, err) = ai6ctl(&server, &["accounts", "disable", "boss"]).await;
    assert!(!ok && err.contains("only a chief"), "{err}");
}
