//! Sealing a secret field to a per-request key, and opening it, by HPKE (RFC 9180) in base mode with DHKEM(X25519, HKDF-SHA256),
//! HKDF-SHA256 and AES-128-GCM. The `info` of each field names the extension, the method, the request, the digest of what the
//! person was asked, and the field, so a ciphertext opens only for the field and request it was made for. The sealed values are
//! then bound, as a set, to the identity that sent them.

use super::binding::{Binding, IdentityId, Kind, Method, Prover, Statement, Verifier};
use super::context::{Context, plain_values};
use super::error::Error;
use super::form::{Form, Limits};
use super::{b64u, b64u_decode};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::time::Duration;
use hpke::aead::{AeadTag, AesGcm128};
use hpke::inout::InOutBuf;
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable, Kem as KemTrait, OpModeR, OpModeS, Serializable};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

type Kem = X25519HkdfSha256;
type PrivateKey = <Kem as KemTrait>::PrivateKey;
type PublicKey = <Kem as KemTrait>::PublicKey;
type EncappedKey = <Kem as KemTrait>::EncappedKey;

const INFO_LABEL: &[u8] = b"dev.boriken.coqui.acp/secret/v1 field";
const VALUES_DIGEST_LABEL: &[u8] = b"dev.boriken.coqui.acp/secret/v1 values digest";
const TAG_BYTES: usize = 16;

/// How long a receiver's binding lasts by default: well under the 5 minutes a collector accepts, so a binding made now still
/// verifies at a peer whose clock is behind by the default skew.
pub const DEFAULT_LIFETIME: Duration = Duration::from_secs(240);

/// A sealed field as it travels: HPKE's encapsulated key and the ciphertext with its tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sealed {
    /// The encapsulated key, 32 bytes.
    pub enc: [u8; 32],
    /// The ciphertext followed by its 16-byte tag.
    pub ct: Vec<u8>,
}

impl Sealed {
    /// `{"enc", "ct"}`, bytes in unpadded base64url.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("enc".into(), Value::String(b64u(&self.enc)));
        map.insert("ct".into(), Value::String(b64u(&self.ct)));
        Value::Object(map)
    }

    /// Reads a sealed field.
    ///
    /// # Errors
    /// [`Error::Malformed`] for anything else.
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let map = value
            .as_object()
            .filter(|map| map.len() == 2)
            .ok_or(Error::Malformed("a sealed value is not {enc, ct}"))?;
        let part = |key: &str| {
            map.get(key)
                .and_then(Value::as_str)
                .ok_or(Error::Malformed("a sealed value is not {enc, ct}"))
                .and_then(b64u_decode)
        };
        let ct = part("ct")?;
        if ct.len() < TAG_BYTES {
            return Err(Error::Malformed("a sealed value is shorter than its tag"));
        }
        Ok(Self {
            enc: part("enc")?
                .try_into()
                .map_err(|_| Error::Malformed("a sealed value's key is not 32 bytes"))?,
            ct,
        })
    }
}

/// A secret value held for its use: zeroed when dropped, in no `Debug` and no `Display`.
pub struct SecretValue(Zeroizing<Vec<u8>>);

impl SecretValue {
    /// The value as bytes, UTF-8 text for a text field and decimal text for a number.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// The value as text; it was checked to be UTF-8 when it was opened.
    #[must_use]
    pub fn expose_str(&self) -> &str {
        core::str::from_utf8(&self.0).unwrap_or_default()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(..)")
    }
}

/// The receiver's per-request public key with its binding: what the collector is sent, and checks, before it seals.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Recipient {
    /// The X25519 public key the fields are sealed to.
    pub key: [u8; 32],
    /// Why the collector may believe the key is the receiver's, for this request and what the person is asked.
    pub binding: Binding,
}

impl Recipient {
    /// `{"key", "binding"}`.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("key".into(), Value::String(b64u(&self.key)));
        map.insert("binding".into(), self.binding.to_value());
        Value::Object(map)
    }

    /// Reads a recipient.
    ///
    /// # Errors
    /// [`Error::Malformed`].
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let map = value
            .as_object()
            .ok_or(Error::Malformed("a recipient is not an object"))?;
        let key = map
            .get("key")
            .and_then(Value::as_str)
            .ok_or(Error::Malformed("a recipient has no key"))?;
        Ok(Self {
            key: b64u_decode(key)?
                .try_into()
                .map_err(|_| Error::Malformed("a recipient's key is not 32 bytes"))?,
            binding: Binding::from_value(
                map.get("binding")
                    .ok_or(Error::Malformed("a recipient has no binding"))?,
            )?,
        })
    }
}

