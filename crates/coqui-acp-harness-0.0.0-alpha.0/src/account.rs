//! Applying an account to a harness started through a host's spawner: the credential files it keeps in the account's
//! directory, watched for a renewal or a loss, and the keepers that hear of it.
//!
//! One [`Accounts`] lives as long as the provider. A call that runs the harness on an account takes a [`Hold`]: the
//! files of the grant are placed in the directory (only when the rules below say so), the call's keeper is
//! registered, and every look at the files, by this hold or any other on the account, that finds them changed is one
//! event with one sequence, told to every keeper then live for the account.
//!
//! The rules for placing a file, per watched file:
//! - absent, with no record of it or no hold on it now: the grant's copy is written;
//! - planted (longer than [`MAX_FILE_BYTES`], a link, not a regular file): removed, and the grant's copy written;
//! - there with no record: the same text is kept; a login changed since the provider began (by the spawner's clock) is
//!   news (a renewal, told and kept); a blank or older one is replaced by the grant's copy;
//! - there with a record and no other hold: the file last recorded is kept unless the grant is a newer revision with
//!   other credentials; a login changed since the last look is news; a blank or older one is replaced by the grant's
//!   copy (so a loss, once reported, is not reported again by the next hold);
//! - there with a record and another hold on it: kept (it may have been renewed since), unless the grant is newer.
//!
//! A change is read twice, a moment apart, and reported only when both reads agree: a file the harness is still
//! writing is not a renewal, and one it has just truncated is not a loss. A login that is missing or blank is `lost`,
//! once, and comes back as a renewal when a login appears again. A login that names another account than the
//! grant's (when the text names one) is refused as lost, and logged.
//!
//! **Keepers.** A keeper is called with no lock of the registry held, on the thread that found the change (a
//! connection's watcher, a call, a renewal). It may call back into the provider (`models`, `renew`, ending its own
//! hold or closing the connection): ending a hold waits for the calls to its keeper that other threads have in
//! flight, never for one on its own thread, and a watcher stopped from its own thread is not joined. A keeper that
//! blocks holds the thread that called it.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hash::Hasher;
use core::ops::Deref;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::collections::hash_map::DefaultHasher;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{JoinHandle, ThreadId};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{Secret, Status, Wipe};
use coqui_acp_api::{
    AccountKeeper, AccountLoss, AccountRenewal, Credentials, FileInfo, ProcessSpawner, SecretBytes,
};

/// The most a credential file may hold, in bytes.
pub const MAX_FILE_BYTES: u64 = 1 << 20;

/// The default of a provider's `renew_poll_ms`.
pub const DEFAULT_POLL_MS: u64 = 30_000;

/// How long a change waits before it is read again to see that the writer is done.
const SETTLE: Duration = Duration::from_millis(100);

/// How much older than the provider a file may be, in milliseconds, and still count as changed since it began: file
/// times are coarse (two seconds on some file systems) and two clocks never agree to the millisecond.
const CLOCK_SLACK_MS: i64 = 2000;

/// The file a hold writes to ask the spawner what time it is.
const CLOCK_FILE: &str = ".coqui-clock";

/// Text that is a credential: wiped when dropped, and `Debug` shows only its length.
pub struct SecretText(String);

impl SecretText {
    /// Keeps `text`.
    #[must_use]
    pub const fn new(text: String) -> Self {
        Self(text)
    }
}

impl Deref for SecretText {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Debug for SecretText {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[redacted {} bytes]", self.0.len())
    }
}

impl Drop for SecretText {
    fn drop(&mut self) {
        self.0.wipe();
    }
}

/// What a file holds.
#[derive(Debug)]
pub enum FileState {
    /// It is not there.
    Missing,
    /// Its text.
    Text(SecretText),
    /// Something is there that is not a file this provider may read: longer than [`MAX_FILE_BYTES`], a link, not a
    /// regular file.
    Planted,
}

/// The file methods of a host's spawner, shared with whoever else holds it.
///
/// The spawner is destroyed when the last holder lets go, so a connection's [`Hold`] drops its copy before the
/// connection reports that it has closed.
#[derive(Clone)]
pub struct Files {
    spawner: Arc<Object<dyn ProcessSpawner>>,
}

impl core::fmt::Debug for Files {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Files").finish_non_exhaustive()
    }
}

impl Files {
    /// The file methods of `spawner`.
    #[must_use]
    pub const fn new(spawner: Arc<Object<dyn ProcessSpawner>>) -> Self {
        Self { spawner }
    }

