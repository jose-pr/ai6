//! The four handshake messages and their one encoding.
//!
//! Decoding parses a message and encodes it back: only the exact bytes `encode` gives are
//! accepted, so member order, whitespace, escapes, duplicates and unknown members need no rule of
//! their own, and a JavaScript peer's `JSON.parse` cannot read a message differently.

use data_encoding::BASE64URL_NOPAD;
use p256::PublicKey;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::elliptic_curve::sec1::ToSec1Point as _;
use serde::{Deserialize, Serialize};

use crate::error::Error;

/// The profile every message and transcript names.
pub const PROFILE: &str = "dev.boriken.coqui.peer-webcrypto/v1";

/// The longest handshake message a [`WebCrypto`](crate::WebCrypto) accepts by default.
pub const DEFAULT_MAX_MESSAGE: usize = 3072;

const VERSION: u32 = 1;

/// A P-256 SubjectPublicKeyInfo holding an uncompressed point is this prefix and the point's 64
/// coordinate bytes, so comparing the prefix leaves no ASN.1 for two parsers to read differently.
const SPKI_PREFIX: [u8; 27] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
];

/// Bytes of the SubjectPublicKeyInfo of a P-256 key.
pub(crate) const SPKI_LEN: usize = 91;
/// Bytes of an uncompressed P-256 point.
pub(crate) const POINT_LEN: usize = 65;
pub(crate) const NONCE_LEN: usize = 32;
const SIGNATURE_LEN: usize = 64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    profile: String,
    version: u32,
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "publicKey", skip_serializing_if = "Option::is_none")]
    public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ephemeral: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
}

impl Wire {
    fn new(kind: &str) -> Self {
        Self {
            profile: PROFILE.to_owned(),
            version: VERSION,
            kind: kind.to_owned(),
            public_key: None,
            ephemeral: None,
            nonce: None,
            signature: None,
        }
    }

    fn encode(&self) -> Vec<u8> {
        // A struct of strings and a number: serialising it cannot fail.
        serde_json::to_vec(self).unwrap_or_default()
    }

    fn decode(raw: &[u8], kind: &str, max: usize) -> Result<Self, Error> {
        if raw.len() > max {
            return Err(Error::TooLong);
        }
        let wire: Self = serde_json::from_slice(raw).map_err(|_| Error::Malformed)?;
        if wire.profile != PROFILE
            || wire.version != VERSION
            || wire.kind != kind
            || wire.encode() != raw
        {
            return Err(Error::Malformed);
        }
        Ok(wire)
    }
}

fn base64<const N: usize>(text: Option<&str>) -> Result<[u8; N], Error> {
    let text = text.ok_or(Error::Malformed)?.as_bytes();
    let mut out = [0u8; N];
    if BASE64URL_NOPAD.decode_len(text.len()) != Ok(N) {
        return Err(Error::Malformed);
    }
    match BASE64URL_NOPAD.decode_mut(text, &mut out) {
        Ok(length) if length == N => Ok(out),
        _ => Err(Error::Malformed),
    }
}

/// A P-256 public key as this protocol sends it, known to be on the curve.
#[derive(Clone)]
pub(crate) struct Spki {
    pub(crate) der: [u8; SPKI_LEN],
    pub(crate) key: VerifyingKey,
}

impl Spki {
    pub(crate) fn parse(der: [u8; SPKI_LEN]) -> Result<Self, Error> {
        if der[..SPKI_PREFIX.len()] != SPKI_PREFIX {
            return Err(Error::Malformed);
        }
        let key = VerifyingKey::from_sec1_bytes(&der[SPKI_PREFIX.len() - 1..])
            .map_err(|_| Error::Malformed)?;
        Ok(Self { der, key })
    }

    pub(crate) fn of(key: &VerifyingKey) -> Self {
        let point = key.to_sec1_point(false);
        let mut der = [0u8; SPKI_LEN];
        der[..SPKI_PREFIX.len()].copy_from_slice(&SPKI_PREFIX);
        der[SPKI_PREFIX.len() - 1..].copy_from_slice(point.as_bytes());
        Self { der, key: *key }
    }
}

