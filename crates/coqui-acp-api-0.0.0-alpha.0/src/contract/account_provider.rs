//! The checks of an `account_provider`: what it offers, signing in, renewing, watching and usage.
//!
//! The suite hands a provider a spawner that is virtual (it starts nothing: it records the specs it
//! is asked to start and hands back idle processes) and serves files in a directory of the suite's,
//! a prompter that answers every form with canary secrets, and a keeper that records what it is
//! told. An account is made the way a host makes one, by signing in through each method the
//! provider lists; a kind no method makes is given a grant of canary credentials.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use std::path::PathBuf;
use std::time::Instant;

use super::accounts::{
    BAD_VALUE, GONE, ROTATED, SECOND, SignIn, UNSUPPORTED, distinct, grant_of, inputs_for,
    kind_rules, login_rules, sign_in, why,
};
use super::checks::{CANARY, Outcome, Place, text};
use super::host::{self, GRACE, PATIENCE, Script, status};
use super::{Failure, Failures};
use crate::guatiao::library::{Object, ProviderError};
use crate::usage::Refill;
use crate::{
    AccountGrant, AccountKindInfo, AccountLoss, AccountOffer, AccountProvider, AccountRenewal,
    AccountUsageQuery, AccountWatch, Credentials, Environment, LoginMethodInfo, LoginOutcome,
    LoginRequest, RenewRequest, WatchRequest,
};

macro_rules! ensure {
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            return Err(format!($($arg)+));
        }
    };
}

/// What an `account_provider` is held to, and how it is driven. From [`AccountContract::new`].
#[derive(Debug, Clone)]
#[must_use]
pub struct AccountContract {
    name: String,
    logins: bool,
    kind: Option<String>,
}

impl AccountContract {
    /// The contract for a provider called `name` in the failures.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            logins: true,
            kind: None,
        }
    }

    /// Whether the provider offers login methods (the default). One that does not must list none
    /// and refuse `login` with `GUATIAO_ERR_UNSUPPORTED` and a reason.
    pub const fn logins(mut self, logins: bool) -> Self {
        self.logins = logins;
        self
    }

    /// Checks the account checks against this kind alone, in place of every kind the provider
    /// lists. The offer checks still read all of them.
    pub fn account(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Runs every check against `provider`.
    ///
    /// # Errors
    ///
    /// Every rule it broke.
    pub fn run(&self, provider: &dyn AccountProvider) -> Result<(), Failures> {
        let _running = host::Running::on(false);
        let failures: Vec<Failure> = all(self, provider)
            .into_iter()
            .filter_map(|(check, outcome)| outcome.err().map(|message| Failure { check, message }))
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Failures {
                provider: self.name.clone(),
                failures,
            })
        }
    }
}

fn all(contract: &AccountContract, provider: &dyn AccountProvider) -> Vec<(&'static str, Outcome)> {
    let place = match Place::make() {
        Ok(place) => place,
        Err(why) => return vec![("the suite's directories", Err(why))],
    };
    let checks = vec![
        ("describes itself", describes(provider)),
        ("lists what it offers", offers(contract, provider)),
        ("signs in as it lists", signs_in(contract, provider, &place)),
        ("renews as it lists", renews(contract, provider, &place)),
        ("watches as it lists", watches(contract, provider, &place)),
        (
            "reports plan usage as it lists",
            usage(contract, provider, &place),
        ),
    ];
    place.remove();
    checks
}

fn offer_of(provider: &dyn AccountProvider) -> Result<AccountOffer, String> {
    provider.offers().map_err(|e| why(&e))
}

fn describes(provider: &dyn AccountProvider) -> Outcome {
    let info = provider.describe().map_err(|e| why(&e))?;
    ensure!(!info.title.is_empty(), "describe gave no title");
    ensure!(
        info == provider.describe().map_err(|e| why(&e))?,
        "describe answered differently on a second call"
    );
    Ok(())
}

/// The kinds the suite checks: all, or the one the contract names.
fn kinds_of(
    contract: &AccountContract,
    offer: &AccountOffer,
) -> Result<Vec<AccountKindInfo>, String> {
    match &contract.kind {
        None => Ok(offer.kinds.clone()),
        Some(wanted) => {
            let kind = offer
                .kind(wanted)
                .ok_or_else(|| format!("the provider lists no kind {wanted:?}"))?;
            Ok(vec![kind.clone()])
        }
    }
}

