//! Pages Moneypenny publishes and how they are served.
//!
//! A page is what she makes for the person to open in a browser tab outside Outer Office: a
//! dashboard reading live data, a report (a snapshot) or one file. The gateway keeps pages in
//! a [`PageStore`] (a directory, so remembered pages outlive a restart), serves each through
//! a capability link (`{base}/v/{token}/`) and never lets a page act as the app: every page
//! response carries a CSP `sandbox` without `allow-same-origin`, so the page is an opaque
//! origin, and `/v1` answers no cross-origin request. A page is shown in the gateway's own
//! frame under a classification bar that says who made it, what she read and when it expires.
//!
//! Quotas and options are `[assistant.pages]` in the roster's `assistant` record, read at each
//! use, so an administrator's change applies to the next publish.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ai6::{AssistantPageFile, AssistantPageLimits, PageNetwork};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION,
    REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
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
/// the read tool `tool` with the fixed `input` as whoever holds the link.
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
    /// What she read before she made it, in order.
    pub(crate) exposed_by: Vec<String>,
    pub(crate) exposed: bool,
    pub(crate) feeds: Vec<Feed>,
    /// Subjects the person shares it with.
    #[serde(default)]
    pub(crate) shares: Vec<String>,
}

impl Page {
    /// How the app and her `pages` tool list it.
    pub(crate) fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind.as_str(),
            "title": self.title,
            "bytes": self.bytes,
            "expires_at": self.expires_at,
            "exposed": self.exposed,
            "exposed_by": self.exposed_by,
            "feeds": self.feeds.iter().map(|feed| feed.name.as_str()).collect::<Vec<_>>(),
        })
    }
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

