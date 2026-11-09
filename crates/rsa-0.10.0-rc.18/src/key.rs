use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};

use crypto_bigint::{
    modular::{BoxedMontyForm, BoxedMontyParams},
    BoxedUint, ConcatenatingMul, Integer, NonZero, Odd, Resize,
};
use rand_core::CryptoRng;
use zeroize::{Zeroize, ZeroizeOnDrop};
#[cfg(feature = "serde")]
use {
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
    serdect::serde::{de, ser, Deserialize, Serialize},
    spki::{DecodePublicKey, EncodePublicKey},
};

use crate::algorithms::generate::generate_multi_prime_key_with_exp;
use crate::algorithms::rsa::{
    compute_modulus, compute_private_exponent_carmicheal, compute_private_exponent_euler_totient,
    recover_primes,
};

use crate::dummy_rng::DummyRng;
use crate::errors::{Error, Result};
use crate::traits::keys::{CrtValue, PrivateKeyParts, PublicKeyParts};
use crate::traits::{PaddingScheme, SignatureScheme};

/// Represents the public part of an RSA key.
#[derive(Debug, Clone)]
pub struct RsaPublicKey {
    /// Modulus: product of prime numbers `p` and `q`
    n: NonZero<BoxedUint>,
    /// Public exponent: power to which a plaintext message is raised in
    /// order to encrypt it.
    ///
    /// Typically `0x10001` (`65537`)
    e: BoxedUint,

    n_params: BoxedMontyParams,
}

impl Eq for RsaPublicKey {}

impl PartialEq for RsaPublicKey {
    #[inline]
    fn eq(&self, other: &RsaPublicKey) -> bool {
        self.n == other.n && self.e == other.e
    }
}

impl Hash for RsaPublicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Domain separator for RSA private keys
        state.write(b"RsaPublicKey");
        // TODO(tarcieri): to match the `PartialEq` impl we should strip leading zeros
        Hash::hash(&self.n.as_limbs(), state);
        Hash::hash(&self.e.as_limbs(), state);
    }
}

/// Represents a whole RSA key, public and private parts.
#[derive(Clone)]
pub struct RsaPrivateKey {
    /// Public components of the private key.
    pubkey_components: RsaPublicKey,
    /// Private exponent
    pub(crate) d: BoxedUint,
    /// Prime factors of N, contains >= 2 elements.
    pub(crate) primes: Vec<BoxedUint>,
    /// Precomputed values to speed up private operations
    pub(crate) precomputed: Option<PrecomputedValues>,
}

impl fmt::Debug for RsaPrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let precomputed = if self.precomputed.is_some() {
            "Some(...)"
        } else {
            "None"
        };
        f.debug_struct("RsaPrivateKey")
            .field("pubkey_components", &self.pubkey_components)
            .field("d", &"...")
            .field("primes", &"&[...]")
            .field("precomputed", &precomputed)
            .finish()
    }
}

impl Eq for RsaPrivateKey {}
impl PartialEq for RsaPrivateKey {
    #[inline]
    fn eq(&self, other: &RsaPrivateKey) -> bool {
        self.pubkey_components == other.pubkey_components
            && self.d == other.d
            && self.primes == other.primes
    }
}

impl AsRef<RsaPublicKey> for RsaPrivateKey {
    fn as_ref(&self) -> &RsaPublicKey {
        &self.pubkey_components
    }
}

impl Hash for RsaPrivateKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Domain separator for RSA private keys
        state.write(b"RsaPrivateKey");
        Hash::hash(&self.pubkey_components, state);
    }
}

impl Drop for RsaPrivateKey {
    fn drop(&mut self) {
        self.d.zeroize();
        self.primes.zeroize();
        self.precomputed.zeroize();
    }
}

impl ZeroizeOnDrop for RsaPrivateKey {}

#[derive(Clone)]
pub(crate) struct PrecomputedValues {
    /// D mod (P-1)
    pub(crate) dp: BoxedUint,
    /// D mod (Q-1)
    pub(crate) dq: BoxedUint,
    /// Q^-1 mod P
    pub(crate) qinv: BoxedMontyForm,

    /// Montgomery params for `p`
    pub(crate) p_params: BoxedMontyParams,
    /// Montgomery params for `q`
    pub(crate) q_params: BoxedMontyParams,
}

impl ZeroizeOnDrop for PrecomputedValues {}

impl Zeroize for PrecomputedValues {
    fn zeroize(&mut self) {
        self.dp.zeroize();
        self.dq.zeroize();
        // TODO: once these have landed in crypto-bigint
        // self.p_params.zeroize();
        // self.q_params.zeroize();
    }
}

impl Drop for PrecomputedValues {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl From<RsaPrivateKey> for RsaPublicKey {
    fn from(private_key: RsaPrivateKey) -> Self {
        (&private_key).into()
    }
}

impl From<&RsaPrivateKey> for RsaPublicKey {
    fn from(private_key: &RsaPrivateKey) -> Self {
        let n = PublicKeyParts::n(private_key);
        let e = PublicKeyParts::e(private_key);
        let n_params = PublicKeyParts::n_params(private_key);
        RsaPublicKey {
            n: n.clone(),
            e: e.clone(),
            n_params: n_params.clone(),
        }
    }
}

impl PublicKeyParts for RsaPublicKey {
    fn n(&self) -> &NonZero<BoxedUint> {
        &self.n
    }

    fn e(&self) -> &BoxedUint {
        &self.e
    }

    fn n_params(&self) -> &BoxedMontyParams {
        &self.n_params
    }
}

impl RsaPublicKey {
    /// Encrypt the given message.
    pub fn encrypt<R: CryptoRng + ?Sized, P: PaddingScheme>(
        &self,
        rng: &mut R,
        padding: P,
        msg: &[u8],
    ) -> Result<Vec<u8>> {
        padding.encrypt(rng, self, msg)
    }

    /// Verify a signed message.
    ///
    /// `hashed` must be the result of hashing the input using the hashing function
    /// passed in through `hash`.
    ///
    /// If the message is valid `Ok(())` is returned, otherwise an `Err` indicating failure.
    pub fn verify<S: SignatureScheme>(&self, scheme: S, hashed: &[u8], sig: &[u8]) -> Result<()> {
        scheme.verify(self, hashed, sig)
    }
}

impl RsaPublicKey {
    /// Minimum value of the public exponent `e`.
    pub const MIN_PUB_EXPONENT: u64 = 2;

    /// Maximum value of the public exponent `e`.
    pub const MAX_PUB_EXPONENT: u64 = (1 << 33) - 1;

    /// Maximum size of the modulus `n` in bits.
    pub const MAX_SIZE: usize = 8192;

    /// Create a new public key from its components.
    ///
    /// This function accepts public keys with a modulus size up to 8192-bits,
    /// i.e. [`RsaPublicKey::MAX_SIZE`].
    pub fn new(n: BoxedUint, e: BoxedUint) -> Result<Self> {
        Self::new_with_max_size(n, e, Self::MAX_SIZE)
    }

    /// Create a new public key from its components.
    pub fn new_with_max_size(n: BoxedUint, e: BoxedUint, max_size: usize) -> Result<Self> {
        check_public_with_max_size(&n, &e, Some(max_size))?;

        let n_odd = Odd::new(n.clone())
            .into_option()
            .ok_or(Error::InvalidModulus)?;
        let n_params = BoxedMontyParams::new(n_odd);
        let n = NonZero::new(n).expect("checked above");

        Ok(Self { n, e, n_params })
    }

    /// Create a new public key, bypassing checks around the modulus and public
    /// exponent size.
    ///
    /// This method is not recommended, and only intended for unusual use cases.
    /// Most applications should use [`RsaPublicKey::new`] or
    /// [`RsaPublicKey::new_with_max_size`] instead.
    pub fn new_unchecked(n: BoxedUint, e: BoxedUint) -> Self {
        let n_odd = Odd::new(n.clone()).expect("n must be odd");
        let n_params = BoxedMontyParams::new(n_odd);
        let n = NonZero::new(n).expect("odd numbers are non zero");

        Self { n, e, n_params }
    }
}

impl PublicKeyParts for RsaPrivateKey {
    fn n(&self) -> &NonZero<BoxedUint> {
        &self.pubkey_components.n
    }

    fn e(&self) -> &BoxedUint {
        &self.pubkey_components.e
    }

    fn n_params(&self) -> &BoxedMontyParams {
        &self.pubkey_components.n_params
    }
}

impl RsaPrivateKey {
    /// Default exponent for RSA keys.
    const EXP: u64 = 65537;

    /// Minimum size of the modulus `n` in bits. Currently only applies to keygen.
    const MIN_SIZE: u32 = 1024;

    /// Generate a new RSA key pair with a modulus of the given bit size using the passed in `rng`.
    ///
    /// # Errors
    /// - If `bit_size` is lower than the minimum 1024-bits.
    pub fn new<R: CryptoRng + ?Sized>(rng: &mut R, bit_size: usize) -> Result<Self> {
        Self::new_with_exp(rng, bit_size, Self::EXP.into())
    }

    /// Generate a new RSA key pair of the given bit size.
    ///
    /// #⚠️Warning: Hazmat!
    /// This version does not apply minimum key size checks, and as such may generate keys
    /// which are insecure!
    #[cfg(feature = "hazmat")]
    pub fn new_unchecked<R: CryptoRng + ?Sized>(rng: &mut R, bit_size: usize) -> Result<Self> {
        Self::new_with_exp_unchecked(rng, bit_size, Self::EXP.into())
    }

    /// Generate a new RSA key pair of the given bit size and the public exponent
    /// using the passed in `rng`.
    ///
    /// Unless you have specific needs, you should use [`RsaPrivateKey::new`] instead.
    pub fn new_with_exp<R: CryptoRng + ?Sized>(
        rng: &mut R,
        bit_size: usize,
        exp: BoxedUint,
    ) -> Result<RsaPrivateKey> {
        if bit_size < Self::MIN_SIZE as usize {
            return Err(Error::ModulusTooSmall);
        }

        let components = generate_multi_prime_key_with_exp(rng, 2, bit_size, exp)?;
        RsaPrivateKey::from_components(
            components.n.get(),
            components.e,
            components.d,
            components.primes,
        )
    }

