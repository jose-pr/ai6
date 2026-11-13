use super::binding::Method;
use super::error::Error;
use super::json::strict;
use alloc::string::String;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const LABEL: &[u8] = b"dev.boriken.coqui.acp/secret/v1 context";

/// What the person is asked or shown, exactly as the host sent it: the session, the request, the agent's message, and the form's
/// schema and layout as the JSON text that was sent. A binding and every sealed field cover the digest of these bytes, so a form
/// changed on the way (a secret field made plain, a different message, another session) fails the binding instead of being drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    /// The session the request belongs to.
    pub session_id: String,
    /// The request's id, 1 to 128 bytes with no control character, unique among the requests of a host.
    pub request_id: String,
    /// What the agent says it needs the values for. Its words, untrusted.
    pub message: String,
    /// The questions as the JSON Schema text that was sent. These exact bytes are hashed, never a re-serialization.
    pub schema: String,
    /// A guatiao-intake form document laying the questions out, as the JSON text that was sent, when there is one.
    pub form: Option<String>,
    /// For a show: the values that are not secret, as the JSON object text that was sent, covered like the rest.
    pub values: Option<String>,
}

/// Reads the plain values of an answer or a show from the JSON object text that was sent.
pub(crate) fn plain_values(text: &str) -> Result<Map<String, Value>, Error> {
    match strict(
        text,
        Error::Malformed,
        "the plain values are not a JSON object",
        "the plain values repeat a key",
    ) {
        Ok(Value::Object(map)) => Ok(map),
        Err(error) => Err(error),
        _ => Err(Error::Malformed("the plain values are not a JSON object")),
    }
}

impl Context {
    /// SHA-256 of the method and every part, each with a 4-byte length, and a byte that says whether a layout and a byte that says whether plain values were sent.
    #[must_use]
    pub fn digest(&self, method: Method) -> [u8; 32] {
        let mut hash = Sha256::new();
        let mut part = |bytes: &[u8]| {
            hash.update((bytes.len() as u32).to_be_bytes());
            hash.update(bytes);
        };
        part(LABEL);
        part(method.as_str().as_bytes());
        part(self.session_id.as_bytes());
        part(self.request_id.as_bytes());
        part(self.message.as_bytes());
        part(self.schema.as_bytes());
        part(&[u8::from(self.form.is_some())]);
        part(self.form.as_deref().unwrap_or("").as_bytes());
        part(&[u8::from(self.values.is_some())]);
        part(self.values.as_deref().unwrap_or("").as_bytes());
        hash.finalize().into()
    }
}
