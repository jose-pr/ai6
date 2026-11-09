//! Keeping and checking secrets: bearer keys and passwords, as self-describing strings.
//!
//! These are plain functions and types for anyone writing a provider over their own data;
//! nothing here knows about tables or files.
//!
//! | Kept as | Written | For |
//! | --- | --- | --- |
//! | SHA-256 | `sha256:<64 lowercase hex>` | bearer keys of 256 random bits, which need no slow hash |
//! | Argon2id | `$argon2id$v=19$m=…,t=…,p=…$salt$hash` | passwords, or keys that must resist guessing |
//!
//! A stored string names its own algorithm, so one table may hold both.

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher as _, PasswordVerifier as _};
use argon2::{Algorithm, Argon2, Params, Version};
use coqui_access::Limit;
use coqui_access::guatiao::{FromValue, Kinds, Map, MapError, Schema, ToValue, Value};

use crate::secret;

/// The longest password hashed or verified unless [`PasswordHasher::max_password_bytes`] says
/// otherwise: 4 096 bytes. It bounds the work one attempt costs.
pub const DEFAULT_MAX_PASSWORD_BYTES: Limit = Limit::at(4096);

const SHA256_PREFIX: &str = "sha256:";
const ARGON2ID_PREFIX: &str = "$argon2id$";

/// The cost of an Argon2id hash. The default is the Argon2 crate's: 19 MiB, 2 passes, 1 lane.
///
/// As configuration it is a map of `memory_kib`, `iterations` and `parallelism`, each
/// defaulting to the library's value, so a config may give any of them or none. Reading it
/// checks the combination, as [`Argon2Params::new`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
pub struct Argon2Params {
    /// Memory in KiB.
    #[map(default = Params::DEFAULT_M_COST)]
    memory_kib: u32,
    /// Passes over the memory.
    #[map(default = Params::DEFAULT_T_COST)]
    iterations: u32,
    /// Lanes.
    #[map(default = Params::DEFAULT_P_COST)]
    parallelism: u32,
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            memory_kib: Params::DEFAULT_M_COST,
            iterations: Params::DEFAULT_T_COST,
            parallelism: Params::DEFAULT_P_COST,
        }
    }
}

const OUT_OF_RANGE: &str = "argon2 parameters are out of range";

/// Reads each field the derive's way and then checks the combination, so a config Argon2
/// would refuse fails as it is read.
impl FromValue for Argon2Params {
    const KINDS: Kinds = Kinds::MAP;

    fn from_value(value: &Value) -> Result<Self, MapError> {
        let map = <&Map>::try_from(value)?;
        let default = Self::default();
        let field = |key: &str, default: u32| match map.get(key) {
            Some(field) => u32::from_value(field).map_err(|error| error.under(key)),
            None => Ok(default),
        };
        Self::new(
            field("memory_kib", default.memory_kib)?,
            field("iterations", default.iterations)?,
            field("parallelism", default.parallelism)?,
        )
        .map_err(|_| MapError::bad_value(OUT_OF_RANGE))
    }
}

impl Argon2Params {
    /// Memory in KiB, passes over it and lanes.
    ///
    /// # Errors
    ///
    /// Returns an error when Argon2 refuses the combination: memory under 8 KiB per lane, or
    /// a zero count.
    pub fn new(memory_kib: u32, iterations: u32, parallelism: u32) -> Result<Self, String> {
        Params::new(memory_kib, iterations, parallelism, None)
            .map_err(|_| OUT_OF_RANGE.to_owned())?;
        Ok(Self {
            memory_kib,
            iterations,
            parallelism,
        })
    }

    /// Memory in KiB.
    pub fn memory_kib(&self) -> u32 {
        self.memory_kib
    }

    /// Passes over the memory.
    pub fn iterations(&self) -> u32 {
        self.iterations
    }

    /// Lanes.
    pub fn parallelism(&self) -> u32 {
        self.parallelism
    }

    fn hash(&self, secret: &[u8]) -> Result<String, String> {
        let params = Params::new(self.memory_kib, self.iterations, self.parallelism, None)
            .map_err(|_| OUT_OF_RANGE.to_owned())?;
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password(secret)
            .map(|hash| hash.to_string())
            .map_err(|_| "secret could not be hashed".to_owned())
    }
}

