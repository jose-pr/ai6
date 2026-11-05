//! Harness logins kept sealed in the Registry: a login's files, what a Session's home does with
//! them, and the renewal round.
//!
//! Harnesses do their own refreshing, so AI6 keeps no provider's token endpoint. A login is a set
//! of files sealed together as one value. A Session gets a copy in a home of its own; when its
//! harness renews it, the new copy goes back under a revision check, and when another Station's
//! does, the Session's files are rewritten in place. A renewal round renews a login before it ends
//! by running that CLI's own command in a throwaway home.
//!
//! What a harness writes is not trusted to be a login: one whose refresh was refused deletes its
//! file (Grok 1.0.70) or blanks its tokens (Claude Code 2.1.318), and a command's exit status does
//! not show it (Grok exits 0). So only the files decide, a copy that lost anything a login had or
//! ends earlier never replaces the one in the Registry, and the Session's files are put back.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::pools::{Account, AccountKind, AccountPool};
use super::sealing::{Recipient, Vault, VaultError};
use crate::bounds::Limit;
use crate::directorates::registry::accounts::{AccountRef, LoginState, RenewalLease};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::sealed::SealedExpect;
use crate::station::limits::AccountLimits;

/// The name of the sealed value holding `account`'s login.
pub fn sealed_name(account: &AccountRef) -> String {
    format!("login/{}/{}", account.pool, account.account)
}

/// The files of one login, by path relative to a home. `Debug` shows the paths alone, and the bytes
/// are overwritten when it is dropped.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct LoginFiles(BTreeMap<String, Vec<u8>>);

impl fmt::Debug for LoginFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

impl Drop for LoginFiles {
    fn drop(&mut self) {
        self.0.values_mut().for_each(Zeroize::zeroize);
    }
}

