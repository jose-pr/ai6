//! Station certificate identities and mutual TLS trust.
//!
//! A Station's identity, the peers it trusts and the TLS configurations that use them come
//! from `coqui-peer-tls` and `coqui-peer`; this module fixes where AI6 keeps them and the
//! protocol it speaks. A peer is named by its [`PeerId`], printed `sha256:<hex>` and read in
//! any spelling `coqui-peer` knows, grouped base32 included. Every setting that decides what a
//! handshake accepts is a [`Policy`]: callers pass [`Policy::modern`] (TLS 1.3, SHA-2 pins)
//! unless an operator chose otherwise.

use std::fmt;
use std::path::Path;

pub use coqui_peer::{Digest, WaitingEntry, WaitingLimits};
pub use coqui_peer_tls::{
    Built, CaPolicy, Identity, NonDefault, PeerId, PeerIdentity, Pins, Policy, Trust, TrustStore,
    TrustStoreExt,
};
use coqui_peer_tls::{PeerError, client_config_with, server_config_with};
use rustls::{ClientConfig, ServerConfig};

use crate::directorates::registry::sealed::StationSealingKey;

/// The ALPN protocol Stations speak to each other.
pub const STATION_ALPN: &[u8] = b"ai6-station/1";

/// The identity file in a state directory: the certificate (and any intermediates) followed by
/// its private key.
pub const IDENTITY_FILE: &str = "identity.pem";

/// Why a Station identity, trust store or TLS configuration could not be made.
///
/// The messages never hold key material. A CA file that cannot be used
/// ([`PeerError::CaFile`]) is named by its path.
#[derive(Debug)]
#[non_exhaustive]
pub enum StationIdentityError {
    /// The identity or a TLS configuration was refused.
    Tls(PeerError),
    /// The trust store or a fingerprint was refused.
    Peers(coqui_peer::Error),
}

impl fmt::Display for StationIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tls(error) => error.fmt(formatter),
            Self::Peers(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StationIdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tls(error) => Some(error),
            Self::Peers(error) => Some(error),
        }
    }
}

impl From<PeerError> for StationIdentityError {
    fn from(error: PeerError) -> Self {
        Self::Tls(error)
    }
}

impl From<coqui_peer::Error> for StationIdentityError {
    fn from(error: coqui_peer::Error) -> Self {
        Self::Peers(error)
    }
}

/// Opens this Station's identity in `state_dir`, creating a self-signed one on first use. A
/// file holding a chain (the leaf first, then its intermediates) is read whole.
///
/// # Errors
///
/// Returns an error when the file is unreadable, holds no key matching its first
/// certificate, or cannot be created.
pub fn open_identity(state_dir: &Path) -> Result<Identity, StationIdentityError> {
    Ok(Identity::open_or_create(&state_dir.join(IDENTITY_FILE))?)
}

/// Opens the peers this Station has accepted or is asked to accept, under
/// `<state_dir>/peers`, bounded by `policy.waiting` (per source address, in total, and how
/// often one peer from one address is written down again).
///
/// # Errors
///
/// Returns an error when the directories cannot be created.
pub fn open_peers(state_dir: &Path, policy: &Policy) -> Result<TrustStore, StationIdentityError> {
    Ok(TrustStore::open(state_dir)?.with_limits(policy.waiting))
}

/// Trust exactly the peers in `store`: a peer that is not accepted is refused, and recorded as
/// waiting (with the address it came from) once it has proved it holds its key.
pub fn trust_peers(store: TrustStore) -> Trust {
    Trust::Pinned(Pins::store(store))
}

/// The client side of a Station connection: presents `identity` and accepts only the Station
/// whose certificate has the fingerprint `server`.
///
/// # Errors
///
/// Returns an error when `policy` asks for something the TLS library cannot do, or lists no
/// digest that `server` uses.
pub fn client_config(
    identity: &Identity,
    server: &PeerId,
    policy: &Policy,
) -> Result<Built<ClientConfig>, StationIdentityError> {
    let trust = Trust::Pinned(Pins::fixed([*server]));
    Ok(client_config_with(
        identity,
        &trust,
        &[STATION_ALPN],
        policy,
    )?)
}

