//! Every request for a private page names its viewer and is checked against the page: by the
//! page cookie a sign-in in a browser sets, or by the gateway's page authenticator. Neither opens
//! anything under `/v1`.
//!
//! What Chrome 155 and Edge 156 send for a frame like the gateway's (a top-level document whose
//! sandboxed `<iframe>` holds the page, `allow-scripts` without `allow-same-origin`):
//! - the frame, typed, reloaded, opened in a new tab or reached by a link from another site: a
//!   `SameSite=Lax` or `None` cookie, `Strict` too except from another site;
//! - the page's document, loaded by the frame: every cookie (`Sec-Fetch-Site: same-origin`,
//!   `Sec-Fetch-Dest: iframe`);
//! - everything the page asks for itself, scripts, styles and images (`no-cors`) and
//!   `fetch`/XHR (`cors`, `Origin: null`): `Sec-Fetch-Site: cross-site`, no `Referer` whatever
//!   the policy, and only a `SameSite=None` cookie; a `fetch` or XHR sends even that only with
//!   `credentials: 'include'` (`withCredentials`), an `EventSource` never. A `Partitioned`
//!   cookie set on the gateway's own site is not sent on them either;
//! - another site's page asking for the same addresses (`<img>`, `<script>`, a credentialed
//!   `fetch`, from a sandboxed frame or not): the `SameSite=None` cookie as well.
//!
//! So the cookie is `SameSite=None`, and a page's files and feeds also need the view token its
//! frame names for its viewer, which another site does not know.

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