/// What the receiver gets back: the plain answers, and each secret one opened.
#[derive(Debug)]
#[non_exhaustive]
pub struct Opened {
    /// The plain questions' answers, as the form allows them.
    pub plain: Map<String, Value>,
    /// The secret questions' values.
    pub secrets: BTreeMap<String, SecretValue>,
}

fn lp(out: &mut Vec<u8>, part: &[u8]) {
    out.extend_from_slice(&(part.len() as u32).to_be_bytes());
    out.extend_from_slice(part);
}

/// The `info` of one field: the extension, the method, the request, the digest of what was asked, and the field, each with a
/// 4-byte length.
pub(crate) fn info(method: Method, request_id: &str, digest: &[u8; 32], field: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for part in [
        INFO_LABEL,
        method.as_str().as_bytes(),
        request_id.as_bytes(),
        digest,
        field.as_bytes(),
    ] {
        lp(&mut out, part);
    }
    out
}

/// The digest of a whole answer: the plain values as the JSON object text that was sent, then the sealed values in the order of their
/// names. It is what the sender's binding covers.
pub(crate) fn values_digest(plain: &str, sealed: &BTreeMap<String, Sealed>) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update((VALUES_DIGEST_LABEL.len() as u32).to_be_bytes());
    hash.update(VALUES_DIGEST_LABEL);
    hash.update((plain.len() as u32).to_be_bytes());
    hash.update(plain.as_bytes());
    for (name, value) in sealed {
        hash.update((name.len() as u32).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update(value.enc);
        hash.update((value.ct.len() as u32).to_be_bytes());
        hash.update(&value.ct);
    }
    hash.finalize().into()
}

fn seal_buffer(
    value: &[u8],
    seal: impl FnOnce(&mut [u8]) -> Result<(EncappedKey, AeadTag<AesGcm128>), Error>,
) -> Result<Sealed, Error> {
    let mut buffer = Zeroizing::new(value.to_vec());
    let (enc, tag) = seal(&mut buffer)?;
    let mut ct = buffer.to_vec();
    ct.extend_from_slice(tag.to_bytes().as_slice());
    Ok(Sealed {
        enc: enc
            .to_bytes()
            .as_slice()
            .try_into()
            .map_err(|_| Error::Seal)?,
        ct,
    })
}

/// Seals `value` to `key` for `info`, from the system's randomness.
fn seal(key: &[u8; 32], info: &[u8], value: &[u8]) -> Result<Sealed, Error> {
    let pk = PublicKey::from_bytes(key).map_err(|_| Error::Key("not an X25519 public key"))?;
    seal_buffer(value, |buffer| {
        hpke::single_shot_seal_inout_detached::<AesGcm128, HkdfSha256, Kem>(
            &OpModeS::Base,
            &pk,
            info,
            InOutBuf::from(buffer),
            &[],
        )
        .map_err(|_| Error::Seal)
    })
}

/// Opens `sealed` with `private` for `info`.
fn open(private: &PrivateKey, info: &[u8], sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>, Error> {
    let enc = EncappedKey::from_bytes(&sealed.enc).map_err(|_| Error::Open)?;
    let split = sealed.ct.len().checked_sub(TAG_BYTES).ok_or(Error::Open)?;
    let tag = AeadTag::<AesGcm128>::from_bytes(&sealed.ct[split..]).map_err(|_| Error::Open)?;
    let mut buffer = Zeroizing::new(sealed.ct[..split].to_vec());
    hpke::single_shot_open_inout_detached::<AesGcm128, HkdfSha256, Kem>(
        &OpModeR::Base,
        private,
        &enc,
        info,
        InOutBuf::from(&mut buffer[..]),
        &[],
        &tag,
    )
    .map_err(|_| Error::Open)?;
    Ok(buffer)
}

/// What a context must be besides its id and its schema: a layout that changes nothing a secret question is, and plain values only in
/// a show, which fit the form.
fn check_context(method: Method, context: &Context, form: &Form) -> Result<(), Error> {
    if let Some(layout) = &context.form {
        form.check_layout(layout)?;
    }
    match (method, &context.values) {
        (Method::Show, Some(values)) => form.check_plain(&plain_values(values)?),
        (Method::Show, None) => Err(Error::Malformed("a show has no values")),
        (Method::Request, Some(_)) => Err(Error::Malformed("a request carries no values")),
        (Method::Request, None) => Ok(()),
    }
}

fn check_id(request_id: &str) -> Result<(), Error> {
    if request_id.is_empty() || request_id.len() > 128 || request_id.chars().any(char::is_control) {
        return Err(Error::Malformed(
            "a request id is 1 to 128 bytes with no control character",
        ));
    }
    Ok(())
}

/// The receiving side of one request: it holds the per-request private key, and gives the collector the public key with a binding.
/// The host is the receiver of a `request`, the frontend of a `show`. It opens one answer: [`Receiver::open_values`] consumes it, and
/// whoever owns a receiver remembers the request id for as long as a binding can verify (the verifier's `max_lifetime` and `skew`)
/// so that the same request is not answered twice.
pub struct Receiver {
    method: Method,
    request_id: String,
    digest: [u8; 32],
    form: Form,
    private: PrivateKey,
    recipient: Recipient,
}

impl fmt::Debug for Receiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Receiver")
            .field("method", &self.method)
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}

