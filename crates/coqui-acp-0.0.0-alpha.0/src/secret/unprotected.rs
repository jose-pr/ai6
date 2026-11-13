//! Identities whose private keys are held in this process's memory, for tests and tools.
//!
//! A product does not use these: its identity key lives in a platform key store, a browser's WebCrypto or a device, and it hands
//! [`Prover::signing`] and [`Prover::agreeing`] functions that call into it. Everything here keeps the raw key where a memory dump or
//! a core file can read it, and its `p256` signatures are deterministic (RFC 6979).

use super::binding::{P256_SPKI_PREFIX, Prover, X25519_SPKI_PREFIX};
use super::error::Error;
use alloc::vec::Vec;
use p256::ecdsa::SigningKey;
use p256::ecdsa::signature::Signer as _;
use p256::pkcs8::DecodePrivateKey;
use x25519_dalek::{PublicKey, StaticSecret};

/// The PKCS#8 `PrivateKeyInfo` of an X25519 key before its 32 bytes (RFC 8410).
const X25519_PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04, 0x22, 0x04, 0x20,
];

fn signing(key: SigningKey) -> Result<Prover, Error> {
    let mut spki = P256_SPKI_PREFIX.to_vec();
    spki.extend_from_slice(key.verifying_key().to_sec1_point(false).as_bytes());
    Prover::signing(spki, move |message| {
        let signature: p256::ecdsa::Signature = key.sign(message);
        Ok(signature.to_bytes().into())
    })
}

/// An identity that signs, from its P-256 private scalar (32 bytes).
///
/// # Errors
/// [`Error::Key`] for a scalar that is no key.
pub fn ecdsa_p256(scalar: &[u8]) -> Result<Prover, Error> {
    signing(SigningKey::from_slice(scalar).map_err(|_| Error::Key("not a P-256 private scalar"))?)
}

/// An identity that signs, from its PKCS#8 private key, as the `webcrypto` peer method writes it.
///
/// # Errors
/// [`Error::Key`] for what is not a P-256 PKCS#8 key.
pub fn ecdsa_p256_pkcs8(der: &[u8]) -> Result<Prover, Error> {
    signing(SigningKey::from_pkcs8_der(der).map_err(|_| Error::Key("not a P-256 PKCS#8 key"))?)
}

/// An X25519 identity's `SubjectPublicKeyInfo` and its key agreement, from its private key (32 bytes): what
/// [`Prover::agreeing`] and [`VerifierBuilder::agreement`](super::VerifierBuilder::agreement) take.
///
/// # Errors
/// [`Error::Key`] for a key of another length.
#[allow(clippy::type_complexity)]
pub fn x25519_agreement(
    secret: &[u8],
) -> Result<
    (
        Vec<u8>,
        impl Fn(&[u8; 32]) -> Result<[u8; 32], Error> + Send + Sync + 'static,
    ),
    Error,
> {
    let secret: [u8; 32] = secret
        .try_into()
        .map_err(|_| Error::Key("an X25519 private key is 32 bytes"))?;
    let secret = StaticSecret::from(secret);
    let mut spki = X25519_SPKI_PREFIX.to_vec();
    spki.extend_from_slice(PublicKey::from(&secret).as_bytes());
    Ok((spki, move |peer: &[u8; 32]| {
        Ok(secret.diffie_hellman(&PublicKey::from(*peer)).to_bytes())
    }))
}

/// An X25519 identity, from its private key (32 bytes), for the collector whose `SubjectPublicKeyInfo` is `collector_spki`.
///
/// # Errors
/// [`Error::Key`].
pub fn x25519(secret: &[u8], collector_spki: &[u8]) -> Result<Prover, Error> {
    let (spki, agree) = x25519_agreement(secret)?;
    Prover::agreeing(spki, agree, collector_spki)
}

/// An X25519 identity from its PKCS#8 private key, as the `noise` peer method writes it (RFC 8410, 48 bytes).
///
/// # Errors
/// [`Error::Key`].
pub fn x25519_pkcs8(der: &[u8], collector_spki: &[u8]) -> Result<Prover, Error> {
    let key = der
        .strip_prefix(&X25519_PKCS8_PREFIX[..])
        .filter(|key| key.len() == 32)
        .ok_or(Error::Key("not an X25519 PKCS#8 key"))?;
    x25519(key, collector_spki)
}
