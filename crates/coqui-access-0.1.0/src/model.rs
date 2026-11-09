use std::collections::BTreeMap;
use std::fmt;

use guatiao::{FromValue, Schema, ToValue, Value};

/// What an authenticator takes, as [`Description::takes`] names it.
pub mod takes {
    /// A name and a password, once, when a client signs in.
    pub const PASSWORD: &str = "password";
    /// A key by itself, which is the account: for what has nobody to type a password.
    pub const KEY: &str = "key";
    /// The request as it arrived, every time: a header a proxy set, or the certificate a
    /// client proved it holds.
    pub const REQUEST: &str = "request";
}

/// What an authenticator or a directory says it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Description {
    /// A name to show; a host falls back to the provider's own name when absent.
    pub name: Option<String>,
    /// What an authenticator is shown: one of [`takes`](crate::takes).
    pub takes: Option<String>,
}

/// Credentials and request facts presented to an authenticator.
///
/// Secret material and request metadata are omitted from `Debug` output.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Attempt {
    /// Account name supplied by the user, when the method has one.
    pub name: Option<String>,
    /// Password or bearer key supplied for this attempt.
    pub secret: Option<String>,
    /// Request header `(name, value)` pairs in the order received. A header sent twice
    /// appears twice; names are matched case-insensitively.
    pub headers: Vec<(String, String)>,
    /// Remote peer address when available.
    pub peer: Option<String>,
    /// Whether the request arrived over a protected transport.
    pub secure: Option<bool>,
    /// Identity of a client certificate whose key was verified by the transport.
    pub certificate: Option<String>,
    /// The client's certificates as DER, leaf first, then the intermediates it sent, when the
    /// transport proved the leaf's key. A provider that decides trust or reads names from the
    /// certificate takes them from here.
    pub certificates: Option<Vec<guatiao::Bytes>>,
}

impl fmt::Debug for Attempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Attempt")
            .field("name", &self.name.as_ref().map(|_| "…"))
            .field("secret", &self.secret.as_ref().map(|_| "…"))
            .field("headers", &self.headers.len())
            .field("peer", &self.peer.as_ref().map(|_| "…"))
            .field("secure", &self.secure)
            .field("certificate", &self.certificate.as_ref().map(|_| "…"))
            .field("certificates", &self.certificates.as_ref().map(Vec::len))
            .finish()
    }
}

/// An identity lookup key returned after an authenticator accepts credentials.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Lookup {
    /// Directory attribute to match, such as `id`, `name` or `email`.
    pub by: String,
    /// Value to resolve in a configured directory.
    pub value: String,
}

impl fmt::Debug for Lookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Lookup")
            .field("by", &self.by)
            .field("value", &"…")
            .finish()
    }
}

/// Result of checking one credential attempt.
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao, tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    /// This authenticator does not handle the presented method or identity.
    Unknown,
    /// This authenticator recognized the identity but rejected the credentials.
    Invalid,
    /// The credentials are valid; resolve this reference through a directory.
    Valid(Lookup),
}

/// A person record returned only by an authoritative directory.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Person {
    /// Stable directory identity.
    pub id: String,
    /// Canonical account name.
    pub name: String,
    /// Human-readable name, when the directory has one.
    pub display_name: Option<String>,
    /// Email address, when the directory has one.
    pub email: Option<String>,
    /// Directory groups used by authorization policy.
    pub groups: Vec<String>,
    /// Additional directory-owned attributes.
    pub attributes: BTreeMap<String, Value>,
}

impl fmt::Debug for Person {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Person")
            .field("id", &"…")
            .field("name", &"…")
            .field("display_name", &self.display_name.as_ref().map(|_| "…"))
            .field("email", &self.email.as_ref().map(|_| "…"))
            .field("groups", &self.groups.len())
            .field("attributes", &self.attributes.len())
            .finish()
    }
}

/// A resource-scoped authorization request.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct AuthorizationRequest {
    /// Person returned by the authoritative directory.
    pub person: Person,
    /// Requested operation, such as `document.read`.
    pub action: String,
    /// Resource identifier in the host's own scheme, such as `site:north`, or `*` for the
    /// whole service.
    pub resource: String,
    /// The resources `resource` sits under in the host's own hierarchy, nearest first, such
    /// as `["site:europe", "site:world"]` for `site:north`; empty when it has none. An
    /// authorizer may let a grant on one of them reach `resource`.
    #[map(default, skip_serializing_if = Vec::is_empty)]
    pub ancestors: Vec<String>,
}

/// Authorization result and provider-defined limits for the granted action.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct AuthorizationDecision {
    /// Whether policy permits the requested action.
    pub allowed: bool,
    /// Limits such as lease duration or maximum concurrent sessions.
    pub limits: BTreeMap<String, Value>,
}

/// An account as a provider that keeps its own lists it; never with what proves it.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct AccountInfo {
    /// Stable identity, the [`Person::id`] a directory returns for it.
    pub id: String,
    /// Canonical account name.
    pub name: String,
    /// Groups the account is in.
    pub groups: Vec<String>,
    /// Whether it is refused for now.
    pub disabled: Option<bool>,
    /// When it was made, in seconds since the Unix epoch, where the provider keeps it.
    pub made_unix: Option<i64>,
    /// When it stops being taken, in seconds since the Unix epoch, where the provider
    /// keeps one.
    pub expires_unix: Option<i64>,
}

/// An account to make, or what to change of one there is. A field not said is left as it
/// is.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct AccountPut {
    /// The account's canonical name; it selects the account to change.
    pub name: String,
    /// What proves the account from now on: a password, or a peer certificate's
    /// fingerprint. A provider that makes secrets itself takes none and answers with the
    /// one it made.
    pub secret: Option<String>,
    /// Groups, replacing the ones there are.
    pub groups: Option<Vec<String>>,
    /// Whether the account is refused.
    pub disabled: Option<bool>,
    /// When it stops being taken, in seconds since the Unix epoch.
    pub expires_unix: Option<i64>,
}

impl fmt::Debug for AccountPut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountPut")
            .field("name", &self.name)
            .field("secret", &self.secret.as_ref().map(|_| "…"))
            .field("groups", &self.groups)
            .field("disabled", &self.disabled)
            .field("expires_unix", &self.expires_unix)
            .finish()
    }
}

/// What came of putting an account.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct PutOutcome {
    /// The secret the provider made for the account, said this once and kept nowhere as
    /// itself.
    pub secret: Option<String>,
}

impl fmt::Debug for PutOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PutOutcome")
            .field("secret", &self.secret.as_ref().map(|_| "…"))
            .finish()
    }
}

/// Something an authenticator was shown and did not know, kept so that whoever runs the
/// service can accept it as an account.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Waiting {
    /// What it showed, as it is given to [`AccountPut::secret`] to accept it: a
    /// certificate fingerprint.
    pub shown: String,
    /// The address it came from.
    pub peer: Option<String>,
    /// When it last did, in seconds since the Unix epoch.
    pub seen_unix: Option<i64>,
}