fn offers(contract: &AccountContract, provider: &dyn AccountProvider) -> Outcome {
    let offer = offer_of(provider)?;
    ensure!(
        offer == offer_of(provider)?,
        "offers() answered differently on a second call"
    );
    ensure!(
        !offer.kinds.is_empty(),
        "the provider offers no kind of account"
    );
    kind_rules(&offer.kinds)?;
    for info in &offer.kinds {
        ensure!(
            !info.provides.is_empty(),
            "{} provides no standard, so no harness could be given an account of it",
            info.id
        );
    }
    login_rules(&offer.kinds, &offer.logins)?;
    let mut listed = Vec::new();
    for id in &offer.usage {
        ensure!(
            offer.kind(id).is_some(),
            "usage names {id}, which is not a listed kind"
        );
        ensure!(!listed.contains(&id), "usage names {id} twice");
        listed.push(id);
    }
    if contract.logins {
        ensure!(
            !offer.logins.is_empty(),
            "the provider lists no login method; one with none is run with logins off"
        );
    } else {
        ensure!(
            offer.logins.is_empty(),
            "logins are off, and the provider lists {} method(s)",
            offer.logins.len()
        );
    }
    for info in &offer.kinds {
        if info.renews {
            ensure!(
                info.state_dir || !info.rotates,
                "{} renews and rotates but keeps no directory, so nothing could be watched",
                info.id
            );
        }
    }
    kinds_of(contract, &offer)?;
    Ok(())
}

fn signs_in(contract: &AccountContract, provider: &dyn AccountProvider, place: &Place) -> Outcome {
    let offer = offer_of(provider)?;
    let login = |request, spawner, prompter| provider.login(request, spawner, prompter);
    let made = |method: &LoginMethodInfo, outcome: &LoginOutcome, watch: &host::Watch| -> Outcome {
        let label = method.id.as_str();
        ensure!(
            watch.specs().iter().all(|spec| {
                !format!("{:?}", spec.args).contains(CANARY)
                    && !spec
                        .environment
                        .expose()
                        .iter()
                        .any(|(_, v)| v.contains(CANARY))
            }),
            "{label}: the sign-in's program was handed a secret the person typed on its command line or in its \
             environment"
        );
        ensure!(
            offer.kind(&outcome.kind).is_some(),
            "{label}: it made a kind the provider does not list"
        );
        ensure!(
            outcome.kind == method.produces,
            "{label}: it produces {}, and made {}",
            method.produces,
            outcome.kind
        );
        ensure!(
            !outcome.credentials.expose().is_empty(),
            "{label}: it made an account with no credentials"
        );
        let shown = format!(
            "{} {:?}",
            super::json::to_json(&outcome.settings),
            (&outcome.label, &outcome.expires_at)
        );
        ensure!(
            !shown.contains(CANARY),
            "{label}: the outcome's settings or label carry a secret"
        );
        Ok(())
    };
    sign_in(
        place,
        &SignIn {
            offers: contract.logins,
            methods: &offer.logins,
            only: contract.kind.as_deref(),
            login: &login,
            made: &made,
        },
    )
}

/// An account the suite made for a kind: by signing in through a method of the provider, or, for a
/// kind no method makes, from canary credentials.
struct Made {
    info: AccountKindInfo,
    grant: AccountGrant,
    dir: PathBuf,
    /// The file in `dir` that holds its credential, when the sign-in wrote one.
    file: Option<String>,
    by_login: bool,
}

fn holding(writes: &[(String, Vec<u8>)], credentials: &Credentials) -> Option<String> {
    writes
        .iter()
        .find(|(_, bytes)| {
            let content = String::from_utf8_lossy(bytes);
            credentials
                .iter()
                .any(|(_, value)| !value.is_empty() && content.contains(value.as_str()))
        })
        .map(|(path, _)| path.clone())
}

