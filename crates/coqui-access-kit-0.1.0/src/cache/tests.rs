use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;

use coqui_access::error;

use super::*;

/// A directory and authorizer that counts its calls and answers as it is told.
#[derive(Default)]
struct Counting {
    calls: AtomicUsize,
    fail: Mutex<bool>,
}

impl Counting {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn answer(&self) -> Result<(), ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if *self.fail.lock().unwrap() {
            return Err(error::internal("the directory is down"));
        }
        Ok(())
    }
}

impl Directory for Counting {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        self.answer()?;
        Ok((lookup.value != "nobody").then(|| Person {
            id: lookup.value,
            ..Person::default()
        }))
    }
}

impl Authorizer for Counting {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        self.answer()?;
        Ok(AuthorizationDecision {
            allowed: request.action == "read",
            ..AuthorizationDecision::default()
        })
    }
}

impl Accounts for Counting {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        Ok(Vec::new())
    }

    fn put(&self, _account: AccountPut) -> Result<PutOutcome, ProviderError> {
        Ok(PutOutcome::default())
    }

    fn remove(&self, _name: String) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn by_name(value: &str) -> Lookup {
    Lookup {
        by: "name".to_owned(),
        value: value.to_owned(),
    }
}

fn request(groups: &[&str], action: &str) -> AuthorizationRequest {
    AuthorizationRequest {
        person: Person {
            id: "1".to_owned(),
            name: "alice".to_owned(),
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
            ..Person::default()
        },
        action: action.to_owned(),
        resource: "site:north".to_owned(),
        ancestors: Vec::new(),
    }
}

const SHORT: Duration = Duration::from_millis(150);

#[test]
fn the_defaults_are_five_seconds_one_second_and_ten_thousand() {
    let options = CacheOptions::default();
    assert_eq!(
        (options.ttl, options.negative_ttl, options.max_entries),
        (Duration::from_secs(5), Duration::from_secs(1), 10_000)
    );
    assert!(!options.exceeds_default());
    assert!(options.ttl(Duration::from_secs(6)).exceeds_default());
    assert_eq!(CacheSetting::default().options(), options);
    assert_eq!(CacheSetting::Enabled(false).options(), CacheOptions::off());
}

#[test]
fn a_lookup_is_asked_once_within_its_ttl_and_again_after() {
    let directory = Arc::new(Counting::default());
    let cached = Cached::new(directory.clone(), CacheOptions::default().ttl(SHORT));
    for _ in 0..2 {
        assert_eq!(
            cached.lookup(by_name("alice")).unwrap().unwrap().id,
            "alice"
        );
    }
    assert_eq!(directory.calls(), 1);
    sleep(SHORT * 2);
    cached.lookup(by_name("alice")).unwrap();
    assert_eq!(directory.calls(), 2);
}

#[test]
fn no_such_person_is_kept_for_the_negative_ttl() {
    let directory = Arc::new(Counting::default());
    let cached = Cached::new(
        directory.clone(),
        CacheOptions::default().negative_ttl(SHORT),
    );
    for _ in 0..2 {
        assert!(cached.lookup(by_name("nobody")).unwrap().is_none());
    }
    assert_eq!(directory.calls(), 1);
    sleep(SHORT * 2);
    cached.lookup(by_name("nobody")).unwrap();
    assert_eq!(directory.calls(), 2);
}

#[test]
fn an_error_is_never_kept() {
    let directory = Arc::new(Counting::default());
    *directory.fail.lock().unwrap() = true;
    let cached = Cached::new(directory.clone(), CacheOptions::default());
    for _ in 0..2 {
        assert!(cached.lookup(by_name("alice")).is_err());
        assert!(cached.authorize(request(&[], "read")).is_err());
    }
    assert_eq!(directory.calls(), 4);
}

#[test]
fn putting_or_removing_an_account_clears_what_is_kept() {
    let directory = Arc::new(Counting::default());
    let cached = Cached::new(directory.clone(), CacheOptions::default());
    cached.lookup(by_name("alice")).unwrap();
    cached.authorize(request(&[], "read")).unwrap();
    cached.put(AccountPut::default()).unwrap();
    cached.lookup(by_name("alice")).unwrap();
    cached.authorize(request(&[], "read")).unwrap();
    assert_eq!(directory.calls(), 4);
    cached.remove("alice".to_owned()).unwrap();
    cached.lookup(by_name("alice")).unwrap();
    assert_eq!(directory.calls(), 5);
    cached.invalidate(&by_name("alice"));
    cached.lookup(by_name("alice")).unwrap();
    assert_eq!(directory.calls(), 6);
}

#[test]
fn a_decision_is_kept_by_who_what_and_where_whatever_the_group_order() {
    let authorizer = Arc::new(Counting::default());
    let cached = Cached::new(
        authorizer.clone(),
        CacheOptions::default().negative_ttl(SHORT),
    );
    assert!(
        cached
            .authorize(request(&["a", "b"], "read"))
            .unwrap()
            .allowed
    );
    assert!(
        cached
            .authorize(request(&["b", "a"], "read"))
            .unwrap()
            .allowed
    );
    assert_eq!(authorizer.calls(), 1);
    assert!(!cached.authorize(request(&["a"], "write")).unwrap().allowed);
    assert!(!cached.authorize(request(&["a"], "write")).unwrap().allowed);
    assert_eq!(authorizer.calls(), 2);
    sleep(SHORT * 2);
    cached.authorize(request(&["a"], "write")).unwrap();
    cached.authorize(request(&["a", "b"], "read")).unwrap();
    assert_eq!(
        authorizer.calls(),
        3,
        "the grant is still kept, the denial is not"
    );
    let mut moved = request(&["a", "b"], "read");
    moved.ancestors = vec!["site:europe".to_owned()];
    cached.authorize(moved).unwrap();
    assert_eq!(
        authorizer.calls(),
        4,
        "another ancestry is another question"
    );
}

#[test]
fn off_asks_every_time_and_debug_shows_counts_only() {
    let directory = Arc::new(Counting::default());
    let cached = Cached::new(directory.clone(), CacheOptions::off());
    for _ in 0..3 {
        cached.lookup(by_name("alice")).unwrap();
    }
    assert_eq!(directory.calls(), 3);
    let kept = Cached::new(directory, CacheOptions::default());
    kept.lookup(by_name("alice")).unwrap();
    assert!(!format!("{kept:?}").contains("alice"));
}

#[test]
fn a_setting_reads_as_a_flag_or_as_options() {
    use coqui_access::guatiao::{FromValue, Map, Value};
    assert_eq!(
        CacheSetting::from_value(&Value::from(false)).unwrap(),
        CacheSetting::Enabled(false)
    );
    let mut tuned = Map::new();
    tuned.set("ttl", 2u32);
    let CacheSetting::Tuned(options) = CacheSetting::from_value(&tuned.into()).unwrap() else {
        panic!("a map is options");
    };
    assert_eq!(
        (options.ttl, options.negative_ttl),
        (Duration::from_secs(2), DEFAULT_NEGATIVE_TTL)
    );
}
