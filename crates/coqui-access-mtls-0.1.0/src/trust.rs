//! Which client certificates are trusted, and which account a CA-trusted one names.

use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{Bytes, FromValue, Schema, ToValue};
use coqui_access::{Lookup, Verdict, error};
use coqui_access_kit::pick::{Pick, Picked, pick};
use coqui_peer_tls::{CertificateFields, PeerError, Policy, Trust};

/// How a client certificate is trusted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, rename_all = "kebab-case")]
pub enum TrustMode {
    /// Its fingerprint is a record of the table: a pinned peer.
    #[default]
    Pinned,
    /// A CA of the configured `ca_file` signed it, and its names match the globs.
    Ca,
    /// Pinned, or else signed by the CA.
    PinnedOrCa,
}

impl TrustMode {
    /// Whether the table's records are consulted.
    pub(crate) fn pins(self) -> bool {
        matches!(self, Self::Pinned | Self::PinnedOrCa)
    }

    /// Whether the CA is consulted.
    pub(crate) fn uses_ca(self) -> bool {
        matches!(self, Self::Ca | Self::PinnedOrCa)
    }
}

/// The account a CA-trusted certificate names by default: its own fingerprint, looked up by
/// `certificate`.
pub fn default_account() -> Vec<Pick> {
    vec![Pick::new("fingerprint", "certificate")]
}

/// Checks a chain against the CA and picks the account it names; `None` when the CA does not
/// trust it.
pub(crate) fn by_ca(
    trust: &Trust,
    policy: &Policy,
    account: &[Pick],
    chain: &[Bytes],
) -> Result<Option<Verdict>, ProviderError> {
    let Some((leaf, rest)) = chain.split_first() else {
        return Ok(None);
    };
    let intermediates: Vec<&[u8]> = rest.iter().map(AsRef::as_ref).collect();
    match trust.verify_client_chain(policy, leaf.as_ref(), &intermediates) {
        Ok(()) => {}
        // The chain is refused: not trusted.
        Err(PeerError::Io(_)) => return Ok(None),
        // The trust could not be built (an unreadable CA file): could not find out.
        Err(other) => {
            return Err(error::internal(&format!(
                "client certificate authority: {other}"
            )));
        }
    }
    let Ok(fields) = CertificateFields::parse(leaf.as_ref()) else {
        return Ok(None);
    };
    Ok(Some(match pick(&fields.to_map(), account) {
        Picked::Found { by, value } => Verdict::Valid(Lookup { by, value }),
        Picked::Ambiguous => Verdict::Invalid,
        _ => Verdict::Unknown,
    }))
}
