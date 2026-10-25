//! Station certificate identities and mutual TLS trust.

use std::borrow::Borrow;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use globset::{Glob, GlobSet, GlobSetBuilder};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig};
use sha2::{Digest, Sha256};

const IDENTITY_FILE: &str = "identity.pem";
const STATION_ALPN: &[u8] = b"ai6-station/1";
const MAX_CERTIFICATE_BYTES: usize = 64 * 1024;
const MAX_WAITING_PEERS: usize = 1_000;
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A SHA-256 certificate fingerprint, printed as grouped base32.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StationFingerprint(String);

impl FromStr for StationFingerprint {
    type Err = StationIdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let letters: String = value
            .chars()
            .filter(|character| !matches!(character, '-' | ' ' | '\t' | '\n' | '\r'))
            .map(|character| character.to_ascii_uppercase())
            .collect();
        if letters.len() != 52 || !letters.bytes().all(|byte| BASE32.contains(&byte)) {
            return Err(StationIdentityError::InvalidId);
        }
        // SHA-256 has 256 bits; the final base32 symbol contains four zero padding bits.
        if !letters.ends_with('A') && !letters.ends_with('Q') {
            return Err(StationIdentityError::InvalidId);
        }
        Ok(Self(grouped(&letters)))
    }
}

impl TryFrom<String> for StationFingerprint {
    type Error = StationIdentityError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl AsRef<str> for StationFingerprint {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for StationFingerprint {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl StationFingerprint {
    fn of_certificate(certificate: &[u8]) -> Self {
        let hash = Sha256::digest(certificate);
        let mut letters = String::with_capacity(52);
        let (mut held, mut bits) = (0u32, 0u32);
        for byte in hash {
            held = (held << 8) | u32::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                letters.push(char::from(BASE32[((held >> bits) & 31) as usize]));
            }
        }
        if bits > 0 {
            letters.push(char::from(BASE32[((held << (5 - bits)) & 31) as usize]));
        }
        Self(grouped(&letters))
    }

    /// Returns the stable certificate fingerprint.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StationFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn grouped(letters: &str) -> String {
    letters
        .as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

/// An station's self-issued TLS certificate and its private key.
pub struct StationIdentity {
    id: StationFingerprint,
    certificate: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl fmt::Debug for StationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("StationIdentity")
            .field(&self.id)
            .finish()
    }
}

impl Clone for StationIdentity {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            certificate: self.certificate.clone(),
            key: self.key.clone_key(),
        }
    }
}

impl StationIdentity {
    /// Creates an station identity without writing it to disk.
    pub fn generate() -> Result<Self, StationIdentityError> {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(vec!["localhost".to_owned()])
                .map_err(|_| StationIdentityError::Certificate)?;
        let text = format!("{}{}", cert.pem(), signing_key.serialize_pem());
        Self::from_pem(&text)
    }

    /// Reads an identity PEM containing one certificate and its matching private key.
    pub fn from_pem(text: &str) -> Result<Self, StationIdentityError> {
        let certificate = CertificateDer::from_pem_slice(text.as_bytes())
            .map_err(|_| StationIdentityError::Certificate)?;
        let key = PrivateKeyDer::from_pem_slice(text.as_bytes())
            .map_err(|_| StationIdentityError::Certificate)?;
        validate_key_pair(&certificate, &key)?;
        Ok(Self {
            id: StationFingerprint::of_certificate(&certificate),
            certificate,
            key,
        })
    }

