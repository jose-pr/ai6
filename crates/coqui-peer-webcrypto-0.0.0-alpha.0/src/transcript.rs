//! What both sides sign, the session id it names, and the keys derived from it.

use hkdf::Hkdf;
use p256::ecdsa::signature::{Signer as _, Verifier as _};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::error::Error;
use crate::message::{NONCE_LEN, POINT_LEN, PROFILE, SPKI_LEN};

pub(crate) const ROLE_SERVER: &str = "server";
pub(crate) const ROLE_CLIENT: &str = "client";
pub(crate) const ROLE_ACCEPT: &str = "accept";
const KEY_LEN: usize = 32;

/// One AES-256 key per direction and the secret exports are drawn from. Each is held on the heap,
/// so moving this value copies no key bytes.
pub(crate) struct Keys {
    pub(crate) initiator: Zeroizing<Vec<u8>>,
    pub(crate) responder: Zeroizing<Vec<u8>>,
    pub(crate) exporter: Zeroizing<Vec<u8>>,
}

/// Everything either side contributed, and the optional outer binding, which is never sent.
pub(crate) struct Transcript {
    pub(crate) initiator_key: [u8; SPKI_LEN],
    pub(crate) responder_key: [u8; SPKI_LEN],
    pub(crate) initiator_nonce: [u8; NONCE_LEN],
    pub(crate) responder_nonce: [u8; NONCE_LEN],
    pub(crate) initiator_ephemeral: [u8; POINT_LEN],
    pub(crate) responder_ephemeral: [u8; POINT_LEN],
    pub(crate) binding: Vec<u8>,
}

pub(crate) fn field(out: &mut Vec<u8>, bytes: &[u8]) {
    let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(bytes);
}

impl Transcript {
    fn tail(&self, out: &mut Vec<u8>) {
        field(out, &self.initiator_key);
        field(out, &self.responder_key);
        field(out, &self.initiator_nonce);
        field(out, &self.responder_nonce);
        field(out, &self.initiator_ephemeral);
        field(out, &self.responder_ephemeral);
        field(out, &self.binding);
    }

    /// The bytes signed for `role`.
    pub(crate) fn signed(&self, role: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(512);
        field(&mut out, PROFILE.as_bytes());
        field(&mut out, role.as_bytes());
        self.tail(&mut out);
        out
    }

    /// The session id: the same bytes without a role, hashed. Every signature covers what it
    /// hashes, so both ends holding one id means both signed for the same two keys.
    pub(crate) fn id(&self) -> [u8; 32] {
        let mut out = Vec::with_capacity(512);
        field(&mut out, PROFILE.as_bytes());
        self.tail(&mut out);
        Sha256::digest(&out).into()
    }

    pub(crate) fn sign(&self, key: &SigningKey, role: &str) -> Signature {
        key.sign(&self.signed(role))
    }

    pub(crate) fn verify(
        &self,
        key: &VerifyingKey,
        role: &str,
        signature: &Signature,
    ) -> Result<(), Error> {
        key.verify(&self.signed(role), signature)
            .map_err(|_| Error::BadProof)
    }

    /// The session keys from the ECDH shared secret, the point's x coordinate. The transcript goes
    /// in `info`: RFC 5869 section 3.4 assumes a salt independent of the input keying material, and
    /// the id hashes the ephemeral keys that material comes from.
    pub(crate) fn keys(&self, shared: &[u8]) -> Result<Keys, Error> {
        let mut info = Vec::with_capacity(128);
        field(&mut info, format!("{PROFILE}/keys").as_bytes());
        field(&mut info, &self.id());
        let mut okm = Zeroizing::new([0u8; 3 * KEY_LEN]);
        Hkdf::<Sha256>::new(None, shared)
            .expand(&info, &mut *okm)
            .map_err(|_| Error::TooLong)?;
        let part = |range: core::ops::Range<usize>| {
            let mut key = Zeroizing::new(Vec::with_capacity(KEY_LEN));
            key.extend_from_slice(&okm[range]);
            key
        };
        Ok(Keys {
            initiator: part(0..KEY_LEN),
            responder: part(KEY_LEN..2 * KEY_LEN),
            exporter: part(2 * KEY_LEN..3 * KEY_LEN),
        })
    }
}
