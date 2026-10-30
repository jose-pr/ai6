//! Moneypenny's page tools (`publish_page`, `unpublish_page`, `pages`), the Stations' upload
//! routes and the person's own routes to their pages.
//!
//! Audit comes first: `moneypenny.page.published` is recorded before the page exists, so no
//! audit means no page.

use std::collections::BTreeMap;
use std::time::Duration;

use ai6::{AssistantPageFile, AssistantPageLimits, PageNetwork, page_call_digest};
use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use super::pages::{self, Feed, Kind, Page, PageStore, Serve, classify};
use super::store::{Entry, Role};
use super::tools::{self, Outcome, check, object, text};
use super::{Assistant, Granted, assistant_settings, audit, cut, now};
use crate::{ApiError, Gateway, bearer, resolve_user};

use super::pages::RESOURCE;

pub(crate) const PUBLISH: &str = "publish_page";
pub(crate) const UNPUBLISH: &str = "unpublish_page";
pub(crate) const LIST: &str = "pages";

pub(crate) const PUBLISH_DESCRIPTION: &str = "Publish a page the person opens in a browser tab \
    outside the app. kind is dashboard (reads live data through feeds, each a read tool with \
    fixed input, run with the viewer's own permissions), report (a snapshot of data you put in \
    the files; make one only when the person asks for a report) or file (one file shown by its \
    format: a log, CSV, Markdown, an image). Give path (a file or a directory under personal/ \
    or scratch/; a bundle's entry is index.html unless entry names another) or content (one \
    inline text file of the given format). replace takes your earlier page's id. A page lasts \
    a day unless the person remembers it. Say what you made and that the person opens it from \
    its card; a page made after you read text from outside AI6 is marked for the person.";

pub(crate) const UNPUBLISH_DESCRIPTION: &str = "Delete a page you published, by its id.";
pub(crate) const LIST_DESCRIPTION: &str = "List the person's pages: id, kind, title, size, \
    expiry and whether each was made after outside text.";

/// The input of `publish_page`.
pub(crate) fn publish_schema() -> Value {
    object(
        &[
            (
                "kind",
                json!({"type": "string", "enum": ["dashboard", "report", "file"]}),
            ),
            ("title", text("What the page is, in a few words.")),
            (
                "path",
                text("A file or directory under personal/ or scratch/."),
            ),
            ("content", text("One inline text file, instead of a path.")),
            (
                "format",
                json!({"type": "string",
                       "enum": ["html", "markdown", "text", "csv", "json", "svg"]}),
            ),
            ("entry", text("A bundle's first page (default index.html).")),
            (
                "replace",
                text("The id of your earlier page this one replaces."),
            ),
            (
                "feeds",
                json!({"type": "array", "items": object(
                    &[
                        ("name", text("What the page asks for as data/NAME.")),
                        ("tool", text("A read tool.")),
                        ("input", json!({"type": "object", "description": "Its fixed input."})),
                    ],
                    &["name", "tool", "input"],
                )}),
            ),
        ],
        &["kind", "title"],
    )
}

pub(crate) fn unpublish_schema() -> Value {
    object(&[("id", text("The page's id."))], &["id"])
}

pub(crate) fn list_schema() -> Value {
    object(&[], &[])
}

fn refused(reason: impl Into<String>) -> Outcome {
    Outcome::error(format!("Nothing was published: {}", reason.into()))
}

