use core::fmt;

/// Why a secret request could not be read, bound, sealed or opened. No variant holds a value, a key or a signature: only which check
/// refused.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// The form's schema is not one a secret form may have; the text says which rule.
    Schema(&'static str),
    /// A message of the extension is not in the shape the extension writes.
    Malformed(&'static str),
    /// A version, binding method or feature this side does not have.
    Unsupported(&'static str),
    /// A key that is not of the shape its binding method takes.
    Key(&'static str),
    /// The key's holder is not an identity this side knows, by pairing or by pinning. A key a pipe presents proves nothing.
    UnknownIdentity,
    /// The proof does not bind the key to its holder for this request.
    BadProof,
    /// The binding is past its end, or ends later than this side allows.
    Expired,
    /// The binding was made for another collector than this one.
    WrongAudience,
    /// A value that does not fit its field, or a field that is not there or is the other kind.
    Field(&'static str),
    /// Encrypting failed.
    Seal,
    /// The ciphertext is not one this receiver can open for this field of this request: damaged, made for another request or field,
    /// or made for another key.
    Open,
    /// Something is larger than the limit allows.
    TooLarge(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schema(why) => write!(f, "not a secret form: {why}"),
            Self::Malformed(what) => write!(f, "a secret message is malformed: {what}"),
            Self::Unsupported(what) => write!(f, "not supported: {what}"),
            Self::Key(why) => write!(f, "unusable key: {why}"),
            Self::UnknownIdentity => {
                f.write_str("the key's holder is not an identity this side knows")
            }
            Self::BadProof => f.write_str("the key's binding does not verify"),
            Self::Expired => f.write_str("the key's binding is expired or lasts too long"),
            Self::WrongAudience => f.write_str("the key's binding was made for another collector"),
            Self::Field(why) => write!(f, "a field is refused: {why}"),
            Self::Seal => f.write_str("sealing failed"),
            Self::Open => f.write_str("the value cannot be opened for this request and field"),
            Self::TooLarge(what) => write!(f, "{what} is over its limit"),
        }
    }
}

impl core::error::Error for Error {}
