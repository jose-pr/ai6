//! The PIN exchange's cryptography: SPAKE2 over Ed25519 (the `spake2` crate), keys per direction
//! by HKDF-SHA256 from its shared key, and HMAC-SHA256 key confirmation compared in constant
//! time.

use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use subtle::ConstantTimeEq;

use crate::error::Error;
use crate::id::PeerId;
use crate::pair::code::Pin;
use crate::random::Random;

/// HKDF info for the key that authenticates the joiner's confirmation.
const JOINER: &[u8] = b"coqui-pair joiner confirmation";
/// HKDF info for the key that authenticates the responder's confirmation.
const RESPONDER: &[u8] = b"coqui-pair responder confirmation";

/// One side's SPAKE2 state.
pub(crate) struct Pake(Spake2<Ed25519Group>);

impl core::fmt::Debug for Pake {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("Pake")
    }
}

/// The confirmation keys both sides derive from the SPAKE2 key.
pub(crate) struct Keys {
    pub(crate) joiner: [u8; 32],
    pub(crate) responder: [u8; 32],
}

impl Drop for Keys {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.joiner);
        zeroize::Zeroize::zeroize(&mut self.responder);
    }
}

/// Serves `rand_core` from a [`Random`], remembering a failure instead of hiding it.
struct Draw<'a> {
    random: &'a Random,
    failed: bool,
}

impl rand_core::RngCore for Draw<'_> {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0u8; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    fn fill_bytes(&mut self, out: &mut [u8]) {
        if self.random.fill(out).is_err() {
            self.failed = true;
        }
    }

    fn try_fill_bytes(&mut self, out: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(out);
        Ok(())
    }
}

impl rand_core::CryptoRng for Draw<'_> {}

/// Starts SPAKE2 as the joiner (side A) or the responder (side B), with both peers' ids as the
/// identities. Returns the state and the message to send.
///
/// # Errors
///
/// [`Error::Random`] when `random` fails; the state is then discarded.
pub(crate) fn start(
    joiner: bool,
    pin: &Pin,
    joiner_id: &PeerId,
    responder_id: &PeerId,
    random: &Random,
) -> Result<(Pake, Vec<u8>), Error> {
    let password = Password::new(pin.as_str().as_bytes());
    let (a, b) = (
        Identity::new(joiner_id.canonical().as_bytes()),
        Identity::new(responder_id.canonical().as_bytes()),
    );
    let mut draw = Draw {
        random,
        failed: false,
    };
    let (state, message) = if joiner {
        Spake2::<Ed25519Group>::start_a_with_rng(&password, &a, &b, &mut draw)
    } else {
        Spake2::<Ed25519Group>::start_b_with_rng(&password, &a, &b, &mut draw)
    };
    if draw.failed {
        return Err(Error::Random);
    }
    Ok((Pake(state), message))
}

/// Finishes SPAKE2 with the other side's message and derives the confirmation keys; `None`
/// when the message is malformed.
pub(crate) fn finish(pake: Pake, other: &[u8]) -> Option<Keys> {
    let shared = zeroize::Zeroizing::new(pake.0.finish(other).ok()?);
    let hkdf = Hkdf::<Sha256>::new(None, &shared);
    let mut keys = Keys {
        joiner: [0; 32],
        responder: [0; 32],
    };
    hkdf.expand(JOINER, &mut keys.joiner).ok()?;
    hkdf.expand(RESPONDER, &mut keys.responder).ok()?;
    Some(keys)
}

/// HMAC-SHA256 of `transcript` under `key`; HMAC takes a key of any length, so this is `None`
/// only if the `hmac` crate refuses one.
pub(crate) fn mac(key: &[u8; 32], transcript: &[u8]) -> Option<[u8; 32]> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).ok()?;
    mac.update(transcript);
    Some(mac.finalize().into_bytes().into())
}

/// Whether `tag` is the MAC of `transcript` under `key`, compared in constant time.
pub(crate) fn verify(key: &[u8; 32], transcript: &[u8], tag: &[u8; 32]) -> bool {
    mac(key, transcript).is_some_and(|expected| bool::from(expected.ct_eq(tag)))
}