impl Assistant {
    /// Runs `publish_page`: checks the input against the quotas, takes the files (inline, or
    /// from the Station's upload), audits, then stores the page and shows the person its card.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn publish_page(
        &self,
        gateway: &Gateway,
        granted: &Granted,
        input: &Value,
    ) -> Outcome {
        let Some(store) = gateway.pages.clone() else {
            return refused("this gateway keeps no pages");
        };
        if let Err(problem) = check(&publish_schema(), input, "input") {
            return Outcome::error(format!("INVALID_INPUT: {problem}"));
        }
        if let Err(problem) = super::actions::hidden_characters(input, "input") {
            return Outcome::error(format!("INVALID_INPUT: {problem}"));
        }
        let Ok(settings) = assistant_settings(gateway).await else {
            return refused("the roster could not be read");
        };
        let limits = settings.pages().clone();
        let Ok(person) = resolve_user(gateway, &granted.bearer).await else {
            return refused("the person's sign-in has ended");
        };
        let field = |name: &str| input.get(name).and_then(Value::as_str);
        let Some(kind) = field("kind").and_then(Kind::parse) else {
            return refused("kind is dashboard, report or file");
        };
        let title = field("title").unwrap_or_default().trim().to_owned();
        if title.is_empty()
            || !limits.title_chars.allows(title.chars().count())
            || title.chars().any(char::is_control)
        {
            return refused(format!(
                "the title is 1 to {} characters without control characters",
                limits.title_chars
            ));
        }
        let feeds: Vec<Feed> = match input.get("feeds") {
            None => Vec::new(),
            Some(value) => match serde_json::from_value(value.clone()) {
                Ok(feeds) => feeds,
                Err(_) => return refused("feeds is a list of {name, tool, input}"),
            },
        };
        if kind != Kind::Dashboard && !feeds.is_empty() {
            return refused(format!(
                "a {} has no feeds; only a dashboard reads live data",
                kind.as_str()
            ));
        }
        if !feeds.is_empty()
            && let Err(reason) = pages::check_feeds(&feeds, &limits)
        {
            return refused(reason);
        }
        // The page this one replaces keeps its id, shares and card.
        let previous = match field("replace") {
            None => None,
            Some(id) => match store.get(id) {
                Some(page) if page.person_id == person.id => Some(page),
                _ => return refused(format!("there is no page {id} of yours to replace")),
            },
        };
        // The files.
        let (files, outside): (BTreeMap<String, Vec<u8>>, Vec<String>) =
            match (field("path"), field("content")) {
                (Some(path), None) => {
                    let digest = page_call_digest(path);
                    match store
                        .claim(&granted.conversation_id, &digest, limits.upload_wait.0)
                        .await
                    {
                        Ok((files, manifest)) => {
                            let outside = manifest
                                .iter()
                                .filter(|file| file.outside_text)
                                .map(|file| format!("file:{}", file.path))
                                .collect();
                            (files, outside)
                        }
                        Err(reason) => return refused(reason),
                    }
                }
                (None, Some(content)) => {
                    let Some(name) = field("format").and_then(pages::inline_name) else {
                        return refused(
                            "content needs a format: html, markdown, text, csv, json or svg",
                        );
                    };
                    if !limits.inline_bytes.allows(content.len()) {
                        return refused(format!(
                            "inline content is at most {} bytes; write a file and give its path",
                            limits.inline_bytes
                        ));
                    }
                    (
                        BTreeMap::from([(name.to_owned(), content.as_bytes().to_vec())]),
                        Vec::new(),
                    )
                }
                _ => return refused("give a path or content, not both and not neither"),
            };
        for (path, bytes) in &files {
            if matches!(
                classify(path),
                Serve::Html | Serve::Markdown | Serve::Text(_)
            ) && std::str::from_utf8(bytes).is_err()
            {
                return refused(format!("{path} is not UTF-8 text"));
            }
        }
        if kind == Kind::File && files.len() != 1 {
            return refused("a file page is one file; publish a dashboard or report for a bundle");
        }
        let entry = match field("entry") {
            Some(entry) => entry.to_owned(),
            None if files.contains_key("index.html") => "index.html".to_owned(),
            None if files.len() == 1 => files.keys().next().cloned().unwrap_or_default(),
            None => return refused("a bundle needs an index.html, or entry naming its first page"),
        };
        if !files.contains_key(&entry) {
            return refused(format!("the entry {entry} is not one of the files"));
        }
        // What she had read, as the bar will say.
        let (exposed_now, mut exposed_by) = {
            let mut desk = self.desk();
            match desk
                .store
                .owned(&granted.conversation_id, &granted.person_id)
            {
                Some(conversation) => (conversation.exposed, conversation.exposed_by.clone()),
                None => return refused("the conversation is gone"),
            }
        };
        for source in &outside {
            if !exposed_by.contains(source)
                && self
                    .config
                    .gateway
                    .exposed_sources
                    .allows(exposed_by.len() + 1)
            {
                exposed_by.push(cut(source, self.config.gateway.source_bytes.or_max()));
            }
        }
        let at = now();
        let id = previous
            .as_ref()
            .map_or_else(PageStore::new_id, |page| page.id.clone());
        let page = Page {
            id,
            person_id: person.id.clone(),
            conversation_id: granted.conversation_id.clone(),
            kind,
            title,
            entry,
            files: files
                .iter()
                .map(|(path, bytes)| pages::FileMeta {
                    path: path.clone(),
                    bytes: bytes.len() as u64,
                    sha256: pages::sha256_hex(bytes),
                })
                .collect(),
            bytes: files.values().map(|bytes| bytes.len() as u64).sum(),
            created_at: at,
            expires_at: Some(at + limits.default_ttl.0.as_secs_f64()),
            exposed: exposed_now || !outside.is_empty(),
            exposed_by,
            feeds,
            shares: previous.map(|page| page.shares).unwrap_or_default(),
        };
        let prepared = match store.prepare(page, &files, &limits) {
            Ok(prepared) => prepared,
            Err(reason) => return refused(reason),
        };
        let target = format!(
            "{} {} {} bytes, conversation {}, exposed: {}",
            prepared.page.id,
            prepared.page.kind.as_str(),
            prepared.page.bytes,
            granted.conversation_id,
            prepared.page.exposed
        );
        if let Err(reason) = audit(
            gateway,
            &person,
            "moneypenny.page.published",
            String::new(),
            Some(RESOURCE.to_owned()),
            &target,
        )
        .await
        {
            store.discard(prepared);
            return refused(reason);
        }
        let page = match store.commit(prepared) {
            Ok(page) => page,
            Err(error) => return refused(format!("the page could not be stored: {error}")),
        };
        let max_entries = self.config.bounds();
        {
            let mut desk = self.desk();
            if let Some(conversation) = desk
                .store
                .owned(&granted.conversation_id, &granted.person_id)
            {
                conversation.push(
                    Entry {
                        role: Role::Page,
                        text: page.title.clone(),
                        at: now(),
                        data: Some(page.summary()),
                    },
                    max_entries,
                );
            }
        }
        Outcome {
            text: format!(
                "Published as {}: {}. The person opens it from its card in the app{}.",
                page.id,
                page.title,
                match page.expires_at {
                    Some(_) => "; it is kept for a day unless they remember it".to_owned(),
                    None => String::new(),
                }
            ),
            is_error: false,
        }
    }

    /// Runs `unpublish_page`.
    pub(crate) async fn unpublish_page(
        &self,
        gateway: &Gateway,
        granted: &Granted,
        input: &Value,
    ) -> Outcome {
        let Some(store) = gateway.pages.clone() else {
            return Outcome::error("This gateway keeps no pages.");
        };
        if let Err(problem) = check(&unpublish_schema(), input, "input") {
            return Outcome::error(format!("INVALID_INPUT: {problem}"));
        }
        let id = input["id"].as_str().unwrap_or_default();
        let Ok(person) = resolve_user(gateway, &granted.bearer).await else {
            return Outcome::error("The person's sign-in has ended.");
        };
        match store.get(id) {
            Some(page) if page.person_id == person.id => {}
            _ => return Outcome::error(format!("There is no page {id} of the person's.")),
        }
        if let Err(reason) = audit(
            gateway,
            &person,
            "moneypenny.page.deleted",
            String::new(),
            Some(RESOURCE.to_owned()),
            &format!("{id} by Moneypenny"),
        )
        .await
        {
            return Outcome::error(reason);
        }
        store.delete(id);
        Outcome {
            text: format!("Deleted {id}."),
            is_error: false,
        }
    }

    /// Runs `pages`: the person's pages. A listed page that was made after outside text
    /// exposes the conversation.
    pub(crate) async fn list_pages(&self, gateway: &Gateway, granted: &Granted) -> Outcome {
        let Some(store) = gateway.pages.clone() else {
            return Outcome::error("This gateway keeps no pages.");
        };
        let pages = store.of(&granted.person_id);
        if let Some(page) = pages.iter().find(|page| page.exposed) {
            let mut desk = self.desk();
            if let Some(conversation) = desk.store.conversations.get_mut(&granted.conversation_id) {
                conversation.expose_by(&format!("page:{}", page.id), &self.config.gateway);
            }
        }
        let list: Vec<Value> = pages.iter().map(Page::summary).collect();
        Outcome {
            text: tools::wrap(
                "pages",
                &Value::Array(list),
                self.config.gateway.result_chars,
            ),
            is_error: false,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Stations
// ---------------------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct UploadQuery {
    station: String,
    conversation: String,
}

/// `PUT /v1/station/assistant/pages/{digest}/{path}?station=&conversation=`: one file of the
/// page the Station is publishing for that call; the body is the file's bytes, held to
/// `[assistant.pages] max_file_bytes` on this route only.
pub(crate) async fn upload(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    UrlPath((digest, path)): UrlPath<(String, String)>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    let assistant = gateway.assistant.clone().ok_or_else(ApiError::not_found)?;
    let store = gateway.pages.clone().ok_or_else(ApiError::not_found)?;
    assistant.run_by(&query.station, &query.conversation, None)?;
    let limits = page_limits(&gateway).await?;
    let bytes = axum::body::to_bytes(body, limits.max_file_bytes.or_max())
        .await
        .map_err(|_| ApiError::payload_too_large())?;
    store
        .stage_file(
            &query.station,
            &query.conversation,
            &digest,
            &path,
            bytes.to_vec(),
            &limits,
        )
        .map_err(|_| ApiError::bad_request())?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Done {
    files: Option<Vec<AssistantPageFile>>,
    refused: Option<String>,
}

/// `POST /v1/station/assistant/pages/{digest}/done?station=&conversation=`: the manifest of
/// what was sent, or why nothing was.
pub(crate) async fn done(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    UrlPath(digest): UrlPath<String>,
    Query(query): Query<UploadQuery>,
    Json(done): Json<Done>,
) -> Result<StatusCode, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    let assistant = gateway.assistant.clone().ok_or_else(ApiError::not_found)?;
    let store = gateway.pages.clone().ok_or_else(ApiError::not_found)?;
    assistant.run_by(&query.station, &query.conversation, None)?;
    let limits = page_limits(&gateway).await?;
    let outcome = match (done.files, done.refused) {
        (Some(files), None) => Ok(files),
        (None, Some(reason)) => Err(cut(&reason, 512)),
        _ => return Err(ApiError::bad_request()),
    };
    store
        .stage_done(
            &query.station,
            &query.conversation,
            &digest,
            outcome,
            &limits,
        )
        .map_err(|_| ApiError::bad_request())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn page_limits(gateway: &Gateway) -> Result<AssistantPageLimits, ApiError> {
    assistant_settings(gateway)
        .await
        .map(|settings| settings.pages().clone())
        .map_err(|_| ApiError::unavailable())
}

// ---------------------------------------------------------------------------------------
// The person
// ---------------------------------------------------------------------------------------

/// `GET /v1/user/pages`: the person's own pages, newest first.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let store = gateway.pages.clone().ok_or_else(ApiError::not_found)?;
    Ok(Json(Value::Array(
        store.of(&person.id).iter().map(Page::summary).collect(),
    )))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct OpenRequest {
    network: Option<String>,
}

/// `POST /v1/user/pages/{id}/open {network?}`: a link to the page for this sign-in, valid
/// `open_ttl` or until the page goes: `{url, expires_at}`. Only the person whose page it is
/// may ask; any other gets 404, an administrator included.
pub(crate) async fn open(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    request: Option<Json<OpenRequest>>,
) -> Result<Json<Value>, ApiError> {
    let token = bearer(&headers)?;
    let person = resolve_user(&gateway, token).await?;
    let store = gateway.pages.clone().ok_or_else(ApiError::not_found)?;
    let page = store.get(&id).ok_or_else(ApiError::not_found)?;
    if !pages::may_view(&gateway, &page, &person.id).await {
        return Err(ApiError::not_found());
    }
    let asked = match request.and_then(|Json(request)| request.network).as_deref() {
        None => None,
        Some("none") => Some(PageNetwork::None),
        Some("own") => Some(PageNetwork::Own),
        Some("open") => Some(PageNetwork::Open),
        Some(_) => return Err(ApiError::bad_request()),
    };
    let limits = page_limits(&gateway).await?;
    let network = pages::profile(&limits, asked);
    let (link, expires) = store.mint(
        &page,
        &person.id,
        token,
        &crate::access_token_digest(token),
        network,
        limits.open_ttl.0,
    );
    Ok(Json(json!({
        "url": format!("{}/v/{link}/", pages::pages_base(&gateway)),
        "expires_at": expires,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RememberRequest {
    remember: bool,
}

/// `POST /v1/user/pages/{id}/remember {remember}`: keeps the page with no expiry, or lets it
/// expire a day from now. Only the page's person.
pub(crate) async fn remember(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(request): Json<RememberRequest>,
) -> Result<Json<Value>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let store = gateway.pages.clone().ok_or_else(ApiError::not_found)?;
    match store.get(&id) {
        Some(page) if page.person_id == person.id => {}
        _ => return Err(ApiError::not_found()),
    }
    let limits = page_limits(&gateway).await?;
    let action = if request.remember {
        "moneypenny.page.remembered"
    } else {
        "moneypenny.page.forgotten"
    };
    audit(
        &gateway,
        &person,
        action,
        String::new(),
        Some(RESOURCE.to_owned()),
        &id,
    )
    .await
    .map_err(|_| ApiError::internal())?;
    let page = store
        .remember(&id, request.remember, limits.default_ttl.0)
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(page.summary()))
}

/// `DELETE /v1/user/pages/{id}`: deletes the page. Only the page's person.
pub(crate) async fn delete(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let store = gateway.pages.clone().ok_or_else(ApiError::not_found)?;
    match store.get(&id) {
        Some(page) if page.person_id == person.id => {}
        _ => return Err(ApiError::not_found()),
    }
    audit(
        &gateway,
        &person,
        "moneypenny.page.deleted",
        String::new(),
        Some(RESOURCE.to_owned()),
        &format!("{id} by the person"),
    )
    .await
    .map_err(|_| ApiError::internal())?;
    store.delete(&id);
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------------------
// The sweeper
// ---------------------------------------------------------------------------------------

/// Deletes the expired pages, expired links and stale uploads once, auditing each page as
/// `moneypenny.page.expired` by its person; answers how many pages went.
pub(crate) async fn sweep_once(gateway: &Gateway) -> usize {
    let Some(store) = gateway.pages.clone() else {
        return 0;
    };
    let stage = assistant_settings(gateway)
        .await
        .map(|settings| settings.pages().stage_ttl.0)
        .unwrap_or(Duration::from_secs(300));
    let gone = store.sweep(now(), stage);
    for page in &gone {
        let person = ai6::Person {
            id: page.person_id.clone(),
            name: page.person_id.clone(),
            ..ai6::Person::default()
        };
        let _ = audit(
            gateway,
            &person,
            "moneypenny.page.expired",
            String::new(),
            Some(RESOURCE.to_owned()),
            &page.id,
        )
        .await;
    }
    gone.len()
}

/// Sweeps every `[assistant.pages] sweep_every`, for as long as the gateway lives.
pub(crate) async fn sweeper(gateway: Gateway) {
    loop {
        let every = assistant_settings(&gateway)
            .await
            .map(|settings| settings.pages().sweep_every.0)
            .unwrap_or(Duration::from_secs(60));
        tokio::time::sleep(every).await;
        sweep_once(&gateway).await;
    }
}
