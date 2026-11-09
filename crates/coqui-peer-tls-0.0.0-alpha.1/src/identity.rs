use core::fmt;
#[cfg(feature = "generate")]
use core::time::Duration;
use std::fs;
use std::io;
use std::path::Path;
#[cfg(feature = "generate")]
use std::time::{SystemTime, UNIX_EPOCH};

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::error::PeerError;
use alloc::sync::Arc;
use coqui_peer::PeerId;
use coqui_peer::file::{self, Access};
use coqui_peer::pem;

/// A verified peer: the certificate the other side proved it holds the key of.
///
/// Returned by [`dial`](crate::dial) and [`accept`](crate::accept) after the handshake.
#[derive(Clone, PartialEq, Eq)]
pub struct PeerIdentity {
    /// The certificate's fingerprint.
    pub fingerprint: PeerId,
    /// The certificate, DER encoded.
    pub certificate: Vec<u8>,
}

impl fmt::Debug for PeerIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("PeerIdentity")
            .field(&self.fingerprint)
            .finish()
    }
}

/// This side's certificate and its private key, with the intermediates that issued the
/// certificate when it has any.
///
/// A peer identity is one self-issued certificate ([`from_pem`](Identity::from_pem)). A server
/// a browser reaches under web PKI presents a CA-issued certificate with its intermediates
/// ([`from_pem_chain`](Identity::from_pem_chain)). The fingerprint is always the leaf
/// certificate's.
///
/// On disk it is one PEM file: the certificate block, then a PKCS#8 `PRIVATE KEY` block.
pub struct Identity {
    fingerprint: PeerId,
    certificate: CertificateDer<'static>,
    intermediates: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl fmt::Debug for Identity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Identity")
            .field(&self.fingerprint)
            .finish()
    }
}

impl Clone for Identity {
    fn clone(&self) -> Self {
        Self {
            fingerprint: self.fingerprint,
            certificate: self.certificate.clone(),
            intermediates: self.intermediates.clone(),
            key: self.key.clone_key(),
        }
    }
}

impl Identity {
    /// Reads one certificate and its matching private key from PEM text.
    ///
    /// # Errors
    ///
    /// [`PeerError::Certificate`] when either is missing, malformed or not a pair, or when more
    /// than one certificate is present.
    pub fn from_pem(text: &str) -> Result<Self, PeerError> {
        // A chain is refused: an identity is one self-issued certificate.
        if CertificateDer::pem_slice_iter(text.as_bytes()).count() != 1 {
            return Err(PeerError::Certificate);
        }
        Self::from_pem_chain(text)
    }

    /// Reads a certificate, the intermediates that issued it and the leaf's private key from
    /// PEM text: the leaf certificate first, then each issuer in order up to (not necessarily
    /// including) the root, and one private key in any block position.
    ///
    /// A configuration built from it presents the whole chain. The fingerprint is the leaf's.
    /// A peer that pins this identity refuses the chain unless its policy sets
    /// [`allow_chain_when_pinned`](crate::Policy::allow_chain_when_pinned). One certificate
    /// alone gives the same identity as [`from_pem`](Identity::from_pem).
    ///
    /// # Errors
    ///
    /// [`PeerError::Certificate`] when there is no certificate or no key, either is malformed,
    /// or the key is not the first certificate's.
    pub fn from_pem_chain(text: &str) -> Result<Self, PeerError> {
        let mut certificates = CertificateDer::pem_slice_iter(text.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PeerError::Certificate)?;
        if certificates.is_empty() {
            return Err(PeerError::Certificate);
        }
        let key =
            PrivateKeyDer::from_pem_slice(text.as_bytes()).map_err(|_| PeerError::Certificate)?;
        // rustls refuses a key whose public key is not the first certificate's. A key ring
        // cannot load (ML-DSA) is checked by aws-lc-rs when this build has it.
        let matches = |provider: rustls::crypto::CryptoProvider| {
            ServerConfig::builder_with_provider(Arc::new(provider))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .is_ok_and(|builder| {
                    builder
                        .with_no_client_auth()
                        .with_single_cert(certificates.clone(), key.clone_key())
                        .is_ok()
                })
        };
        let loaded = matches(rustls::crypto::ring::default_provider());
        #[cfg(feature = "aws-lc")]
        let loaded = loaded || matches(rustls::crypto::aws_lc_rs::default_provider());
        if !loaded {
            return Err(PeerError::Certificate);
        }
        let certificate = certificates.remove(0);
        Ok(Self {
            fingerprint: PeerId::of(certificate.as_ref()),
            certificate,
            intermediates: certificates,
            key,
        })
    }

