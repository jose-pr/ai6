//! Pages Moneypenny publishes and how they are served.
//!
//! A page is what she makes for the person to open in a browser tab outside Outer Office: a
//! dashboard reading live data, a report (a snapshot) or one file. The gateway keeps pages in
//! a [`PageStore`] (a directory, so remembered pages outlive a restart), serves each at its own
//! address (`{base}/p/{id}/`) to its person and whom they share it with, every request naming its
//! viewer by the gateway's page authenticator or the page cookie, or to anyone through a public
//! link (`{base}/s/{token}/`) the person makes, and never lets a page act as the app: every page
//! response carries a CSP `sandbox` without `allow-same-origin`, so the page is an opaque
//! origin, and `/v1` answers no cross-origin request. A page is shown in the gateway's own
//! frame under a classification bar that says who made it and when it expires.
//!
//! Quotas and options are `[assistant.pages]` in the roster's `assistant` record, read at each
//! use, so an administrator's change applies to the next publish.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ai6::{AssistantPageFile, AssistantPageLimits, PageNetwork, Person};
use axum::Extension;
use axum::extract::{ConnectInfo, Path as UrlPath, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION, ORIGIN,
    REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use coqui_peer::PeerId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::sync::Notify;

use super::tools;
use crate::Gateway;
use crate::page_access::{self, Connection, NoViewer};
use crate::tls::ClientCertificates;

// ---------------------------------------------------------------------------------------
// What a page is
// ---------------------------------------------------------------------------------------

/// The scope every page audit record is made under.
pub(crate) const RESOURCE: &str = "config:assistant";

/// Where a page's top-level files read its feeds, `data/NAME`: no file of a page is under it.
pub(crate) const FEEDS_DIR: &str = "data/";

/// What she made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    /// A page that reads live data through its feeds.
    Dashboard,
    /// A snapshot of data she put in the files, made when the person asks for one.
    Report,
    /// One file shown by its format.
    File,
}

impl Kind {
    pub(crate) fn parse(word: &str) -> Option<Self> {
        match word {
            "dashboard" => Some(Self::Dashboard),
            "report" => Some(Self::Report),
            "file" => Some(Self::File),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Report => "report",
            Self::File => "file",
        }
    }
}

/// A live read a dashboard declares: the page asks for `data/{name}` and the gateway runs
/// the read tool `tool` with the fixed `input` as the page's viewer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Feed {
    pub(crate) name: String,
    pub(crate) tool: String,
    pub(crate) input: Value,
}

/// One stored file of a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileMeta {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

/// A page's record, `{id}/meta.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Page {
    pub(crate) id: String,
    pub(crate) person_id: String,
    pub(crate) conversation_id: String,
    pub(crate) kind: Kind,
    pub(crate) title: String,
    /// The first file shown.
    pub(crate) entry: String,
    pub(crate) files: Vec<FileMeta>,
    pub(crate) bytes: u64,
    pub(crate) created_at: f64,
    /// When it goes; `None` once the person remembers it.
    pub(crate) expires_at: Option<f64>,
    pub(crate) feeds: Vec<Feed>,
    /// Subjects the person shares it with.
    #[serde(default)]
    pub(crate) shares: Vec<String>,
    /// The token of its public link (`{base}/s/{token}/`), while the person keeps one. It sits
    /// beside the files it opens, so keeping it here exposes nothing they do not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) public: Option<String>,
}

impl Page {
    /// How the app and her `pages` tool list it to its person, with whom it is shared.
    pub(crate) fn summary(&self) -> Value {
        let mut summary = self.common();
        summary["shares"] = json!(self.shares);
        summary["public"] = json!(self.public.is_some());
        summary
    }

    /// [`summary`](Self::summary) with the address of its public link, under `base`.
    pub(crate) fn summary_with_link(&self, base: &str) -> Value {
        let mut summary = self.summary();
        if let Some(token) = &self.public {
            summary["public_url"] = json!(format!("{base}/s/{token}/"));
        }
        summary
    }

    /// How it is listed to `viewer`: its person sees [`summary`](Self::summary); anyone it is
    /// shared with sees whose it is, and not whom else it is shared with.
    pub(crate) fn summary_for(&self, viewer: &str) -> Value {
        if viewer == self.person_id {
            return self.summary();
        }
        let mut summary = self.common();
        summary["shared_by"] = json!(self.person_id);
        summary
    }

    fn common(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind.as_str(),
            "title": self.title,
            "bytes": self.bytes,
            "expires_at": self.expires_at,
            "feeds": self.feeds.iter().map(|feed| feed.name.as_str()).collect::<Vec<_>>(),
        })
    }
}

/// A share's kind and ID: `person:`, `group:` or `account:` (as the access policy names people),
/// `compartment:{id}` (the people read into it) or `operation:{id}` (the people who hold
/// `mission.read` there). The ID is 1 to 256 bytes without control characters.
pub(crate) fn share_subject(text: &str) -> Option<(&str, &str)> {
    let (kind, id) = text.split_once(':')?;
    let known = matches!(
        kind,
        "person" | "group" | "account" | "compartment" | "operation"
    );
    (known && !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        .then_some((kind, id))
}

// ---------------------------------------------------------------------------------------
// Formats
// ---------------------------------------------------------------------------------------

/// How a file is served, from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Serve {
    Html,
    Markdown,
    /// UTF-8 text with this content type.
    Text(&'static str),
    /// A bundle asset with this content type.
    Asset(&'static str),
    /// Anything else: offered as a download, never rendered.
    Attachment,
}

/// The one table of formats.
pub(crate) fn classify(path: &str) -> Serve {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "html" | "htm" => Serve::Html,
        "md" | "markdown" => Serve::Markdown,
        "txt" | "log" => Serve::Text("text/plain; charset=utf-8"),
        "csv" => Serve::Text("text/csv; charset=utf-8"),
        "json" => Serve::Text("application/json; charset=utf-8"),
        "svg" => Serve::Asset("image/svg+xml"),
        "js" | "mjs" => Serve::Asset("text/javascript; charset=utf-8"),
        "css" => Serve::Asset("text/css; charset=utf-8"),
        "png" => Serve::Asset("image/png"),
        "jpg" | "jpeg" => Serve::Asset("image/jpeg"),
        "gif" => Serve::Asset("image/gif"),
        "webp" => Serve::Asset("image/webp"),
        "ico" => Serve::Asset("image/x-icon"),
        "woff2" => Serve::Asset("font/woff2"),
        "wasm" => Serve::Asset("application/wasm"),
        _ => Serve::Attachment,
    }
}

/// The inline formats of `publish_page`'s `content`, with the file name each becomes.
pub(crate) fn inline_name(format: &str) -> Option<&'static str> {
    match format {
        "html" => Some("index.html"),
        "markdown" => Some("index.md"),
        "text" => Some("page.txt"),
        "csv" => Some("page.csv"),
        "json" => Some("page.json"),
        "svg" => Some("page.svg"),
        _ => None,
    }
}

/// A page path as stored: `/`-separated, no empty part or hidden name (so no `.` or `..`),
/// no backslash or control
/// character, at most 512 bytes and 255 to a part.
pub(crate) fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && part.len() <= 255)
}

/// Markdown rendered as a page: tables, strikethrough and task lists; raw HTML is kept, the
/// result being served as any HTML page is.
pub(crate) fn markdown_page(title: &str, source: &str) -> String {
    use pulldown_cmark::{Options, Parser, html};
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut body = String::new();
    html::push_html(&mut body, Parser::new_ext(source, options));
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title>\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style>{MARKDOWN_STYLE}</style>\
         </head><body><main>{body}</main></body></html>",
        escape(title)
    )
}

