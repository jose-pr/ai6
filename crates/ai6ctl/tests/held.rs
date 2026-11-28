//! `ai6ctl held-briefs` against a stand-in gateway that refuses a reader.

use std::process::Command;

use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::any;
use serde_json::json;

async fn refuse() -> impl IntoResponse {
    (
        StatusCode::FORBIDDEN,
        axum::Json(json!({"error": "request is not authorized"})),
    )
}

async fn serve() -> String {
    let app = Router::new().fallback(any(refuse));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
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
async fn a_refused_held_brief_list_names_mission_create_and_that_read_is_not_enough() {
    let server = serve().await;
    let (ok, _, err) = ai6ctl(&server, &["held-briefs", "list", "--operation", "g"]).await;
    assert!(
        !ok && err.contains("mission.create") && err.contains("not enough"),
        "{err}"
    );
}

#[tokio::test]
async fn the_verb_help_states_the_requirement() {
    let server = serve().await;
    let (ok, out, err) = ai6ctl(&server, &["held-briefs", "list", "--help"]).await;
    assert!(ok, "{err}");
    assert!(out.contains("mission.read is not enough"), "{out}");
}
