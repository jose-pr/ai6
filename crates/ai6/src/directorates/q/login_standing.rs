//! A harness login's standing in the Registry and the changes a person asks of it that need no
//! key: what a command or the gateway shows and records. Opening or sealing a login is
//! [`login_admin`](super::login_admin)'s, which needs a Station's key.

use std::io;

use crate::directorates::registry::accounts::{
    AccountRef, LoginRequest, LoginRequestAction, LoginState,
};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::store::AuditRecord;

/// The name of the sealed value holding `account`'s login.
pub fn sealed_name(account: &AccountRef) -> String {
    format!("login/{}/{}", account.pool, account.account)
}

/// Writes the audit record of an act; the act waits for it, and is not made when this fails.
///
/// # Errors
///
/// When the Registry refuses the record.
pub fn audit_first(
    registry: &dyn RegistryBackend,
    actor: &str,
    action: &str,
    operation: &str,
    target: &str,
    resource: Option<String>,
    now: f64,
) -> io::Result<()> {
    let record = AuditRecord {
        id: format!(
            "{}-{}",
            action.replace('.', "-"),
            uuid::Uuid::new_v4().simple()
        ),
        actor: actor.to_owned(),
        action: action.to_owned(),
        operation: operation.to_owned(),
        resource,
        target: target.to_owned(),
        at: now,
    };
    registry.record_audit(&record)
}

/// Whether `account` holds a login: a sealed value or a recorded standing.
///
/// # Errors
///
/// When the Registry cannot be read.
pub fn holds_login(registry: &dyn RegistryBackend, account: &AccountRef) -> io::Result<bool> {
    if registry.sealed_value(&sealed_name(account))?.is_some() {
        return Ok(true);
    }
    Ok(registry
        .accounts()?
        .iter()
        .any(|state| state.account == *account && state.login.is_some()))
}

/// Forgets `account`'s login: the sealed value and the standing kept beside it. Needs no key.
/// Whether it held one. The audit is the caller's, and comes first.
///
/// # Errors
///
/// When the Registry cannot be read or written.
pub fn forget(registry: &dyn RegistryBackend, account: &AccountRef) -> io::Result<bool> {
    if !holds_login(registry, account)? {
        return Ok(false);
    }
    registry.remove_sealed(&sealed_name(account))?;
    registry.change_login(account, &mut |state| *state = None)?;
    Ok(true)
}

/// Records a change that needs a Station holding the login, for one to make at its next check-in;
/// whether the account holds a login. The act's audit is the caller's, and comes first.
///
/// # Errors
///
/// When the Registry cannot be written.
pub fn request(
    registry: &dyn RegistryBackend,
    account: &AccountRef,
    action: LoginRequestAction,
    by: &str,
    now: f64,
) -> io::Result<bool> {
    let mut held = false;
    registry.change_login(account, &mut |state| {
        held = false;
        if let Some(state) = state {
            held = true;
            let asked = LoginRequest {
                action: action.clone(),
                by: by.to_owned(),
                at: now,
            };
            if !state
                .requests
                .iter()
                .any(|known| known.action == asked.action)
            {
                state.requests.push(asked);
            }
        }
    })?;
    Ok(held)
}

/// A request as a person reads it.
pub fn describe(action: &LoginRequestAction) -> String {
    match action {
        LoginRequestAction::AddStation { station } => format!("adding Station {station:?}"),
        LoginRequestAction::RemoveStation { station } => format!("removing Station {station:?}"),
        LoginRequestAction::SignOutAndForget => "signing out and forgetting the login".to_owned(),
    }
}

/// The login's state as the CLI shows it: one line, no contents.
pub fn standing(login: &LoginState, now: f64) -> String {
    let mut parts = vec![format!("sealed to {}", login.stations.join(", "))];
    if let Some(end) = login.expires_at {
        parts.push(format!("ends in {}", span(end - now)));
    }
    if let Some(at) = login.renewed_at {
        parts.push(format!("renewed {} ago", span(now - at)));
    }
    if let Some(lease) = login.leased_at(now) {
        parts.push(format!("renewal held by {}", lease.station));
    }
    if let Some(result) = &login.last_result {
        parts.push(format!("last: {result}"));
    }
    if let Some(why) = &login.needs_person {
        parts.push(format!("NEEDS A PERSON: {why}"));
    }
    for asked in &login.requests {
        parts.push(format!("waiting: {}", describe(&asked.action)));
    }
    parts.join("; ")
}

/// Seconds as a person reads a span, a negative one as `0s`.
fn span(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    match seconds {
        0..=119 => format!("{seconds}s"),
        120..=7199 => format!("{}m", seconds / 60),
        7200..=172_799 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// What to run, on a host where the CLI is signed in, to give an account a login again.
pub fn add_login_command(account: &AccountRef, handler: &str) -> String {
    format!(
        "ai6 pools add-login {} {} --handler {handler} --replace",
        account.pool, account.account
    )
}
