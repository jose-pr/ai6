//! Bona fides: where a brief came from, and proof that the code a Station loads is genuine.
//!
//! A [`Provenance`] records who asked for a Mission. AI6 sets it at the point of entry from
//! what it authenticated there (the gateway's person, the Source provider and item, the
//! Sleeper, the proposing Agent and Session) and the Registry keeps it apart from the brief
//! record, so a `provenance` field written into a brief file or row is never read. A brief
//! the Registry holds no provenance for was dropped into the queue by hand; the Station that
//! takes it records it as [`Origin::RegistryDrop`]. Anything that can write the Registry's own
//! records can still forge them: the queue is the integration point this distinguishes.

use std::io;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::store::{Brief, MissionProposal, hex_digest};

/// Who a brief came from.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A person, through the Universal Exports gateway.
    Operator,
    /// An item at a configured [`Source`](super::sources::Source).
    Source,
    /// An authorized Sleeper's standing order.
    Sleeper,
    /// An Agent's Mission proposal that was approved.
    Proposal,
    /// A brief found in the queue with no provenance: written there by hand or by an
    /// integration that did not go through AI6.
    RegistryDrop,
    /// A Chief of Staff review Headquarters queued for a brief or proposal.
    Review,
}

impl Origin {
    /// The name used in records and on the board: `operator`, `source`, `sleeper`,
    /// `proposal`, `registry_drop`, `review`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Source => "source",
            Self::Sleeper => "sleeper",
            Self::Proposal => "proposal",
            Self::RegistryDrop => "registry_drop",
            Self::Review => "review",
        }
    }
}

impl core::fmt::Display for Origin {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where a brief came from, recorded by AI6 where the brief entered.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// What kind of entry point accepted the brief.
    pub origin: Origin,
    /// Who asked, as AI6 authenticated it: the person's ID, the Source provider, the
    /// Sleeper's name, the proposing Agent. `None` for a registry drop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    /// What identifies the request at its principal: the Source item, the Sleeper
    /// definition's fingerprint, the proposing Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The Source item's link, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_url: Option<String>,
    /// When the brief was accepted, as Unix seconds.
    pub received_at: i64,
    /// The Station, or `universal-exports` for the gateway, that accepted it.
    pub received_by: String,
    /// `sha256:HEX` of the whole brief record as stored ([`record_digest`]): any change to
    /// it after it was accepted, its Operation, assignment, data or text, drops the
    /// provenance.
    pub digest: String,
}

impl Provenance {
    /// Provenance for `brief` accepted from `origin` by `received_by` at `received_at`
    /// (Unix seconds). The digest is the brief's [`record_digest`]; the Registry takes it
    /// again from the record it stores.
    pub fn new(
        origin: Origin,
        brief: &Brief,
        received_by: impl Into<String>,
        received_at: i64,
    ) -> Self {
        Self {
            origin,
            principal: None,
            reference: None,
            item_url: None,
            received_at,
            received_by: received_by.into(),
            digest: record_digest(brief),
        }
    }

    /// Records who asked.
    #[must_use]
    pub fn with_principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = Some(principal.into());
        self
    }

    /// Records what identifies the request at its principal.
    #[must_use]
    pub fn with_reference(mut self, reference: impl Into<String>) -> Self {
        self.reference = Some(reference.into());
        self
    }

    /// Records the Source item's link.
    #[must_use]
    pub fn with_item_url(mut self, url: Option<String>) -> Self {
        self.item_url = url;
        self
    }

    /// The provenance a Station gives a brief the Registry holds none for.
    pub fn registry_drop(brief: &Brief, station: impl Into<String>, at: i64) -> Self {
        Self::new(Origin::RegistryDrop, brief, station, at)
    }

    /// Whether this provenance was recorded for `brief` as it is now: its record still
    /// hashes to [`Provenance::digest`].
    pub fn matches(&self, brief: &Brief) -> bool {
        self.digest == record_digest(brief)
    }

    pub(super) fn validate(&self) -> io::Result<()> {
        let bounded = |text: &str| text.len() <= 2_048 && !text.chars().any(char::is_control);
        if self.received_by.is_empty()
            || !bounded(&self.received_by)
            || [&self.principal, &self.reference, &self.item_url]
                .into_iter()
                .flatten()
                .any(|text| !bounded(text))
            || !is_digest(&self.digest)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "brief provenance is invalid",
            ));
        }
        Ok(())
    }
}

