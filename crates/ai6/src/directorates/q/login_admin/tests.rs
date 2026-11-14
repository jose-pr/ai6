//! What a person does to a login: add it from a host, change its Stations, forget it, sign it out.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::directorates::q::pools::AccountKind;
use crate::directorates::q::sealing::SealingKey;
use crate::directorates::registry::accounts::LoginState;
use crate::directorates::registry::sealed::{SealedStore as _, StationSealingKey};
use crate::directorates::registry::suite::{Store, TestRegistry, backend_tests};

const NOW: f64 = 1_790_000_000.0;
const FILE: &str = ".fake/auth.json";
/// Never to be seen anywhere but in a login file and what a Station opens.
const PLANTED: &str = "planted-refresh-0451";
const TIMEOUT: Duration = Duration::from_secs(5);

fn reference() -> AccountRef {
    AccountRef::new("fake", "shared")
}

fn declared(logout: bool) -> Account {
    let account = Account::new("shared", AccountKind::HarnessLogin).files([FILE]);
    if logout {
        account.logout_command(["fake-logout"])
    } else {
        account
    }
}

fn station(registry: &Arc<dyn TestRegistry>, name: &str) -> Vault {
    let key = Arc::new(SealingKey::generate());
    registry
        .publish_sealing_key(&StationSealingKey::new(name, key.public_key().to_text()))
        .unwrap();
    let backend: Arc<dyn RegistryBackend> = registry.clone();
    Vault::new(backend, name, key)
}

/// A host the CLI is signed in on.
fn host(root: &Path) -> std::path::PathBuf {
    let dir = root.join("logged-in");
    std::fs::create_dir_all(dir.join(".fake")).unwrap();
    std::fs::write(
        dir.join(FILE),
        format!(
            r#"{{"access_token":"a","refresh_token":"{PLANTED}","expires_at":{}}}"#,
            NOW + 86_400.0
        ),
    )
    .unwrap();
    dir
}

fn add(vault: &Vault, dir: &Path, stations: &[&str], replace: bool) -> Result<u64, AdminError> {
    let named: Vec<Recipient> = stations
        .iter()
        .map(|name| Recipient::named(*name))
        .collect();
    add_login(
        vault,
        "person",
        &reference(),
        &declared(true),
        Source {
            from: dir,
            stations: &named,
            replace,
        },
        NOW,
    )
}

fn commands<'a>(renewer: &'a dyn LoginRenewer, scratch: &'a Path) -> Commands<'a> {
    Commands {
        renewer,
        scratch,
        timeout: TIMEOUT,
    }
}

fn actions(registry: &dyn RegistryBackend) -> Vec<(String, String, String)> {
    let mut records = registry.audit_records().unwrap();
    records.sort_by(|left, right| left.at.total_cmp(&right.at).then(left.id.cmp(&right.id)));
    records
        .into_iter()
        .map(|record| (record.actor, record.action, record.target))
        .collect()
}

fn state(registry: &dyn RegistryBackend) -> Option<LoginState> {
    registry
        .accounts()
        .unwrap()
        .into_iter()
        .find(|state| state.account == reference())
        .and_then(|state| state.login)
}

/// A fake sign-out: records what its home held and ends as told.
struct Fake {
    end: RenewEnd,
    saw: Mutex<Vec<bool>>,
}

impl Fake {
    fn new(end: RenewEnd) -> Self {
        Self {
            end,
            saw: Mutex::new(Vec::new()),
        }
    }

    fn runs(&self) -> usize {
        self.saw.lock().unwrap().len()
    }
}

impl LoginRenewer for Fake {
    fn renew(&self, _account: &Account, _home: &Path, _timeout: Duration) -> RenewEnd {
        RenewEnd::Success
    }

    fn sign_out(&self, _account: &Account, home: &Path, _timeout: Duration) -> RenewEnd {
        self.saw.lock().unwrap().push(home.join(FILE).exists());
        self.end.clone()
    }
}

backend_tests!(
    add_login_seals_audits_and_shows_nothing,
    add_login_refuses_an_account_that_holds_one_unless_replaced,
    add_login_refuses_what_is_not_there_before_it_audits,
    a_station_is_added_and_removed_by_one_that_holds_the_login,
    a_request_is_made_by_a_station_holding_the_login,
    removal_forgets_the_login_and_may_sign_out_first,
    a_queued_sign_out_is_made_by_a_holder,
);

