use core::net::SocketAddr;

use coqui_peer::{KeyMaterial, PeerId, TrustStore};

use crate::error::PeerError;

/// The method name under which certificates are kept in a [`TrustStore`].
pub const METHOD: &str = "tls";

/// A DER-encoded certificate as the trust store's key material: method `tls`, PEM label
/// `CERTIFICATE`.
///
/// # Errors
///
/// [`PeerError::Certificate`] when `der` is empty or larger than the store holds.
pub fn certificate_key(der: &[u8]) -> Result<KeyMaterial, PeerError> {
    KeyMaterial::new(METHOD, "CERTIFICATE", der).map_err(|_| PeerError::Certificate)
}

/// Certificate-shaped access to a [`TrustStore`].
pub trait TrustStoreExt {
    /// Accepts a certificate directly, without it having waited.
    ///
    /// # Errors
    ///
    /// [`PeerError::Certificate`] when `der` is too large, [`PeerError::AlreadyAccepted`],
    /// [`PeerError::Io`] on write failure.
    fn accept_certificate(&self, der: &[u8]) -> Result<PeerId, PeerError>;

    /// Records a turned-away peer's certificate as waiting. Call it only after the peer
    /// proved it holds the certificate's key, as the verifiers of this crate do.
    fn record_waiting_certificate(&self, der: &[u8], source: Option<SocketAddr>);
}

impl TrustStoreExt for TrustStore {
    fn accept_certificate(&self, der: &[u8]) -> Result<PeerId, PeerError> {
        Ok(self.accept_key(&certificate_key(der)?)?)
    }

    fn record_waiting_certificate(&self, der: &[u8], source: Option<SocketAddr>) {
        if let Ok(key) = certificate_key(der) {
            self.record_waiting(&key, source);
        }
    }
}