    /// The identity as PEM text: the certificate, its intermediates in order, then the PKCS#8
    /// private key. Text with intermediates reads back with
    /// [`from_pem_chain`](Identity::from_pem_chain) only.
    ///
    /// # Errors
    ///
    /// [`PeerError::Certificate`] when the key is not PKCS#8.
    pub fn to_pem(&self) -> Result<String, PeerError> {
        let PrivateKeyDer::Pkcs8(key) = &self.key else {
            return Err(PeerError::Certificate);
        };
        let mut pem = pem::encode("CERTIFICATE", self.certificate.as_ref());
        for intermediate in &self.intermediates {
            pem.push_str(&pem::encode("CERTIFICATE", intermediate.as_ref()));
        }
        pem.push_str(&pem::encode("PRIVATE KEY", key.secret_pkcs8_der()));
        Ok(pem)
    }

    /// Reads the identity file at `path`; `None` when it is absent. The file holds the
    /// certificate, the intermediates that issued it, if any, and its key, as
    /// [`from_pem_chain`](Identity::from_pem_chain) reads them: what
    /// [`write_new`](Identity::write_new) writes for any identity.
    ///
    /// # Errors
    ///
    /// [`PeerError::Io`] when it cannot be read, [`PeerError::Certificate`] when invalid.
    pub fn read(path: &Path) -> Result<Option<Self>, PeerError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_pem_chain(&text).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(PeerError::Io(error)),
        }
    }

    /// Writes the identity file without replacing an existing one, readable by its owner
    /// alone (`0600` on Unix). Parent directories are created.
    ///
    /// # Errors
    ///
    /// [`PeerError::Io`] with kind `AlreadyExists` when `path` exists.
    pub fn write_new(&self, path: &Path) -> Result<(), PeerError> {
        path.parent().ok_or(PeerError::Path)?;
        let pem = self.to_pem()?;
        file::write_new(path, pem.as_bytes(), Access::OwnerOnly).map_err(PeerError::Io)
    }

    /// The certificate's fingerprint: what a peer pins.
    pub fn fingerprint(&self) -> PeerId {
        self.fingerprint
    }

    /// The certificate, DER encoded.
    pub fn certificate_der(&self) -> &[u8] {
        self.certificate.as_ref()
    }

    /// The intermediates presented after the certificate, DER encoded, in order; none for an
    /// identity read by [`from_pem`](Identity::from_pem) or generated.
    pub fn intermediates_der(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.intermediates.iter().map(AsRef::as_ref)
    }

    /// This identity as the value a remote peer sees after a handshake.
    pub fn peer_identity(&self) -> PeerIdentity {
        PeerIdentity {
            fingerprint: self.fingerprint,
            certificate: self.certificate.as_ref().to_vec(),
        }
    }

    /// The certificate followed by its intermediates, as a configuration presents them: for a
    /// host that builds a rustls or quinn configuration of its own.
    pub fn certificate_chain(&self) -> Vec<CertificateDer<'static>> {
        let mut chain = Vec::with_capacity(1 + self.intermediates.len());
        chain.push(self.certificate.clone());
        chain.extend(self.intermediates.iter().cloned());
        chain
    }

    /// A copy of the private key, DER encoded, for a host that builds a rustls or quinn
    /// configuration of its own. It is the secret of this identity: keep it in memory only.
    /// Its `Debug` shows no key material.
    pub fn private_key_der(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }
}

/// Generates a self-signed ECDSA P-256 identity without writing it: [`Identity::builder`], the
/// validity, then [`generate`](Self::generate).
///
/// With nothing set the certificate is valid for rcgen's default period (decades), as
/// [`Identity::generate`] makes it. Every generated certificate has the common name
/// `coqui-peer`, a DNS subjectAltName [`SERVER_NAME`](crate::SERVER_NAME), a
/// subjectKeyIdentifier and basicConstraints `CA:FALSE`. Peers pin it by fingerprint, so the
/// name is not checked. Times are whole seconds: both are truncated.
///
/// The end of the validity is one setting: [`not_after`](Self::not_after) and
/// [`valid_for`](Self::valid_for) each set it, and the last one called wins.
///
/// For WebTransport's `serverCertificateHashes`, which accepts only a certificate whose whole
/// validity period is at most 14 days, use `valid_for(Duration::from_secs(14 * 86_400))` at most
/// and replace the identity before it expires; a peer whose clock runs behind sees it as not yet
/// valid, so an earlier `not_before`, within the same 14 days, covers that.
#[cfg(feature = "generate")]
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct IdentityBuilder {
    not_before: Option<SystemTime>,
    end: Option<End>,
}

