use core::fmt;

use subtle::ConstantTimeEq;

use crate::error::Error;
use crate::random::Random;

/// The characters and length of a short code a person reads and types: a PIN or a device-flow
/// user code.
///
/// The crate matches codes exactly over `chars`: it ignores no case, dash or space. Normalising
/// what a person typed, and grouping a code for display, is the caller's. Any alphabet and
/// length with a meaning is accepted; [`combinations`](CodeAlphabet::combinations) says how many
/// codes there are, which is what an attacker's one guess is against.
#[derive(Debug, Clone, PartialEq, Eq, coqui_builder::Describe)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct CodeAlphabet {
    /// The characters a code is made of, each once.
    pub chars: String,
    /// How many characters a code has.
    pub len: usize,
}

impl CodeAlphabet {
    /// An alphabet of the characters `chars`, each once, and codes of `len` of them.
    pub fn new(chars: impl Into<String>, len: usize) -> Self {
        Self {
            chars: chars.into(),
            len,
        }
    }

    /// The default PIN alphabet: `0123456789`, 6 characters (10^6 codes).
    pub fn pin() -> Self {
        Self {
            chars: "0123456789".to_owned(),
            len: 6,
        }
    }

    /// The default user-code alphabet of RFC 8628 section 6.1: `BCDFGHJKLMNPQRSTVWXZ`, 8
    /// characters (about 2.6 × 10^10 codes).
    pub fn user_code() -> Self {
        Self {
            chars: "BCDFGHJKLMNPQRSTVWXZ".to_owned(),
            len: 8,
        }
    }

    /// How many distinct codes there are, saturating at `u128::MAX`.
    pub fn combinations(&self) -> u128 {
        let base = self.chars.chars().count() as u128;
        u32::try_from(self.len)
            .ok()
            .and_then(|len| base.checked_pow(len))
            .unwrap_or(u128::MAX)
    }

    /// Whether the alphabet has a meaning.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOptions`] when `chars` is empty, repeats a character or holds a control
    /// character, or `len` is 0.
    pub fn check(&self) -> Result<(), Error> {
        let chars: Vec<char> = self.chars.chars().collect();
        let repeated = chars
            .iter()
            .enumerate()
            .any(|(index, c)| chars[..index].contains(c));
        if chars.is_empty() || repeated || chars.iter().any(|c| c.is_control()) || self.len == 0 {
            return Err(Error::InvalidOptions);
        }
        Ok(())
    }

    /// A uniformly random code.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOptions`] as [`check`](CodeAlphabet::check); [`Error::Random`] when the
    /// source fails.
    pub(crate) fn generate(&self, random: &Random) -> Result<String, Error> {
        self.check()?;
        let chars: Vec<char> = self.chars.chars().collect();
        let mut code = String::with_capacity(self.len);
        for _ in 0..self.len {
            code.push(chars[below(random, chars.len())?]);
        }
        Ok(code)
    }

    /// `text` when it is exactly `len` characters of this alphabet.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOptions`] as [`check`](CodeAlphabet::check); [`Error::InvalidCode`]
    /// otherwise.
    pub(crate) fn parse(&self, text: &str) -> Result<String, Error> {
        self.check()?;
        if text.chars().count() != self.len || !text.chars().all(|c| self.chars.contains(c)) {
            return Err(Error::InvalidCode);
        }
        Ok(text.to_owned())
    }
}

/// A uniformly random index below `bound`, by rejection sampling of four random bytes.
pub(crate) fn below(random: &Random, bound: usize) -> Result<usize, Error> {
    let bound = u64::try_from(bound.max(1)).unwrap_or(u64::MAX);
    let span = 1u64 << 32;
    let limit = span - span % bound;
    loop {
        let mut bytes = [0u8; 4];
        random.fill(&mut bytes)?;
        let value = u64::from(u32::from_be_bytes(bytes));
        if value < limit {
            return Ok(usize::try_from(value % bound).unwrap_or(0));
        }
    }
}

