//! `grant`: let a peer this user paired with open a protocol, or no longer.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

use agentic_peer::mcp_offer;
use cabuya::forward::{self, Target};
use cabuya::mux;
use cabuya::peers::{FilePeers, PeerRecord, PeerStore};

use std::path::Path;

use crate::common::grants_of;
use crate::serve::ACP;

/// Gives `peer` the protocol, or with `revoke` takes it away.
///
/// # Errors
///
/// The protocol or target is malformed, or the grants could not be written.
pub fn run(home: Option<&Path>, peer: &str, protocol: &str, revoke: bool) -> Result<u8, String> {
    // `PairedGrants` gives these to whoever is paired, so taking them away here would say it worked and change nothing.
    if revoke && (protocol == ACP || protocol == mcp_offer::OFFER_PROTOCOL) {
        return Err(format!(
            "{protocol} comes with being paired, so this cannot take it away: remove the peer from the trust store instead"
        ));
    }
    let target = match protocol.strip_prefix("tcp/1:") {
        Some(target) => Some(
            target
                .parse::<Target>()
                .map_err(|error| error.to_string())?,
        ),
        None if mux::valid_protocol(protocol) => None,
        None => {
            return Err(
                "a protocol token is printable, with no spaces, commas or equals signs".to_owned(),
            );
        }
    };
    let store = FilePeers::new(grants_of(home)?);
    let mut record = store
        .get(peer)
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| PeerRecord::new(peer));
    match (&target, revoke) {
        (Some(target), false) => record = forward::grant(record, target),
        (Some(target), true) => {
            record.grants.remove(&target.grant_token());
        }
        (None, false) => {
            record.grants.insert(protocol.to_owned());
        }
        (None, true) => {
            record.grants.remove(protocol);
            // Otherwise a later grant of one target would bring back all the old ones.
            if protocol == forward::TCP_PROTOCOL {
                record
                    .grants
                    .retain(|grant| Target::from_grant_token(grant).is_none());
            }
        }
    }
    store.put(&record).map_err(|error| error.to_string())?;
    Ok(0)
}
