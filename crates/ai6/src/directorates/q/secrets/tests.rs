use super::{SecretBroker, SecretError, SecretLeaseRequest, SecretValue};
use crate::directorates::q::LoadoutCatalog;
use crate::directorates::q::secret_kind::builtin::{EnvSecrets, FileSecrets, InlineSecrets};
use crate::directorates::q::secret_kind::{
    SecretKeeper, SecretLeaseAsk, SecretLeased, SecretProvider, SecretRead,
};
use guatiao::library::ProviderError;
use guatiao::{Alloc, FromValue, Map, Secret, Status, ToValue, Value};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TEST_TTL: Duration = Duration::from_secs(300);
const PLANTED: &str = "pl4nted-s3cret-value";

#[derive(Default)]
struct VaultState {
    leased: Vec<SecretLeaseAsk>,
    revoked: Vec<String>,
    fail_lease: bool,
    fail_revoke: bool,
    empty: bool,
}

/// A provider that leases [`PLANTED`] and remembers what it leased and revoked.
#[derive(Clone, Default)]
struct Vault(Arc<Mutex<VaultState>>);

impl SecretProvider for Vault {
    fn read(&self, _ask: &Value) -> Result<Value, ProviderError> {
        Err(ProviderError::new(
            Status::GUATIAO_ERR_INTERNAL,
            "a leasing provider is not read",
        ))
    }

    fn lease(&self, ask: &Value) -> Result<Value, ProviderError> {
        let mut state = self.0.lock().unwrap();
        if state.fail_lease {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_INTERNAL,
                &format!("refused, and here is the value: {PLANTED}"),
            ));
        }
        let ask = SecretLeaseAsk::from_value(ask).unwrap();
        let lease = format!("lease-{}", state.leased.len());
        state.leased.push(ask);
        let value = if state.empty { "" } else { PLANTED };
        Ok(SecretLeased {
            lease,
            value: Secret::new(value.to_owned()),
        }
        .to_value_in(Alloc::wiping())
        .unwrap())
    }

    fn revoke(&self, lease: &str) -> Result<(), ProviderError> {
        let mut state = self.0.lock().unwrap();
        if state.fail_revoke {
            return Err(ProviderError::new(Status::GUATIAO_ERR_INTERNAL, "down"));
        }
        state.revoked.push(lease.to_owned());
        Ok(())
    }
}

/// A provider that reads `value-of-<key>` and does not lease.
struct Reader;

impl SecretProvider for Reader {
    fn read(&self, ask: &Value) -> Result<Value, ProviderError> {
        let ask = crate::directorates::q::secret_kind::SecretAsk::from_value(ask).unwrap();
        Ok(SecretRead {
            value: Secret::new(format!("value-of-{}", ask.key)),
            revision: None,
        }
        .to_value_in(Alloc::wiping())
        .unwrap())
    }
}

fn keeper(provider: impl SecretProvider + 'static) -> Arc<SecretKeeper> {
    Arc::new(SecretKeeper::new(Box::new(provider)).unwrap())
}

/// A broker whose every secret is `vault`'s, unless a `[secret.<name>]` in `extra` says other.
fn broker(vault: &Vault, extra: &str) -> SecretBroker {
    let catalog = catalog(extra);
    let vault = keeper(vault.clone());
    let reader = keeper(Reader);
    SecretBroker::new(Arc::new(catalog.clone()), move |provider| match provider {
        "vault" => Ok(vault.clone()),
        "reader" => Ok(reader.clone()),
        other => Err(format!("no provider {other:?}")),
    })
}

fn catalog(extra: &str) -> LoadoutCatalog {
    LoadoutCatalog::from_str(&format!(
        "[loadout.007]\nsecrets = ['token', 'other']\n\
         [secret.token]\nprovider = 'vault'\n[secret.other]\nprovider = 'vault'\n{extra}"
    ))
    .expect("valid test loadout")
}

