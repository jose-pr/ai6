//! Images in her chat: the app sends them with a message, the gateway keeps them by digest, her
//! conversation shows them by reference, and the Station that takes the job reads their bytes.

use base64::Engine as _;

use super::*;

/// A PNG's first bytes: enough to be one by its signature.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01";

fn encoded(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

async fn say_with(socket: &mut Socket, text: &str, images: Value) {
    let message = json!({"type": "message", "text": text, "images": images});
    socket
        .send(WsMessage::Text(message.to_string().into()))
        .await
        .unwrap();
}

async fn station_image(served: &Served, conversation: &str, sha256: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!(
            "{}/v1/station/assistant/images/{sha256}",
            served.base
        ))
        .query(&[("station", "hq"), ("conversation", conversation)])
        .bearer_auth(STATION_TOKEN)
        .send()
        .await
        .unwrap()
}

async fn user_image(served: &Served, bearer: &str, sha256: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!(
            "{}/v1/user/moneypenny/images/{sha256}",
            served.base
        ))
        .bearer_auth(bearer)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn an_image_goes_with_the_job_by_reference_and_the_station_reads_its_bytes() {
    let served = served(AssistantConfig::default()).await;
    let (mut app, _) = open(&served, &served.operator).await;
    say_with(
        &mut app,
        "What is wrong here?",
        json!([{"media_type": "image/png", "data": encoded(PNG)}]),
    )
    .await;
    let job = take(&served, "hq").await;
    let sha256 = crate::moneypenny::pages::sha256_hex(PNG);
    assert_eq!(
        job.images,
        [ai6::AssistantImage {
            sha256: sha256.clone(),
            media_type: "image/png".to_owned(),
            bytes: PNG.len() as u64,
        }]
    );
    assert!(job.message.ends_with("What is wrong here?"));
    assert!(
        !serde_json::to_string(&job).unwrap().contains(&encoded(PNG)),
        "the job names the image, it does not carry it"
    );
    // The person's entry names it too, and the conversation shows it from there.
    let frames = receive_until(&mut app, |frame| frame["type"] == "entry").await;
    let entry = &frames.last().unwrap()["entry"];
    assert_eq!(entry["role"], "person");
    assert_eq!(entry["data"]["images"][0]["sha256"], sha256);
    assert!(!entry.to_string().contains(&encoded(PNG)));

    // The Station that runs the conversation reads the bytes; another Station, or another
    // conversation, reads nothing.
    let read = station_image(&served, &job.conversation_id, &sha256).await;
    assert_eq!(read.status(), StatusCode::OK);
    assert_eq!(read.headers()["content-type"], "image/png");
    assert_eq!(read.bytes().await.unwrap().as_ref(), PNG);
    let other = reqwest::Client::new()
        .get(format!(
            "{}/v1/station/assistant/images/{sha256}",
            served.base
        ))
        .query(&[
            ("station", "worker"),
            ("conversation", &job.conversation_id),
        ])
        .bearer_auth(STATION_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        station_image(&served, "no-such-conversation", &sha256)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // The person reads it back; anyone else gets 404, an administrator included.
    let mine = user_image(&served, &served.operator, &sha256).await;
    assert_eq!(mine.status(), StatusCode::OK);
    assert_eq!(mine.headers()["x-content-type-options"], "nosniff");
    for other in [&served.admin, &served.viewer] {
        assert_eq!(
            user_image(&served, other, &sha256).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn an_image_that_is_not_one_or_too_many_is_refused_with_its_reason() {
    let served = served(AssistantConfig::default()).await;
    let (mut app, _) = open(&served, &served.operator).await;
    for (images, reason) in [
        (
            json!([{"media_type": "image/svg+xml", "data": encoded(b"<svg/>")}]),
            "not an image she takes",
        ),
        (
            json!([{"media_type": "image/jpeg", "data": encoded(PNG)}]),
            "not the image/jpeg it says",
        ),
        (
            json!([{"media_type": "image/png", "data": "not base64!"}]),
            "not base64",
        ),
        (
            Value::Array(
                (0..5)
                    .map(|_| json!({"media_type": "image/png", "data": encoded(PNG)}))
                    .collect(),
            ),
            "at most 4 images",
        ),
    ] {
        say_with(&mut app, "Look.", images).await;
        let frames = receive_until(&mut app, |frame| frame["type"] == "error").await;
        let error = frames.last().unwrap();
        assert!(
            error["message"].as_str().unwrap().contains(reason),
            "{reason}: {error}"
        );
    }
    assert!(nothing_for(&served, "hq").await, "no job was queued");
}
