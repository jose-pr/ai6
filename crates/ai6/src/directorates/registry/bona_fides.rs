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
}

impl Origin {
    /// The name used in records and on the board: `operator`, `source`, `sleeper`,
    /// `proposal`, `registry_drop`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Source => "source",
            Self::Sleeper => "sleeper",
            Self::Proposal => "proposal",
            Self::RegistryDrop => "registry_drop",
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
    /// `sha256:HEX` of the brief's title and body as accepted ([`brief_digest`]).
    pub digest: String,
}

impl Provenance {
    /// Provenance for `brief` accepted from `origin` by `received_by` at `received_at`
    /// (Unix seconds). The digest is computed from the brief's title and body.
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
            digest: brief_digest(&brief.title, &brief.body),
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

    /// Whether this provenance was recorded for `brief` as it is now: its title and body
    /// still hash to [`Provenance::digest`].
    pub fn matches(&self, brief: &Brief) -> bool {
        self.digest == brief_digest(&brief.title, &brief.body)
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

/// `sha256:HEX` over the length-framed title and body, so no two (title, body) pairs share a
/// digest by moving text across the boundary.
pub fn brief_digest(title: &str, body: &str) -> String {
    let mut digest = Sha256::new();
    for part in [title.as_bytes(), body.as_bytes()] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    format!("sha256:{}", hex_digest(&digest.finalize()))
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
/// kept beside it.
pub(super) fn seal(brief: &Brief) -> io::Result<(Brief, Option<Provenance>)> {
    let mut record = brief.clone();
    let provenance = record.provenance.take();
    if let Some(provenance) = &provenance {
        provenance.validate()?;
    }
    Ok((record, provenance))
}

/// Joins a stored brief with the provenance kept beside it, dropping any provenance the
/// record itself carried and any kept one that does not match the brief's text.
pub(super) fn unseal(brief: &mut Brief, kept: Option<Provenance>) {
    brief.provenance =
        kept.filter(|provenance| provenance.validate().is_ok() && provenance.matches(brief));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests, brief};

    backend_tests!(
        provenance_is_kept_apart_from_the_brief_through_the_queue,
        a_dropped_brief_never_carries_the_provenance_it_claims,
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

    #[test]
    fn the_digest_frames_title_and_body() {
        assert_ne!(brief_digest("ab", "c"), brief_digest("a", "bc"));
        assert!(is_digest(&brief_digest("t", "b")));
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
    fn origins_have_stable_names() {
        assert_eq!(
            serde_json::to_value(Origin::RegistryDrop).unwrap(),
            "registry_drop"
        );
        assert_eq!(Origin::Operator.to_string(), "operator");
    }
}