    /// Whether the spawner serves files at all.
    #[must_use]
    pub fn available(&self) -> bool {
        self.spawner.describe().is_ok_and(|info| info.files)
    }

    /// Writes `text` at `path` in the account's directory, replacing a file there, readable by the owner alone where
    /// the system lets.
    ///
    /// # Errors
    ///
    /// The spawner's refusal.
    pub fn write(&self, path: &str, text: &str) -> Result<(), ProviderError> {
        self.spawner.write_file(
            path,
            Secret::new(SecretBytes::new(text.as_bytes().to_vec())),
            true,
        )
    }

    /// What `path` holds. A file the spawner will not read (too long, a link, not a file) is [`FileState::Planted`].
    ///
    /// # Errors
    ///
    /// The spawner's refusal, other than the file being missing or one it will not read.
    pub fn state(&self, path: &str) -> Result<FileState, ProviderError> {
        match self.spawner.read_file(path, MAX_FILE_BYTES) {
            Ok(bytes) => Ok(FileState::Text(SecretText::new(bytes.expose().to_text()))),
            Err(error)
                if error.status == Status::GUATIAO_ERR_NOT_FOUND
                    || error.status == Status::GUATIAO_ERR_BAD_VALUE =>
            {
                // The spawner says not found for a missing file and for one it refuses: the file's info tells which.
                match self.spawner.file_info(path) {
                    Ok(None) => Ok(FileState::Missing),
                    _ => Ok(FileState::Planted),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// The text of `path`; `None` when it is not there or is not a file this provider may read.
    ///
    /// # Errors
    ///
    /// The spawner's refusal, other than the file being missing or one it will not read.
    pub fn read(&self, path: &str) -> Result<Option<SecretText>, ProviderError> {
        Ok(match self.state(path)? {
            FileState::Text(text) => Some(text),
            FileState::Missing | FileState::Planted => None,
        })
    }

    /// What is known of `path`; `None` when it is not there.
    ///
    /// # Errors
    ///
    /// The spawner's refusal.
    pub fn info(&self, path: &str) -> Result<Option<FileInfo>, ProviderError> {
        self.spawner.file_info(path)
    }

    /// Removes `path`; no error when it is not there.
    ///
    /// # Errors
    ///
    /// The spawner's refusal.
    pub fn remove(&self, path: &str) -> Result<(), ProviderError> {
        self.spawner.remove_file(path)
    }
}

/// One file that holds a login.
#[derive(Clone)]
#[non_exhaustive]
pub struct Watched {
    /// Where, relative to the account's directory.
    pub path: String,
    /// The key of the grant's credentials that holds the file's text.
    pub key: String,
    /// Whether a file's text holds a login; one that does not (blank, or a login the harness blanked) is lost.
    pub usable: fn(&str) -> bool,
    /// When the login in the text stops working, as an RFC 3339 time, when the text says.
    pub expires: fn(&str) -> Option<String>,
    /// Whom the login in the text is for, when it says; a renewal naming someone else than the grant is refused.
    pub identity: fn(&str) -> Option<String>,
}

impl core::fmt::Debug for Watched {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Watched")
            .field("path", &self.path)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl Watched {
    /// The file `path`, held by the credential `key`; usable when it is not blank.
    #[must_use]
    pub fn new(path: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            key: key.into(),
            usable: |text| !text.trim().is_empty(),
            expires: |_| None,
            identity: |_| None,
        }
    }

    /// The same file, usable as `usable` says.
    #[must_use]
    pub fn usable(mut self, usable: fn(&str) -> bool) -> Self {
        self.usable = usable;
        self
    }

    /// The same file, expiring as `expires` reads from its text.
    #[must_use]
    pub fn expires(mut self, expires: fn(&str) -> Option<String>) -> Self {
        self.expires = expires;
        self
    }

    /// The same file, naming whom its login is for as `identity` reads from its text.
    #[must_use]
    pub fn identity(mut self, identity: fn(&str) -> Option<String>) -> Self {
        self.identity = identity;
        self
    }
}

/// What the keepers of one hold are told through: destroyed when its hold ends.
struct Slot {
    state: Mutex<SlotState>,
    idle: Condvar,
}

struct SlotState {
    keeper: Option<Arc<Object<dyn AccountKeeper>>>,
    /// The threads that are inside a call to the keeper.
    calling: Vec<ThreadId>,
}

impl Slot {
    fn new(keeper: Option<Object<dyn AccountKeeper>>) -> Self {
        Self {
            state: Mutex::new(SlotState {
                keeper: keeper.map(Arc::new),
                calling: Vec::new(),
            }),
            idle: Condvar::new(),
        }
    }

    /// Calls the keeper with no lock held.
    fn call(&self, what: impl FnOnce(&Object<dyn AccountKeeper>) -> Result<(), ProviderError>) {
        let keeper = {
            let mut state = lock(&self.state);
            let Some(keeper) = state.keeper.clone() else {
                return;
            };
            state.calling.push(std::thread::current().id());
            keeper
        };
        let result = what(&keeper);
        drop(keeper);
        let mut state = lock(&self.state);
        let me = std::thread::current().id();
        if let Some(at) = state.calling.iter().position(|id| *id == me) {
            state.calling.swap_remove(at);
        }
        if result.is_err_and(|error| error.status == Status::GUATIAO_ERR_GONE) {
            state.keeper = None;
        }
        drop(state);
        self.idle.notify_all();
    }

    /// Takes the keeper away, waits for the calls other threads have in flight and destroys it.
    fn close(&self) {
        let mut state = lock(&self.state);
        let taken = state.keeper.take();
        let me = std::thread::current().id();
        while state.calling.iter().any(|id| *id != me) {
            state = self
                .idle
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(state);
        drop(taken);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A short fingerprint of `text` for telling whether it changed: it is not a hash to keep a secret by.
#[must_use]
pub fn fingerprint(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write(text.as_bytes());
    hasher.write_usize(text.len());
    hasher.finish()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or(0)
}

/// A time in milliseconds since the Unix epoch as an RFC 3339 time in UTC.
#[must_use]
pub fn rfc3339_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// When a JWT stops working (its `exp` claim), as an RFC 3339 time. The token is not verified, only read.
#[must_use]
pub fn jwt_expiry(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let exp = claims.get("exp")?.as_i64()?;
    exp.checked_mul(1000).map(rfc3339_ms)
}

/// What the provider knows of one watched file.
struct Record {
    /// The fingerprint of the login last recorded; `None` when the login is lost.
    fingerprint: Option<u64>,
    /// The revision of the newest grant that placed it.
    revision: u64,
    /// How many holds are on it now.
    live: usize,
}

#[derive(Default)]
struct Entry {
    sequence: u64,
    keepers: Vec<(u64, Arc<Slot>)>,
    next_token: u64,
    files: BTreeMap<(String, String), Record>,
    /// Held while the files of the account are read, compared and recorded, so an older reading never takes a later
    /// sequence.
    gate: Arc<Mutex<()>>,
}

/// What the provider knows of its accounts, for as long as it lives.
pub struct Accounts {
    began_ms: i64,
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl core::fmt::Debug for Accounts {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Accounts").finish_non_exhaustive()
    }
}

impl Default for Accounts {
    fn default() -> Self {
        Self::new()
    }
}

/// Something a look found.
enum Event {
    Renewed(Secret<Credentials>, Option<String>),
    Lost(String),
}

/// An event with its sequence, to be told to the keepers once no lock is held.
struct Pending {
    sequence: u64,
    slots: Vec<Arc<Slot>>,
    event: Event,
}

/// What a [`Hold::look`] found and told.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Looked {
    /// Each renewal found: the credentials now, whole, and when they expire.
    pub renewed: Vec<(Secret<Credentials>, Option<String>)>,
    /// Why each login found lost is.
    pub lost: Vec<String>,
}

impl Accounts {
    /// A registry that has seen nothing; files changed from now on are news.
    #[must_use]
    pub fn new() -> Self {
        Self {
            began_ms: now_ms(),
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    /// Holds the account `id` for a call or a connection: registers `keeper` (a call that has none, such as a renewal,
    /// hears nothing and tells the others), places `credentials`' files (see the module's rules) and answers the hold.
    ///
    /// `dir` is the account's directory as the host named it; every path of `watched` is relative to it.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_BAD_VALUE` when a watched file's key is not among the credentials; the spawner's refusal to read
    /// or write a file.
    #[expect(
        clippy::too_many_arguments,
        reason = "one call places everything a grant carries"
    )]
    pub fn hold(
        self: &Arc<Self>,
        files: Files,
        keeper: Option<Object<dyn AccountKeeper>>,
        id: &str,
        dir: &str,
        revision: u64,
        credentials: &Credentials,
        watched: Vec<Watched>,
    ) -> Result<Hold, ProviderError> {
        for file in &watched {
            if credentials.get(&file.key).is_none() {
                return Err(ProviderError::new(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    "a credential the account needs is missing",
                ));
            }
        }
        let listening = keeper.is_some();
        let slot = Arc::new(Slot::new(keeper));
        let token = {
            let mut entries = lock(&self.entries);
            let entry = entries.entry(id.to_string()).or_default();
            entry.next_token += 1;
            let token = entry.next_token;
            if listening {
                entry.keepers.push((token, Arc::clone(&slot)));
            }
            token
        };
        let hold = Hold(Arc::new(Inner {
            accounts: Arc::clone(self),
            id: id.to_string(),
            dir: dir.to_string(),
            revision,
            credentials: Mutex::new(Secret::new(credentials.clone())),
            watched,
            files: Mutex::new(Some(files)),
            slot,
            token,
            placed: AtomicBool::new(false),
            done: AtomicBool::new(false),
        }));
        // A failure leaves the hold to drop, which lets go of everything it took.
        hold.0.place()?;
        Ok(hold)
    }

    fn gate(&self, id: &str) -> Arc<Mutex<()>> {
        Arc::clone(&lock(&self.entries).entry(id.to_string()).or_default().gate)
    }

    /// Gives `event` the account's next sequence, and the keepers it is to be told to.
    fn assign(&self, id: &str, event: Event) -> Pending {
        let mut entries = lock(&self.entries);
        let entry = entries.entry(id.to_string()).or_default();
        entry.sequence += 1;
        Pending {
            sequence: entry.sequence,
            slots: entry
                .keepers
                .iter()
                .map(|(_, slot)| Arc::clone(slot))
                .collect(),
            event,
        }
    }

    /// Tells the keepers, with no lock of the registry held.
    fn deliver(id: &str, revision: u64, pending: Pending) {
        let Pending {
            sequence,
            slots,
            event,
        } = pending;
        for slot in slots {
            slot.call(|keeper| match &event {
                Event::Renewed(credentials, expires) => {
                    let mut renewal =
                        AccountRenewal::new(id, revision, sequence, credentials.expose().clone());
                    renewal.expires_at.clone_from(expires);
                    keeper.renewed(renewal)
                }
                Event::Lost(reason) => keeper.lost(AccountLoss::new(id, sequence, reason.clone())),
            });
        }
    }
}

struct Inner {
    accounts: Arc<Accounts>,
    id: String,
    dir: String,
    revision: u64,
    credentials: Mutex<Secret<Credentials>>,
    watched: Vec<Watched>,
    files: Mutex<Option<Files>>,
    slot: Arc<Slot>,
    token: u64,
    /// The files are placed, so the hold counts on them.
    placed: AtomicBool,
    done: AtomicBool,
}

/// A call's or a connection's hold on an account.
///
/// Ending it ([`finish`](Self::finish), or dropping it) looks once more, lets go of the keeper (destroying it) and of
/// the spawner.
#[derive(Clone)]
#[must_use]
pub struct Hold(Arc<Inner>);

impl core::fmt::Debug for Hold {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Hold").finish_non_exhaustive()
    }
}

/// What a file holds now.
enum Now {
    Lost,
    Foreign,
    Has(SecretText),
}

/// What placing a file does.
enum Act {
    Keep,
    Write,
    /// The file is something else than a file: removed, then written.
    Replace,
    News(SecretText),
}

impl Inner {
    fn files(&self) -> Result<Files, ProviderError> {
        lock(&self.files)
            .clone()
            .ok_or_else(|| ProviderError::new(Status::GUATIAO_ERR_GONE, "the hold has ended"))
    }

    fn key(&self, file: &Watched) -> (String, String) {
        (self.dir.clone(), file.path.clone())
    }

    /// The grant's credentials with `file`'s text in its place.
    fn whole(&self, file: &Watched, text: &str) -> Secret<Credentials> {
        let mut credentials = lock(&self.credentials).expose().clone();
        credentials.insert(file.key.clone(), text);
        Secret::new(credentials)
    }

    /// The text of `file` in the grant.
    fn granted(&self, file: &Watched) -> SecretText {
        SecretText::new(
            lock(&self.credentials)
                .expose()
                .get(&file.key)
                .unwrap_or_default()
                .to_string(),
        )
    }

    /// Whether `text` names another account than the grant does.
    fn foreign(&self, file: &Watched, text: &str) -> bool {
        let granted = self.granted(file);
        match ((file.identity)(&granted), (file.identity)(text)) {
            (Some(own), Some(other)) => own != other,
            _ => false,
        }
    }

    fn classify(&self, file: &Watched, state: FileState) -> Now {
        match state {
            FileState::Text(text) if (file.usable)(&text) => {
                if self.foreign(file, &text) {
                    Now::Foreign
                } else {
                    Now::Has(text)
                }
            }
            _ => Now::Lost,
        }
    }

    /// The fingerprint a `Now` would be recorded with.
    fn print_of(now: &Now) -> Option<u64> {
        match now {
            Now::Has(text) => Some(fingerprint(text)),
            Now::Lost | Now::Foreign => None,
        }
    }

    /// What `file` holds, read until two reads a moment apart agree when it differs from what was recorded; `None`
    /// when the writer was not done.
    fn settled(
        &self,
        files: &Files,
        file: &Watched,
        recorded: Option<u64>,
    ) -> Result<Option<Now>, ProviderError> {
        let first = self.classify(file, files.state(&file.path)?);
        if Self::print_of(&first) == recorded && !matches!(first, Now::Foreign) {
            return Ok(Some(first));
        }
        std::thread::sleep(SETTLE);
        let second = self.classify(file, files.state(&file.path)?);
        let same = match (&first, &second) {
            (Now::Has(a), Now::Has(b)) => **a == **b,
            (Now::Lost, Now::Lost) | (Now::Foreign, Now::Foreign) => true,
            _ => false,
        };
        Ok(same.then_some(second))
    }

    /// The time on the spawner's clock when this provider began, from a file the hold writes and reads back.
    fn began_by_spawner(&self, files: &Files) -> i64 {
        let own = now_ms();
        let Ok(()) = files.write(CLOCK_FILE, "") else {
            return self.accounts.began_ms;
        };
        let theirs = files.info(CLOCK_FILE).ok().flatten();
        let _ = files.remove(CLOCK_FILE);
        theirs.map_or(self.accounts.began_ms, |info| {
            self.accounts.began_ms + (info.modified_ms - own)
        })
    }

    fn place(&self) -> Result<(), ProviderError> {
        let files = self.files()?;
        let gate = self.accounts.gate(&self.id);
        let serial = lock(&gate);
        let mut began: Option<i64> = None;
        let mut acts = Vec::new();
        for file in &self.watched {
            let grant = self.granted(file);
            let key = self.key(file);
            let record = lock(&self.accounts.entries)
                .get(&self.id)
                .and_then(|entry| entry.files.get(&key))
                .map(|r| (r.fingerprint, r.revision, r.live));
            let state = files.state(&file.path)?;
            let newer = |recorded: Option<u64>, revision: u64| {
                self.revision > revision && recorded != Some(fingerprint(&grant))
            };
            let act = match (state, record) {
                (FileState::Missing, None) => Act::Write,
                (FileState::Missing, Some((_, _, live))) => {
                    if live == 0 {
                        Act::Write
                    } else {
                        Act::Keep
                    }
                }
                (FileState::Planted, _) => {
                    tracing::warn!(
                        account = %self.id,
                        path = %file.path,
                        reason = "not a file this provider may read; the grant's copy replaces it",
                        "a planted credential file"
                    );
                    Act::Replace
                }
                (FileState::Text(text), None) => {
                    if *text == *grant {
                        Act::Keep
                    } else if !(file.usable)(&text) {
                        Act::Write
                    } else {
                        let when = *began.get_or_insert_with(|| self.began_by_spawner(&files));
                        let changed = files.info(&file.path)?.map_or(0, |info| info.modified_ms);
                        if changed + CLOCK_SLACK_MS >= when {
                            Act::News(text)
                        } else {
                            Act::Write
                        }
                    }
                }
                (FileState::Text(text), Some((recorded, revision, live))) => {
                    if recorded == Some(fingerprint(&text)) || live > 0 {
                        if newer(recorded, revision) {
                            Act::Write
                        } else {
                            Act::Keep
                        }
                    } else if newer(recorded, revision) || !(file.usable)(&text) {
                        Act::Write
                    } else {
                        let when = *began.get_or_insert_with(|| self.began_by_spawner(&files));
                        let changed = files.info(&file.path)?.map_or(0, |info| info.modified_ms);
                        if changed + CLOCK_SLACK_MS >= when {
                            Act::News(text)
                        } else {
                            Act::Write
                        }
                    }
                }
            };
            match &act {
                Act::Replace => {
                    let _ = files.remove(&file.path);
                    files.write(&file.path, &grant)?;
                }
                Act::Write => files.write(&file.path, &grant)?,
                Act::Keep | Act::News(_) => {}
            }
            acts.push((file, act, grant));
        }
        let mut pending = Vec::new();
        {
            let mut entries = lock(&self.accounts.entries);
            let entry = entries.entry(self.id.clone()).or_default();
            for (file, act, grant) in acts {
                let key = self.key(file);
                let fresh = !entry.files.contains_key(&key);
                let record = entry.files.entry(key).or_insert(Record {
                    fingerprint: None,
                    revision: self.revision,
                    live: 0,
                });
                record.live += 1;
                record.revision = record.revision.max(self.revision);
                match act {
                    Act::Write | Act::Replace => record.fingerprint = Some(fingerprint(&grant)),
                    Act::News(text) => {
                        if self.foreign(file, &text) {
                            tracing::warn!(
                                account = %self.id,
                                path = %file.path,
                                reason = "the login is for another account than the grant's",
                                "a login is refused"
                            );
                            if record.fingerprint.take().is_some() {
                                pending.push((
                                    file,
                                    Event::Lost(format!(
                                        "the login in {} is for another account",
                                        file.path
                                    )),
                                ));
                            }
                        } else {
                            record.fingerprint = Some(fingerprint(&text));
                            pending.push((
                                file,
                                Event::Renewed(self.whole(file, &text), (file.expires)(&text)),
                            ));
                        }
                    }
                    Act::Keep => {
                        if fresh {
                            // A file there that holds what the grant does.
                            record.fingerprint = Some(fingerprint(&grant));
                        }
                    }
                }
            }
        }
        let pending: Vec<Pending> = pending
            .into_iter()
            .map(|(_, event)| self.accounts.assign(&self.id, event))
            .collect();
        self.placed.store(true, Ordering::SeqCst);
        drop(serial);
        for one in pending {
            Accounts::deliver(&self.id, self.revision, one);
        }
        Ok(())
    }

    fn look(&self) -> Result<Looked, ProviderError> {
        let files = self.files()?;
        let gate = self.accounts.gate(&self.id);
        let serial = lock(&gate);
        let mut found = Looked::default();
        let mut pending = Vec::new();
        for file in &self.watched {
            let key = self.key(file);
            let recorded = lock(&self.accounts.entries)
                .get(&self.id)
                .and_then(|entry| entry.files.get(&key))
                .map(|r| r.fingerprint);
            let Some(now) = self.settled(&files, file, recorded.flatten())? else {
                continue;
            };
            let event = {
                let mut entries = lock(&self.accounts.entries);
                let entry = entries.entry(self.id.clone()).or_default();
                let record = entry.files.entry(key).or_insert(Record {
                    fingerprint: None,
                    revision: self.revision,
                    live: 0,
                });
                match now {
                    Now::Has(text) => {
                        let print = fingerprint(&text);
                        if record.fingerprint == Some(print) {
                            None
                        } else {
                            record.fingerprint = Some(print);
                            found
                                .renewed
                                .push((self.whole(file, &text), (file.expires)(&text)));
                            Some(Event::Renewed(
                                self.whole(file, &text),
                                (file.expires)(&text),
                            ))
                        }
                    }
                    Now::Lost | Now::Foreign => {
                        if record.fingerprint.take().is_some() {
                            let reason = if matches!(now, Now::Foreign) {
                                tracing::warn!(
                                    account = %self.id,
                                    path = %file.path,
                                    reason = "the login is for another account than the grant's",
                                    "a login is refused"
                                );
                                format!("the login in {} is for another account", file.path)
                            } else {
                                lost_reason(file)
                            };
                            found.lost.push(reason.clone());
                            Some(Event::Lost(reason))
                        } else {
                            None
                        }
                    }
                }
            };
            if let Some(event) = event {
                pending.push(self.accounts.assign(&self.id, event));
            }
        }
        drop(serial);
        for one in pending {
            Accounts::deliver(&self.id, self.revision, one);
        }
        Ok(found)
    }

    fn finish(&self) {
        if self.done.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.placed.load(Ordering::SeqCst) {
            let _ = self.look();
            let mut entries = lock(&self.accounts.entries);
            if let Some(entry) = entries.get_mut(&self.id) {
                for file in &self.watched {
                    if let Some(record) = entry.files.get_mut(&self.key(file)) {
                        record.live = record.live.saturating_sub(1);
                    }
                }
            }
        }
        {
            let mut entries = lock(&self.accounts.entries);
            if let Some(entry) = entries.get_mut(&self.id) {
                entry.keepers.retain(|(token, _)| *token != self.token);
            }
        }
        // Waits for a call to this keeper in flight on another thread, then destroys it.
        self.slot.close();
        drop(lock(&self.files).take());
        *lock(&self.credentials) = Secret::default();
    }
}

fn lost_reason(file: &Watched) -> String {
    format!("the login in {} is missing or empty", file.path)
}

impl Hold {
    /// Looks at the files: every renewal or loss found since the last look is told to every keeper live for the
    /// account, and answered.
    ///
    /// # Errors
    ///
    /// The spawner's refusal to read a file, or the hold has ended.
    pub fn look(&self) -> Result<Looked, ProviderError> {
        self.0.look()
    }

    /// The reason the account's login is lost now, when it is: a file the last look found missing or blank.
    #[must_use]
    pub fn lost(&self) -> Option<String> {
        let entries = lock(&self.0.accounts.entries);
        let entry = entries.get(&self.0.id)?;
        self.0
            .watched
            .iter()
            .find(|file| {
                entry
                    .files
                    .get(&self.0.key(file))
                    .is_some_and(|record| record.fingerprint.is_none())
            })
            .map(lost_reason)
    }

    /// The files' texts as they are now, by credential key: the grant's credentials with each watched file's current
    /// text in its place.
    ///
    /// # Errors
    ///
    /// The spawner's refusal to read a file, or the hold has ended.
    pub fn credentials(&self) -> Result<Secret<Credentials>, ProviderError> {
        let files = self.0.files()?;
        let mut credentials = lock(&self.0.credentials).expose().clone();
        for file in &self.0.watched {
            if let FileState::Text(text) = files.state(&file.path)?
                && (file.usable)(&text)
            {
                credentials.insert(file.key.clone(), &*text);
            }
        }
        Ok(Secret::new(credentials))
    }

    /// The expiry the files now say, the earliest of them.
    #[must_use]
    pub fn expires_at(&self) -> Option<String> {
        let files = self.0.files().ok()?;
        self.0
            .watched
            .iter()
            .filter_map(|file| match files.state(&file.path) {
                Ok(FileState::Text(text)) if (file.usable)(&text) => (file.expires)(&text),
                _ => None,
            })
            .min()
    }

    /// Looks once more, lets go of the keeper (destroying it) and of the spawner. Idempotent.
    pub fn finish(&self) {
        self.0.finish();
    }

    /// Looks every `poll` on a thread of its own until the returned [`Watcher`] is stopped or dropped. A look that
    /// fails is tried again at the next.
    pub fn watch(&self, poll: Duration) -> Watcher {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let looking = self.clone();
        let stopping = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("coqui-acp-account-watch".into())
            .spawn(move || {
                let (flag, changed) = &*stopping;
                let mut stopped = lock(flag);
                loop {
                    let deadline = Instant::now() + poll;
                    while !*stopped {
                        let left = deadline.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            break;
                        }
                        stopped = changed
                            .wait_timeout(stopped, left)
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                    }
                    if *stopped {
                        return;
                    }
                    drop(stopped);
                    let _ = looking.0.look();
                    stopped = lock(flag);
                    if *stopped {
                        return;
                    }
                }
            })
            .ok();
        Watcher { stop, thread }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.finish();
    }
}

/// The thread that looks at an account's files on a deadline of its own.
#[must_use]
pub struct Watcher {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl core::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Watcher").finish_non_exhaustive()
    }
}

impl Watcher {
    /// Stops the thread and waits for it (unless called from the thread itself, from a keeper it was telling).
    pub fn stop(mut self) {
        self.end();
    }

    fn end(&mut self) {
        *lock(&self.stop.0) = true;
        self.stop.1.notify_all();
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != std::thread::current().id()
        {
            let _ = thread.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.end();
    }
}

/// Whether two environment variable names are the same variable: exact on Unix, ignoring case on Windows.
#[must_use]
pub fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}