impl LoginFiles {
    /// Reads `files` from `home`; a file that is not there is left out, and so is one reached
    /// through a link or larger than `largest`, since a Session's commands can put anything in
    /// its home and what is read here is sealed for other Stations. Nothing else is read.
    ///
    /// # Errors
    ///
    /// Any failure but a missing file, naming the path.
    pub fn read(home: &Path, files: &[String], largest: Limit) -> io::Result<Self> {
        let mut read = BTreeMap::new();
        for file in files {
            let path = home.join(file);
            let plain = plain_path(home, Path::new(file))
                && std::fs::symlink_metadata(&path)
                    .is_ok_and(|found| found.is_file() && largest.allows(found.len()));
            if !plain {
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => {
                    read.insert(file.clone(), bytes);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", path.display()),
                    ));
                }
            }
        }
        Ok(Self(read))
    }

    /// Writes each file under `home` readable only by this user, each replaced whole, so a harness
    /// reading one sees the old login or the new.
    ///
    /// # Errors
    ///
    /// When a directory or file cannot be made, naming the path.
    pub fn write(&self, home: &Path) -> io::Result<()> {
        for (file, bytes) in &self.0 {
            let path = home.join(file);
            let at = |error: io::Error| {
                io::Error::new(error.kind(), format!("{}: {error}", path.display()))
            };
            // The file itself may be a link: replacing it leaves what it pointed at alone.
            if !plain_path(home, Path::new(file).parent().unwrap_or(Path::new(""))) {
                return Err(at(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a link stands in its way",
                )));
            }
            let parent = path.parent().unwrap_or(home);
            std::fs::create_dir_all(parent).map_err(at)?;
            private_dirs(home, parent).map_err(at)?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(at)?;
            std::io::Write::write_all(&mut temporary, bytes).map_err(at)?;
            temporary.as_file().sync_all().map_err(at)?;
            temporary.persist(&path).map_err(|error| at(error.error))?;
            private_file(&path).map_err(at)?;
        }
        Ok(())
    }

    /// The bytes of `file`.
    pub fn get(&self, file: &str) -> Option<&[u8]> {
        self.0.get(file).map(Vec::as_slice)
    }

    /// The files as the sealed value's text.
    fn to_value(&self) -> Zeroizing<String> {
        let text = base64::engine::general_purpose::STANDARD;
        let mut value = serde_json::Map::new();
        for (file, bytes) in &self.0 {
            value.insert(file.clone(), serde_json::Value::String(text.encode(bytes)));
        }
        let mut document = serde_json::Value::Object(value);
        let rendered = Zeroizing::new(document.to_string());
        wipe(&mut document);
        rendered
    }

    fn from_value(value: &str) -> Option<Self> {
        let text = base64::engine::general_purpose::STANDARD;
        let mut document: serde_json::Value = serde_json::from_str(value).ok()?;
        let read = document.as_object().and_then(|files| {
            files
                .iter()
                .map(|(file, encoded)| Some((file.clone(), text.decode(encoded.as_str()?).ok()?)))
                .collect::<Option<BTreeMap<_, _>>>()
        });
        wipe(&mut document);
        read.map(Self)
    }

    /// A digest of every file: what tells a home's copy has changed.
    fn digest(&self) -> String {
        let mut digest = Sha256::new();
        for (file, bytes) in &self.0 {
            digest.update((file.len() as u64).to_be_bytes());
            digest.update(file.as_bytes());
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        digest
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                hex.push_str(&format!("{byte:02x}"));
                hex
            })
    }

    /// When the login ends, as Unix seconds, when its files say: the earliest `expires_at`,
    /// `expiresAt` or `expiry` they hold (a number in seconds or milliseconds, or RFC 3339), else
    /// the earliest `exp` of an access token that is a JWT.
    pub fn expires_at(&self) -> Option<f64> {
        let mut named = None;
        let mut token = None;
        for bytes in self.0.values() {
            if let Ok(document) = serde_json::from_slice::<serde_json::Value>(bytes) {
                scan_expiry(&document, &mut named, &mut token);
            }
        }
        named.or(token)
    }

    /// Why this copy is no login to keep, given the copy it would replace; `None` when it can be
    /// kept. A file gone or empty, a file that was JSON and is not, and a text the old copy held
    /// that is now empty or gone are what a harness that was refused leaves behind.
    fn fault_against(&self, older: &Self, files: &[String]) -> Option<&'static str> {
        for file in files {
            let Some(bytes) = self.0.get(file).filter(|bytes| !bytes.is_empty()) else {
                return Some("its harness removed its login file");
            };
            let Some(old) = older
                .0
                .get(file)
                .and_then(|old| serde_json::from_slice::<serde_json::Value>(old).ok())
            else {
                continue;
            };
            let Ok(new) = serde_json::from_slice::<serde_json::Value>(bytes) else {
                return Some("its login file is not whole");
            };
            if lost_text(&old, &new) {
                return Some("its harness blanked its login");
            }
        }
        None
    }
}

/// Whether no part of `file` under `home` is a link, so what is read or written there is the home's
/// own and not whatever a Session pointed it at.
fn plain_path(home: &Path, file: &Path) -> bool {
    let mut path = home.to_path_buf();
    for part in file.components() {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(found) if found.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(_) => return true,
        }
    }
    true
}

fn wipe(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => text.zeroize(),
        serde_json::Value::Array(values) => values.iter_mut().for_each(wipe),
        serde_json::Value::Object(map) => map.values_mut().for_each(wipe),
        _ => {}
    }
}

/// Whether a token in `old` is empty or gone in `new`, at any depth: a key naming a token, or the
/// `key` Grok keeps its token under. Other fields come and go with a harness's version and its
/// account's profile.
fn lost_text(old: &serde_json::Value, new: &serde_json::Value) -> bool {
    let Some(map) = old.as_object() else {
        return false;
    };
    map.iter().any(|(key, value)| match value {
        serde_json::Value::Object(_) => match new.get(key) {
            Some(new) => lost_text(value, new),
            None => holds_token(value),
        },
        serde_json::Value::String(text) if names_token(key) && !text.is_empty() => new
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty),
        _ => false,
    })
}

fn names_token(key: &str) -> bool {
    key == "key" || key.to_ascii_lowercase().contains("token")
}

