//! Logins kept sealed in the Registry, on every Registry backend, with a fake provider that rotates
//! its refresh token and refuses an old one, as the safest reading of each harness assumes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::directorates::q::sealing::{SealingKey, Vault};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::sealed::StationSealingKey;
use crate::directorates::registry::suite::{Store, TestRegistry, backend_tests};

const NOW: f64 = 1_790_000_000.0;
const FILE: &str = ".fake/auth.json";
/// Never to be seen anywhere but in a login file and in what a Station opens.
const PLANTED: &str = "planted-refresh-0451";

fn pool_account(command: bool) -> Account {
    let account = Account::new("shared", AccountKind::HarnessLogin).files([FILE]);
    if command {
        account.renew_command(["fake-harness"])
    } else {
        account
    }
}

fn reference() -> AccountRef {
    AccountRef::new("fake", "shared")
}

fn limits() -> AccountLimits {
    AccountLimits {
        renew_within: crate::bounds::Seconds::new(3_600),
        renew_after: crate::bounds::Seconds::new(86_400),
        renew_lease: crate::bounds::Seconds::new(300),
        renew_backoff: crate::bounds::Seconds::new(60),
        renew_backoff_max: crate::bounds::Seconds::new(200),
        renew_attempts: crate::bounds::Limit::at(3),
        ..AccountLimits::default()
    }
}

/// The provider: it knows one good refresh token, rotates it at each refresh, and refuses any
/// other, the way a harness's login is signed out.
#[derive(Clone)]
struct Provider(Arc<Mutex<ProviderState>>);

struct ProviderState {
    refresh: String,
    refreshes: usize,
    refused: usize,
    lifetime: f64,
    now: f64,
}

fn login_text(access: &str, refresh: &str, expires_at: f64) -> String {
    serde_json::json!({"access_token": access, "refresh_token": refresh, "expires_at": expires_at})
        .to_string()
}

impl Provider {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(ProviderState {
            refresh: PLANTED.to_owned(),
            refreshes: 0,
            refused: 0,
            lifetime: 86_400.0,
            now: NOW,
        })))
    }

    fn refreshes(&self) -> usize {
        self.0.lock().unwrap().refreshes
    }

    fn refused(&self) -> usize {
        self.0.lock().unwrap().refused
    }

    fn valid(&self) -> String {
        self.0.lock().unwrap().refresh.clone()
    }

    /// Someone else rotated the token: every copy here is signed out.
    fn rotate_elsewhere(&self) {
        self.0.lock().unwrap().refresh = "rotated-elsewhere".to_owned();
    }

    /// What a harness does in `home`: refreshes through the provider and writes the new login, or
    /// deletes its file when refused.
    fn refresh_in(&self, home: &Path) -> bool {
        let path = home.join(FILE);
        let Ok(bytes) = std::fs::read(&path) else {
            return false;
        };
        let held: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut state = self.0.lock().unwrap();
        if held["refresh_token"] != serde_json::Value::String(state.refresh.clone()) {
            state.refused += 1;
            drop(state);
            std::fs::remove_file(&path).unwrap();
            return false;
        }
        state.refreshes += 1;
        state.refresh = format!("refresh-{}", state.refreshes);
        state.now += 1.0;
        let text = login_text(
            &format!("access-{}", state.refreshes),
            &state.refresh,
            state.now + state.lifetime,
        );
        drop(state);
        std::fs::write(&path, text).unwrap();
        true
    }
}

/// The fake harness's renewal.
struct Rotating {
    provider: Provider,
    runs: AtomicUsize,
    exit: RenewEnd,
}

impl Rotating {
    fn new(provider: &Provider) -> Self {
        Self {
            provider: provider.clone(),
            runs: AtomicUsize::new(0),
            exit: RenewEnd::Success,
        }
    }
}

impl LoginRenewer for Rotating {
    fn renew(&self, _account: &Account, home: &Path, _timeout: Duration) -> RenewEnd {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.provider.refresh_in(home);
        // Exit status proves nothing: a refused Grok exits 0.
        self.exit.clone()
    }
}

/// A Station with its key published.
fn station(registry: &Arc<dyn TestRegistry>, name: &str) -> Vault {
    let key = Arc::new(SealingKey::generate());
    registry
        .publish_sealing_key(&StationSealingKey::new(name, key.public_key().to_text()))
        .unwrap();
    let backend: Arc<dyn RegistryBackend> = registry.clone();
    Vault::new(backend, name, key)
}

/// A host that has logged in: a directory holding the login.
fn host_login(root: &Path, refresh: &str, expires_at: f64) -> std::path::PathBuf {
    let dir = root.join("logged-in");
    std::fs::create_dir_all(dir.join(".fake")).unwrap();
    std::fs::write(dir.join(FILE), login_text("access-0", refresh, expires_at)).unwrap();
    dir
}

fn add(vault: &Vault, root: &Path, expires_at: f64, stations: &[&str]) {
    let dir = host_login(root, PLANTED, expires_at);
    let stations: Vec<Recipient> = stations
        .iter()
        .map(|name| Recipient::named(*name))
        .collect();
    add_login(
        vault,
        &reference(),
        &dir,
        &[FILE.to_owned()],
        &stations,
        NOW,
    )
    .unwrap();
    assert!(
        dir.join(FILE).exists(),
        "nothing is taken from where it was"
    );
}

