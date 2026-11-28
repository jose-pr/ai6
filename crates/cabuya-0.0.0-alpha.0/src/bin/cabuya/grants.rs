//! `grant` and `revoke`: what a paired peer may open, in the grants file `serve` reads on every question.

use cabuya::forward::{Rule, TCP_PROTOCOL, grant_rule};
use cabuya::mux::valid_protocol;
use cabuya::peers::{FilePeers, PeerRecord, PeerStore as _};
use coqui_peer::{PeerId, TrustStore};

use crate::serve::record_of;

/// A protocol, or `tcp/1:RULE` for one target or a pattern of them, which also grants `tcp/1`.
fn check(token: &str) -> Result<(), String> {
    if let Some(rule) = token.strip_prefix(&format!("{TCP_PROTOCOL}:")) {
        return rule.parse::<Rule>().map(drop).map_err(|_| {
            format!("{token}: not HOST:PORT, a name, address or network, and a port or range")
        });
    }
    if valid_protocol(token) {
        Ok(())
    } else {
        Err(format!("{token}: not a protocol name"))
    }
}

/// `record` with `token` granted.
fn granting(record: PeerRecord, token: &str) -> PeerRecord {
    match Rule::from_grant_token(token) {
        Some(rule) => grant_rule(record, &rule),
        None => record.granting(token),
    }
}

/// Grants each of `tokens` to `peer`.
pub fn add(grants: &FilePeers, peer: PeerId, tokens: &[String]) -> Result<(), String> {
    tokens.iter().try_for_each(|token| check(token))?;
    let mut record = record_of(grants, &peer.to_string())?;
    for token in tokens {
        record = granting(record, token);
    }
    grants.put(&record).map_err(|error| error.to_string())
}

/// Takes each of `tokens` from `peer`, or with none takes the peer's record and its key: it cannot prove itself again.
pub fn revoke(
    grants: &FilePeers,
    served: &TrustStore,
    peer: PeerId,
    tokens: &[String],
) -> Result<(), String> {
    let name = peer.to_string();
    if tokens.is_empty() {
        grants.remove(&name).map_err(|error| error.to_string())?;
        served
            .remove(&peer)
            .map_err(|error| error.to_string())
            .map(drop)
    } else {
        let mut record = record_of(grants, &name)?;
        for token in tokens {
            record.grants.remove(token);
        }
        grants.put(&record).map_err(|error| error.to_string())
    }
}