/// Where the validity ends.
#[cfg(feature = "generate")]
#[derive(Debug, Clone, Copy)]
enum End {
    At(SystemTime),
    After(Duration),
}

#[cfg(feature = "generate")]
impl IdentityBuilder {
    /// The first instant the certificate is valid. Default rcgen's.
    pub fn not_before(mut self, at: SystemTime) -> Self {
        self.not_before = Some(at);
        self
    }

    /// The last instant the certificate is valid. Replaces [`valid_for`](Self::valid_for).
    pub fn not_after(mut self, at: SystemTime) -> Self {
        self.end = Some(End::At(at));
        self
    }

    /// How long the certificate is valid from `not_before`, from now when that is not set.
    /// Replaces [`not_after`](Self::not_after).
    pub fn valid_for(mut self, validity: Duration) -> Self {
        self.end = Some(End::After(validity));
        self
    }

    /// Generates the identity.
    ///
    /// # Errors
    ///
    /// [`PeerError::Certificate`] when `valid_for` is under one second, when `not_after` is not
    /// at least one second after `not_before`, when either is before 1970 or out of range, or
    /// when key generation fails.
    pub fn generate(self) -> Result<Identity, PeerError> {
        let seconds = |at: SystemTime| -> Result<time::OffsetDateTime, PeerError> {
            let since = at
                .duration_since(UNIX_EPOCH)
                .map_err(|_| PeerError::Certificate)?;
            let since = i64::try_from(since.as_secs()).map_err(|_| PeerError::Certificate)?;
            time::OffsetDateTime::from_unix_timestamp(since).map_err(|_| PeerError::Certificate)
        };
        let (not_before, not_after) = match self.end {
            Some(End::After(validity)) => {
                if validity < Duration::from_secs(1) {
                    return Err(PeerError::Certificate);
                }
                let start = self.not_before.unwrap_or_else(SystemTime::now);
                let end = start.checked_add(validity).ok_or(PeerError::Certificate)?;
                (Some(start), Some(end))
            }
            Some(End::At(end)) => (self.not_before, Some(end)),
            None => (self.not_before, None),
        };
        let mut parameters = Identity::parameters()?;
        if let Some(at) = not_before {
            parameters.not_before = seconds(at)?;
        }
        if let Some(at) = not_after {
            parameters.not_after = seconds(at)?;
        }
        if not_before.is_some()
            && not_after.is_some()
            && parameters.not_after <= parameters.not_before
        {
            return Err(PeerError::Certificate);
        }
        Identity::generate_from(parameters)
    }
}

#[cfg(feature = "generate")]
impl Identity {
    /// Creates a self-signed ECDSA P-256 identity without writing it, valid for rcgen's
    /// default period (decades).
    ///
    /// Every generated certificate has the common name `coqui-peer`, a DNS subjectAltName
    /// [`SERVER_NAME`](crate::SERVER_NAME), a subjectKeyIdentifier and basicConstraints
    /// `CA:FALSE`. Peers pin it by fingerprint, so the name is not checked.
    ///
    /// # Errors
    ///
    /// [`PeerError::Certificate`] when key generation fails.
    pub fn generate() -> Result<Self, PeerError> {
        Self::generate_from(Self::parameters()?)
    }

    /// A builder for a self-signed identity with a validity window of your own; see
    /// [`IdentityBuilder`]. [`generate`](Identity::generate) is the same with the defaults.
    pub fn builder() -> IdentityBuilder {
        IdentityBuilder::default()
    }

    /// rcgen writes the subjectKeyIdentifier only beside basicConstraints, hence
    /// `ExplicitNoCa`. Chrome's `serverCertificateHashes` verifier refuses a certificate
    /// with no extensions.
    fn parameters() -> Result<rcgen::CertificateParams, PeerError> {
        let mut parameters = rcgen::CertificateParams::new(vec![crate::SERVER_NAME.to_owned()])
            .map_err(|_| PeerError::Certificate)?;
        parameters
            .distinguished_name
            .push(rcgen::DnType::CommonName, "coqui-peer");
        parameters.is_ca = rcgen::IsCa::ExplicitNoCa;
        Ok(parameters)
    }