fn login_of(registry: &dyn RegistryBackend) -> LoginState {
    registry
        .accounts()
        .unwrap()
        .into_iter()
        .find(|state| state.account == reference())
        .and_then(|state| state.login)
        .expect("the account has a login")
}

fn home_login(home: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(home.join(FILE)).unwrap()).unwrap()
}

fn refresh_in_home(home: &Path) -> String {
    home_login(home)["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn revision(registry: &dyn RegistryBackend) -> Option<u64> {
    registry
        .sealed_revisions()
        .unwrap()
        .get(&sealed_name(&reference()))
        .copied()
}

fn files() -> Vec<String> {
    vec![FILE.to_owned()]
}

backend_tests!(
    a_named_station_opens_and_a_session_home_holds_the_login,
    a_harness_renewal_goes_back_and_reaches_the_other_station,
    an_older_renewed_copy_never_replaces_a_newer_one,
    a_station_that_died_with_a_renewed_copy_writes_it_when_it_returns,
    a_station_that_died_with_a_copy_made_stale_loses_it,
    a_harness_that_was_refused_does_not_replace_the_login_and_its_home_is_put_back,
    a_blanked_login_does_not_replace_the_one_in_the_registry,
    a_file_past_the_limit_or_a_link_is_no_login,
    the_round_renews_one_near_its_end_with_sessions_running_and_neither_is_signed_out,
    one_lease_holder_however_many_stations_ask,
    a_login_of_unknown_end_is_renewed_by_its_age,
    a_named_station_renews_when_headquarters_is_not_named,
    the_round_backs_off_and_then_marks_one_that_will_not_renew,
    a_renewal_is_judged_by_the_files_not_the_exit_status,
    a_lost_headquarters_round_carries_on_elsewhere,
    a_lost_lease_holder_gives_way_when_its_lease_lapses,
    one_with_no_way_to_renew_is_left_with_a_warning,
    a_planted_login_appears_in_no_row_notice_error_or_debug,
);

fn a_named_station_opens_and_a_session_home_holds_the_login(store: &dyn Store) {
    let registry = store.handle();
    let (a, c) = (station(&registry, "a"), station(&registry, "c"));
    let root = tempfile::tempdir().unwrap();
    add(&a, root.path(), NOW + 86_400.0, &["a"]);
    let state = login_of(registry.as_ref());
    assert_eq!(state.stations, ["a"]);
    assert_eq!(state.expires_at, Some(NOW + 86_400.0));
    assert_eq!(state.renewed_at, Some(NOW));
    let home = root.path().join("home");
    let login = SessionLogin::materialize(&a, &reference(), &files(), &home).unwrap();
    assert_eq!(login.revision, 1);
    assert_eq!(refresh_in_home(&home), PLANTED);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&home.join(FILE)), 0o600);
        assert_eq!(mode(&home.join(".fake")), 0o700);
    }
    let elsewhere = root.path().join("elsewhere");
    let refused = SessionLogin::materialize(&c, &reference(), &files(), &elsewhere).unwrap_err();
    assert!(
        refused.to_string().contains("not sealed to this Station"),
        "{refused}"
    );
    assert!(!elsewhere.join(FILE).exists());
    login.remove().unwrap();
    assert!(!home.exists());
    assert!(
        SessionLogin::recall(&home).is_none(),
        "nothing is left beside it"
    );
}

fn a_harness_renewal_goes_back_and_reaches_the_other_station(store: &dyn Store) {
    let registry = store.handle();
    let (a, b) = (station(&registry, "a"), station(&registry, "b"));
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    add(&a, root.path(), NOW + 100.0, &["a", "b"]);
    let (home_a, home_b) = (root.path().join("a"), root.path().join("b"));
    let mut session_a = SessionLogin::materialize(&a, &reference(), &files(), &home_a).unwrap();
    let mut session_b = SessionLogin::materialize(&b, &reference(), &files(), &home_b).unwrap();

    assert!(provider.refresh_in(&home_a));
    assert_eq!(
        session_a
            .sync(&a, revision(registry.as_ref()), NOW + 1.0)
            .unwrap(),
        Synced::Renewed(2)
    );
    let state = login_of(registry.as_ref());
    assert_eq!(state.revision, 2);
    assert_eq!(state.renewed_at, Some(NOW + 1.0));
    assert_eq!(state.expires_at, home_login(&home_a)["expires_at"].as_f64());

    assert_eq!(
        session_b
            .sync(&b, revision(registry.as_ref()), NOW + 2.0)
            .unwrap(),
        Synced::Rewritten(2),
        "the next poll rewrites the other Station's Session"
    );
    assert_eq!(refresh_in_home(&home_b), provider.valid());
    assert_eq!(
        session_b
            .sync(&b, revision(registry.as_ref()), NOW + 3.0)
            .unwrap(),
        Synced::Unchanged,
        "the rewrite is not taken for a renewal"
    );
    // The other Session's next refresh works, with the token the first one's rotation left.
    assert!(provider.refresh_in(&home_b));
    assert_eq!(provider.refused(), 0);
}