impl Receiver {
    /// Makes a key pair for one request and binds the public key to `prover`'s identity, for what `context` asks, until `now` plus
    /// `lifetime` (Unix seconds; [`DEFAULT_LIFETIME`]), for the collector `audience`.
    ///
    /// # Errors
    /// [`Error::Schema`] for a schema that is no secret form under `limits`; [`Error::Malformed`] for a request id that is empty,
    /// long or holds a control character; [`Error::WrongAudience`] for an audience a Diffie-Hellman identity was not paired with;
    /// [`Error::Seal`] when the binding cannot be made.
    pub fn new(
        method: Method,
        context: &Context,
        limits: &Limits,
        prover: &Prover,
        audience: &IdentityId,
        now: u64,
        lifetime: Duration,
    ) -> Result<Self, Error> {
        let (private, public) = Kem::gen_keypair();
        Self::from_key_pair(
            method, context, limits, prover, audience, now, lifetime, private, &public,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_key_pair(
        method: Method,
        context: &Context,
        limits: &Limits,
        prover: &Prover,
        audience: &IdentityId,
        now: u64,
        lifetime: Duration,
        private: PrivateKey,
        public: &PublicKey,
    ) -> Result<Self, Error> {
        check_id(&context.request_id)?;
        let form = Form::parse_text(&context.schema, limits)?;
        check_context(method, context, &form)?;
        let digest = context.digest(method);
        let key: [u8; 32] = public
            .to_bytes()
            .as_slice()
            .try_into()
            .map_err(|_| Error::Seal)?;
        let statement = Statement {
            kind: Kind::Key,
            method,
            request_id: &context.request_id,
            digest: &digest,
            key: &key,
            payload: &[],
        };
        let binding = prover.bind(&statement, now.saturating_add(lifetime.as_secs()), audience)?;
        Ok(Self {
            method,
            request_id: context.request_id.clone(),
            digest,
            form,
            private,
            recipient: Recipient { key, binding },
        })
    }

    /// What to send the collector.
    #[must_use]
    pub const fn recipient(&self) -> &Recipient {
        &self.recipient
    }

    /// The request this receiver is for.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Opens one secret field.
    pub(crate) fn open_field(&self, name: &str, sealed: &Sealed) -> Result<SecretValue, Error> {
        if !self.form.field(name).is_some_and(|field| field.secret) {
            return Err(Error::Field("not a secret question of the form"));
        }
        let plain = open(
            &self.private,
            &info(self.method, &self.request_id, &self.digest, name),
            sealed,
        )?;
        self.form.check_secret(name, &plain)?;
        Ok(SecretValue(plain))
    }

    /// Opens an answer, once: checks that `sender`, an identity `verifier` knows, sent this answer (`auth` binds the plain values, as
    /// the JSON object text that was sent, and the sealed ones, to this request and key), checks the plain values against the form, and
    /// opens every sealed one. Nothing is read or opened before the sender is verified. For a show, `plain` is the text of the open
    /// step's values.
    ///
    /// # Errors
    /// The refusals of the binding ([`Error::UnknownIdentity`] for a sender other than `sender`, [`Error::BadProof`] for sealed
    /// values that are not the ones `auth` covers, [`Error::Expired`], [`Error::WrongAudience`], ...); [`Error::Field`] for a name the
    /// form lacks, a plain value for a secret question, a sealed one for a plain question, a required question left out or a value that
    /// does not fit; [`Error::Open`] for a ciphertext made for another request, field or key, or damaged.
    pub fn open_values(
        self,
        verifier: &Verifier,
        sender: &IdentityId,
        plain: &str,
        sealed: &BTreeMap<String, Sealed>,
        auth: &Binding,
        now: u64,
    ) -> Result<Opened, Error> {
        // The binding it made is over; a later answer is refused whatever its sender proves.
        if now
            > self
                .recipient
                .binding
                .expires
                .saturating_add(verifier.skew_secs())
        {
            return Err(Error::Expired);
        }
        self.form.check_sealed(sealed.keys().map(String::as_str))?;
        let payload = values_digest(plain, sealed);
        let statement = Statement {
            kind: Kind::Values,
            method: self.method,
            request_id: &self.request_id,
            digest: &self.digest,
            key: &self.recipient.key,
            payload: &payload,
        };
        if verifier.verify(&statement, auth, now)? != *sender {
            return Err(Error::UnknownIdentity);
        }
        let plain = plain_values(plain)?;
        self.form.check_plain(&plain)?;
        let secrets = sealed
            .iter()
            .map(|(name, value)| Ok((name.clone(), self.open_field(name, value)?)))
            .collect::<Result<BTreeMap<_, _>, Error>>()?;
        Ok(Opened { plain, secrets })
    }
}

/// The collecting side of one request, after it checked the receiver's key: it seals each secret value to that key.
#[derive(Debug)]
pub struct Collector {
    method: Method,
    request_id: String,
    digest: [u8; 32],
    form: Form,
    key: [u8; 32],
    holder: IdentityId,
}

impl Verifier {
    /// Checks what the person is asked and the receiver's key and its binding at `now` (Unix seconds), and returns the collector that
    /// seals to it. The binding covers the digest of `context` as this side received it, so a form changed on the way is refused here
    /// before it is drawn. Only `expected`, the identity this request must come from, is taken: another that this verifier knows is
    /// refused.
    ///
    /// # Errors
    /// The refusals of the binding ([`Error::UnknownIdentity`], [`Error::BadProof`], [`Error::Expired`], [`Error::WrongAudience`], ...)
    /// and [`Error::Schema`] for a form this verifier's limits do not take.
    pub fn collector(
        &self,
        expected: &IdentityId,
        method: Method,
        context: &Context,
        recipient: &Recipient,
        now: u64,
    ) -> Result<Collector, Error> {
        check_id(&context.request_id)?;
        let digest = context.digest(method);
        let statement = Statement {
            kind: Kind::Key,
            method,
            request_id: &context.request_id,
            digest: &digest,
            key: &recipient.key,
            payload: &[],
        };
        let holder = self.verify(&statement, &recipient.binding, now)?;
        if holder != *expected {
            return Err(Error::UnknownIdentity);
        }
        PublicKey::from_bytes(&recipient.key)
            .map_err(|_| Error::Key("not an X25519 public key"))?;
        let form = Form::parse_text(&context.schema, &self.limits)?;
        check_context(method, context, &form)?;
        Ok(Collector {
            method,
            request_id: context.request_id.clone(),
            digest,
            form,
            key: recipient.key,
            holder,
        })
    }
}

impl Collector {
    /// The identity that stands behind the key.
    #[must_use]
    pub const fn holder(&self) -> IdentityId {
        self.holder
    }

    /// The form this collector draws or sends, read from the context its binding covers.
    #[must_use]
    pub const fn form(&self) -> &Form {
        &self.form
    }

    /// Seals the value of secret field `name`: UTF-8 text, or a number as decimal text. The ciphertext is as long as the value plus 48
    /// bytes (the encapsulated key and the tag), so its length tells a relay how long the secret is; nothing is padded.
    ///
    /// # Errors
    /// [`Error::Field`] for a name that is no secret question or a value that does not fit it; [`Error::Seal`].
    pub fn seal(&self, name: &str, value: &str) -> Result<Sealed, Error> {
        self.form.check_secret(name, value.as_bytes())?;
        seal(
            &self.key,
            &info(self.method, &self.request_id, &self.digest, name),
            value.as_bytes(),
        )
    }

    /// Binds the whole answer to `prover`'s identity for this request and key until `now` plus `lifetime`: what the receiver verifies
    /// before it reads or opens anything. `plain` is the JSON object text of the plain values, exactly as it will be sent (for a show,
    /// the open step's values), and `sealed` is every sealed value of the answer.
    ///
    /// # Errors
    /// [`Error::Field`] for an answer the form does not allow; [`Error::WrongAudience`] for a Diffie-Hellman identity not paired with
    /// the receiver's; [`Error::Seal`].
    pub fn authenticate(
        &self,
        prover: &Prover,
        plain: &str,
        sealed: &BTreeMap<String, Sealed>,
        now: u64,
        lifetime: Duration,
    ) -> Result<Binding, Error> {
        // A sender signs only an answer the form allows: a plain value for a secret question, or a required secret left out, is
        // refused here, before anything is signed.
        self.form.check_plain(&plain_values(plain)?)?;
        self.form.check_sealed(sealed.keys().map(String::as_str))?;
        let payload = values_digest(plain, sealed);
        let statement = Statement {
            kind: Kind::Values,
            method: self.method,
            request_id: &self.request_id,
            digest: &self.digest,
            key: &self.key,
            payload: &payload,
        };
        prover.bind(
            &statement,
            now.saturating_add(lifetime.as_secs()),
            &self.holder,
        )
    }

    /// Whether the field is one this collector would seal, without sealing anything.
    #[must_use]
    pub fn is_secret(&self, name: &str) -> bool {
        self.form.field(name).is_some_and(|field| field.secret)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use hpke::rand_core::{TryCryptoRng, TryRng};

    /// Randomness that reads off the bytes given, as RFC 9180's vectors fix them.
    pub(crate) struct Fixed<'a>(pub(crate) &'a [u8]);

    impl TryRng for Fixed<'_> {
        type Error = core::convert::Infallible;

        fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
            unreachable!("only bytes are asked for")
        }

        fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
            unreachable!("only bytes are asked for")
        }

        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Self::Error> {
            let (taken, rest) = self.0.split_at(dest.len());
            dest.copy_from_slice(taken);
            self.0 = rest;
            Ok(())
        }
    }

    impl TryCryptoRng for Fixed<'_> {}

    pub(crate) fn key_pair(ikm: &[u8]) -> (PrivateKey, PublicKey) {
        Kem::derive_keypair(ikm)
    }

    pub(crate) fn seal_raw(
        key: &[u8; 32],
        info: &[u8],
        value: &[u8],
        aad: &[u8],
        ikm: &[u8],
    ) -> Sealed {
        let pk = PublicKey::from_bytes(key).unwrap();
        seal_buffer(value, |buffer| {
            hpke::single_shot_seal_inout_detached_with_rng::<AesGcm128, HkdfSha256, Kem>(
                &OpModeS::Base,
                &pk,
                info,
                InOutBuf::from(buffer),
                aad,
                &mut Fixed(ikm),
            )
            .map_err(|_| Error::Seal)
        })
        .unwrap()
    }

    pub(crate) fn open_raw(
        private: &PrivateKey,
        info: &[u8],
        aad: &[u8],
        sealed: &Sealed,
    ) -> Result<Vec<u8>, Error> {
        let enc = EncappedKey::from_bytes(&sealed.enc).map_err(|_| Error::Open)?;
        let split = sealed.ct.len() - TAG_BYTES;
        let tag = AeadTag::<AesGcm128>::from_bytes(&sealed.ct[split..]).map_err(|_| Error::Open)?;
        let mut buffer = sealed.ct[..split].to_vec();
        hpke::single_shot_open_inout_detached::<AesGcm128, HkdfSha256, Kem>(
            &OpModeR::Base,
            private,
            &enc,
            info,
            InOutBuf::from(&mut buffer[..]),
            aad,
            &tag,
        )
        .map_err(|_| Error::Open)?;
        Ok(buffer)
    }

    pub(crate) fn seal_with(
        key: &[u8; 32],
        method: Method,
        request_id: &str,
        digest: &[u8; 32],
        field: &str,
        value: &[u8],
        ikm: &[u8],
    ) -> Sealed {
        seal_raw(
            key,
            &info(method, request_id, digest, field),
            value,
            &[],
            ikm,
        )
    }

    pub(crate) fn open_with(
        private: &PrivateKey,
        method: Method,
        request_id: &str,
        digest: &[u8; 32],
        field: &str,
        sealed: &Sealed,
    ) -> Result<Vec<u8>, Error> {
        open_raw(
            private,
            &info(method, request_id, digest, field),
            &[],
            sealed,
        )
    }
}
