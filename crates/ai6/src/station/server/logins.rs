//! A Station's side of harness logins kept sealed in the Registry: the key it opens them with, the
//! homes it writes them to, and what it does when a revision moves or a harness renews.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use crate::directorates::q::logins::{
    CommandRenewer, LoginNotice, NoticeKind, SessionLogin, Synced, assign_renewals, login_accounts,
    run_leases, sealed_name,
};
use crate::directorates::q::sealing::{SEALING_KEY_FILE, SealingKey, Vault};
use crate::directorates::q::{Account, AccountPool};
use crate::directorates::registry::accounts::AccountRef;
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::sealed::StationSealingKey;
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;

use super::DaemonState;
use super::accounts::Site;

/// The timeline kind of an account's event.
const TIMELINE_KIND: &str = "account";

/// The Station's vault, the homes it keeps current, and where its renewals stand.
#[derive(Clone, Default)]
pub(crate) struct LoginDesk {
    vault: Option<Arc<Vault>>,
    /// Every home this Station wrote a login to, by path.
    homes: Arc<Mutex<BTreeMap<PathBuf, SessionLogin>>>,
    /// What each home last failed with, so a failure that lasts is logged once.
    failed: Arc<Mutex<BTreeMap<PathBuf, String>>>,
    /// When this Station last ran the Headquarters round.
    last_round: Arc<Mutex<Option<Instant>>>,
    renewing: Arc<AtomicBool>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl LoginDesk {
    pub(crate) fn with_vault(mut self, vault: Arc<Vault>) -> Self {
        self.vault = Some(vault);
        self
    }

    pub(crate) fn vault(&self) -> Option<&Arc<Vault>> {
        self.vault.as_ref()
    }
}

/// Opens this Station's sealing key in `dir`, making it on first use, and publishes its public
/// half, vouched for by the identity in the same directory where this build has one.
///
/// # Errors
///
/// When the key cannot be read or made, or the Registry cannot be written.
pub(crate) fn open_vault(
    dir: &Path,
    registry: &Arc<dyn RegistryBackend>,
    station: &str,
) -> std::io::Result<Arc<Vault>> {
    let key = Arc::new(SealingKey::open_or_create(&dir.join(SEALING_KEY_FILE))?);
    registry.publish_sealing_key(&published(dir, station, &key))?;
    Ok(Arc::new(Vault::new(registry.clone(), station, key)))
}

#[cfg(feature = "station-tls")]
fn published(dir: &Path, station: &str, key: &SealingKey) -> StationSealingKey {
    let public = key.public_key().to_text();
    let signed = crate::station::identity::open_identity(dir)
        .map_err(|error| error.to_string())
        .and_then(|identity| {
            crate::station::identity::sign_sealing_key(&identity, station, &public)
        });
    signed.unwrap_or_else(|error| {
        eprintln!("ai6: this Station's sealing key is published unsigned: {error}");
        StationSealingKey::new(station, public.clone())
    })
}

#[cfg(not(feature = "station-tls"))]
fn published(_dir: &Path, station: &str, key: &SealingKey) -> StationSealingKey {
    StationSealingKey::new(station, key.public_key().to_text())
}

/// Picks up the homes an earlier run of this Station left, so what a harness renewed while the
/// Station was down still goes back, and removes any throwaway home a renewal left.
pub(super) fn recover(state: &DaemonState) {
    let desk = &state.station.credentials.accounts.logins;
    let Ok(root) = state.station.credentials.accounts.homes() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let mut homes = lock(&desk.homes);
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("renewing-") {
            if let Err(error) = std::fs::remove_dir_all(&path) {
                eprintln!("ai6: a renewal's home {name:?} was not removed: {error}");
            }
            continue;
        }
        let Ok(inside) = std::fs::read_dir(&path) else {
            continue;
        };
        for home in inside.flatten().filter(|home| home.path().is_dir()) {
            if let Some(login) = SessionLogin::recall(&home.path())
                && login.home == home.path()
            {
                homes.insert(home.path(), login);
            }
        }
    }
    drop(homes);
    tend(state);
}