/// A capability link: who may read which page, with what network, until when.
#[derive(Debug, Clone)]
pub(crate) struct Link {
    pub(crate) page: String,
    pub(crate) viewer: String,
    /// The viewer's access token, with which feeds run; held in memory only.
    pub(crate) bearer: String,
    /// The access token's digest, to see that the sign-in still stands.
    pub(crate) access: String,
    pub(crate) network: PageNetwork,
    pub(crate) expires: f64,
    /// Whether the first request was audited.
    pub(crate) opened: bool,
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
        state.links.retain(|_, link| link.page != id);
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
        let pages = &state.pages;
        let live: Vec<String> = state
            .links
            .iter()
            .filter(|(_, link)| link.expires > at && pages.contains_key(&link.page))
            .map(|(key, _)| key.clone())
            .collect();
        state.links.retain(|key, _| live.contains(key));
        state.reads.retain(|key, _| live.contains(key));
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
        access: &str,
        network: PageNetwork,
        ttl: Duration,
    ) -> (String, f64) {
        let token = random_token();
        let mut expires = now() + ttl.as_secs_f64();
        if let Some(end) = page.expires_at {
            expires = expires.min(end);
        }
        self.state().links.insert(
            sha256_hex(token.as_bytes()),
            Link {
                page: page.id.clone(),
                viewer: viewer.to_owned(),
                bearer: bearer.to_owned(),
                access: access.to_owned(),
                network,
                expires,
                opened: false,
            },
        );
        (token, expires)
    }

    /// The link for `token` and its page while both stand.
    pub(crate) fn link(&self, token: &str) -> Option<(Link, Page)> {
        let key = sha256_hex(token.as_bytes());
        let at = now();
        let state = self.state();
        let link = state.links.get(&key)?;
        let page = state.pages.get(&link.page)?;
        if link.expires <= at || page.expires_at.is_some_and(|end| end <= at) {
            return None;
        }
        Some((link.clone(), page.clone()))
    }

    /// Marks the link opened; true the first time.
    pub(crate) fn first_open(&self, token: &str) -> bool {
        let key = sha256_hex(token.as_bytes());
        let mut state = self.state();
        match state.links.get_mut(&key) {
            Some(link) if !link.opened => {
                link.opened = true;
                true
            }
            _ => false,
        }
    }

    /// Drops every link to `page` held by a viewer who no longer qualifies.
    pub(crate) fn revoke_viewers(&self, page: &str, keep: &dyn Fn(&str) -> bool) {
        self.state()
            .links
            .retain(|_, link| link.page != page || keep(&link.viewer));
    }

    /// Counts a feed read on `token`; false past `most` in the minute.
    pub(crate) fn note_read(&self, token: &str, most: usize) -> bool {
        let key = sha256_hex(token.as_bytes());
        let mut state = self.state();
        let entry = state.reads.entry(key).or_insert((Instant::now(), 0));
        if entry.0.elapsed() >= Duration::from_secs(60) {
            *entry = (Instant::now(), 0);
        }
        entry.1 += 1;
        usize::try_from(entry.1).is_ok_and(|count| ai6::Limit::at(most as u64).allows(count))
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
    ) -> Result<Claimed, String> {
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

/// The files of a claimed upload and its manifest.
pub(crate) type Claimed = (BTreeMap<String, Vec<u8>>, Vec<AssistantPageFile>);

/// The files a Station sent, held to the manifest it ended with: each listed file arrived with
/// its size and SHA-256, and nothing else did.
fn verify(
    files: BTreeMap<String, Vec<u8>>,
    manifest: Vec<AssistantPageFile>,
) -> Result<Claimed, String> {
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
    Ok((files, manifest))
}

// ---------------------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------------------

/// The CSP of every page response for `network` and `scripts`; `base` is the pages' base, the
/// only origin that may frame a page. The sandbox never has `allow-same-origin` or
/// `allow-top-navigation*`, so a page is an opaque origin and cannot reach its host.
pub(crate) fn page_csp(base: &str, network: PageNetwork, scripts: bool) -> String {
    let mut csp = String::from(
        "sandbox allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals",
    );
    if scripts {
        csp.push_str(" allow-scripts");
    }
    let _ = write!(csp, "; frame-ancestors {base}");
    match network {
        PageNetwork::Open => {}
        _ => csp.push_str(
            "; default-src 'self' data: blob: 'unsafe-inline' 'unsafe-eval'; connect-src 'self' data: blob:",
        ),
    }
    csp
}

/// The CSP of the gateway's frame document: no script, only what is under the pages' base
/// framed (`frame-src` takes a path as a prefix only with a trailing slash; the page's own
/// `frame-ancestors` ignores paths).
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

/// The routes of pages: `/v/{token}/` (the frame), `/v/{token}/f/{path}` (a file) and
/// `/v/{token}/data/{name}` (a feed). Nothing of `/v1`.
pub(crate) fn pages_router() -> Router<Gateway> {
    Router::new()
        .route("/v/{token}/", get(frame))
        .route("/v/{token}/f/{*path}", get(file))
        .route("/v/{token}/data/{name}", get(feed))
}

fn gone() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// Whether `viewer` may read `page`: its person, and nobody else; an administrator included.
pub(crate) async fn may_view(gateway: &Gateway, page: &Page, viewer: &str) -> bool {
    let _ = gateway;
    page.person_id == viewer
}

/// A link that stands: not expired, its page there, and its viewer still signed in and still
/// allowed to read the page. Also the live `[assistant.pages]`.
async fn standing(gateway: &Gateway, token: &str) -> Option<(Link, Page, AssistantPageLimits)> {
    let store = gateway.pages.clone()?;
    let (link, page) = store.link(token)?;
    let registry = gateway.registry.clone()?;
    let access = link.access.clone();
    let signed_in = tokio::task::spawn_blocking(move || registry.find_access_token(&access))
        .await
        .ok()?
        .ok()??;
    let _ = signed_in;
    let limits = super::assistant_settings(gateway)
        .await
        .ok()?
        .pages()
        .clone();
    if !may_view(gateway, &page, &link.viewer).await {
        store.revoke_viewers(&page.id, &|viewer| viewer != link.viewer);
        return None;
    }
    Some((link, page, limits))
}

async fn frame(State(gateway): State<Gateway>, UrlPath(token): UrlPath<String>) -> Response {
    let Some((link, page, limits)) = standing(&gateway, &token).await else {
        return gone();
    };
    let base = pages_base(&gateway);
    first_open(&gateway, &token, &link, &page).await;
    let mut response = frame_html(&page, &link, &limits).into_response();
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

/// Audits a link's first request, as the viewer.
async fn first_open(gateway: &Gateway, token: &str, link: &Link, page: &Page) {
    let Some(store) = gateway.pages.clone() else {
        return;
    };
    if !store.first_open(token) {
        return;
    }
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

/// The frame: the classification bar above an iframe of the page's entry.
fn frame_html(page: &Page, link: &Link, limits: &AssistantPageLimits) -> String {
    let mut note = format!(
        "Made by Moneypenny for {}, not part of Outer Office. {} <b>{}</b>. {}",
        escape(&page.person_id),
        escape(page.kind.as_str()),
        escape(&page.title),
        match page.expires_at {
            Some(end) => format!("Expires {}.", escape(&utc_text(end))),
            None => "Remembered.".to_owned(),
        }
    );
    if link.viewer != page.person_id {
        let _ = write!(note, " Shared by {}.", escape(&page.person_id));
    }
    if page.exposed {
        let shown: Vec<String> = page
            .exposed_by
            .iter()
            .take(5)
            .map(|source| escape(source))
            .collect();
        let more = page.exposed_by.len().saturating_sub(5);
        let _ = write!(
            note,
            " <strong>Made after she read text from outside AI6:</strong> {}{}.",
            shown.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        );
    }
    let sandbox = frame_sandbox(limits.scripts);
    let sources = String::new();
    let _ = sources;
    let entry = page
        .entry
        .split('/')
        .map(percent)
        .collect::<Vec<_>>()
        .join("/");
    let class = if page.exposed { "bar warn" } else { "bar" };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style>\
html,body{{height:100%;margin:0}}body{{display:flex;flex-direction:column;font:14px system-ui,sans-serif}}\
.bar{{background:#e8eef7;color:#10233f;padding:.45rem .8rem;display:flex;gap:.8rem;align-items:flex-start;\
border-bottom:1px solid #9fb3cf}}.bar.warn{{background:#ffe9b3;color:#4a3000;border-color:#d9a400}}\
.bar span{{flex:1}}.bar label{{cursor:pointer;font-weight:700;padding:0 .4rem}}\
#c{{position:absolute;opacity:0;pointer-events:none}}#c:checked~.bar{{display:none}}\
iframe{{flex:1;border:0;width:100%;background:#fff}}</style></head><body>\
<input type=\"checkbox\" id=\"c\" aria-label=\"Hide the notice\">\
<div class=\"{class}\" role=\"note\"><span>{note}</span><label for=\"c\" title=\"Hide for this view\">&#10005;</label></div>\
<iframe src=\"f/{entry}\" sandbox=\"{sandbox}\" allow=\"fullscreen\" referrerpolicy=\"no-referrer\" \
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

async fn file(
    State(gateway): State<Gateway>,
    UrlPath((token, path)): UrlPath<(String, String)>,
    Query(query): Query<FileQuery>,
    request: Request,
) -> Response {
    let Some((link, page, limits)) = standing(&gateway, &token).await else {
        return gone();
    };
    let headers = request.headers();
    let base = pages_base(&gateway);
    // A top-level request for a file is sent to the frame, which carries the bar; the framed
    // request is a `Sec-Fetch-Dest: iframe` and a page's own loads are other destinations.
    let top_level = headers
        .get("sec-fetch-dest")
        .and_then(|value| value.to_str().ok())
        == Some("document");
    let frame_wanted = page.exposed || limits.frame;
    if top_level && frame_wanted {
        let mut response = StatusCode::SEE_OTHER.into_response();
        if let Ok(to) = HeaderValue::from_str(&format!("{base}/v/{token}/")) {
            response.headers_mut().insert(LOCATION, to);
        }
        return response;
    }
    let Some(store) = gateway.pages.clone() else {
        return gone();
    };
    let Some(bytes) = store.read_file(&page, &path) else {
        return gone();
    };
    first_open(&gateway, &token, &link, &page).await;
    let profile = link.network.tighter(limits.network);
    let mut headers = page_headers(&page_csp(&base, profile, limits.scripts));
    let content_type = |value: &'static str| HeaderValue::from_static(value);
    let body: Vec<u8> = match classify(&path) {
        Serve::Html => {
            headers.insert(CONTENT_TYPE, content_type("text/html; charset=utf-8"));
            bytes
        }
        Serve::Markdown if query.source.is_some() => {
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

async fn feed(
    State(gateway): State<Gateway>,
    UrlPath((token, name)): UrlPath<(String, String)>,
) -> Response {
    let Some((link, page, limits)) = standing(&gateway, &token).await else {
        return gone();
    };
    let Some(store) = gateway.pages.clone() else {
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
    if !store.note_read(&token, limits.feed_reads_per_minute.or_max()) {
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
