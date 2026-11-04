//! Secret providers loaded as Guatiao libraries, built by the Station's desk and leased through.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use guatiao::library::{ProviderError, Registry};
use guatiao::{Alloc, FromValue, Schema, Secret, Status, ToValue, Value};

use super::CredentialDesk;
use crate::directorates::q::secret_kind::{
    SecretAsk, SecretLeaseAsk, SecretLeased, SecretProvider, SecretRead,
};
use crate::directorates::q::{LoadoutCatalog, SecretError, SecretLeaseRequest};
use crate::station::plugins::Plugins;

const PLANTED: &str = "pl4nted-s3cret-value";

/// The sessions each built vault was given, and the leases it revoked.
static SESSIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static REVOKED: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(ToValue, FromValue, Schema, Debug, Clone, Default)]
#[map(crate = ::guatiao)]
struct TestVaultConfig {
    /// The vault's own credential.
    #[schema(title = "Session")]
    session: Secret<String>,
}

/// A vault that leases `leased-<key>-in-<mount>` and declares one field, `mount`.
#[derive(guatiao::Provider, Debug, Clone)]
#[provider(
    SecretProvider,
    id = "test-vault",
    name = "Test vault",
    crate = ::guatiao,
    config = TestVaultConfig
)]
struct TestVault;

impl From<TestVaultConfig> for TestVault {
    fn from(config: TestVaultConfig) -> Self {
        SESSIONS
            .lock()
            .unwrap()
            .push(config.session.expose().clone());
        Self
    }
}

#[derive(ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
struct VaultFields {
    /// Where in the vault.
    #[schema(title = "Mount")]
    mount: Option<String>,
}

fn refused(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

impl SecretProvider for TestVault {
    fn read(&self, ask: &Value) -> Result<Value, ProviderError> {
        let ask = SecretAsk::from_value(ask).map_err(|_| refused("ask"))?;
        SecretRead {
            value: Secret::new(format!("read-{}", ask.key)),
            revision: None,
        }
        .to_value_in(Alloc::wiping())
        .map_err(|_| refused("answer"))
    }

    fn fields(&self) -> Result<Value, ProviderError> {
        VaultFields::schema().map_err(|_| refused("schema"))
    }

    fn lease(&self, ask: &Value) -> Result<Value, ProviderError> {
        let ask = SecretLeaseAsk::from_value(ask).map_err(|_| refused("ask"))?;
        let mount = ask
            .fields
            .get("mount")
            .and_then(|mount| <&str>::try_from(mount).ok())
            .unwrap_or("root")
            .to_owned();
        if ask.key == "refused" {
            return Err(refused(&format!("no, and {PLANTED}")));
        }
        SecretLeased {
            lease: format!("lease-of-{}", ask.key),
            value: Secret::new(format!("leased-{}-in-{mount}", ask.key)),
        }
        .to_value_in(Alloc::wiping())
        .map_err(|_| refused("answer"))
    }

    fn revoke(&self, lease: &str) -> Result<(), ProviderError> {
        REVOKED.lock().unwrap().push(lease.to_owned());
        Ok(())
    }
}

/// A provider that keeps `env` for a field of its own.
#[derive(guatiao::Provider, Debug, Clone, Default)]
#[provider(SecretProvider, id = "test-greedy", name = "Greedy", crate = ::guatiao)]
struct Greedy;

#[derive(ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
struct GreedyFields {
    /// Taken from AI6.
    #[schema(title = "Env")]
    env: Option<String>,
}

impl SecretProvider for Greedy {
    fn read(&self, _ask: &Value) -> Result<Value, ProviderError> {
        Err(refused("never"))
    }

    fn fields(&self) -> Result<Value, ProviderError> {
        GreedyFields::schema().map_err(|_| refused("schema"))
    }
}

mod library {
    use super::{Greedy, TestVault};

    guatiao::local_providers!(
        id = "ai6-test-secret-library",
        version = "0.0.0",
        providers = [TestVault, Greedy]
    );
}

fn plugins() -> Arc<Plugins> {
    let mut registry = Registry::new("ai6-test", env!("CARGO_PKG_VERSION"));
    registry
        .register_local("ai6-test-secret-library", library::library)
        .expect("the test library registers");
    Arc::new(Plugins::from_registry(registry))
}

fn desk_of(catalog: &str) -> (CredentialDesk, Arc<LoadoutCatalog>) {
    let catalog = Arc::new(LoadoutCatalog::from_str(catalog).unwrap());
    (
        CredentialDesk::default().with_catalog((*catalog).clone()),
        catalog,
    )
}

fn request() -> SecretLeaseRequest {
    SecretLeaseRequest::with_ttl("station-a", "mission-lib", Duration::from_secs(60)).unwrap()
}

#[test]
fn a_provider_from_a_library_is_built_once_with_its_credential_and_leases() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vault-session"), format!("{PLANTED}\n")).unwrap();
    let (desk, catalog) = desk_of(
        "[loadout.007]\nsecrets = ['db', 'api', 'refused']\n\
         [secret_provider.vault]\nprovider = 'test-vault'\n\
         session = { provider = 'file', key = 'vault-session' }\n\
         [secret.db]\nprovider = 'vault'\nkey = 'db'\nmount = 'prod'\n\
         [secret.api]\nprovider = 'vault'\nkey = 'api'\n\
         [secret.refused]\nprovider = 'vault'\nkey = 'refused'\n",
    );
    let desk = desk.in_secrets_dir(dir.path());
    let plugins = plugins();
    assert_eq!(desk.check_providers(&plugins), Vec::<String>::new());
    let broker = desk.broker(&plugins, &catalog, crate::bounds::TimeLimit::UNBOUNDED);
    let loadout = catalog.for_agent("007").unwrap();
    let lease = broker
        .lease_names(loadout, &["db".to_owned(), "api".to_owned()], request())
        .unwrap();
    assert_eq!(
        lease.with_secret("db", str::to_owned).unwrap(),
        "leased-db-in-prod"
    );
    assert_eq!(
        lease.with_secret("api", str::to_owned).unwrap(),
        "leased-api-in-root"
    );
    // One instance, built once, with its credential read from the file its settings name.
    let built = SESSIONS
        .lock()
        .unwrap()
        .iter()
        .filter(|session| *session == PLANTED)
        .count();
    assert_eq!(built, 1);
    lease.revoke().unwrap();
    let revoked = REVOKED.lock().unwrap().clone();
    assert!(revoked.contains(&"lease-of-db".to_owned()), "{revoked:?}");
    assert!(revoked.contains(&"lease-of-api".to_owned()), "{revoked:?}");

    // The provider's own words never come back.
    let error = broker
        .lease_names(loadout, &["refused".to_owned()], request())
        .unwrap_err();
    assert_eq!(error, SecretError::Backend);
    for shown in [
        error.to_string(),
        format!("{error:?}"),
        format!("{lease:?}"),
        format!("{catalog:?}"),
        format!("{desk:?}"),
        format!("{broker:?}"),
    ] {
        assert!(!shown.contains(PLANTED), "{shown}");
    }
    for line in desk.weakenings() {
        assert!(!line.contains(PLANTED), "{line}");
    }
}