/// Writes `account`'s login under the Mission's home and points the harness at it. A home it
/// already has, from an earlier Session of the Mission, is brought up to date first, so whatever
/// its last harness renewed goes back and a resume takes the current login.
pub(super) fn hand_over(
    state: &DaemonState,
    pool: &AccountPool,
    account: &Account,
    mission_id: &str,
    site: Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let name = format!("{}/{}", pool.id(), account.name());
    if !site.on_station {
        return Err(format!(
            "account {name}'s login is written to a home on the Station, so its harness must run \
             on the Station's own host"
        ));
    }
    let desk = &state.station.credentials.accounts;
    let vault = desk
        .logins
        .vault()
        .ok_or_else(|| format!("account {name}: this Station has no sealing key"))?;
    let home = desk
        .private_home(mission_id, pool, account)
        .map_err(|error| format!("account {name}'s home could not be made: {error}"))?;
    let reference = AccountRef::new(pool.id(), account.name());
    let mut homes = lock(&desk.logins.homes);
    let login = match homes.remove(&home) {
        Some(mut login) => {
            let now = state.clock.now() as f64;
            let held = vault
                .registry()
                .sealed_value(&sealed_name(&reference))
                .ok()
                .flatten()
                .map(|record| record.revision);
            match login.sync(vault, held, now) {
                Ok(synced) => report_sync(state, &login, &synced),
                Err(error) => {
                    homes.insert(home, login);
                    return Err(format!("account {name}'s login could not be read: {error}"));
                }
            }
            login
        }
        None => SessionLogin::materialize(vault, &reference, account.login_files(), &home)
            .map_err(|error| format!("account {name}'s login could not be written: {error}"))?,
    };
    let mut login = login;
    login.harness_starts();
    homes.insert(home.clone(), login);
    let home = home.display().to_string();
    environment.insert("HOME".to_owned(), home.clone());
    if cfg!(windows) {
        environment.insert("USERPROFILE".to_owned(), home);
    }
    Ok(())
}

/// One look at every home this Station keeps: what a harness renewed goes back under the revision
/// check, and a revision another Station wrote comes to the home. A home its Mission's end removed
/// is forgotten. Blocks on the Registry and the disk.
pub(super) fn tend(state: &DaemonState) {
    let desk = &state.station.credentials.accounts.logins;
    let Some(vault) = desk.vault() else {
        return;
    };
    let mut homes = lock(&desk.homes);
    homes.retain(|home, _| home.exists());
    if homes.is_empty() {
        return;
    }
    let revisions = match state.intake.sealed_revisions() {
        Ok(revisions) => revisions,
        Err(error) => {
            eprintln!("ai6: logins were not checked: {error}");
            return;
        }
    };
    let now = state.clock.now() as f64;
    let mut failed = lock(&desk.failed);
    for (home, login) in homes.iter_mut() {
        let held = revisions.get(&sealed_name(&login.account)).copied();
        match login.sync(vault, held, now) {
            Ok(synced) => {
                failed.remove(home);
                report_sync(state, login, &synced);
            }
            Err(error) => {
                let text = error.to_string();
                if failed.get(home) != Some(&text) {
                    eprintln!(
                        "ai6: account {}'s login was not kept current: {text}",
                        login.account
                    );
                    failed.insert(home.clone(), text);
                }
            }
        }
    }
}

fn report_sync(state: &DaemonState, login: &SessionLogin, synced: &Synced) {
    let account = &login.account;
    match synced {
        Synced::Unchanged => {}
        Synced::Rewritten(revision) => {
            eprintln!("ai6: account {account}'s login reached a Session at revision {revision}");
        }
        Synced::Renewed(revision) => {
            eprintln!("ai6: a harness renewed account {account}'s login: revision {revision}");
            audit(state, "pool.login.renewed", account);
        }
        Synced::Restored(revision, reason) => {
            eprintln!(
                "ai6: account {account}'s login in a Session's home was not kept ({reason}); it \
                 holds revision {revision} again"
            );
            audit(state, "pool.login.restored", account);
        }
    }
}

/// The Headquarters Station's round, at most every `[accounts] renew_every`; 0 runs none. Blocks on
/// the Registry.
pub(super) fn round(state: &DaemonState) {
    let limits = &state.station.limits().accounts;
    let every = limits.renew_every.0;
    let desk = &state.station.credentials.accounts.logins;
    if every.is_zero() {
        return;
    }
    {
        let mut last = lock(&desk.last_round);
        if last.is_some_and(|at| at.elapsed() < every) {
            return;
        }
        *last = Some(Instant::now());
    }
    let Some(catalog) = state.station.credentials.catalog() else {
        return;
    };
    let logins = login_accounts(catalog.pools());
    if logins.is_empty() {
        return;
    }
    let live: Vec<String> = state
        .intake
        .stations(state.intake.limits().stations.gone_dark_after.0)
        .unwrap_or_default()
        .iter()
        .map(|record| record.station().to_owned())
        .collect();
    let now = state.clock.now() as f64;
    match assign_renewals(
        state.intake.as_ref(),
        &logins,
        &state.station_id,
        &live,
        limits,
        now,
    ) {
        Ok(notices) => notices.iter().for_each(|notice| tell(state, notice)),
        Err(error) => eprintln!("ai6: the login renewal round did not run: {error}"),
    }
}

