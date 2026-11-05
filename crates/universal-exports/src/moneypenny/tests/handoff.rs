//! A browser that holds no page cookie gets one from the app: `POST .../open` makes a single-use
//! token, the browser's first navigation spends it, sets the cookie and goes on to the clean
//! address.

use super::viewing::get_with;
use super::*;

/// The operator's report, published.
async fn report(served: &Served) -> String {
    let (_app, job) = accepted(served, &served.operator, "A report.").await;
    published_id(
        &publish(
            served,
            &job.mcp_token,
            json!({"kind": "report", "title": "Report", "content": "<p>r</p>", "format": "html"}),
        )
        .await,
    )
}

/// The address `bearer` is given to open page `id`, on the listener it is served from.
async fn handed(served: &Served, bearer: &str, id: &str, network: Option<&str>) -> String {
    let response = open_page(served, bearer, id, network).await;
    assert_eq!(response.status(), StatusCode::OK);
    let url = response.json::<Value>().await.unwrap()["url"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(url.contains("handoff="), "{url}");
    url.replacen(
        "https://localhost/pages",
        &format!("{}/pages", served.base),
        1,
    )
}

/// The token in a handed-off address.
fn token_of(url: &str) -> &str {
    url.rsplit("handoff=").next().unwrap()
}

/// The value of the page cookie an answer sets.
fn cookie_set(answer: &reqwest::Response) -> String {
    answer.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .and_then(|pair| pair.strip_prefix(&format!("{PAGE_COOKIE}=")))
        .unwrap()
        .to_owned()
}

/// A browser's navigation to `url`, holding no cookie.
async fn navigate(url: &str) -> reqwest::Response {
    get_with(url, None, &[("sec-fetch-dest", "document")]).await
}

#[tokio::test]
async fn the_first_navigation_spends_it_sets_the_cookie_and_cleans_the_address() {
    let served = served(AssistantConfig::default()).await;
    let id = report(&served).await;
    let url = handed(&served, &served.operator, &id, Some("none")).await;
    assert_eq!(audits(&served, "moneypenny.page.handoff").len(), 1);
    assert!(audits(&served, "moneypenny.page.handoff_used").is_empty());
    let answer = navigate(&url).await;
    assert_eq!(answer.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        answer.headers()["location"],
        format!("https://localhost/pages/p/{id}/?network=none").as_str(),
        "the token leaves the address bar"
    );
    assert_eq!(answer.headers()["cache-control"], "no-store");
    assert_eq!(answer.headers()["referrer-policy"], "no-referrer");
    let set = answer.headers()["set-cookie"].to_str().unwrap().to_owned();
    let attributes: Vec<&str> = set.split("; ").skip(1).collect();
    assert_eq!(
        attributes[..4],
        ["Path=/pages/", "HttpOnly", "Secure", "SameSite=None"],
        "{set}"
    );
    assert!(attributes[4].starts_with("Max-Age="), "{set}");
    assert_eq!(audits(&served, "moneypenny.page.handoff_used").len(), 1);
    // The cookie opens the page by itself from here on.
    let cookie = cookie_set(&answer);
    let frame = format!("{}/pages/p/{id}/", served.base);
    assert_eq!(
        get_with(&frame, Some(&cookie), &[]).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_second_use_an_ended_one_and_another_page_s_are_an_unknown_page() {
    let roster = pages_roster(json!({"handoff_ttl": 0.3}));
    let served = served_with(AssistantConfig::default(), Some(roster)).await;
    let id = report(&served).await;
    let other = report(&served).await;
    let answer = |response: reqwest::Response| async move {
        (response.status(), response.text().await.unwrap())
    };
    let missing = answer(navigate(&format!("{}/pages/p/page-nope/", served.base)).await).await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);

    // A second use, with the cookie the first set or without.
    let url = handed(&served, &served.operator, &id, None).await;
    let first = navigate(&url).await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    let cookie = cookie_set(&first);
    assert_eq!(answer(navigate(&url).await).await, missing);
    assert_eq!(
        answer(get_with(&url, Some(&cookie), &[]).await).await,
        missing
    );
    assert_eq!(audits(&served, "moneypenny.page.handoff_used").len(), 1);

    // Another page's: refused there, and spent by being tried.
    let url = handed(&served, &served.operator, &id, None).await;
    let elsewhere = url.replace(&format!("/p/{id}/"), &format!("/p/{other}/"));
    assert_eq!(answer(navigate(&elsewhere).await).await, missing);
    assert_eq!(answer(navigate(&url).await).await, missing);

    // One that has ended.
    let url = handed(&served, &served.operator, &id, None).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(answer(navigate(&url).await).await, missing);

    // A token nobody made.
    let made_up = format!("{}/pages/p/{id}/?handoff={}", served.base, "a".repeat(64));
    assert_eq!(answer(navigate(&made_up).await).await, missing);
    assert_eq!(audits(&served, "moneypenny.page.handoff_used").len(), 1);
}

#[tokio::test]
async fn it_stands_for_the_sign_in_and_the_page_as_they_are_when_it_is_used() {
    let served = served(AssistantConfig::default()).await;
    let id = report(&served).await;
    // The sign-in ended between the two.
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
    let url = handed(&served, &bearer, &id, None).await;
    let out = reqwest::Client::new()
        .post(format!("{}/v1/auth/logout", served.base))
        .bearer_auth(&bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    assert_eq!(navigate(&url).await.status(), StatusCode::NOT_FOUND);
    // The viewer lost the page between the two.
    let share = |subjects: Value| {
        reqwest::Client::new()
            .put(format!("{}/v1/user/pages/{id}/shares", served.base))
            .bearer_auth(&served.operator)
            .json(&json!({"subjects": subjects}))
            .send()
    };
    assert_eq!(
        share(json!(["person:viewer-007"])).await.unwrap().status(),
        StatusCode::OK
    );
    let url = handed(&served, &served.viewer, &id, None).await;
    assert_eq!(share(json!([])).await.unwrap().status(), StatusCode::OK);
    assert_eq!(navigate(&url).await.status(), StatusCode::NOT_FOUND);
    // Nobody the page is hidden from is given one.
    assert_eq!(
        open_page(&served, &served.viewer, &id, None).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn only_a_navigation_spends_it() {
    let served = served(AssistantConfig::default()).await;
    let id = report(&served).await;
    let url = handed(&served, &served.operator, &id, None).await;
    for (name, value) in [("sec-fetch-dest", "image"), ("sec-purpose", "prefetch")] {
        let answer = get_with(&url, None, &[(name, value)]).await;
        assert_eq!(answer.status(), StatusCode::NOT_FOUND, "{name}");
        assert!(answer.headers().get("set-cookie").is_none(), "{name}");
    }
    let head = reqwest::Client::new().head(&url).send().await.unwrap();
    assert_eq!(head.status(), StatusCode::NOT_FOUND);
    assert!(head.headers().get("set-cookie").is_none());
    assert_eq!(navigate(&url).await.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn a_person_holds_as_many_as_the_setting_says_and_may_ask_for_none() {
    let roster = pages_roster(json!({"handoffs": 1}));
    let served = served_with(AssistantConfig::default(), Some(roster)).await;
    let id = report(&served).await;
    let older = handed(&served, &served.operator, &id, None).await;
    let newer = handed(&served, &served.operator, &id, None).await;
    assert_eq!(navigate(&older).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(navigate(&newer).await.status(), StatusCode::SEE_OTHER);
    // The plain address, for a browser that holds the cookie: no token, nothing to audit.
    let audited = audits(&served, "moneypenny.page.handoff").len();
    let plain = reqwest::Client::new()
        .post(format!("{}/v1/user/pages/{id}/open", served.base))
        .bearer_auth(&served.operator)
        .json(&json!({"handoff": false}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        plain["url"],
        format!("https://localhost/pages/p/{id}/").as_str()
    );
    assert_eq!(audits(&served, "moneypenny.page.handoff").len(), audited);
}

#[tokio::test]
async fn on_the_pages_own_origin_it_sets_the_cookie_a_sign_in_cannot() {
    let served = served_full(AssistantConfig::default(), None, true).await;
    let pages_base = served.pages_server.as_ref().unwrap().0.clone();
    let id = report(&served).await;
    let url = handed(&served, &served.operator, &id, None).await;
    assert!(url.starts_with(&format!("{pages_base}/p/{id}/")), "{url}");
    let answer = navigate(&url).await;
    assert_eq!(answer.status(), StatusCode::SEE_OTHER);
    assert!(
        answer.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("; Path=/; "),
        "{:?}",
        answer.headers()
    );
    let cookie = cookie_set(&answer);
    let frame = format!("{pages_base}/p/{id}/");
    assert_eq!(navigate(&frame).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        get_with(&frame, Some(&cookie), &[]).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn its_token_is_never_logged_audited_or_answered_back() {
    let served = served(AssistantConfig::default()).await;
    let id = report(&served).await;
    let other = report(&served).await;
    let good = handed(&served, &served.operator, &id, None).await;
    let elsewhere = handed(&served, &served.operator, &id, None).await;
    let spent = handed(&served, &served.operator, &id, None).await;
    let mut answers = Vec::new();
    let mut ask = async |url: String| {
        let response = navigate(&url).await;
        let headers = format!("{:?}", response.headers());
        answers.push(format!("{headers} {}", response.text().await.unwrap()));
    };
    ask(good.clone()).await;
    ask(good.clone()).await;
    ask(elsewhere.replace(&format!("/p/{id}/"), &format!("/p/{other}/"))).await;
    ask(spent.replace(&format!("/p/{id}/"), "/p/page-nope/")).await;
    ask(spent.clone()).await;
    let logged = served.gateway.pages.clone().unwrap().logged();
    assert!(!logged.is_empty(), "the refusals were logged");
    let audited = serde_json::to_string(&served.registry.list_audit(500).unwrap()).unwrap();
    for secret in [token_of(&good), token_of(&elsewhere), token_of(&spent)] {
        for (what, text) in answers
            .iter()
            .map(|answer| ("an answer", answer.as_str()))
            .chain(logged.iter().map(|line| ("the log", line.as_str())))
            .chain([("the audit", audited.as_str())])
        {
            assert!(!text.contains(secret), "{what} holds the token: {text}");
        }
    }
}