fn an_older_renewed_copy_never_replaces_a_newer_one(store: &dyn Store) {
    let registry = store.handle();
    let (a, b) = (station(&registry, "a"), station(&registry, "b"));
    let root = tempfile::tempdir().unwrap();
    add(&a, root.path(), NOW + 100.0, &["a", "b"]);
    let (home_a, home_b) = (root.path().join("a"), root.path().join("b"));
    let mut session_a = SessionLogin::materialize(&a, &reference(), &files(), &home_a).unwrap();
    let mut session_b = SessionLogin::materialize(&b, &reference(), &files(), &home_b).unwrap();
    // Both renew from revision 1; A ends far later and reaches the Registry first.
    std::fs::write(
        home_a.join(FILE),
        login_text("new-a", "refresh-a", NOW + 9_000.0),
    )
    .unwrap();
    std::fs::write(
        home_b.join(FILE),
        login_text("new-b", "refresh-b", NOW + 5_000.0),
    )
    .unwrap();
    assert_eq!(
        session_a.sync(&a, Some(1), NOW).unwrap(),
        Synced::Renewed(2)
    );
    let synced = session_b.sync(&b, Some(2), NOW).unwrap();
    assert!(matches!(synced, Synced::Restored(2, _)), "{synced:?}");
    assert_eq!(refresh_in_home(&home_b), "refresh-a");
    // With no way to tell, the one that reached the Registry first stands.
    std::fs::write(
        home_a.join(FILE),
        "{\"access_token\":\"x\",\"refresh_token\":\"r2\"}",
    )
    .unwrap();
    std::fs::write(
        home_b.join(FILE),
        "{\"access_token\":\"y\",\"refresh_token\":\"r3\"}",
    )
    .unwrap();
    assert_eq!(
        session_a.sync(&a, Some(2), NOW).unwrap(),
        Synced::Renewed(3)
    );
    let synced = session_b.sync(&b, Some(3), NOW).unwrap();
    assert!(matches!(synced, Synced::Restored(3, _)), "{synced:?}");
    assert_eq!(refresh_in_home(&home_b), "r2");
}

fn a_station_that_died_with_a_renewed_copy_writes_it_when_it_returns(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    add(&a, root.path(), NOW + 100.0, &["a"]);
    let home = root.path().join("home");
    drop(SessionLogin::materialize(&a, &reference(), &files(), &home).unwrap());
    // The harness renews while its Station is down: only the home holds the live token.
    assert!(provider.refresh_in(&home));
    assert_eq!(revision(registry.as_ref()), Some(1));

    let mut recalled = SessionLogin::recall(&home).expect("kept beside the home");
    assert_eq!(
        recalled.sync(&a, Some(1), NOW + 5.0).unwrap(),
        Synced::Renewed(2)
    );
    assert_eq!(
        LoginFiles::from_value(&a.open(&sealed_name(&reference())).unwrap().value)
            .unwrap()
            .get(FILE)
            .map(<[u8]>::to_vec),
        Some(std::fs::read(home.join(FILE)).unwrap())
    );
}

fn a_station_that_died_with_a_copy_made_stale_loses_it(store: &dyn Store) {
    let registry = store.handle();
    let (a, b) = (station(&registry, "a"), station(&registry, "b"));
    let root = tempfile::tempdir().unwrap();
    add(&a, root.path(), NOW + 100.0, &["a", "b"]);
    let home = root.path().join("home");
    drop(SessionLogin::materialize(&a, &reference(), &files(), &home).unwrap());
    std::fs::write(
        home.join(FILE),
        login_text("dead-a", "refresh-a", NOW + 5_000.0),
    )
    .unwrap();
    // Meanwhile B renews to a later end.
    let home_b = root.path().join("b");
    let mut session_b = SessionLogin::materialize(&b, &reference(), &files(), &home_b).unwrap();
    std::fs::write(
        home_b.join(FILE),
        login_text("new-b", "refresh-b", NOW + 9_000.0),
    )
    .unwrap();
    assert_eq!(
        session_b.sync(&b, Some(1), NOW).unwrap(),
        Synced::Renewed(2)
    );

    let mut recalled = SessionLogin::recall(&home).unwrap();
    let synced = recalled.sync(&a, Some(2), NOW + 1.0).unwrap();
    assert!(matches!(synced, Synced::Restored(2, _)), "{synced:?}");
    assert_eq!(refresh_in_home(&home), "refresh-b");
    assert_eq!(revision(registry.as_ref()), Some(2));
}

fn a_harness_that_was_refused_does_not_replace_the_login_and_its_home_is_put_back(
    store: &dyn Store,
) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    add(&a, root.path(), NOW + 100.0, &["a"]);
    let home = root.path().join("home");
    let mut session = SessionLogin::materialize(&a, &reference(), &files(), &home).unwrap();
    provider.rotate_elsewhere();
    assert!(!provider.refresh_in(&home), "refused");
    assert!(
        !home.join(FILE).exists(),
        "a refused harness removes its file"
    );
    let synced = session.sync(&a, Some(1), NOW + 1.0).unwrap();
    assert!(matches!(synced, Synced::Restored(1, _)), "{synced:?}");
    assert_eq!(revision(registry.as_ref()), Some(1));
    assert_eq!(refresh_in_home(&home), PLANTED);
}

