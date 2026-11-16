//! Seals a one-time code secret with a key every gateway sharing the Registry holds, so the
//! database keeps none in the clear.
//!
//! AES-256-GCM with a fresh nonce per write; the account and the kind of factor it belongs to are
//! the associated data, so a sealed secret moved to another account or table does not open. The
//! key is derived from the configured secret with HKDF-SHA256. With no key nothing is sealed and
//! nothing opens: a factor is refused, never kept in the clear.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::hkdf::{HKDF_SHA256, KeyType, Salt};
use ring::rand::{SecureRandom, SystemRandom};

/// What a sealed text starts with: the scheme, so another can follow.
const PREFIX: &str = "aes256gcm1:";

/// Why a secret could not be sealed or opened. Never holds a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SealError {
    /// No `[unex] factor_key` is configured.
    NoKey,
    /// The text is not sealed, was sealed under another key, or belongs to another account.
    Unreadable,
    /// The system could not make a nonce or seal.
    Sealing,
}

impl std::fmt::Display for SealError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NoKey => "no usable [unex] factor_key seals one-time code secrets",
            Self::Unreadable => "a one-time code secret cannot be opened with the factor_key",
            Self::Sealing => "a one-time code secret could not be sealed",
        })
    }
}

impl std::error::Error for SealError {}

struct Key32;

impl KeyType for Key32 {
    fn len(&self) -> usize {
        32
    }
}

/// The key that seals one-time code secrets, or none.
pub struct FactorSealer {
    key: Option<LessSafeKey>,
    /// Why a configured key is not used: the bytes it has and the fewest it needs.
    too_short: Option<(usize, usize)>,
    random: SystemRandom,
}

impl std::fmt::Debug for FactorSealer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FactorSealer")
            .field("ready", &self.key.is_some())
            .finish()
    }
}

impl FactorSealer {
    /// The sealer for `secret`, the configured key; none seals nothing, and neither does one of
    /// fewer than `min_bytes` bytes ([`refusal`](Self::refusal) says so).
    ///
    /// # Errors
    ///
    /// When `secret` is empty.
    pub fn new(secret: Option<&str>, min_bytes: usize) -> Result<Self, String> {
        let mut too_short = None;
        let key = match secret {
            None => None,
            Some("") => return Err("the one-time code sealing key is empty".to_owned()),
            Some(secret) if secret.len() < min_bytes => {
                too_short = Some((secret.len(), min_bytes));
                None
            }
            Some(secret) => {
                let mut derived = [0u8; 32];
                Salt::new(HKDF_SHA256, b"ai6 one-time code sealing")
                    .extract(secret.as_bytes())
                    .expand(&[b"aes-256-gcm"], Key32)
                    .and_then(|okm| okm.fill(&mut derived))
                    .map_err(|_| "the one-time code sealing key cannot be used".to_owned())?;
                let unbound = UnboundKey::new(&AES_256_GCM, &derived)
                    .map_err(|_| "the one-time code sealing key cannot be used".to_owned())?;
                Some(LessSafeKey::new(unbound))
            }
        };
        Ok(Self {
            key,
            too_short,
            random: SystemRandom::new(),
        })
    }

    /// Whether a key is configured and long enough to seal with.
    pub fn ready(&self) -> bool {
        self.key.is_some()
    }

    /// Why nothing is sealed, when it is not; never the key.
    pub fn refusal(&self) -> Option<String> {
        if self.key.is_some() {
            return None;
        }
        Some(match self.too_short {
            Some((_, least)) => format!("[unex] factor_key holds fewer than {least} bytes"),
            None => "no [unex] factor_key is configured".to_owned(),
        })
    }