const MARKDOWN_STYLE: &str = "body{font:16px/1.55 system-ui,sans-serif;margin:0;color:#1d1d1f;\
background:#fff}main{max-width:50rem;margin:0 auto;padding:1.5rem}table{border-collapse:collapse}\
td,th{border:1px solid #ccc;padding:.3rem .6rem}pre,code{background:#f4f4f6}pre{padding:.8rem;\
overflow:auto}img{max-width:100%}blockquote{margin-left:0;padding-left:1rem;border-left:4px solid #ccc}";

/// `text` safe inside HTML text and quoted attributes.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// `seconds` (Unix time) as `2026-11-01 12:30 UTC`.
pub(crate) fn utc_text(seconds: f64) -> String {
    // Past the years `time` represents (+-9999) the text shows the last one.
    let at = time::OffsetDateTime::from_unix_timestamp(seconds.max(0.0) as i64).unwrap_or(
        time::OffsetDateTime::new_utc(time::Date::MAX, time::Time::MIDNIGHT),
    );
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute()
    )
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    PeerId::of(bytes).hex()
}

/// `bytes` as unpadded base64url.
fn base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// 32 random bytes.
fn random_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes
}

/// 32 random bytes as base64url: a capability token or a page ID suffix.
fn random_token() -> String {
    base64url(&random_bytes())
}

/// The HMAC-SHA256 under `key` of `parts`, each ended by a NUL so no two lists run together.
fn keyed(key: &[u8], parts: &[&str]) -> Vec<u8> {
    use hmac::{KeyInit, Mac};
    let Ok(mut mac) = hmac::Hmac::<Sha256>::new_from_slice(key) else {
        unreachable!("HMAC takes a key of any length")
    };
    for part in parts {
        mac.update(part.as_bytes());
        mac.update(&[0]);
    }
    mac.finalize().into_bytes().to_vec()
}

/// The key at `path`, made of 32 random bytes and written there (owner-only on Unix) when there
/// is none.
fn read_or_make_key(path: &std::path::Path) -> io::Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(key) if key.len() == 32 => return Ok(key),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not a key of 32 bytes", path.display()),
            ));
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        Err(_) => {}
    }
    let key = random_bytes();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            use std::io::Write as _;
            file.write_all(&key)?;
            file.sync_all()?;
            Ok(key.to_vec())
        }
        // Another gateway on the same directory made it first.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => std::fs::read(path),
        Err(error) => Err(error),
    }
}

fn now() -> f64 {
    crate::unix_timestamp().unwrap_or(0.0)
}

// ---------------------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------------------

/// The key the page cookies and view tokens are made with, in the pages directory: hidden, so no
/// page is ever read from it.
const KEY_FILE: &str = ".key";

/// Files a Station sent for a `publish_page` call, until the call takes them.
struct Staged {
    station: String,
    files: BTreeMap<String, Vec<u8>>,
    bytes: u64,
    at: Instant,
    done: Option<Result<Vec<AssistantPageFile>, String>>,
}

#[derive(Default)]
struct Inner {
    pages: BTreeMap<String, Page>,
    /// Staged uploads by (conversation, call digest).
    staged: HashMap<(String, String), Staged>,
    /// Feed reads in the current minute, by [`viewing`] key.
    reads: HashMap<String, (Instant, u32)>,
    /// Hand-offs not yet used, by the digest of their token. In memory only, so a restart ends
    /// them.
    handoffs: HashMap<String, Handoff>,
    /// Why requests were refused, each logged once per page, viewer and reason.
    noted: HashSet<(String, String)>,
    /// What was logged, for the tests to read.
    #[cfg(test)]
    log: Vec<String>,
}

/// What a hand-off's single-use token stands for.
#[derive(Debug, Clone)]
pub(crate) struct Handoff {
    pub(crate) page: String,
    pub(crate) person: String,
    /// The login the page cookie it sets will name, which must still stand when it is used.
    pub(crate) sign_in: String,
    /// Unix seconds.
    pub(crate) expires: f64,
}

/// The key of one viewer's viewing of one page, which notes and read counts are kept by.
pub(crate) fn viewing(page: &str, viewer: &str) -> String {
    format!("{page} {viewer}")
}