fn a_blanked_login_does_not_replace_the_one_in_the_registry(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    // No readable end in the file, so only its emptied tokens can tell.
    let dir = root.path().join("logged-in");
    std::fs::create_dir_all(dir.join(".fake")).unwrap();
    std::fs::write(
        dir.join(FILE),
        r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","scopes":["x"]}}"#,
    )
    .unwrap();
    add_login(
        &a,
        &reference(),
        &dir,
        &files(),
        &[Recipient::named("a")],
        NOW,
    )
    .unwrap();
    let home = root.path().join("home");
    let mut session = SessionLogin::materialize(&a, &reference(), &files(), &home).unwrap();
    std::fs::write(
        home.join(FILE),
        r#"{"claudeAiOauth":{"accessToken":"","refreshToken":"","scopes":["x"],"new":1}}"#,
    )
    .unwrap();
    let synced = session.sync(&a, Some(1), NOW + 1.0).unwrap();
    assert!(
        matches!(synced, Synced::Restored(1, "its harness blanked its login")),
        "{synced:?}"
    );
    assert_eq!(revision(registry.as_ref()), Some(1));
    // A profile field that goes is no blanking.
    std::fs::write(
        home.join(FILE),
        r#"{"claudeAiOauth":{"accessToken":"a2","refreshToken":"r2"}}"#,
    )
    .unwrap();
    assert_eq!(
        session.sync(&a, Some(1), NOW + 2.0).unwrap(),
        Synced::Renewed(2)
    );
}

fn a_file_past_the_limit_or_a_link_is_no_login(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    add(&a, root.path(), NOW + 100.0, &["a"]);
    let home = root.path().join("home");
    let mut session = SessionLogin::materialize(&a, &reference(), &files(), &home).unwrap();
    let largest = registry.limits().accounts.sealed_bytes.get().unwrap();
    std::fs::write(home.join(FILE), vec![b'x'; largest as usize + 1]).unwrap();
    let synced = session.sync(&a, Some(1), NOW + 1.0).unwrap();
    assert!(matches!(synced, Synced::Restored(1, _)), "{synced:?}");
    assert_eq!(refresh_in_home(&home), PLANTED);

    // What a Session points its login file at is not sealed for other Stations.
    let elsewhere = root.path().join("elsewhere");
    std::fs::write(
        &elsewhere,
        r#"{"refresh_token":"another-secret","expires_at":1890000000}"#,
    )
    .unwrap();
    std::fs::remove_file(home.join(FILE)).unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&elsewhere, home.join(FILE)).is_ok();
    // Windows lets only some accounts make a link.
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&elsewhere, home.join(FILE)).is_ok();
    if linked {
        let synced = session.sync(&a, Some(1), NOW + 2.0).unwrap();
        assert!(matches!(synced, Synced::Restored(1, _)), "{synced:?}");
        assert_eq!(revision(registry.as_ref()), Some(1));
        // The home's copy replaces the link itself, and what it pointed at stays as it was.
        assert_eq!(refresh_in_home(&home), PLANTED);
        assert!(
            !std::fs::symlink_metadata(home.join(FILE))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_to_string(&elsewhere)
                .unwrap()
                .contains("another-secret")
        );
    }
}

fn the_round_renews_one_near_its_end_with_sessions_running_and_neither_is_signed_out(
    store: &dyn Store,
) {
    let registry = store.handle();
    let (a, b) = (station(&registry, "a"), station(&registry, "b"));
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    let limits = limits();
    let account = pool_account(true);
    let logins = [(reference(), &account)];
    let live = ["a".to_owned(), "b".to_owned()];
    add(&a, root.path(), NOW + 7_200.0, &["a", "b"]);
    let (home_a, home_b) = (root.path().join("a"), root.path().join("b"));
    let mut session_a = SessionLogin::materialize(&a, &reference(), &files(), &home_a).unwrap();
    let mut session_b = SessionLogin::materialize(&b, &reference(), &files(), &home_b).unwrap();

    // Far from its end: nothing to do.
    let notices = assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, NOW).unwrap();
    assert!(notices.is_empty(), "{notices:?}");

    // Near it: Headquarters leases it to itself, and only the holder renews.
    let near = NOW + 3_700.0;
    let notices = assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, near).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].kind, NoticeKind::Leased("a".to_owned()));
    let renewer = Rotating::new(&provider);
    let scratch = root.path().join("scratch");
    assert!(
        run_leases(&b, &logins, &renewer, &scratch, &limits, near)
            .unwrap()
            .is_empty()
    );
    assert_eq!(renewer.runs.load(Ordering::SeqCst), 0, "B holds no lease");
    let notices = run_leases(&a, &logins, &renewer, &scratch, &limits, near).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Renewed, "{notices:?}");
    assert_eq!(renewer.runs.load(Ordering::SeqCst), 1);
    assert_eq!(provider.refreshes(), 1);
    assert_eq!(
        std::fs::read_dir(&scratch).unwrap().count(),
        0,
        "no throwaway home is left"
    );
    let state = login_of(registry.as_ref());
    assert_eq!(state.lease, None);
    assert_eq!(state.revision, 2);
    assert!(state.expires_at.unwrap() > near + limits.renew_within.0.as_secs_f64());

    // Both Sessions take it in their next poll, and neither is signed out.
    let held = revision(registry.as_ref());
    assert_eq!(
        session_a.sync(&a, held, near + 1.0).unwrap(),
        Synced::Rewritten(2)
    );
    assert_eq!(
        session_b.sync(&b, held, near + 1.0).unwrap(),
        Synced::Rewritten(2)
    );
    assert_eq!(refresh_in_home(&home_a), provider.valid());
    assert_eq!(refresh_in_home(&home_b), provider.valid());
    assert!(provider.refresh_in(&home_a));
    assert_eq!(
        session_a.sync(&a, held, near + 2.0).unwrap(),
        Synced::Renewed(3)
    );
    assert_eq!(
        session_b
            .sync(&b, revision(registry.as_ref()), near + 2.0)
            .unwrap(),
        Synced::Rewritten(3)
    );
    assert!(provider.refresh_in(&home_b));
    assert_eq!(provider.refused(), 0, "nobody was signed out");

    // Just renewed: the next round has nothing to do.
    let later = near + 3.0;
    assert!(
        assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, later)
            .unwrap()
            .is_empty()
    );
}

