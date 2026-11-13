//! Binding a per-request key, and the values sealed to it, to the identity that holds them, so a collector seals only to a key a peer
//! it already knows stands behind, and a receiver opens only what that peer sent.
//!
//! A binding proves, for one request, that the holder of an identity made a statement. Two proofs cover the peer identity methods:
//! an ECDSA signature (P-256, SHA-256, IEEE P1363, low s) by an identity that signs, and a Diffie-Hellman proof (HMAC under a key
//! derived from the two X25519 static keys) by one that cannot, such as a Noise static key. The identity's private key is never
//! handed to this crate: a [`Prover`] calls a signer or a key agreement the caller supplies, which is how a platform key store or a
//! WebCrypto key is used.

use super::error::Error;
use super::{b64u, b64u_decode};
use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::str::FromStr;
use core::time::Duration;
use hmac::{Hmac, KeyInit, Mac};
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The binding method for an identity that signs: ECDSA over P-256 and SHA-256, the signature in IEEE P1363 form (`r` then `s`, 64
/// bytes, `s` low), as the `webcrypto` peer method signs. The key is the identity's SPKI.
pub const BINDING_ECDSA: &str = "ecdsa-p256-sha256";
/// The binding method for an X25519 identity: HMAC-SHA-256 under a key derived from the Diffie-Hellman of the holder's static key
/// and the collector's, so only those two can make it. The key is the identity's SPKI.
pub const BINDING_DH: &str = "x25519-hmac-sha256";

const KEY_LABEL: &[u8] = b"dev.boriken.coqui.acp/secret/v1 binding";
const VALUES_LABEL: &[u8] = b"dev.boriken.coqui.acp/secret/v1 values";
const DH_LABEL: &[u8] = b"dev.boriken.coqui.acp/secret/v1 dh";

/// `SubjectPublicKeyInfo` of a P-256 key before its 65-byte uncompressed point.
pub(crate) const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];
/// `SubjectPublicKeyInfo` of an X25519 key before its 32 bytes (RFC 8410).
pub(crate) const X25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x03, 0x21, 0x00,
];

/// An identity as the peer methods name it: the SHA-256 of its public key's encoding (its `SubjectPublicKeyInfo`), written
/// `sha256:` and 64 lower-case hexadecimal digits. A proof's bytes are not an identifier of anything: an ECDSA signature may be
/// re-encoded by someone who has seen it, so nothing may key a table or a replay set by it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdentityId([u8; 32]);

impl IdentityId {
    /// The id of the public key whose encoding is `spki`.
    #[must_use]
    pub fn of(spki: &[u8]) -> Self {
        Self(Sha256::digest(spki).into())
    }

    /// The id as its 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for IdentityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", data_encoding::HEXLOWER.encode(&self.0))
    }
}

impl fmt::Debug for IdentityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IdentityId({self})")
    }
}

impl FromStr for IdentityId {
    type Err = Error;

    /// Reads `sha256:` and 64 hexadecimal digits, either case.
    fn from_str(text: &str) -> Result<Self, Error> {
        const WHY: &str = "an identity is sha256: and 64 hexadecimal digits";
        let hex = text.strip_prefix("sha256:").ok_or(Error::Malformed(WHY))?;
        let bytes = data_encoding::HEXLOWER_PERMISSIVE
            .decode(hex.as_bytes())
            .map_err(|_| Error::Malformed(WHY))?;
        Ok(Self(bytes.try_into().map_err(|_| Error::Malformed(WHY))?))
    }
}

/// Which of the extension's two methods a request is. It is bound into every sealed field and every key binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// The person is asked for secrets: the host receives them.
    Request,
    /// The person is shown secrets: the frontend receives them.
    Show,
}

impl Method {
    /// `request` or `show`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Show => "show",
        }
    }
}

/// What a binding is a statement about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The receiver's per-request public key.
    Key,
    /// The sealed values a collector sends back.
    Values,
}

/// A binding as it travels.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Binding {
    /// [`BINDING_ECDSA`] or [`BINDING_DH`]; any other is refused.
    pub method: String,
    /// The holder's public key as `SubjectPublicKeyInfo`. Its [`IdentityId`] is what a collector looks up.
    pub key: Vec<u8>,
    /// When the binding ends, in Unix seconds.
    pub expires: u64,
    /// The identity the binding was made for. A verifier refuses a binding made for anyone but itself.
    pub audience: IdentityId,
    /// The signature or the tag.
    pub proof: Vec<u8>,
}