/// How a bearer key is kept.
///
/// As configuration it is a map tagged by `algorithm`, the costs beside it under Argon2id:
/// `{ "algorithm": "sha256" }` or `{ "algorithm": "argon2id", "memory_kib": 65536 }`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, tag = "algorithm", rename_all = "lowercase")]
#[non_exhaustive]
pub enum KeyHash {
    /// `sha256:<hex>`: fast, and enough for a key of 256 random bits. The default.
    #[default]
    Sha256,
    /// Argon2id with these costs: for a key a person might have chosen. Every presented key
    /// then costs one Argon2id verification per Argon2id record it is checked against.
    Argon2id(Argon2Params),
}

impl KeyHash {
    /// Argon2id with the default costs.
    pub fn argon2id() -> Self {
        Self::Argon2id(Argon2Params::default())
    }

    /// What to keep of `secret`, in the form [`verify_key`] reads.
    ///
    /// # Errors
    ///
    /// Returns an error when the secret cannot be hashed (Argon2id only).
    pub fn hash(&self, secret: &str) -> Result<String, String> {
        match self {
            Self::Sha256 => Ok(format!("{SHA256_PREFIX}{}", secret::hash(secret, None))),
            Self::Argon2id(params) => params.hash(secret.as_bytes()),
        }
    }
}

/// Whether `presented` is the secret `stored` was made from, whatever algorithm `stored`
/// names. A string that names no known algorithm is never a match.
///
/// The only branch is on the algorithm prefix of `stored`. Past it nothing returns early on
/// where the values differ: SHA-256 digests are compared in constant time and Argon2id does
/// its full work.
pub fn verify_key(stored: &str, presented: &[u8]) -> bool {
    if let Some(digest) = stored.strip_prefix(SHA256_PREFIX) {
        secret::same(digest, &secret::hash_bytes(presented))
    } else if stored.starts_with(ARGON2ID_PREFIX) {
        verify_argon2id(stored, presented)
    } else {
        false
    }
}

/// Whether `stored` is a string [`verify_key`] can check: a well-formed `sha256:` digest or
/// an Argon2id hash.
pub fn is_key_hash(stored: &str) -> bool {
    match stored.strip_prefix(SHA256_PREFIX) {
        Some(digest) => secret::is_hash(digest),
        None => is_password_hash(stored),
    }
}

/// Whether `stored` is a well-formed Argon2id hash, as [`PasswordHasher::hash`] writes it.
pub fn is_password_hash(stored: &str) -> bool {
    stored.starts_with(ARGON2ID_PREFIX) && PasswordHash::new(stored).is_ok()
}

/// Makes a key: `prefix` and 256 random bits. See [`secret::generate`].
///
/// # Errors
///
/// Returns an error when the operating system random source is unavailable.
pub fn generate_key(prefix: &str) -> Result<String, String> {
    secret::generate(prefix)
}

fn verify_argon2id(stored: &str, presented: &[u8]) -> bool {
    PasswordHash::new(stored)
        .ok()
        .is_some_and(|hash| Argon2::default().verify_password(presented, &hash).is_ok())
}

/// Whether `presented` is the password `stored` (an Argon2id hash) was made from. A password
/// over [`DEFAULT_MAX_PASSWORD_BYTES`] or a stored string that is not Argon2id never matches;
/// [`PasswordHasher::verify`] applies the hasher's own bound instead.
pub fn verify_password(stored: &str, presented: &str) -> bool {
    verify_within(stored, presented, DEFAULT_MAX_PASSWORD_BYTES)
}

fn verify_within(stored: &str, presented: &str, most: Limit) -> bool {
    most.allows(presented.len())
        && stored.starts_with(ARGON2ID_PREFIX)
        && verify_argon2id(stored, presented.as_bytes())
}

/// Hashes passwords with Argon2id and checks them, with a dummy hash for names nobody keeps.
///
/// ```
/// use coqui_access_kit::{Argon2Params, PasswordHasher};
///
/// let hasher = PasswordHasher::new(Argon2Params::new(64, 1, 1).unwrap()).unwrap();
/// let stored = hasher.hash("correct horse").unwrap();
/// assert!(hasher.verify(Some(&stored), "correct horse"));
/// assert!(!hasher.verify(Some(&stored), "wrong"));
/// // No such account: the same work is done, and the answer is no.
/// assert!(!hasher.verify(None, "correct horse"));
/// ```
#[derive(Debug, Clone)]
pub struct PasswordHasher {
    params: Argon2Params,
    dummy: String,
    max_password_bytes: Limit,
}

impl Default for PasswordHasher {
    fn default() -> Self {
        Self::new(Argon2Params::default()).expect("default parameters hash")
    }
}

impl PasswordHasher {
    /// A hasher with these costs.
    ///
    /// # Errors
    ///
    /// Returns an error when the dummy hash cannot be made.
    pub fn new(params: Argon2Params) -> Result<Self, String> {
        Ok(Self {
            params,
            dummy: params.hash(b"unknown account password")?,
            max_password_bytes: DEFAULT_MAX_PASSWORD_BYTES,
        })
    }