/// The pages on disk, and the uploads and counters in memory.
pub(crate) struct PageStore {
    dir: PathBuf,
    /// The key view tokens and page cookies are made with, kept in the pages directory.
    key: Vec<u8>,
    state: Mutex<Inner>,
    staged: Notify,
    sweeping: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for PageStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageStore")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

/// A page checked against the quotas and written aside, not yet in the store: the audit
/// record comes between [`PageStore::prepare`] and [`PageStore::commit`].
#[derive(Debug)]
pub(crate) struct Prepared {
    pub(crate) page: Page,
    temp: PathBuf,
}

impl PageStore {
    /// Opens `dir`, created private (`0700` on Unix), reading each page's record and the key,
    /// which it makes the first time.
    ///
    /// # Errors
    ///
    /// The directory cannot be made or read, or the key cannot be read or made.
    pub(crate) fn open(dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let mut pages = BTreeMap::new();
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".tmp-") {
                // A publish that never finished.
                let _ = std::fs::remove_dir_all(entry.path());
                continue;
            }
            let Ok(text) = std::fs::read(entry.path().join("meta.json")) else {
                continue;
            };
            if let Ok(page) = serde_json::from_slice::<Page>(&text)
                && page.id == name
            {
                pages.insert(page.id.clone(), page);
            }
        }
        let key = read_or_make_key(&dir.join(KEY_FILE))?;
        Ok(Self {
            dir,
            key,
            state: Mutex::new(Inner {
                pages,
                ..Inner::default()
            }),
            staged: Notify::new(),
            sweeping: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// The keyed digest of `parts` under the store's key, as base64url.
    pub(crate) fn keyed_text(&self, parts: &[&str]) -> String {
        base64url(&keyed(&self.key, parts))
    }

    /// The view token the frame of `page` names for `viewer`, with `network`, until `expires`
    /// (Unix seconds): `{expires}.{network}.{mark}`, its end in Unix milliseconds. It names the page's files and feeds and
    /// opens nothing by itself; it ties those requests to the frame its viewer was shown, which
    /// another site cannot know.
    pub(crate) fn view_token(
        &self,
        page: &str,
        viewer: &str,
        network: PageNetwork,
        expires: f64,
    ) -> String {
        let expires = format!("{:.0}", (expires * 1000.0).floor());
        let mark = self.keyed_text(&["view", page, viewer, network.as_str(), &expires]);
        format!("{expires}.{}.{mark}", network.as_str())
    }

    /// The network of the view token `token` when it is `viewer`'s for `page` and has not
    /// expired; why not, otherwise.
    pub(crate) fn viewed(
        &self,
        token: &str,
        page: &str,
        viewer: &str,
    ) -> Result<PageNetwork, &'static str> {
        let mut parts = token.splitn(3, '.');
        let (Some(expires), Some(network), Some(mark)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err("a view token that is none");
        };
        let network = match network {
            "open" => PageNetwork::Open,
            "own" => PageNetwork::Own,
            "none" => PageNetwork::None,
            _ => return Err("a view token that is none"),
        };
        let expected = self.keyed_text(&["view", page, viewer, network.as_str(), expires]);
        if !crate::constant_time_eq(expected.as_bytes(), mark.as_bytes()) {
            return Err("a view token that is not this viewer's for this page");
        }
        match expires.parse::<f64>() {
            Ok(end) if end / 1000.0 > now() => Ok(network),
            _ => Err("a view token that has expired; reload the page"),
        }
    }

    /// Makes a single-use token for `handoff`, keeping only its digest. A person past `most`
    /// unused ones loses their oldest.
    pub(crate) fn hand_off(&self, handoff: Handoff, most: usize) -> String {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut inner = self.state();
        let at = now();
        inner.handoffs.retain(|_, held| held.expires > at);
        let person = handoff.person.clone();
        inner.handoffs.insert(sha256_hex(token.as_bytes()), handoff);
        loop {
            let mine = inner
                .handoffs
                .iter()
                .filter(|(_, held)| held.person == person);
            if mine.clone().count() <= most {
                break;
            }
            let Some(oldest) = mine
                .min_by(|(_, one), (_, other)| one.expires.total_cmp(&other.expires))
                .map(|(digest, _)| digest.clone())
            else {
                break;
            };
            inner.handoffs.remove(&oldest);
        }
        token
    }

    /// Spends the hand-off `token`, whatever comes of it: what it stands for, unless it was spent
    /// already or has ended.
    pub(crate) fn take_handoff(&self, token: &str) -> Option<Handoff> {
        let handoff = self
            .state()
            .handoffs
            .remove(&sha256_hex(token.as_bytes()))?;
        (handoff.expires > now()).then_some(handoff)
    }

    /// Logs why a request for a page was refused, once for each [`viewing`] key and reason.
    pub(crate) fn note(&self, key: &str, reason: &str) {
        let mut state = self.state();
        if state.noted.insert((key.to_owned(), reason.to_owned())) {
            let line = match key.split_once(' ') {
                Some((page, viewer)) => format!("page {page}, viewer {viewer}: {reason}"),
                None => format!("page {key}: {reason}"),
            };
            eprintln!("universal-exports: {line}");
            #[cfg(test)]
            state.log.push(line);
        }
    }

    /// What [`note`](Self::note) logged.
    #[cfg(test)]
    pub(crate) fn logged(&self) -> Vec<String> {
        self.state().log.clone()
    }

    /// Whether this call is the one that starts the sweeper.
    pub(crate) fn start_sweeper(&self) -> bool {
        !self
            .sweeping
            .swap(true, std::sync::atomic::Ordering::AcqRel)
    }

    fn state(&self) -> MutexGuard<'_, Inner> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A fresh page ID.
    pub(crate) fn new_id() -> String {
        format!("page-{}", uuid::Uuid::new_v4().simple())
    }

    pub(crate) fn get(&self, id: &str) -> Option<Page> {
        self.state().pages.get(id).cloned()
    }

    /// Pages of other people than `person` that are shared with someone, newest first: the
    /// ones that might be shared with `person`.
    pub(crate) fn shared_by_others(&self, person: &str) -> Vec<Page> {
        let mut pages: Vec<Page> = self
            .state()
            .pages
            .values()
            .filter(|page| page.person_id != person && !page.shares.is_empty())
            .cloned()
            .collect();
        pages.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        pages
    }

    /// Sets whom a page is shared with.
    ///
    /// # Errors
    ///
    /// The record could not be written; nothing changed.
    pub(crate) fn share(&self, id: &str, shares: Vec<String>) -> io::Result<Option<Page>> {
        let mut state = self.state();
        let Some(page) = state.pages.get(id) else {
            return Ok(None);
        };
        let mut changed = page.clone();
        changed.shares = shares;
        self.save(&changed)?;
        state.pages.insert(changed.id.clone(), changed.clone());
        Ok(Some(changed))
    }

    /// `person`'s pages, newest first.
    pub(crate) fn of(&self, person: &str) -> Vec<Page> {
        let mut pages: Vec<Page> = self
            .state()
            .pages
            .values()
            .filter(|page| page.person_id == person)
            .cloned()
            .collect();
        pages.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        pages
    }

    /// Checks `page` and its `files` against `limits` and writes them aside. `replace` is the
    /// ID of the page these files take the place of, whose size does not count.
    ///
    /// # Errors
    ///
    /// The quota or path that refuses it, in words for her.
    pub(crate) fn prepare(
        &self,
        page: Page,
        files: &BTreeMap<String, Vec<u8>>,
        limits: &AssistantPageLimits,
    ) -> Result<Prepared, String> {
        let total: u64 = files.values().map(|bytes| bytes.len() as u64).sum();
        if files.is_empty() {
            return Err("the page has no file".to_owned());
        }
        if !limits.max_page_files.allows(files.len()) {
            return Err(format!("a page may hold {} files", limits.max_page_files));
        }
        for (path, bytes) in files {
            if !valid_path(path) {
                return Err(format!("{path:?} is not a path inside a page"));
            }
            if !limits.max_file_bytes.allows(bytes.len()) {
                return Err(format!(
                    "{path} is longer than {} bytes, the most one file of a page may hold",
                    limits.max_file_bytes
                ));
            }
        }
        if let Some(path) = files.keys().find(|path| path.starts_with(FEEDS_DIR)) {
            return Err(format!(
                "{path} is under a top-level {FEEDS_DIR} directory, where a dashboard reads its \
                 feeds as {FEEDS_DIR}NAME; put it in another directory"
            ));
        }
        if !limits.max_page_bytes.allows(total as usize) {
            return Err(format!(
                "the page is longer than {} bytes",
                limits.max_page_bytes
            ));
        }
        {
            let state = self.state();
            let others = state.pages.values().filter(|other| other.id != page.id);
            let (mut count, mut person, mut all) = (0usize, 0u64, 0u64);
            for other in others {
                all += other.bytes;
                if other.person_id == page.person_id {
                    count += 1;
                    person += other.bytes;
                }
            }
            if !limits.max_pages.allows(count + 1) {
                return Err(format!(
                    "you already have {} pages, the most kept; delete one first",
                    limits.max_pages
                ));
            }
            if !limits.max_person_bytes.allows((person + total) as usize) {
                return Err(format!(
                    "your pages would be longer than {} bytes in all",
                    limits.max_person_bytes
                ));
            }
            if !limits.max_total_bytes.allows((all + total) as usize) {
                return Err("this gateway holds all the page bytes it is set to".to_owned());
            }
        }
        let temp = self
            .dir
            .join(format!("{}.tmp-{}", page.id, uuid::Uuid::new_v4().simple()));
        let write = || -> io::Result<()> {
            for (path, bytes) in files {
                let at = path
                    .split('/')
                    .fold(temp.join("files"), |at, part| at.join(part));
                if let Some(parent) = at.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(at, bytes)?;
            }
            std::fs::write(
                temp.join("meta.json"),
                serde_json::to_vec_pretty(&page).map_err(io::Error::other)?,
            )
        };
        if let Err(error) = write() {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(format!("the page could not be stored: {error}"));
        }
        Ok(Prepared { page, temp })
    }

    /// Puts a prepared page in the store, in place of the one with its ID.
    ///
    /// # Errors
    ///
    /// The directory could not be moved into place.
    pub(crate) fn commit(&self, prepared: Prepared) -> io::Result<Page> {
        let Prepared { page, temp } = prepared;
        let at = self.dir.join(&page.id);
        let mut state = self.state();
        if at.exists() {
            std::fs::remove_dir_all(&at)?;
        }
        std::fs::rename(&temp, &at)?;
        state.pages.insert(page.id.clone(), page.clone());
        Ok(page)
    }

    /// Throws a prepared page away.
    pub(crate) fn discard(&self, prepared: Prepared) {
        let _ = std::fs::remove_dir_all(prepared.temp);
    }

    /// Rewrites a page's record.
    fn save(&self, page: &Page) -> io::Result<()> {
        std::fs::write(
            self.dir.join(&page.id).join("meta.json"),
            serde_json::to_vec_pretty(page).map_err(io::Error::other)?,
        )
    }

    /// Removes a page, with what was counted and noted of its viewing.
    pub(crate) fn delete(&self, id: &str) -> Option<Page> {
        let mut state = self.state();
        let page = state.pages.remove(id)?;
        Self::forget_viewing(&mut state);
        let _ = std::fs::remove_dir_all(self.dir.join(id));
        Some(page)
    }

    /// Drops the read counts and notes of pages that are gone.
    fn forget_viewing(state: &mut Inner) {
        let Inner {
            pages,
            reads,
            noted,
            ..
        } = state;
        let standing = |key: &str| pages.contains_key(key.split(' ').next().unwrap_or(key));
        reads.retain(|key, _| standing(key));
        noted.retain(|(key, _)| standing(key));
    }

    /// Sets or clears a page's expiry: remembered pages have none.
    pub(crate) fn remember(&self, id: &str, remember: bool, ttl: Duration) -> Option<Page> {
        let mut state = self.state();
        let page = state.pages.get_mut(id)?;
        page.expires_at = if remember {
            None
        } else {
            Some(now() + ttl.as_secs_f64())
        };
        let page = page.clone();
        let _ = self.save(&page);
        Some(page)
    }

    /// The bytes of one listed file of a page.
    pub(crate) fn read_file(&self, page: &Page, path: &str) -> Option<Vec<u8>> {
        if !page.files.iter().any(|file| file.path == path) {
            return None;
        }
        let at = path
            .split('/')
            .fold(self.dir.join(&page.id).join("files"), |at, part| {
                at.join(part)
            });
        std::fs::read(at).ok()
    }

    /// Deletes expired pages and stale uploads; answers the pages deleted.
    pub(crate) fn sweep(&self, at: f64, stage_ttl: Duration) -> Vec<Page> {
        let mut state = self.state();
        let expired: Vec<String> = state
            .pages
            .values()
            .filter(|page| page.expires_at.is_some_and(|end| end <= at))
            .map(|page| page.id.clone())
            .collect();
        let mut gone = Vec::new();
        for id in expired {
            if let Some(page) = state.pages.remove(&id) {
                let _ = std::fs::remove_dir_all(self.dir.join(&id));
                gone.push(page);
            }
        }
        Self::forget_viewing(&mut state);
        state
            .staged
            .retain(|_, staged| staged.at.elapsed() < stage_ttl);
        gone
    }

    /// Counts a feed read of the [`viewing`] key `key`; false past `most` in the minute.
    pub(crate) fn note_read(&self, key: &str, most: usize) -> bool {
        let mut state = self.state();
        let entry = state
            .reads
            .entry(key.to_owned())
            .or_insert((Instant::now(), 0));
        if entry.0.elapsed() >= Duration::from_secs(60) {
            *entry = (Instant::now(), 0);
        }
        entry.1 += 1;
        usize::try_from(entry.1).is_ok_and(|count| ai6::Limit::at(most as u64).allows(count))
    }

    // ---- public links

    /// Gives page `id` a public link, keeping the one it has; `None` turns it off. Answers the
    /// page as it now stands.
    ///
    /// # Errors
    ///
    /// The record could not be written; nothing changed.
    pub(crate) fn set_public(&self, id: &str, on: bool) -> io::Result<Option<Page>> {
        let mut state = self.state();
        let Some(page) = state.pages.get(id) else {
            return Ok(None);
        };
        let mut changed = page.clone();
        changed.public = match (on, changed.public.take()) {
            (true, Some(kept)) => Some(kept),
            (true, None) => Some(random_token()),
            (false, _) => None,
        };
        self.save(&changed)?;
        state.pages.insert(changed.id.clone(), changed.clone());
        Ok(Some(changed))
    }

    /// The page whose public link is `token`, while it stands.
    pub(crate) fn public(&self, token: &str) -> Option<Page> {
        let at = now();
        self.state()
            .pages
            .values()
            .find(|page| {
                page.public.as_deref() == Some(token)
                    && !page.expires_at.is_some_and(|end| end <= at)
            })
            .cloned()
    }

    // ---- uploads from Stations

    /// Takes one file of a page a Station uploads for the call `digest` of `conversation`.
    ///
    /// # Errors
    ///
    /// The reason it was refused.
    pub(crate) fn stage_file(
        &self,
        station: &str,
        conversation: &str,
        digest: &str,
        path: &str,
        bytes: Vec<u8>,
        limits: &AssistantPageLimits,
    ) -> Result<(), String> {
        if !valid_path(path) {
            return Err(format!("{path:?} is not a path inside a page"));
        }
        if !limits.max_file_bytes.allows(bytes.len()) {
            return Err(format!(
                "{path} is longer than {} bytes",
                limits.max_file_bytes
            ));
        }
        let key = (conversation.to_owned(), digest.to_owned());
        let mut state = self.state();
        if !state.staged.contains_key(&key) && !limits.staged_uploads.allows(state.staged.len() + 1)
        {
            return Err("too many uploads wait to be published".to_owned());
        }
        let staged = state.staged.entry(key).or_insert_with(|| Staged {
            station: station.to_owned(),
            files: BTreeMap::new(),
            bytes: 0,
            at: Instant::now(),
            done: None,
        });
        if staged.station != station || staged.done.is_some() {
            return Err("this upload is not open to that Station".to_owned());
        }
        let before = staged.files.get(path).map_or(0, Vec::len) as u64;
        let bytes_total = staged.bytes - before + bytes.len() as u64;
        if !limits
            .max_page_files
            .allows(staged.files.len() + usize::from(before == 0))
        {
            return Err(format!("a page may hold {} files", limits.max_page_files));
        }
        if !limits.max_page_bytes.allows(bytes_total as usize) {
            return Err(format!("a page may hold {} bytes", limits.max_page_bytes));
        }
        staged.bytes = bytes_total;
        staged.files.insert(path.to_owned(), bytes);
        Ok(())
    }

    /// Ends an upload with the Station's manifest, or its refusal.
    ///
    /// # Errors
    ///
    /// No such upload for that Station.
    pub(crate) fn stage_done(
        &self,
        station: &str,
        conversation: &str,
        digest: &str,
        outcome: Result<Vec<AssistantPageFile>, String>,
        limits: &AssistantPageLimits,
    ) -> Result<(), String> {
        let key = (conversation.to_owned(), digest.to_owned());
        {
            let mut state = self.state();
            if !state.staged.contains_key(&key)
                && !limits.staged_uploads.allows(state.staged.len() + 1)
            {
                return Err("too many uploads wait to be published".to_owned());
            }
            let staged = state.staged.entry(key).or_insert_with(|| Staged {
                station: station.to_owned(),
                files: BTreeMap::new(),
                bytes: 0,
                at: Instant::now(),
                done: None,
            });
            if staged.station != station {
                return Err("this upload is not open to that Station".to_owned());
            }
            staged.done = Some(outcome);
        }
        self.staged.notify_waiters();
        Ok(())
    }

    /// Takes the upload for the call, waiting up to `wait` for the Station to finish it; its
    /// files are checked against the manifest. A refusal, a mismatch or silence is the error.
    pub(crate) async fn claim(
        &self,
        conversation: &str,
        digest: &str,
        wait: Duration,
    ) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let key = (conversation.to_owned(), digest.to_owned());
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.staged.notified();
            tokio::pin!(notified);
            // Registered before the look, so a `done` between the look and the wait is seen.
            notified.as_mut().enable();
            {
                let mut state = self.state();
                if state
                    .staged
                    .get(&key)
                    .is_some_and(|staged| staged.done.is_some())
                    && let Some(staged) = state.staged.remove(&key)
                {
                    return match staged.done {
                        Some(Ok(manifest)) => verify(staged.files, manifest),
                        Some(Err(reason)) => Err(format!("the Station refused it: {reason}")),
                        None => Err("the upload did not finish".to_owned()),
                    };
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                self.state().staged.remove(&key);
                return Err(format!(
                    "the Station sent no files within {} seconds; is the path under \
                     personal/ or scratch/?",
                    wait.as_secs()
                ));
            }
        }
    }
}

