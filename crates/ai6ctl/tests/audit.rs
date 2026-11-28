//! `ai6ctl audit` against a stand-in gateway that answers as `GET /v1/user/audit` does.

use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::any;
use serde_json::json;

type Seen = Arc<Mutex<Vec<String>>>;

async fn stand_in(State(seen): State<Seen>, uri: Uri) -> impl IntoResponse {
    seen.lock().unwrap().push(uri.to_string());
    let query = uri.query().unwrap_or_default();
    if query.contains("operation_id=secret") {
        return StatusCode::FORBIDDEN.into_response();
    }
    let all = [
        json!({"id": "a1", "actor": "station-api", "action": "internal.read", "operation": "G",
               "resource": "station:s1", "target": "GET /internal/missions/{mission_id}", "at": 1000.9}),
        json!({"id": "a2", "actor": "alice via key k1", "action": "run.context.read", "operation": "G",
               "resource": null, "target": "run-1\u{1b}[31m", "at": 900.0}),
    ];
    // Like the gateway: only what starts with the prefix.
    let wanted = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("action="))
        .unwrap_or_default();
    let rows: Vec<_> = all
        .into_iter()
        .filter(|row| row["action"].as_str().unwrap().starts_with(wanted))
        .collect();
    axum::Json(rows).into_response()
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
async fn audit_list_prints_entries_filters_by_action_prefix_and_names_a_refusal() {
    let (server, seen) = serve().await;
    let (ok, out, err) = ai6ctl(&server, &["audit", "list", "--operation", "G"]).await;
    assert!(ok, "{err}");
    assert!(
        out.starts_with("AT\tACTOR\tACTION\tRESOURCE\tTARGET\n"),
        "{out}"
    );
    assert!(
        out.contains(
            "1000\tstation-api\tinternal.read\tstation:s1\tGET /internal/missions/{mission_id}"
        ),
        "{out}"
    );
    assert!(
        out.contains("900\talice via key k1\trun.context.read\toperation:G\t"),
        "a record with no resource is of its Operation: {out}"
    );
    assert!(
        !out.contains('\u{1b}'),
        "no control character reaches the terminal: {out}"
    );

    let (ok, out, err) = ai6ctl(
        &server,
        &["audit", "list", "--action", "internal.", "--limit", "5"],
    )
    .await;
    assert!(ok, "{err}");
    assert!(
        out.contains("internal.read") && !out.contains("run.context"),
        "{out}"
    );
    let last = seen.lock().unwrap().last().unwrap().clone();
    assert!(
        last.contains("action=internal.") && last.contains("limit=5"),
        "{last}"
    );

    let (ok, _, err) = ai6ctl(&server, &["audit", "list", "--operation", "secret"]).await;
    assert!(!ok && err.contains("audit.read"), "{err}");
}