    /// `plain` sealed for `context`.
    ///
    /// # Errors
    ///
    /// [`SealError::NoKey`] without a key.
    pub fn seal(&self, context: &str, plain: &str) -> Result<String, SealError> {
        let key = self.key.as_ref().ok_or(SealError::NoKey)?;
        let mut nonce = [0u8; NONCE_LEN];
        self.random
            .fill(&mut nonce)
            .map_err(|_| SealError::Sealing)?;
        let mut sealed = plain.as_bytes().to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(context.as_bytes()),
            &mut sealed,
        )
        .map_err(|_| SealError::Sealing)?;
        let mut text = nonce.to_vec();
        text.extend_from_slice(&sealed);
        Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(text)))
    }

    /// What `sealed` holds for `context`.
    ///
    /// # Errors
    ///
    /// [`SealError::NoKey`] without a key; [`SealError::Unreadable`] when it does not open.
    pub fn open(&self, context: &str, sealed: &str) -> Result<String, SealError> {
        let key = self.key.as_ref().ok_or(SealError::NoKey)?;
        let bytes = sealed
            .strip_prefix(PREFIX)
            .and_then(|body| URL_SAFE_NO_PAD.decode(body).ok())
            .filter(|bytes| bytes.len() > NONCE_LEN)
            .ok_or(SealError::Unreadable)?;
        let (nonce, body) = bytes.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| SealError::Unreadable)?;
        let mut body = body.to_vec();
        let plain = key
            .open_in_place(nonce, Aad::from(context.as_bytes()), &mut body)
            .map_err(|_| SealError::Unreadable)?;
        String::from_utf8(plain.to_vec()).map_err(|_| SealError::Unreadable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealer(secret: &str) -> FactorSealer {
        FactorSealer::new(Some(secret), 0).unwrap()
    }

    #[test]
    fn a_secret_opens_only_under_its_key_and_its_account() {
        let sealed = sealer("key one")
            .seal("totp\0id-alice", "JBSWY3DPEHPK3PXP")
            .unwrap();
        assert!(!sealed.contains("JBSWY3DPEHPK3PXP"), "{sealed}");
        assert_eq!(
            sealer("key one").open("totp\0id-alice", &sealed).unwrap(),
            "JBSWY3DPEHPK3PXP"
        );
        assert_eq!(
            sealer("key two").open("totp\0id-alice", &sealed),
            Err(SealError::Unreadable),
            "another key"
        );
        assert_eq!(
            sealer("key one").open("totp\0id-bob", &sealed),
            Err(SealError::Unreadable),
            "another account"
        );
        assert_eq!(
            sealer("key one").open("totp_enrolling\0id-alice", &sealed),
            Err(SealError::Unreadable),
            "another table"
        );
        assert_eq!(
            sealer("key one").open("totp\0id-alice", "JBSWY3DPEHPK3PXP"),
            Err(SealError::Unreadable),
            "a secret in the clear is not a sealed one"
        );
    }

    #[test]
    fn every_seal_has_a_fresh_nonce() {
        let key = sealer("key one");
        let first = key.seal("totp\0id-alice", "same").unwrap();
        let second = key.seal("totp\0id-alice", "same").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn with_no_key_nothing_is_sealed_or_opened() {
        let none = FactorSealer::new(None, 32).unwrap();
        assert!(!none.ready());
        assert!(none.refusal().unwrap().contains("configured"));
        assert_eq!(none.seal("c", "secret"), Err(SealError::NoKey));
        assert_eq!(none.open("c", "aes256gcm1:AAAA"), Err(SealError::NoKey));
        assert!(FactorSealer::new(Some(""), 32).is_err());
    }

    #[test]
    fn a_key_shorter_than_the_floor_seals_nothing_and_says_so() {
        let short = FactorSealer::new(Some("only a few bytes"), 32).unwrap();
        assert!(!short.ready());
        assert_eq!(short.seal("c", "secret"), Err(SealError::NoKey));
        let reason = short.refusal().unwrap();
        assert!(
            reason.contains("32") && !reason.contains("only a few"),
            "{reason}"
        );
        assert!(
            FactorSealer::new(Some(&"k".repeat(32)), 32)
                .unwrap()
                .ready()
        );
    }
}
