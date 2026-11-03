//! A page is private: its link opens in the browser that first asked for it and nowhere else, and
//! a public link is the person's choice, with no feeds.

use ai6::{ConfigProvider, ConfigRecord, ConfigScope, SqlProvider};

use super::*;

/// What a response says, apart from the moment it was made: its status, headers and body.
async fn answer(response: reqwest::Response) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
    let status = response.status();
    let mut headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str() != "date")
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    headers.sort();
    (status, headers, response.bytes().await.unwrap().to_vec())
}

async fn get(url: &str) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
    answer(plain().get(url).send().await.unwrap()).await
}

/// The operator's dashboard with one feed, `board`.
async fn board_dashboard(served: &Served) -> String {
    let (_app, job) = accepted(served, &served.operator, "Private.").await;
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

/// The token in a link's address.
fn token_of(url: &str) -> &str {
    url.trim_end_matches('/').rsplit('/').next().unwrap()
}

#[tokio::test]
async fn pages_a_copied_link_answers_as_an_unknown_one_and_the_first_browser_keeps_it() {
    let served = served(AssistantConfig::default()).await;
    let id = board_dashboard(&served).await;
    let frame = frame_url(&served, &served.operator, &id, None).await;
    let token = token_of(&frame).to_owned();
    let unknown = frame.replace(&token, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");

    // The first request binds the link to this browser: its cookie applies to that link only.
    let first = plain().get(&frame).send().await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let set = first.headers()["set-cookie"].to_str().unwrap().to_owned();
    let cookie = set
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("ai6_page=")
        .unwrap()
        .to_owned();
    let attributes: Vec<&str> = set.split("; ").skip(1).collect();
    assert_eq!(
        attributes[..4],
        [
            format!("Path=/pages/v/{token}/").as_str(),
            "HttpOnly",
            "Secure",
            "SameSite=Strict"
        ],
        "{set}"
    );
    let max_age: u64 = attributes[4]
        .strip_prefix("Max-Age=")
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=3600).contains(&max_age), "{set}");
    assert_eq!(attributes.len(), 5, "{set}");
    let html = first.text().await.unwrap();
    let view = html
        .split("<iframe src=\"../")
        .nth(1)
        .and_then(|rest| rest.split("/f/").next())
        .unwrap()
        .to_owned();
    assert!(
        !html.contains(&token),
        "the frame does not repeat its own token"
    );
    assert_eq!(audits(&served, "moneypenny.page.opened").len(), 1);

    // A copy of the address, without the cookie or with another, is no link at all: the same
    // answer as an unknown token, on the frame and with the address-bar token on f/ and data/.
    let copied = get(&frame).await;
    assert_eq!(copied.0, StatusCode::NOT_FOUND);
    assert_eq!(copied, get(&unknown).await);
    let wrong = plain()
        .get(&frame)
        .header("cookie", "ai6_page=not-the-cookie")
        .send()
        .await
        .unwrap();
    assert_eq!(answer(wrong).await, get(&unknown).await);
    for suffix in ["f/index.html", "data/board"] {
        assert_eq!(
            get(&format!("{frame}{suffix}")).await,
            get(&format!("{unknown}{suffix}")).await,
            "{suffix}"
        );
    }
    // The view token opens the frame of nothing either.
    let view_frame = frame.replace(&token, &view);
    assert_eq!(get(&view_frame).await, get(&unknown).await);

    // The browser that opened it reloads it, and the page's files and feeds answer under the
    // view token alone.
    let again = plain()
        .get(&frame)
        .header("cookie", format!("other=1; ai6_page={cookie}"))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    assert!(again.headers().get("set-cookie").is_none());
    let base = frame.replace(&token, &view);
    assert_eq!(get(&format!("{base}f/index.html")).await.0, StatusCode::OK);
    assert_eq!(get(&format!("{base}data/board")).await.0, StatusCode::OK);
    assert_eq!(audits(&served, "moneypenny.page.opened").len(), 1);
}

#[tokio::test]
async fn pages_on_their_own_origin_the_cookie_is_the_links_path_there() {
    let served = served_full(AssistantConfig::default(), None, true).await;
    let id = board_dashboard(&served).await;
    let frame = frame_url(&served, &served.operator, &id, None).await;
    let token = token_of(&frame);
    let first = plain().get(&frame).send().await.unwrap();
    let set = first.headers()["set-cookie"].to_str().unwrap();
    assert!(set.contains(&format!("; Path=/v/{token}/; ")), "{set}");
}

#[tokio::test]
async fn pages_a_page_hidden_from_a_person_answers_as_a_missing_one() {
    let served = served(AssistantConfig::default()).await;
    let id = board_dashboard(&served).await;
    for bearer in [&served.viewer, &served.admin, &served.member] {
        let hidden = answer(open_page(&served, bearer, &id, None).await).await;
        let missing = answer(open_page(&served, bearer, "page-nope", None).await).await;
        assert_eq!(hidden.0, StatusCode::NOT_FOUND);
        assert_eq!(hidden, missing);
    }
}

