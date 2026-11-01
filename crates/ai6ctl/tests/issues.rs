//! `ai6ctl mission issues`, `ai6ctl issues` and `ai6ctl issue-proposals` against a stand-in
//! gateway that records what it was asked and answers as the real routes do.

use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::any;
use serde_json::{Value, json};

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

async fn stand_in(
    State(seen): State<Seen>,
    method: Method,
    uri: Uri,
    body: String,
) -> impl IntoResponse {
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    seen.lock()
        .unwrap()
        .push((method.to_string(), uri.path().to_owned(), parsed));
    let path = uri.path();
    match (method.as_str(), path) {
        ("GET", "/v1/user/operations/G/missions/m1/issues") => axum::Json(json!({
            "issues": [{
                "issue": "gh:o/r#1", "tracker": "gh", "item": "o/r#1",
                "url": "https://github.com/o/r/issues/1", "relation": "source", "state": "active",
                "linked_by": "m", "title": "A title that is not printed", "labels": [],
                "phase": "waiting", "open_question": {"asked_at": 1.0, "expires_at": 2.0},
            }],
            "entries": [{
                "id": "ox-1", "kind": "question", "issue": "gh:o/r#1", "actor": "agent:007",
                "state": "posted", "url": "https://github.com/o/r/issues/1#c", "reason": null,
                "dropped": [], "at": 1.0,
            }],
            "replies": {"delivered": 1, "pending": 0, "ignored": 2},
        }))
        .into_response(),
        ("GET", "/v1/user/operations/G/issue-proposals") => axum::Json(json!([{
            "id": "ip-1", "operation": "G", "mission_id": "m1", "proposer": "agent:007",
            "specs": [{"tracker": "gh", "title": "Text that is not printed"}],
            "findings": ["duplicate"], "dropped": ["assignees"], "state": "pending",
        }]))
        .into_response(),
        ("POST", "/v1/user/operations/G/issue-proposals/ip-1/approve")
        | ("POST", "/v1/user/operations/G/issue-proposals/ip-1/reject") => axum::Json(json!({
            "id": "ip-1", "operation": "G", "mission_id": "m1", "proposer": "agent:007",
            "specs": [], "findings": [], "dropped": [],
            "state": if path.ends_with("approve") { "approved" } else { "rejected" },
        }))
        .into_response(),
        ("POST", "/v1/user/operations/G/issues") => (
            StatusCode::ACCEPTED,
            axum::Json(json!({"proposal": "ip-2", "state": "approved"})),
        )
            .into_response(),
        (_, p) if p.starts_with("/v1/user/operations/G/missions/m1/issues") => {
            axum::Json(json!({"issue": "gh:o/r#1", "state": "queued"})).into_response()
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
async fn mission_issues_prints_links_and_posts_and_no_issue_text() {
    let (server, _) = serve().await;
    let (ok, stdout, stderr) =
        ai6ctl(&server, &["mission", "issues", "m1", "--operation", "G"]).await;
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("gh:o/r#1\tsource\tactive\twaiting\topen"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ox-1\tquestion\tgh:o/r#1\tposted"),
        "{stdout}"
    );
    assert!(!stdout.contains("not printed"), "{stdout}");
}

#[tokio::test]
async fn each_issues_command_reaches_its_route_with_its_body() {
    let (server, seen) = serve().await;
    let common = ["--mission", "m1", "--operation", "G"];
    let run = |args: Vec<&'static str>| {
        let server = server.clone();
        async move {
            let (ok, stdout, stderr) = ai6ctl(&server, &args).await;
            assert!(ok, "{args:?}: {stderr}");
            stdout
        }
    };
    let with = |head: &[&'static str], tail: &[&'static str]| {
        let mut all: Vec<&'static str> = head.to_vec();
        all.extend_from_slice(&common);
        all.extend_from_slice(tail);
        all
    };
    let linked = run(with(
        &["issues", "link", "gh:o/r#1"],
        &["--relation", "parent"],
    ))
    .await;
    assert!(linked.contains("gh:o/r#1\tqueued"), "{linked}");
    run(with(&["issues", "accept", "gh:o/r#1"], &[])).await;
    run(with(
        &["issues", "comment", "gh:o/r#1"],
        &["--body", "Use main."],
    ))
    .await;
    run(with(
        &["issues", "update", "gh:o/r#1"],
        &["--add-label", "bug", "--close", "--transition", "Done"],
    ))
    .await;
    run(with(&["issues", "unlink", "gh:o/r#1"], &[])).await;
    let created = run(vec![
        "issues",
        "create",
        "gh",
        "--operation",
        "G",
        "--title",
        "New",
        "--label",
        "bug",
        "--mission",
        "m1",
    ])
    .await;
    assert!(created.contains("ip-2\tapproved"), "{created}");

    let seen = seen.lock().unwrap();
    let base = "/v1/user/operations/G/missions/m1/issues";
    let reference = "gh:o%2Fr%231";
    assert_eq!(seen[0].0, "POST");
    assert_eq!(seen[0].1, base);
    assert_eq!(seen[0].2["relation"], "parent");
    assert_eq!(seen[1].1, format!("{base}/{reference}/accept"));
    assert_eq!(seen[2].1, format!("{base}/{reference}/comments"));
    assert_eq!(seen[2].2["body"], "Use main.");
    assert_eq!(seen[3].1, format!("{base}/{reference}/update"));
    assert_eq!(seen[3].2["add_labels"][0], "bug");
    assert_eq!(seen[3].2["close"], true);
    assert_eq!(seen[3].2["transition"], "Done");
    assert_eq!(seen[4].0, "DELETE");
    assert_eq!(seen[4].1, format!("{base}/{reference}"));
    assert_eq!(seen[5].1, "/v1/user/operations/G/issues");
    assert_eq!(seen[5].2["tracker"], "gh");
    assert_eq!(seen[5].2["mission"], "m1");
}

#[tokio::test]
async fn issue_proposals_are_listed_without_their_text_and_decided() {
    let (server, seen) = serve().await;
    let (ok, listed, stderr) =
        ai6ctl(&server, &["issue-proposals", "list", "--operation", "G"]).await;
    assert!(ok, "{stderr}");
    assert!(
        listed.contains("ip-1\tpending\tagent:007\tm1\t1\tduplicate\tassignees"),
        "{listed}"
    );
    assert!(!listed.contains("not printed"), "{listed}");

    let (ok, approved, stderr) = ai6ctl(
        &server,
        &[
            "issue-proposals",
            "approve",
            "ip-1",
            "--operation",
            "G",
            "--mark-ready",
        ],
    )
    .await;
    assert!(ok, "{stderr}");
    assert!(approved.contains("ip-1\tapproved"), "{approved}");
    let (ok, rejected, stderr) = ai6ctl(
        &server,
        &[
            "issue-proposals",
            "reject",
            "ip-1",
            "--operation",
            "G",
            "--reason",
            "no",
        ],
    )
    .await;
    assert!(ok, "{stderr}");
    assert!(rejected.contains("ip-1\trejected"), "{rejected}");
    let seen = seen.lock().unwrap();
    assert_eq!(seen[1].2["mark_ready"], true);
    assert_eq!(seen[2].2["reason"], "no");
}
