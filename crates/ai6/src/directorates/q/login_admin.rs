//! What a person does to a harness login kept sealed in the Registry: add one from a host where
//! the CLI is signed in, change which Stations may hold it, forget it, and optionally sign it out
//! at the provider.
//!
//! Every act is audited first, and an audit the Registry refuses refuses the act. Nothing here
//! prints or returns a login's contents: a person's request that needs a Station's sealing key is
//! recorded on the login as a [`LoginRequest`], and a Station holding the login makes it at its
//! next round ([`apply_requests`]).

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::Path;
use std::time::Duration;

use super::login_standing::{self as standing, describe, sealed_name};
use super::logins::{
    LoginError, LoginFiles, LoginNotice, LoginRenewer, NoticeKind, RenewEnd, add_login as seal,
};
use super::pools::Account;
use super::sealing::{Recipient, Vault, VaultError};
use crate::directorates::registry::accounts::{AccountRef, LoginRequest, LoginRequestAction};
use crate::directorates::registry::backend::RegistryBackend;

/// Why an act on a login was refused. Never holds a login's contents.
#[derive(Debug)]
pub enum AdminError {
    /// The audit record could not be written, so nothing was done.
    Audit(io::Error),
    /// The account already holds a login and the act was not asked to replace it.
    Exists,
    /// The account's declaration names no login files, so it keeps no login in the Registry.
    NoFiles,
    /// The account holds no login.
    NoLogin,
    /// This Station is not one the login is sealed to, so it cannot open it.
    NotHolder,
    /// The account declares no sign-out command.
    NoSignOut,
    /// The sign-out command did not end with success, so the login was kept.
    SignOut(String),
    /// Sealing, opening or the files.
    Login(LoginError),
    /// The Registry.
    Registry(io::Error),
}

impl fmt::Display for AdminError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audit(error) => write!(f, "the audit record was refused, so nothing was done: {error}"),
            Self::Exists => f.write_str("the account already holds a login; pass --replace to replace it"),
            Self::NoFiles => f.write_str("the account declares no login `files`, so it keeps no login in the Registry"),
            Self::NoLogin => f.write_str("the account holds no login"),
            Self::NotHolder => f.write_str("this Station is not one the login is sealed to"),
            Self::NoSignOut => f.write_str("the account declares no `logout_command`, so it cannot be signed out at its provider; nothing was changed"),
            Self::SignOut(why) => write!(f, "signing out failed ({why}); the login was kept"),
            Self::Login(error) => error.fmt(f),
            Self::Registry(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AdminError {}

impl From<LoginError> for AdminError {
    fn from(error: LoginError) -> Self {
        Self::Login(error)
    }
}

impl From<VaultError> for AdminError {
    fn from(error: VaultError) -> Self {
        match error {
            VaultError::NotNamed | VaultError::NotFound => Self::NotHolder,
            other => Self::Login(LoginError::Vault(other)),
        }
    }
}

impl From<io::Error> for AdminError {
    fn from(error: io::Error) -> Self {
        Self::Registry(error)
    }
}

/// Writes the audit record of an act; the act waits for it.
fn audit_first(
    registry: &dyn RegistryBackend,
    actor: &str,
    action: &str,
    operation: &str,
    target: &str,
    resource: Option<String>,
    now: f64,
) -> Result<(), AdminError> {
    standing::audit_first(registry, actor, action, operation, target, resource, now)
        .map_err(AdminError::Audit)
}

/// Where a login is read from, and who may hold it.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    /// The home the CLI is signed in under.
    pub from: &'a Path,
    /// The Stations it is sealed to.
    pub stations: &'a [Recipient],
    /// Whether it may replace a login the account already holds.
    pub replace: bool,
}

/// What runs a command and where: the renewer that does, the directory throwaway homes are made in
/// and how long a command may take.
#[derive(Clone, Copy)]
pub struct Commands<'a> {
    /// Runs the account's commands.
    pub renewer: &'a dyn LoginRenewer,
    /// Where a throwaway home is made, removed whatever happens.
    pub scratch: &'a Path,
    /// The longest a command may take.
    pub timeout: Duration,
}