/// The files a Station sent, held to the manifest it ended with: each listed file arrived with
/// its size and SHA-256, and nothing else did.
fn verify(
    files: BTreeMap<String, Vec<u8>>,
    manifest: Vec<AssistantPageFile>,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    if manifest.len() != files.len() {
        return Err("the Station's manifest does not match what it sent".to_owned());
    }
    for file in &manifest {
        match files.get(&file.path) {
            Some(bytes) if bytes.len() as u64 == file.bytes && sha256_hex(bytes) == file.sha256 => {
            }
            _ => {
                return Err(format!(
                    "{} does not match the Station's manifest",
                    file.path
                ));
            }
        }
    }
    Ok(files)
}

// ---------------------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------------------

/// The origin of the pages' base: `frame-ancestors` is compared with a parent's origin, whose path
/// is `/`, so a source with any other path never matches and the browser blocks the frame.
fn origin_of(base: &str) -> &str {
    let after_scheme = base.find("://").map_or(0, |at| at + 3);
    base[after_scheme..]
        .find('/')
        .map_or(base, |at| &base[..after_scheme + at])
}

/// The CSP of every page response for `network` and `scripts`; `base` is the pages' base, whose
/// origin alone may frame a page. The sandbox never has `allow-same-origin` or
/// `allow-top-navigation*`, so a page is an opaque origin and cannot reach its host.
pub(crate) fn page_csp(base: &str, network: PageNetwork, scripts: bool) -> String {
    let mut csp = String::from(
        "sandbox allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals",
    );
    if scripts {
        csp.push_str(" allow-scripts");
    }
    let _ = write!(csp, "; frame-ancestors {}", origin_of(base));
    match network {
        PageNetwork::Open => {}
        _ => csp.push_str(
            "; default-src 'self' data: blob: 'unsafe-inline' 'unsafe-eval'; connect-src 'self' data: blob:",
        ),
    }
    csp
}