    /// Generate a new RSA key pair of the given bit size and the public exponent
    /// using the passed in `rng`.
    ///
    /// Unless you have specific needs, you should use [`RsaPrivateKey::new`] instead.
    ///
    /// #⚠️Warning: Hazmat!
    /// This version does not apply minimum key size checks, and as such may generate keys
    /// which are insecure!
    #[cfg(feature = "hazmat")]
    pub fn new_with_exp_unchecked<R: CryptoRng + ?Sized>(
        rng: &mut R,
        bit_size: usize,
        exp: BoxedUint,
    ) -> Result<RsaPrivateKey> {
        let components = generate_multi_prime_key_with_exp(rng, 2, bit_size, exp)?;
        RsaPrivateKey::from_components(
            components.n.get(),
            components.e,
            components.d,
            components.primes,
        )
    }

    /// Private helper function that constructs an RSA key pair from components
    /// WITHOUT performing any validation or precomputation.
    ///
    /// This is the shared implementation used by `from_components` and
    /// `from_components_with_large_exponent`.
    ///
    /// Callers are responsible for:
    /// 1. Validating the key (to ensure precomputation won't fail)
    /// 2. Calling precompute() after validation
    fn from_components_inner(
        n: BoxedUint,
        e: BoxedUint,
        d: BoxedUint,
        mut primes: Vec<BoxedUint>,
    ) -> Result<RsaPrivateKey> {
        let n = Odd::new(n).into_option().ok_or(Error::InvalidModulus)?;

        // The modulus may come in padded with zeros, shorten it
        // to ensure optimal performance of arithmetic operations.
        let n_bits = n.bits_vartime();
        let n = n.resize_unchecked(n_bits);

        let n_params = BoxedMontyParams::new(n.clone());
        let n_c = NonZero::new(n.get())
            .into_option()
            .ok_or(Error::InvalidModulus)?;

        match primes.len() {
            0 => {
                // Recover `p` and `q` from `d`.
                // See method in Appendix C.2: https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-56Br2.pdf
                let (p, q) = recover_primes(&n_c, &e, &d)?;
                primes.push(p);
                primes.push(q);
            }
            1 => return Err(Error::NprimesTooSmall),
            _ => {
                // Check that the product of primes matches the modulus.
                // This also ensures that `bit_precision` of each prime is <= that of the modulus,
                // and `bit_precision` of their product is >= that of the modulus.
                if primes
                    .iter()
                    .fold(BoxedUint::one(), |acc, p| acc.concatenating_mul(&p))
                    != n_c.as_ref()
                {
                    return Err(Error::InvalidModulus);
                }
            }
        }

        // The primes may come in padded with zeros too, so we need to shorten them as well.
        let primes = primes
            .into_iter()
            .map(|p| {
                let p_bits = p.bits();
                p.resize_unchecked(p_bits)
            })
            .collect();

        let k = RsaPrivateKey {
            pubkey_components: RsaPublicKey {
                n: n_c,
                e,
                n_params,
            },
            d,
            primes,
            precomputed: None,
        };

        Ok(k)
    }

    /// Constructs an RSA key pair from individual components, accepting exponents outside
    /// the normal size bounds.
    ///
    /// See [`RsaPrivateKey::from_components`] for an explanation on the parameters.
    ///
    /// # ⚠️ Warning: Hazmat!
    ///
    /// This method accepts public exponents outside the standard bounds (2 ≤ e ≤ 2^33-1),
    /// but still performs full cryptographic validation to ensure the key is mathematically
    /// correct (i.e., verifies that de ≡ 1 mod λ(n)).
    ///
    /// **Note:** This method is dangerous as it can be used as a DOS vector if used with
    /// untrusted input https://www.imperialviolet.org/2012/03/17/rsados.html
    ///
    /// This is intended for interoperating with systems that use non-standard exponents
    /// or loading legacy keys. Use [`RsaPrivateKey::from_components`] for standard key
    /// construction.
    #[cfg(feature = "hazmat")]
    pub fn from_components_with_large_exponent(
        n: BoxedUint,
        e: BoxedUint,
        d: BoxedUint,
        primes: Vec<BoxedUint>,
    ) -> Result<RsaPrivateKey> {
        let mut k = Self::from_components_inner(n, e, d, primes)?;

        // Validate everything except exponent size bounds (to ensure precompute can't fail)
        validate_skip_exponent_size(&k)?;

        // Precompute when possible, ignore error otherwise.
        k.precompute().ok();

        Ok(k)
    }

    /// Constructs an RSA key pair from individual components:
    ///
    /// - `n`: RSA modulus
    /// - `e`: public exponent (i.e. encrypting exponent)
    /// - `d`: private exponent (i.e. decrypting exponent)
    /// - `primes`: prime factors of `n`: typically two primes `p` and `q`. More than two primes can
    ///   be provided for multiprime RSA, however this is generally not recommended. If no `primes`
    ///   are provided, a prime factor recovery algorithm will be employed to attempt to recover the
    ///   factors (as described in [NIST SP 800-56B Revision 2] Appendix C.2). This algorithm only
    ///   works if there are just two prime factors `p` and `q` (as opposed to multiprime), and `e`
    ///   is between 2^16 and 2^256.
    ///
    ///  [NIST SP 800-56B Revision 2]: https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-56Br2.pdf
    pub fn from_components(
        n: BoxedUint,
        e: BoxedUint,
        d: BoxedUint,
        primes: Vec<BoxedUint>,
    ) -> Result<RsaPrivateKey> {
        let mut k = Self::from_components_inner(n, e, d, primes)?;

        // Always validate the key, to ensure precompute can't fail
        k.validate()?;

        // Precompute when possible, ignore error otherwise.
        k.precompute().ok();

        Ok(k)
    }

    /// Constructs an RSA key pair from its two primes p and q.
    ///
    /// This will rebuild the private exponent and the modulus.
    ///
    /// Private exponent will be rebuilt using the method defined in
    /// [NIST 800-56B Section 6.2.1](https://nvlpubs.nist.gov/nistpubs/SpecialPublications/NIST.SP.800-56Br2.pdf#page=47).
    pub fn from_p_q(
        p: BoxedUint,
        q: BoxedUint,
        public_exponent: BoxedUint,
    ) -> Result<RsaPrivateKey> {
        if p == q {
            return Err(Error::InvalidPrime);
        }

        let d = compute_private_exponent_carmicheal(&p, &q, &public_exponent)?;
        let primes = vec![p, q];
        let n = compute_modulus(&primes);

        Self::from_components(n.get(), public_exponent, d, primes)
    }

    /// Constructs an RSA key pair from its primes.
    ///
    /// This will rebuild the private exponent and the modulus.
    pub fn from_primes(
        primes: Vec<BoxedUint>,
        public_exponent: BoxedUint,
    ) -> Result<RsaPrivateKey> {
        if primes.len() < 2 {
            return Err(Error::NprimesTooSmall);
        }

        // Makes sure that the primes are pairwise unequal.
        for (i, prime1) in primes.iter().enumerate() {
            for prime2 in primes.iter().take(i) {
                if prime1 == prime2 {
                    return Err(Error::InvalidPrime);
                }
            }
        }

        let n = compute_modulus(&primes);
        let d = compute_private_exponent_euler_totient(&primes, &public_exponent)?;

        Self::from_components(n.get(), public_exponent, d, primes)
    }

    /// Get the public key from the private key.
    ///
    /// Specific alternative to [`AsRef::as_ref`].
    pub fn as_public_key(&self) -> &RsaPublicKey {
        &self.pubkey_components
    }

    /// Get the public key from the private key, cloning `n` and `e`.
    ///
    /// Generally this is not needed since `RsaPrivateKey` implements the `PublicKey` trait,
    /// but it can occasionally be useful to discard the private information entirely.
    pub fn to_public_key(&self) -> RsaPublicKey {
        self.pubkey_components.clone()
    }

    /// Performs some calculations to speed up private key operations.
    pub fn precompute(&mut self) -> Result<()> {
        if self.precomputed.is_some() {
            return Ok(());
        }

        let d = &self.d;
        let p = self.primes[0].clone();
        let q = self.primes[1].clone();

        let p_odd = Odd::new(p.clone())
            .into_option()
            .ok_or(Error::InvalidPrime)?;
        let p_params = BoxedMontyParams::new(p_odd);
        let q_odd = Odd::new(q.clone())
            .into_option()
            .ok_or(Error::InvalidPrime)?;
        let q_params = BoxedMontyParams::new(q_odd);

        let x = NonZero::new(p.wrapping_sub(BoxedUint::one()))
            .into_option()
            .ok_or(Error::InvalidPrime)?;
        let dp = d.rem_vartime(&x);

        let x = NonZero::new(q.wrapping_sub(BoxedUint::one()))
            .into_option()
            .ok_or(Error::InvalidPrime)?;
        let dq = d.rem_vartime(&x);

        // Note that since `p` and `q` may have different `bits_precision`,
        // so we have to equalize them to calculate the remainder.
        let q_mod_p = match p.bits_precision().cmp(&q.bits_precision()) {
            Ordering::Less => (&q
                % NonZero::new(p.clone())
                    .expect("`p` is non-zero")
                    .resize_unchecked(q.bits_precision()))
            .resize_unchecked(p.bits_precision()),
            Ordering::Greater => {
                (&q).resize_unchecked(p.bits_precision())
                    % &NonZero::new(p.clone()).expect("`p` is non-zero")
            }
            Ordering::Equal => &q % NonZero::new(p.clone()).expect("`p` is non-zero"),
        };

        let q_mod_p = BoxedMontyForm::new(q_mod_p, &p_params);
        let qinv = q_mod_p.invert().into_option().ok_or(Error::InvalidPrime)?;

        debug_assert_eq!(dp.bits_precision(), p.bits_precision());
        debug_assert_eq!(dq.bits_precision(), q.bits_precision());
        debug_assert_eq!(qinv.bits_precision(), p.bits_precision());
        debug_assert_eq!(p_params.bits_precision(), p.bits_precision());
        debug_assert_eq!(q_params.bits_precision(), q.bits_precision());

        self.precomputed = Some(PrecomputedValues {
            dp,
            dq,
            qinv,
            p_params,
            q_params,
        });

        Ok(())
    }

    /// Clears precomputed values by setting to None
    pub fn clear_precomputed(&mut self) {
        self.precomputed = None;
    }

    /// Compute CRT coefficient: `(1/q) mod p`.
    pub fn crt_coefficient(&self) -> Option<BoxedUint> {
        let p = &self.primes[0];
        let q = &self.primes[1];
        // TODO: maybe store primes as `NonZero`?
        Option::from(q.invert_mod(&NonZero::new(p.clone()).expect("prime")))
    }