/// `sha256:HEX` of the brief record without its provenance, as canonical JSON: every field,
/// object keys sorted at every depth, no whitespace.
pub fn record_digest(brief: &Brief) -> String {
    let record = Brief {
        provenance: None,
        ..brief.clone()
    };
    let canonical = serde_json::to_value(&record)
        .map(canonical)
        .and_then(|value| serde_json::to_vec(&value))
        .unwrap_or_default();
    format!("sha256:{}", hex_digest(&Sha256::digest(canonical)))
}

/// `value` with its object keys sorted at every depth, whatever order the map kept.
fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: std::collections::BTreeMap<String, serde_json::Value> = map
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonical).collect())
        }
        other => other,
    }
}

fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// The current time as Unix seconds, `0` before the epoch.
pub(super) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// The provenance of an approved proposal: the proposing Agent and its Session.
pub(super) fn proposal_provenance(proposal: &MissionProposal, received_by: &str) -> Provenance {
    Provenance::new(Origin::Proposal, &proposal.brief, received_by, now_secs())
        .with_principal(proposal.proposer_agent_id.clone())
        .with_reference(proposal.proposer_session_id.to_string())
}

/// Splits what the Registry stores: the brief record without provenance, and the provenance
/// kept beside it, its digest taken from that record.
pub(super) fn seal(brief: &Brief) -> io::Result<(Brief, Option<Provenance>)> {
    let mut record = brief.clone();
    let mut provenance = record.provenance.take();
    if let Some(provenance) = &mut provenance {
        provenance.digest = record_digest(&record);
        provenance.validate()?;
    }
    Ok((record, provenance))
}

/// Joins a stored brief with the provenance kept beside it, dropping any provenance the
/// record itself carried and any kept one that does not match the record.
pub(super) fn unseal(brief: &mut Brief, kept: Option<Provenance>) {
    brief.provenance =
        kept.filter(|provenance| provenance.validate().is_ok() && provenance.matches(brief));
}

/// How a Station treats the provider libraries it finds on its plugin path.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginProfile {
    /// Only a library a pin or publisher accepts loads, and from a copy the Station verified.
    Strict,
    /// Every library loads; each one no pin accepts is reported as a weakening.
    Report,
    /// Libraries load unchecked.
    Off,
}

impl PluginProfile {
    /// `strict`, `report` or `off`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Report => "report",
            Self::Off => "off",
        }
    }

    /// The profile a Station uses when none is named: `strict` with a pin set, `report`
    /// without one, so a Station that never pinned anything loads what it did before.
    pub const fn default_for(pins: bool) -> Self {
        if pins { Self::Strict } else { Self::Report }
    }
}

impl core::str::FromStr for PluginProfile {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "strict" => Ok(Self::Strict),
            "report" => Ok(Self::Report),
            "off" => Ok(Self::Off),
            other => Err(format!(
                "plugin profile {other:?} is not strict, report or off"
            )),
        }
    }
}

/// Checks a publisher's Ed25519 signature: `(key, message, signature)`, true when it verifies.
pub type SignatureCheck<'a> = dyn Fn(&PublisherKey, &[u8], &[u8]) -> bool + 'a;

/// An Ed25519 publisher key, written `ed25519:` and 64 hexadecimal digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublisherKey([u8; 32]);