impl Binding {
    /// The binding as JSON: `{"method", "key", "expires", "audience", "proof"}`, bytes in unpadded base64url.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("method".into(), Value::String(self.method.clone()));
        map.insert("key".into(), Value::String(b64u(&self.key)));
        map.insert("expires".into(), Value::from(self.expires));
        map.insert(
            "audience".into(),
            Value::String(alloc::format!("{}", self.audience)),
        );
        map.insert("proof".into(), Value::String(b64u(&self.proof)));
        Value::Object(map)
    }

    /// Reads a binding.
    ///
    /// # Errors
    /// [`Error::Malformed`] for anything not in the shape `to_value` writes.
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let map = value
            .as_object()
            .ok_or(Error::Malformed("a binding is not an object"))?;
        let text = |key: &str, why| {
            map.get(key)
                .and_then(Value::as_str)
                .ok_or(Error::Malformed(why))
        };
        Ok(Self {
            method: text("method", "a binding has no method")?.into(),
            key: b64u_decode(text("key", "a binding has no key")?)?,
            expires: map
                .get("expires")
                .and_then(Value::as_u64)
                .ok_or(Error::Malformed("a binding has no end"))?,
            audience: text("audience", "a binding has no audience")?.parse()?,
            proof: b64u_decode(text("proof", "a binding has no proof")?)?,
        })
    }
}

fn push(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// The bytes a binding proves: the label of its kind, the method, the request, the digest of what the person is asked, the receiver's
/// key, the payload (the sealed values' digest for [`Kind::Values`]), the end and the audience, each with a 4-byte length.
pub(crate) struct Statement<'a> {
    pub(crate) kind: Kind,
    pub(crate) method: Method,
    pub(crate) request_id: &'a str,
    pub(crate) digest: &'a [u8; 32],
    pub(crate) key: &'a [u8; 32],
    pub(crate) payload: &'a [u8],
}

impl Statement<'_> {
    pub(crate) fn bytes(&self, expires: u64, audience: &IdentityId) -> Vec<u8> {
        let mut out = Vec::new();
        push(
            &mut out,
            if self.kind == Kind::Key {
                KEY_LABEL
            } else {
                VALUES_LABEL
            },
        );
        push(&mut out, self.method.as_str().as_bytes());
        push(&mut out, self.request_id.as_bytes());
        push(&mut out, self.digest);
        push(&mut out, self.key);
        push(&mut out, self.payload);
        push(&mut out, &expires.to_be_bytes());
        push(&mut out, &audience.0);
        out
    }
}

fn x25519_of_spki(spki: &[u8]) -> Result<[u8; 32], Error> {
    spki.strip_prefix(&X25519_SPKI_PREFIX[..])
        .and_then(|key| <[u8; 32]>::try_from(key).ok())
        .ok_or(Error::Key("not an X25519 SubjectPublicKeyInfo"))
}

fn mac(key: &[u8], parts: &[&[u8]]) -> Result<Hmac<Sha256>, Error> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).map_err(|_| Error::Seal)?;
    for part in parts {
        mac.update(part);
    }
    Ok(mac)
}

fn mac_over(key: &[u8], parts: &[&[u8]]) -> Result<Zeroizing<[u8; 32]>, Error> {
    let mac = mac(key, parts)?;
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&mac.finalize().into_bytes());
    Ok(out)
}

/// The MAC key two X25519 static keys share: HKDF-SHA-256 (RFC 5869, no salt, one output block) of their Diffie-Hellman, bound to
/// both public keys, holder first.
fn dh_key(
    shared: &[u8; 32],
    holder: &[u8; 32],
    collector: &[u8; 32],
) -> Result<Zeroizing<[u8; 32]>, Error> {
    if shared.iter().fold(0u8, |all, byte| all | byte) == 0 {
        return Err(Error::Key("a Diffie-Hellman key of low order"));
    }
    let prk = mac_over(&[0u8; 32], &[shared])?;
    mac_over(&*prk, &[DH_LABEL, holder, collector, &[1]])
}

/// A signer: the 64-byte IEEE P1363 ECDSA P-256 signature of SHA-256 of the message, by the identity's key.
type Signer = Box<dyn Fn(&[u8]) -> Result<[u8; 64], Error> + Send + Sync>;
/// A key agreement: the X25519 Diffie-Hellman of the identity's static key and a peer's public key.
type Agreement = Box<dyn Fn(&[u8; 32]) -> Result<[u8; 32], Error> + Send + Sync>;

/// The holder's side of a binding: an identity that signs or agrees on keys through functions the caller supplies, so the private key
/// can stay in a platform key store, a browser's WebCrypto or a device. Keys held in this process's memory are made by the
/// [`unprotected`](super::unprotected) helpers, which are for tests and tools.
pub struct Prover {
    kind: ProverKind,
}

enum ProverKind {
    Signing {
        spki: Vec<u8>,
        sign: Signer,
    },
    Agreeing {
        spki: Vec<u8>,
        agree: Agreement,
        collector: [u8; 32],
        audience: IdentityId,
    },
}

impl fmt::Debug for Prover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Prover({})", self.id())
    }
}