fn one_lease_holder_however_many_stations_ask(store: &dyn Store) {
    let registry = store.handle();
    let (a, b) = (station(&registry, "a"), station(&registry, "b"));
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    let limits = limits();
    let account = pool_account(true);
    add(&a, root.path(), NOW + 100.0, &["a", "b"]);
    let live = ["a".to_owned(), "b".to_owned()];
    let threads: Vec<_> = ["a", "b"]
        .into_iter()
        .cycle()
        .take(8)
        .map(|headquarters| {
            let registry = store.handle();
            let account = account.clone();
            let (live, limits) = (live.clone(), limits.clone());
            std::thread::spawn(move || {
                let logins = [(reference(), &account)];
                assign_renewals(
                    registry.as_ref(),
                    &logins,
                    headquarters,
                    &live,
                    &limits,
                    NOW,
                )
                .unwrap()
                .len()
            })
        })
        .collect();
    let granted: usize = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .sum();
    assert_eq!(granted, 1, "one lease, whoever asked");
    let logins = [(reference(), &account)];
    let renewer = Rotating::new(&provider);
    let scratch = root.path().join("scratch");
    run_leases(&a, &logins, &renewer, &scratch, &limits, NOW).unwrap();
    run_leases(&b, &logins, &renewer, &scratch, &limits, NOW).unwrap();
    assert_eq!(
        renewer.runs.load(Ordering::SeqCst),
        1,
        "one Station renewed"
    );
    assert_eq!(provider.refreshes(), 1);
}

fn a_login_of_unknown_end_is_renewed_by_its_age(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let limits = limits();
    let account = pool_account(true);
    let logins = [(reference(), &account)];
    let dir = root.path().join("logged-in");
    std::fs::create_dir_all(dir.join(".fake")).unwrap();
    std::fs::write(dir.join(FILE), r#"{"refresh_token":"r"}"#).unwrap();
    add_login(
        &a,
        &reference(),
        &dir,
        &files(),
        &[Recipient::named("a")],
        NOW,
    )
    .unwrap();
    assert_eq!(login_of(registry.as_ref()).expires_at, None);
    let live = ["a".to_owned()];
    let young = NOW + 86_000.0;
    assert!(
        assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, young)
            .unwrap()
            .is_empty()
    );
    let old = NOW + 86_401.0;
    assert_eq!(
        assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, old)
            .unwrap()
            .len(),
        1
    );
    // A command that leaves it as it was succeeds and is not asked again until the age is up.
    struct Idle;
    impl LoginRenewer for Idle {
        fn renew(&self, _: &Account, _: &Path, _: Duration) -> RenewEnd {
            RenewEnd::Success
        }
    }
    let scratch = root.path().join("scratch");
    let notices = run_leases(&a, &logins, &Idle, &scratch, &limits, old).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Unchanged);
    let state = login_of(registry.as_ref());
    assert_eq!(
        (state.checked_at, state.failures, state.lease),
        (Some(old), 0, None)
    );
    assert!(
        assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, old + 60.0)
            .unwrap()
            .is_empty()
    );
}

fn a_named_station_renews_when_headquarters_is_not_named(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let limits = limits();
    let account = pool_account(true);
    let logins = [(reference(), &account)];
    add(&a, root.path(), NOW + 100.0, &["a"]);
    // Headquarters is not named and its named Station is gone: nobody gets the lease.
    assert!(
        assign_renewals(
            registry.as_ref(),
            &logins,
            "hq",
            &["hq".to_owned()],
            &limits,
            NOW
        )
        .unwrap()
        .is_empty()
    );
    let notices = assign_renewals(
        registry.as_ref(),
        &logins,
        "hq",
        &["hq".to_owned(), "a".to_owned()],
        &limits,
        NOW,
    )
    .unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Leased("a".to_owned()));
}