fn add_login_seals_audits_and_shows_nothing(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let dir = host(root.path());
    let revision = add(&a, &dir, &["a"], false).unwrap();
    assert_eq!(revision, 1);
    assert!(dir.join(FILE).exists(), "nothing is taken from the host");
    assert!(a.open(&sealed_name(&reference())).is_ok());
    let login = state(registry.as_ref()).unwrap();
    assert_eq!(login.stations, ["a"]);
    assert_eq!(login.last_result.as_deref(), Some("added"));
    assert_eq!(
        actions(registry.as_ref()),
        [(
            "person".to_owned(),
            "pool.login.add".to_owned(),
            "fake/shared".to_owned()
        )]
    );
    // Ciphertext only: no row, record or message holds it.
    let record = registry.sealed_value(&sealed_name(&reference())).unwrap();
    assert!(!format!("{record:?}").contains(PLANTED));
    assert!(!serde_json::to_string(&record).unwrap().contains(PLANTED));
    assert!(!format!("{login:?}").contains(PLANTED));
    assert!(!format!("{:?}", registry.audit_records().unwrap()).contains(PLANTED));
    assert!(!standing::standing(&login, NOW).contains(PLANTED));
    let refused = add(&a, &dir, &["a"], false).unwrap_err();
    assert!(!format!("{refused} {refused:?}").contains(PLANTED));
}

fn add_login_refuses_an_account_that_holds_one_unless_replaced(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let dir = host(root.path());
    add(&a, &dir, &["a"], false).unwrap();
    let refused = add(&a, &dir, &["a"], false).unwrap_err();
    assert!(matches!(refused, AdminError::Exists), "{refused}");
    assert!(refused.to_string().contains("--replace"));
    assert_eq!(
        registry.sealed_revisions().unwrap()[&sealed_name(&reference())],
        1,
        "the login it holds is untouched"
    );
    assert_eq!(add(&a, &dir, &["a"], true).unwrap(), 2);
    let kinds: Vec<String> = actions(registry.as_ref())
        .into_iter()
        .map(|(_, action, _)| action)
        .collect();
    assert_eq!(kinds, ["pool.login.add", "pool.login.replace"]);
}

fn add_login_refuses_what_is_not_there_before_it_audits(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let empty = root.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let missing = add(&a, &empty, &["a"], false).unwrap_err();
    assert!(matches!(missing, AdminError::Login(LoginError::Missing(_))));
    let unnamed = add_login(
        &a,
        "person",
        &reference(),
        &Account::new("shared", AccountKind::HarnessLogin),
        Source {
            from: &host(root.path()),
            stations: &[Recipient::named("a")],
            replace: false,
        },
        NOW,
    )
    .unwrap_err();
    assert!(matches!(unnamed, AdminError::NoFiles));
    assert!(registry.audit_records().unwrap().is_empty());
    assert!(
        registry
            .sealed_value(&sealed_name(&reference()))
            .unwrap()
            .is_none()
    );
}

fn a_station_is_added_and_removed_by_one_that_holds_the_login(store: &dyn Store) {
    let registry = store.handle();
    let (a, b, c) = (
        station(&registry, "a"),
        station(&registry, "b"),
        station(&registry, "c"),
    );
    let root = tempfile::tempdir().unwrap();
    add(&a, &host(root.path()), &["a"], false).unwrap();
    let name = sealed_name(&reference());
    assert!(matches!(b.open(&name), Err(VaultError::NotNamed)));
    assert!(matches!(
        add_station(&b, "person", &reference(), &Recipient::named("c"), NOW),
        Err(AdminError::NotHolder)
    ));
    add_station(&a, "person", &reference(), &Recipient::named("b"), NOW).unwrap();
    assert!(b.open(&name).is_ok(), "the new Station opens it");
    assert_eq!(state(registry.as_ref()).unwrap().stations, ["a", "b"]);
    assert!(matches!(
        add_station(&a, "person", &reference(), &Recipient::named("nobody"), NOW),
        Err(AdminError::Login(LoginError::Vault(VaultError::NoKey(_))))
    ));

    remove_station(&a, "person", &reference(), "b", NOW).unwrap();
    assert!(
        matches!(b.open(&name), Err(VaultError::NotNamed)),
        "its sealed copy is dropped"
    );
    assert_eq!(state(registry.as_ref()).unwrap().stations, ["a"]);
    assert!(matches!(c.open(&name), Err(VaultError::NotNamed)));
    assert!(
        matches!(
            remove_station(&a, "person", &reference(), "a", NOW),
            Err(AdminError::Login(LoginError::Vault(VaultError::Sealing)))
        ),
        "the last Station cannot be removed"
    );
    let kinds: Vec<(String, String)> = actions(registry.as_ref())
        .into_iter()
        .map(|(_, action, target)| (action, target))
        .collect();
    assert!(kinds.contains(&(
        "pool.login.station.add".to_owned(),
        "fake/shared b".to_owned()
    )));
    assert!(kinds.contains(&(
        "pool.login.station.remove".to_owned(),
        "fake/shared b".to_owned()
    )));
}