fn loadout(catalog: &LoadoutCatalog) -> crate::directorates::q::Loadout {
    catalog.for_agent("007").unwrap().clone()
}

fn request() -> SecretLeaseRequest {
    SecretLeaseRequest::with_ttl("station-a", "mission-1", TEST_TTL).expect("valid lease request")
}

fn token() -> Vec<String> {
    vec!["token".to_owned()]
}

#[test]
fn a_lease_comes_from_the_provider_and_debug_redacts_material() {
    let vault = Vault::default();
    let catalog = catalog("");
    let lease = broker(&vault, "")
        .lease_names(&loadout(&catalog), &token(), request())
        .expect("the provider grants the declared secret");
    assert!(lease.is_active());
    assert_eq!(lease.names(), ["token"]);
    assert_eq!(lease.with_secret("token", str::to_owned).unwrap(), PLANTED);
    assert_eq!(
        lease.with_secret("other", str::to_owned),
        Err(SecretError::AccessDenied)
    );
    {
        let state = vault.0.lock().unwrap();
        let asked = &state.leased[0];
        assert_eq!(
            (
                asked.key.as_str(),
                asked.station.as_str(),
                asked.holder.as_str()
            ),
            ("token", "station-a", "mission-1")
        );
        assert_eq!(asked.ttl_secs, TEST_TTL.as_secs_f64());
    }
    assert!(!format!("{lease:?}").contains(PLANTED));
    assert!(!format!("{:?}", SecretValue::new(PLANTED)).contains(PLANTED));
    lease.revoke().unwrap();
    lease.revoke().unwrap();
    assert!(lease.names().is_empty());
    assert_eq!(vault.0.lock().unwrap().revoked, ["lease-0"]);
}

#[test]
fn a_provider_that_does_not_lease_is_read_and_has_nothing_to_revoke() {
    let vault = Vault::default();
    let catalog = catalog("");
    let broker = broker(
        &vault,
        "[loadout.r]\nsecrets = ['plain']\n[secret.plain]\nprovider = 'reader'\nkey = 'k'\n",
    );
    let lease = broker
        .lease_names(
            &catalog_with_reader().for_agent("r").unwrap().clone(),
            &["plain".to_owned()],
            request(),
        )
        .unwrap();
    assert_eq!(
        lease.with_secret("plain", str::to_owned).unwrap(),
        "value-of-k"
    );
    lease.revoke().unwrap();
    assert!(vault.0.lock().unwrap().revoked.is_empty());
    drop(catalog);
}

fn catalog_with_reader() -> LoadoutCatalog {
    catalog("[loadout.r]\nsecrets = ['plain']\n[secret.plain]\nprovider = 'reader'\nkey = 'k'\n")
}

#[test]
fn a_failed_lease_revokes_what_was_leased_and_withholds_the_provider_s_words() {
    let vault = Vault::default();
    let catalog = catalog("");
    // `other` is leased first, by name order, then `token` fails.
    let broker = broker(&vault, "");
    let names = vec!["token".to_owned(), "other".to_owned()];
    {
        let mut state = vault.0.lock().unwrap();
        state.fail_lease = false;
    }
    let first = broker
        .lease_names(&loadout(&catalog), &names, request())
        .unwrap();
    first.revoke().unwrap();
    vault.0.lock().unwrap().revoked.clear();
    // The second lease of a two-name request fails after the first succeeded.
    struct FailSecond(Vault);
    impl SecretProvider for FailSecond {
        fn read(&self, ask: &Value) -> Result<Value, ProviderError> {
            self.0.read(ask)
        }
        fn lease(&self, ask: &Value) -> Result<Value, ProviderError> {
            let leased = self.0.0.lock().unwrap().leased.len();
            if leased >= 3 {
                return Err(ProviderError::new(
                    Status::GUATIAO_ERR_INTERNAL,
                    &format!("no: {PLANTED}"),
                ));
            }
            self.0.lease(ask)
        }
        fn revoke(&self, lease: &str) -> Result<(), ProviderError> {
            self.0.revoke(lease)
        }
    }
    let failing = keeper(FailSecond(vault.clone()));
    let failing_broker = SecretBroker::new(Arc::new(catalog.clone()), move |_| Ok(failing.clone()));
    let error = failing_broker
        .lease_names(&loadout(&catalog), &names, request())
        .unwrap_err();
    assert_eq!(error, SecretError::Backend);
    assert!(!error.to_string().contains(PLANTED));
    assert!(!format!("{error:?}").contains(PLANTED));
    assert_eq!(
        vault.0.lock().unwrap().revoked,
        ["lease-2"],
        "the lease of `other` is revoked"
    );
}