    fn generate_from(parameters: rcgen::CertificateParams) -> Result<Self, PeerError> {
        let key = rcgen::KeyPair::generate().map_err(|_| PeerError::Certificate)?;
        let certificate = parameters
            .self_signed(&key)
            .map_err(|_| PeerError::Certificate)?;
        Self::from_pem(&format!("{}{}", certificate.pem(), key.serialize_pem()))
    }

    /// Reads the identity file at `path`, creating and writing a new identity when absent.
    /// Two processes racing to create it end with the same identity.
    ///
    /// # Errors
    ///
    /// As [`read`](Identity::read) and [`write_new`](Identity::write_new).
    pub fn open_or_create(path: &Path) -> Result<Self, PeerError> {
        if let Some(identity) = Self::read(path)? {
            return Ok(identity);
        }
        let identity = Self::generate()?;
        match identity.write_new(path) {
            Ok(()) => Ok(identity),
            Err(PeerError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                Self::read(path)?.ok_or(PeerError::Certificate)
            }
            Err(error) => Err(error),
        }
    }
}

/// Encodes DER as a PEM block with 64-character lines and LF line endings.
#[cfg(all(test, feature = "generate"))]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;

    fn window(identity: &Identity) -> (i64, i64) {
        crate::x509::validity(identity.certificate_der()).unwrap()
    }

    #[test]
    fn a_short_lived_identity_has_the_requested_window() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let identity = Identity::builder()
            .valid_for(Duration::from_secs(14 * DAY))
            .generate()
            .unwrap();
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let (not_before, not_after) = window(&identity);
        let not_before = u64::try_from(not_before).unwrap();
        assert!((before..=after).contains(&not_before));
        assert_eq!(u64::try_from(not_after).unwrap() - not_before, 14 * DAY);
        assert_eq!(identity.intermediates_der().len(), 0);
    }

    #[test]
    fn an_explicit_window_is_kept_to_the_second() {
        let start = UNIX_EPOCH + Duration::from_secs(1_800_000_000) + Duration::from_millis(700);
        let end = start + Duration::from_secs(3 * DAY);
        let identity = Identity::builder()
            .not_before(start)
            .not_after(end)
            .generate()
            .unwrap();
        assert_eq!(
            window(&identity),
            (1_800_000_000, 1_800_000_000 + 3 * 86_400)
        );
    }

    /// Chrome's `serverCertificateHashes` verifier refuses a certificate with no extensions.
    #[test]
    fn generated_identities_carry_a_server_name_and_a_key_identifier() {
        use x509_parser::extensions::{GeneralName, ParsedExtension};

        let start = SystemTime::now();
        for identity in [
            Identity::generate().unwrap(),
            Identity::builder()
                .valid_for(Duration::from_secs(14 * DAY))
                .generate()
                .unwrap(),
            Identity::builder()
                .not_before(start)
                .not_after(start + Duration::from_secs(DAY))
                .generate()
                .unwrap(),
        ] {
            let (_, certificate) =
                x509_parser::parse_x509_certificate(identity.certificate_der()).unwrap();
            let names: Vec<_> = certificate
                .subject_alternative_name()
                .unwrap()
                .expect("a subjectAltName")
                .value
                .general_names
                .clone();
            assert_eq!(names, vec![GeneralName::DNSName(crate::SERVER_NAME)]);
            let key_identifier = certificate
                .extensions()
                .iter()
                .find_map(|extension| match extension.parsed_extension() {
                    ParsedExtension::SubjectKeyIdentifier(id) => Some(id.0.to_vec()),
                    _ => None,
                })
                .expect("a subjectKeyIdentifier");
            assert!(!key_identifier.is_empty());
            let constraints = certificate.basic_constraints().unwrap().unwrap();
            assert!(!constraints.value.ca);
            // The fingerprint names the certificate as made, extensions included.
            assert_eq!(
                identity.fingerprint(),
                PeerId::of(identity.certificate_der())
            );
        }
    }

    #[test]
    fn an_empty_or_inverted_window_is_refused() {
        assert!(matches!(
            Identity::builder().valid_for(Duration::ZERO).generate(),
            Err(PeerError::Certificate)
        ));
        assert!(matches!(
            Identity::builder()
                .valid_for(Duration::from_millis(999))
                .generate(),
            Err(PeerError::Certificate)
        ));
        let now = SystemTime::now();
        assert!(matches!(
            Identity::builder()
                .not_before(now)
                .not_after(now - Duration::from_secs(60))
                .generate(),
            Err(PeerError::Certificate)
        ));
    }
}