/// The CSP of the gateway's frame document: no script, only what is under the pages' base
/// framed (`frame-src` takes a path as a prefix only with a trailing slash).
pub(crate) fn frame_csp(base: &str) -> String {
    format!(
        "default-src 'none'; style-src 'unsafe-inline'; img-src data:; frame-src {base}/; \
         form-action 'none'; base-uri 'none'; frame-ancestors 'none'"
    )
}

/// The headers every page response carries, the CSP in `csp`.
fn page_headers(csp: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let set = |headers: &mut HeaderMap, name: HeaderName, value: &str| {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(name, value);
        }
    };
    set(&mut headers, CONTENT_SECURITY_POLICY, csp);
    set(&mut headers, X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(&mut headers, REFERRER_POLICY, "no-referrer");
    set(&mut headers, CACHE_CONTROL, "no-store, private");
    // The opaque page loads its own files from another origin as it sees it: who asks, and the
    // view token its frame named, guard them.
    set(
        &mut headers,
        HeaderName::from_static("cross-origin-resource-policy"),
        "cross-origin",
    );
    set(
        &mut headers,
        HeaderName::from_static("access-control-allow-origin"),
        "*",
    );
    headers
}

/// The profile a link is served under: the administrator's, or tighter when the viewer asked.
pub(crate) fn profile(limits: &AssistantPageLimits, asked: Option<PageNetwork>) -> PageNetwork {
    limits.network.tighter(asked.unwrap_or(PageNetwork::Open))
}

// ---------------------------------------------------------------------------------------
// Serving
// ---------------------------------------------------------------------------------------

/// Where pages are served from: a directory, and optionally their own origin and listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagesConfig {
    /// Where pages are kept.
    pub dir: PathBuf,
    /// `[pages] origin`: the pages' own origin, such as `https://pages.example.net`. Without
    /// one, pages are served under `/pages/` on the API's origin, by design.
    pub origin: Option<String>,
    /// `[pages] listen`: the loopback address of the pages' own listener (with `origin`).
    pub listen: Option<std::net::SocketAddr>,
}

impl PagesConfig {
    /// Pages kept in `dir`, served on the API's own origin.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            origin: None,
            listen: None,
        }
    }

    /// Serves pages on their own `origin` and `listen` address.
    #[must_use]
    pub fn with_origin(mut self, origin: impl Into<String>, listen: std::net::SocketAddr) -> Self {
        self.origin = Some(origin.into());
        self.listen = Some(listen);
        self
    }
}

/// The base URL pages are served under, no trailing slash.
pub(crate) fn pages_base(gateway: &Gateway) -> String {
    match gateway
        .pages_config
        .as_ref()
        .and_then(|config| config.origin.clone())
    {
        Some(origin) => origin.trim_end_matches('/').to_owned(),
        None => format!("{}/pages", gateway.public_origin().trim_end_matches('/')),
    }
}

/// The routes of pages. `/p/{id}/` is a private page's frame; `/p/{id}/{token}/f/{path}` and
/// `/p/{id}/{token}/data/{name}` are its files and feeds under the view token the frame names,
/// each feed also at `/p/{id}/{token}/f/data/{name}`, where `data/NAME` from a file at the top of
/// the page leads. Every one of them names its viewer ([`page_access::viewer`]) and checks
/// [`may_view`]. `/s/{token}/` and `/s/{token}/f/{path}` are a public link's frame and files, with
/// no feed. Nothing of `/v1`.
pub(crate) fn pages_router() -> Router<Gateway> {
    Router::new()
        .route("/p/{id}/", get(frame))
        .route("/p/{id}/{token}/f/{*path}", get(file))
        .route("/p/{id}/{token}/data/{name}", get(feed))
        .route("/p/{id}/{token}/f/data/{name}", get(feed))
        .route("/s/{token}/", get(public_frame))
        .route("/s/{token}/f/{*path}", get(public_file))
}

fn gone() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// What the compartments grant now, read for one decision; `None` without a configuration
/// provider or when it cannot be read, which grants nothing.
async fn compartments_now(gateway: &Gateway) -> Option<ai6::CompiledCompartments> {
    let roster = gateway.roster.clone()?;
    tokio::task::spawn_blocking(move || {
        ai6::directorates::m::compartments::current(roster.as_ref()).ok()
    })
    .await
    .ok()?
}

/// Whether `person` holds `mission.read` on Operation `operation`, compartments and the tree of
/// Operations included.
pub(crate) async fn reads_operation(gateway: &Gateway, person: &Person, operation: &str) -> bool {
    crate::resource_action_allowed(
        gateway,
        person.clone(),
        crate::tree::operation_resource(operation),
        "mission.read",
    )
    .await
    .unwrap_or(false)
}

