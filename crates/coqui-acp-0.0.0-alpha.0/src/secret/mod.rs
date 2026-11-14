//! Secrets an agent can ask the person for, and show them, without the model ever seeing them: an ACP extension between a host and
//! the frontend of the session's owner, and the sealing that carries each secret value to the one party that may read it.
//!
//! The host sends the frontend `_coqui_acp/secret_request` (the person fills in a form) or `_coqui_acp/secret_show_open` and
//! `_coqui_acp/secret_show_deliver` (the person is shown values). The questions are a JSON Schema object whose secret properties are `writeOnly` or `x-sensitive`. Whoever will
//! receive the values makes a key pair for that request and sends the public key with a [`Binding`] to its identity; whoever collects
//! them refuses a key whose holder it does not already know, and seals each secret value to it (HPKE, RFC 9180). No relay on the
//! way, and no agent, sees a secret value.
//!
//! The roles, for a `request`: the host is the [`Receiver`] and the frontend verifies and seals through a [`Verifier`]. For a `show`
//! the frontend is the receiver, so it answers the `open` step with its key, and the host seals in the `deliver` step.
//!
//! This crate does the cryptography and the messages; the handle store, the tools an agent calls and the approvals are the host's.

mod binding;
mod context;
mod error;
mod form;
mod json;
mod seal;
pub mod unprotected;
mod wire;

#[cfg(test)]
mod tests;

pub use binding::{
    BINDING_DH, BINDING_ECDSA, BINDING_LINK, Binding, IdentityId, LinkKey, LinkRole, Method,
    Prover, Verifier, VerifierBuilder,
};
pub use context::Context;
pub use error::Error;
pub use form::{Form, FormField, Limits};
pub use seal::{Collector, DEFAULT_LIFETIME, Opened, Receiver, Recipient, Sealed, SecretValue};
pub use wire::{
    CAPABILITY, Capability, REQUEST_METHOD, RequestParams, RequestReply, SHOW_DELIVER_METHOD,
    SHOW_OPEN_METHOD, ShowDeliver, ShowDone, ShowOpen, ShowOpened, VERSION,
};

use alloc::string::String;
use alloc::vec::Vec;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Bytes as unpadded base64url, the extension's text form of every key, signature and ciphertext.
fn b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The bytes of unpadded base64url text; padding, other alphabets and trailing bits are refused.
fn b64u_decode(text: &str) -> Result<Vec<u8>, Error> {
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| Error::Malformed("not unpadded base64url"))
}