/// `url` asked for with the page cookie `cookie`, when there is one, and `headers`, by a client
/// that keeps no cookie of its own.
pub(super) async fn get_with(
    url: &str,
    cookie: Option<&str>,
    headers: &[(&str, &str)],
) -> reqwest::Response {
    let mut request = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(url);
    if let Some(cookie) = cookie {
        request = request.header("cookie", format!("{PAGE_COOKIE}={cookie}"));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.send().await.unwrap()
}

/// Shares page `id` with `subjects`, as its person.
async fn share(served: &Served, id: &str, subjects: &[&str]) {
    let shared = reqwest::Client::new()
        .put(format!("{}/v1/user/pages/{id}/shares", served.base))
        .bearer_auth(&served.operator)
        .json(&json!({"subjects": subjects}))
        .send()
        .await
        .unwrap();
    assert_eq!(shared.status(), StatusCode::OK);
}

/// The base of the files and feeds the frame at `frame` names for the viewer of `cookie`.
async fn view_of(frame: &str, cookie: &str) -> String {
    let html = get_with(frame, Some(cookie), &[])
        .await
        .text()
        .await
        .unwrap();
    let token = html
        .split("<iframe src=\"")
        .nth(1)
        .and_then(|rest| rest.split("/f/").next())
        .unwrap();
    format!("{frame}{token}/")
}

#[tokio::test]
async fn a_sign_in_sets_the_page_cookie_and_the_page_opens_by_itself_with_it() {
    let served = served(AssistantConfig::default()).await;
    let id = dashboard(&served).await;
    let set = page_cookie_header(&served, &served.operator).await;
    let attributes: Vec<&str> = set.split("; ").skip(1).collect();
    assert_eq!(
        attributes[..4],
        ["Path=/pages/", "HttpOnly", "Secure", "SameSite=None"],
        "{set}"
    );
    assert!(attributes[4].starts_with("Max-Age="), "{set}");
    let cookie = page_cookie(&served, &served.operator).await;
    let frame = format!("{}/pages/p/{id}/", served.base);
    // Opened by itself: a bookmark, a new tab, a reload from another site.
    for site in ["none", "same-origin", "cross-site"] {
        let opened = get_with(&frame, Some(&cookie), &[("sec-fetch-site", site)]).await;
        assert_eq!(opened.status(), StatusCode::OK, "{site}");
    }
    let view = view_of(&frame, &cookie).await;
    for asked in ["f/index.html", "data/board", "f/data/board"] {
        let answer = get_with(
            &format!("{view}{asked}"),
            Some(&cookie),
            &[
                ("sec-fetch-dest", "empty"),
                ("sec-fetch-site", "cross-site"),
                ("origin", "null"),
            ],
        )
        .await;
        assert_eq!(answer.status(), StatusCode::OK, "{asked}");
        // The page's script, which asked with its credentials, reads the answer.
        assert_eq!(answer.headers()["access-control-allow-origin"], "null");
        assert_eq!(answer.headers()["access-control-allow-credentials"], "true");
    }
    // Every opening is audited as its viewer.
    assert!(audits(&served, "moneypenny.page.opened").len() >= 4);
}

#[tokio::test]
async fn nobody_else_opens_it_and_without_a_credential_it_is_an_unknown_page() {
    let served = served(AssistantConfig::default()).await;
    let id = dashboard(&served).await;
    let frame = format!("{}/pages/p/{id}/", served.base);
    let unknown = format!("{}/pages/p/page-nope/", served.base);
    let answer = |response: reqwest::Response| async move {
        (response.status(), response.text().await.unwrap())
    };
    let missing = answer(get_with(&unknown, None, &[]).await).await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
    assert_eq!(answer(get_with(&frame, None, &[]).await).await, missing);
    for other in [&served.viewer, &served.admin, &served.member] {
        let theirs = page_cookie(&served, other).await;
        assert_eq!(
            answer(get_with(&frame, Some(&theirs), &[]).await).await,
            missing
        );
    }
    // A forged cookie names nobody.
    let mine = page_cookie(&served, &served.operator).await;
    let (sign_in, _) = mine.rsplit_once('.').unwrap();
    assert_eq!(
        answer(get_with(&frame, Some(&format!("{sign_in}.AAAA")), &[]).await).await,
        missing
    );
    // The page's files and feeds need the cookie and the view token its frame named for this
    // viewer: another site that knows the page's address has neither.
    let view = view_of(&frame, &mine).await;
    let file = format!("{view}f/index.html");
    assert_eq!(
        get_with(&file, None, &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    // Asked for as a top-level document, a file sends its viewer to the frame, and nobody else.
    let top = [("sec-fetch-dest", "document")];
    assert_eq!(
        get_with(&file, None, &top).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_with(&file, Some(&mine), &top).await.status(),
        StatusCode::SEE_OTHER
    );
    let token = view.trim_end_matches('/').rsplit('/').next().unwrap();
    let (expires, _) = token.split_once('.').unwrap();
    let guessed = file.replace(token, &format!("{expires}.open.AAAA"));
    assert_eq!(
        get_with(&guessed, Some(&mine), &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    // Shared with the viewer, it opens for them, under a view token of their own.
    share(&served, &id, &["person:viewer-007"]).await;
    let theirs = page_cookie(&served, &served.viewer).await;
    assert_eq!(
        get_with(&frame, Some(&theirs), &[]).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        get_with(&file, Some(&theirs), &[]).await.status(),
        StatusCode::NOT_FOUND,
        "the operator's view token is not the viewer's"
    );
    let their_view = view_of(&frame, &theirs).await;
    assert_eq!(
        get_with(&format!("{their_view}f/index.html"), Some(&theirs), &[])
            .await
            .status(),
        StatusCode::OK
    );
    // Unshared, they are refused at their next request.
    share(&served, &id, &[]).await;
    assert_eq!(
        get_with(&format!("{their_view}f/index.html"), Some(&theirs), &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_ended_sign_in_s_cookie_says_so_and_v1_takes_no_page_cookie() {
    let served = served(AssistantConfig::default()).await;
    let id = dashboard(&served).await;
    let bearer = issue_access_token_record(
        Arc::new(served.registry.clone()),
        "operator-007".into(),
        Duration::from_secs(600),
        ai6::TimeLimit::UNBOUNDED,
    )
    .await
    .ok()
    .unwrap()
    .token;
    let cookie = page_cookie(&served, &bearer).await;
    let frame = format!("{}/pages/p/{id}/", served.base);
    assert_eq!(
        get_with(&frame, Some(&cookie), &[]).await.status(),
        StatusCode::OK
    );
    // /v1 takes the cookie as nothing.
    let profile = get_with(
        &format!("{}/v1/user/profile", served.base),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(profile.status(), StatusCode::UNAUTHORIZED);
    // Signing out ends it, and tells the browser to drop it.
    let out = reqwest::Client::new()
        .post(format!("{}/v1/auth/logout", served.base))
        .bearer_auth(&bearer)
        .header("origin", "https://localhost")
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    assert!(
        out.headers()
            .get_all("set-cookie")
            .iter()
            .any(|value| value.to_str().unwrap().starts_with("ai6_pages=;")),
        "{:?}",
        out.headers()
    );
    let ended = get_with(&frame, Some(&cookie), &[]).await;
    assert_eq!(ended.status(), StatusCode::FORBIDDEN);
    assert!(ended.text().await.unwrap().contains("signed out"));
}

#[tokio::test]
async fn no_cookie_token_or_view_token_is_ever_logged_audited_or_answered() {
    let served = served(AssistantConfig::default()).await;
    let id = dashboard(&served).await;
    let cookie = page_cookie(&served, &served.operator).await;
    let (sign_in, mark) = cookie.rsplit_once('.').unwrap();
    let frame = format!("{}/pages/p/{id}/", served.base);
    let view = view_of(&frame, &cookie).await;
    let token = view.trim_end_matches('/').rsplit('/').next().unwrap();
    let view_mark = token.rsplit('.').next().unwrap();
    // Every refusal a request can meet, and the answers to them.
    let mut answers = Vec::new();
    let mut ask = async |url: String, cookie: Option<String>| {
        let response = get_with(&url, cookie.as_deref(), &[("origin", "null")]).await;
        let headers = format!("{:?}", response.headers());
        answers.push(format!("{headers} {}", response.text().await.unwrap()));
    };
    let forged = format!("{sign_in}.{}", "A".repeat(mark.len()));
    let theirs = page_cookie(&served, &served.viewer).await;
    ask(frame.clone(), Some(forged.clone())).await;
    ask(frame.clone(), Some(theirs.clone())).await;
    ask(format!("{view}f/index.html"), Some(theirs.clone())).await;
    ask(format!("{view}f/missing.html"), Some(cookie.clone())).await;
    ask(format!("{view}data/nope"), Some(cookie.clone())).await;
    ask(format!("{view}data/board"), Some(cookie.clone())).await;
    ask(format!("{view}f/index.html"), None).await;
    ask(
        format!("{}/v1/user/profile", served.base),
        Some(cookie.clone()),
    )
    .await;
    let guessed = view.replace(view_mark, &"A".repeat(view_mark.len()));
    ask(format!("{guessed}f/index.html"), Some(cookie.clone())).await;
    // Signed out, the same cookie and view token.
    let out = reqwest::Client::new()
        .post(format!("{}/v1/auth/logout", served.base))
        .bearer_auth(&served.operator)
        .header("origin", "https://localhost")
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    ask(frame.clone(), Some(cookie.clone())).await;
    ask(format!("{view}f/index.html"), Some(cookie.clone())).await;
    let logged = served.gateway.pages.clone().unwrap().logged();
    assert!(!logged.is_empty(), "the refusals were logged");
    let audited = serde_json::to_string(&served.registry.list_audit(500).unwrap()).unwrap();
    for secret in [
        mark,
        view_mark,
        served.operator.as_str(),
        &forged[sign_in.len() + 1..],
    ] {
        for (what, text) in answers
            .iter()
            .map(|answer| ("an answer", answer.as_str()))
            .chain(logged.iter().map(|line| ("the log", line.as_str())))
            .chain([("the audit", audited.as_str())])
        {
            assert!(!text.contains(secret), "{what} holds a secret: {text}");
        }
    }
}