/// A new account of `info`.
fn make(
    provider: &dyn AccountProvider,
    offer: &AccountOffer,
    info: &AccountKindInfo,
    place: &Place,
) -> Result<Made, String> {
    let dir = place.scratch()?;
    let Some(method) = offer.logins.iter().find(|m| m.produces == info.id) else {
        let grant = grant_of(info, Some(&dir), 1)?;
        return Ok(Made {
            info: info.clone(),
            grant,
            dir,
            file: None,
            by_login: false,
        });
    };
    let (watch, spawner, prompter) = host::login_in(&dir, Script::default());
    let outcome = provider
        .login(
            LoginRequest::new(&method.id, text(&dir)).inputs(inputs_for(method)?),
            spawner,
            prompter,
        )
        .map_err(|e| format!("{}: login: {}", method.id, why(&e)))?;
    ensure!(
        outcome.kind == info.id,
        "{}: it made {}, not the {} it produces",
        method.id,
        outcome.kind,
        info.id
    );
    let mut grant = AccountGrant::new("contract-signed-in", &info.id)
        .settings(outcome.settings.clone())
        .credentials(outcome.credentials.expose().clone())
        .provides(info.provides.clone())
        .revision(1);
    if info.state_dir {
        grant = grant.state_dir(text(&dir));
    }
    Ok(Made {
        info: info.clone(),
        grant,
        dir,
        file: holding(&watch.writes(), outcome.credentials.expose()),
        by_login: true,
    })
}

/// What a call on a refused or answered request leaves of the objects it was handed.
fn objects(label: &str, watch: &host::Watch) -> Outcome {
    ensure!(
        watch.destroyed(),
        "{label}: it answered with an object it was handed alive"
    );
    Ok(())
}

/// Requests the provider must refuse with `GUATIAO_ERR_BAD_VALUE`, whatever the method: a kind it
/// does not list, and an id that is not an account id.
fn bad_grants(grant: &AccountGrant) -> Vec<(&'static str, AccountGrant)> {
    let mut unlisted = grant.clone();
    unlisted.kind = String::from("no-such-kind-of-account");
    let mut empty = grant.clone();
    empty.id = String::new();
    let mut long = grant.clone();
    long.id = "x".repeat(crate::ACCOUNT_ID_MAX + 1);
    vec![
        ("a kind it does not list", unlisted),
        ("an empty id", empty),
        ("an id past 128 bytes", long),
    ]
}

fn refused_as_bad(label: &str, watch: &host::Watch, answer: Result<(), ProviderError>) -> Outcome {
    let error = answer
        .err()
        .ok_or_else(|| format!("{label}: it answered"))?;
    ensure!(
        status(&error) == BAD_VALUE
            && !error.message().is_empty()
            && !error.message().contains(CANARY),
        "{label}: it refused with {}, not {BAD_VALUE} and a reason without a secret",
        why(&error)
    );
    objects(label, watch)
}