impl Prover {
    /// An identity that signs. `spki` is its P-256 `SubjectPublicKeyInfo`; `sign` returns the 64-byte IEEE P1363 signature (ECDSA,
    /// SHA-256) of the message it is given, deterministic or randomized, with `s` high or low: the binding takes the low form.
    ///
    /// # Errors
    /// [`Error::Key`] for an `spki` that is no P-256 key.
    pub fn signing(
        spki: impl Into<Vec<u8>>,
        sign: impl Fn(&[u8]) -> Result<[u8; 64], Error> + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        let spki = spki.into();
        p256_key_of_spki(&spki)?;
        Ok(Self {
            kind: ProverKind::Signing {
                spki,
                sign: Box::new(sign),
            },
        })
    }

    /// An X25519 identity, for the one collector whose `SubjectPublicKeyInfo` is `collector_spki` (the peer it is paired with). `agree`
    /// returns the Diffie-Hellman of the identity's static key and the public key it is given.
    ///
    /// # Errors
    /// [`Error::Key`] for an `spki` or a collector that is no X25519 key.
    pub fn agreeing(
        spki: impl Into<Vec<u8>>,
        agree: impl Fn(&[u8; 32]) -> Result<[u8; 32], Error> + Send + Sync + 'static,
        collector_spki: &[u8],
    ) -> Result<Self, Error> {
        let spki = spki.into();
        x25519_of_spki(&spki)?;
        Ok(Self {
            kind: ProverKind::Agreeing {
                spki,
                agree: Box::new(agree),
                collector: x25519_of_spki(collector_spki)?,
                audience: IdentityId::of(collector_spki),
            },
        })
    }

    /// The identity's public key as `SubjectPublicKeyInfo`.
    #[must_use]
    pub fn spki(&self) -> &[u8] {
        match &self.kind {
            ProverKind::Signing { spki, .. } | ProverKind::Agreeing { spki, .. } => spki,
        }
    }

    /// The identity's id.
    #[must_use]
    pub fn id(&self) -> IdentityId {
        IdentityId::of(self.spki())
    }

    /// Makes the binding of `statement`.
    pub(crate) fn bind(
        &self,
        statement: &Statement<'_>,
        expires: u64,
        audience: &IdentityId,
    ) -> Result<Binding, Error> {
        let bytes = statement.bytes(expires, audience);
        match &self.kind {
            ProverKind::Signing { spki, sign } => {
                let signature = Signature::from_slice(&sign(&bytes)?).map_err(|_| Error::Seal)?;
                Ok(Binding {
                    method: BINDING_ECDSA.into(),
                    key: spki.clone(),
                    expires,
                    audience: *audience,
                    proof: signature.normalize_s().to_bytes().to_vec(),
                })
            }
            ProverKind::Agreeing {
                spki,
                agree,
                collector,
                audience: only,
            } => {
                if audience != only {
                    return Err(Error::WrongAudience);
                }
                let shared = Zeroizing::new(agree(collector)?);
                let key = dh_key(&shared, &x25519_of_spki(spki)?, collector)?;
                Ok(Binding {
                    method: BINDING_DH.into(),
                    key: spki.clone(),
                    expires,
                    audience: *only,
                    proof: mac_over(&*key, &[&bytes])?.to_vec(),
                })
            }
        }
    }
}

fn p256_key_of_spki(spki: &[u8]) -> Result<VerifyingKey, Error> {
    spki.strip_prefix(&P256_SPKI_PREFIX[..])
        .filter(|_| spki.len() == 91)
        .and_then(|point| VerifyingKey::from_sec1_bytes(point).ok())
        .ok_or(Error::Key("not a P-256 SubjectPublicKeyInfo"))
}

/// The collector's side: the identities it knows by pairing or pinning, its own id, and its key agreement when it takes
/// [`BINDING_DH`].
pub struct Verifier {
    known: BTreeSet<IdentityId>,
    own: IdentityId,
    agreement: Option<OwnAgreement>,
    max_lifetime: Duration,
    skew: Duration,
    pub(crate) limits: super::Limits,
}

struct OwnAgreement {
    public: [u8; 32],
    agree: Agreement,
}

impl fmt::Debug for Verifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Verifier({}, {} known)", self.own, self.known.len())
    }
}

/// Builds a [`Verifier`].
#[must_use]
pub struct VerifierBuilder {
    verifier: Verifier,
}

impl fmt::Debug for VerifierBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.verifier.fmt(f)
    }
}

impl VerifierBuilder {
    /// Knows an identity by its id: one the person paired or pinned. A key whose holder is not known is refused, whatever it carries.
    pub fn trust(mut self, id: IdentityId) -> Self {
        self.verifier.known.insert(id);
        self
    }

