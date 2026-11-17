//! Tying a certificate to an account through the pins and the explicit map the Registry keeps.
//!
//! An authenticator that has verified a certificate answers with its claims and no account. The
//! directory a gateway asks ties them to one: first the explicit map (a certificate field value
//! an account is named by), then rules that nominate an account from a field and require the
//! account to hold a pin of this certificate. Both read the Registry at each sign-in, so every
//! gateway sharing it sees a change at its next read.

use std::sync::Arc;

use coqui_access::guatiao::library::ProviderError;
use coqui_access::{Directory, Lookup, Person};
use coqui_access_kit::matcher::{Match, MatchRule, Matcher, MatcherConfig};
use coqui_access_kit::pins::{PinCheck, PinSet, PinSource, pin_matches};
use coqui_access_kit::table::SourceError;

use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::pins::PinStore;

/// The name the rules give the pin source.
const SOURCE: &str = "registry";

/// The pins of an account, as the Registry holds them.
pub struct RegistryPins {
    registry: Arc<dyn RegistryBackend>,
}

impl RegistryPins {
    /// A source over `registry`.
    pub fn new(registry: Arc<dyn RegistryBackend>) -> Self {
        Self { registry }
    }
}

impl PinSource for RegistryPins {
    fn check(&self, person: &Person, pins: &PinSet) -> Result<PinCheck, ProviderError> {
        let held = self
            .registry
            .account_pins(&person.name)
            .map_err(|_| SourceError::unavailable("the Registry cannot be read").reading("pins"))?;
        let holds = held
            .pins
            .iter()
            .any(|pin| pin_matches(pin, pins) == Some(true));
        Ok(if holds {
            PinCheck::Holds
        } else {
            PinCheck::Absent
        })
    }
}

/// The directory a gateway asks: a certificate's claims are answered from the Registry's pins
/// and explicit map, and every other lookup by `inner`.
pub struct CertificateDirectory {
    inner: Arc<dyn Directory>,
    registry: Arc<dyn RegistryBackend>,
    matcher: Matcher,
}

impl CertificateDirectory {
    /// `inner` with the pins and the explicit map of `registry`.
    ///
    /// The rules nominate an account by the certificate's UPN, email address or common name;
    /// each also needs the account to hold a pin of this certificate, so a name a certificate
    /// carries is never enough alone.
    pub fn new(inner: Arc<dyn Directory>, registry: Arc<dyn RegistryBackend>) -> Self {
        let mut config = MatcherConfig::default();
        config.rule = vec![
            MatchRule::new("name", "{san.upn|local}").pins(SOURCE),
            MatchRule::new("email", "{san.email}").pins(SOURCE),
            MatchRule::new("name", "{subject.cn}").pins(SOURCE),
        ];
        let matcher = Matcher::builder(config)
            .pin_source(SOURCE, Arc::new(RegistryPins::new(registry.clone())))
            .build()
            .expect("the certificate rules are valid");
        Self {
            inner,
            registry,
            matcher,
        }
    }

    /// The accounts the explicit map names for `lookup`'s claims.
    fn mapped(&self, lookup: &Lookup) -> Result<Vec<String>, ProviderError> {
        let Some(claims) = &lookup.claims else {
            return Ok(Vec::new());
        };
        let all = self.registry.all_pins().map_err(|_| {
            SourceError::unavailable("the Registry cannot be read").reading("pin map")
        })?;
        Ok(all
            .into_iter()
            .filter(|(_, held)| {
                held.matches.iter().any(|entry| {
                    claims
                        .get(&entry.field)
                        .iter()
                        .any(|value| value.eq_ignore_ascii_case(&entry.value))
                })
            })
            .map(|(account, _)| account)
            .collect())
    }
}