impl PublisherKey {
    /// The key from its 32 bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32 key bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl core::str::FromStr for PublisherKey {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || "a publisher is \"ed25519:\" and 64 hexadecimal digits".to_owned();
        let hex = text.trim().strip_prefix("ed25519:").ok_or_else(invalid)?;
        if hex.len() != 64 {
            return Err(invalid());
        }
        let mut key = [0u8; 32];
        for (slot, pair) in key.iter_mut().zip(hex.as_bytes().chunks(2)) {
            let pair = core::str::from_utf8(pair).map_err(|_| invalid())?;
            *slot = u8::from_str_radix(pair, 16).map_err(|_| invalid())?;
        }
        Ok(Self(key))
    }
}

impl core::fmt::Display for PublisherKey {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "ed25519:{}", hex_digest(&self.0))
    }
}

impl core::fmt::Debug for PublisherKey {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "PublisherKey({self})")
    }
}

impl Serialize for PublisherKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PublisherKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// What an entry needs before its library loads.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Require {
    /// The pin or the publisher's signature: either suffices.
    #[default]
    Either,
    /// The pin and the publisher's signature.
    Both,
}

/// One library a Station accepts: by its digest, by its publisher's signature, or both.
///
/// A publisher signs [`publisher_message`] for the library's file name and SHA-256 with
/// Ed25519 and ships the 64-byte signature beside it as `FILE.sig`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryPin {
    /// The library's file name as it sits on the plugin path (`ai6_claude.dll`).
    pub file: String,
    /// The digest its bytes must have (`sha256:HEX`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<coqui_peer::PeerId>,
    /// The key whose signature it must carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<PublisherKey>,
    /// Whether the pin and the publisher are each enough, or both are needed.
    #[serde(default)]
    pub require: Require,
}

impl LibraryPin {
    /// Accepts the library `file` whose bytes have the digest `pin`.
    pub fn digest(file: impl Into<String>, pin: coqui_peer::PeerId) -> Self {
        Self {
            file: file.into(),
            pin: Some(pin),
            publisher: None,
            require: Require::Either,
        }
    }

    /// Accepts the library `file` that `publisher` signed.
    pub fn publisher(file: impl Into<String>, publisher: PublisherKey) -> Self {
        Self {
            file: file.into(),
            pin: None,
            publisher: Some(publisher),
            require: Require::Either,
        }
    }

    /// Adds a digest the library may match.
    #[must_use]
    pub fn with_pin(mut self, pin: coqui_peer::PeerId) -> Self {
        self.pin = Some(pin);
        self
    }

    /// Adds a publisher whose signature the library may carry.
    #[must_use]
    pub fn with_publisher(mut self, publisher: PublisherKey) -> Self {
        self.publisher = Some(publisher);
        self
    }

    /// Needs both the pin and the signature.
    #[must_use]
    pub fn requiring_both(mut self) -> Self {
        self.require = Require::Both;
        self
    }

    fn validate(&self) -> Result<(), String> {
        if self.file.is_empty()
            || self.file.contains(['/', '\\'])
            || self.file.chars().any(char::is_control)
        {
            return Err(format!(
                "library file {:?} must be a file name, not a path",
                self.file
            ));
        }
        match (&self.pin, &self.publisher, self.require) {
            (None, None, _) => Err(format!("library {} names no pin or publisher", self.file)),
            (Some(_), Some(_), _) | (_, _, Require::Either) => Ok(()),
            _ => Err(format!(
                "library {} requires both a pin and a publisher",
                self.file
            )),
        }
    }