#[test]
fn an_unknown_provider_or_an_empty_value_is_refused() {
    let vault = Vault::default();
    let catalog = catalog("[loadout.x]\nsecrets = ['lost']\n[secret.lost]\nprovider = 'nowhere'\n");
    let broker = broker(
        &vault,
        "[loadout.x]\nsecrets = ['lost']\n[secret.lost]\nprovider = 'nowhere'\n",
    );
    assert_eq!(
        broker
            .lease_names(
                catalog.for_agent("x").unwrap(),
                &["lost".to_owned()],
                request()
            )
            .err(),
        Some(SecretError::Backend)
    );
    vault.0.lock().unwrap().empty = true;
    assert_eq!(
        broker
            .lease_names(&loadout(&catalog), &token(), request())
            .err(),
        Some(SecretError::InvalidResponse)
    );
    assert_eq!(vault.0.lock().unwrap().revoked, ["lease-0"]);
}

#[test]
fn expired_lease_clears_values_and_revokes_at_the_provider() {
    let vault = Vault::default();
    let catalog = catalog("");
    let lease = broker(&vault, "")
        .lease_names(&loadout(&catalog), &token(), request())
        .unwrap();
    assert!(lease.is_active());
    lease.values.lock().unwrap().expires_at = std::time::Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        lease.with_secret("token", str::to_owned),
        Err(SecretError::Expired)
    );
    assert!(lease.names().is_empty());
    assert_eq!(vault.0.lock().unwrap().revoked.len(), 1);
}

#[test]
fn invalid_lifetimes_and_provider_revocation_fail_closed() {
    assert_eq!(
        SecretLeaseRequest::with_ttl("station-a", "mission-1", Duration::ZERO).err(),
        Some(SecretError::InvalidTtl)
    );
    let vault = Vault::default();
    let catalog = catalog("");
    let broker = broker(&vault, "").with_max_ttl(crate::bounds::TimeLimit::secs(60));
    assert_eq!(
        broker
            .lease_names(&loadout(&catalog), &token(), request())
            .err(),
        Some(SecretError::InvalidTtl)
    );
    let lease = self::broker(&vault, "")
        .lease_names(&loadout(&catalog), &token(), request())
        .unwrap();
    vault.0.lock().unwrap().fail_revoke = true;
    assert_eq!(lease.revoke(), Err(SecretError::Backend));
    assert!(lease.names().is_empty());
    vault.0.lock().unwrap().fail_revoke = false;
    lease.revoke().unwrap();
    assert_eq!(vault.0.lock().unwrap().revoked.len(), 1);
}

#[test]
fn an_empty_request_leases_nothing() {
    let vault = Vault::default();
    let empty = LoadoutCatalog::from_str("[loadout.007]")
        .unwrap()
        .for_agent("007")
        .unwrap()
        .clone();
    let lease = broker(&vault, "").lease(&empty, request()).unwrap();
    lease.revoke().unwrap();
    assert!(vault.0.lock().unwrap().leased.is_empty());
    assert!(vault.0.lock().unwrap().revoked.is_empty());
}