/// Seals the login the CLI left under `source.from` as `account`'s, to `source.stations`, from the
/// files `declared` names. Nothing is taken from the host.
///
/// # Errors
///
/// [`AdminError::Exists`] without `replace` when the account already holds a login,
/// [`AdminError::NoFiles`], a missing file ([`LoginError::Missing`]), the audit, or a Station that
/// published no usable key.
pub fn add_login(
    vault: &Vault,
    actor: &str,
    account: &AccountRef,
    declared: &Account,
    source: Source<'_>,
    now: f64,
) -> Result<u64, AdminError> {
    let Source {
        from,
        stations,
        replace,
    } = source;
    if !declared.keeps_login_files() {
        return Err(AdminError::NoFiles);
    }
    let registry = vault.registry();
    if !replace && standing::holds_login(registry, account)? {
        return Err(AdminError::Exists);
    }
    // Looking at the files harms nothing, so a login that is not there is refused before it is
    // audited.
    let found = LoginFiles::read(
        from,
        declared.login_files(),
        registry.limits().accounts.sealed_bytes,
    )
    .map_err(LoginError::from)?;
    if let Some(file) = declared
        .login_files()
        .iter()
        .find(|file| found.get(file).is_none())
    {
        return Err(LoginError::Missing(file.clone()).into());
    }
    drop(found);
    audit_first(
        registry,
        actor,
        if replace {
            "pool.login.replace"
        } else {
            "pool.login.add"
        },
        "",
        &account.to_string(),
        Some(format!("station:{}", vault.station())),
        now,
    )?;
    Ok(seal(
        vault,
        account,
        from,
        declared.login_files(),
        stations,
        now,
    )?)
}

/// Records which Stations a login is sealed to after a change, keeping its other standing.
fn note_stations(
    vault: &Vault,
    account: &AccountRef,
    revision: u64,
    stations: &BTreeSet<String>,
) -> Result<(), AdminError> {
    let names: Vec<String> = stations.iter().cloned().collect();
    vault.registry().change_login(account, &mut |state| {
        if let Some(state) = state
            && state.revision <= revision
        {
            state.revision = revision;
            state.stations.clone_from(&names);
        }
    })?;
    Ok(())
}

/// Seals `account`'s login to one more Station, as this Station can open it.
///
/// # Errors
///
/// [`AdminError::NotHolder`] when this Station cannot open it, the audit, or the Station's key.
pub fn add_station(
    vault: &Vault,
    actor: &str,
    account: &AccountRef,
    station: &Recipient,
    now: f64,
) -> Result<u64, AdminError> {
    let name = sealed_name(account);
    vault.open(&name)?;
    audit_first(
        vault.registry(),
        actor,
        "pool.login.station.add",
        "",
        &format!("{account} {}", station.station()),
        Some(format!("station:{}", vault.station())),
        now,
    )?;
    let mut stations = vault.open(&name)?.stations;
    let revision = vault.add_station(&name, station, now)?;
    stations.insert(station.station().to_owned());
    note_stations(vault, account, revision, &stations)?;
    Ok(revision)
}

/// Stops sealing `account`'s login to `station`, which drops that Station's copy. What it already
/// opened stays with it, so a login it held should be replaced.
///
/// # Errors
///
/// [`AdminError::NotHolder`] when this Station cannot open it, the audit, or the last Station.
pub fn remove_station(
    vault: &Vault,
    actor: &str,
    account: &AccountRef,
    station: &str,
    now: f64,
) -> Result<u64, AdminError> {
    let name = sealed_name(account);
    vault.open(&name)?;
    audit_first(
        vault.registry(),
        actor,
        "pool.login.station.remove",
        "",
        &format!("{account} {station}"),
        Some(format!("station:{}", vault.station())),
        now,
    )?;
    let mut stations = vault.open(&name)?.stations;
    let revision = vault.remove_station(&name, station, now)?;
    stations.remove(station);
    note_stations(vault, account, revision, &stations)?;
    Ok(revision)
}

/// Forgets `account`'s login: the sealed value and what the Registry kept of its standing. Needs
/// no key, so any host with the Registry can.
///
/// # Errors
///
/// [`AdminError::NoLogin`] when there is none, or the audit.
pub fn forget_login(
    registry: &dyn RegistryBackend,
    actor: &str,
    station: &str,
    account: &AccountRef,
    now: f64,
) -> Result<(), AdminError> {
    if !standing::holds_login(registry, account)? {
        return Err(AdminError::NoLogin);
    }
    audit_first(
        registry,
        actor,
        "pool.login.forget",
        "",
        &account.to_string(),
        Some(format!("station:{station}")),
        now,
    )?;
    standing::forget(registry, account)?;
    Ok(())
}