fn the_round_backs_off_and_then_marks_one_that_will_not_renew(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    let limits = limits();
    let account = pool_account(true);
    let logins = [(reference(), &account)];
    let live = ["a".to_owned()];
    let scratch = root.path().join("scratch");
    add(&a, root.path(), NOW + 100.0, &["a"]);
    provider.rotate_elsewhere();
    let renewer = Rotating::new(&provider);
    let round = |now: f64| {
        assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, now).unwrap();
        run_leases(&a, &logins, &renewer, &scratch, &limits, now).unwrap()
    };

    let notices = round(NOW);
    assert_eq!(notices[0].kind, NoticeKind::Failed, "{notices:?}");
    let state = login_of(registry.as_ref());
    assert_eq!((state.failures, state.next_try), (1, Some(NOW + 60.0)));
    assert_eq!(state.lease, None);
    assert_eq!(state.needs_person, None);
    assert_eq!(
        revision(registry.as_ref()),
        Some(1),
        "the refused copy was not kept"
    );

    // During the backoff nothing runs.
    assert!(round(NOW + 59.0).is_empty());
    assert_eq!(renewer.runs.load(Ordering::SeqCst), 1);

    let notices = round(NOW + 60.0);
    assert_eq!(notices[0].kind, NoticeKind::Failed);
    let state = login_of(registry.as_ref());
    assert_eq!(
        (state.failures, state.next_try),
        (2, Some(NOW + 60.0 + 120.0))
    );

    // The third failure marks it; the wait is capped at its maximum.
    let at = NOW + 180.0;
    let notices = round(at);
    assert_eq!(notices[0].kind, NoticeKind::NeedsPerson, "{notices:?}");
    let state = login_of(registry.as_ref());
    assert_eq!((state.failures, state.next_try), (3, Some(at + 200.0)));
    assert!(
        state
            .needs_person
            .as_deref()
            .unwrap()
            .contains("3 renewals failed")
    );
    assert!(
        !state.usable_by("a"),
        "its Sessions go to the next account or route"
    );

    // Marked: the round leaves it alone, until a good copy reaches the Registry.
    assert!(round(at + 10_000.0).is_empty());
    assert_eq!(renewer.runs.load(Ordering::SeqCst), 3);
    let home = root.path().join("home");
    let mut session = SessionLogin::materialize(&a, &reference(), &files(), &home).unwrap();
    std::fs::write(home.join(FILE), login_text("x", "new", at + 90_000.0)).unwrap();
    assert_eq!(
        session.sync(&a, Some(1), at + 10_001.0).unwrap(),
        Synced::Renewed(2)
    );
    let state = login_of(registry.as_ref());
    assert_eq!(
        (state.failures, state.next_try, state.needs_person),
        (0, None, None)
    );
}

fn a_renewal_is_judged_by_the_files_not_the_exit_status(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    let limits = limits();
    let account = pool_account(true);
    let logins = [(reference(), &account)];
    let live = ["a".to_owned()];
    let scratch = root.path().join("scratch");
    add(&a, root.path(), NOW + 100.0, &["a"]);
    // A token was minted and the command then failed: the token must not be lost.
    let mut renewer = Rotating::new(&provider);
    renewer.exit = RenewEnd::Failed(Some(1));
    assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, NOW).unwrap();
    let notices = run_leases(&a, &logins, &renewer, &scratch, &limits, NOW).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Renewed, "{notices:?}");
    assert_eq!(revision(registry.as_ref()), Some(2));
    // Refused, removing its file, with an exit status of success: a failure.
    provider.rotate_elsewhere();
    let renewer = Rotating::new(&provider);
    let soon = login_of(registry.as_ref()).expires_at.unwrap() - 10.0;
    assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, soon).unwrap();
    let notices = run_leases(&a, &logins, &renewer, &scratch, &limits, soon).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Failed, "{notices:?}");
    assert!(
        notices[0].text.contains("removed its login file"),
        "{}",
        notices[0].text
    );
    // A command that ran and changed nothing on a login that has ended: a failure too.
    struct Idle;
    impl LoginRenewer for Idle {
        fn renew(&self, _: &Account, _: &Path, _: Duration) -> RenewEnd {
            RenewEnd::Success
        }
    }
    let ended = soon + 1_000.0;
    assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, ended).unwrap();
    let notices = run_leases(&a, &logins, &Idle, &scratch, &limits, ended).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Failed, "{notices:?}");
}

fn a_lost_headquarters_round_carries_on_elsewhere(store: &dyn Store) {
    let registry = store.handle();
    let (a, b) = (station(&registry, "a"), station(&registry, "b"));
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    let limits = limits();
    let account = pool_account(true);
    let logins = [(reference(), &account)];
    let live = ["a".to_owned(), "b".to_owned()];
    let scratch = root.path().join("scratch");
    add(&a, root.path(), NOW + 100.0, &["a", "b"]);
    // A failure on Headquarters A leaves a backoff in the Registry, then A is gone.
    let refusing = Rotating::new(&Provider({
        let other = Provider::new();
        other.rotate_elsewhere();
        other.0
    }));
    assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, NOW).unwrap();
    run_leases(&a, &logins, &refusing, &scratch, &limits, NOW).unwrap();
    assert_eq!(login_of(registry.as_ref()).failures, 1);

    // B is Headquarters of the next term: it reads the backoff, honours it and carries on.
    let handle = store.handle();
    assert!(handle.elect("b", Duration::from_secs(60)).unwrap());
    let survivor = ["b".to_owned()];
    assert!(
        assign_renewals(handle.as_ref(), &logins, "b", &survivor, &limits, NOW + 1.0)
            .unwrap()
            .is_empty()
    );
    let after = NOW + 61.0;
    let notices =
        assign_renewals(handle.as_ref(), &logins, "b", &survivor, &limits, after).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Leased("b".to_owned()));
    let renewer = Rotating::new(&provider);
    let notices = run_leases(&b, &logins, &renewer, &scratch, &limits, after).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Renewed, "{notices:?}");
    let state = login_of(handle.as_ref());
    assert_eq!((state.revision, state.failures), (2, 0));
}