fn a_request_is_made_by_a_station_holding_the_login(store: &dyn Store) {
    let registry = store.handle();
    let (a, b) = (station(&registry, "a"), station(&registry, "b"));
    let root = tempfile::tempdir().unwrap();
    add(&a, &host(root.path()), &["a"], false).unwrap();
    let logins = [(reference(), &declared(true))];
    let fake = Fake::new(RenewEnd::Success);
    let scratch = root.path().join("scratch");
    let asked = |action| {
        standing::request(
            registry.as_ref(),
            &reference(),
            action,
            "person via the app",
            NOW,
        )
        .unwrap()
    };
    assert!(asked(LoginRequestAction::AddStation {
        station: "b".to_owned()
    }));
    assert!(asked(LoginRequestAction::AddStation {
        station: "nobody".to_owned()
    }));
    assert_eq!(state(registry.as_ref()).unwrap().requests.len(), 2);

    // A Station the login is not sealed to cannot make it.
    let none = apply_requests(&b, &logins, commands(&fake, &scratch), NOW).unwrap();
    assert!(none.is_empty());
    assert_eq!(state(registry.as_ref()).unwrap().requests.len(), 2);

    let done = apply_requests(&a, &logins, commands(&fake, &scratch), NOW + 1.0).unwrap();
    assert_eq!(done.len(), 2, "{done:?}");
    assert!(b.open(&sealed_name(&reference())).is_ok());
    let login = state(registry.as_ref()).unwrap();
    assert!(login.requests.is_empty());
    assert_eq!(login.stations, ["a", "b"]);
    assert!(
        login
            .last_result
            .as_deref()
            .is_some_and(|result| result.contains("nobody")),
        "a Station with no key is said so on the login: {:?}",
        login.last_result
    );
    let made = actions(registry.as_ref());
    assert!(
        made.iter()
            .any(|(actor, action, _)| actor == "station:a" && action == "pool.login.station.add")
    );

    // A request for an account that holds none is refused.
    assert!(
        !standing::request(
            registry.as_ref(),
            &AccountRef::new("fake", "other"),
            LoginRequestAction::SignOutAndForget,
            "person",
            NOW
        )
        .unwrap()
    );
}

fn removal_forgets_the_login_and_may_sign_out_first(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let dir = host(root.path());
    let scratch = root.path().join("scratch");
    let name = sealed_name(&reference());

    // Signing out needs a command; with none nothing is changed and nothing is audited.
    add(&a, &dir, &["a"], false).unwrap();
    let before = registry.audit_records().unwrap().len();
    let none = sign_out_and_forget(
        &a,
        commands(&Fake::new(RenewEnd::Success), &scratch),
        "person",
        &reference(),
        &declared(false),
        NOW,
    )
    .unwrap_err();
    assert!(matches!(none, AdminError::NoSignOut));
    assert_eq!(registry.audit_records().unwrap().len(), before);
    assert!(a.open(&name).is_ok());

    // A sign-out that fails keeps the login.
    let failing = Fake::new(RenewEnd::Failed(Some(1)));
    let kept = sign_out_and_forget(
        &a,
        commands(&failing, &scratch),
        "person",
        &reference(),
        &declared(true),
        NOW,
    )
    .unwrap_err();
    assert!(matches!(kept, AdminError::SignOut(_)), "{kept}");
    assert!(a.open(&name).is_ok(), "the login is kept");
    assert!(state(registry.as_ref()).is_some());

    // One that succeeds runs on a home holding the login, removes the home, and forgets it.
    let working = Fake::new(RenewEnd::Success);
    sign_out_and_forget(
        &a,
        commands(&working, &scratch),
        "person",
        &reference(),
        &declared(true),
        NOW,
    )
    .unwrap();
    assert_eq!(*working.saw.lock().unwrap(), [true]);
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
    assert!(registry.sealed_value(&name).unwrap().is_none());
    assert!(state(registry.as_ref()).is_none());
    let kinds: Vec<String> = actions(registry.as_ref())
        .into_iter()
        .map(|(_, action, _)| action)
        .collect();
    assert!(kinds.contains(&"pool.login.sign_out".to_owned()));
    assert!(kinds.contains(&"pool.login.forget".to_owned()));

    // Forgetting without signing out never runs a command.
    add(&a, &dir, &["a"], false).unwrap();
    let idle = Fake::new(RenewEnd::Success);
    forget_login(registry.as_ref(), "person", "a", &reference(), NOW).unwrap();
    assert_eq!(idle.runs(), 0);
    assert!(registry.sealed_value(&name).unwrap().is_none());
    assert!(matches!(
        forget_login(registry.as_ref(), "person", "a", &reference(), NOW),
        Err(AdminError::NoLogin)
    ));
}

