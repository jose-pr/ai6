//! Pages Moneypenny publishes and how they are served.
//!
//! A page is what she makes for the person to open in a browser tab outside Outer Office: a
//! dashboard reading live data, a report (a snapshot) or one file. The gateway keeps pages in
//! a [`PageStore`] (a directory, so remembered pages outlive a restart), serves each to the
//! person and whom they share it with through a link of their own (`{base}/v/{token}/`) that
//! opens only in the browser that first asks for it, or to anyone through a public link
//! (`{base}/s/{token}/`) the person makes, and never lets a page act as the app: every page
//! response carries a CSP `sandbox` without `allow-same-origin`, so the page is an opaque
//! origin, and `/v1` answers no cross-origin request. A page is shown in the gateway's own
//! frame under a classification bar that says who made it and when it expires.
//!
//! Quotas and options are `[assistant.pages]` in the roster's `assistant` record, read at each
//! use, so an administrator's change applies to the next publish.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ai6::{AssistantPageFile, AssistantPageLimits, PageNetwork, Person};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, LOCATION,
    REFERRER_POLICY, SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use super::tools;
use crate::Gateway;

// ---------------------------------------------------------------------------------------
// What a page is
// ---------------------------------------------------------------------------------------

/// The scope every page audit record is made under.
pub(crate) const RESOURCE: &str = "config:assistant";

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
/// the read tool `tool` with the fixed `input` as the link's viewer.
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
    let total = seconds.max(0.0) as i64;
    let (days, rest) = (total.div_euclid(86_400), total.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3_600,
        rest % 3_600 / 60
    )
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// `bytes` as unpadded base64url.
fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | (u32::from(*byte) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(BASE64URL[(n >> (18 - 6 * i) & 63) as usize]));
        }
    }
    out
}

/// 32 random bytes as base64url: a capability token or a page ID suffix.
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    base64url(&bytes)
}

fn now() -> f64 {
    crate::unix_timestamp().unwrap_or(0.0)
}

// ---------------------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------------------

/// A link for one viewer: who may read which page, with what network, until when.
///
/// Its token, in the address bar, opens only the gateway's frame, and only in the browser its
/// first request bound with a cookie. The frame names the page's files and feeds under a second
/// token, `view`, which no address bar shows.
#[derive(Debug, Clone)]
pub(crate) struct Link {
    pub(crate) page: String,
    pub(crate) viewer: String,
    /// The viewer's access token, with which the sign-in is checked and feeds run; held in
    /// memory only.
    pub(crate) bearer: String,
    pub(crate) network: PageNetwork,
    pub(crate) expires: f64,
    /// The address-bar token, to send a file asked for on its own back to the frame.
    pub(crate) token: String,
    /// The token of the page's own files and feeds.
    pub(crate) view: String,
    /// The SHA-256 of the cookie the first request set; `None` until then.
    pub(crate) cookie: Option<String>,
}

/// What a request for the frame is, against the link's cookie.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admit {
    /// The first request: the link is bound to this cookie, which the answer sets.
    First(String),
    /// A later request carrying the cookie.
    Again,
    /// A request without it: answered as a link that does not exist.
    Refused,
}

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
    /// Links by the SHA-256 of their token.
    links: HashMap<String, Link>,
    /// The key of each link in `links` by the SHA-256 of its view token.
    views: HashMap<String, String>,
    /// Feed reads in the current minute, by link.
    reads: HashMap<String, (Instant, u32)>,
}

/// The pages on disk and the links, uploads and counters in memory.
pub(crate) struct PageStore {
    dir: PathBuf,
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
    /// Opens `dir`, created private (`0700` on Unix), reading each page's record.
    ///
    /// # Errors
    ///
    /// The directory cannot be made or read.
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
        Ok(Self {
            dir,
            state: Mutex::new(Inner {
                pages,
                ..Inner::default()
            }),
            staged: Notify::new(),
            sweeping: std::sync::atomic::AtomicBool::new(false),
        })
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