    /// Performs basic sanity checks on the key.
    /// Returns `Ok(())` if everything is good, otherwise an appropriate error.
    pub fn validate(&self) -> Result<()> {
        check_public(self)?;
        validate_private_key_parts(self)?;
        Ok(())
    }

    /// Decrypt the given message.
    pub fn decrypt<P: PaddingScheme>(&self, padding: P, ciphertext: &[u8]) -> Result<Vec<u8>> {
        padding.decrypt(Option::<&mut DummyRng>::None, self, ciphertext)
    }

    /// Decrypt the given message.
    ///
    /// Uses `rng` to blind the decryption process.
    pub fn decrypt_blinded<R: CryptoRng + ?Sized, P: PaddingScheme>(
        &self,
        rng: &mut R,
        padding: P,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>> {
        padding.decrypt(Some(rng), self, ciphertext)
    }

    /// Sign the given digest.
    pub fn sign<S: SignatureScheme>(&self, padding: S, digest_in: &[u8]) -> Result<Vec<u8>> {
        padding.sign(Option::<&mut DummyRng>::None, self, digest_in)
    }

    /// Sign the given digest using the provided `rng`, which is used in the
    /// following ways depending on the [`SignatureScheme`]:
    ///
    /// - [`Pkcs1v15Sign`][`crate::Pkcs1v15Sign`] padding: uses the RNG
    ///   to mask the private key operation with random blinding, which helps
    ///   mitigate sidechannel attacks.
    /// - [`Pss`][`crate::Pss`] always requires randomness. Use
    ///   [`Pss::new`][`crate::Pss::new`] for a standard RSASSA-PSS signature, or
    ///   [`Pss::new_blinded`][`crate::Pss::new_blinded`] for RSA-BSSA blind
    ///   signatures.
    pub fn sign_with_rng<R: CryptoRng + ?Sized, S: SignatureScheme>(
        &self,
        rng: &mut R,
        padding: S,
        digest_in: &[u8],
    ) -> Result<Vec<u8>> {
        padding.sign(Some(rng), self, digest_in)
    }
}

impl PrivateKeyParts for RsaPrivateKey {
    fn d(&self) -> &BoxedUint {
        &self.d
    }

    fn primes(&self) -> &[BoxedUint] {
        &self.primes
    }

    fn dp(&self) -> Option<&BoxedUint> {
        self.precomputed.as_ref().map(|p| &p.dp)
    }

    fn dq(&self) -> Option<&BoxedUint> {
        self.precomputed.as_ref().map(|p| &p.dq)
    }

    fn qinv(&self) -> Option<&BoxedMontyForm> {
        self.precomputed.as_ref().map(|p| &p.qinv)
    }

    fn crt_values(&self) -> Option<&[CrtValue]> {
        None
    }

    fn p_params(&self) -> Option<&BoxedMontyParams> {
        self.precomputed.as_ref().map(|p| &p.p_params)
    }