    /// Loads the identity PEM at path, returning None when it is absent.
    pub fn read(path: &Path) -> Result<Option<Self>, StationIdentityError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_pem(&text).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(StationIdentityError::Io(error)),
        }
    }

    /// Opens or creates identity.pem in an AI6 state directory.
    pub fn open_or_create(state_dir: &Path) -> Result<Self, StationIdentityError> {
        let path = state_dir.join(IDENTITY_FILE);
        if let Some(identity) = Self::read(&path)? {
            return Ok(identity);
        }
        let identity = Self::generate()?;
        match identity.write_new(&path) {
            Ok(()) => {}
            Err(StationIdentityError::Io(error))
                if error.kind() == io::ErrorKind::AlreadyExists =>
            {
                return Self::read(&path)?.ok_or(StationIdentityError::Certificate);
            }
            Err(error) => return Err(error),
        }
        Self::read(&path)?.ok_or(StationIdentityError::Certificate)
    }

    /// Writes the combined certificate and key without replacing an existing identity.
    pub fn write_new(&self, path: &Path) -> Result<(), StationIdentityError> {
        let parent = path.parent().ok_or(StationIdentityError::Path)?;
        fs::create_dir_all(parent).map_err(StationIdentityError::Io)?;
        let pem = format!(
            "{}{}",
            encode_pem("CERTIFICATE", self.certificate.as_ref()),
            private_key_pem(&self.key)?
        );
        let temporary = temporary_path(path);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(StationIdentityError::Io)?;
        let result = (|| {
            file.write_all(pem.as_bytes())
                .map_err(StationIdentityError::Io)?;
            file.sync_all().map_err(StationIdentityError::Io)?;
            fs::hard_link(&temporary, path).map_err(StationIdentityError::Io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                    .map_err(StationIdentityError::Io)?;
            }
            Ok(())
        })();
        drop(file);
        let _ = fs::remove_file(&temporary);
        result
    }

    /// Returns this station's certificate fingerprint.
    pub fn id(&self) -> &StationFingerprint {
        &self.id
    }

    fn certificate(&self) -> CertificateDer<'static> {
        self.certificate.clone()
    }

    fn key(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }
}

fn validate_key_pair(
    certificate: &CertificateDer<'static>,
    key: &PrivateKeyDer<'static>,
) -> Result<(), StationIdentityError> {
    let provider = crypto_provider();
    ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|_| StationIdentityError::Tls)?
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], key.clone_key())
        .map_err(|_| StationIdentityError::Certificate)?;
    Ok(())
}

fn private_key_pem(key: &PrivateKeyDer<'static>) -> Result<String, StationIdentityError> {
    match key {
        PrivateKeyDer::Pkcs8(key) => Ok(encode_pem("PRIVATE KEY", key.secret_pkcs8_der())),
        PrivateKeyDer::Pkcs1(key) => Ok(encode_pem("RSA PRIVATE KEY", key.secret_pkcs1_der())),
        PrivateKeyDer::Sec1(key) => Ok(encode_pem("EC PRIVATE KEY", key.secret_sec1_der())),
        _ => Err(StationIdentityError::Certificate),
    }
}

fn encode_pem(label: &str, der: &[u8]) -> String {
    let encoded = data_encoding::BASE64.encode(der);
    let mut pem = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        pem.push('\n');
    }
    pem.push_str(&format!("-----END {label}-----\n"));
    pem
}

fn temporary_path(path: &Path) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{}.tmp", std::process::id(), nonce));
    path.with_file_name(name)
}

/// Persistent certificate pins learned from rejected incoming handshakes.
#[derive(Debug, Clone)]
pub struct PeerTrustStore {
    root: PathBuf,
}

impl PeerTrustStore {
    /// Opens the peer store under an AI6 state directory.
    pub fn open(state_dir: &Path) -> Result<Self, StationIdentityError> {
        let root = state_dir.join("peers");
        fs::create_dir_all(root.join("accepted")).map_err(StationIdentityError::Io)?;
        fs::create_dir_all(root.join("waiting")).map_err(StationIdentityError::Io)?;
        Ok(Self { root })
    }

    /// Lists the certificate IDs awaiting operator acceptance.
    pub fn waiting(&self) -> Result<Vec<StationFingerprint>, StationIdentityError> {
        self.list("waiting")
    }

    /// Lists accepted certificate IDs.
    pub fn accepted(&self) -> Result<Vec<StationFingerprint>, StationIdentityError> {
        self.list("accepted")
    }

