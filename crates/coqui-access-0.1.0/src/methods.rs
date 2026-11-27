//! The words that name how a person proves who they are, as a sign-in policy and a consumer
//! agree on them.
//!
//! A policy may use words of its own beside these; the ones here are what the providers of
//! this repository and the consumers know.

/// A name and a password.
pub const PASSWORD: &str = "password";
/// A client certificate whose key the transport verified.
pub const CERTIFICATE: &str = "certificate";
/// An identity a trusted reverse proxy vouches for.
pub const PROXY: &str = "proxy";
/// A bearer key that is the account.
pub const KEY: &str = "key";
/// A one-time code from an authenticator app.
pub const TOTP: &str = "totp";
/// A one-time recovery code made at enrolment.
pub const RECOVERY_CODE: &str = "recovery_code";
/// A passkey (WebAuthn): a signed answer to a challenge the provider issued. It names its own
/// account, so it is offered before any step has named someone.
pub const PASSKEY: &str = "passkey";

/// Methods that check a code against an account already named, so nothing is offered of them
/// until an earlier step has named someone.
pub const NEEDS_ACCOUNT: &[&str] = &[TOTP, RECOVERY_CODE];
