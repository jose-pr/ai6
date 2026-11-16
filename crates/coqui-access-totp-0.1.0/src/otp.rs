//! The one-time password algorithms: HOTP (RFC 4226) over a time step (RFC 6238), and the
//! text forms around them.

use coqui_builder::Leaf;
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use subtle::ConstantTimeEq;

/// The hash a code is made with. Authenticator apps default to SHA-1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Leaf)]
#[map(rename_all = "lowercase")]
pub enum Algorithm {
    /// HMAC-SHA-1, which every authenticator app reads.
    #[default]
    Sha1,
    /// HMAC-SHA-256.
    Sha256,
    /// HMAC-SHA-512.
    Sha512,
}

impl Algorithm {
    /// The word a record keeps: `sha1`, `sha256` or `sha512`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }

    /// The word an `otpauth://` link uses: `SHA1`, `SHA256` or `SHA512`.
    pub(crate) fn uri_word(self) -> &'static str {
        match self {
            Self::Sha1 => "SHA1",
            Self::Sha256 => "SHA256",
            Self::Sha512 => "SHA512",
        }
    }

    pub(crate) fn parse_word(word: &str) -> Option<Self> {
        match word {
            "sha1" => Some(Self::Sha1),
            "sha256" => Some(Self::Sha256),
            "sha512" => Some(Self::Sha512),
            _ => None,
        }
    }
}

