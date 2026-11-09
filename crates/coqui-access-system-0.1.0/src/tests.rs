use coqui_access::guatiao::Status;

use super::*;

fn account(uid: Option<u32>, groups: &[&str]) -> os::Account {
    os::Account {
        id: "1".to_owned(),
        name: "someone".to_owned(),
        display_name: None,
        groups: groups.iter().map(|group| (*group).to_owned()).collect(),
        uid,
        disabled: false,
    }
}

#[test]
fn the_defaults_are_the_documented_ones_and_list_no_non_default() {
    let config = SystemAccountsConfig::default();
    assert_eq!(config.pam_service, "coqui-access");
    assert_eq!(config.domain, ".");
    assert!(config.groups.is_empty());
    #[cfg(target_os = "linux")]
    assert_eq!(config.min_uid, 1000);
    #[cfg(target_os = "freebsd")]
    assert_eq!(config.min_uid, 1001);
    #[cfg(target_os = "macos")]
    assert_eq!(config.min_uid, 501);
    assert!(config.non_defaults().is_empty());
}

#[test]
fn a_configuration_omitting_every_key_reads_as_the_defaults() {
    let empty = coqui_access::guatiao::Map::new().into();
    assert_eq!(
        SystemAccountsConfig::from_value(&empty).unwrap(),
        SystemAccountsConfig::default()
    );
    // Windows accounts carry no uid, so a min_uid there is refused and the default is kept.
    let uid = if cfg!(unix) { 5 } else { DEFAULT_MIN_UID };
    let written = SystemAccounts::builder()
        .min_uid(uid)
        .groups(["wheel"])
        .build()
        .unwrap()
        .config()
        .to_value()
        .unwrap();
    let read = SystemAccountsConfig::from_value(&written).unwrap();
    assert_eq!(read.min_uid, uid);
    assert_eq!(read.groups, ["wheel"]);
}

#[test]
fn a_misspelled_key_is_refused() {
    let mut map = coqui_access::guatiao::Map::new();
    map.set("min_id", 5u32.to_value().unwrap());
    assert!(SystemAccountsConfig::from_value(&map.into()).is_err());
}

#[test]
fn admitting_root_or_service_accounts_is_reported_not_refused() {
    #[cfg(unix)]
    {
        let root = SystemAccounts::builder().min_uid(0).build().unwrap();
        let service = SystemAccounts::builder().min_uid(1).build().unwrap();
        assert_eq!(root.config().non_defaults(), [NonDefault::AdmitsRoot]);
        assert_eq!(
            service.config().non_defaults(),
            [NonDefault::AdmitsServiceAccounts]
        );
    }
    assert_eq!(NonDefault::AdmitsRoot.to_string(), "min_uid 0 admits root");
}

#[cfg(not(unix))]
#[test]
fn a_min_uid_is_refused_where_accounts_carry_no_uid() {
    assert!(
        SystemAccounts::builder()
            .min_uid(DEFAULT_MIN_UID)
            .build()
            .is_ok()
    );
    assert!(SystemAccounts::builder().min_uid(5).build().is_err());
}

#[test]
fn malformed_options_are_refused_with_a_bad_value() {
    for builder in [
        SystemAccounts::builder().pam_service(""),
        SystemAccounts::builder().pam_service("../login"),
        SystemAccounts::builder().pam_service("a\0b"),
        SystemAccounts::builder().domain(""),
        SystemAccounts::builder().groups([""]),
    ] {
        let refused = builder.build().unwrap_err();
        assert_eq!(refused.status, Status::GUATIAO_ERR_BAD_VALUE);
    }
}

#[test]
fn min_uid_and_groups_narrow_who_is_admitted() {
    let config = SystemAccountsConfig {
        min_uid: 1000,
        groups: vec!["staff".to_owned(), "wheel".to_owned()],
        ..SystemAccountsConfig::default()
    };
    assert!(account(Some(1000), &["users", "wheel"]).admitted_by(&config));
    assert!(!account(Some(999), &["wheel"]).admitted_by(&config));
    assert!(!account(Some(1000), &["users"]).admitted_by(&config));
    // No uid (Windows): only the groups apply.
    assert!(account(None, &["staff"]).admitted_by(&config));
    let any = SystemAccountsConfig {
        min_uid: 0,
        ..SystemAccountsConfig::default()
    };
    assert!(account(Some(0), &[]).admitted_by(&any));
}

#[test]
fn an_attempt_without_a_name_and_password_is_unknown_and_an_unknown_name_too() {
    let accounts = SystemAccounts::builder().build().unwrap();
    assert_eq!(
        accounts.authenticate(Attempt::default()).unwrap(),
        Verdict::Unknown
    );
    let nobody = Attempt {
        name: Some("no-such-account-x".to_owned()),
        secret: Some("pw".to_owned()),
        ..Attempt::default()
    };
    assert_eq!(accounts.authenticate(nobody).unwrap(), Verdict::Unknown);
    for value in ["", "a\0b"] {
        let lookup = Lookup {
            by: "name".to_owned(),
            value: value.to_owned(),
        };
        assert_eq!(accounts.lookup(lookup).unwrap(), None);
    }
    let other = Lookup {
        by: "email".to_owned(),
        value: "a@b".to_owned(),
    };
    assert_eq!(accounts.lookup(other).unwrap(), None);
}

#[test]
fn the_provider_says_what_it_takes() {
    let accounts = SystemAccounts::builder().build().unwrap();
    let described = Authenticator::describe(&accounts).unwrap();
    assert_eq!(described.takes.as_deref(), Some("password"));
    assert_eq!(described.name.as_deref(), Some("System accounts"));
}

#[cfg(unix)]
#[test]
fn root_is_not_admitted_by_default_but_is_with_min_uid_zero() {
    let root = || Lookup {
        by: "id".to_owned(),
        value: "0".to_owned(),
    };
    let accounts = SystemAccounts::builder().build().unwrap();
    assert_eq!(accounts.lookup(root()).unwrap(), None);
    let everyone = SystemAccounts::builder().min_uid(0).build().unwrap();
    let person = everyone.lookup(root()).unwrap().unwrap();
    assert_eq!(person.name, "root");
    assert_eq!(person.id, "0");
    let attempt = Attempt {
        name: Some("root".to_owned()),
        secret: Some("not root's password".to_owned()),
        ..Attempt::default()
    };
    assert_eq!(accounts.authenticate(attempt).unwrap(), Verdict::Unknown);
}