/// The server side of a Station connection: requires a client certificate that `trust`
/// accepts, and proof that the client holds its key.
///
/// # Errors
///
/// Returns an error when `policy` asks for something the TLS library cannot do, or `trust`
/// cannot be used by a server.
pub fn server_config(
    identity: &Identity,
    trust: &Trust,
    policy: &Policy,
) -> Result<Built<ServerConfig>, StationIdentityError> {
    Ok(server_config_with(
        identity,
        trust,
        &[STATION_ALPN],
        policy,
    )?)
}

/// What a Station's identity signs about its sealing key. A TLS 1.3 handshake's signed text starts
/// with 64 spaces, so this cannot be replayed as one or taken from one.
fn sealing_key_message(station: &str, public_key: &str) -> Vec<u8> {
    format!("ai6 sealing key\n{station}\n{public_key}\n").into_bytes()
}

/// The sealing key `public_key` of `station`, signed by `identity`. The identity signs and never
/// opens anything: the key it vouches for is a separate one.
///
/// # Errors
///
/// When the identity's key cannot sign.
pub fn sign_sealing_key(
    identity: &Identity,
    station: &str,
    public_key: &str,
) -> Result<StationSealingKey, String> {
    use base64::Engine as _;
    use rustls::SignatureScheme;

    let key = rustls::crypto::ring::sign::any_supported_type(&identity.private_key_der())
        .map_err(|_| "the identity's key cannot sign".to_owned())?;
    let signer = key
        .choose_scheme(&[
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ED25519,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PSS_SHA256,
        ])
        .ok_or("the identity's key signs no scheme this Station knows")?;
    let signature = signer
        .sign(&sealing_key_message(station, public_key))
        .map_err(|_| "the identity's key did not sign".to_owned())?;
    let text = base64::engine::general_purpose::STANDARD;
    Ok(StationSealingKey::new(station, public_key).signed(
        text.encode(identity.certificate_der()),
        u16::from(signer.scheme()),
        text.encode(signature),
    ))
}