    /// Whether `bytes`, with the detached `signature` shipped beside them, satisfy this
    /// entry; `Err` says why not. `verify` checks an Ed25519 signature over a message.
    fn accepts(
        &self,
        bytes: &[u8],
        digest: &[u8; 32],
        signature: Option<&[u8]>,
        verify: &SignatureCheck<'_>,
    ) -> Result<String, String> {
        let pinned = self.pin.map(|pin| {
            let found = pin.digest().of(bytes);
            if found == pin {
                Ok(format!("pin {pin}"))
            } else {
                Err(format!("expected {pin}, found {found}"))
            }
        });
        let signed = self.publisher.map(|publisher| match signature {
            None => Err(format!(
                "publisher {publisher}: no signature file {}.sig",
                self.file
            )),
            Some(signature)
                if verify(
                    &publisher,
                    &publisher_message(&self.file, digest),
                    signature,
                ) =>
            {
                Ok(format!("publisher {publisher}"))
            }
            Some(_) => Err(format!(
                "publisher {publisher}: the signature does not verify"
            )),
        });
        let outcomes: Vec<Result<String, String>> =
            [pinned, signed].into_iter().flatten().collect();
        let accepted: Vec<&String> = outcomes.iter().filter_map(|o| o.as_ref().ok()).collect();
        let refused: Vec<&String> = outcomes.iter().filter_map(|o| o.as_ref().err()).collect();
        let enough = match self.require {
            Require::Either => !accepted.is_empty(),
            Require::Both => refused.is_empty(),
        };
        if enough {
            Ok(accepted
                .into_iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" and "))
        } else {
            Err(refused
                .into_iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("; "))
        }
    }
}

/// What a pin set says about one library.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinCheck {
    /// An entry accepts it; the text names what held (`pin sha256:..`, `publisher ed25519:..`).
    Accepted(String),
    /// No entry names its file.
    Unpinned,
    /// Entries name it and none accepts it; the text says why, with expected and found
    /// digests or the publisher that did not sign it.
    Refused(String),
}

/// The provider libraries a Station accepts, and the profile it loads them under.
///
/// Built in code with [`PinSet::with_library`], or read from a `plugins.toml`:
///
/// ```toml
/// profile = "strict"          # optional: strict when a pin set exists, report otherwise
/// [[library]]
/// file = "ai6_claude.dll"
/// pin = "sha256:..."
/// [[library]]
/// file = "ai6_codex.dll"
/// publisher = "ed25519:..."   # FILE.sig beside it signs the library's SHA-256
/// ```
///
/// Removing an entry or its publisher is how a library or a key stops being accepted; there
/// is no key server and no revocation list.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinSet {
    /// The profile the pin set asks for; a Station's own setting overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<PluginProfile>,
    /// The accepted libraries.
    #[serde(default, rename = "library")]
    pub libraries: Vec<LibraryPin>,
}

impl PinSet {
    /// An empty pin set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks for `profile`.
    #[must_use]
    pub fn with_profile(mut self, profile: PluginProfile) -> Self {
        self.profile = Some(profile);
        self
    }

    /// Accepts one more library.
    #[must_use]
    pub fn with_library(mut self, library: LibraryPin) -> Self {
        self.libraries.push(library);
        self
    }

    /// Reads a `plugins.toml`.
    ///
    /// # Errors
    ///
    /// Returns the TOML error, or names the first entry with no pin or publisher, a path in
    /// place of a file name, or `require = "both"` without both.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let pins: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        pins.validate()?;
        Ok(pins)
    }

    /// Reads the pin set at `path`, `None` when there is no file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or is not a valid pin set.
    pub fn load(path: &std::path::Path) -> io::Result<Option<Self>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Self::from_toml(&text)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// Checks every entry.
    ///
    /// # Errors
    ///
    /// Names the first invalid entry.
    pub fn validate(&self) -> Result<(), String> {
        self.libraries.iter().try_for_each(LibraryPin::validate)
    }

    /// The pins that are weaker than a SHA-256 one: SHA-1 pins.
    pub fn weakenings(&self) -> Vec<String> {
        self.libraries
            .iter()
            .filter_map(|library| {
                let pin = library.pin?;
                (pin.digest() == coqui_peer::Digest::Sha1)
                    .then(|| format!("library {} is pinned by SHA-1 ({pin})", library.file))
            })
            .collect()
    }

    /// What the pin set says about the library `file` with these `bytes` and the detached
    /// `signature` found beside it. `verify(key, message, signature)` checks an Ed25519
    /// signature; the message a publisher signs is [`publisher_message`].
    pub fn check(
        &self,
        file: &str,
        bytes: &[u8],
        signature: Option<&[u8]>,
        verify: &SignatureCheck<'_>,
    ) -> PinCheck {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let mut reasons = Vec::new();
        for entry in self.libraries.iter().filter(|entry| entry.file == file) {
            match entry.accepts(bytes, &digest, signature, verify) {
                Ok(held) => return PinCheck::Accepted(held),
                Err(reason) => reasons.push(reason),
            }
        }
        if reasons.is_empty() {
            PinCheck::Unpinned
        } else {
            PinCheck::Refused(reasons.join("; "))
        }
    }
}

