//! Secrets that are made, not chosen, and what is kept of them.

use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Characters of base64url text that 32 random bytes take.
pub const SECRET_CHARS: usize = 43;

/// Makes a secret: `prefix` followed by 256 random bits as 43 URL-safe base64 characters.
///
/// Random and that long, it needs no slow hash to be kept: keep [`hash`] of it.
///
/// # Errors
///
/// Returns an error when the operating system random source is unavailable.
pub fn generate(prefix: &str) -> Result<String, String> {
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| "system random source is unavailable".to_owned())?;
    Ok(format!("{prefix}{}", BASE64URL_NOPAD.encode(&random)))
}

/// Makes a random 128-bit identifier as 32 lowercase hexadecimal characters.
///
/// # Errors
///
/// Returns an error when the operating system random source is unavailable.
pub fn identifier() -> Result<String, String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| "system random source is unavailable".to_owned())?;
    Ok(HEXLOWER.encode(&random))
}

/// What a secret is kept as: its SHA-256 as 64 lowercase hexadecimal characters.
///
/// With a `domain`, the digest is over `domain`, a zero byte, then the secret, so a digest
/// kept for one purpose cannot be presented for another.
pub fn hash(secret: &str, domain: Option<&str>) -> String {
    let mut digest = Sha256::new();
    if let Some(domain) = domain {
        digest.update(domain.as_bytes());
        digest.update([0]);
    }
    digest.update(secret.as_bytes());
    HEXLOWER.encode(&digest.finalize())
}

/// The SHA-256 of `bytes` as 64 lowercase hexadecimal characters.
pub fn hash_bytes(bytes: &[u8]) -> String {
    HEXLOWER.encode(&Sha256::digest(bytes))
}

/// Compares two strings in time that does not depend on where they differ.
pub fn same(a: &str, b: &str) -> bool {
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Whether `value` is a SHA-256 digest as [`hash`] writes it.
pub fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_prefixed_long_random_and_url_safe() {
        let (one, other) = (generate("ck_").unwrap(), generate("ck_").unwrap());
        assert_ne!(one, other);
        for secret in [&one, &other] {
            assert_eq!(secret.len(), 3 + SECRET_CHARS);
            let letters = secret.strip_prefix("ck_").unwrap();
            assert!(
                letters
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            );
        }
    }

    #[test]
    fn hashes_are_sha256_hex_and_domain_separated() {
        assert_eq!(
            hash("abc", None),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(hash("abc", Some("login")), hash("abc", Some("ticket")));
        assert_ne!(hash("abc", Some("login")), hash("abc", None));
        assert!(is_hash(&hash("x", None)));
        assert!(!is_hash("ABC"));
    }

    #[test]
    fn a_secret_decodes_back_to_32_bytes() {
        let secret = generate("").unwrap();
        assert_eq!(BASE64URL_NOPAD.decode(secret.as_bytes()).unwrap().len(), 32);
        assert_eq!(BASE64URL_NOPAD.encode(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn comparison_and_identifiers() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd") && !same("abc", "ab"));
        let id = identifier().unwrap();
        assert_eq!(id.len(), 32);
        assert_ne!(id, identifier().unwrap());
    }
}