#[test]
fn a_lease_of_some_names_takes_only_declared_ones() {
    let catalog = catalog("");
    let broker = broker(&Vault::default(), "");
    let lease = broker
        .lease_names(&loadout(&catalog), &token(), request())
        .unwrap();
    assert_eq!(lease.names(), ["token"]);
    assert_eq!(
        broker
            .lease_names(&loadout(&catalog), &["undeclared".to_owned()], request())
            .err(),
        Some(SecretError::AccessDenied)
    );
}

/// A broker over the three built-ins, `file` reading from `dir`.
fn builtins(catalog: &LoadoutCatalog, dir: &std::path::Path) -> SecretBroker {
    let file = keeper(FileSecrets::from(
        crate::directorates::q::secret_kind::builtin::FileSecretsConfig {
            dir: Some(dir.to_path_buf()),
        },
    ));
    let env = keeper(EnvSecrets);
    let inline = keeper(InlineSecrets);
    SecretBroker::new(Arc::new(catalog.clone()), move |provider| match provider {
        "file" => Ok(file.clone()),
        "env" => Ok(env.clone()),
        "inline" => Ok(inline.clone()),
        other => Err(format!("no provider {other:?}")),
    })
}

#[test]
fn the_built_ins_read_a_file_a_variable_and_an_inline_value() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("token"), format!("{PLANTED}-file\r\n")).unwrap();
    std::fs::write(dir.path().join("empty"), "\n").unwrap();
    // A variable every test process has: setting one would need `unsafe`, which this crate forbids.
    let variable = "PATH";
    let path = std::env::var(variable).unwrap();
    let catalog = LoadoutCatalog::from_str(&format!(
        "[loadout.007]\nsecrets = ['token', 'kept', 'from-env', 'written', 'missing', 'empty']\n\
         [secret.kept]\nkey = {:?}\n\
         [secret.from-env]\nprovider = 'env'\nkey = '{variable}'\n\
         [secret.written]\nprovider = 'inline'\nvalue = '{PLANTED}-inline'\n\
         [secret.missing]\nkey = 'absent'\n",
        dir.path().join("token").to_string_lossy()
    ))
    .unwrap();
    let loadout = catalog.for_agent("007").unwrap();
    let broker = builtins(&catalog, dir.path());
    let read = |name: &str| {
        broker
            .lease_names(loadout, &[name.to_owned()], request())
            .and_then(|lease| lease.with_secret(name, str::to_owned))
    };
    // With no table, a secret is the file of its own name in the secrets directory.
    assert_eq!(read("token").unwrap(), format!("{PLANTED}-file"));
    assert_eq!(read("kept").unwrap(), format!("{PLANTED}-file"));
    assert_eq!(read("from-env").unwrap(), path);
    assert_eq!(read("written").unwrap(), format!("{PLANTED}-inline"));
    assert_eq!(read("missing"), Err(SecretError::Backend));
    assert_eq!(read("empty"), Err(SecretError::Backend));
    for name in ["token", "kept", "from-env", "written", "missing"] {
        let definition = catalog.secret(name);
        assert!(!format!("{definition:?}").contains(PLANTED), "{name}");
    }
    assert!(!format!("{catalog:?}").contains(PLANTED));
}

#[test]
fn a_built_in_refuses_a_field_it_does_not_take_without_repeating_it() {
    let dir = tempfile::tempdir().unwrap();
    let keeper = keeper(FileSecrets::default());
    let mut fields = serde_json::Map::new();
    fields.insert("extra".to_owned(), PLANTED.into());
    let fields = crate::directorates::q::secret_kind::SecretFields::new(fields);
    let refused = keeper.check_fields(&fields).unwrap_err();
    assert!(refused.contains("extra") && !refused.contains(PLANTED));
    let read = keeper
        .read(&dir.path().join("x").to_string_lossy(), &fields)
        .err()
        .unwrap();
    assert!(read.to_string().contains("extra"));
    assert!(!read.to_string().contains(PLANTED));
    assert!(!format!("{fields:?}").contains(PLANTED));
}