/// Whether `person` is read into compartment `id` now.
pub(crate) async fn read_into(gateway: &Gateway, person: &Person, id: &str) -> bool {
    compartments_now(gateway)
        .await
        .is_some_and(|compiled| compiled.person_holds(id, person))
}

/// Whether `viewer` may read `page`: its person, or someone one of its shares names while
/// `limits` lets pages be shared. An administrator is no exception.
pub(crate) async fn may_view(
    gateway: &Gateway,
    page: &Page,
    viewer: &Person,
    limits: &AssistantPageLimits,
) -> bool {
    if page.person_id == viewer.id {
        return true;
    }
    if !limits.sharing {
        return false;
    }
    for share in &page.shares {
        let named = match share_subject(share) {
            Some(("compartment", id)) => read_into(gateway, viewer, id).await,
            Some(("operation", id)) => reads_operation(gateway, viewer, id).await,
            Some(_) => share
                .parse::<ai6::Subject>()
                .is_ok_and(|subject| subject.matches(viewer)),
            None => false,
        };
        if named {
            return true;
        }
    }
    false
}

/// The live `[assistant.pages]`; `None` when the roster cannot be read.
async fn page_limits(gateway: &Gateway) -> Option<AssistantPageLimits> {
    Some(
        super::assistant_settings(gateway)
            .await
            .ok()?
            .pages()
            .clone(),
    )
}

/// What a viewer whose sign-in has ended reads.
pub(crate) const SIGNED_OUT: &str = "You are signed out: your sign-in has ended. Sign in to \
                                     Outer Office and open the page again.";

/// `403` with [`SIGNED_OUT`], as plain text nothing runs in.
fn signed_out() -> Response {
    let mut headers = page_headers("sandbox; default-src 'none'");
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    (StatusCode::FORBIDDEN, headers, SIGNED_OUT).into_response()
}

/// A request for private page `id`, once it names a person who may read the page now: the page,
/// the person and the live `[assistant.pages]`; otherwise the answer to give, its reason logged.
/// A request that names nobody, or someone the page is hidden from, is answered as a page that
/// does not exist.
async fn asked(
    gateway: &Gateway,
    id: &str,
    headers: &HeaderMap,
    connection: Connection,
) -> Result<(std::sync::Arc<PageStore>, Page, Person, AssistantPageLimits), Box<Response>> {
    let refused = || Box::new(gone());
    let store = gateway.pages.clone().ok_or_else(refused)?;
    let at = now();
    let page = store
        .get(id)
        .filter(|page| !page.expires_at.is_some_and(|end| end <= at))
        .ok_or_else(refused)?;
    let viewer = match page_access::viewer(gateway, headers, connection).await {
        Ok(viewer) => viewer,
        Err(NoViewer::SignedOut) => {
            store.note(
                &page.id,
                "asked for with the cookie of a sign-in that has ended",
            );
            return Err(Box::new(signed_out()));
        }
        Err(NoViewer::Nobody) => return Err(refused()),
        Err(NoViewer::Unavailable) => {
            store.note(&page.id, "the viewer could not be told");
            return Err(Box::new(StatusCode::SERVICE_UNAVAILABLE.into_response()));
        }
    };
    let Some(limits) = page_limits(gateway).await else {
        store.note(&page.id, "the roster could not be read");
        return Err(Box::new(StatusCode::SERVICE_UNAVAILABLE.into_response()));
    };
    if !may_view(gateway, &page, &viewer, &limits).await {
        store.note(
            &viewing(&page.id, &viewer.id),
            "the viewer may not read the page",
        );
        return Err(refused());
    }
    Ok((store, page, viewer, limits))
}

/// A `303 See Other` to `to`.
fn see_other(to: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    if let Ok(to) = HeaderValue::from_str(to) {
        response.headers_mut().insert(LOCATION, to);
    }
    response
}