fn renews(contract: &AccountContract, provider: &dyn AccountProvider, place: &Place) -> Outcome {
    let offer = offer_of(provider)?;
    for info in kinds_of(contract, &offer)? {
        let account = make(provider, &offer, &info, place)?;
        let grant = &account.grant;
        let renew = |grant: &AccountGrant| {
            let (watch, spawner, keeper) = host::spawner_in(Some(&account.dir));
            drop(keeper);
            let answer = provider.renew(RenewRequest::new(grant.clone()), spawner);
            (watch, answer)
        };
        for (what, bad) in bad_grants(grant) {
            let label = format!("{}: renew of {what}", info.id);
            let (watch, answer) = renew(&bad);
            refused_as_bad(&label, &watch, answer.map(|_| ()))?;
        }
        let (watch, answer) = renew(grant);
        ensure!(
            watch.spawner_destroyed(),
            "{}: renew answered with the spawner it was handed alive",
            info.id
        );
        if info.renews {
            // A kind no sign-in made has no login file to renew from: it may say so, as GONE.
            let report = match answer {
                Ok(report) => report,
                Err(error) if !account.by_login && status(&error) == GONE => continue,
                Err(error) => return Err(format!("{}: renew: {}", info.id, why(&error))),
            };
            ensure!(
                report.credentials.is_some()
                    || report
                        .not_renewed_because
                        .as_deref()
                        .is_some_and(|reason| !reason.is_empty()),
                "{}: renew changed nothing and gave no reason",
                info.id
            );
            ensure!(
                !format!("{:?} {:?}", report.not_renewed_because, report.expires_at)
                    .contains(CANARY),
                "{}: renew's report carried a credential",
                info.id
            );
            // A login the provider lost is GONE, whatever is left of its file: never a renewal
            // made of nothing.
            if let Some(path) = &account.file {
                let file = account.dir.join(path);
                for (how, blank) in [("blank", true), ("missing", false)] {
                    if blank {
                        std::fs::write(&file, "").map_err(|e| format!("{}: {e}", info.id))?;
                    } else {
                        std::fs::remove_file(&file).map_err(|e| format!("{}: {e}", info.id))?;
                    }
                    let (_, lost) = renew(grant);
                    let error = lost.err().ok_or_else(|| {
                        format!("{}: renew of a {how} login file answered a report", info.id)
                    })?;
                    ensure!(
                        status(&error) == GONE
                            && !error.message().is_empty()
                            && !error.message().contains(CANARY),
                        "{}: renew of a {how} login file refused with {}, not {GONE} and a reason",
                        info.id,
                        why(&error)
                    );
                }
            }
        } else {
            let error = answer.err().ok_or_else(|| {
                format!(
                    "{}: renew answered where the provider says it cannot",
                    info.id
                )
            })?;
            ensure!(
                status(&error) == UNSUPPORTED
                    && !error.message().is_empty()
                    && !error.message().contains(CANARY),
                "{}: renew refused with {}, not {UNSUPPORTED} and a reason",
                info.id,
                why(&error)
            );
        }
    }
    Ok(())
}

/// A watch the suite holds, and what it was handed.
struct Watching {
    watch: Object<dyn AccountWatch>,
    seen: host::Watch,
}

fn begin_watch(provider: &dyn AccountProvider, account: &Made) -> Result<Watching, String> {
    let (seen, spawner, keeper) = host::spawner_in(Some(&account.dir));
    let watch = provider
        .watch(WatchRequest::new(account.grant.clone()), spawner, keeper)
        .map_err(|e| format!("{}: watch: {}", account.info.id, why(&e)))?;
    Ok(Watching { watch, seen })
}

fn renewals(watches: &[&Watching]) -> Vec<Vec<AccountRenewal>> {
    watches
        .iter()
        .map(|w| distinct(w.seen.renewed().into_iter(), |e| (e.id.clone(), e.sequence)))
        .collect()
}

fn losses(watches: &[&Watching]) -> Vec<Vec<AccountLoss>> {
    watches
        .iter()
        .map(|w| distinct(w.seen.lost().into_iter(), |e| (e.id.clone(), e.sequence)))
        .collect()
}