/// Tags the message a publisher signs, so a signature over it means nothing else.
pub const PUBLISHER_MESSAGE_TAG: &[u8] = b"ai6-provider-library-v1\0";

/// What a publisher signs for the library `file` whose SHA-256 is `digest`:
/// [`PUBLISHER_MESSAGE_TAG`], the file name, a NUL, then the 32 digest bytes. The name binds
/// the signature to the slot it was made for, so a library signed as one provider cannot be
/// renamed into another's; a file name holds no NUL, so the framing is unambiguous.
pub fn publisher_message(file: &str, digest: &[u8; 32]) -> Vec<u8> {
    let mut message =
        Vec::with_capacity(PUBLISHER_MESSAGE_TAG.len() + file.len() + 1 + digest.len());
    message.extend_from_slice(PUBLISHER_MESSAGE_TAG);
    message.extend_from_slice(file.as_bytes());
    message.push(0);
    message.extend_from_slice(digest);
    message
}

/// The `sha256:HEX` pin of a library's bytes, as [`LibraryPin::digest`] takes it.
pub fn library_pin(bytes: &[u8]) -> coqui_peer::PeerId {
    coqui_peer::PeerId::of(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests, brief};

    backend_tests!(
        provenance_is_kept_apart_from_the_brief_through_the_queue,
        a_dropped_brief_never_carries_the_provenance_it_claims,
        a_brief_edited_after_it_was_sealed_is_a_registry_drop,
        a_finished_brief_dropped_again_does_not_reuse_its_provenance,
    );

    fn operator(brief: &Brief) -> Provenance {
        Provenance::new(Origin::Operator, brief, "universal-exports", 1_792_922_400)
            .with_principal("person-m")
    }

    fn provenance_is_kept_apart_from_the_brief_through_the_queue(store: &dyn Store) {
        let registry = store.handle();
        let mut asked = brief("asked");
        asked.provenance = Some(operator(&asked));
        registry.submit_brief(&asked).unwrap();
        registry.submit_brief(&brief("unattributed")).unwrap();

        let queued = registry.list_new().unwrap();
        let find = |id: &str| queued.iter().find(|brief| brief.id == id).unwrap();
        assert_eq!(find("asked").provenance, asked.provenance);
        assert_eq!(find("unattributed").provenance, None);

        let claimed = registry.claim_new(Some(&asked.key())).unwrap().unwrap();
        assert_eq!(claimed.provenance, asked.provenance);
        assert_eq!(
            registry
                .read_claimed(&asked.key())
                .unwrap()
                .unwrap()
                .provenance,
            asked.provenance,
            "a claimed brief keeps it for the Station that runs it"
        );
    }

    fn a_dropped_brief_never_carries_the_provenance_it_claims(store: &dyn Store) {
        let registry = store.handle();
        let mut forged = brief("forged");
        forged.provenance = Some(operator(&forged));
        registry.drop_brief(&forged.key(), &serde_json::to_string(&forged).unwrap());

        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 1, "a drop is a valid way in");
        assert_eq!(
            queued[0].provenance, None,
            "its own provenance field is not read"
        );
        let claimed = registry.claim_new(None).unwrap().unwrap();
        assert_eq!(claimed.provenance, None);
    }

    fn a_brief_edited_after_it_was_sealed_is_a_registry_drop(store: &dyn Store) {
        let registry = store.handle();
        let mut asked = brief("asked");
        asked.operation = Some("G".to_owned());
        asked.provenance = Some(operator(&asked));
        registry.submit_brief(&asked).unwrap();
        assert!(registry.list_new().unwrap()[0].provenance.is_some());

        // The same text, aimed at another Operation and carrying exposure data of its own.
        let mut edited = Brief {
            provenance: None,
            ..asked.clone()
        };
        edited.operation = Some("H".to_owned());
        edited
            .data
            .insert("exposure".to_owned(), serde_json::json!([]));
        registry.drop_brief(&asked.key(), &serde_json::to_string(&edited).unwrap());
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].operation.as_deref(), Some("H"));
        assert_eq!(queued[0].provenance, None, "the edit drops the origin");
    }

    fn a_finished_brief_dropped_again_does_not_reuse_its_provenance(store: &dyn Store) {
        let registry = store.handle();
        let mut asked = brief("replayed");
        asked.provenance = Some(operator(&asked));
        registry.submit_brief(&asked).unwrap();
        let claimed = registry.claim_new(None).unwrap().unwrap();
        assert!(claimed.provenance.is_some());
        registry
            .finish(
                &claimed,
                &serde_json::json!({"state": "done"}),
                true,
                None,
                None,
            )
            .unwrap();

        let record = Brief {
            provenance: None,
            ..asked.clone()
        };
        registry.drop_brief(&asked.key(), &serde_json::to_string(&record).unwrap());
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 1, "the same record, queued again by hand");
        assert_eq!(
            queued[0].provenance, None,
            "a finished brief's origin is spent"
        );
    }

    #[test]
    fn the_record_digest_covers_every_field_and_ignores_key_order() {
        let base = brief("d");
        let digest = record_digest(&base);
        assert!(is_digest(&digest));
        for edit in [
            |brief: &mut Brief| brief.operation = Some("H".to_owned()),
            |brief: &mut Brief| brief.agent = Some("008".to_owned()),
            |brief: &mut Brief| brief.role = Some("r".to_owned()),
            |brief: &mut Brief| brief.stations = vec!["s".to_owned()],
            |brief: &mut Brief| brief.url = Some("https://example.invalid".to_owned()),
            |brief: &mut Brief| {
                brief
                    .data
                    .insert("exposure".to_owned(), serde_json::json!([]));
            },
            |brief: &mut Brief| brief.title.push('!'),
        ] {
            let mut edited = base.clone();
            edit(&mut edited);
            assert_ne!(record_digest(&edited), digest);
        }
        let mut with_data = base.clone();
        with_data
            .data
            .insert("x".to_owned(), serde_json::json!({"b": 1, "a": 2}));
        let reordered: Brief = serde_json::from_str(
            &serde_json::to_string(&with_data)
                .unwrap()
                .replace(r#"{"b":1,"a":2}"#, r#"{"a":2,"b":1}"#),
        )
        .unwrap();
        assert_eq!(record_digest(&reordered), record_digest(&with_data));
    }

    #[test]
    fn a_record_s_own_provenance_is_never_read_and_a_kept_one_must_match_the_text() {
        let mut forged = brief("forged");
        forged.provenance = Some(
            Provenance::new(Origin::Operator, &forged, "universal-exports", 1).with_principal("m"),
        );
        let kept = forged.provenance.clone();
        let mut dropped = forged.clone();
        unseal(&mut dropped, None);
        assert_eq!(
            dropped.provenance, None,
            "the record's own field is not trusted"
        );

        let mut genuine = forged.clone();
        unseal(&mut genuine, kept.clone());
        assert_eq!(genuine.provenance, kept);

        let mut edited = forged;
        edited.body.push_str(" and something else");
        unseal(&mut edited, kept);
        assert_eq!(edited.provenance, None, "edited text loses its provenance");
    }

    #[test]
    fn a_library_pin_is_the_sha256_text_of_its_bytes() {
        let bytes = b"provider library";
        assert_eq!(
            library_pin(bytes).to_string(),
            format!("sha256:{}", hex_digest(&Sha256::digest(bytes)))
        );
    }

    #[test]
    fn a_pin_set_reads_from_toml_and_refuses_entries_that_accept_nothing() {
        let key = format!("ed25519:{}", "ab".repeat(32));
        let pins = PinSet::from_toml(&format!(
            "profile = \"report\"\n[[library]]\nfile = \"a.so\"\npin = \"sha256:{}\"\n\
             [[library]]\nfile = \"b.so\"\npublisher = \"{key}\"\n\
             [[library]]\nfile = \"c.so\"\npin = \"sha1:{}\"\npublisher = \"{key}\"\nrequire = \"both\"\n",
            "0".repeat(64),
            "1".repeat(40),
        ))
        .unwrap();
        assert_eq!(pins.profile, Some(PluginProfile::Report));
        assert_eq!(pins.libraries.len(), 3);
        assert_eq!(pins.libraries[1].publisher.unwrap().to_string(), key);
        assert_eq!(pins.libraries[2].require, Require::Both);
        assert_eq!(pins.weakenings().len(), 1, "a SHA-1 pin is reported");

        for refused in [
            "[[library]]\nfile = \"a.so\"\n",
            "[[library]]\nfile = \"dir/a.so\"\npin = \"sha256:00\"\n",
            &format!("[[library]]\nfile = \"a.so\"\npublisher = \"{key}\"\nrequire = \"both\"\n"),
            "[[library]]\nfile = \"a.so\"\npublisher = \"ed25519:short\"\n",
            "[[library]]\nfile = \"a.so\"\npin = \"md5:00\"\n",
            "profile = \"lenient\"\n",
        ] {
            assert!(PinSet::from_toml(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_pin_set_accepts_by_pin_or_publisher_and_says_why_it_refuses() {
        let bytes = b"library bytes";
        let other = b"other bytes";
        let key = PublisherKey::from_bytes([7; 32]);
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        // A fake verifier: "good" signs the framed message for b.so or c.so, nothing else.
        let signed = |_: &PublisherKey, message: &[u8], signature: &[u8]| {
            signature == b"good"
                && (message == publisher_message("b.so", &digest)
                    || message == publisher_message("c.so", &digest))
        };
        let pins = PinSet::new()
            .with_library(LibraryPin::digest("a.so", library_pin(bytes)))
            .with_library(LibraryPin::publisher("b.so", key))
            .with_library(
                LibraryPin::digest("c.so", library_pin(bytes))
                    .with_publisher(key)
                    .requiring_both(),
            );
        assert!(matches!(
            pins.check("a.so", bytes, None, &signed),
            PinCheck::Accepted(_)
        ));
        assert_eq!(pins.check("z.so", bytes, None, &signed), PinCheck::Unpinned);
        let PinCheck::Refused(why) = pins.check("a.so", other, None, &signed) else {
            panic!("a changed library is refused");
        };
        assert!(why.contains(&format!(
            "expected {}, found {}",
            library_pin(bytes),
            library_pin(other)
        )));
        assert_eq!(
            pins.check("b.so", bytes, Some(b"good"), &signed),
            PinCheck::Accepted(format!("publisher {key}"))
        );
        assert!(matches!(
            pins.check("b.so", bytes, None, &signed),
            PinCheck::Refused(why) if why.contains("no signature file b.so.sig")
        ));
        assert!(matches!(
            pins.check("c.so", bytes, Some(b"bad"), &signed),
            PinCheck::Refused(why) if why.contains(&format!("publisher {key}"))
        ));
        assert!(matches!(
            pins.check("c.so", bytes, Some(b"good"), &signed),
            PinCheck::Accepted(held) if held.contains(" and ")
        ));
    }

    #[test]
    fn origins_have_stable_names() {
        assert_eq!(
            serde_json::to_value(Origin::RegistryDrop).unwrap(),
            "registry_drop"
        );
        assert_eq!(Origin::Operator.to_string(), "operator");
    }
}