    /// Each viewer holding a link to `page`, with the access token the link was made with.
    pub(crate) fn viewers(&self, page: &str) -> Vec<(String, String)> {
        let mut viewers: Vec<(String, String)> = self
            .state()
            .links
            .values()
            .filter(|link| link.page == page)
            .map(|link| (link.viewer.clone(), link.bearer.clone()))
            .collect();
        viewers.sort();
        viewers.dedup();
        viewers
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

    /// Removes a page and every link to it.
    pub(crate) fn delete(&self, id: &str) -> Option<Page> {
        let mut state = self.state();
        let page = state.pages.remove(id)?;
        Self::retain_links(&mut state, |link| link.page != id);
        let _ = std::fs::remove_dir_all(self.dir.join(id));
        Some(page)
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

    /// Deletes expired pages, expired links and stale uploads; answers the pages deleted.
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
        let pages: std::collections::BTreeSet<String> = state.pages.keys().cloned().collect();
        Self::retain_links(&mut state, |link| {
            link.expires > at && pages.contains(&link.page)
        });
        state
            .staged
            .retain(|_, staged| staged.at.elapsed() < stage_ttl);
        gone
    }

    // ---- links

    /// A new link for `viewer` to `page`, valid `ttl` or until the page goes; answers the
    /// token and when it ends.
    pub(crate) fn mint(
        &self,
        page: &Page,
        viewer: &str,
        bearer: &str,
        network: PageNetwork,
        ttl: Duration,
    ) -> (String, f64) {
        let (token, view) = (random_token(), random_token());
        let mut expires = now() + ttl.as_secs_f64();
        if let Some(end) = page.expires_at {
            expires = expires.min(end);
        }
        let key = sha256_hex(token.as_bytes());
        let mut state = self.state();
        state.views.insert(sha256_hex(view.as_bytes()), key.clone());
        state.links.insert(
            key,
            Link {
                page: page.id.clone(),
                viewer: viewer.to_owned(),
                bearer: bearer.to_owned(),
                network,
                expires,
                token: token.clone(),
                view,
                cookie: None,
            },
        );
        (token, expires)
    }

    /// The link under `key` and its page while both stand.
    fn standing_link(state: &Inner, key: &str) -> Option<(Link, Page)> {
        let at = now();
        let link = state.links.get(key)?;
        let page = state.pages.get(&link.page)?;
        if link.expires <= at || page.expires_at.is_some_and(|end| end <= at) {
            return None;
        }
        Some((link.clone(), page.clone()))
    }

    /// The link whose address-bar token is `token`, and its page, while both stand.
    pub(crate) fn link(&self, token: &str) -> Option<(Link, Page)> {
        Self::standing_link(&self.state(), &sha256_hex(token.as_bytes()))
    }

    /// The link whose view token is `view`, and its page, while both stand.
    pub(crate) fn viewed(&self, view: &str) -> Option<(Link, Page)> {
        let state = self.state();
        let key = state.views.get(&sha256_hex(view.as_bytes()))?;
        Self::standing_link(&state, key)
    }

    /// Admits a request for the frame of link `token` carrying `cookies`, the values of every
    /// page cookie it sent: the first request binds the link to a new cookie, and every later
    /// one must carry it.
    pub(crate) fn admit(&self, token: &str, cookies: &[&str]) -> Admit {
        let mut state = self.state();
        let Some(link) = state.links.get_mut(&sha256_hex(token.as_bytes())) else {
            return Admit::Refused;
        };
        match &link.cookie {
            None => {
                let cookie = random_token();
                link.cookie = Some(sha256_hex(cookie.as_bytes()));
                Admit::First(cookie)
            }
            Some(bound)
                if cookies
                    .iter()
                    .any(|cookie| sha256_hex(cookie.as_bytes()) == *bound) =>
            {
                Admit::Again
            }
            Some(_) => Admit::Refused,
        }
    }

    /// Keeps only the links `keep` accepts, with their view tokens and read counts.
    fn retain_links(state: &mut Inner, keep: impl Fn(&Link) -> bool) {
        state.links.retain(|_, link| keep(link));
        let links = &state.links;
        state.views.retain(|_, key| links.contains_key(key));
        state.reads.retain(|key, _| links.contains_key(key));
    }

    /// Drops every link to `page` held by a viewer who no longer qualifies.
    pub(crate) fn revoke_viewers(&self, page: &str, keep: &dyn Fn(&str) -> bool) {
        Self::retain_links(&mut self.state(), |link| {
            link.page != page || keep(&link.viewer)
        });
    }

    /// Counts a feed read on the link viewed through `view`; false past `most` in the minute.
    pub(crate) fn note_read(&self, view: &str, most: usize) -> bool {
        let mut state = self.state();
        let Some(key) = state.views.get(&sha256_hex(view.as_bytes())).cloned() else {
            return false;
        };
        let entry = state.reads.entry(key).or_insert((Instant::now(), 0));
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
    // The opaque page loads and fetches its own files: the capability in the URL guards them.
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

/// The path of the pages' base, `""` on an origin of their own: where a link's cookie applies.
fn base_path(base: &str) -> &str {
    &base[origin_of(base).len()..]
}

/// The routes of pages. `/v/{token}/` is a viewer's frame, bound to the browser that first
/// asked for it; `/v/{view}/f/{path}` and `/v/{view}/data/{name}` are the page's files and feeds
/// under the view token the frame names. `/s/{token}/` and `/s/{token}/f/{path}` are a public
/// link's frame and files, with no feed. Nothing of `/v1`.
pub(crate) fn pages_router() -> Router<Gateway> {
    Router::new()
        .route("/v/{token}/", get(frame))
        .route("/v/{token}/f/{*path}", get(file))
        .route("/v/{token}/data/{name}", get(feed))
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

/// Whether the viewer of `link` is still signed in and still may read `page`, with the live
/// `[assistant.pages]`; a viewer who may not loses every link to it.
async fn stands_for(gateway: &Gateway, link: &Link, page: &Page) -> Option<AssistantPageLimits> {
    let store = gateway.pages.clone()?;
    let viewer = crate::resolve_user(gateway, &link.bearer).await.ok()?;
    let limits = page_limits(gateway).await?;
    if viewer.id != link.viewer || !may_view(gateway, page, &viewer, &limits).await {
        store.revoke_viewers(&page.id, &|held| held != link.viewer);
        return None;
    }
    Some(limits)
}

/// The cookie that binds a viewer's link to the browser that first asked for it.
const VIEW_COOKIE: &str = "ai6_page";

/// The value of every page cookie a request carries; a client speaking HTTP/2 may split
/// `Cookie` over several headers.
fn view_cookies(headers: &HeaderMap) -> Vec<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|line| line.to_str().ok())
        .flat_map(|line| line.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(name, _)| *name == VIEW_COOKIE)
        .map(|(_, value)| value)
        .collect()
}

/// The `Set-Cookie` that binds link `token` to this browser: sent only to that link's frame,
/// never to a script, never from another site, and gone when the link ends.
fn view_cookie(base: &str, token: &str, value: &str, expires: f64) -> Option<HeaderValue> {
    let seconds = (expires - now()).ceil().max(1.0);
    HeaderValue::from_str(&format!(
        "{VIEW_COOKIE}={value}; Path={}/v/{token}/; HttpOnly; Secure; SameSite=Strict; \
         Max-Age={seconds:.0}",
        base_path(base)
    ))
    .ok()
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

/// `GET /v/{token}/`: the frame of a viewer's link. Its first request binds the link to this
/// browser with a cookie and audits the opening; any later request without that cookie, as
/// from a copied address, is answered as a link that does not exist.
async fn frame(
    State(gateway): State<Gateway>,
    UrlPath(token): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some(store) = gateway.pages.clone() else {
        return gone();
    };
    let Some((link, page)) = store.link(&token) else {
        return gone();
    };
    let cookies = view_cookies(&headers);
    if let Some(bound) = &link.cookie
        && !cookies
            .iter()
            .any(|cookie| sha256_hex(cookie.as_bytes()) == *bound)
    {
        return gone();
    }
    let Some(limits) = stands_for(&gateway, &link, &page).await else {
        return gone();
    };
    let base = pages_base(&gateway);
    let set_cookie = match store.admit(&token, &cookies) {
        Admit::Refused => return gone(),
        Admit::Again => None,
        Admit::First(cookie) => {
            audit_opened(&gateway, &link, &page).await;
            view_cookie(&base, &token, &cookie, link.expires)
        }
    };
    let src = format!("../{}/f/", link.view);
    let mut response = frame_html(&page, &Viewing::Link(&link), &src, &limits).into_response();
    *response.headers_mut() = frame_headers(&base);
    if let Some(set_cookie) = set_cookie {
        response.headers_mut().insert(SET_COOKIE, set_cookie);
    }
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

/// Audits a link's first request, as its viewer.
async fn audit_opened(gateway: &Gateway, link: &Link, page: &Page) {
    if let Ok(person) = crate::resolve_user(gateway, &link.bearer).await {
        let _ = super::audit(
            gateway,
            &person,
            "moneypenny.page.opened",
            String::new(),
            Some(RESOURCE.to_owned()),
            &format!("{} ({})", page.id, link.network.as_str()),
        )
        .await;
    }
}

/// Whom a frame is shown to.
enum Viewing<'a> {
    /// A signed-in viewer, through their link.
    Link(&'a Link),
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
        Viewing::Link(link) => {
            let mut note = format!(
                "Made by Moneypenny for {}, not part of Outer Office. {what} {expiry}",
                escape(&page.person_id),
            );
            if link.viewer != page.person_id {
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

/// `GET /v/{view}/f/{path}`: a file of the page, under the view token its frame names. A
/// top-level request is sent to the frame, which carries the bar and needs the browser's
/// cookie, so a view token in an address bar opens nothing elsewhere.
async fn file(
    State(gateway): State<Gateway>,
    UrlPath((view, path)): UrlPath<(String, String)>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(store) = gateway.pages.clone() else {
        return gone();
    };
    let Some((link, page)) = store.viewed(&view) else {
        return gone();
    };
    let Some(limits) = stands_for(&gateway, &link, &page).await else {
        return gone();
    };
    let base = pages_base(&gateway);
    if top_level(&headers) {
        return see_other(&format!("{base}/v/{}/", link.token));
    }
    let csp = page_csp(&base, link.network.tighter(limits.network), limits.scripts);
    serve_file(&store, &page, &path, query.source.is_some(), &csp)
}

/// One listed file of `page`, by its format, under `csp`.
fn serve_file(store: &PageStore, page: &Page, path: &str, source: bool, csp: &str) -> Response {
    let Some(bytes) = store.read_file(page, path) else {
        return gone();
    };
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
        Serve::Markdown => match String::from_utf8(bytes) {
            Ok(text) => {
                headers.insert(CONTENT_TYPE, content_type("text/html; charset=utf-8"));
                markdown_page(&page.title, &text).into_bytes()
            }
            Err(_) => return gone(),
        },
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
    (headers, body).into_response()
}

/// `GET /v/{view}/data/{name}`: a feed of the page, run as the link's viewer.
async fn feed(
    State(gateway): State<Gateway>,
    UrlPath((view, name)): UrlPath<(String, String)>,
) -> Response {
    let Some(store) = gateway.pages.clone() else {
        return gone();
    };
    let Some((link, page)) = store.viewed(&view) else {
        return gone();
    };
    let Some(limits) = stands_for(&gateway, &link, &page).await else {
        return gone();
    };
    let base = pages_base(&gateway);
    let headers = page_headers(&page_csp(&base, link.network, limits.scripts));
    let reply =
        |status: StatusCode, body: Value| (status, headers.clone(), Json(body)).into_response();
    if !limits.feeds || link.network.tighter(limits.network) == PageNetwork::None {
        return reply(
            StatusCode::FORBIDDEN,
            json!({"error": "this page may not read live data"}),
        );
    }
    let Some(feed) = page.feeds.iter().find(|feed| feed.name == name) else {
        return gone();
    };
    if !store.note_read(&view, limits.feed_reads_per_minute.or_max()) {
        return reply(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": "too many reads this minute"}),
        );
    }
    match tools::read(&gateway, &link.bearer, &feed.tool, &feed.input).await {
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
    serve_file(&store, &page, &path, query.source.is_some(), &csp)
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