/// Whether a published key can be trusted: a signature it carries must hold for its certificate,
/// and `fingerprint`, when one is named, must be that certificate's.
///
/// # Errors
///
/// `Err(())` when it does not hold; the caller names the Station.
#[allow(clippy::result_unit_err)]
pub fn check_sealing_key(record: &StationSealingKey, fingerprint: Option<&str>) -> Result<(), ()> {
    use base64::Engine as _;
    let text = base64::engine::general_purpose::STANDARD;
    let (Some(certificate), Some(scheme), Some(signature)) =
        (&record.certificate, record.scheme, &record.signature)
    else {
        return if fingerprint.is_some() {
            Err(())
        } else {
            Ok(())
        };
    };
    let certificate = text.decode(certificate).map_err(|_| ())?;
    let signature = text.decode(signature).map_err(|_| ())?;
    if let Some(fingerprint) = fingerprint {
        let wanted = fingerprint.parse::<PeerId>().map_err(|_| ())?;
        if coqui_peer_tls::certificate_key(&certificate)
            .map_err(|_| ())?
            .id()
            != wanted
        {
            return Err(());
        }
    }
    let der = rustls_pki_types::CertificateDer::from(certificate.as_slice());
    let end_entity = webpki::EndEntityCert::try_from(&der).map_err(|_| ())?;
    let message = sealing_key_message(&record.station, &record.public_key);
    let provider = rustls::crypto::ring::default_provider();
    let scheme = rustls::SignatureScheme::from(scheme);
    let (_, algorithms) = provider
        .signature_verification_algorithms
        .mapping
        .iter()
        .find(|(known, _)| *known == scheme)
        .ok_or(())?;
    if algorithms.iter().any(|algorithm| {
        end_entity
            .verify_signature(*algorithm, &message, &signature)
            .is_ok()
    }) {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use coqui_peer_tls::asynch::{accept, dial};
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    #[test]
    fn identity_is_created_once_and_keeps_its_fingerprint() {
        let directory = tempfile::tempdir().unwrap();
        let identity = open_identity(directory.path()).unwrap();
        let path = directory.path().join(IDENTITY_FILE);
        assert_eq!(
            Identity::read(&path).unwrap().unwrap().fingerprint(),
            identity.fingerprint()
        );
        assert_eq!(
            open_identity(directory.path()).unwrap().fingerprint(),
            identity.fingerprint()
        );
        assert!(!format!("{identity:?}").contains("PRIVATE KEY"));
        assert!(identity.fingerprint().to_string().starts_with("sha256:"));

        // The certificate block, then the PKCS#8 key block, with 64-character lines.
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("-----BEGIN CERTIFICATE-----\n"));
        assert!(text.contains("-----BEGIN PRIVATE KEY-----\n"));
        assert!(
            text.lines()
                .all(|line| line.len() <= 64 || line.starts_with("-----"))
        );
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
    fn a_fingerprint_reads_in_every_spelling() {
        let identity = Identity::generate().unwrap();
        let fingerprint = identity.fingerprint();
        let grouped = fingerprint.to_base32_grouped();
        assert_eq!(grouped.parse::<PeerId>().unwrap(), fingerprint);
        assert_eq!(
            grouped.replace('-', "").parse::<PeerId>().unwrap(),
            fingerprint
        );
        assert_eq!(fingerprint.hex().parse::<PeerId>().unwrap(), fingerprint);
        assert_eq!(
            fingerprint.to_string().parse::<PeerId>().unwrap(),
            fingerprint
        );
        assert!("not-a-station-id".parse::<PeerId>().is_err());
    }

    #[test]
    fn the_policy_bounds_the_waiting_list_per_source_address() {
        let directory = tempfile::tempdir().unwrap();
        let mut policy = Policy::modern();
        policy.waiting.per_source = 1;
        let peers = open_peers(directory.path(), &policy).unwrap();
        let source: SocketAddr = "192.0.2.7:4000".parse().unwrap();
        let first = Identity::generate().unwrap();
        let second = Identity::generate().unwrap();
        peers.record_waiting_certificate(first.certificate_der(), Some(source));
        peers.record_waiting_certificate(second.certificate_der(), Some(source));
        let waiting = peers.waiting().unwrap();
        assert_eq!(waiting.len(), 1, "one source holds one place");
        // A full source refuses the newcomer rather than evicting the peer already waiting.
        assert_eq!(waiting[0].id, first.fingerprint());
        assert_eq!(waiting[0].source, Some(source));
    }

    #[tokio::test]
    async fn mtls_refuses_an_unknown_peer_lists_it_with_its_address_and_accepts_it_after_the_operator()
     {
        let directory = tempfile::tempdir().unwrap();
        let policy = Policy::modern();
        let peers = open_peers(directory.path(), &policy).unwrap();
        let server_identity = Identity::generate().unwrap();
        let client_identity = Identity::generate().unwrap();
        let server = server_config(&server_identity, &trust_peers(peers.clone()), &policy).unwrap();
        assert!(server.non_defaults.is_empty());
        let client =
            client_config(&client_identity, &server_identity.fingerprint(), &policy).unwrap();
        let refused = handshake(server.config, client.config).await;
        assert!(refused.is_err());
        let waiting = peers.waiting().unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].id, client_identity.fingerprint());
        assert!(
            waiting[0]
                .source
                .is_some_and(|source| source.ip().is_loopback())
        );

        peers.accept(&client_identity.fingerprint()).unwrap();
        let server = server_config(&server_identity, &trust_peers(peers), &policy).unwrap();
        let client =
            client_config(&client_identity, &server_identity.fingerprint(), &policy).unwrap();
        let (server_sees, client_sees, protocol) =
            handshake(server.config, client.config).await.unwrap();
        assert_eq!(server_sees, client_identity.fingerprint());
        assert_eq!(client_sees, server_identity.fingerprint());
        assert_eq!(protocol.as_deref(), Some(STATION_ALPN));
    }

    #[tokio::test]
    async fn a_client_refuses_a_server_that_is_not_the_pinned_station() {
        let policy = Policy::modern();
        let server_identity = Identity::generate().unwrap();
        let client_identity = Identity::generate().unwrap();
        let other = Identity::generate().unwrap();
        let server = server_config(&server_identity, &Trust::AnyWithKeyProof, &policy).unwrap();
        let client = client_config(&client_identity, &other.fingerprint(), &policy).unwrap();
        assert!(handshake(server.config, client.config).await.is_err());
    }

    #[tokio::test]
    async fn ca_signed_peer_connects_when_subject_and_san_match_listener_globs() {
        let directory = tempfile::tempdir().unwrap();
        let policy = Policy::modern();
        let (ca_pem, issuer) = ca();
        let ca_path = directory.path().join("ca.pem");
        fs::write(&ca_path, ca_pem).unwrap();
        let server_identity = signed_identity(&issuer, "AI6 Server");
        let client_identity = signed_identity(&issuer, "AI6 Client");
        let trust = |subject: &str| {
            Trust::Ca(CaPolicy {
                ca_file: ca_path.clone(),
                subject: vec![subject.to_owned()],
                san: vec!["localhost".to_owned()],
                ip_in_dns: true,
            })
        };
        let server = server_config(&server_identity, &trust("*AI6 Client*"), &policy).unwrap();
        let client =
            client_config(&client_identity, &server_identity.fingerprint(), &policy).unwrap();
        assert!(handshake(server.config, client.config).await.is_ok());

        let server = server_config(&server_identity, &trust("*Somebody Else*"), &policy).unwrap();
        let client =
            client_config(&client_identity, &server_identity.fingerprint(), &policy).unwrap();
        assert!(handshake(server.config, client.config).await.is_err());
    }

    #[test]
    fn a_sealing_key_is_vouched_for_by_its_station_s_identity_and_no_other() {
        let identity = Identity::generate().unwrap();
        let other = Identity::generate().unwrap();
        let own = identity.fingerprint().to_string();
        let signed = sign_sealing_key(&identity, "station-a", "AAAA").unwrap();
        assert!(check_sealing_key(&signed, None).is_ok());
        assert!(check_sealing_key(&signed, Some(&own)).is_ok());
        assert!(check_sealing_key(&signed, Some(&other.fingerprint().to_string())).is_err());
        let mut swapped = signed.clone();
        swapped.public_key = "BBBB".to_owned();
        assert!(check_sealing_key(&swapped, None).is_err());
        let mut moved = signed.clone();
        moved.station = "station-b".to_owned();
        assert!(check_sealing_key(&moved, None).is_err());
        let unsigned = StationSealingKey::new("station-a", "AAAA");
        assert!(check_sealing_key(&unsigned, None).is_ok());
        assert!(check_sealing_key(&unsigned, Some(&own)).is_err());
    }

    #[test]
    fn a_looser_policy_is_reported() {
        let identity = Identity::generate().unwrap();
        let mut policy = Policy::modern();
        policy.check_validity = false;
        let built = server_config(&identity, &Trust::AnyWithKeyProof, &policy).unwrap();
        assert_eq!(built.non_defaults.len(), 1);
    }

    /// Runs one handshake over loopback. On success: the fingerprint each side saw of the
    /// other, and the protocol the server negotiated.
    async fn handshake(
        server: ServerConfig,
        client: ClientConfig,
    ) -> Result<(PeerId, PeerId, Option<Vec<u8>>), PeerError> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = Arc::new(server);
        let accepted = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accept(server, stream).await.map(|(stream, peer)| {
                (
                    peer.fingerprint,
                    stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec),
                )
            })
        });
        let stream = TcpStream::connect(address).await.unwrap();
        let dialed = dial(Arc::new(client), stream).await;
        let accepted = accepted.await.unwrap();
        match (dialed, accepted) {
            (Ok((_, server_peer)), Ok((client_fingerprint, protocol))) => {
                Ok((client_fingerprint, server_peer.fingerprint, protocol))
            }
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
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

    fn signed_identity(issuer: &Issuer<'_, KeyPair>, common_name: &str) -> Identity {
        let key = KeyPair::generate().unwrap();
        let mut parameters = CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
        parameters
            .distinguished_name
            .push(DnType::CommonName, common_name);
        let certificate = parameters.signed_by(&key, issuer).unwrap();
        Identity::from_pem(&format!("{}{}", certificate.pem(), key.serialize_pem())).unwrap()
    }
}