#[test]
fn a_reserved_field_a_cycle_a_taken_name_and_an_unknown_field_are_refused() {
    let plugins = plugins();
    let (desk, _) = desk_of(
        "[loadout.007]\nsecrets = ['greedy']\n\
         [secret.greedy]\nprovider = 'test-greedy'\n",
    );
    let refused = desk.check_providers(&plugins);
    assert!(
        refused
            .iter()
            .any(|line| line.contains("test-greedy") && line.contains("`env`")),
        "{refused:?}"
    );

    let (desk, catalog) = desk_of(
        "[secret_provider.a]\nprovider = 'test-vault'\nsession = { provider = 'b', key = 'x' }\n\
         [secret_provider.b]\nprovider = 'test-vault'\nsession = { provider = 'a', key = 'y' }\n\
         [secret_provider.fake-secrets]\nprovider = 'test-vault'\n\
         session = { provider = 'inline', value = 'v' }\n\
         [secret_provider.c]\nprovider = 'test-vault'\nsession = { provider = 'inline', value = 'v' }\n\
         [loadout.007]\nsecrets = ['odd']\n\
         [secret.odd]\nprovider = 'c'\nshelf = 'x'\n",
    );
    let refused = desk.check_providers(&plugins);
    assert!(
        refused
            .iter()
            .any(|line| line.contains("leads back") && line.contains("a -> b -> a")),
        "{refused:?}"
    );
    assert!(
        refused
            .iter()
            .any(|line| line.contains("\"fake-secrets\"") && line.contains("loaded provider")),
        "{refused:?}"
    );
    assert!(
        refused
            .iter()
            .any(|line| line.contains("\"odd\"") && line.contains("`shelf`")),
        "{refused:?}"
    );
    let broker = desk.broker(&plugins, &catalog, crate::bounds::TimeLimit::UNBOUNDED);
    assert_eq!(
        broker
            .lease_names(
                catalog.for_agent("007").unwrap(),
                &["odd".to_owned()],
                request()
            )
            .err(),
        Some(SecretError::Backend)
    );
}

#[test]
fn the_built_ins_are_registered_first_and_appear_with_their_schemas() {
    let plugins = plugins();
    let ids: Vec<String> = plugins
        .secret_providers()
        .into_iter()
        .map(|provider| provider.id)
        .collect();
    for id in ["file", "env", "inline", "test-vault"] {
        assert!(ids.iter().any(|loaded| loaded == id), "{id}: {ids:?}");
    }
    let file = plugins.config_schema("file").unwrap();
    assert!(file["properties"].get("dir").is_some(), "{file}");
    let vault = plugins.config_schema("test-vault").unwrap();
    assert_eq!(
        vault["properties"]["session"]["x-sensitive"], true,
        "{vault}"
    );
}