/// Waits up to `patience` until `done` holds of the numbers of renewals and losses each watch heard.
fn until(watches: &[&Watching], patience: Duration, done: impl Fn(usize, usize) -> bool) {
    let deadline = Instant::now() + patience;
    loop {
        let every = renewals(watches)
            .iter()
            .zip(losses(watches))
            .all(|(r, l)| done(r.len(), l.len()));
        if every || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Every watch heard each event once, and heard the same events as the first.
fn same_events(label: &str, watches: &[&Watching], renewed: usize, lost: usize) -> Outcome {
    let heard_renewed = renewals(watches);
    let heard_lost = losses(watches);
    for (index, (r, l)) in heard_renewed.iter().zip(&heard_lost).enumerate() {
        ensure!(
            r.len() == renewed && l.len() == lost,
            "{label}: watch {index} heard {} renewal(s) and {} loss(es), not {renewed} and {lost}",
            r.len(),
            l.len()
        );
        ensure!(
            watches[index].seen.renewed().len() == r.len()
                && watches[index].seen.lost().len() == l.len(),
            "{label}: watch {index} told its keeper of one event more than once"
        );
    }
    for (r, l) in heard_renewed.iter().zip(&heard_lost).skip(1) {
        ensure!(
            r.iter().map(|e| e.sequence).collect::<Vec<_>>()
                == heard_renewed[0]
                    .iter()
                    .map(|e| e.sequence)
                    .collect::<Vec<_>>()
                && l.iter().map(|e| e.sequence).collect::<Vec<_>>()
                    == heard_lost[0].iter().map(|e| e.sequence).collect::<Vec<_>>(),
            "{label}: two watches of one account heard events with different sequences"
        );
    }
    Ok(())
}

fn watches(contract: &AccountContract, provider: &dyn AccountProvider, place: &Place) -> Outcome {
    let offer = offer_of(provider)?;
    for info in kinds_of(contract, &offer)? {
        let watchable = info.rotates && info.state_dir;
        let account = make(provider, &offer, &info, place)?;
        for (what, bad) in bad_grants(&account.grant) {
            let label = format!("{}: watch of {what}", info.id);
            let (seen, spawner, keeper) = host::spawner_in(Some(&account.dir));
            let answer = provider.watch(WatchRequest::new(bad), spawner, keeper);
            refused_as_bad(&label, &seen, answer.map(|_| ()))?;
        }
        if !watchable {
            let (seen, spawner, keeper) = host::spawner_in(Some(&account.dir));
            let error = provider
                .watch(WatchRequest::new(account.grant.clone()), spawner, keeper)
                .err()
                .ok_or_else(|| {
                    format!(
                        "{}: watch answered for a kind that does not both rotate and keep a directory",
                        info.id
                    )
                })?;
            ensure!(
                status(&error) == UNSUPPORTED
                    && !error.message().is_empty()
                    && !error.message().contains(CANARY),
                "{}: watch refused with {}, not {UNSUPPORTED} and a reason",
                info.id,
                why(&error)
            );
            objects(&format!("{}: a refused watch", info.id), &seen)?;
            continue;
        }
        if !account.by_login {
            // No sign-in makes this kind, so the suite does not know the file its credential is
            // in: it holds the watch to starting, and to letting go of what it was handed
            // when stopped.
            let one = begin_watch(provider, &account)?;
            std::thread::sleep(GRACE);
            one.watch
                .stop()
                .map_err(|e| format!("{}: stop: {}", info.id, why(&e)))?;
            ensure!(
                one.seen.destroyed(),
                "{}: a stopped watch kept the spawner or the keeper it was handed",
                info.id
            );
            continue;
        }
        ensure!(
            account.file.is_some(),
            "{}: it rotates and keeps a directory, but its sign-in left no file holding the              credential, so nothing shows that it watches",
            info.id
        );
        for scenario in [
            "rotated twice then blanked",
            "deleted",
            "gone before the watch",
        ] {
            let label = format!("{} ({scenario})", info.id);
            let account = make(provider, &offer, &info, place)?;
            let path = account.file.clone().ok_or_else(|| {
                format!("{label}: the sign-in left no file holding its credential")
            })?;
            let file = account.dir.join(&path);
            let original = std::fs::read_to_string(&file)
                .map_err(|e| format!("{label}: the file {path} cannot be read: {e}"))?;
            let grant_revision = account.grant.revision;
            if scenario == "gone before the watch" {
                std::fs::remove_file(&file).map_err(|e| format!("{label}: {e}"))?;
                let one = begin_watch(provider, &account)?;
                until(&[&one], PATIENCE, |_, lost| lost >= 1);
                same_events(&label, &[&one], 0, 1)?;
                one.watch
                    .stop()
                    .map_err(|e| format!("{label}: stop: {}", why(&e)))?;
                continue;
            }
            let one = begin_watch(provider, &account)?;
            let two = begin_watch(provider, &account)?;
            let both = [&one, &two];
            // Nothing changed: nothing is told.
            std::thread::sleep(GRACE);
            same_events(&format!("{label}, untouched"), &both, 0, 0)?;
            if scenario == "deleted" {
                std::fs::remove_file(&file).map_err(|e| format!("{label}: {e}"))?;
                until(&both, PATIENCE, |_, lost| lost >= 1);
                same_events(&label, &both, 0, 1)?;
                let loss = &losses(&both)[0][0];
                ensure!(
                    loss.id == account.grant.id
                        && loss.sequence >= 1
                        && !loss.reason.is_empty()
                        && !loss.reason.contains(CANARY),
                    "{label}: the loss is {loss:?}"
                );
                std::thread::sleep(GRACE);
                same_events(&format!("{label}, later"), &both, 0, 1)?;
            } else {
                let rotated = original.replace(&credential_text(&account), ROTATED);
                ensure!(
                    rotated != original,
                    "{label}: the file does not hold the credential"
                );
                std::fs::write(&file, &rotated).map_err(|e| format!("{label}: {e}"))?;
                until(&both, PATIENCE, |renewed, _| renewed >= 1);
                same_events(&label, &both, 1, 0)?;
                let first = renewals(&both)[0][0].clone();
                ensure!(
                    first.id == account.grant.id
                        && first.from_revision == grant_revision
                        && first.sequence >= 1,
                    "{label}: the renewal is for {} at revision {}, sequence {}",
                    first.id,
                    first.from_revision,
                    first.sequence
                );
                let held = first.credentials.expose();
                ensure!(
                    held.iter().any(|(_, value)| value.contains(ROTATED)),
                    "{label}: the renewal does not hold the rotated credential"
                );
                std::thread::sleep(GRACE);
                same_events(&format!("{label}, settled"), &both, 1, 0)?;

                // A second rotation, chained on the first: a later sequence.
                let again = rotated.replace(ROTATED, SECOND);
                std::fs::write(&file, &again).map_err(|e| format!("{label}: {e}"))?;
                until(&both, PATIENCE, |renewed, _| renewed >= 2);
                same_events(&label, &both, 2, 0)?;
                let second = renewals(&both)[0]
                    .iter()
                    .find(|e| e.sequence != first.sequence)
                    .cloned()
                    .ok_or("the second rotation has the first one's sequence")?;
                ensure!(
                    second.sequence > first.sequence
                        && second
                            .credentials
                            .expose()
                            .iter()
                            .any(|(_, value)| value.contains(SECOND)),
                    "{label}: the second rotation has sequence {} after {}",
                    second.sequence,
                    first.sequence
                );

                // Blanked: lost, never renewed, and later than both.
                std::fs::write(&file, "").map_err(|e| format!("{label}: {e}"))?;
                until(&both, PATIENCE, |_, lost| lost >= 1);
                same_events(&label, &both, 2, 1)?;
                let loss = losses(&both)[0][0].clone();
                ensure!(
                    loss.id == account.grant.id
                        && !loss.reason.is_empty()
                        && !loss.reason.contains(CANARY)
                        && loss.sequence > second.sequence,
                    "{label}: the loss is {loss:?}, after sequence {}",
                    second.sequence
                );

                // A stopped watch destroys what it was handed and tells nothing more; the other
                // goes on.
                one.watch
                    .stop()
                    .map_err(|e| format!("{label}: stop: {}", why(&e)))?;
                one.watch
                    .stop()
                    .map_err(|e| format!("{label}: a second stop: {}", why(&e)))?;
                ensure!(
                    one.seen.destroyed(),
                    "{label}: a stopped watch kept the spawner or the keeper it was handed"
                );
                std::fs::write(&file, &original).map_err(|e| format!("{label}: {e}"))?;
                until(&[&two], PATIENCE, |renewed, _| renewed >= 3);
                ensure!(
                    renewals(&[&two])[0].len() == 3,
                    "{label}: a restored login was not told as renewed"
                );
                std::thread::sleep(GRACE);
                ensure!(
                    renewals(&[&one])[0].len() == 2 && losses(&[&one])[0].len() == 1,
                    "{label}: a stopped watch told its keeper of a change"
                );
            }
            ensure!(
                !format!(
                    "{:?}",
                    both.iter().map(|w| w.seen.said()).collect::<Vec<_>>()
                )
                .contains(ROTATED),
                "{label}: a credential came back through the prompter"
            );
            let Watching {
                watch: dropped,
                seen,
            } = two;
            drop(dropped);
            ensure!(
                seen.destroyed(),
                "{label}: a watch that was dropped kept the spawner or the keeper it was handed"
            );
        }
    }
    Ok(())
}

/// The text of the credential file the sign-in made, as the grant holds it.
fn credential_text(account: &Made) -> String {
    account
        .grant
        .credentials
        .expose()
        .iter()
        .map(|(_, value)| value.clone())
        .find(|value| {
            account.file.as_ref().is_some_and(|path| {
                std::fs::read_to_string(account.dir.join(path))
                    .is_ok_and(|c| c.contains(value.as_str()))
            })
        })
        .unwrap_or_default()
}

fn usage(contract: &AccountContract, provider: &dyn AccountProvider, place: &Place) -> Outcome {
    let offer = offer_of(provider)?;
    for info in kinds_of(contract, &offer)? {
        let account = make(provider, &offer, &info, place)?;
        let ask = |grant: &AccountGrant| {
            let (watch, spawner, keeper) = host::spawner_in(Some(&account.dir));
            let mut environment = Environment::new();
            environment.insert("COQUI_ACP_CONTRACT_VARIABLE", CANARY);
            let answer = provider.usage(
                AccountUsageQuery::new(grant.clone()).environment(environment),
                spawner,
                keeper,
            );
            (watch, answer)
        };
        for (what, bad) in bad_grants(&account.grant) {
            let label = format!("{}: usage of {what}", info.id);
            let (watch, answer) = ask(&bad);
            refused_as_bad(&label, &watch, answer.map(|_| ()))?;
        }
        let (watch, answer) = ask(&account.grant);
        let report = answer.map_err(|e| format!("{}: usage: {}", info.id, why(&e)))?;
        objects(&format!("{}: usage", info.id), &watch)?;
        ensure!(
            !format!("{report:?}").contains(CANARY),
            "{}: the usage report carries a value of the environment or a credential",
            info.id
        );
        ensure!(
            report
                .account
                .as_deref()
                .is_none_or(|id| id == account.grant.id),
            "{}: the report names the account {:?}, not {}",
            info.id,
            report.account,
            account.grant.id
        );
        if !offer.reports_usage(&info.id) {
            ensure!(
                !report.supported
                    && report
                        .unsupported_because
                        .as_deref()
                        .is_some_and(|reason| !reason.is_empty())
                    && report.buckets.is_empty(),
                "{}: it lists no usage for the kind, and did not answer `not offered` with a reason and no bucket",
                info.id
            );
            continue;
        }
        ensure!(
            report.supported && report.unsupported_because.is_none(),
            "{}: it lists usage for the kind and answered `not offered`",
            info.id
        );
        ensure!(
            report.account.as_deref() == Some(account.grant.id.as_str()),
            "{}: the report does not name its account",
            info.id
        );
        let mut ids = Vec::new();
        for bucket in &report.buckets {
            let label = format!("{}: bucket {:?}", info.id, bucket.id);
            ensure!(!bucket.id.is_empty(), "{}: a bucket has no id", info.id);
            ensure!(!ids.contains(&&bucket.id), "{label} is listed twice");
            ids.push(&bucket.id);
            ensure!(
                bucket.observed_at > 0 && bucket.observed_at <= report.observed_at,
                "{label}: observed_at {} is not within the report's {}",
                bucket.observed_at,
                report.observed_at
            );
            if let (Some(used), Some(limit)) = (bucket.used, bucket.limit) {
                ensure!(
                    used <= limit || limit == 0,
                    "{label}: {used} used of {limit}"
                );
            }
            if let (Some(left), Some(limit)) = (bucket.remaining, bucket.limit) {
                ensure!(left <= limit, "{label}: {left} left of {limit}");
            }
            if let Refill::FixedWindow {
                resets_at: Some(at),
                ..
            } = &bucket.refill
            {
                ensure!(*at > 0, "{label}: its window resets at {at}");
            }
            ensure!(
                bucket.used.is_some() || bucket.remaining.is_some(),
                "{label}: it says neither how much is used nor how much is left"
            );
        }
        ensure!(
            report
                .buckets
                .iter()
                .map(|b| b.observed_at)
                .max()
                .unwrap_or(0)
                == report.observed_at,
            "{}: the report's observed_at is not its newest bucket's",
            info.id
        );
        let (_, again) = ask(&account.grant);
        let again = again.map_err(|e| format!("{}: usage, again: {}", info.id, why(&e)))?;
        ensure!(
            again.revision >= report.revision,
            "{}: the report's revision went from {} to {}",
            info.id,
            report.revision,
            again.revision
        );
    }
    Ok(())
}