fn mac<M: Mac + KeyInit>(key: &[u8], message: &[u8]) -> Vec<u8> {
    // HMAC accepts a key of any length.
    let mut mac = <M as KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// HOTP (RFC 4226): the `digits`-digit code for `counter`.
pub(crate) fn hotp(algorithm: Algorithm, key: &[u8], counter: u64, digits: u32) -> u32 {
    let message = counter.to_be_bytes();
    let digest = match algorithm {
        Algorithm::Sha1 => mac::<Hmac<Sha1>>(key, &message),
        Algorithm::Sha256 => mac::<Hmac<Sha256>>(key, &message),
        Algorithm::Sha512 => mac::<Hmac<Sha512>>(key, &message),
    };
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    let truncated = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    truncated % 10u32.pow(digits)
}

/// The code for `step`, zero-padded to `digits`.
pub(crate) fn code_at(algorithm: Algorithm, key: &[u8], step: i64, digits: u32) -> String {
    let counter = u64::try_from(step).unwrap_or(0);
    format!(
        "{:0width$}",
        hotp(algorithm, key, counter, digits),
        width = digits as usize
    )
}

/// The time step of `unix` seconds in steps of `period`.
pub(crate) fn step_of(unix: i64, period: u32) -> i64 {
    unix.div_euclid(i64::from(period))
}

/// The settings a code is checked under.
#[derive(Clone, Copy)]
pub(crate) struct Check {
    pub(crate) algorithm: Algorithm,
    pub(crate) digits: u32,
    pub(crate) period: u32,
    /// Steps each side of the present one that are accepted.
    pub(crate) skew: u32,
}

/// The newest step within `skew` of `unix` whose code is `presented` and that is later than
/// `after`, so a step already used is not accepted again. Every step in range is compared in
/// constant time, and the work does not depend on which one matches.
pub(crate) fn verify(
    check: Check,
    key: &[u8],
    presented: &str,
    unix: i64,
    after: Option<i64>,
) -> Option<i64> {
    let digits = check.digits as usize;
    if presented.len() != digits || !presented.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let now = step_of(unix, check.period);
    let skew = i64::from(check.skew);
    let mut matched = None;
    for step in (now - skew)..=(now + skew) {
        if step < 0 {
            continue;
        }
        let expected = code_at(check.algorithm, key, step, check.digits);
        let same = bool::from(expected.as_bytes().ct_eq(presented.as_bytes()));
        if same && after.is_none_or(|used| step > used) {
            matched = Some(matched.map_or(step, |best: i64| best.max(step)));
        }
    }
    matched
}

/// The base32 text authenticator apps read a secret in.
pub(crate) fn base32(key: &[u8]) -> String {
    BASE32_NOPAD.encode(key)
}

/// The key a base32 secret names, or `None`.
pub(crate) fn unbase32(text: &str) -> Option<Vec<u8>> {
    BASE32_NOPAD.decode(text.as_bytes()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA1_KEY: &[u8] = b"12345678901234567890";
    const SHA256_KEY: &[u8] = b"12345678901234567890123456789012";
    const SHA512_KEY: &[u8] = b"1234567890123456789012345678901234567890123456789012345678901234";

    /// RFC 6238 Appendix B: eight digits at fixed times, for the three hashes.
    #[test]
    fn every_rfc_6238_appendix_b_value_for_the_three_hashes() {
        let table: [(i64, &str, &str, &str); 6] = [
            (59, "94287082", "46119246", "90693936"),
            (1_111_111_109, "07081804", "68084774", "25091201"),
            (1_111_111_111, "14050471", "67062674", "99943326"),
            (1_234_567_890, "89005924", "91819424", "93441116"),
            (2_000_000_000, "69279037", "90698825", "38618901"),
            (20_000_000_000, "65353130", "77737706", "47863826"),
        ];
        for (unix, sha1, sha256, sha512) in table {
            let step = step_of(unix, 30);
            assert_eq!(code_at(Algorithm::Sha1, SHA1_KEY, step, 8), sha1, "{unix}");
            assert_eq!(
                code_at(Algorithm::Sha256, SHA256_KEY, step, 8),
                sha256,
                "{unix}"
            );
            assert_eq!(
                code_at(Algorithm::Sha512, SHA512_KEY, step, 8),
                sha512,
                "{unix}"
            );
        }
    }

    /// RFC 4226 Appendix D: the HOTP values for counters 0 to 9.
    #[test]
    fn the_rfc_4226_hotp_values() {
        let expected = [
            755224, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489,
        ];
        for (counter, code) in expected.into_iter().enumerate() {
            assert_eq!(hotp(Algorithm::Sha1, SHA1_KEY, counter as u64, 6), code);
        }
    }

    fn check(skew: u32) -> Check {
        Check {
            algorithm: Algorithm::Sha1,
            digits: 6,
            period: 30,
            skew,
        }
    }

    #[test]
    fn a_code_one_step_early_or_late_passes_with_skew_one_and_fails_with_none() {
        let now = 1_700_000_000;
        let step = step_of(now, 30);
        for offset in [-1i64, 0, 1] {
            let code = code_at(Algorithm::Sha1, SHA1_KEY, step + offset, 6);
            assert_eq!(
                verify(check(1), SHA1_KEY, &code, now, None),
                Some(step + offset)
            );
            assert_eq!(
                verify(check(0), SHA1_KEY, &code, now, None).is_some(),
                offset == 0,
                "offset {offset}"
            );
        }
        let two_late = code_at(Algorithm::Sha1, SHA1_KEY, step + 2, 6);
        assert_eq!(verify(check(1), SHA1_KEY, &two_late, now, None), None);
    }

    #[test]
    fn a_step_at_or_before_the_last_used_is_refused() {
        let now = 1_700_000_000;
        let step = step_of(now, 30);
        let code = code_at(Algorithm::Sha1, SHA1_KEY, step, 6);
        assert_eq!(
            verify(check(1), SHA1_KEY, &code, now, Some(step - 1)),
            Some(step)
        );
        assert_eq!(verify(check(1), SHA1_KEY, &code, now, Some(step)), None);
        assert_eq!(verify(check(1), SHA1_KEY, &code, now, Some(step + 1)), None);
    }

    #[test]
    fn a_code_of_the_wrong_shape_is_refused_without_computing() {
        let now = 1_700_000_000;
        for bad in ["", "12345", "1234567", "12a456", " 12345", "١٢٣٤٥٦"] {
            assert_eq!(verify(check(1), SHA1_KEY, bad, now, None), None, "{bad:?}");
        }
    }

    #[test]
    fn base32_round_trips_and_refuses_other_text() {
        let text = base32(SHA1_KEY);
        assert_eq!(unbase32(&text).as_deref(), Some(SHA1_KEY));
        assert_eq!(unbase32("not base32!"), None);
    }
}