async fn set_public(served: &Served, bearer: &str, id: &str, on: bool) -> reqwest::Response {
    reqwest::Client::new()
        .put(format!("{}/v1/user/pages/{id}/public", served.base))
        .bearer_auth(bearer)
        .json(&json!({"on": on}))
        .send()
        .await
        .unwrap()
}

/// A public URL as the test listener serves it.
fn local(served: &Served, url: &str) -> String {
    url.replacen(
        "https://localhost/pages",
        &format!("{}/pages", served.base),
        1,
    )
}

#[tokio::test]
async fn pages_a_public_link_needs_no_sign_in_serves_no_feed_and_stops_when_turned_off() {
    let directory = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", directory.path().join("roster.db").display());
    let source = Arc::new(SqlProvider::open(&location).unwrap());
    let put_pages = |pages: Value| {
        source
            .put(
                ConfigRecord::Assistant,
                &ConfigScope::Global,
                "settings",
                &json!({"pages": pages}),
            )
            .unwrap();
    };
    put_pages(json!({}));
    let served = served_with(AssistantConfig::default(), Some(source.clone())).await;
    let id = board_dashboard(&served).await;
    // Only its person makes one.
    assert_eq!(
        set_public(&served, &served.admin, &id, true).await.status(),
        StatusCode::NOT_FOUND
    );
    let on = set_public(&served, &served.operator, &id, true).await;
    assert_eq!(on.status(), StatusCode::OK);
    let summary = on.json::<Value>().await.unwrap();
    assert_eq!(summary["public"], true);
    let url = summary["public_url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("https://localhost/pages/s/"), "{url}");
    let public = local(&served, &url);
    // Anyone opens it, with no cookie and no sign-in, as often as they like; its bar says so.
    for _ in 0..2 {
        let frame = plain().get(&public).send().await.unwrap();
        assert_eq!(frame.status(), StatusCode::OK);
        assert!(frame.headers().get("set-cookie").is_none());
        let html = frame.text().await.unwrap();
        assert!(html.contains("Public: anyone with this link"), "{html}");
        assert!(!html.contains("operator-007"), "{html}");
        assert!(html.contains("<iframe src=\"f/index.html\""), "{html}");
    }
    let file = plain()
        .get(format!("{public}f/index.html"))
        .header("sec-fetch-dest", "iframe")
        .send()
        .await
        .unwrap();
    assert_eq!(file.status(), StatusCode::OK);
    assert!(
        file.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .starts_with("sandbox ")
    );
    // No feed: nobody is signed in to read with.
    assert_eq!(
        get(&format!("{public}data/board")).await.0,
        StatusCode::NOT_FOUND
    );
    // The same link stays while on; the person's list carries it.
    let again = set_public(&served, &served.operator, &id, true).await;
    assert_eq!(again.json::<Value>().await.unwrap()["public_url"], url);
    let listed: Value = reqwest::Client::new()
        .get(format!("{}/v1/user/pages", served.base))
        .bearer_auth(&served.operator)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed[0]["public_url"], url);

    // Replacing the page keeps it, audited.
    let (_app, job) = accepted(&served, &served.operator, "Again.").await;
    published_id(
        &publish(
            &served,
            &job.mcp_token,
            json!({"kind": "dashboard", "title": "Board 2", "content": "<p>y</p>",
                   "format": "html", "replace": id}),
        )
        .await,
    );
    let replaced = get(&public).await;
    assert_eq!(replaced.0, StatusCode::OK);
    assert!(String::from_utf8_lossy(&replaced.2).contains("Board 2"));
    let records = audits(&served, "moneypenny.page.public");
    let targets: Vec<&str> = records
        .iter()
        .map(|record| record.target.as_str())
        .collect();
    assert!(
        targets.contains(&format!("{id} on, kept by a replace").as_str()),
        "{targets:?}"
    );

    // An administrator who turns public links off stops every one, and none can be made.
    put_pages(json!({"public": false}));
    assert_eq!(
        get(&public).await,
        get(&format!("{}/pages/s/AAAA/", served.base)).await
    );
    assert_eq!(
        get(&format!("{public}f/index.html")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        set_public(&served, &served.operator, &id, true)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    put_pages(json!({}));
    assert_eq!(get(&public).await.0, StatusCode::OK);

    // Turned off, it is gone; turned on again, it is a new link.
    let off = set_public(&served, &served.operator, &id, false).await;
    assert_eq!(off.json::<Value>().await.unwrap()["public"], false);
    assert_eq!(get(&public).await.0, StatusCode::NOT_FOUND);
    let renewed = set_public(&served, &served.operator, &id, true)
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_ne!(renewed["public_url"], url);
    let records = audits(&served, "moneypenny.page.public");
    assert!(
        records
            .iter()
            .any(|record| record.target == format!("{id} off"))
    );
    assert!(records.iter().all(|record| record.actor == "operator-007"));
}