fn a_queued_sign_out_is_made_by_a_holder(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    add(&a, &host(root.path()), &["a"], false).unwrap();
    let scratch = root.path().join("scratch");
    standing::request(
        registry.as_ref(),
        &reference(),
        LoginRequestAction::SignOutAndForget,
        "person",
        NOW,
    )
    .unwrap();
    // The account is not one this Station declares: the request waits.
    let fake = Fake::new(RenewEnd::Failed(Some(2)));
    assert!(
        apply_requests(&a, &[], commands(&fake, &scratch), NOW)
            .unwrap()
            .is_empty()
    );
    assert_eq!(state(registry.as_ref()).unwrap().requests.len(), 1);

    // A sign-out that fails clears the request and keeps the login, with the reason on it.
    let logins = [(reference(), &declared(true))];
    apply_requests(&a, &logins, commands(&fake, &scratch), NOW).unwrap();
    let login = state(registry.as_ref()).unwrap();
    assert!(login.requests.is_empty());
    assert!(
        login
            .last_result
            .as_deref()
            .is_some_and(|result| result.contains("failed")),
        "{:?}",
        login.last_result
    );
    assert!(a.open(&sealed_name(&reference())).is_ok());

    standing::request(
        registry.as_ref(),
        &reference(),
        LoginRequestAction::SignOutAndForget,
        "person",
        NOW + 1.0,
    )
    .unwrap();
    let working = Fake::new(RenewEnd::Success);
    apply_requests(&a, &logins, commands(&working, &scratch), NOW + 1.0).unwrap();
    assert_eq!(working.runs(), 1);
    assert!(state(registry.as_ref()).is_none());
    assert!(
        registry
            .sealed_value(&sealed_name(&reference()))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_refused_audit_refuses_the_act() {
    let root = tempfile::tempdir().unwrap();
    let registry = Arc::new(crate::directorates::registry::store::FileRegistry::open(
        root.path().join("registry"),
    ));
    let key = Arc::new(SealingKey::generate());
    registry
        .publish_sealing_key(&StationSealingKey::new("a", key.public_key().to_text()))
        .unwrap();
    let vault = Vault::new(registry.clone(), "a", key);
    let dir = host(root.path());
    add(&vault, &dir, &["a"], false).unwrap();
    // The audit log cannot be written to: a file stands where its directory belongs.
    let audit = root.path().join("registry").join("audit");
    std::fs::remove_dir_all(&audit).unwrap();
    std::fs::write(&audit, b"not a directory").unwrap();
    let name = sealed_name(&reference());
    let before = registry.sealed_value(&name).unwrap().unwrap().revision;

    let refused = add(&vault, &dir, &["a"], true).unwrap_err();
    assert!(matches!(refused, AdminError::Audit(_)), "{refused}");
    assert!(matches!(
        add_station(&vault, "person", &reference(), &Recipient::named("a"), NOW),
        Err(AdminError::Audit(_))
    ));
    assert!(matches!(
        remove_station(&vault, "person", &reference(), "a", NOW),
        Err(AdminError::Audit(_))
    ));
    assert!(matches!(
        forget_login(registry.as_ref(), "person", "a", &reference(), NOW),
        Err(AdminError::Audit(_))
    ));
    let fake = Fake::new(RenewEnd::Success);
    assert!(matches!(
        sign_out_and_forget(
            &vault,
            commands(&fake, &root.path().join("scratch")),
            "person",
            &reference(),
            &declared(true),
            NOW
        ),
        Err(AdminError::Audit(_))
    ));
    assert_eq!(fake.runs(), 0, "no command ran");
    let after = registry.sealed_value(&name).unwrap().unwrap();
    assert_eq!(after.revision, before, "nothing was changed");
    assert!(state(registry.as_ref()).is_some());
}