fn a_lost_lease_holder_gives_way_when_its_lease_lapses(store: &dyn Store) {
    let registry = store.handle();
    let (a, b) = (station(&registry, "a"), station(&registry, "b"));
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    let limits = limits();
    let account = pool_account(true);
    let logins = [(reference(), &account)];
    let live = ["a".to_owned(), "b".to_owned()];
    let scratch = root.path().join("scratch");
    add(&a, root.path(), NOW + 100.0, &["a", "b"]);
    assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, NOW).unwrap();
    assert_eq!(login_of(registry.as_ref()).lease.unwrap().station, "a");
    // A dies holding it. While it is good, B gets nothing and renews nothing.
    let renewer = Rotating::new(&provider);
    assert!(
        assign_renewals(registry.as_ref(), &logins, "b", &live, &limits, NOW + 299.0)
            .unwrap()
            .is_empty()
    );
    assert!(
        run_leases(&b, &logins, &renewer, &scratch, &limits, NOW + 299.0)
            .unwrap()
            .is_empty()
    );
    assert_eq!(renewer.runs.load(Ordering::SeqCst), 0);
    // Past it, the next round goes to B, and A's lapsed lease lets nothing run on A.
    let after = NOW + 301.0;
    assert!(
        run_leases(&a, &logins, &renewer, &scratch, &limits, after)
            .unwrap()
            .is_empty()
    );
    let notices = assign_renewals(registry.as_ref(), &logins, "b", &live, &limits, after).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Leased("b".to_owned()));
    let notices = run_leases(&b, &logins, &renewer, &scratch, &limits, after).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::Renewed);
    assert_eq!(provider.refreshes(), 1);
}

fn one_with_no_way_to_renew_is_left_with_a_warning(store: &dyn Store) {
    let registry = store.handle();
    let a = station(&registry, "a");
    let root = tempfile::tempdir().unwrap();
    let limits = limits();
    let account = pool_account(false);
    let logins = [(reference(), &account)];
    let live = ["a".to_owned()];
    add(&a, root.path(), NOW + 100.0, &["a"]);
    let notices = assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, NOW).unwrap();
    assert_eq!(notices[0].kind, NoticeKind::MayLapse, "{notices:?}");
    assert_eq!(login_of(registry.as_ref()).lease, None);
    assert!(
        assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, NOW + 60.0)
            .unwrap()
            .is_empty(),
        "told once a while, not at every round"
    );
    let later = NOW + 86_401.0;
    let notices = assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, later).unwrap();
    assert_eq!(notices.len(), 1);
    assert!(
        login_of(registry.as_ref()).usable_by("a"),
        "its next real use may renew it"
    );
}

fn a_planted_login_appears_in_no_row_notice_error_or_debug(store: &dyn Store) {
    let registry = store.handle();
    let (a, b, c) = (
        station(&registry, "a"),
        station(&registry, "b"),
        station(&registry, "c"),
    );
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new();
    let limits = limits();
    let account = pool_account(true);
    let logins = [(reference(), &account)];
    let live = ["a".to_owned(), "b".to_owned()];
    let scratch = root.path().join("scratch");
    add(&a, root.path(), NOW + 100.0, &["a", "b"]);
    let home = root.path().join("home");
    let mut session = SessionLogin::materialize(&a, &reference(), &files(), &home).unwrap();
    assert_eq!(refresh_in_home(&home), PLANTED, "it is there to be found");

    let mut seen = Vec::new();
    // What people read: every notice, error and Debug on the way through a round.
    let notices = assign_renewals(registry.as_ref(), &logins, "a", &live, &limits, NOW).unwrap();
    seen.push(format!("{notices:?}"));
    let renewer = Rotating::new(&provider);
    let notices = run_leases(&a, &logins, &renewer, &scratch, &limits, NOW).unwrap();
    seen.push(format!("{notices:?}"));
    seen.push(format!(
        "{:?}",
        session.sync(&a, revision(registry.as_ref()), NOW)
    ));
    seen.push(format!("{session:?} {a:?} {account:?}"));
    seen.push(format!(
        "{:?}",
        LoginFiles::read(&home, &files(), crate::bounds::Limit::UNBOUNDED).unwrap()
    ));
    seen.push(c.open(&sealed_name(&reference())).unwrap_err().to_string());
    let refused = SessionLogin::materialize(&c, &reference(), &files(), &root.path().join("c"));
    seen.push(format!("{refused:?}"));
    provider.rotate_elsewhere();
    let refusing = Rotating::new(&provider);
    let soon = login_of(registry.as_ref()).expires_at.unwrap();
    assign_renewals(registry.as_ref(), &logins, "b", &live, &limits, soon).unwrap();
    let notices = run_leases(&b, &logins, &refusing, &scratch, &limits, soon).unwrap();
    seen.push(format!("{notices:?}"));

    // What a dump or `pool.read` sees: every record of the family, as stored.
    let states = registry.accounts().unwrap();
    seen.push(serde_json::to_string(&states).unwrap());
    seen.push(format!("{states:?}"));
    let record = registry
        .sealed_value(&sealed_name(&reference()))
        .unwrap()
        .unwrap();
    seen.push(serde_json::to_string(&record).unwrap());
    seen.push(format!("{record:?}"));
    seen.push(format!("{:?}", registry.sealing_key("a").unwrap()));
    seen.push(format!("{:?}", registry.sealed_revisions().unwrap()));

    let all = seen.join("\n");
    for secret in [PLANTED, "access-0", "access-1", "refresh-1"] {
        assert!(!all.contains(secret), "{secret} leaked: {all}");
    }
    // And never its base64, as it travels inside the value.
    let encoded = base64::engine::general_purpose::STANDARD.encode(login_text(
        "access-0",
        PLANTED,
        NOW + 100.0,
    ));
    assert!(!all.contains(&encoded), "{all}");
}