fn holds_token(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|map| {
        map.iter().any(|(key, value)| match value {
            serde_json::Value::Object(_) => holds_token(value),
            serde_json::Value::String(text) => names_token(key) && !text.is_empty(),
            _ => false,
        })
    })
}

fn scan_expiry(value: &serde_json::Value, named: &mut Option<f64>, token: &mut Option<f64>) {
    let earliest = |slot: &mut Option<f64>, at: f64| {
        *slot = Some(slot.map_or(at, |own: f64| own.min(at)));
    };
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                match key.as_str() {
                    "expires_at" | "expiresAt" | "expiry" => {
                        if let Some(at) = time_of(value) {
                            earliest(named, at);
                        }
                    }
                    "access_token" | "accessToken" => {
                        if let Some(at) = value.as_str().and_then(jwt_expiry) {
                            earliest(token, at);
                        }
                    }
                    _ => scan_expiry(value, named, token),
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                scan_expiry(value, named, token);
            }
        }
        _ => {}
    }
}

/// A time as a login file writes it. A number past year 5000 in seconds is in milliseconds.
fn time_of(value: &serde_json::Value) -> Option<f64> {
    if let Some(number) = value.as_f64() {
        return Some(if number > 1e11 {
            number / 1000.0
        } else {
            number
        });
    }
    let parsed = time::OffsetDateTime::parse(
        value.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()?;
    Some(parsed.unix_timestamp_nanos() as f64 / 1e9)
}

/// The `exp` of a JWT, unverified: it only says when to renew, and the provider decides.
fn jwt_expiry(token: &str) -> Option<f64> {
    let claims = token.split('.').nth(1)?;
    let bytes = Zeroizing::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(claims.trim_end_matches('='))
            .ok()?,
    );
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()?
        .get("exp")?
        .as_f64()
}