/// Whether two codes are equal, in time that does not depend on where they differ.
pub(crate) fn same(left: &str, right: &str) -> bool {
    left.len() == right.len() && bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

/// A PIN: a short code one side shows and a person types on the other, checked by a PAKE.
/// `Debug` does not show it.
#[derive(Clone, PartialEq, Eq)]
pub struct Pin(String);

impl Pin {
    /// A random PIN of `alphabet`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOptions`] for a meaningless alphabet, [`Error::Random`].
    pub fn generate(alphabet: &CodeAlphabet, random: &Random) -> Result<Self, Error> {
        alphabet.generate(random).map(Self)
    }

    /// `text` as a PIN of `alphabet`, matched exactly.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidCode`] when it is not exactly `len` characters of the alphabet.
    pub fn parse(alphabet: &CodeAlphabet, text: &str) -> Result<Self, Error> {
        alphabet.parse(text).map(Self)
    }

    /// The characters, to show.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

impl fmt::Debug for Pin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Pin(..)")
    }
}

/// What a person types at the responder when the joiner shows its PIN: the slot the responder
/// assigned to that joiner, and the PIN. Plain data: joining and splitting the text a person
/// reads (`37-482913`, say) is the caller's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    /// The responder's handle for the waiting joiner, 0 to 99; not secret. `None` when the
    /// responder shows the PIN.
    pub slot: Option<u8>,
    /// The PIN.
    pub pin: Pin,
}

/// A device-flow user code: the responder issues it, the joiner shows it, and the operator
/// enters it at the responder to approve. `Debug` does not show it.
#[derive(Clone, PartialEq, Eq)]
pub struct UserCode(String);

impl UserCode {
    /// A code as the responder sent it; the joiner shows it as it is.
    pub(crate) fn issued(text: String) -> Self {
        Self(text)
    }

    /// A random user code of `alphabet`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOptions`] for a meaningless alphabet, [`Error::Random`].
    pub fn generate(alphabet: &CodeAlphabet, random: &Random) -> Result<Self, Error> {
        alphabet.generate(random).map(Self)
    }

    /// `text` as a user code of `alphabet`, matched exactly.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidCode`] when it is not exactly `len` characters of the alphabet.
    pub fn parse(alphabet: &CodeAlphabet, text: &str) -> Result<Self, Error> {
        alphabet.parse(text).map(Self)
    }

    /// The characters, to show.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for UserCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("UserCode(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_exactly_over_their_alphabet() {
        let alphabet = CodeAlphabet::user_code();
        assert!(UserCode::parse(&alphabet, "WDJBMJHT").is_ok());
        for bad in [
            "wdjbmjht",
            "WDJB-MJHT",
            "WDJB MJHT",
            "WDJBMJH",
            "WDJBMJHTA",
            "AAAAAAAA",
        ] {
            assert!(matches!(
                UserCode::parse(&alphabet, bad),
                Err(Error::InvalidCode)
            ));
        }
        assert!(Pin::parse(&CodeAlphabet::pin(), "482913").is_ok());
        assert!(Pin::parse(&CodeAlphabet::pin(), "48291a").is_err());
    }

    #[test]
    fn combinations_are_reported_not_refused() {
        assert_eq!(CodeAlphabet::pin().combinations(), 1_000_000);
        assert_eq!(CodeAlphabet::user_code().combinations(), 20u128.pow(8));
        let tiny = CodeAlphabet::new("01", 1);
        assert!(tiny.check().is_ok());
        assert_eq!(tiny.combinations(), 2);
        let huge = CodeAlphabet::new("01", 1000);
        assert_eq!(huge.combinations(), u128::MAX);
    }

    #[test]
    fn meaningless_alphabets_are_errors() {
        for (chars, len) in [("", 6), ("0123", 0), ("0012", 4), ("01\n", 4)] {
            let alphabet = CodeAlphabet {
                chars: chars.to_owned(),
                len,
            };
            assert!(matches!(alphabet.check(), Err(Error::InvalidOptions)));
        }
    }

    #[test]
    fn generated_codes_use_the_whole_alphabet() {
        let counter = core::sync::atomic::AtomicU32::new(0);
        let random = Random::from_fn(move |out| {
            let value = counter.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
            out.copy_from_slice(&value.to_be_bytes()[..out.len()]);
            Ok(())
        });
        let alphabet = CodeAlphabet::pin();
        let code = Pin::generate(&alphabet, &random).unwrap();
        assert_eq!(code.as_str(), "012345");
        assert!(Pin::parse(&alphabet, code.as_str()).is_ok());
    }
}