/// Signs `account`'s login out at its provider with the account's `logout_command`, run in a
/// throwaway home under `scratch`, then forgets it. When the command does not end with success the
/// login is kept.
///
/// # Errors
///
/// [`AdminError::NoSignOut`] when the account declares no command (nothing is changed),
/// [`AdminError::NotHolder`], [`AdminError::SignOut`], or those of [`forget_login`].
pub fn sign_out_and_forget(
    vault: &Vault,
    commands: Commands<'_>,
    actor: &str,
    account: &AccountRef,
    declared: &Account,
    now: f64,
) -> Result<(), AdminError> {
    let Commands {
        renewer,
        scratch,
        timeout,
    } = commands;
    if declared.logout().is_empty() {
        return Err(AdminError::NoSignOut);
    }
    let opened = vault.open(&sealed_name(account))?;
    let login = LoginFiles::from_value(&opened.value).ok_or(VaultError::Unreadable)?;
    audit_first(
        vault.registry(),
        actor,
        "pool.login.sign_out",
        "",
        &account.to_string(),
        Some(format!("station:{}", vault.station())),
        now,
    )?;
    std::fs::create_dir_all(scratch)?;
    let home = tempfile::Builder::new()
        .prefix("renewing-")
        .tempdir_in(scratch)?;
    login.write(home.path()).map_err(LoginError::from)?;
    let end = renewer.sign_out(declared, home.path(), timeout);
    drop(home);
    if end != RenewEnd::Success {
        return Err(AdminError::SignOut(match end {
            RenewEnd::Failed(Some(code)) => format!("its command exited with code {code}"),
            RenewEnd::Failed(None) => "its command was stopped by a signal".to_owned(),
            RenewEnd::NotRun(reason) => reason,
            RenewEnd::Success => unreachable!("handled above"),
        }));
    }
    forget_login(vault.registry(), actor, vault.station(), account, now)
}

/// Makes the requests waiting on logins this Station holds, oldest first, each audited before it
/// is made and cleared once it is done. One that cannot be made at all (a Station with no key, the
/// last Station, a sign-out that failed) is cleared with its reason on the login; one that only
/// waits (an account this Station does not declare, a lost race) stays for the next round.
///
/// # Errors
///
/// When the Registry cannot be read.
pub fn apply_requests(
    vault: &Vault,
    logins: &[(AccountRef, &Account)],
    commands: Commands<'_>,
    now: f64,
) -> io::Result<Vec<LoginNotice>> {
    let states = vault.registry().accounts()?;
    let mut notices = Vec::new();
    for state in &states {
        let Some(login) = state.login.as_ref().filter(|login| {
            !login.requests.is_empty() && login.stations.iter().any(|s| s == vault.station())
        }) else {
            continue;
        };
        let account = &state.account;
        for asked in &login.requests {
            let done = make(vault, logins, commands, account, asked, now);
            let Some(outcome) = done else {
                continue;
            };
            let text = match &outcome {
                Ok(text) => text.clone(),
                Err(error) => format!("{} was not made: {error}", describe(&asked.action)),
            };
            let result = match outcome {
                Ok(_) => None,
                Err(error) => Some(format!("{} failed: {error}", describe(&asked.action))),
            };
            let cleared = vault.registry().change_login(account, &mut |state| {
                if let Some(state) = state {
                    state.requests.retain(|known| known != asked);
                    if let Some(result) = &result {
                        state.last_result = Some(result.clone());
                        state.last_attempt_at = Some(now);
                    }
                }
            });
            if let Err(error) = cleared {
                eprintln!("ai6: account {account}'s request was not cleared: {error}");
            }
            notices.push(LoginNotice {
                account: account.clone(),
                kind: NoticeKind::Changed,
                text: format!("account {account}: {text}"),
            });
        }
    }
    Ok(notices)
}

/// Makes one request: `None` when it must wait, else how it ended.
fn make(
    vault: &Vault,
    logins: &[(AccountRef, &Account)],
    commands: Commands<'_>,
    account: &AccountRef,
    asked: &LoginRequest,
    now: f64,
) -> Option<Result<String, AdminError>> {
    let actor = format!("station:{}", vault.station());
    let by = &asked.by;
    match &asked.action {
        LoginRequestAction::AddStation { station } => {
            match add_station(vault, &actor, account, &Recipient::named(station), now) {
                Err(AdminError::Login(LoginError::Vault(VaultError::Conflict { .. }))) => None,
                other => {
                    Some(other.map(|_| format!("sealed to Station {station:?}, asked by {by}")))
                }
            }
        }
        LoginRequestAction::RemoveStation { station } => {
            match remove_station(vault, &actor, account, station, now) {
                Err(AdminError::Login(LoginError::Vault(VaultError::Conflict { .. }))) => None,
                other => Some(
                    other
                        .map(|_| format!("no longer sealed to Station {station:?}, asked by {by}")),
                ),
            }
        }
        LoginRequestAction::SignOutAndForget => {
            let (_, declared) = logins.iter().find(|(known, _)| known == account)?;
            Some(
                sign_out_and_forget(vault, commands, &actor, account, declared, now)
                    .map(|()| format!("signed out at the provider and forgotten, asked by {by}")),
            )
        }
    }
}

#[cfg(test)]
mod tests;