#[test]
fn a_provider_declaring_a_reserved_field_is_refused() {
    struct Greedy(&'static str);
    impl SecretProvider for Greedy {
        fn read(&self, _ask: &Value) -> Result<Value, ProviderError> {
            Err(ProviderError::new(Status::GUATIAO_ERR_INTERNAL, "never"))
        }
        fn fields(&self) -> Result<Value, ProviderError> {
            let mut field = Map::new();
            field.set("type", "string");
            let mut properties = Map::new();
            properties.set(self.0, field);
            properties.set("vault_path", Map::new());
            let mut schema = Map::new();
            schema.set("type", "object");
            schema.set("properties", properties);
            Ok(schema.into())
        }
    }
    for reserved in ["env", "provider", "key", "green_ink"] {
        let refused = SecretKeeper::new(Box::new(Greedy(reserved))).unwrap_err();
        assert!(refused.contains(&format!("`{reserved}`")), "{refused}");
    }
    let accepted = SecretKeeper::new(Box::new(Greedy("mount"))).unwrap();
    assert_eq!(accepted.fields(), ["mount", "vault_path"]);
    assert_eq!(
        SecretKeeper::new(Box::new(InlineSecrets)).unwrap().fields(),
        ["value"]
    );
}

#[test]
fn store_reports_a_conflict_and_a_provider_that_does_not_store() {
    struct Shelf(Mutex<Option<(String, String)>>);
    impl SecretProvider for Shelf {
        fn read(&self, _ask: &Value) -> Result<Value, ProviderError> {
            Err(ProviderError::new(Status::GUATIAO_ERR_NOT_FOUND, "empty"))
        }
        fn store(&self, request: &Value) -> Result<Value, ProviderError> {
            let request = crate::directorates::q::secret_kind::SecretStore::from_value(request)
                .map_err(|_| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, "bad"))?;
            let mut held = self.0.lock().unwrap();
            let current = held.as_ref().map(|(revision, _)| revision.clone());
            if request.expect.is_some() && request.expect != current {
                return Err(ProviderError::new(Status::GUATIAO_ERR_BUSY, "changed"));
            }
            let revision = format!("r{}", current.map_or(0, |r| r.len()) + 1);
            *held = Some((revision.clone(), request.value.expose().clone()));
            Ok(crate::directorates::q::secret_kind::SecretStored {
                revision: Some(revision),
            }
            .to_value()
            .unwrap())
        }
    }
    let shelf = SecretKeeper::new(Box::new(Shelf(Mutex::new(None)))).unwrap();
    let fields = crate::directorates::q::secret_kind::SecretFields::default();
    let first = shelf.store("k", &fields, PLANTED, None).unwrap();
    assert_eq!(first.revision.as_deref(), Some("r1"));
    let refused = shelf.store("k", &fields, PLANTED, Some("r0")).unwrap_err();
    assert!(matches!(
        refused,
        crate::directorates::q::secret_kind::SecretStoreError::Conflict
    ));
    assert!(!format!("{refused} {refused:?}").contains(PLANTED));
    shelf.store("k", &fields, PLANTED, Some("r1")).unwrap();
    let reader = SecretKeeper::new(Box::new(Reader)).unwrap();
    assert!(matches!(
        reader.store("k", &fields, PLANTED, None),
        Err(crate::directorates::q::secret_kind::SecretStoreError::NotOffered)
    ));
}

#[test]
fn secret_lease_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SecretBroker>();
    assert_send_sync::<super::SecretLease>();
    assert_send_sync::<SecretValue>();
    assert_send_sync::<SecretKeeper>();
}

#[test]
fn floor_hash_is_pinned() {
    // A changed required signature affects every built provider.
    assert_eq!(
        crate::directorates::q::secret_kind::SecretProviderVtable::FLOOR_HASH,
        2_241_544_374
    );
}