    /// Accepts one previously observed peer certificate by its full fingerprint.
    pub fn accept(&self, id: &StationFingerprint) -> Result<(), StationIdentityError> {
        let source = self.peer_path("waiting", id);
        let target = self.peer_path("accepted", id);
        if target.exists() {
            return if self.is_accepted(id) {
                Err(StationIdentityError::AlreadyAccepted)
            } else {
                Err(StationIdentityError::Certificate)
            };
        }
        let text = fs::read_to_string(&source).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                StationIdentityError::NotWaiting
            } else {
                StationIdentityError::Io(error)
            }
        })?;
        let certificate = CertificateDer::from_pem_slice(text.as_bytes())
            .map_err(|_| StationIdentityError::Certificate)?;
        if StationFingerprint::of_certificate(&certificate) != *id {
            return Err(StationIdentityError::Certificate);
        }
        fs::rename(source, target).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                StationIdentityError::NotWaiting
            } else {
                StationIdentityError::Io(error)
            }
        })
    }

    fn list(&self, section: &str) -> Result<Vec<StationFingerprint>, StationIdentityError> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(self.root.join(section)).map_err(StationIdentityError::Io)? {
            let entry = entry.map_err(StationIdentityError::Io)?;
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "pem")
            {
                let path = entry.path();
                let stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or(StationIdentityError::Certificate)?;
                ids.push(stem.parse()?);
            }
        }
        ids.sort();
        Ok(ids)
    }

    fn peer_path(&self, section: &str, id: &StationFingerprint) -> PathBuf {
        self.root.join(section).join(format!("{}.pem", id.as_str()))
    }

    fn is_accepted(&self, id: &StationFingerprint) -> bool {
        let Ok(text) = fs::read_to_string(self.peer_path("accepted", id)) else {
            return false;
        };
        let Ok(certificate) = CertificateDer::from_pem_slice(text.as_bytes()) else {
            return false;
        };
        StationFingerprint::of_certificate(&certificate) == *id
    }

    fn record_waiting(&self, certificate: &CertificateDer<'_>, id: &StationFingerprint) {
        if certificate.len() > MAX_CERTIFICATE_BYTES {
            return;
        }
        let waiting = self.root.join("waiting");
        let path = self.peer_path("waiting", id);
        if path.exists() {
            return;
        }
        let Ok(entries) = fs::read_dir(&waiting) else {
            return;
        };
        if entries.take(MAX_WAITING_PEERS).count() >= MAX_WAITING_PEERS {
            return;
        }
        let pem = encode_pem("CERTIFICATE", certificate.as_ref());
        let temp = temporary_path(&path);
        let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(&temp) else {
            return;
        };
        if file.write_all(pem.as_bytes()).is_ok() && file.sync_all().is_ok() {
            let _ = fs::hard_link(&temp, path);
        }
        drop(file);
        let _ = fs::remove_file(temp);
    }
}

/// Trust policy for an station listener.
#[derive(Debug, Clone)]
pub enum TlsTrust {
    /// Trust only certificates explicitly accepted through ai6 peer accept.
    Peers(PeerTrustStore),
    /// Trust certificates signed by the listed CAs and matching optional name globs.
    Ca {
        ca_file: PathBuf,
        subject: Vec<String>,
        san: Vec<String>,
    },
    /// Accept an explicitly pinned peer or a certificate matching the CA policy.
    PeersOrCa {
        peers: PeerTrustStore,
        ca_file: PathBuf,
        subject: Vec<String>,
        san: Vec<String>,
    },
}

/// Builds the client TLS configuration for one certificate-pinned station.
pub fn client_config(
    identity: &StationIdentity,
    expected: &StationFingerprint,
) -> Result<ClientConfig, StationIdentityError> {
    let provider = crypto_provider();
    let verifier = PinnedServerVerifier {
        expected: expected.clone(),
        provider: Arc::clone(&provider),
    };
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| StationIdentityError::Tls)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(vec![identity.certificate()], identity.key())
        .map_err(|_| StationIdentityError::Certificate)?;
    config.alpn_protocols = vec![STATION_ALPN.to_vec()];
    Ok(config)
}

