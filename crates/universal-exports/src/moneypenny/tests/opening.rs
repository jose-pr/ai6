//! A page keeps opening: a restart of the gateway leaves an open page and its address working, and
//! when a request stops being answered it says why, or logs why.

use super::*;

/// The operator's dashboard with one feed, `board`.
async fn dashboard(served: &Served) -> String {
    let (_app, job) = accepted(served, &served.operator, "A dashboard.").await;
    published_id(
        &publish(
            served,
            &job.mcp_token,
            json!({"kind": "dashboard", "title": "Board", "content": "<p>x</p>", "format": "html",
                   "feeds": [{"name": "board", "tool": "board", "input": {}}]}),
        )
        .await,
    )
}

/// What the page's frame asks for at `url`: its status and body.
async fn framed(url: &str) -> (StatusCode, String) {
    let response = plain()
        .get(url)
        .header("sec-fetch-dest", "iframe")
        .send()
        .await
        .unwrap();
    (response.status(), response.text().await.unwrap())
}

#[tokio::test]
async fn pages_an_open_page_keeps_its_files_and_feeds_across_a_restart() {
    let mut served = served(AssistantConfig::default()).await;
    let id = dashboard(&served).await;
    let opened = open_frame(&frame_url(&served, &served.operator, &id, None).await).await;
    assert_eq!(
        framed(&format!("{}f/index.html", opened.view)).await.0,
        StatusCode::OK
    );

    let before = served.base.clone();
    served.restart().await;
    let moved = |url: &str| url.replacen(&before, &served.base, 1);
    // The browser still holds its cookie, and the page already open names its files as before.
    let view = moved(&opened.view);
    viewing_cookies()
        .lock()
        .unwrap()
        .push((view.clone(), opened.cookie.clone()));
    let file = framed(&format!("{view}f/index.html")).await;
    assert_eq!(file.0, StatusCode::OK, "{}", file.1);
    assert!(file.1.contains("<p>x</p>"), "{}", file.1);
    assert_eq!(framed(&format!("{view}data/board")).await.0, StatusCode::OK);
    // Its address opens it again.
    let reloaded = plain()
        .get(format!("{}#{}", moved(&opened.frame), opened.cookie))
        .send()
        .await
        .unwrap();
    assert_eq!(reloaded.status(), StatusCode::OK);
}

#[tokio::test]
async fn pages_a_missing_file_answers_404_and_the_log_names_it_once() {
    let served = served(AssistantConfig::default()).await;
    let id = dashboard(&served).await;
    let opened = open_frame(&frame_url(&served, &served.operator, &id, None).await).await;
    for _ in 0..2 {
        assert_eq!(
            framed(&format!("{}f/missing.html", opened.view)).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let logged = served.gateway.pages.clone().unwrap().logged();
    let named: Vec<&String> = logged
        .iter()
        .filter(|line| line.contains("missing.html"))
        .collect();
    assert_eq!(named.len(), 1, "{logged:?}");
    assert!(
        named[0].contains(&id) && named[0].contains("operator-007"),
        "{named:?}"
    );
    // A file that is there is served and logs nothing.
    assert_eq!(
        framed(&format!("{}f/index.html", opened.view)).await.0,
        StatusCode::OK
    );
    assert_eq!(served.gateway.pages.clone().unwrap().logged(), logged);
}

#[tokio::test]
async fn pages_a_viewer_whose_sign_in_ended_is_told_so() {
    let served = served(AssistantConfig::default()).await;
    let id = dashboard(&served).await;
    let opened = open_frame(&frame_url(&served, &served.operator, &id, None).await).await;
    // The operator's sign-in runs out while the page is open.
    age_token(&served, &served.operator, 3600.0);
    for asked in ["f/index.html", "data/board", "f/data/board"] {
        let (status, body) = framed(&format!("{}{asked}", opened.view)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{asked}");
        assert!(body.contains("signed out"), "{asked}: {body}");
    }
    let reloaded = opened.reload().await;
    assert_eq!(reloaded.status(), StatusCode::FORBIDDEN);
    assert!(reloaded.text().await.unwrap().contains("signed out"));
    let logged = served.gateway.pages.clone().unwrap().logged();
    let ended = logged
        .iter()
        .filter(|line| line.contains("sign-in that has ended"))
        .count();
    assert_eq!(ended, 1, "logged once for the page: {logged:?}");
}