/// Whether this Station keeps logins that could be leased to it, and no run is going.
pub(super) fn begin_renewals(state: &DaemonState) -> bool {
    let desk = &state.station.credentials.accounts.logins;
    if desk.vault().is_none() {
        return false;
    }
    state
        .station
        .credentials
        .catalog()
        .is_some_and(|catalog| !login_accounts(catalog.pools()).is_empty())
        && !desk.renewing.swap(true, Ordering::AcqRel)
}

/// Runs the renewals this Station holds the lease of. Blocks on the commands: call it off the
/// scheduler's beat, between [`begin_renewals`] and [`end_renewals`].
pub(super) fn run_renewals(state: &DaemonState) {
    let accounts = &state.station.credentials.accounts;
    let (Some(vault), Some(catalog), Ok(scratch)) = (
        accounts.logins.vault(),
        state.station.credentials.catalog(),
        accounts.homes(),
    ) else {
        return;
    };
    let logins = login_accounts(catalog.pools());
    match run_leases(
        vault,
        &logins,
        &CommandRenewer,
        &scratch,
        &state.station.limits().accounts,
        state.clock.now() as f64,
    ) {
        Ok(notices) => notices.iter().for_each(|notice| tell(state, notice)),
        Err(error) => eprintln!("ai6: leased renewals did not run: {error}"),
    }
}

pub(super) fn end_renewals(state: &DaemonState) {
    state
        .station
        .credentials
        .accounts
        .logins
        .renewing
        .store(false, Ordering::Release);
}

/// A line for a Session that failed while the login it started with was renewed from under it: a
/// harness may hold its token in memory and be refused, and a resume takes the current login.
pub(super) fn stale_note(state: &DaemonState, mission_id: &str) -> Option<String> {
    let homes = lock(&state.station.credentials.accounts.logins.homes);
    let wanted = super::accounts::path_component(mission_id);
    let login = homes.values().find(|login| {
        login
            .home
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.to_string_lossy() == wanted)
    })?;
    let account = &login.account;
    let started = login.started;
    (login.revision > started).then(|| {
        format!(
            "account {account}'s login was renewed while this Session ran (revision {started} to \
             {}); a harness that keeps its token in memory may have been refused, and a resume \
             takes the current one",
            login.revision
        )
    })
}

/// Says what a round did: the log always; a person is told through the audit log and the timeline
/// of each Operation that may draw the account, and the board shows the account's state.
fn tell(state: &DaemonState, notice: &LoginNotice) {
    eprintln!("ai6: {}", notice.text);
    let action = match notice.kind {
        NoticeKind::Renewed => "pool.login.renewed",
        NoticeKind::NeedsPerson => "pool.login.needs_person",
        NoticeKind::MayLapse => "pool.login.may_lapse",
        NoticeKind::Leased(_) | NoticeKind::Unchanged | NoticeKind::Failed => return,
    };
    audit(state, action, &notice.account);
    if !matches!(notice.kind, NoticeKind::NeedsPerson | NoticeKind::MayLapse) {
        return;
    }
    let operations = state
        .intake
        .accounts()
        .ok()
        .and_then(|states| {
            states
                .into_iter()
                .find(|account| account.account == notice.account)
        })
        .and_then(|account| account.profile())
        .map(|profile| profile.operations)
        .unwrap_or_default();
    let now = state.clock.now() as f64;
    let event_action = action.rsplit('.').next().unwrap_or(action);
    for operation in operations {
        let event = TimelineEvent::new(TIMELINE_KIND, event_action, operation, now)
            .station(state.station_id.clone())
            .detail("account", notice.account.to_string())
            .detail("note", notice.text.as_str());
        if let Err(error) = state.intake.record_event(&event) {
            eprintln!("ai6: account event was not recorded: {error}");
        }
    }
}

fn audit(state: &DaemonState, action: &str, account: &AccountRef) {
    let record = AuditRecord {
        id: format!(
            "{}-{}",
            action.replace('.', "-"),
            uuid::Uuid::new_v4().simple()
        ),
        actor: state.station_id.clone(),
        action: action.to_owned(),
        operation: String::new(),
        resource: Some(format!("station:{}", state.station_id)),
        target: account.to_string(),
        at: state.clock.now() as f64,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: audit record {action} was not written: {error}");
    }
}