/// Builds a server configuration that requires a trusted client certificate.
pub fn server_config(
    identity: &StationIdentity,
    trust: TlsTrust,
) -> Result<ServerConfig, StationIdentityError> {
    let provider = crypto_provider();
    let verifier = OfficeClientVerifier::new(trust, Arc::clone(&provider))?;
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| StationIdentityError::Tls)?
        .with_client_cert_verifier(Arc::new(verifier))
        .with_single_cert(vec![identity.certificate()], identity.key())
        .map_err(|_| StationIdentityError::Certificate)?;
    config.alpn_protocols = vec![STATION_ALPN.to_vec()];
    Ok(config)
}

fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

#[derive(Debug)]
struct PinnedServerVerifier {
    expected: StationFingerprint,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        certificate: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if StationFingerprint::of_certificate(certificate) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "station certificate does not match the pinned identity".to_owned(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            certificate,
            signed,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            certificate,
            signed,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
struct OfficeClientVerifier {
    peers: Option<PeerTrustStore>,
    ca: Option<Arc<dyn ClientCertVerifier>>,
    subjects: Option<GlobSet>,
    sans: Option<GlobSet>,
    provider: Arc<CryptoProvider>,
}

impl OfficeClientVerifier {
    fn new(trust: TlsTrust, provider: Arc<CryptoProvider>) -> Result<Self, StationIdentityError> {
        let (peers, ca_file, subject, san) = match trust {
            TlsTrust::Peers(peers) => (Some(peers), None, Vec::new(), Vec::new()),
            TlsTrust::Ca {
                ca_file,
                subject,
                san,
            } => (None, Some(ca_file), subject, san),
            TlsTrust::PeersOrCa {
                peers,
                ca_file,
                subject,
                san,
            } => (Some(peers), Some(ca_file), subject, san),
        };
        let ca = ca_file
            .as_deref()
            .map(|path| load_ca_verifier(path, Arc::clone(&provider)))
            .transpose()?;
        Ok(Self {
            peers,
            ca,
            subjects: compile_globs(&subject)?,
            sans: compile_globs(&san)?,
            provider,
        })
    }

    fn names_match(&self, certificate: &CertificateDer<'_>) -> Result<bool, rustls::Error> {
        if self.subjects.is_none() && self.sans.is_none() {
            return Ok(true);
        }
        let (_, certificate) =
            x509_parser::parse_x509_certificate(certificate.as_ref()).map_err(|_| {
                rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
            })?;
        if let Some(subjects) = &self.subjects
            && !subjects.is_match(certificate.subject().to_string())
        {
            return Ok(false);
        }
        if let Some(sans) = &self.sans {
            let extension = certificate.subject_alternative_name().map_err(|_| {
                rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
            })?;
            let names = extension
                .as_ref()
                .into_iter()
                .flat_map(|extension| extension.value.general_names.iter())
                .filter_map(|name| match name {
                    x509_parser::extensions::GeneralName::DNSName(name) => Some((*name).to_owned()),
                    x509_parser::extensions::GeneralName::RFC822Name(name) => {
                        Some((*name).to_owned())
                    }
                    x509_parser::extensions::GeneralName::URI(name) => Some((*name).to_owned()),
                    x509_parser::extensions::GeneralName::IPAddress(address) => {
                        Some(format!("{address:?}"))
                    }
                    _ => None,
                });
            if !names.into_iter().any(|name| sans.is_match(name)) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl ClientCertVerifier for OfficeClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.ca
            .as_ref()
            .map_or(&[], |verifier| verifier.root_hint_subjects())
    }

    fn verify_client_cert(
        &self,
        certificate: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let id = StationFingerprint::of_certificate(certificate);
        let accepted = self
            .peers
            .as_ref()
            .is_some_and(|peers| peers.is_accepted(&id));
        if accepted {
            return Ok(ClientCertVerified::assertion());
        }
        if let Some(ca) = &self.ca
            && ca
                .verify_client_cert(certificate, intermediates, now)
                .is_ok()
            && self.names_match(certificate)?
        {
            return Ok(ClientCertVerified::assertion());
        }
        if let Some(peers) = &self.peers {
            peers.record_waiting(certificate, &id);
        }
        Err(rustls::Error::General(
            "client certificate is not trusted by this station".to_owned(),
        ))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            certificate,
            signed,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            certificate,
            signed,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn load_ca_verifier(
    path: &Path,
    provider: Arc<CryptoProvider>,
) -> Result<Arc<dyn ClientCertVerifier>, StationIdentityError> {
    let file = File::open(path).map_err(StationIdentityError::Io)?;
    let certs = rustls_pemfile::certs(&mut BufReader::new(file))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StationIdentityError::Certificate)?;
    if certs.is_empty() {
        return Err(StationIdentityError::Certificate);
    }
    let mut roots = RootCertStore::empty();
    for cert in certs {
        roots
            .add(cert)
            .map_err(|_| StationIdentityError::Certificate)?;
    }
    rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
        .build()
        .map_err(|_| StationIdentityError::Certificate)
}

fn compile_globs(patterns: &[String]) -> Result<Option<GlobSet>, StationIdentityError> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|_| StationIdentityError::TrustPattern)?);
    }
    builder
        .build()
        .map(Some)
        .map_err(|_| StationIdentityError::TrustPattern)
}