#[test]
fn a_file_registry_holds_nothing_of_a_planted_login_in_any_file() {
    let dir = tempfile::tempdir().unwrap();
    let registry: Arc<dyn RegistryBackend> = Arc::new(
        crate::directorates::registry::store::FileRegistry::open(dir.path()),
    );
    let key = Arc::new(SealingKey::generate());
    registry
        .publish_sealing_key(&StationSealingKey::new("a", key.public_key().to_text()))
        .unwrap();
    let a = Vault::new(registry.clone(), "a", key);
    let root = tempfile::tempdir().unwrap();
    add(&a, root.path(), NOW + 100.0, &["a"]);
    let mut stored = String::new();
    let mut pending = vec![dir.path().to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(next).unwrap().flatten() {
            if entry.path().is_dir() {
                pending.push(entry.path());
            } else {
                stored.push_str(&String::from_utf8_lossy(
                    &std::fs::read(entry.path()).unwrap(),
                ));
            }
        }
    }
    assert!(stored.contains("login/fake/shared"), "the record is there");
    assert!(!stored.contains(PLANTED));
    assert!(!stored.contains("access-0"));
}

#[test]
fn an_end_is_read_from_each_harness_s_shape() {
    let read = |file: &str, text: &str| {
        let mut files = BTreeMap::new();
        files.insert(file.to_owned(), text.as_bytes().to_vec());
        LoginFiles(files).expires_at()
    };
    // Grok: RFC 3339 under a key per issuer.
    assert_eq!(
        read(
            ".grok/auth.json",
            r#"{"https://auth.example::client":{"key":"k","expires_at":"2026-11-05T10:51:11.164570300Z"}}"#
        ),
        Some(1_793_875_871.1645703)
    );
    // Claude Code: milliseconds; the refresh token's own end is not the access token's.
    assert_eq!(
        read(
            ".claude/.credentials.json",
            r#"{"claudeAiOauth":{"expiresAt":1793875871000,"refreshTokenExpiresAt":1893875871000}}"#
        ),
        Some(1_793_875_871.0)
    );
    // Antigravity: Go's oauth2 token.
    assert_eq!(
        read(
            ".gemini/antigravity-cli/antigravity-oauth-token",
            r#"{"token":{"access_token":"not-a-jwt","expiry":"2026-11-05T10:51:11Z"}}"#
        ),
        Some(1_793_875_871.0)
    );
    // Codex: only the access token's own `exp`.
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"exp":1793875871}"#);
    assert_eq!(
        read(
            ".codex/auth.json",
            &format!(
                r#"{{"tokens":{{"access_token":"x.{claims}.y"}},"last_refresh":"2026-11-01T00:00:00Z"}}"#
            )
        ),
        Some(1_793_875_871.0)
    );
    assert_eq!(read("x", "plain text"), None);
    assert_eq!(read("x", r#"{"expires_at":"soon"}"#), None);
}

#[test]
fn a_real_command_runs_in_its_home_with_nothing_of_the_station_s_environment_but_what_it_needs() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let shell: Vec<String> = if cfg!(windows) {
        [
            "cmd",
            "/c",
            "echo %HOME%> seen.txt& echo %AI6_LOGIN_TEST_LEAK%>> seen.txt& echo %EXTRA%>> seen.txt",
        ]
        .map(str::to_owned)
        .to_vec()
    } else {
        [
            "sh",
            "-c",
            "printf '%s\\n' \"$HOME\" \"${AI6_LOGIN_TEST_LEAK:-unset}\" \"$EXTRA\" > seen.txt",
        ]
        .map(str::to_owned)
        .to_vec()
    };
    let account = Account::new("a", AccountKind::HarnessLogin)
        .files(["x"])
        .renew_command(shell)
        .renew_env("EXTRA", "given");
    assert_eq!(
        CommandRenewer.renew(&account, home, Duration::from_secs(60)),
        RenewEnd::Success
    );
    let seen = std::fs::read_to_string(home.join("seen.txt")).unwrap();
    assert!(seen.contains("given"), "{seen}");
    assert!(!seen.contains("leak-0451"), "{seen}");

    let missing = Account::new("a", AccountKind::HarnessLogin)
        .files(["x"])
        .renew_command(["no-such-program-0451"]);
    assert!(matches!(
        CommandRenewer.renew(&missing, home, Duration::from_secs(5)),
        RenewEnd::NotRun(_)
    ));
    let sleeper: Vec<String> = if cfg!(windows) {
        ["cmd", "/c", "ping -n 30 127.0.0.1 >NUL"]
            .map(str::to_owned)
            .to_vec()
    } else {
        ["sh", "-c", "sleep 30"].map(str::to_owned).to_vec()
    };
    let slow = Account::new("a", AccountKind::HarnessLogin)
        .files(["x"])
        .renew_command(sleeper);
    assert!(matches!(
        CommandRenewer.renew(&slow, home, Duration::from_millis(100)),
        RenewEnd::NotRun(_)
    ));
}