/// Whether a request is a top-level navigation: the frame's own load is an `iframe` and a
/// page's loads are other destinations.
fn top_level(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-dest")
        .and_then(|value| value.to_str().ok())
        == Some("document")
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FrameQuery {
    /// `none`, `own` or `open`: a tighter network than the deployment's, for this opening.
    network: Option<String>,
    /// A hand-off's single-use token, which [`handed_off`] spends.
    handoff: Option<String>,
}

/// `GET /p/{id}/?handoff=`: a browser the app sent here with a hand-off and perhaps no cookie.
/// Spends the token, then sets the page cookie and sends the browser to the page's clean address,
/// so the token leaves the address bar. Anything but a good token for this page answers as a page
/// that does not exist. Only a navigation spends one: another site's `<img>`, a prefetch or a
/// `HEAD` would otherwise use it up, or set the cookie, without the person opening anything.
async fn handed_off(
    gateway: &Gateway,
    id: &str,
    token: &str,
    network: Option<&str>,
    method: &Method,
    headers: &HeaderMap,
) -> Response {
    let navigation = *method == Method::GET
        && !headers.contains_key("sec-purpose")
        && headers
            .get("sec-fetch-dest")
            .is_none_or(|dest| dest == "document");
    if !navigation || !matches!(network, None | Some("none" | "own" | "open")) {
        return gone();
    }
    let Some(store) = gateway.pages.clone() else {
        return gone();
    };
    let handoff = store.take_handoff(token);
    let at = now();
    let Some(page) = store
        .get(id)
        .filter(|page| !page.expires_at.is_some_and(|end| end <= at))
    else {
        return gone();
    };
    let Some(handoff) = handoff.filter(|handoff| handoff.page == page.id) else {
        store.note(&page.id, "a hand-off spent, ended or for another page");
        return gone();
    };
    let Some(viewer) = handoff_viewer(gateway, &store, &page, &handoff).await else {
        return gone();
    };
    let registry = gateway.registry.clone();
    let sign_in = handoff.sign_in.clone();
    let Some(Ok(Ok(Some(login)))) = (match registry {
        Some(registry) => {
            Some(tokio::task::spawn_blocking(move || registry.login_by_id(&sign_in)).await)
        }
        None => None,
    }) else {
        store.note(
            &viewing(&page.id, &viewer.id),
            "a hand-off whose sign-in has ended",
        );
        return gone();
    };
    let seconds = login.expires_at() - now();
    let Some(cookie) = page_access::handed_off_cookie(gateway, &handoff.sign_in, seconds) else {
        return gone();
    };
    // The audit record comes before the browser holds the cookie.
    if super::audit(
        gateway,
        &viewer,
        "moneypenny.page.handoff_used",
        String::new(),
        Some(RESOURCE.to_owned()),
        &page.id,
    )
    .await
    .is_err()
    {
        store.note(&page.id, "a hand-off that could not be audited");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mut clean = format!("{}/p/{}/", pages_base(gateway), percent(&page.id));
    if let Some(network) = network {
        clean.push_str(&format!("?network={network}"));
    }
    let mut response = see_other(&clean);
    let answer = response.headers_mut();
    answer.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    answer.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    page_access::append(answer, Some(cookie));
    response
}

/// The person a hand-off stands for, when the page may still be shown to them.
async fn handoff_viewer(
    gateway: &Gateway,
    store: &PageStore,
    page: &Page,
    handoff: &Handoff,
) -> Option<Person> {
    let Ok(viewer) = crate::person_by_id(gateway, handoff.person.clone()).await else {
        store.note(&page.id, "a hand-off whose person cannot be found");
        return None;
    };
    let limits = page_limits(gateway).await?;
    if !may_view(gateway, page, &viewer, &limits).await {
        store.note(
            &viewing(&page.id, &viewer.id),
            "a hand-off to someone who may not read the page",
        );
        return None;
    }
    Some(viewer)
}

/// `GET /p/{id}/`: a private page's frame, for its person and whom it is shared with, opened from
/// the app, a bookmark, a new tab or another device alike. The page in it is named under a view
/// token for this viewer and opening ([`PageStore::view_token`]); each opening is audited.
async fn frame(
    State(gateway): State<Gateway>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<FrameQuery>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    certificates: Option<Extension<ClientCertificates>>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if let Some(token) = &query.handoff {
        let network = query.network.as_deref();
        return handed_off(&gateway, &id, token, network, &method, &headers).await;
    }
    let (store, page, viewer, limits) =
        match asked(&gateway, &id, &headers, (connection, certificates)).await {
            Ok(asked) => asked,
            Err(refused) => return *refused,
        };
    let asked_network = match query.network.as_deref() {
        None => None,
        Some("none") => Some(PageNetwork::None),
        Some("own") => Some(PageNetwork::Own),
        Some("open") => Some(PageNetwork::Open),
        Some(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let network = profile(&limits, asked_network);
    let mut expires = now() + limits.open_ttl.0.as_secs_f64();
    if let Some(end) = page.expires_at {
        expires = expires.min(end);
    }
    let token = store.view_token(&page.id, &viewer.id, network, expires);
    audit_opened(&gateway, &viewer, &page, network).await;
    let base = pages_base(&gateway);
    let src = format!("{token}/f/");
    let mut response =
        frame_html(&page, &Viewing::Person(&viewer.id), &src, &limits).into_response();
    *response.headers_mut() = frame_headers(&base);
    response
}

/// Everything the frame document and the framed page need to be served.
fn frame_headers(base: &str) -> HeaderMap {
    let mut headers = page_headers(&frame_csp(base));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.remove(HeaderName::from_static("access-control-allow-origin"));
    headers
}

/// Audits an opening of `page` by `viewer`.
async fn audit_opened(gateway: &Gateway, viewer: &Person, page: &Page, network: PageNetwork) {
    let _ = super::audit(
        gateway,
        viewer,
        "moneypenny.page.opened",
        String::new(),
        Some(RESOURCE.to_owned()),
        &format!("{} ({})", page.id, network.as_str()),
    )
    .await;
}

/// Whom a frame is shown to.
enum Viewing<'a> {
    /// A signed-in viewer, by their ID.
    Person(&'a str),
    /// Anyone holding the page's public link.
    Public,
}

/// The frame: the classification bar above an iframe of the page's entry under `src`. A
/// viewer's frame drops the bar with `[assistant.pages] frame = false`; a public one never does.
fn frame_html(page: &Page, viewing: &Viewing, src: &str, limits: &AssistantPageLimits) -> String {
    let expiry = match page.expires_at {
        Some(end) => format!("Expires {}.", escape(&utc_text(end))),
        None => "Remembered.".to_owned(),
    };
    let what = format!(
        "{} <b>{}</b>.",
        escape(page.kind.as_str()),
        escape(&page.title)
    );
    let (note, bar) = match viewing {
        Viewing::Person(viewer) => {
            let mut note = format!(
                "Made by Moneypenny for {}, not part of Outer Office. {what} {expiry}",
                escape(&page.person_id),
            );
            if *viewer != page.person_id {
                let _ = write!(note, " Shared by {}.", escape(&page.person_id));
            }
            (note, limits.frame)
        }
        Viewing::Public => (
            format!(
                "Made by Moneypenny, not part of Outer Office. {what} Public: anyone with this \
                 link sees this page, without live data. {expiry}"
            ),
            true,
        ),
    };
    let bar = if bar {
        format!(
            "<input type=\"checkbox\" id=\"c\" aria-label=\"Hide the notice\">\
<div class=\"bar\" role=\"note\"><span>{note}</span><label for=\"c\" title=\"Hide for this view\">&#10005;</label></div>"
        )
    } else {
        String::new()
    };
    let sandbox = frame_sandbox(limits.scripts);
    let entry = page
        .entry
        .split('/')
        .map(percent)
        .collect::<Vec<_>>()
        .join("/");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style>\
html,body{{height:100%;margin:0}}body{{display:flex;flex-direction:column;font:14px system-ui,sans-serif}}\
.bar{{background:#e8eef7;color:#10233f;padding:.45rem .8rem;display:flex;gap:.8rem;align-items:flex-start;\
border-bottom:1px solid #9fb3cf}}\
.bar span{{flex:1}}.bar label{{cursor:pointer;font-weight:700;padding:0 .4rem}}\
#c{{position:absolute;opacity:0;pointer-events:none}}#c:checked~.bar{{display:none}}\
iframe{{flex:1;border:0;width:100%;background:#fff}}</style></head><body>{bar}\
<iframe src=\"{src}{entry}\" sandbox=\"{sandbox}\" allow=\"fullscreen\" referrerpolicy=\"no-referrer\" \
title=\"{title}\"></iframe></body></html>",
        title = escape(&page.title),
    )
}

/// The iframe's own `sandbox` tokens: those of the page's CSP.
fn frame_sandbox(scripts: bool) -> String {
    let mut tokens = String::from(
        "allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals",
    );
    if scripts {
        tokens.push_str(" allow-scripts");
    }
    tokens
}

/// `part` percent-encoded for a URL path segment.
pub(crate) fn percent(part: &str) -> String {
    let mut out = String::new();
    for byte in part.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[derive(Deserialize)]
struct FileQuery {
    source: Option<String>,
}

/// A request for a file or feed of private page `id` under view token `token`: the request names
/// its viewer, who may read the page, and the token is the one the frame named for them. Answers
/// the page, the viewer, the live `[assistant.pages]` and the token's network.
async fn viewed(
    gateway: &Gateway,
    id: &str,
    token: &str,
    headers: &HeaderMap,
    connection: Connection,
) -> Result<
    (
        std::sync::Arc<PageStore>,
        Page,
        Person,
        AssistantPageLimits,
        PageNetwork,
    ),
    Box<Response>,
> {
    let (store, page, viewer, limits) = asked(gateway, id, headers, connection).await?;
    match store.viewed(token, &page.id, &viewer.id) {
        Ok(network) => Ok((store, page, viewer, limits, network)),
        Err(reason) => {
            store.note(&viewing(&page.id, &viewer.id), reason);
            Err(Box::new(gone()))
        }
    }
}

/// `GET /p/{id}/{token}/f/{path}`: a file of the page. A top-level request is sent to the
/// frame, which carries the bar.
async fn file(
    State(gateway): State<Gateway>,
    UrlPath((id, token, path)): UrlPath<(String, String, String)>,
    Query(query): Query<FileQuery>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    certificates: Option<Extension<ClientCertificates>>,
    headers: HeaderMap,
) -> Response {
    let base = pages_base(&gateway);
    if top_level(&headers) {
        return match asked(&gateway, &id, &headers, (connection, certificates)).await {
            Ok(_) => see_other(&format!("{base}/p/{}/", percent(&id))),
            Err(refused) => *refused,
        };
    }
    let (store, page, viewer, limits, network) =
        match viewed(&gateway, &id, &token, &headers, (connection, certificates)).await {
            Ok(viewed) => viewed,
            Err(refused) => return *refused,
        };
    let csp = page_csp(&base, network.tighter(limits.network), limits.scripts);
    let Some(mut served) = serve_file(&store, &page, &path, query.source.is_some(), &csp) else {
        store.note(
            &viewing(&page.id, &viewer.id),
            &format!("{path} is not a file of the page"),
        );
        return gone();
    };
    credentialed(&headers, served.headers_mut());
    served
}

/// Lets the page's own script read the answer to a request it made with its credentials: the
/// page is an opaque origin, which sends `Origin: null`, and `*` admits no credentialed request.
fn credentialed(asked: &HeaderMap, answer: &mut HeaderMap) {
    if asked.get(ORIGIN).is_some_and(|origin| origin == "null") {
        answer.insert(
            HeaderName::from_static("access-control-allow-origin"),
            HeaderValue::from_static("null"),
        );
        answer.insert(
            HeaderName::from_static("access-control-allow-credentials"),
            HeaderValue::from_static("true"),
        );
    }
}

/// One listed file of `page`, by its format, under `csp`; `None` when there is no such file.
fn serve_file(
    store: &PageStore,
    page: &Page,
    path: &str,
    source: bool,
    csp: &str,
) -> Option<Response> {
    let bytes = store.read_file(page, path)?;
    let mut headers = page_headers(csp);
    let content_type = |value: &'static str| HeaderValue::from_static(value);
    let body: Vec<u8> = match classify(path) {
        Serve::Html => {
            headers.insert(CONTENT_TYPE, content_type("text/html; charset=utf-8"));
            bytes
        }
        Serve::Markdown if source => {
            headers.insert(CONTENT_TYPE, content_type("text/plain; charset=utf-8"));
            bytes
        }
        Serve::Markdown => {
            let text = String::from_utf8(bytes).ok()?;
            headers.insert(CONTENT_TYPE, content_type("text/html; charset=utf-8"));
            markdown_page(&page.title, &text).into_bytes()
        }
        Serve::Text(kind) | Serve::Asset(kind) => {
            headers.insert(CONTENT_TYPE, content_type(kind));
            bytes
        }
        Serve::Attachment => {
            headers.insert(CONTENT_TYPE, content_type("application/octet-stream"));
            let name: String = path
                .rsplit('/')
                .next()
                .unwrap_or("download")
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                .collect();
            if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
                headers.insert(CONTENT_DISPOSITION, value);
            }
            bytes
        }
    };
    Some((headers, body).into_response())
}

/// `GET /p/{id}/{token}/data/{name}`: a feed of the page, run as its viewer. A page's script
/// reads it with `fetch(url, {credentials: "include"})`, which sends the page cookie from the
/// page's opaque origin (`Origin: null`); the answer lets that script read it.
async fn feed(
    State(gateway): State<Gateway>,
    UrlPath((id, token, name)): UrlPath<(String, String, String)>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    certificates: Option<Extension<ClientCertificates>>,
    headers: HeaderMap,
) -> Response {
    let (store, page, viewer, limits, network) =
        match viewed(&gateway, &id, &token, &headers, (connection, certificates)).await {
            Ok(viewed) => viewed,
            Err(refused) => return *refused,
        };
    let key = viewing(&page.id, &viewer.id);
    let base = pages_base(&gateway);
    let mut answer_headers = page_headers(&page_csp(&base, network, limits.scripts));
    credentialed(&headers, &mut answer_headers);
    let reply = |status: StatusCode, body: Value| {
        (status, answer_headers.clone(), Json(body)).into_response()
    };
    if !limits.feeds || network.tighter(limits.network) == PageNetwork::None {
        return reply(
            StatusCode::FORBIDDEN,
            json!({"error": "this page may not read live data"}),
        );
    }
    let Some(feed) = page.feeds.iter().find(|feed| feed.name == name) else {
        store.note(&key, &format!("{name} is not a feed of the page"));
        return gone();
    };
    if !store.note_read(&key, limits.feed_reads_per_minute.or_max()) {
        return reply(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": "too many reads this minute"}),
        );
    }
    let stand_in = gateway.stand_in(&viewer.id);
    match tools::read(&gateway, stand_in.token(), &feed.tool, &feed.input).await {
        Ok(tools::Fetched::Value(data)) => {
            reply(StatusCode::OK, json!({"data": data, "at": now()}))
        }
        Ok(tools::Fetched::Guide(_)) => reply(StatusCode::NOT_FOUND, json!({"error": "no data"})),
        Err(failed) => match failed.status {
            Some(StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED) => reply(
                StatusCode::FORBIDDEN,
                json!({"error": "not allowed to read this"}),
            ),
            Some(StatusCode::NOT_FOUND) => {
                reply(StatusCode::NOT_FOUND, json!({"error": "not found"}))
            }
            _ => reply(StatusCode::BAD_GATEWAY, json!({"error": "the read failed"})),
        },
    }
}

/// The page a public link opens, while public links are allowed, with the live
/// `[assistant.pages]`.
async fn public_page(gateway: &Gateway, token: &str) -> Option<(Page, AssistantPageLimits)> {
    let store = gateway.pages.clone()?;
    let limits = page_limits(gateway).await?;
    if !limits.public {
        return None;
    }
    Some((store.public(token)?, limits))
}

/// `GET /s/{token}/`: a public link's frame, for anyone holding it; its bar says it is public.
async fn public_frame(State(gateway): State<Gateway>, UrlPath(token): UrlPath<String>) -> Response {
    let Some((page, limits)) = public_page(&gateway, &token).await else {
        return gone();
    };
    let mut response = frame_html(&page, &Viewing::Public, "f/", &limits).into_response();
    *response.headers_mut() = frame_headers(&pages_base(&gateway));
    response
}

/// `GET /s/{token}/f/{path}`: a file of a public page, under the deployment's network profile.
/// A public link has no feed: nobody signed in to read with.
async fn public_file(
    State(gateway): State<Gateway>,
    UrlPath((token, path)): UrlPath<(String, String)>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Response {
    let Some((page, limits)) = public_page(&gateway, &token).await else {
        return gone();
    };
    let Some(store) = gateway.pages.clone() else {
        return gone();
    };
    let base = pages_base(&gateway);
    if top_level(&headers) {
        return see_other(&format!("{base}/s/{token}/"));
    }
    let csp = page_csp(&base, limits.network, limits.scripts);
    let Some(mut served) = serve_file(&store, &page, &path, query.source.is_some(), &csp) else {
        return gone();
    };
    credentialed(&headers, served.headers_mut());
    served
}

/// Checks the declared feeds of a dashboard: each a read tool with input its schema accepts, at
/// most `limits.max_feeds`, distinct names.
pub(crate) fn check_feeds(feeds: &[Feed], limits: &AssistantPageLimits) -> Result<(), String> {
    if !limits.feeds {
        return Err("this deployment turns live feeds off".to_owned());
    }
    if !limits.max_feeds.allows(feeds.len()) {
        return Err(format!(
            "a dashboard may declare {} feeds",
            limits.max_feeds
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for feed in feeds {
        let name_ok = !feed.name.is_empty()
            && feed.name.len() <= 64
            && feed
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        if !name_ok || !seen.insert(feed.name.as_str()) {
            return Err(format!(
                "feed name {:?} is not a distinct name of letters, digits, - or _",
                feed.name
            ));
        }
        let Some(tool) = tools::read_tools()
            .iter()
            .find(|tool| tool.name == feed.tool && tool.name != "field_guide")
        else {
            return Err(format!(
                "{:?} is not a read tool a feed may use; feeds read, they never propose or send",
                feed.tool
            ));
        };
        tools::check(&tool.schema(), &feed.input, "input")
            .map_err(|problem| format!("feed {}: {problem}", feed.name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
