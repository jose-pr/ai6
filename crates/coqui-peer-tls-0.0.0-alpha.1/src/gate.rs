//! A policy's access control list, applied: by address when the `ClientHello` arrives, and by
//! the certificate's fingerprint before the trust store is consulted.

use alloc::sync::Arc;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use coqui_net_acl::{Acl, Outcome};
use coqui_peer::{Digest, PeerId};
use rustls::DigitallySignedStruct;
use rustls::DistinguishedName;
use rustls::SignatureScheme;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use crate::connection::Slot;
use crate::error::{PeerError, Refusal};

/// `acl` with every rule's fingerprint in canonical `sha256:<hex>` text, the form a verified
/// certificate is decided by.
///
/// # Errors
///
/// [`PeerError::Policy`] naming the first rule whose fingerprint is not a peer id, or is one
/// of another digest: a certificate is known by its SHA-256 id, so such a rule could never
/// match.
pub(crate) fn canonical(acl: &Acl) -> Result<Acl, PeerError> {
    let mut acl = acl.clone();
    acl.canonicalize_fingerprints(|index, text| match text.parse::<PeerId>() {
        Ok(id) if id.digest() == Digest::Sha256 => Ok(id.canonical()),
        Ok(_) => Err(PeerError::Policy(format!(
            "acl rule {index}: the fingerprint is not a SHA-256 peer id; a certificate is known \
             by its SHA-256 id"
        ))),
        Err(_) => Err(PeerError::Policy(format!(
            "acl rule {index}: the fingerprint is not a peer id"
        ))),
    })?;
    Ok(acl)
}

/// A canonical access control list, and the connection's addresses when they are known when
/// it is built (a configuration or verifier made for one connection); otherwise they are read
/// from the scope the accept helpers set (`with_addresses`). A refusal at the `ClientHello`
/// goes to `refused` when given, and to that scope otherwise.
#[derive(Debug, Clone)]
pub(crate) struct Gate {
    acl: Arc<Acl>,
    addresses: Option<(SocketAddr, SocketAddr)>,
    refused: Option<Arc<Slot>>,
}

impl Gate {
    pub(crate) fn new(
        acl: Arc<Acl>,
        addresses: Option<(SocketAddr, SocketAddr)>,
        refused: Option<Arc<Slot>>,
    ) -> Self {
        Self {
            acl,
            addresses,
            refused,
        }
    }

    /// Keeps `refusal` where this gate reports to.
    fn refuse(&self, refusal: Refusal) {
        match &self.refused {
            Some(slot) => slot.keep(refusal),
            None => crate::source::refuse(refusal),
        }
    }

    /// The connection's source and local addresses. A local address only rules with a
    /// destination or port read may be unknown: an unspecified one of the source's family
    /// stands in, which no such rule is consulted for.
    fn addresses(&self) -> Result<(SocketAddr, SocketAddr), Refusal> {
        if let Some(addresses) = self.addresses {
            return Ok(addresses);
        }
        let source = crate::source::current().ok_or(Refusal::AddressUnknown)?;
        match crate::source::local() {
            Some(local) => Ok((source, local)),
            None if self
                .acl
                .rules
                .iter()
                .all(|rule| rule.destination.is_none() && rule.port.is_none()) =>
            {
                let unspecified = match source.ip() {
                    IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                };
                Ok((source, SocketAddr::new(unspecified, 0)))
            }
            None => Err(Refusal::AddressUnknown),
        }
    }

    /// The decision from the addresses alone: for a client that will present a certificate
    /// (`identified`), a rule naming a fingerprint defers to [`check_peer`](Gate::check_peer);
    /// for one that will not, such rules cannot match it.
    pub(crate) fn check_address(&self, identified: bool) -> Result<(), Refusal> {
        let (source, local) = self.addresses()?;
        decide_address(&self.acl, source, local, identified)
    }

    /// The decision for the client whose certificate is `der`, from these addresses.
    pub(crate) fn check_peer(&self, der: &[u8]) -> Result<(), Refusal> {
        let (source, local) = self.addresses()?;
        let decision = self
            .acl
            .decide_peer(source, local, &PeerId::of(der).canonical());
        match decision.outcome {
            Outcome::Permit => Ok(()),
            _ => Err(Refusal::PeerDenied {
                rule: decision.rule_index,
            }),
        }
    }
}

/// The address decision of `acl` for a connection from `source` to `local`.
pub(crate) fn decide_address(
    acl: &Acl,
    source: SocketAddr,
    local: SocketAddr,
    identified: bool,
) -> Result<(), Refusal> {
    let decision = if identified {
        acl.decide(source, local)
    } else {
        acl.decide_anonymous(source, local)
    };
    match decision.outcome {
        Outcome::Deny => Err(Refusal::AddressDenied {
            rule: decision.rule_index,
        }),
        _ => Ok(()),
    }
}

/// A certificate resolver that answers only connections the list admits by address: a refused
/// one gets no certificate, so rustls ends the handshake with `access_denied` before any
/// certificate is sent or read.
#[derive(Debug)]
pub(crate) struct AddressGate {
    pub(crate) inner: Arc<dyn ResolvesServerCert>,
    pub(crate) gate: Gate,
    /// Whether every client must present a certificate. Rules naming a fingerprint defer to
    /// the certificate only then: under optional client authentication a client may send
    /// none, and the certificate check would never run.
    pub(crate) identified: bool,
}

impl ResolvesServerCert for AddressGate {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        match self.gate.check_address(self.identified) {
            Ok(()) => self.inner.resolve(client_hello),
            Err(refusal) => {
                self.gate.refuse(refusal);
                None
            }
        }
    }

    fn only_raw_public_keys(&self) -> bool {
        self.inner.only_raw_public_keys()
    }
}

/// A client verifier that refuses a certificate the list denies from these addresses before
/// `inner` sees it, so a trust store never records as waiting a client the list will not
/// admit.
#[derive(Debug)]
pub(crate) struct PeerGate {
    pub(crate) inner: Arc<dyn ClientCertVerifier>,
    pub(crate) gate: Gate,
}

impl ClientCertVerifier for PeerGate {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        certificate: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.gate
            .check_peer(certificate.as_ref())
            .map_err(Refusal::into_rustls)?;
        self.inner
            .verify_client_cert(certificate, intermediates, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner
            .verify_tls12_signature(message, certificate, signed)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner
            .verify_tls13_signature(message, certificate, signed)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }

    fn requires_raw_public_keys(&self) -> bool {
        self.inner.requires_raw_public_keys()
    }
}