/// Sanitized failures while loading, storing or building station TLS identities.
#[derive(Debug)]
#[non_exhaustive]
pub enum StationIdentityError {
    /// The station fingerprint is malformed.
    InvalidId,
    /// The certificate or its private key is missing or invalid.
    Certificate,
    /// The certificate did not satisfy this TLS configuration.
    Tls,
    /// The certificate trust glob is malformed.
    TrustPattern,
    /// The requested peer has not made a rejected connection.
    NotWaiting,
    /// The peer was already accepted.
    AlreadyAccepted,
    /// The path does not name a file inside a directory.
    Path,
    /// Filesystem operation failed.
    Io(io::Error),
}

impl fmt::Display for StationIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "station certificate id is invalid",
            Self::Certificate => "station certificate or private key is invalid",
            Self::Tls => "station TLS configuration is invalid",
            Self::TrustPattern => "station certificate trust pattern is invalid",
            Self::NotWaiting => "station certificate is not waiting for acceptance",
            Self::AlreadyAccepted => "station certificate is already accepted",
            Self::Path => "identity path must have a parent directory",
            Self::Io(_) => "station identity storage failed",
        })
    }
}

impl std::error::Error for StationIdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    #[test]
    fn identity_is_created_once_has_a_private_file_mode_and_requires_its_key() {
        let directory = tempfile::tempdir().unwrap();
        let identity = StationIdentity::open_or_create(directory.path()).unwrap();
        let path = directory.path().join(IDENTITY_FILE);
        assert_eq!(
            StationIdentity::read(&path).unwrap().unwrap().id(),
            identity.id()
        );
        let certificate_only = encode_pem("CERTIFICATE", identity.certificate().as_ref());
        assert!(StationIdentity::from_pem(&certificate_only).is_err());
        assert!(!format!("{identity:?}").contains("PRIVATE KEY"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn office_ids_parse_grouped_and_ungrouped_base32() {
        let identity = StationIdentity::generate().unwrap();
        let grouped = identity.id().to_string();
        let mut noncanonical = grouped.replace('-', "");
        assert_eq!(
            grouped
                .replace('-', "")
                .parse::<StationFingerprint>()
                .unwrap(),
            *identity.id()
        );
        assert!("not-an-station-id".parse::<StationFingerprint>().is_err());
        let last = noncanonical.pop().unwrap();
        noncanonical.push(if last == 'A' { 'B' } else { 'R' });
        assert!(noncanonical.parse::<StationFingerprint>().is_err());
    }

    #[test]
    fn peer_store_accepts_only_waiting_certificate_ids() {
        let directory = tempfile::tempdir().unwrap();
        let peers = PeerTrustStore::open(directory.path()).unwrap();
        let identity = StationIdentity::generate().unwrap();
        let id = identity.id().clone();
        peers.record_waiting(&identity.certificate(), &id);
        assert_eq!(peers.waiting().unwrap(), vec![id.clone()]);
        peers.accept(&id).unwrap();
        assert!(peers.waiting().unwrap().is_empty());
        assert_eq!(peers.accepted().unwrap(), vec![id.clone()]);
        assert!(matches!(
            peers.accept(&id),
            Err(StationIdentityError::AlreadyAccepted)
        ));
    }

    #[test]
    fn peer_accept_checks_the_certificate_fingerprint_not_only_the_filename() {
        let directory = tempfile::tempdir().unwrap();
        let peers = PeerTrustStore::open(directory.path()).unwrap();
        let identity = StationIdentity::generate().unwrap();
        let id = identity.id().clone();
        let path = peers.peer_path("waiting", &id);
        peers.record_waiting(&identity.certificate(), &id);

        let unrelated = StationIdentity::generate().unwrap();
        fs::write(
            path,
            encode_pem("CERTIFICATE", unrelated.certificate().as_ref()),
        )
        .unwrap();

        assert!(matches!(
            peers.accept(&id),
            Err(StationIdentityError::Certificate)
        ));
        assert!(!peers.is_accepted(&id));
    }

    #[tokio::test]
    async fn mtls_refuses_unknown_peer_lists_it_and_accepts_it_after_operator_action() {
        let directory = tempfile::tempdir().unwrap();
        let peers = PeerTrustStore::open(directory.path()).unwrap();
        let server_identity = StationIdentity::generate().unwrap();
        let client_identity = StationIdentity::generate().unwrap();
        let server = server_config(&server_identity, TlsTrust::Peers(peers.clone())).unwrap();
        let client = client_config(&client_identity, server_identity.id()).unwrap();
        assert!(!handshake(server, client).await);
        assert_eq!(peers.waiting().unwrap(), vec![client_identity.id().clone()]);

        peers.accept(client_identity.id()).unwrap();
        let server = server_config(&server_identity, TlsTrust::Peers(peers.clone())).unwrap();
        let client = client_config(&client_identity, server_identity.id()).unwrap();
        assert!(handshake(server, client).await);
    }

    #[tokio::test]
    async fn ca_signed_peer_connects_when_subject_and_san_match_listener_globs() {
        let directory = tempfile::tempdir().unwrap();
        let (ca_pem, issuer) = ca();
        let ca_path = directory.path().join("ca.pem");
        fs::write(&ca_path, ca_pem).unwrap();
        let server_identity = signed_identity(&issuer, "AI6 Server");
        let client_identity = signed_identity(&issuer, "AI6 Client");
        let server = server_config(
            &server_identity,
            TlsTrust::Ca {
                ca_file: ca_path,
                subject: vec!["*AI6 Client*".to_owned()],
                san: vec!["localhost".to_owned()],
            },
        )
        .unwrap();
        let client = client_config(&client_identity, server_identity.id()).unwrap();
        assert!(handshake(server, client).await);
    }

    async fn handshake(server: ServerConfig, client: ClientConfig) -> bool {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server));
        let accepted = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            acceptor.accept(stream).await.is_ok()
        });
        let connector = TlsConnector::from(Arc::new(client));
        let stream = TcpStream::connect(address).await.unwrap();
        let name = ServerName::try_from("localhost".to_owned()).unwrap();
        let connected = connector.connect(name, stream).await.is_ok();
        connected && accepted.await.unwrap()
    }

    fn ca() -> (String, Issuer<'static, KeyPair>) {
        let key = KeyPair::generate().unwrap();
        let mut parameters = CertificateParams::new(Vec::<String>::new()).unwrap();
        parameters.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        parameters
            .distinguished_name
            .push(DnType::CommonName, "AI6 test CA");
        let certificate = parameters.self_signed(&key).unwrap();
        let pem = certificate.pem();
        (pem, Issuer::new(parameters, key))
    }

    fn signed_identity(issuer: &Issuer<'_, KeyPair>, common_name: &str) -> StationIdentity {
        let key = KeyPair::generate().unwrap();
        let mut parameters = CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
        parameters
            .distinguished_name
            .push(DnType::CommonName, common_name);
        let certificate = parameters.signed_by(&key, issuer).unwrap();
        StationIdentity::from_pem(&format!("{}{}", certificate.pem(), key.serialize_pem())).unwrap()
    }
}
