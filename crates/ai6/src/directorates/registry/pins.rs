//! The certificate pins an account holds and the certificate fields that name it, kept in the
//! Registry so every gateway and Station sharing it reads the same ones.
//!
//! One document per account, in the factor document store, so a change is made from what is
//! held under the same lock or transaction that serializes every other write. A read takes no
//! lock and does not move the Registry's revision.

use std::io;

use serde::{Deserialize, Serialize};

use super::factors::{FactorChange, FactorStore};

/// The family the documents are kept under; each key is an account's name.
const FAMILY: &str = "pins";

/// A certificate field value that names an account without a pin, such as `san.email` and an
/// address. Field names are lowercase and values compare without regard to ASCII case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinMatch {
    /// The certificate field, as the matcher's claims name it.
    pub field: String,
    /// What the field holds.
    pub value: String,
}

impl PinMatch {
    /// This entry as it is kept: field and value lowercase.
    #[must_use]
    pub fn normal(&self) -> Self {
        Self {
            field: self.field.to_ascii_lowercase(),
            value: self.value.to_ascii_lowercase(),
        }
    }
}

/// What one account holds: pins as `sha256:<hex>` (a whole certificate) or
/// `spki-sha256:<hex>` (its public key), and the fields that name it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountPins {
    /// The pins, in the order they were added.
    #[serde(default)]
    pub pins: Vec<String>,
    /// The explicit map entries, in the order they were added.
    #[serde(default)]
    pub matches: Vec<PinMatch>,
}

/// A change to what one account holds: removals first, then additions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinChange {
    /// Pins to add; one already held stays as it is.
    pub add: Vec<String>,
    /// Pins to take away; one not held is ignored.
    pub remove: Vec<String>,
    /// Map entries to add.
    pub match_add: Vec<PinMatch>,
    /// Map entries to take away.
    pub match_remove: Vec<PinMatch>,
}

/// The pins and the explicit map the Registry keeps per account.
pub trait PinStore: FactorStore {
    /// What `account` holds; nothing for an account that holds nothing.
    ///
    /// # Errors
    ///
    /// `InvalidData` for a document that is not one this Registry wrote; the store's own errors.
    fn account_pins(&self, account: &str) -> io::Result<AccountPins> {
        match self.factor_get(FAMILY, account)? {
            Some(text) => decode(&text),
            None => Ok(AccountPins::default()),
        }
    }

    /// Every account that holds anything, with what it holds.
    ///
    /// # Errors
    ///
    /// As [`account_pins`](Self::account_pins).
    fn all_pins(&self) -> io::Result<Vec<(String, AccountPins)>> {
        self.factor_documents(FAMILY)?
            .into_iter()
            .map(|(account, text)| Ok((account, decode(&text)?)))
            .collect()
    }

    /// Applies `change` to what `account` holds as one step and answers the result. Pins are
    /// kept lowercase.
    ///
    /// # Errors
    ///
    /// `QuotaExceeded`, with nothing changed, when the result would hold more than `max` pins or
    /// more than `max` map entries; `InvalidInput` for an empty or control-character name;
    /// the store's own errors.
    fn change_pins(
        &self,
        account: &str,
        change: &PinChange,
        max: usize,
    ) -> io::Result<AccountPins> {
        let mut result = AccountPins::default();
        self.factor_update(FAMILY, account, &mut |held| {
            let mut now = held.map(decode).transpose()?.unwrap_or_default();
            let gone: Vec<String> = change
                .remove
                .iter()
                .map(|pin| pin.to_ascii_lowercase())
                .collect();
            now.pins.retain(|pin| !gone.contains(pin));
            let gone: Vec<PinMatch> = change.match_remove.iter().map(PinMatch::normal).collect();
            now.matches.retain(|entry| !gone.contains(entry));
            for pin in &change.add {
                let pin = pin.to_ascii_lowercase();
                if !now.pins.contains(&pin) {
                    now.pins.push(pin);
                }
            }
            for entry in change.match_add.iter().map(PinMatch::normal) {
                if !now.matches.contains(&entry) {
                    now.matches.push(entry);
                }
            }
            if now.pins.len() > max || now.matches.len() > max {
                return Err(io::Error::new(
                    io::ErrorKind::QuotaExceeded,
                    "the account would hold more pins than max_pins allows",
                ));
            }
            result = now.clone();
            if now == AccountPins::default() {
                return Ok(FactorChange::Remove);
            }
            Ok(FactorChange::Put(
                serde_json::to_string(&now).map_err(io::Error::other)?,
            ))
        })?;
        Ok(result)
    }
}

impl<T: FactorStore + ?Sized> PinStore for T {}

fn decode(text: &str) -> io::Result<AccountPins> {
    serde_json::from_str(text)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a pin document is unreadable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        pins_are_added_removed_and_capped,
        a_second_handle_reads_what_the_first_wrote
    );

    const A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "spki-sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn added(pin: &str) -> PinChange {
        PinChange {
            add: vec![pin.to_owned()],
            ..PinChange::default()
        }
    }

    fn pins_are_added_removed_and_capped(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(
            registry.account_pins("alice").unwrap(),
            AccountPins::default()
        );
        registry.change_pins("alice", &added(A), 2).unwrap();
        let held = registry
            .change_pins("alice", &added(&B.to_ascii_uppercase()), 2)
            .unwrap();
        assert_eq!(held.pins, [A, B], "kept lowercase");
        let again = registry.change_pins("alice", &added(A), 2).unwrap();
        assert_eq!(again, held, "a pin already held is not added twice");
        let third = format!("sha256:{}", "c".repeat(64));
        let refused = registry
            .change_pins("alice", &added(&third), 2)
            .unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::QuotaExceeded);
        assert_eq!(
            registry.account_pins("alice").unwrap(),
            held,
            "nothing changed"
        );
        // A removal in the same change makes room.
        let swapped = PinChange {
            add: vec![third.clone()],
            remove: vec![A.to_owned()],
            ..PinChange::default()
        };
        let held = registry.change_pins("alice", &swapped, 2).unwrap();
        assert_eq!(held.pins, [B.to_owned(), third]);
        assert_eq!(
            registry
                .change_pins("bob", &added(A), 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::QuotaExceeded,
            "no floor: a cap of none holds none"
        );
        let entry = PinMatch {
            field: "San.Email".to_owned(),
            value: "Alice@Example.Org".to_owned(),
        };
        let held = registry
            .change_pins(
                "alice",
                &PinChange {
                    match_add: vec![entry.clone()],
                    ..PinChange::default()
                },
                2,
            )
            .unwrap();
        assert_eq!(held.matches, [entry.normal()]);
        assert_eq!(registry.all_pins().unwrap().len(), 1);
        registry
            .change_pins(
                "alice",
                &PinChange {
                    remove: held.pins.clone(),
                    match_remove: vec![entry],
                    ..PinChange::default()
                },
                2,
            )
            .unwrap();
        assert!(
            registry.all_pins().unwrap().is_empty(),
            "an empty account leaves no document"
        );
        assert!(registry.change_pins("", &added(A), 2).is_err());
    }

    fn a_second_handle_reads_what_the_first_wrote(store: &dyn Store) {
        store.handle().change_pins("bob", &added(A), 4).unwrap();
        assert_eq!(store.handle().account_pins("bob").unwrap().pins, [A]);
    }
}