#[cfg(unix)]
fn private_dirs(home: &Path, leaf: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut dir = leaf;
    while dir.starts_with(home) {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    Ok(())
}

/// Windows keeps the access list inherited from the Station's state directory, logged as a
/// weakening where the account is declared.
#[cfg(not(unix))]
fn private_dirs(_home: &Path, _leaf: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn private_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn private_file(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Why a login could not be handled. Never holds a token: its files' contents reach no message.
#[derive(Debug)]
pub enum LoginError {
    /// The sealed value's.
    Vault(VaultError),
    /// A file.
    Io(io::Error),
    /// A login directory lacks a file the account names.
    Missing(String),
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vault(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
            Self::Missing(file) => write!(f, "the login has no file {file:?}"),
        }
    }
}

impl std::error::Error for LoginError {}

impl From<VaultError> for LoginError {
    fn from(error: VaultError) -> Self {
        Self::Vault(error)
    }
}

impl From<io::Error> for LoginError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The most a login's file may hold: no more than the Registry would keep of it.
fn largest_file(vault: &Vault) -> Limit {
    vault.registry().limits().accounts.sealed_bytes
}

/// Seals the login in `dir` to `stations` as `account`'s, replacing one it had. Nothing is taken
/// from `dir`.
///
/// # Errors
///
/// A file `files` names is not there, or a Station's key cannot be used.
pub fn add_login(
    vault: &Vault,
    account: &AccountRef,
    dir: &Path,
    files: &[String],
    stations: &[Recipient],
    now: f64,
) -> Result<u64, LoginError> {
    let login = LoginFiles::read(dir, files, largest_file(vault))?;
    if let Some(file) = files.iter().find(|file| login.get(file).is_none()) {
        return Err(LoginError::Missing(file.clone()));
    }
    let name = sealed_name(account);
    let revision = vault.seal_to(&name, &login.to_value(), stations, SealedExpect::Any, now)?;
    let names: Vec<String> = stations
        .iter()
        .map(|station| station.station().to_owned())
        .collect();
    let expires_at = login.expires_at();
    vault
        .registry()
        .change_login(account, &mut |state| {
            *state = Some(LoginState {
                stations: names.clone(),
                revision,
                expires_at,
                renewed_at: Some(now),
                ..LoginState::default()
            });
        })
        .map_err(VaultError::from)?;
    Ok(revision)
}

/// Records `revision` of `account`'s login, with who it is sealed to now and when it ends; a
/// `renewed` one clears what failed before.
fn note_revision(
    vault: &Vault,
    account: &AccountRef,
    revision: u64,
    stations: &BTreeSet<String>,
    expires_at: Option<f64>,
    renewed: Option<f64>,
) -> Result<(), LoginError> {
    let names: Vec<String> = stations.iter().cloned().collect();
    vault
        .registry()
        .change_login(account, &mut |state| {
            let state = state.get_or_insert_with(LoginState::default);
            if state.revision > revision {
                return;
            }
            state.stations.clone_from(&names);
            state.revision = revision;
            state.expires_at = expires_at;
            if renewed.is_some() {
                state.renewed_at = renewed;
                state.checked_at = None;
                state.failures = 0;
                state.next_try = None;
                state.warned_at = None;
                state.needs_person = None;
            }
        })
        .map_err(VaultError::from)?;
    Ok(())
}

/// What a home's copy and the Registry's are known to be, kept beside the home so a Station that
/// restarts can still tell what its harness renewed while it was down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLogin {
    /// The account.
    pub account: AccountRef,
    /// The files, relative to the home.
    pub files: Vec<String>,
    /// The Session's home.
    pub home: PathBuf,
    /// The revision the home's copy was last at.
    pub revision: u64,
    /// The revision its harness last started on.
    #[serde(default)]
    pub started: u64,
    digest: String,
}

/// What one look at a home did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Synced {
    /// Nothing had changed.
    Unchanged,
    /// The harness renewed the login, and the new copy reached the Registry as this revision.
    Renewed(u64),
    /// Another Station's renewal reached the home as this revision.
    Rewritten(u64),
    /// What the harness left was not kept, for this reason; the home holds this revision again.
    Restored(u64, &'static str),
}

impl SessionLogin {
    /// Writes `account`'s current login under `home`.
    ///
    /// # Errors
    ///
    /// When it cannot be opened, or written.
    pub fn materialize(
        vault: &Vault,
        account: &AccountRef,
        files: &[String],
        home: &Path,
    ) -> Result<Self, LoginError> {
        let opened = vault.open(&sealed_name(account))?;
        let login = LoginFiles::from_value(&opened.value).ok_or(VaultError::Unreadable)?;
        if let Some(file) = files.iter().find(|file| login.get(file).is_none()) {
            return Err(LoginError::Missing(file.clone()));
        }
        login.write(home)?;
        let written = Self {
            account: account.clone(),
            files: files.to_vec(),
            home: home.to_path_buf(),
            revision: opened.revision,
            started: opened.revision,
            digest: login.digest(),
        };
        written.save()?;
        Ok(written)
    }

    fn state_path(home: &Path) -> PathBuf {
        let mut name = home.as_os_str().to_owned();
        name.push(".login.json");
        PathBuf::from(name)
    }

    fn save(&self) -> io::Result<()> {
        let path = Self::state_path(&self.home);
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        std::io::Write::write_all(&mut temporary, &bytes)?;
        temporary.persist(&path).map_err(|error| error.error)?;
        Ok(())
    }

    /// Notes that a harness starts on the home's copy as it is now.
    pub fn harness_starts(&mut self) {
        self.started = self.revision;
    }

    /// What a Station that restarted kept about `home`; `None` when it kept nothing.
    pub fn recall(home: &Path) -> Option<Self> {
        let bytes = std::fs::read(Self::state_path(home)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Removes the home and what is kept beside it.
    ///
    /// # Errors
    ///
    /// What removing them failed with.
    pub fn remove(&self) -> io::Result<()> {
        match std::fs::remove_dir_all(&self.home) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match std::fs::remove_file(Self::state_path(&self.home)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Writes the current login over the home's copy, keeping nothing the harness left.
    fn rewrite(&mut self, opened: &super::sealing::Opened) -> Result<LoginFiles, LoginError> {
        let login = LoginFiles::from_value(&opened.value).ok_or(VaultError::Unreadable)?;
        login.write(&self.home)?;
        self.revision = opened.revision;
        self.digest = login.digest();
        self.save()?;
        Ok(login)
    }

    /// One look at the home: a renewed copy goes back under the revision check, and a newer
    /// revision comes to the home. `registry_revision` is what the Registry holds now, when the
    /// caller has read it.
    ///
    /// # Errors
    ///
    /// When the home or the Registry fails; the next look tries again.
    pub fn sync(
        &mut self,
        vault: &Vault,
        registry_revision: Option<u64>,
        now: f64,
    ) -> Result<Synced, LoginError> {
        let on_disk = LoginFiles::read(&self.home, &self.files, largest_file(vault))?;
        if on_disk.digest() == self.digest {
            if registry_revision.is_none_or(|held| held == self.revision) {
                return Ok(Synced::Unchanged);
            }
            let opened = vault.open(&sealed_name(&self.account))?;
            self.rewrite(&opened)?;
            return Ok(Synced::Rewritten(opened.revision));
        }
        let name = sealed_name(&self.account);
        let held = vault.open(&name)?;
        let current = LoginFiles::from_value(&held.value).ok_or(VaultError::Unreadable)?;
        if let Some(fault) = on_disk.fault_against(&current, &self.files) {
            self.rewrite(&held)?;
            return Ok(Synced::Restored(held.revision, fault));
        }
        let (ours, theirs) = (on_disk.expires_at(), current.expires_at());
        if let (Some(ours), Some(theirs)) = (ours, theirs)
            && ours < theirs
        {
            self.rewrite(&held)?;
            return Ok(Synced::Restored(
                held.revision,
                "it ends sooner than the Registry's",
            ));
        }
        // A copy made from a revision since replaced is kept only if it plainly ends later.
        let newest = held.revision == self.revision
            || matches!((ours, theirs), (Some(ours), Some(theirs)) if ours > theirs);
        if !newest {
            self.rewrite(&held)?;
            return Ok(Synced::Restored(
                held.revision,
                "another Station's renewal reached the Registry first",
            ));
        }
        match vault.replace(&name, &on_disk.to_value(), held.revision, now) {
            Ok(revision) => {
                self.revision = revision;
                self.digest = on_disk.digest();
                self.save()?;
                note_revision(
                    vault,
                    &self.account,
                    revision,
                    &held.stations,
                    ours,
                    Some(now),
                )?;
                Ok(Synced::Renewed(revision))
            }
            // Beaten since the read: the next look sees which copy is newest.
            Err(VaultError::Conflict { .. }) => Ok(Synced::Unchanged),
            Err(error) => Err(error.into()),
        }
    }
}

/// How a renewal command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenewEnd {
    /// It exited with success.
    Success,
    /// It exited otherwise, with its code when it had one.
    Failed(Option<i32>),
    /// It could not start, or was killed at its time limit.
    NotRun(String),
}

/// Runs a login's renewal command. What it prints is never read: it could hold a token.
pub trait LoginRenewer: Send + Sync {
    /// Runs `account`'s command with `home` as its home, for no longer than `timeout`.
    fn renew(&self, account: &Account, home: &Path, timeout: Duration) -> RenewEnd;
}

/// Runs the command with `HOME` (and `USERPROFILE`) at the throwaway home, the account's
/// `renew_env`, the Station's `PATH` unless that names one, and on Windows what a process needs to
/// start. A harness's other variables stay out, so no other login can be reached.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandRenewer;

impl LoginRenewer for CommandRenewer {
    fn renew(&self, account: &Account, home: &Path, timeout: Duration) -> RenewEnd {
        let Some((program, arguments)) = account.renewal().split_first() else {
            return RenewEnd::NotRun("no renewal command".to_owned());
        };
        let mut command = Command::new(program);
        command
            .args(arguments)
            .current_dir(home)
            .env_clear()
            .env("HOME", home)
            .env("USERPROFILE", home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for name in ["PATH", "SYSTEMROOT", "TEMP", "TMP", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.envs(account.renewal_env());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return RenewEnd::NotRun(format!("{program}: {error}")),
        };
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return RenewEnd::Success,
                Ok(Some(status)) => return RenewEnd::Failed(status.code()),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return RenewEnd::NotRun(format!(
                        "{program} did not finish within {timeout:?}"
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => return RenewEnd::NotRun(format!("{program}: {error}")),
            }
        }
    }
}

/// A login's standing, for what a round says to people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    /// A Station was leased its renewal.
    Leased(String),
    /// It was renewed.
    Renewed,
    /// A renewal ran and left it as it was.
    Unchanged,
    /// A renewal failed; another comes after the backoff.
    Failed,
    /// It will not renew without a person.
    NeedsPerson,
    /// It may lapse, and nothing can renew it with no model turn.
    MayLapse,
}

/// One thing a round did or found. The text names files and exit codes, never their contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginNotice {
    /// The account.
    pub account: AccountRef,
    /// What happened.
    pub kind: NoticeKind,
    /// As a person reads it.
    pub text: String,
}

/// Whether a round should renew this login at `now`: its known end is within `renew_within`, or
/// where that cannot be read its last renewal is `renew_after` old, and no backoff or person stands
/// in the way.
pub fn renewal_due(login: &LoginState, limits: &AccountLimits, now: f64) -> bool {
    if login.needs_person.is_some() || login.next_try.is_some_and(|next| next > now) {
        return false;
    }
    match login.expires_at {
        Some(end) => end - now <= limits.renew_within.0.as_secs_f64(),
        None => {
            let last = login.renewed_at.into_iter().chain(login.checked_at);
            last.fold(f64::NEG_INFINITY, f64::max) + limits.renew_after.0.as_secs_f64() <= now
        }
    }
}

/// The logins of `pools`: the accounts that keep one, and whether a command can renew each.
pub fn login_accounts<'a>(
    pools: impl IntoIterator<Item = &'a AccountPool>,
) -> Vec<(AccountRef, &'a Account)> {
    let mut accounts = Vec::new();
    for pool in pools {
        for account in pool.accounts() {
            if account.kind() == AccountKind::HarnessLogin && account.keeps_login_files() {
                accounts.push((AccountRef::new(pool.id(), account.name()), account));
            }
        }
    }
    accounts
}

/// The Headquarters Station's round: for each login that is due, grants one Station its renewal
/// lease, this one when the login names it, else the first named Station in `live`. A login no
/// command can renew is left to its next real use, and a person is told it may lapse.
///
/// A lease is one Registry write that fails when another is still good, so however many Stations
/// believe they are Headquarters, one holds it. The round's state is the Registry's: the next
/// Headquarters carries on where this one stopped.
///
/// # Errors
///
/// When the Registry cannot be read.
pub fn assign_renewals(
    registry: &dyn RegistryBackend,
    logins: &[(AccountRef, &Account)],
    headquarters: &str,
    live: &[String],
    limits: &AccountLimits,
    now: f64,
) -> io::Result<Vec<LoginNotice>> {
    let states = registry.accounts()?;
    let mut notices = Vec::new();
    for (reference, account) in logins {
        let Some(login) = states
            .iter()
            .find(|state| state.account == *reference)
            .and_then(|state| state.login.as_ref())
        else {
            continue;
        };
        if !renewal_due(login, limits, now) || login.leased_at(now).is_some() {
            continue;
        }
        if account.renewal().is_empty() {
            let warned = login
                .warned_at
                .is_some_and(|at| at + limits.renew_after.0.as_secs_f64() > now);
            if !warned {
                registry.change_login(reference, &mut |state| {
                    if let Some(state) = state {
                        state.warned_at = Some(now);
                    }
                })?;
                notices.push(LoginNotice {
                    account: reference.clone(),
                    kind: NoticeKind::MayLapse,
                    text: format!(
                        "account {reference}'s login may lapse: its handler has no way to renew \
                         it with no model turn here, so it waits for its next real use"
                    ),
                });
            }
            continue;
        }
        let holder = if login.stations.iter().any(|named| named == headquarters) {
            headquarters
        } else if let Some(named) = login
            .stations
            .iter()
            .find(|named| live.iter().any(|alive| alive == *named))
        {
            named.as_str()
        } else {
            continue;
        };
        let until = now + limits.renew_lease.0.as_secs_f64();
        let mut granted = false;
        registry.change_login(reference, &mut |state| {
            granted = false;
            if let Some(state) = state
                && state.leased_at(now).is_none()
                && renewal_due(state, limits, now)
            {
                state.lease = Some(RenewalLease {
                    station: holder.to_owned(),
                    until,
                });
                granted = true;
            }
        })?;
        if granted {
            notices.push(LoginNotice {
                account: reference.clone(),
                kind: NoticeKind::Leased(holder.to_owned()),
                text: format!("account {reference}'s renewal is leased to Station {holder:?}"),
            });
        }
    }
    Ok(notices)
}

/// Runs the renewals this Station holds the lease of, each in a throwaway home under `scratch`,
/// removed whatever happens.
///
/// # Errors
///
/// When the Registry cannot be read.
pub fn run_leases(
    vault: &Vault,
    logins: &[(AccountRef, &Account)],
    renewer: &dyn LoginRenewer,
    scratch: &Path,
    limits: &AccountLimits,
    now: f64,
) -> io::Result<Vec<LoginNotice>> {
    let states = vault.registry().accounts()?;
    let mut notices = Vec::new();
    for (reference, account) in logins {
        let lease = states
            .iter()
            .find(|state| state.account == *reference)
            .and_then(|state| state.login.as_ref())
            .and_then(|login| login.leased_at(now))
            .filter(|lease| lease.station == vault.station())
            .cloned();
        let Some(lease) = lease else {
            continue;
        };
        let timeout = limits
            .renew_timeout
            .0
            .min(Duration::from_secs_f64((lease.until - now).max(0.0)));
        let outcome = renew_once(vault, reference, account, renewer, scratch, timeout, now);
        notices.extend(settle(vault, reference, outcome, limits, now)?);
    }
    Ok(notices)
}

/// What one renewal came to.
enum Outcome {
    Renewed,
    Unchanged { expired: bool },
    Failed(String),
}

fn renew_once(
    vault: &Vault,
    reference: &AccountRef,
    account: &Account,
    renewer: &dyn LoginRenewer,
    scratch: &Path,
    timeout: Duration,
    now: f64,
) -> Outcome {
    let attempt = || -> Result<Outcome, LoginError> {
        let name = sealed_name(reference);
        let opened = vault.open(&name)?;
        let before = LoginFiles::from_value(&opened.value).ok_or(VaultError::Unreadable)?;
        std::fs::create_dir_all(scratch)?;
        let home = tempfile::Builder::new()
            .prefix("renewing-")
            .tempdir_in(scratch)?;
        before.write(home.path())?;
        let end = renewer.renew(account, home.path(), timeout);
        let after = LoginFiles::read(home.path(), account.login_files(), largest_file(vault))?;
        // A token the provider has already rotated must not be lost, whatever the exit status.
        if let Some(fault) = after.fault_against(&before, account.login_files()) {
            return Ok(Outcome::Failed(format!("{fault} ({})", describe(&end))));
        }
        if after.digest() == before.digest() {
            return match end {
                RenewEnd::Success => Ok(Outcome::Unchanged {
                    expired: before.expires_at().is_some_and(|at| at <= now),
                }),
                other => Ok(Outcome::Failed(describe(&other))),
            };
        }
        let (ours, theirs) = (after.expires_at(), before.expires_at());
        if let (Some(ours), Some(theirs)) = (ours, theirs)
            && ours < theirs
        {
            return Ok(Outcome::Failed(
                "the renewed login ends sooner than the one it would replace".to_owned(),
            ));
        }
        match vault.replace(&name, &after.to_value(), opened.revision, now) {
            Ok(revision) => {
                note_revision(
                    vault,
                    reference,
                    revision,
                    &opened.stations,
                    ours,
                    Some(now),
                )?;
                Ok(Outcome::Renewed)
            }
            Err(VaultError::Conflict { .. }) => {
                // A Session's harness renewed it meanwhile: the Registry's copy may be the only one
                // that still works. Keep whichever plainly ends later.
                let held = vault.open(&name)?;
                let current = LoginFiles::from_value(&held.value).ok_or(VaultError::Unreadable)?;
                if matches!((ours, current.expires_at()), (Some(ours), Some(held_end)) if ours > held_end)
                {
                    let revision = vault.replace(&name, &after.to_value(), held.revision, now)?;
                    note_revision(vault, reference, revision, &held.stations, ours, Some(now))?;
                }
                Ok(Outcome::Renewed)
            }
            Err(error) => Err(error.into()),
        }
    };
    match attempt() {
        Ok(outcome) => outcome,
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

fn describe(end: &RenewEnd) -> String {
    match end {
        RenewEnd::Success => "its command exited with success".to_owned(),
        RenewEnd::Failed(Some(code)) => format!("its command exited with code {code}"),
        RenewEnd::Failed(None) => "its command was stopped by a signal".to_owned(),
        RenewEnd::NotRun(reason) => reason.clone(),
    }
}

/// Records how a renewal ended and frees its lease.
fn settle(
    vault: &Vault,
    reference: &AccountRef,
    outcome: Outcome,
    limits: &AccountLimits,
    now: f64,
) -> io::Result<Vec<LoginNotice>> {
    let failure = match &outcome {
        Outcome::Failed(reason) => Some(reason.clone()),
        Outcome::Unchanged { expired: true } => {
            Some("its renewal left a login that has ended as it was".to_owned())
        }
        _ => None,
    };
    let mut marked = false;
    let after = vault.registry().change_login(reference, &mut |state| {
        let Some(state) = state else {
            return;
        };
        state.lease = None;
        match &failure {
            None => {
                if !matches!(outcome, Outcome::Renewed) {
                    state.checked_at = Some(now);
                }
                state.failures = 0;
                state.next_try = None;
            }
            Some(reason) => {
                state.failures = state.failures.saturating_add(1);
                state.next_try = Some(now + backoff(state.failures, limits));
                marked = !limits
                    .renew_attempts
                    .allows(state.failures.saturating_add(1));
                if marked {
                    state.needs_person = Some(format!(
                        "{} renewals failed in a row; the last: {reason}",
                        state.failures
                    ));
                }
            }
        }
    })?;
    let failures = after.as_ref().map_or(0, |login| login.failures);
    Ok(vec![match (&outcome, failure) {
        (Outcome::Renewed, _) => LoginNotice {
            account: reference.clone(),
            kind: NoticeKind::Renewed,
            text: format!("account {reference}'s login was renewed"),
        },
        (_, Some(reason)) if marked => LoginNotice {
            account: reference.clone(),
            kind: NoticeKind::NeedsPerson,
            text: format!(
                "account {reference}'s login needs a person: {failures} renewals failed in a row; \
                 the last: {reason}. Its Sessions go to the next account or route"
            ),
        },
        (_, Some(reason)) => LoginNotice {
            account: reference.clone(),
            kind: NoticeKind::Failed,
            text: format!("account {reference}'s login was not renewed ({failures}): {reason}"),
        },
        _ => LoginNotice {
            account: reference.clone(),
            kind: NoticeKind::Unchanged,
            text: format!("account {reference}'s renewal left its login as it was"),
        },
    }])
}

/// The wait after the `failures`th failure: `renew_backoff`, doubled each time, up to
/// `renew_backoff_max`.
fn backoff(failures: u32, limits: &AccountLimits) -> f64 {
    let first = limits.renew_backoff.0.as_secs_f64();
    let most = limits.renew_backoff_max.0.as_secs_f64();
    (first * 2f64.powi(i32::try_from(failures.saturating_sub(1)).unwrap_or(i32::MAX))).min(most)
}

#[cfg(test)]
mod tests;