    /// Knows an identity by its `SubjectPublicKeyInfo`.
    pub fn trust_spki(self, spki: &[u8]) -> Self {
        self.trust(IdentityId::of(spki))
    }

    /// Gives the collector's own X25519 identity, which [`BINDING_DH`] needs: its `SubjectPublicKeyInfo`, which must be the identity
    /// the verifier was built for, and the key agreement. Without it that method is refused.
    ///
    /// # Errors
    /// [`Error::Key`] for an `spki` that is no X25519 key or is not this verifier's identity.
    pub fn agreement(
        mut self,
        spki: &[u8],
        agree: impl Fn(&[u8; 32]) -> Result<[u8; 32], Error> + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        let public = x25519_of_spki(spki)?;
        if IdentityId::of(spki) != self.verifier.own {
            return Err(Error::Key(
                "the key agreement is not this verifier's identity",
            ));
        }
        self.verifier.agreement = Some(OwnAgreement {
            public,
            agree: Box::new(agree),
        });
        Ok(self)
    }

    /// The longest a binding may last from now, 5 minutes by default; any value is taken.
    pub const fn max_lifetime(mut self, lifetime: Duration) -> Self {
        self.verifier.max_lifetime = lifetime;
        self
    }

    /// How far the peer's clock may differ from this one's, 30 seconds by default: a binding is taken that long after it ended and
    /// that much beyond the longest it may last. Any value is taken.
    pub const fn skew(mut self, skew: Duration) -> Self {
        self.verifier.skew = skew;
        self
    }

    /// What forms this verifier takes.
    pub const fn limits(mut self, limits: super::Limits) -> Self {
        self.verifier.limits = limits;
        self
    }

    /// Builds the collector's verifier.
    #[must_use]
    pub fn build(self) -> Verifier {
        self.verifier
    }
}

impl Verifier {
    /// Starts a verifier that is `own` and knows no one. A binding is taken only if it was made for `own`.
    pub fn builder(own: IdentityId) -> VerifierBuilder {
        VerifierBuilder {
            verifier: Self {
                known: BTreeSet::new(),
                own,
                agreement: None,
                max_lifetime: Duration::from_secs(300),
                skew: Duration::from_secs(30),
                limits: super::Limits::new(),
            },
        }
    }

    /// How far a peer's clock may differ from this one's, in seconds.
    pub(crate) fn skew_secs(&self) -> u64 {
        self.skew.as_secs()
    }

    /// This verifier's own identity.
    #[must_use]
    pub const fn id(&self) -> IdentityId {
        self.own
    }

    /// Checks that `binding` makes `statement`'s claim for a known identity at `now` (Unix seconds); returns that identity.
    ///
    /// The order is: the method and the key's shape, whether the holder is known, the end, the audience, and last the proof, so a
    /// peer nobody knows costs one hash.
    pub(crate) fn verify(
        &self,
        statement: &Statement<'_>,
        binding: &Binding,
        now: u64,
    ) -> Result<IdentityId, Error> {
        let ecdsa = match binding.method.as_str() {
            BINDING_ECDSA => true,
            BINDING_DH => false,
            _ => {
                return Err(Error::Unsupported(
                    "a binding method this side does not have",
                ));
            }
        };
        let point = if ecdsa {
            Some(p256_key_of_spki(&binding.key)?)
        } else {
            x25519_of_spki(&binding.key)?;
            None
        };
        let holder = IdentityId::of(&binding.key);
        if !self.known.contains(&holder) {
            return Err(Error::UnknownIdentity);
        }
        let skew = self.skew.as_secs();
        if now > binding.expires.saturating_add(skew)
            || binding.expires
                > now
                    .saturating_add(self.max_lifetime.as_secs())
                    .saturating_add(skew)
        {
            return Err(Error::Expired);
        }
        if binding.audience != self.own {
            return Err(Error::WrongAudience);
        }
        let signed = statement.bytes(binding.expires, &binding.audience);
        if let Some(verifying) = point {
            let signature = Signature::from_slice(&binding.proof).map_err(|_| Error::BadProof)?;
            // One statement has one signature: a high s is the same signature written the other way.
            if signature.normalize_s() != signature {
                return Err(Error::BadProof);
            }
            verifying
                .verify(&signed, &signature)
                .map_err(|_| Error::BadProof)?;
        } else {
            let own = self.agreement.as_ref().ok_or(Error::Unsupported(
                "this collector has no X25519 identity of its own",
            ))?;
            let holder_key = x25519_of_spki(&binding.key)?;
            let shared = Zeroizing::new((own.agree)(&holder_key)?);
            let key = dh_key(&shared, &holder_key, &own.public)?;
            mac(&*key, &[&signed])?
                .verify_slice(&binding.proof)
                .map_err(|_| Error::BadProof)?;
        }
        Ok(holder)
    }
}