impl Directory for CertificateDirectory {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        if lookup.by != "claims" || lookup.claims.is_none() {
            return self.inner.lookup(lookup);
        }
        let mut named = self.mapped(&lookup)?;
        match named.len() {
            0 => {}
            1 => {
                return self.inner.lookup(Lookup {
                    by: "name".to_owned(),
                    value: named.remove(0),
                    claims: None,
                });
            }
            // Two accounts name one certificate: refused, never a choice.
            _ => return Ok(None),
        }
        Ok(
            match self.matcher.resolve(&lookup, self.inner.as_ref())?.result {
                Match::Found(person) => Some(person),
                _ => None,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use coqui_access::Claims;

    use super::*;
    use crate::directorates::registry::pins::{PinChange, PinMatch};
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_pinned_certificate_is_found_by_a_second_gateway_and_a_removed_pin_is_not,
        the_explicit_map_names_an_account_and_never_two
    );

    const PIN: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// Alice and Bob, found by name or email.
    struct People;

    impl Directory for People {
        fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
            let name = match lookup.by.as_str() {
                "name" => lookup.value.clone(),
                "email" => lookup
                    .value
                    .strip_suffix("@example.org")
                    .unwrap_or_default()
                    .to_owned(),
                _ => return Ok(None),
            };
            Ok(["alice", "bob"].contains(&name.as_str()).then(|| Person {
                id: format!("{name}-id"),
                name,
                ..Person::default()
            }))
        }
    }

    /// What a verified certificate with this address and fingerprint says.
    fn certificate(email: &str) -> Lookup {
        let values = BTreeMap::from([
            ("san.email".to_owned(), vec![email.to_owned()]),
            ("fingerprint".to_owned(), vec![PIN.to_owned()]),
        ]);
        Lookup {
            by: "claims".to_owned(),
            value: "certificate".to_owned(),
            claims: Some(Claims {
                values,
                ..Claims::default()
            }),
        }
    }

    fn gateway(store: &dyn Store) -> CertificateDirectory {
        CertificateDirectory::new(Arc::new(People), store.handle())
    }

    fn added(pin: &str) -> PinChange {
        PinChange {
            add: vec![pin.to_owned()],
            ..PinChange::default()
        }
    }

    fn a_pinned_certificate_is_found_by_a_second_gateway_and_a_removed_pin_is_not(
        store: &dyn Store,
    ) {
        let (first, second) = (gateway(store), gateway(store));
        let registry = store.handle();
        let alice = || certificate("alice@example.org");
        assert_eq!(second.lookup(alice()).unwrap(), None, "no pin, no sign-in");
        registry.change_pins("alice", &added(PIN), 4).unwrap();
        let found = second.lookup(alice()).unwrap();
        assert_eq!(found.map(|person| person.name), Some("alice".to_owned()));
        assert_eq!(first.lookup(alice()).unwrap().unwrap().id, "alice-id");
        // The pin is the certificate's, not the address'.
        assert_eq!(
            second.lookup(certificate("bob@example.org")).unwrap(),
            None,
            "Bob holds no pin of it"
        );
        registry
            .change_pins(
                "alice",
                &PinChange {
                    remove: vec![PIN.to_owned()],
                    ..PinChange::default()
                },
                4,
            )
            .unwrap();
        assert_eq!(second.lookup(alice()).unwrap(), None, "a removed pin");
        assert_eq!(first.lookup(alice()).unwrap(), None);
        // Lookups that are not a certificate's go to the directory.
        let by_name = Lookup {
            by: "name".to_owned(),
            value: "bob".to_owned(),
            claims: None,
        };
        assert!(second.lookup(by_name).unwrap().is_some());
    }

    fn the_explicit_map_names_an_account_and_never_two(store: &dyn Store) {
        let directory = gateway(store);
        let registry = store.handle();
        let entry = PinMatch {
            field: "san.email".to_owned(),
            value: "Carol@Example.org".to_owned(),
        };
        let map = |entry: &PinMatch| PinChange {
            match_add: vec![entry.clone()],
            ..PinChange::default()
        };
        assert_eq!(
            directory.lookup(certificate("carol@example.org")).unwrap(),
            None
        );
        registry.change_pins("bob", &map(&entry), 4).unwrap();
        let found = directory.lookup(certificate("carol@example.org")).unwrap();
        assert_eq!(found.map(|person| person.name), Some("bob".to_owned()));
        registry.change_pins("alice", &map(&entry), 4).unwrap();
        assert_eq!(
            directory.lookup(certificate("carol@example.org")).unwrap(),
            None,
            "two accounts name it"
        );
    }
}
