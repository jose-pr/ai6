use crate::error::Error;
use crate::id::PeerId;

/// The most key material a store holds for one peer.
pub const MAX_KEY_BYTES: usize = 64 * 1024;

/// The public key material a method gives the trust store for one peer, with the name of the
/// method that understands it.
///
/// The store keeps the bytes as they are and never interprets them: a PEM certificate's DER
/// for `tls`, a raw public key for a key-based method. The peer's id is the SHA-256 of
/// `bytes` ([`id`](KeyMaterial::id)); a method whose ids use another algorithm does not store
/// its keys here.
///
/// Plain data: build it with [`new`](KeyMaterial::new) or the public fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMaterial {
    /// The method name, lowercase letters, digits and `-`, 1 to 32 characters: `tls`.
    pub method: String,
    /// The PEM label the bytes are stored under, uppercase letters, digits and spaces:
    /// `CERTIFICATE`. A standard label keeps the file readable by standard tools.
    pub label: String,
    /// The key bytes, at most [`MAX_KEY_BYTES`].
    pub bytes: Vec<u8>,
}

impl KeyMaterial {
    /// Checked key material.
    ///
    /// # Errors
    ///
    /// [`Error::Material`] when the method or label has characters outside the allowed set,
    /// or `bytes` is empty or longer than [`MAX_KEY_BYTES`].
    pub fn new(
        method: impl Into<String>,
        label: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<Self, Error> {
        let key = Self {
            method: method.into(),
            label: label.into(),
            bytes: bytes.into(),
        };
        key.check()?;
        Ok(key)
    }

    pub(crate) fn check(&self) -> Result<(), Error> {
        let method = !self.method.is_empty()
            && self.method.len() <= 32
            && self
                .method
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        let label = !self.label.is_empty()
            && self.label.len() <= 64
            && !self.label.starts_with(' ')
            && !self.label.ends_with(' ')
            && self
                .label
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b' ');
        if method && label && !self.bytes.is_empty() && self.bytes.len() <= MAX_KEY_BYTES {
            Ok(())
        } else {
            Err(Error::Material)
        }
    }

    /// The id of the peer holding this key: the SHA-256 of the bytes.
    pub fn id(&self) -> PeerId {
        PeerId::of(&self.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_is_checked() {
        assert!(KeyMaterial::new("tls", "CERTIFICATE", [1]).is_ok());
        for (method, label, bytes) in [
            ("TLS", "CERTIFICATE", vec![1]),
            ("", "CERTIFICATE", vec![1]),
            ("tls", "certificate", vec![1]),
            ("tls", "CERTIFICATE", vec![]),
            ("tls", "CERTIFICATE", vec![0; MAX_KEY_BYTES + 1]),
        ] {
            assert!(KeyMaterial::new(method, label, bytes).is_err());
        }
    }
}