    fn q_params(&self) -> Option<&BoxedMontyParams> {
        self.precomputed.as_ref().map(|p| &p.q_params)
    }
}

/// Check that the public key is well formed and has an exponent within acceptable bounds.
#[inline]
pub fn check_public(public_key: &impl PublicKeyParts) -> Result<()> {
    check_public_with_max_size(public_key.n(), public_key.e(), None)
}

/// Check that the public key is well formed and has an exponent within acceptable bounds.
#[inline]
fn check_public_with_max_size(n: &BoxedUint, e: &BoxedUint, max_size: Option<usize>) -> Result<()> {
    if let Some(max_size) = max_size {
        if n.bits_vartime() as usize > max_size {
            return Err(Error::ModulusTooLarge);
        }
    }

    check_public_skip_exponent_size(n, e)?;

    if e < &BoxedUint::from(RsaPublicKey::MIN_PUB_EXPONENT) {
        return Err(Error::PublicExponentTooSmall);
    }

    if e > &BoxedUint::from(RsaPublicKey::MAX_PUB_EXPONENT) {
        return Err(Error::PublicExponentTooLarge);
    }

    Ok(())
}

/// Check that the public key is well formed, skipping exponent size bounds checks.
///
/// This is used internally by both public validation functions and hazmat APIs.
#[inline]
fn check_public_skip_exponent_size(n: &BoxedUint, e: &BoxedUint) -> Result<()> {
    if e >= n || n.is_even().into() || n.is_zero().into() {
        return Err(Error::InvalidModulus);
    }

    if e.is_even().into() {
        return Err(Error::InvalidExponent);
    }

    // Skip exponent size bounds checks
    Ok(())
}

/// Helper function that validates the private key structure and cryptographic correctness.
///
/// This performs the structural and mathematical validation checks that are common to both
/// `validate()` and `validate_skip_exponent_size()`.
fn validate_private_key_parts(key: &RsaPrivateKey) -> Result<()> {
    // Check that Πprimes == n.
    let mut m = BoxedUint::one_with_precision(key.pubkey_components.n.bits_precision());
    let one = BoxedUint::one();
    for prime in &key.primes {
        // Any primes ≤ 1 will cause divide-by-zero panics later.
        if prime <= &one {
            return Err(Error::InvalidPrime);
        }
        m = m.wrapping_mul(prime);
    }
    if m != *key.pubkey_components.n {
        return Err(Error::InvalidModulus);
    }

    // Check that de ≡ 1 mod p-1, for each prime.
    // This implies that e is coprime to each p-1 as e has a multiplicative
    // inverse. Therefore e is coprime to lcm(p-1,q-1,r-1,...) =
    // exponent(ℤ/nℤ). It also implies that a^de ≡ a mod p as a^(p-1) ≡ 1
    // mod p. Thus a^de ≡ a mod n for all a coprime to n, as required.
    let de = key.d.concatenating_mul(&key.pubkey_components.e);

    for prime in &key.primes {
        let x = NonZero::new(prime.wrapping_sub(BoxedUint::one())).unwrap();
        let congruence = de.rem_vartime(&x);
        if !bool::from(congruence.is_one()) {
            return Err(Error::InvalidExponent);
        }
    }

    Ok(())
}

/// Validate the private key structure and cryptographic correctness,
/// skipping only the exponent size bounds checks.
///
/// This performs all the same checks as `RsaPrivateKey::validate()` except
/// it doesn't verify that the exponent is within the standard bounds.
#[cfg(feature = "hazmat")]
fn validate_skip_exponent_size(key: &RsaPrivateKey) -> Result<()> {
    // Check public key properties (without exponent size checks)
    check_public_skip_exponent_size(key.pubkey_components.n.as_ref(), &key.pubkey_components.e)?;

    // Perform common private key validation
    validate_private_key_parts(key)?;

    Ok(())
}

#[cfg(feature = "serde")]
impl Serialize for RsaPublicKey {
    fn serialize<S>(&self, serializer: S) -> core::prelude::v1::Result<S::Ok, S::Error>
    where
        S: serdect::serde::Serializer,
    {
        let der = self.to_public_key_der().map_err(ser::Error::custom)?;
        serdect::slice::serialize_hex_lower_or_bin(&der, serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for RsaPublicKey {
    fn deserialize<D>(deserializer: D) -> core::prelude::v1::Result<Self, D::Error>
    where
        D: serdect::serde::Deserializer<'de>,
    {
        let der_bytes = serdect::slice::deserialize_hex_or_bin_vec(deserializer)?;
        Self::from_public_key_der(&der_bytes).map_err(de::Error::custom)
    }
}

#[cfg(feature = "serde")]
impl Serialize for RsaPrivateKey {
    fn serialize<S>(&self, serializer: S) -> core::prelude::v1::Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        let der = self.to_pkcs8_der().map_err(ser::Error::custom)?;
        serdect::slice::serialize_hex_lower_or_bin(&der.as_bytes(), serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for RsaPrivateKey {
    fn deserialize<D>(deserializer: D) -> core::prelude::v1::Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let der_bytes = serdect::slice::deserialize_hex_or_bin_vec(deserializer)?;
        Self::from_pkcs8_der(&der_bytes).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithms::rsa::{rsa_decrypt_and_check, rsa_encrypt};
    use crate::traits::{PrivateKeyParts, PublicKeyParts};

    use hex_literal::hex;
    use rand::rngs::ChaCha8Rng;
    use rand_core::SeedableRng;

    #[cfg(feature = "encoding")]
    use pkcs8::DecodePrivateKey;

    #[test]
    fn test_from_into() {
        let raw_n = BoxedUint::from(101u64);
        let n_odd = Odd::new(raw_n.clone()).unwrap();
        let private_key = RsaPrivateKey {
            pubkey_components: RsaPublicKey {
                n: NonZero::new(raw_n.clone()).unwrap(),
                e: BoxedUint::from(200u64),
                n_params: BoxedMontyParams::new(n_odd),
            },
            d: BoxedUint::from(123u64),
            primes: vec![],
            precomputed: None,
        };
        let public_key: RsaPublicKey = private_key.into();

        let n_limbs: &[u64] = PublicKeyParts::n(&public_key).as_ref().as_ref();
        assert_eq!(n_limbs, &[101u64]);
        assert_eq!(PublicKeyParts::e(&public_key), &BoxedUint::from(200u64));
        assert_eq!(PublicKeyParts::e_bytes(&public_key), [200].into());
        assert_eq!(PublicKeyParts::n_bytes(&public_key), [101].into());
    }

    fn test_key_basics(private_key: &RsaPrivateKey) {
        private_key.validate().expect("invalid private key");

        assert!(
            PrivateKeyParts::d(private_key) < PublicKeyParts::n(private_key).as_ref(),
            "private exponent too large"
        );

        let pub_key: RsaPublicKey = private_key.clone().into();
        let m = BoxedUint::from(42u64);
        let c = rsa_encrypt(&pub_key, &m).expect("encryption successful");

        let m2 = rsa_decrypt_and_check::<ChaCha8Rng>(private_key, None, &c)
            .expect("unable to decrypt without blinding");
        assert_eq!(m, m2);
        let mut rng = ChaCha8Rng::from_seed([42; 32]);
        let m3 = rsa_decrypt_and_check(private_key, Some(&mut rng), &c)
            .expect("unable to decrypt with blinding");
        assert_eq!(m, m3);
    }

    macro_rules! key_generation {
        ($name:ident, $multi:expr, $size:expr) => {
            #[test]
            fn $name() {
                let mut rng = ChaCha8Rng::from_seed([42; 32]);
                let exp = BoxedUint::from(RsaPrivateKey::EXP);

                for _ in 0..10 {
                    let components =
                        generate_multi_prime_key_with_exp(&mut rng, $multi, $size, exp.clone())
                            .unwrap();
                    let private_key = RsaPrivateKey::from_components(
                        components.n.get(),
                        components.e,
                        components.d,
                        components.primes,
                    )
                    .unwrap();
                    assert_eq!(PublicKeyParts::n(&private_key).bits(), $size);

                    test_key_basics(&private_key);
                }
            }
        };
    }

    key_generation!(key_generation_128, 2, 128);
    key_generation!(key_generation_1024, 2, 1024);

    key_generation!(key_generation_multi_3_256, 3, 256);

    key_generation!(key_generation_multi_4_64, 4, 64);

    key_generation!(key_generation_multi_5_64, 5, 64);
    key_generation!(key_generation_multi_8_576, 8, 576);
    key_generation!(key_generation_multi_16_1024, 16, 1024);

    #[test]
    fn test_negative_decryption_value() {
        let bits = 128;
        let private_key = RsaPrivateKey::from_components(
            BoxedUint::from_le_slice(
                &[
                    99, 192, 208, 179, 0, 220, 7, 29, 49, 151, 75, 107, 75, 73, 200, 180,
                ],
                bits,
            )
            .unwrap(),
            BoxedUint::from_le_slice(&[1, 0, 1, 0, 0, 0, 0, 0], 64).unwrap(),
            BoxedUint::from_le_slice(
                &[
                    81, 163, 254, 144, 171, 159, 144, 42, 244, 133, 51, 249, 28, 12, 63, 65,
                ],
                bits,
            )
            .unwrap(),
            vec![
                BoxedUint::from_le_slice(&[105, 101, 60, 173, 19, 153, 3, 192], bits / 2).unwrap(),
                BoxedUint::from_le_slice(&[235, 65, 160, 134, 32, 136, 6, 241], bits / 2).unwrap(),
            ],
        )
        .unwrap();

        for _ in 0..1000 {
            test_key_basics(&private_key);
        }
    }

    #[test]
    #[cfg(all(feature = "hazmat", feature = "serde"))]
    fn test_serde() {
        use rand::rngs::ChaCha8Rng;
        use rand_core::SeedableRng;
        use serde_test::{assert_tokens, Configure, Token};

        let mut rng = ChaCha8Rng::from_seed([42; 32]);
        let priv_key = RsaPrivateKey::new_unchecked(&mut rng, 64).expect("failed to generate key");

        let priv_tokens = [Token::Str(concat!(
            "3056020100300d06092a864886f70d010101050004423040020100020900a",
            "b240c3361d02e370203010001020811e54a15259d22f9020500ceff5cf302",
            "0500d3a7aaad020500ccaddf17020500cb529d3d020500bb526d6f"
        ))];
        assert_tokens(&priv_key.clone().readable(), &priv_tokens);

        let priv_tokens = [Token::Str(
            "3024300d06092a864886f70d01010105000313003010020900ab240c3361d02e370203010001",
        )];
        assert_tokens(
            &RsaPublicKey::from(priv_key.clone()).readable(),
            &priv_tokens,
        );
    }

    #[test]
    fn invalid_coeff_private_key_regression() {
        use base64ct::{Base64, Encoding};

        let n = Base64::decode_vec(
            "wC8GyQvTCZOK+iiBR5fGQCmzRCTWX9TQ3aRG5gGFk0wB6EFoLMAyEEqeG3gS8xhA\
             m2rSWYx9kKufvNat3iWlbSRVqkcbpVAYlj2vTrpqDpJl+6u+zxFYoUEBevlJJkAh\
             l8EuCccOA30fVpcfRvXPTtvRd3yFT9E9EwZljtgSI02w7gZwg7VIxaGeajh5Euz6\
             ZVQZ+qNRKgXrRC7gPRqVyI6Dt0Jc+Su5KBGNn0QcPDzOahWha1ieaeMkFisZ9mdp\
             sJoZ4tw5eicLaUomKzALHXQVt+/rcZSrCd6/7uUo11B/CYBM4UfSpwXaL88J9AE6\
             A5++no9hmJzaF2LLp+Qwx4yY3j9TDutxSAjsraxxJOGZ3XyA9nG++Ybt3cxZ5fP7\
             ROjxCfROBmVv5dYn0O9OBIqYeCH6QraNpZMadlLNIhyMv8Y+P3r5l/PaK4VJaEi5\
             pPosnEPawp0W0yZDzmjk2z1LthaRx0aZVrAjlH0Rb/6goLUQ9qu1xsDtQVVpN4A8\
             9ZUmtTWORnnJr0+595eHHxssd2gpzqf4bPjNITdAEuOCCtpvyi4ls23zwuzryUYj\
             cUOEnsXNQ+DrZpLKxdtsD/qNV/j1hfeyBoPllC3cV+6bcGOFcVGbjYqb+Kw1b0+j\
             L69RSKQqgmS+qYqr8c48nDRxyq3QXhR8qtzUwBFSLVk=",
        )
        .unwrap();
        let e = Base64::decode_vec("AQAB").unwrap();
        let d = Base64::decode_vec(
            "qQazSQ+FRN7nVK1bRsROMRB8AmsDwLVEHivlz1V3Td2Dr+oW3YUMgxedhztML1Id\
             QJPq/ad6qErJ6yRFNySVIjDaxzBTOEoB1eHa1btOnBJWb8rVvvjaorixvJ6Tn3i4\
             EuhsvVy9DoR1k4rGj3qSIiFjUVvLRDAbLyhpGgEfsr0Z577yJmTC5E8JLRMOKX8T\
             mxsk3jPVpsgd65Hu1s8S/ZmabwuHCf9SkdMeY/1bd/9i7BqqJeeDLE4B5x1xcC3z\
             3scqDUTzqGO+vZPhjgprPDRlBamVwgenhr7KwCn8iaLamFinRVwOAag8BeBqOJj7\
             lURiOsKQa9FIX1kdFUS1QMQxgtPycLjkbvCJjriqT7zWKsmJ7l8YLs6Wmm9/+QJR\
             wNCEVdMTXKfCP1cJjudaiskEQThfUldtgu8gUDNYbQ/Filb2eKfiX4h1TiMxZqUZ\
             HVZyb9nShbQoXJ3vj/MGVF0QM8TxhXM8r2Lv9gDYU5t9nQlUMLhs0jVjai48jHAB\
             bFNyH3sEcOmJOIwJrCXw1dzG7AotwyaEVUHOmL04TffmwCFfnyrLjbFgnyOeoyII\
             BYjcY7QFRm/9nupXMTH5hZ2qrHfCJIp0KK4tNBdQqmnHapFl5l6Le1s4qBS5bEIz\
             jitobLvAFm9abPlDGfxmY6mlrMK4+nytwF9Ct7wc1AE=",
        )
        .unwrap();
        let primes = [
            Base64::decode_vec(
                "9kQWEAzsbzOcdPa+s5wFfw4XDd7bB1q9foZ31b1+TNjGNxbSBCFlDF1q98vwpV6n\
                 M8bWDh/wtbNoETSQDgpEnYOQ26LWEw6YY1+q1Q2GGEFceYUf+Myk8/vTc8TN6Zw0\
                 bKZBWy10Qo8h7xk4JpzuI7NcxvjJYTkS9aErFxi3vVH0aiZC0tmfaCqr8a2rJxyV\
                 wqreRpOjwAWrotMsf2wGsF4ofx5ScoFy5GB5fJkkdOrW1LyTvZAUCX3cstPr19+T\
                 NC5zZOk7WzZatnCkN5H5WzalWtZuu0oVL205KPOa3R8V2yv5e6fm0v5fTmqSuvjm\
                 aMJLXCN4QJkmIzojO99ckQ==",
            )
            .unwrap(),
            Base64::decode_vec(
                "x8exdMjVA2CiI+Thx7loHtVcevoeE2sZ7btRVAvmBqo+lkHwxb7FHRnWvuj6eJSl\
                 D2f0T50EewIhhiW3R9BmktCk7hXjbSCnC1u9Oxc1IAUm/7azRqyfCMx43XhLxpD+\
                 xkBCpWkKDLxGczsRwTuaP3lKS3bSdBrNlGmdblubvVBIq4YZ2vXVlnYtza0cS+dg\
                 CK7BGTqUsrCUd/ZbIvwcwZkZtpkhj1KQfto9X/0OMurBzAqbkeq1cyRHXHkOfN/q\
                 bUIIRqr9Ii7Eswf9Vk8xp2O1Nt8nzcYS9PFD12M5eyaeFEkEYfpNMNGuTzp/31oq\
                 VjbpoCxS6vuWAZyADxhISQ==",
            )
            .unwrap(),
            Base64::decode_vec(
                "is7d0LY4HoXszlC2NO7gejkq7XqL4p1W6hZJPYTNx+r37t1CC2n3Vvzg6kNdpRix\
                 DhIpXVTLjN9O7UO/XuqSumYKJIKoP52eb4Tg+a3hw5Iz2Zsb5lUTNSLgkQSBPAf7\
                 1LHxbL82JL4g1nBUog8ae60BwnVArThKY4EwlJguGNw09BAU4lwf6csDl/nX2vfV\
                 wiAloYpeZkHL+L8m+bueGZM5KE2jEz+7ztZCI+T+E5i69rZEYDjx0lfLKlEhQlCW\
                 3HbCPELqXgNJJkRfi6MP9kXa9lSfnZmoT081RMvqonB/FUa4HOcKyCrw9XZEtnbN\
                 CIdbitfDVEX+pSSD7596wQ==",
            )
            .unwrap(),
            Base64::decode_vec(
                "GPs0injugfycacaeIP5jMa/WX55VEnKLDHom4k6WlfDF4L4gIGoJdekcPEUfxOI5\
                 faKvHyFwRP1wObkPoRBDM0qZxRfBl4zEtpvjHrd5MibSyJkM8+J0BIKk/nSjbRIG\
                 eb3hV5O56PvGB3S0dKhCUnuVObiC+ne7izplsD4OTG70l1Yud33UFntyoMxrxGYL\
                 USqhBMmZfHquJg4NOWOzKNY/K+EcHDLj1Kjvkcgv9Vf7ocsVxvpFdD9uGPceQ6kw\
                 RDdEl6mb+6FDgWuXVyqR9+904oanEIkbJ7vfkthagLbEf57dyG6nJlqh5FBZWxGI\
                 R72YGypPuAh7qnnqXXjY2Q==",
            )
            .unwrap(),
            Base64::decode_vec(
                "CUWC+hRWOT421kwRllgVjy6FYv6jQUcgDNHeAiYZnf5HjS9iK2ki7v8G5dL/0f+Y\
                 f+NhE/4q8w4m8go51hACrVpP1p8GJDjiT09+RsOzITsHwl+ceEKoe56ZW6iDHBLl\
                 rNw5/MtcYhKpjNU9KJ2udm5J/c9iislcjgckrZG2IB8ADgXHMEByZ5DgaMl4AKZ1\
                 Gx8/q6KftTvmOT5rNTMLi76VN5KWQcDWK/DqXiOiZHM7Nr4dX4me3XeRgABJyNR8\
                 Fqxj3N1+HrYLe/zs7LOaK0++F9Ul3tLelhrhsvLxei3oCZkF9A/foD3on3luYA+1\
                 cRcxWpSY3h2J4/22+yo4+Q==",
            )
            .unwrap(),
        ];

        let e = BoxedUint::from_be_slice(&e, 64).unwrap();

        let bits = 4096;
        let n = BoxedUint::from_be_slice(&n, bits).unwrap();
        let d = BoxedUint::from_be_slice(&d, bits).unwrap();
        let primes = primes
            .iter()
            .map(|p| BoxedUint::from_be_slice(p, bits / 2).unwrap())
            .collect();
        let res = RsaPrivateKey::from_components(n, e, d, primes);
        assert_eq!(res, Err(Error::InvalidModulus));
    }

    #[test]
    fn reject_oversized_private_key() {
        // -----BEGIN PUBLIC KEY-----
        // MIIEKjANBgkqhkiG9w0BAQEFAAOCBBcAMIIEEgKCBAkAqQn6O7pd9ioQJEOwS2sh
        // nD2bM3+PaLovro+OKOE9t7jxrp+b9Xq81oeT6zN5u5yPewa+V08ZsAJQEbF9D5AM
        // UZkHZc/sW/XAItC8CojQhHoCQfjOXZpONmGsQxnSJNgwLV5TDVKUApbQIPzIm9yD
        // wOvl1yXIypaRINHzthz36ysHmaHlNVZZQ40BHVkOiUd+ws7W9U9vHN0QcaSHC8lH
        // UEqb/Iyb0FSmZs+qbm4NXyaI90oloAFftOnt8VFbHfT/TXS0VwMyescxFsuvcuTr
        // Xx8EYc9TuJThW22wBAFOK6SpftgtZ6i4WJqk0F8JrTwZ3TyhzKsPRwe8KeNmtmqY
        // oaGiPj9lUOc928QzOyTETVd8pV7UpnaOe9Q4WHL0QmnXn61pCu4qpoLuK8jB+IO7
        // xifRZHj3PMfsjJurZ4MF57LgpSrI60ekYNh1o6ViXODHQuzGxzTaF3n/7GITDBQX
        // DRTlGuQH77hykxFqPclRGI0wxECPKasxpzjhiaTua9eipKedXB+o5XFyosnDt/X4
        // Ygqxj/q2/18LPuQgFLqWRzsHd4TdVQyiq9xCmzIoGUjAPz1Q8cjIXRpUnp2rZQjE
        // SCLeTjewrGNbjSMDUhdF5M2OBRmn7Q8XHHCUxT9fY/BZeydeE54KvEdEkomxkbXo
        // hHKEmbWeEdhp78h04/xW364p1Nu9Y49w7gtO+9nmwKcpNJq32M6Qb0d2dQ3wJ0oI
        // I9ml+n/DTna+IIwwbI8UOFEI4KZQzZaqmNv3TzGmpnocHK7eMyEtAUeQZUIGrPmr
        // FQEmKZn9rkgr/2Hw8T20q7e0lE65Is69vTP2wXm17B5zKFYska41jJoZ6jIpbMOt
        // uVPZV3SoGYM39Z4Ax3JaGZE0L/dQ6lJJhdFUACFIQXwNWqzd7srnvbym4hLqoPuM
        // hjkUtTcv6YODEk7LB2FLDcymmH/zCL3w4VSi4+HyZZ13gM7Cz8WmkX4H+jeL0+Ja
        // QyG1CzqV/HA78vUpJv/bb/J1+X1i/1HltLeTjteY4rBhVT1cxBoVBGQaCwindAs+
        // Fjcp+8cAK+/3pMQghnkrGHzrx9YIYoOGXs4vQIMGngYaTa7aXAaft4fWjg4EeSjd
        // rZwqqrPNuUcEuneFP9RPffj8f3vkhqCFgoVBfVM7ontu2d2nRu/hhAkgT13Uc68J
        // dM2imBvHAo6DDUt6msWCAMOAEXYuO7aA+n3eettnBqtECoQAoCJdCHCebjIploMB
        // XMLXyseGtLK9arI48hDvcxSlf7/1lkBB6LgNQmQJ7925TDipiYQIZ63f4d5Z2JCp
        // W0vUkwzrH4iPb2hy+TBQSOw1kvjLyG/lHWjzDQa60xxVW9u59DxQueHsNEMHUORD
        // 1oFXvFLe/AllAgMBAAE=
        // -----END PUBLIC KEY-----

        let n = BoxedUint::from_be_slice(
            &hex!(
                "a909fa3bba5df62a102443b04b6b219c3d9b337f8f68ba2fae8f8e28e13db7b8
                 f1ae9f9bf57abcd68793eb3379bb9c8f7b06be574f19b0025011b17d0f900c51
                 990765cfec5bf5c022d0bc0a88d0847a0241f8ce5d9a4e3661ac4319d224d830
                 2d5e530d52940296d020fcc89bdc83c0ebe5d725c8ca969120d1f3b61cf7eb2b
                 0799a1e5355659438d011d590e89477ec2ced6f54f6f1cdd1071a4870bc94750
                 4a9bfc8c9bd054a666cfaa6e6e0d5f2688f74a25a0015fb4e9edf1515b1df4ff
                 4d74b45703327ac73116cbaf72e4eb5f1f0461cf53b894e15b6db004014e2ba4
                 a97ed82d67a8b8589aa4d05f09ad3c19dd3ca1ccab0f4707bc29e366b66a98a1
                 a1a23e3f6550e73ddbc4333b24c44d577ca55ed4a6768e7bd4385872f44269d7
                 9fad690aee2aa682ee2bc8c1f883bbc627d16478f73cc7ec8c9bab678305e7b2
                 e0a52ac8eb47a460d875a3a5625ce0c742ecc6c734da1779ffec62130c14170d
                 14e51ae407efb87293116a3dc951188d30c4408f29ab31a738e189a4ee6bd7a2
                 a4a79d5c1fa8e57172a2c9c3b7f5f8620ab18ffab6ff5f0b3ee42014ba96473b
                 077784dd550ca2abdc429b32281948c03f3d50f1c8c85d1a549e9dab6508c448
                 22de4e37b0ac635b8d2303521745e4cd8e0519a7ed0f171c7094c53f5f63f059
                 7b275e139e0abc47449289b191b5e884728499b59e11d869efc874e3fc56dfae
                 29d4dbbd638f70ee0b4efbd9e6c0a729349ab7d8ce906f4776750df0274a0823
                 d9a5fa7fc34e76be208c306c8f14385108e0a650cd96aa98dbf74f31a6a67a1c
                 1caede33212d014790654206acf9ab1501262999fdae482bff61f0f13db4abb7
                 b4944eb922cebdbd33f6c179b5ec1e7328562c91ae358c9a19ea32296cc3adb9
                 53d95774a8198337f59e00c7725a1991342ff750ea524985d154002148417c0d
                 5aacddeecae7bdbca6e212eaa0fb8c863914b5372fe98383124ecb07614b0dcc
                 a6987ff308bdf0e154a2e3e1f2659d7780cec2cfc5a6917e07fa378bd3e25a43
                 21b50b3a95fc703bf2f52926ffdb6ff275f97d62ff51e5b4b7938ed798e2b061
                 553d5cc41a1504641a0b08a7740b3e163729fbc7002beff7a4c42086792b187c
                 ebc7d6086283865ece2f4083069e061a4daeda5c069fb787d68e0e047928ddad
                 9c2aaab3cdb94704ba77853fd44f7df8fc7f7be486a0858285417d533ba27b6e
                 d9dda746efe18409204f5dd473af0974cda2981bc7028e830d4b7a9ac58200c3
                 8011762e3bb680fa7dde7adb6706ab440a8400a0225d08709e6e32299683015c
                 c2d7cac786b4b2bd6ab238f210ef7314a57fbff5964041e8b80d426409efddb9
                 4c38a989840867addfe1de59d890a95b4bd4930ceb1f888f6f6872f9305048ec
                 3592f8cbc86fe51d68f30d06bad31c555bdbb9f43c50b9e1ec34430750e443d6
                 8157bc52defc0965"
            ),
            8256,
        )
        .unwrap();

        let e = BoxedUint::from(65_537u64);

        assert_eq!(
            RsaPublicKey::new(n, e).err().unwrap(),
            Error::ModulusTooLarge
        );
    }

    #[test]
    #[cfg(feature = "encoding")]
    fn build_key_from_primes() {
        const RSA_2048_PRIV_DER: &[u8] = &[0x30u8,0x82,0x04,0xbd,0x02,0x01,0x00,0x30,0x0d,0x06,0x09,0x2a,0x86,0x48,0x86,0xf7,0x0d,0x01,0x01,0x01,0x05,0x00,0x04,0x82,0x04,0xa7,0x30,0x82,0x04,0xa3,0x02,0x01,0x00,0x02,0x82,0x01,0x01,0x00,0xb6,0xc4,0x2c,0x51,0x5f,0x10,0xa6,0xaa,0xf2,0x82,0xc6,0x3e,0xdb,0xe2,0x42,0x43,0xa1,0x70,0xf3,0xfa,0x26,0x33,0xbd,0x48,0x33,0x63,0x7f,0x47,0xca,0x4f,0x6f,0x36,0xe0,0x3a,0x5d,0x29,0xef,0xc3,0x19,0x1a,0xc8,0x0f,0x39,0x0d,0x87,0x4b,0x39,0xe3,0x0f,0x41,0x4f,0xce,0xc1,0xfc,0xa0,0xed,0x81,0xe5,0x47,0xed,0xc2,0xcd,0x38,0x2c,0x76,0xf6,0x1c,0x90,0x18,0x97,0x3d,0xb9,0xfa,0x53,0x79,0x72,0xa7,0xc7,0x01,0xf6,0xb7,0x7e,0x09,0x82,0xdf,0xc1,0x5f,0xc0,0x19,0x27,0xee,0x5e,0x7c,0xd9,0x4b,0x4f,0x59,0x9f,0xf0,0x70,0x13,0xa7,0xc8,0x28,0x1b,0xdf,0x22,0xdc,0xbc,0x9a,0xd7,0xca,0xbb,0x7c,0x43,0x11,0xc9,0x82,0xf5,0x8e,0xdb,0x72,0x13,0xad,0x45,0x58,0xb3,0x32,0x26,0x6d,0x74,0x3a,0xed,0x81,0x92,0xd1,0x88,0x4c,0xad,0xb8,0xb1,0x47,0x39,0xa8,0xda,0xda,0x66,0xdc,0x97,0x08,0x06,0xd9,0xc7,0xac,0x45,0x0c,0xb1,0x3d,0x0d,0x7c,0x57,0x5f,0xb1,0x98,0x53,0x4f,0xc6,0x1b,0xc4,0x1b,0xc0,0xf0,0x57,0x4e,0x0e,0x01,0x30,0xc7,0xbb,0xbf,0xbd,0xfd,0xc9,0xf6,0xa6,0xe2,0xe3,0xe2,0xaf,0xf1,0xcb,0xea,0xc8,0x9b,0xa5,0x78,0x84,0x52,0x8d,0x55,0xcf,0xb0,0x83,0x27,0xa1,0xe8,0xc8,0x9f,0x4e,0x00,0x3c,0xf2,0x88,0x8e,0x93,0x32,0x41,0xd9,0xd6,0x95,0xbc,0xbb,0xac,0xdc,0x90,0xb4,0x4e,0x3e,0x09,0x5f,0xa3,0x70,0x58,0xea,0x25,0xb1,0x3f,0x5e,0x29,0x5c,0xbe,0xac,0x6d,0xe8,0x38,0xab,0x8c,0x50,0xaf,0x61,0xe2,0x98,0x97,0x5b,0x87,0x2f,0x02,0x03,0x01,0x00,0x01,0x02,0x82,0x01,0x00,0x7e,0xcc,0x83,0x62,0xc0,0xed,0xb0,0x74,0x11,0x64,0x21,0x5e,0x22,0xf7,0x4a,0xb9,0xd9,0x1b,0xa0,0x69,0x00,0x70,0x0c,0xf6,0x36,0x90,0xe5,0x11,0x4d,0x8e,0xe6,0xbd,0xcf,0xbb,0x2e,0x3f,0x96,0x14,0x69,0x2a,0x67,0x7a,0x08,0x3f,0x16,0x8a,0x5e,0x52,0xe5,0x96,0x8e,0x64,0x07,0xb9,0xd9,0x7c,0x6e,0x0e,0x40,0x64,0xf8,0x2d,0xa0,0xb7,0x58,0xa1,0x4f,0x17,0xb9,0xb7,0xd4,0x1f,0x5f,0x48,0xe2,0x8d,0x65,0x51,0x70,0x4f,0x56,0xe6,0x9e,0x7a,0xa9,0xfa,0x63,0x0f,0xc7,0x64,0x28,0xc0,0x6d,0x25,0xe4,0x55,0xdc,0xfc,0x55,0xb7,0xac,0x2b,0x4f,0x76,0x64,0x3f,0xde,0xd3,0xfe,0x15,0xff,0x78,0xab,0xb2,0x7e,0x65,0xac,0xc4,0xaa,0xd0,0xbd,0xf6,0xdb,0x27,0xef,0x60,0xa6,0x91,0x0c,0x5c,0x4a,0x08,0x5e,0xd4,0x32,0x75,0xab,0x19,0xc1,0xd9,0x97,0xa3,0x2c,0x6e,0xff,0xce,0x7d,0xf2,0xd1,0x93,0x5f,0x6e,0x60,0x1e,0xed,0xe1,0x61,0xa1,0x2b,0x5c,0xc2,0x7c,0xa2,0x1f,0x81,0xd2,0xc9,0x9c,0x3d,0x1e,0xa0,0x8e,0x90,0xe3,0x05,0x3a,0xb0,0x9b,0xef,0xa7,0x24,0xde,0xf0,0xd0,0xc3,0xa3,0xc1,0xe9,0x74,0x0c,0x0d,0x9f,0x76,0x12,0x6a,0x14,0x9e,0xc0,0xaa,0x7d,0x80,0x78,0x20,0x54,0x84,0x25,0x4d,0x95,0x1d,0xb0,0x7c,0x4c,0xf9,0x1f,0xb6,0x45,0x4c,0x09,0x65,0x88,0xfd,0x59,0x24,0xdb,0xab,0xeb,0x35,0x9c,0xa2,0x02,0x52,0x68,0xd0,0x04,0xf9,0xd6,0x6e,0xb3,0xd6,0xf7,0xad,0xc1,0x13,0x9b,0xad,0x40,0xf1,0x6d,0xde,0x63,0x9e,0x11,0x64,0x73,0x76,0xc1,0x02,0x81,0x81,0x00,0xdc,0xc0,0x61,0x24,0x2d,0x4e,0x92,0xaf,0xae,0xe7,0x2a,0xc5,0x13,0xca,0x65,0xb9,0xf7,0x70,0x36,0xf9,0xbd,0x7e,0x0e,0x6e,0x61,0x46,0x1a,0x7e,0xf7,0x65,0x42,0x25,0xec,0x15,0x3c,0x7e,0x5c,0x31,0xa6,0x15,0x7a,0x6e,0x5a,0x13,0xff,0x6e,0x17,0x8e,0x87,0x58,0xc1,0xcb,0x33,0xd9,0xd6,0xbb,0xe3,0x17,0x9e,0xf1,0x89,0x98,0xe4,0x22,0xec,0xdc,0xbe,0xd7,0x8f,0x4e,0xcf,0xdb,0xe5,0xf4,0xfc,0xd8,0xae,0xc2,0xc9,0xd0,0xdc,0x86,0x47,0x3c,0xa9,0xbd,0x16,0xd9,0xd2,0x38,0xd2,0x1f,0xb5,0xdd,0xef,0xbe,0xb1,0x43,0xca,0x61,0xd0,0xbd,0x6a,0xa8,0xd9,0x1f,0x33,0xa0,0x97,0x79,0x0e,0x96,0x40,0xdb,0xc9,0x10,0x85,0xdc,0x5f,0x26,0x34,0x3b,0xa3,0x13,0x8f,0x6b,0x2d,0x67,0x02,0x81,0x81,0x00,0xd3,0xf3,0x14,0x75,0x7e,0x40,0xe9,0x54,0x83,0x6f,0x92,0xbe,0x24,0x23,0x6a,0xf2,0xf0,0xda,0x04,0xa3,0x46,0x53,0xc1,0x80,0xaf,0x67,0xe9,0x60,0x08,0x6d,0x93,0xfd,0xe6,0x5c,0xb2,0x3e,0xfd,0x9d,0x09,0x37,0x47,0x62,0xf5,0x98,0x1e,0x36,0x18,0x49,0xaf,0x68,0xcd,0xd7,0x53,0x94,0xff,0x6a,0x4e,0x06,0xeb,0x69,0xb2,0x09,0xe4,0x22,0x8d,0xb2,0xdf,0xa7,0x0e,0x40,0xf7,0xf9,0x75,0x0a,0x52,0x81,0x76,0x64,0x7b,0x78,0x8d,0x0e,0x57,0x77,0xa2,0xcb,0x8b,0x22,0xe3,0xcd,0x26,0x7f,0xf7,0x0b,0x4f,0x3b,0x02,0xd3,0xaa,0xfb,0x0e,0x18,0xc5,0x90,0xa5,0x64,0xb0,0x31,0x88,0xb0,0xaa,0x5f,0xc4,0x81,0x56,0xb0,0x76,0x22,0x21,0x42,0x43,0xbd,0x12,0x27,0xef,0xa7,0xf2,0xf9,0x02,0x81,0x81,0x00,0xce,0x68,0xb7,0xac,0x1b,0x0d,0x10,0x0d,0x63,0x6e,0x55,0x48,0x87,0x53,0xc5,0xc0,0x98,0x43,0xfd,0xb3,0x90,0xe2,0x70,0x5d,0xf7,0x68,0x94,0x57,0xc9,0xbd,0x8d,0x97,0x65,0xe3,0x09,0x78,0x61,0x7e,0x2e,0xfc,0x80,0x48,0xf4,0xc3,0x24,0x20,0x6d,0xb8,0x60,0x87,0xb6,0x54,0xe9,0x7b,0xb3,0xd4,0x64,0xe7,0xee,0x3f,0x8c,0xd8,0x3f,0xe1,0x04,0x36,0xf7,0xdf,0x18,0xe9,0xa9,0x63,0xc4,0xe6,0x49,0x11,0xd6,0x7e,0xde,0x34,0x04,0x2f,0x2e,0x26,0xe3,0xd3,0xa1,0xad,0x34,0x6a,0xda,0xd6,0xb9,0xb7,0xf6,0x77,0x08,0xcb,0x09,0x4e,0x62,0xde,0xe9,0xff,0x4d,0x5d,0x66,0x69,0xaf,0x98,0x8a,0xf2,0x25,0x5d,0x1c,0xe8,0xed,0x31,0x7c,0x6a,0x7d,0x86,0x91,0xda,0x35,0x4d,0x12,0xdb,0x02,0x81,0x80,0x25,0xf6,0xe5,0x94,0x42,0x20,0x28,0x6b,0x4d,0xfb,0xbf,0x42,0x35,0xc0,0xee,0x58,0x43,0xd2,0x19,0x80,0x91,0x89,0x51,0x20,0xd6,0xca,0x7b,0x20,0x0b,0x82,0x6d,0x3e,0xce,0x73,0x8e,0x2e,0x00,0x49,0x8f,0xac,0x0a,0x2a,0x6c,0xa9,0x69,0xc7,0xf0,0xc3,0xca,0x1a,0xb0,0xbc,0x40,0x29,0x71,0x32,0xbe,0x75,0x38,0xd7,0xbe,0xdf,0x4c,0xb0,0xef,0xc6,0xb9,0x8e,0xf7,0xdb,0xa5,0x4f,0x56,0xaa,0x99,0xaa,0xbc,0xe5,0x34,0xc4,0x9c,0x27,0x94,0x7d,0x46,0x78,0xc5,0x1c,0x63,0xc7,0x8c,0x7c,0xe1,0x68,0x72,0x31,0xb4,0xc8,0xeb,0x58,0x7a,0xe6,0xef,0x04,0x80,0xcb,0xaf,0x4f,0xc0,0x17,0x3c,0xfd,0x58,0x7a,0x7e,0x67,0xaf,0x51,0x5f,0xb9,0xb9,0xde,0x75,0x11,0x18,0x39,0x72,0x29,0x02,0x81,0x80,0x31,0x99,0x54,0x06,0xd4,0x06,0x20,0x7c,0xad,0xea,0xea,0x35,0xb3,0x8d,0x04,0x0c,0x5f,0x8a,0x9a,0x1a,0xe0,0x82,0x7e,0x9e,0xd0,0x6b,0x15,0x3d,0x83,0xb6,0x82,0x19,0x35,0xb4,0xb3,0x6a,0x82,0xbe,0x9d,0x56,0xc7,0x91,0xb5,0x8c,0x27,0x27,0x1a,0x57,0x93,0xd5,0x3a,0x1d,0x65,0x7c,0x08,0x99,0x79,0x60,0xb1,0x43,0x3e,0x51,0x71,0x98,0x7f,0x45,0x2f,0x14,0x4a,0x7c,0x72,0x30,0x6d,0x63,0xe1,0xd3,0xff,0xc0,0xb7,0x1b,0x75,0xab,0x08,0xf2,0xe4,0x5a,0x48,0x2e,0x98,0x84,0x51,0xcb,0xe4,0x78,0xe1,0x2e,0xb2,0x28,0xd0,0x74,0x56,0xc9,0x24,0xb6,0x6f,0x6c,0xed,0x04,0x8d,0x85,0x3f,0x53,0x3e,0x31,0xa6,0x86,0x14,0xf1,0xc3,0xce,0x6d,0x8e,0xc9,0x98,0x3c,0xe7,0x2a,0xf7];
        let ref_key = RsaPrivateKey::from_pkcs8_der(RSA_2048_PRIV_DER).unwrap();
        assert_eq!(ref_key.validate(), Ok(()));

        let primes = PrivateKeyParts::primes(&ref_key).to_vec();

        let exp = PublicKeyParts::e(&ref_key);
        let key = RsaPrivateKey::from_primes(primes, exp.clone())
            .expect("failed to import key from primes");
        assert_eq!(key.validate(), Ok(()));

        assert_eq!(PublicKeyParts::n(&key), PublicKeyParts::n(&ref_key));

        assert_eq!(PrivateKeyParts::dp(&key), PrivateKeyParts::dp(&ref_key));
        assert_eq!(PrivateKeyParts::dq(&key), PrivateKeyParts::dq(&ref_key));

        assert_eq!(PrivateKeyParts::d(&key), PrivateKeyParts::d(&ref_key));
    }

    #[test]
    #[cfg(feature = "encoding")]
    fn build_key_from_p_q() {
        const RSA_2048_SP800_PRIV_DER: &[u8] =
            &[0x30u8,0x82,0x04,0xbd,0x02,0x01,0x00,0x30,0x0d,0x06,0x09,0x2a,0x86,0x48,0x86,0xf7,0x0d,0x01,0x01,0x01,0x05,0x00,0x04,0x82,0x04,0xa7,0x30,0x82,0x04,0xa3,0x02,0x01,0x00,0x02,0x82,0x01,0x01,0x00,0xa6,0x13,0x2a,0xa6,0xbf,0xe0,0xa0,0x59,0xab,0x4b,0xf3,0xad,0x9b,0x31,0xf8,0x69,0xf9,0x6e,0xd7,0xe6,0x53,0x11,0x93,0x11,0xf5,0x8a,0x1a,0x60,0xc0,0xac,0x11,0xaa,0xbc,0x30,0xa9,0x65,0xfc,0x5a,0xe3,0x34,0x61,0x2c,0xe1,0x3b,0x1f,0x59,0x03,0xfa,0xe8,0xd4,0xa4,0x9a,0x4d,0x3a,0x0f,0x86,0xd9,0x11,0x6a,0xa0,0xa6,0x32,0x39,0x07,0xcb,0x92,0x95,0xcf,0x6b,0xe3,0xb1,0xa0,0xd5,0xfe,0x70,0x01,0x7d,0x32,0x9c,0x08,0x8a,0x4a,0x74,0x18,0x93,0xaf,0xc5,0x58,0xa4,0x7f,0xde,0x7b,0x90,0x27,0x7a,0x14,0x98,0x7b,0xad,0xcd,0xba,0x24,0x6b,0xfc,0x96,0xae,0x48,0x11,0x69,0xd5,0xe6,0x4e,0xc7,0x3a,0xe8,0x25,0xec,0xa3,0x8b,0xd3,0xcc,0xbb,0x6e,0x53,0x3a,0x4c,0x46,0xc5,0x3a,0x82,0x60,0x12,0x2a,0x4e,0x0e,0x0e,0x2f,0xb6,0x37,0x71,0x41,0xab,0x66,0x88,0x9a,0x00,0x05,0x13,0xe6,0x02,0xb7,0xd2,0x32,0xa5,0x92,0x49,0x0e,0x35,0x76,0x47,0x13,0xf4,0x1e,0x92,0xf9,0x1b,0x37,0x77,0x09,0x0f,0x0e,0x95,0x85,0xc2,0xa1,0x4b,0x18,0xbc,0x7a,0xd9,0x1d,0xa5,0x05,0x07,0xcd,0x9a,0x26,0x80,0xbe,0xd8,0xd4,0xe0,0x34,0x2a,0x03,0x0a,0x1a,0x32,0x6d,0x72,0x50,0x9d,0x3d,0x2f,0xb9,0x15,0x09,0x1f,0xee,0x79,0xaa,0xc9,0xa3,0xb2,0xa2,0xdf,0x63,0xd3,0x97,0x63,0x4c,0xa5,0x42,0x35,0xe5,0xb9,0x2f,0x17,0x65,0x97,0xa6,0x98,0xc2,0x68,0x32,0xa8,0x99,0x17,0x68,0x3f,0x23,0x9e,0x73,0xee,0x5d,0xca,0xa4,0x33,0x2f,0xf6,0xf7,0xef,0xc1,0x25,0x87,0xd1,0x02,0x03,0x01,0x00,0x01,0x02,0x81,0xff,0x1a,0x07,0xa7,0x07,0xbb,0xcb,0x59,0x56,0xb4,0xa2,0x92,0xef,0x03,0x04,0x32,0x0b,0x6a,0x2d,0x15,0x69,0xe4,0x5b,0x3c,0xd1,0xf3,0xca,0x51,0x98,0x18,0x9a,0xdf,0xaa,0x03,0x15,0x1d,0x77,0xfe,0xb5,0xc0,0x26,0xd5,0x94,0x53,0x39,0x11,0x10,0xc2,0xae,0xf1,0x0f,0x63,0x3d,0x4c,0x1d,0x6d,0x91,0x95,0x34,0x45,0xa2,0x28,0x6a,0x76,0xc5,0xe2,0x02,0x77,0xb8,0xab,0x10,0x65,0x26,0xf0,0x63,0x90,0xea,0xaa,0xd4,0xe3,0xdf,0xf2,0xcc,0xf8,0xa5,0x61,0x80,0x8b,0x4d,0xf9,0x7a,0x91,0x44,0x8c,0xb3,0xa3,0x4e,0xd7,0x17,0x8b,0x86,0x53,0x46,0xa2,0x26,0x54,0xf7,0xbc,0x13,0xfe,0xa2,0xa8,0x16,0x70,0xe3,0xaa,0xbf,0x46,0xf7,0xdb,0x52,0xb7,0x24,0x29,0x86,0xa1,0xfc,0x92,0x9a,0xd6,0x87,0x9f,0xbc,0xe5,0x21,0x35,0x32,0xf0,0xb5,0x02,0x1b,0xcd,0xcd,0xab,0xd2,0x5f,0xe9,0x41,0xfb,0xe7,0xfa,0x69,0x58,0x7a,0xd4,0x0a,0x5f,0xeb,0xec,0xc8,0x96,0xaa,0x1d,0x52,0x65,0xb6,0x2e,0xba,0x32,0x87,0x89,0xa3,0xa3,0x7a,0x5d,0x96,0x10,0x80,0x49,0xe0,0x63,0xea,0x68,0x16,0x5c,0xda,0xcf,0x8b,0x0d,0xa5,0x78,0x00,0x6f,0xc4,0xac,0xfc,0xde,0x38,0xb8,0x78,0xd9,0x01,0xa2,0x88,0xec,0x7c,0xcf,0xf7,0x35,0x3c,0xd5,0x00,0x8b,0x39,0x1d,0xac,0xd4,0x21,0x5d,0x64,0x01,0x1d,0xe2,0xca,0x9f,0xca,0x23,0xc8,0x89,0x79,0xcb,0x86,0xee,0x52,0x51,0x8c,0xb8,0x65,0x43,0x61,0x05,0xff,0xd1,0xd5,0xf6,0x82,0x6b,0xc3,0xd4,0x8d,0x9e,0xc0,0xcd,0xaf,0xe3,0x01,0x02,0x81,0x81,0x00,0xd3,0xf7,0x29,0xb7,0x32,0x69,0x6a,0xcc,0x81,0x4a,0x00,0x7b,0x27,0xbc,0x78,0x6c,0x11,0xc3,0xeb,0xee,0x55,0xb7,0x93,0x5d,0x4b,0xde,0x4d,0xc5,0x00,0x32,0xb4,0x96,0xa0,0x82,0xf0,0xcc,0xc3,0x74,0x72,0xc2,0xf8,0xc6,0x34,0x4c,0xbd,0x57,0x08,0x08,0xf3,0x81,0xc8,0x0d,0x0b,0x01,0x10,0x70,0x69,0x36,0x8a,0xd3,0x2a,0x2e,0x0b,0x0c,0xa4,0x17,0x4b,0x17,0xb1,0x5f,0x21,0x09,0x3f,0x91,0x71,0x58,0xcb,0x46,0xa9,0x6c,0x11,0x3f,0xa4,0x21,0xca,0x73,0xf3,0xc6,0xa5,0xe5,0xe9,0x3c,0x68,0xad,0x72,0x55,0xf5,0x9c,0x79,0x2b,0xbc,0x40,0xc1,0x35,0xfb,0x2d,0xfe,0x48,0x8b,0xdf,0x26,0x6a,0xba,0x6c,0x0c,0x38,0x73,0x3f,0x07,0xa5,0x0d,0x14,0xe0,0x55,0x7c,0xee,0x0c,0x51,0x02,0x81,0x81,0x00,0xc8,0x93,0x6e,0x11,0x79,0xef,0x25,0x8b,0xb3,0xce,0xa3,0x00,0xde,0xae,0x7d,0xa9,0x66,0x0c,0xff,0x7f,0x6f,0x0a,0x1c,0xce,0xea,0x74,0x32,0x79,0x21,0xa1,0xbf,0x35,0xf7,0x54,0x8e,0x32,0x74,0xf1,0x53,0x6d,0x0a,0xcc,0x29,0xb5,0x4b,0x2f,0x60,0x48,0xc4,0x12,0xce,0x5d,0xbf,0x66,0x87,0x35,0xab,0x0e,0xed,0x18,0x08,0xf6,0x67,0x8a,0x10,0x8d,0x7f,0x0b,0x64,0x45,0xb3,0xa6,0x9c,0xb8,0x97,0xa0,0x8e,0x31,0x90,0x38,0x8c,0x6c,0x55,0x9a,0x72,0xf1,0x2d,0x74,0xe6,0x2a,0x28,0x35,0x32,0xd2,0xea,0x20,0xd0,0xeb,0x98,0xe1,0x3c,0x5a,0xdd,0x90,0x67,0x87,0x96,0x38,0x86,0x8b,0xd6,0xba,0xde,0xa2,0x4f,0x93,0xd0,0x2b,0xc1,0x3b,0x8e,0xdc,0xf2,0x9b,0x72,0x0e,0x63,0x81,0x02,0x81,0x81,0x00,0xa0,0xae,0xc0,0x57,0xd9,0x06,0x62,0xe8,0xae,0xa5,0xfb,0xa1,0x60,0xa9,0x16,0x1c,0xce,0x86,0xd8,0x46,0x6f,0x82,0x5f,0x5b,0x1d,0x55,0x63,0x5d,0x10,0xde,0x5f,0x59,0xc4,0x4a,0x1e,0xe3,0x0a,0xd4,0x2e,0xdf,0x30,0x99,0x88,0x32,0x18,0x62,0x6e,0x7b,0xe2,0xa4,0xa2,0x93,0x88,0xa1,0x00,0xe6,0x46,0x4c,0x0b,0x87,0x32,0x75,0x7e,0xf8,0xd1,0x37,0xbb,0x3c,0x35,0xc0,0xa8,0x99,0x8b,0x2f,0xe5,0x5d,0xeb,0x93,0x20,0x82,0x04,0xbd,0x5a,0xde,0x26,0xcc,0x29,0xa3,0x90,0x45,0xae,0xbe,0x95,0x97,0xce,0x91,0xed,0x43,0xf5,0xf3,0x3a,0x9d,0x58,0x1a,0x64,0xd4,0x23,0xec,0xb8,0xe3,0xac,0x0a,0xc2,0x51,0x80,0x1e,0x16,0xf7,0x54,0x03,0x41,0xdc,0x6e,0x87,0x3f,0xd2,0x71,0x51,0x02,0x81,0x81,0x00,0xc0,0x2c,0xc1,0xa0,0xec,0xfb,0x3d,0x36,0x05,0x29,0x4e,0xf4,0x5e,0x67,0x40,0xef,0x35,0x5f,0xee,0xcc,0xe2,0x4f,0x06,0x1b,0x1a,0x78,0x0a,0x84,0x14,0xbd,0x2c,0xbf,0x40,0x22,0x3b,0xbc,0x0c,0x5f,0xcc,0x57,0x05,0xa9,0x67,0xac,0xd4,0x44,0xa8,0x34,0x0f,0xe9,0x14,0xe0,0xad,0xaa,0x46,0x92,0x1d,0x3b,0x47,0xdf,0x67,0x76,0xee,0xed,0x51,0xbe,0xc0,0xa2,0xe1,0x88,0x07,0x00,0xf3,0xd7,0xc9,0x2f,0x84,0x0d,0xe8,0xe8,0xc5,0x62,0x67,0x02,0x7c,0x6e,0xa7,0xe7,0x01,0x40,0x13,0x16,0x25,0x83,0xff,0xdf,0x41,0x4c,0xfa,0x45,0x93,0xd6,0x5b,0xc3,0x27,0xee,0xb8,0xf7,0x9f,0xb2,0x0c,0x23,0x2f,0x7c,0xc4,0x52,0x13,0x7a,0x58,0xc1,0xff,0xb7,0xa2,0x9b,0x5d,0x44,0x7c,0x01,0x02,0x81,0x81,0x00,0x98,0x80,0xda,0x67,0xb7,0x9a,0x27,0xd3,0x78,0x08,0x9e,0xac,0xd7,0x4c,0xbe,0xed,0x5f,0x85,0x07,0x71,0x93,0xd8,0xcf,0xe7,0x7c,0xf3,0x16,0xfd,0xc2,0x5d,0xea,0x09,0x4c,0xf3,0xea,0x54,0x41,0xb1,0x20,0x50,0x9a,0xad,0x4a,0xa7,0x4b,0x43,0x8c,0x71,0x8b,0x6d,0x75,0xb1,0xfa,0xb7,0xb9,0xbd,0x33,0x34,0x49,0x88,0x3e,0xac,0xc0,0x18,0x10,0x93,0x32,0xd2,0x8a,0xba,0x84,0xfa,0x56,0xcd,0x00,0xb0,0x62,0x35,0xad,0xde,0xd6,0x21,0x1c,0xa0,0xaa,0x88,0x41,0x01,0xe7,0x13,0x52,0xf5,0xf3,0x28,0xc2,0xfe,0x56,0xfc,0x29,0xeb,0xc7,0x90,0xd4,0x10,0x51,0xad,0x74,0xbf,0x45,0xd7,0xc1,0x31,0x7b,0x21,0xd2,0x2b,0x30,0xbd,0xb2,0xa6,0xea,0x72,0xf0,0xe7,0x0f,0xc8,0x43,0xfd];
        let ref_key = RsaPrivateKey::from_pkcs8_der(RSA_2048_SP800_PRIV_DER).unwrap();
        assert_eq!(ref_key.validate(), Ok(()));

        let primes = PrivateKeyParts::primes(&ref_key).to_vec();
        let exp = PublicKeyParts::e(&ref_key);

        let key = RsaPrivateKey::from_p_q(primes[0].clone(), primes[1].clone(), exp.clone())
            .expect("failed to import key from primes");
        assert_eq!(key.validate(), Ok(()));

        assert_eq!(PublicKeyParts::n(&key), PublicKeyParts::n(&ref_key));

        assert_eq!(PrivateKeyParts::dp(&key), PrivateKeyParts::dp(&ref_key));
        assert_eq!(PrivateKeyParts::dq(&key), PrivateKeyParts::dq(&ref_key));

        assert_eq!(PrivateKeyParts::d(&key), PrivateKeyParts::d(&ref_key));
    }

    #[test]
    #[cfg(feature = "hazmat")]
    fn test_from_components_with_large_exponent() {
        // Test that from_components_with_large_exponent accepts exponents outside normal bounds
        // while from_components would reject them

        use rand::rngs::ChaCha8Rng;
        use rand_core::SeedableRng;

        let mut rng = ChaCha8Rng::from_seed([42; 32]);

        // Use an exponent larger than the normal maximum (2^33 - 1)
        let large_e = BoxedUint::from((1u64 << 34) + 1); // 2^34 + 1 (odd number)

        // Generate a key with this large exponent
        let components =
            generate_multi_prime_key_with_exp(&mut rng, 2, 1024, large_e.clone()).unwrap();

        // Extract components
        let n = components.n.get().clone();
        let d = components.d;
        let primes = components.primes;

        // from_components should fail with PublicExponentTooLarge
        let result =
            RsaPrivateKey::from_components(n.clone(), large_e.clone(), d.clone(), primes.clone());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), Error::PublicExponentTooLarge);

        // from_components_with_large_exponent should succeed
        let key_with_large_exp = RsaPrivateKey::from_components_with_large_exponent(
            n.clone(),
            large_e.clone(),
            d.clone(),
            primes.clone(),
        );
        assert!(key_with_large_exp.is_ok());

        let key_with_large_exp = key_with_large_exp.unwrap();
        assert_eq!(PublicKeyParts::e(&key_with_large_exp), &large_e);
        assert_eq!(PublicKeyParts::n(&key_with_large_exp).as_ref(), &n);
        assert_eq!(PrivateKeyParts::d(&key_with_large_exp), &d);

        // Verify that the key is still cryptographically valid (de ≡ 1 mod λ(n))
        // by checking that validation with skip_exponent_size passes
        assert!(validate_skip_exponent_size(&key_with_large_exp).is_ok());
    }

    #[test]
    #[cfg(feature = "hazmat")]
    fn test_from_components_with_small_exponent() {
        // Test that from_components_with_large_exponent accepts exponents below normal minimum
        // (despite the name, it works for any non-standard exponent size)

        use rand::rngs::ChaCha8Rng;
        use rand_core::SeedableRng;

        let mut rng = ChaCha8Rng::from_seed([43; 32]);

        // Use an exponent smaller than the normal minimum (2)
        let small_e = BoxedUint::from(1u64); // This is odd, which is required

        // Generate a key with this small exponent
        let components =
            generate_multi_prime_key_with_exp(&mut rng, 2, 1024, small_e.clone()).unwrap();

        // Extract components
        let n = components.n.get().clone();
        let d = components.d;
        let primes = components.primes;

        // from_components should fail
        let result =
            RsaPrivateKey::from_components(n.clone(), small_e.clone(), d.clone(), primes.clone());
        assert!(result.is_err());

        // from_components_with_large_exponent should succeed
        let key_with_small_exp = RsaPrivateKey::from_components_with_large_exponent(
            n.clone(),
            small_e.clone(),
            d.clone(),
            primes,
        );
        assert!(key_with_small_exp.is_ok());

        let key_with_small_exp = key_with_small_exp.unwrap();
        assert_eq!(PublicKeyParts::e(&key_with_small_exp), &small_e);

        // Verify that the key is cryptographically valid
        assert!(validate_skip_exponent_size(&key_with_small_exp).is_ok());
    }
}