/// An ephemeral ECDH public key as this protocol sends it, known to be on the curve.
#[derive(Clone)]
pub(crate) struct Point {
    pub(crate) bytes: [u8; POINT_LEN],
    pub(crate) key: PublicKey,
}

impl Point {
    pub(crate) fn parse(bytes: [u8; POINT_LEN]) -> Result<Self, Error> {
        if bytes[0] != 0x04 {
            return Err(Error::Malformed);
        }
        let key = PublicKey::from_sec1_bytes(&bytes).map_err(|_| Error::Malformed)?;
        Ok(Self { bytes, key })
    }

    pub(crate) fn of(key: PublicKey) -> Self {
        let point = key.to_sec1_point(false);
        let mut bytes = [0u8; POINT_LEN];
        bytes.copy_from_slice(point.as_bytes());
        Self { bytes, key }
    }
}

fn signature(text: Option<&str>) -> Result<Signature, Error> {
    let bytes: [u8; SIGNATURE_LEN] = base64(text)?;
    Signature::from_slice(&bytes).map_err(|_| Error::Malformed)
}

fn text(bytes: &[u8]) -> Option<String> {
    Some(BASE64URL_NOPAD.encode(bytes))
}

/// The initiator's opening: its key, ephemeral key and nonce.
pub(crate) struct Hello {
    pub(crate) public_key: Spki,
    pub(crate) ephemeral: Point,
    pub(crate) nonce: [u8; NONCE_LEN],
}

impl Hello {
    pub(crate) fn decode(raw: &[u8], max: usize) -> Result<Self, Error> {
        let wire = Wire::decode(raw, "hello", max)?;
        if wire.signature.is_some() {
            return Err(Error::Malformed);
        }
        Ok(Self {
            public_key: Spki::parse(base64(wire.public_key.as_deref())?)?,
            ephemeral: Point::parse(base64(wire.ephemeral.as_deref())?)?,
            nonce: base64(wire.nonce.as_deref())?,
        })
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut wire = Wire::new("hello");
        wire.public_key = text(&self.public_key.der);
        wire.ephemeral = text(&self.ephemeral.bytes);
        wire.nonce = text(&self.nonce);
        wire.encode()
    }
}

/// The responder's answer: its key, ephemeral key and nonce, and its proof.
pub(crate) struct Challenge {
    pub(crate) public_key: Spki,
    pub(crate) ephemeral: Point,
    pub(crate) nonce: [u8; NONCE_LEN],
    pub(crate) signature: Signature,
}

impl Challenge {
    pub(crate) fn decode(raw: &[u8], max: usize) -> Result<Self, Error> {
        let wire = Wire::decode(raw, "challenge", max)?;
        Ok(Self {
            public_key: Spki::parse(base64(wire.public_key.as_deref())?)?,
            ephemeral: Point::parse(base64(wire.ephemeral.as_deref())?)?,
            nonce: base64(wire.nonce.as_deref())?,
            signature: signature(wire.signature.as_deref())?,
        })
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut wire = Wire::new("challenge");
        wire.public_key = text(&self.public_key.der);
        wire.ephemeral = text(&self.ephemeral.bytes);
        wire.nonce = text(&self.nonce);
        wire.signature = text(&self.signature.to_bytes());
        wire.encode()
    }
}

/// A message that carries only a signature: `proof` and `accept`.
pub(crate) fn decode_signature(raw: &[u8], kind: &str, max: usize) -> Result<Signature, Error> {
    let wire = Wire::decode(raw, kind, max)?;
    if wire.public_key.is_some() || wire.ephemeral.is_some() || wire.nonce.is_some() {
        return Err(Error::Malformed);
    }
    signature(wire.signature.as_deref())
}

pub(crate) fn encode_signature(kind: &str, signature: &Signature) -> Vec<u8> {
    let mut wire = Wire::new(kind);
    wire.signature = text(&signature.to_bytes());
    wire.encode()
}