    /// This hasher refusing to hash, and never matching, a password longer than `most` bytes.
    /// Default [`DEFAULT_MAX_PASSWORD_BYTES`].
    #[must_use]
    pub fn max_password_bytes(mut self, most: Limit) -> Self {
        self.max_password_bytes = most;
        self
    }

    /// The costs new hashes are made with.
    pub fn params(&self) -> Argon2Params {
        self.params
    }

    /// Hashes a password with a fresh random salt.
    ///
    /// # Errors
    ///
    /// Returns an error when the password is empty or longer than
    /// [`max_password_bytes`](PasswordHasher::max_password_bytes), or hashing fails.
    pub fn hash(&self, password: &str) -> Result<String, String> {
        if password.is_empty() || !self.max_password_bytes.allows(password.len()) {
            return Err(format!(
                "password must not be empty, and {} bytes long",
                self.max_password_bytes
            ));
        }
        self.params.hash(password.as_bytes())
    }

    /// Checks `presented` against `stored`; with `None` (no such account) it checks against
    /// the dummy hash, so an unknown name costs what a wrong password does, and answers no.
    pub fn verify(&self, stored: Option<&str>, presented: &str) -> bool {
        let matched = verify_within(
            stored.unwrap_or(&self.dummy),
            presented,
            self.max_password_bytes,
        );
        stored.is_some() && matched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cheap() -> Argon2Params {
        Argon2Params::new(64, 1, 1).unwrap()
    }

    #[test]
    fn both_algorithms_verify_their_own_secret_and_nothing_else() {
        for how in [KeyHash::Sha256, KeyHash::Argon2id(cheap())] {
            let key = generate_key("ck_").unwrap();
            let stored = how.hash(&key).unwrap();
            assert!(is_key_hash(&stored), "{stored}");
            assert!(verify_key(&stored, key.as_bytes()));
            assert!(!verify_key(
                &stored,
                generate_key("ck_").unwrap().as_bytes()
            ));
            assert!(!verify_key(&stored, b""));
            assert!(!verify_key(&stored, &key.as_bytes()[1..]));
        }
    }

    #[test]
    fn a_table_can_mix_algorithms() {
        let key = |n: &str| format!("ck_{n}");
        let stored = [
            KeyHash::Sha256.hash(&key("one")).unwrap(),
            KeyHash::Argon2id(cheap()).hash(&key("two")).unwrap(),
            KeyHash::Sha256.hash(&key("three")).unwrap(),
        ];
        for (at, name) in ["one", "two", "three"].into_iter().enumerate() {
            let found: Vec<_> = stored
                .iter()
                .enumerate()
                .filter(|(_, kept)| verify_key(kept, key(name).as_bytes()))
                .map(|(index, _)| index)
                .collect();
            assert_eq!(found, [at]);
        }
        assert!(stored[0].starts_with("sha256:"));
        assert!(stored[1].starts_with("$argon2id$"));
    }

    #[test]
    fn an_unknown_or_malformed_stored_string_never_matches() {
        let digest = secret::hash("k", None);
        for stored in [
            "",
            "k",
            // A bare digest has no algorithm.
            digest.as_str(),
            "sha256:",
            "sha256:abc",
            "SHA256:00",
            "$argon2id$garbage",
            "$argon2i$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$aGFzaGhhc2g",
            "$2b$12$abcdefghijklmnopqrstuv",
        ] {
            assert!(!verify_key(stored, b"k"), "{stored}");
            assert!(!is_key_hash(stored), "{stored}");
        }
    }

    #[test]
    fn a_different_prefix_runs_the_whole_comparison_on_the_digest() {
        // The same digest under another prefix is not a match, and a digest of the wrong
        // length is compared (and refused) rather than skipped.
        let stored = KeyHash::Sha256.hash("k").unwrap();
        assert!(verify_key(&stored, b"k"));
        let bare = stored.trim_start_matches("sha256:");
        assert!(!verify_key(&format!("sha257:{bare}"), b"k"));
        assert!(!verify_key(&format!("sha256:{}", &bare[..63]), b"k"));
        assert!(!verify_key(&format!("sha256:{bare}00"), b"k"));
    }

    #[test]
    fn argon2_costs_are_checked_and_exposed() {
        assert_eq!(Argon2Params::default().memory_kib(), 19 * 1024);
        assert_eq!(Argon2Params::default().iterations(), 2);
        assert_eq!(Argon2Params::default().parallelism(), 1);
        assert!(Argon2Params::new(4, 1, 1).is_err());
        assert!(Argon2Params::new(64, 0, 1).is_err());
        let stored = KeyHash::Argon2id(Argon2Params::new(64, 2, 1).unwrap())
            .hash("k")
            .unwrap();
        assert!(stored.contains("m=64,t=2,p=1"));
    }

    fn costs_from(json: &str) -> Result<Argon2Params, MapError> {
        let value =
            guatiao_serde::from_serde(&mut serde_json::Deserializer::from_str(json)).unwrap();
        Argon2Params::from_value(&value)
    }

    #[test]
    fn argon2_costs_read_from_configuration_default_each_field_and_are_checked() {
        assert_eq!(costs_from("{}").unwrap(), Argon2Params::default());
        let set = costs_from(r#"{"memory_kib": 64, "iterations": 1}"#).unwrap();
        assert_eq!(set, Argon2Params::new(64, 1, 1).unwrap());
        assert_eq!(
            Argon2Params::from_value(&set.to_value().unwrap()).unwrap(),
            set
        );
        let refused = costs_from(r#"{"iterations": 0}"#).unwrap_err();
        assert_eq!(refused, MapError::bad_value(OUT_OF_RANGE));
        assert!(costs_from(r#"{"memory_kib": 4}"#).is_err());
        // Reading is lenient about keys it does not know; the schema a host validates with is not.
        assert_eq!(
            costs_from(r#"{"lanes": 1}"#).unwrap(),
            Argon2Params::default()
        );
        assert_eq!(
            costs_from(r#"{"iterations": "2"}"#).unwrap_err().key(),
            "iterations"
        );
        assert!(costs_from("[]").is_err());
    }

    #[test]
    fn argon2_costs_describe_their_defaults() {
        let schema = Argon2Params::schema().unwrap();
        let text = guatiao_serde::text::json::to_string(&schema, Default::default()).unwrap();
        assert!(text.contains("\"default\":19456"), "{text}");
    }

    #[test]
    fn a_key_hash_reads_from_configuration_tagged_by_algorithm() {
        let read = |json: &str| {
            let value =
                guatiao_serde::from_serde(&mut serde_json::Deserializer::from_str(json)).unwrap();
            KeyHash::from_value(&value)
        };
        assert_eq!(read(r#"{"algorithm": "sha256"}"#).unwrap(), KeyHash::Sha256);
        assert_eq!(
            read(r#"{"algorithm": "argon2id"}"#).unwrap(),
            KeyHash::argon2id()
        );
        assert_eq!(
            read(r#"{"algorithm": "argon2id", "memory_kib": 64, "iterations": 1}"#).unwrap(),
            KeyHash::Argon2id(cheap())
        );
        assert!(read(r#"{"algorithm": "argon2id", "iterations": 0}"#).is_err());
        assert!(read(r#"{"algorithm": "md5"}"#).is_err());
        for hash in [KeyHash::Sha256, KeyHash::Argon2id(cheap())] {
            assert_eq!(
                KeyHash::from_value(&hash.to_value().unwrap()).unwrap(),
                hash
            );
        }
    }

    #[test]
    fn passwords_are_hashed_bounded_and_checked_with_a_dummy_for_unknown_names() {
        let hasher = PasswordHasher::new(cheap()).unwrap();
        assert_eq!(hasher.params(), cheap());
        let stored = hasher.hash("pw").unwrap();
        assert_ne!(stored, hasher.hash("pw").unwrap(), "salted");
        assert!(hasher.verify(Some(&stored), "pw"));
        assert!(!hasher.verify(Some(&stored), "px"));
        assert!(!hasher.verify(None, "pw"));
        assert!(!hasher.verify(Some("not a hash"), "pw"));
        assert!(hasher.hash("").is_err());
        assert!(hasher.hash(&"x".repeat(4097)).is_err());
        let longer = hasher.clone().max_password_bytes(Limit::UNBOUNDED);
        let long = longer.hash(&"x".repeat(5000)).unwrap();
        assert!(longer.verify(Some(&long), &"x".repeat(5000)));
        assert!(
            !hasher.verify(Some(&long), &"x".repeat(5000)),
            "past this hasher's bound"
        );
        let shorter = hasher.clone().max_password_bytes(Limit::at(2));
        assert!(shorter.hash("abc").is_err() && shorter.hash("ab").is_ok());
        assert!(!verify_password(&stored, &"x".repeat(4097)));
        // A key digest is not a password hash.
        assert!(!verify_password(&KeyHash::Sha256.hash("pw").unwrap(), "pw"));
    }
}
