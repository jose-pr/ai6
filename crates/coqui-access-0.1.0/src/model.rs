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
    /// A one-time code from an authenticator app, with the account it belongs to, as a
    /// second step of a sign-in.
    pub const OTP: &str = "otp";
    /// A passkey's signed answer to a challenge the same provider issued
    /// ([`Challenger`](crate::Challenger)), presented with [`Attempt::challenge`](crate::Attempt::challenge).
    pub const PASSKEY: &str = "passkey";
}

/// Why a sign-in policy is asked, as [`SignInContext::purpose`] says.
pub mod purposes {
    /// A person is signing in.
    pub const SIGN_IN: &str = "sign_in";
    /// A session that exists is checked again; `done` holds the steps it was opened with.
    pub const RESUME: &str = "resume";
    /// A person sets up or changes a second step of their own.
    pub const ENROL: &str = "enrol";
    /// A sign-in that has passed some steps asks to set up a second step it is waiting for, before any session exists.
    /// Whoever holds those steps may reach it, so a policy allows it only by a rule that names it.
    pub const ENROL_FIRST: &str = "enrol_first";
}

/// What kind of client signs in, as [`SignInContext::client`] says.
pub mod clients {
    /// A web page.
    pub const BROWSER: &str = "browser";
    /// A desktop or mobile application.
    pub const APP: &str = "app";
    /// A program using the interface directly.
    pub const API: &str = "api";
}

/// The names a consumer gives in [`SignInContext::attributes`] that a policy reads.
pub mod attributes {
    /// A boolean: the consumer established [`SignInContext::client`](crate::SignInContext::client) itself, from the
    /// endpoint or listener the request arrived on, and not from anything the client says.
    /// Without it a policy treats the client kind as a claim that can only add steps.
    pub const CLIENT_VERIFIED: &str = "client_verified";
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
    /// The [`methods`](crate::methods) word the consumer presents this credential as, so a
    /// provider serving two methods (a one-time code and a recovery code) tells them apart.
    /// `None` is the provider's own default method.
    pub method: Option<String>,
    /// The challenge a [`Challenger`](crate::Challenger) issued for this sign-in, as it answered
    /// it (`Challenge::challenge`), which the consumer keeps with the pending sign-in and returns
    /// with the answer. `Debug` shows only that there is one.
    pub challenge: Option<String>,
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
            .field("method", &self.method)
            .field("challenge", &self.challenge.as_ref().map(|_| "…"))
            .finish()
    }
}

/// What a consumer asks a [`Challenger`](crate::Challenger) for.
///
/// `Debug` shows the method only.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct ChallengeRequest {
    /// The [`methods`](crate::methods) word the challenge is for; `None` is the provider's own.
    pub method: Option<String>,
    /// The account an earlier step named, when one did; `None` asks for any of the provider's
    /// accounts (a usernameless sign-in).
    pub name: Option<String>,
    /// The remote peer's address, so a provider can bound the challenges one source holds.
    pub peer: Option<String>,
}

impl fmt::Debug for ChallengeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChallengeRequest")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

/// What a [`Challenger`](crate::Challenger) made for one sign-in: a challenge the signed answer
/// is bound to, and the text the client needs to answer it.
///
/// `Debug` shows the expiry only.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Challenge {
    /// The key the consumer keeps with the pending sign-in and returns in
    /// [`Attempt::challenge`] (base64url for a passkey).
    pub challenge: String,
    /// The text the client needs, in the method's own wire format: for a passkey the JSON for
    /// `navigator.credentials.get`. The consumer passes it on unread.
    pub options: String,
    /// When it stops being taken, in seconds since the Unix epoch.
    pub expires_unix: i64,
}

impl fmt::Debug for Challenge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Challenge")
            .field("expires_unix", &self.expires_unix)
            .finish_non_exhaustive()
    }
}

/// One credential of an account that holds several (a passkey), without anything that proves
/// it.
///
/// `Debug` hides the id and the label.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct CredentialInfo {
    /// The credential's id, as the provider names it for `remove:<id>` and `label:<id>`.
    pub id: String,
    /// What the person calls it.
    pub label: Option<String>,
    /// When it was made, in seconds since the Unix epoch.
    pub made_unix: Option<i64>,
    /// When it last signed in, in seconds since the Unix epoch.
    pub used_unix: Option<i64>,
    /// Whether it is a passkey the platform syncs between devices.
    pub synced: Option<bool>,
    /// Whether it is refused for now.
    pub disabled: Option<bool>,
}

impl fmt::Debug for CredentialInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialInfo")
            .field("synced", &self.synced)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

/// The identity an authenticator obtained, in full, for a matcher to tie to a directory
/// account. A claim is what the credential says, not who the person is: the directory
/// decides the account and its groups.
///
/// The names an authenticator that reads a certificate uses: `subject.dn`, `subject.cn` (and
/// the other `subject.<attribute>`), `issuer.dn`, `serial.hex`, `serial.reversed` (the same
/// bytes last first), `san.upn`, `san.email`, `san.dns`, `san.uri`, `san.ip`, `fingerprint`
/// and `spki` (both `sha256:<hex>`), and the leaf in [`certificate`](Claims::certificate).
/// A name an authenticator has nothing for is absent. `Debug` shows counts only.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Claims {
    /// Each claim and its values, in the order the credential gave them.
    #[map(default, skip_serializing_if = BTreeMap::is_empty)]
    pub values: BTreeMap<String, Vec<String>>,
    /// The credential itself, when it has a form of its own: the leaf certificate as DER.
    pub certificate: Option<guatiao::Bytes>,
}

impl Claims {
    /// The values of `name`; empty when the credential did not give it.
    pub fn get(&self, name: &str) -> &[String] {
        self.values.get(name).map_or(&[], Vec::as_slice)
    }
}

impl fmt::Debug for Claims {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Claims")
            .field("values", &self.values.len())
            .field("certificate", &self.certificate.as_ref().map(|_| "…"))
            .finish()
    }
}

/// An identity lookup key returned after an authenticator accepts credentials.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Lookup {
    /// Directory attribute to match, such as `id`, `name` or `email`; `claims` when the
    /// authenticator names no account itself and the consumer's matcher decides from
    /// [`claims`](Lookup::claims).
    pub by: String,
    /// Value to resolve in a configured directory; a label for a log when `by` is `claims`.
    pub value: String,
    /// What the credential said, in full, when the authenticator has more than a name.
    pub claims: Option<Claims>,
}

impl fmt::Debug for Lookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Lookup")
            .field("by", &self.by)
            .field("value", &"…")
            .field("claims", &self.claims)
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
    /// Whether an administrator reset the account (its factor was taken away and only an
    /// administrator sets it up again), where the provider keeps that. Absent when the
    /// provider keeps no such mark.
    pub reset: Option<bool>,
    /// When an administrator last cleared the account's failure counts, in seconds since the
    /// Unix epoch, where the provider keeps it.
    pub unlocked_unix: Option<i64>,
    /// The credentials of an account that holds several (passkeys); `None` for a provider whose
    /// accounts hold one.
    pub credentials: Option<Vec<CredentialInfo>>,
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
    /// The label an enrolment shows the person (an authenticator app's account line); the
    /// account's name when absent. Providers that keep no such label ignore it.
    pub display_name: Option<String>,
    /// The caller has checked the account's current secret in this request (a code from it, or
    /// a sign-in that just used it). A provider that lets a put replace a secret the person
    /// holds refuses the replacement without it; one that does not ignores it. Absent is false.
    #[map(default, skip_serializing_if = is_false)]
    pub proved_current: bool,
    /// An administrator's reset: the provider takes the account's factor away and keeps a mark
    /// ([`AccountInfo::reset`]) until a new one is confirmed. A provider that keeps no such
    /// mark refuses it as unsupported. Absent is false.
    #[map(default, skip_serializing_if = is_false)]
    pub reset: bool,
    /// An administrator's unlock: the provider clears the account's failure count and any
    /// delay and notes when ([`AccountInfo::unlocked_unix`]). A provider that counts no
    /// failures refuses it as unsupported. Absent is false.
    #[map(default, skip_serializing_if = is_false)]
    pub unlock: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
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
            .field("display_name", &self.display_name.as_ref().map(|_| "…"))
            .field("proved_current", &self.proved_current)
            .field("reset", &self.reset)
            .field("unlock", &self.unlock)
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
    /// One-time recovery codes the provider made for the account, said this once and kept
    /// nowhere as themselves.
    pub codes: Option<Vec<String>>,
}

impl fmt::Debug for PutOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PutOutcome")
            .field("secret", &self.secret.as_ref().map(|_| "…"))
            .field("codes", &self.codes.as_ref().map(Vec::len))
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

/// One step of a sign-in that already succeeded.
///
/// `Debug` shows the method and provider, never who the step named.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Factor {
    /// The [`methods`](crate::methods) word it was presented as.
    pub method: String,
    /// The provider that accepted it, by the id the consumer asks it under. A rule that
    /// names `providers` counts only a factor a listed provider accepted.
    pub provider: Option<String>,
    /// The [`Person::id`] the step named. A policy refuses a sign-in whose steps name
    /// different people; a step that names none (`None`) is not held against it.
    pub person: Option<String>,
}

impl fmt::Debug for Factor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Factor")
            .field("method", &self.method)
            .field("provider", &self.provider)
            .field("person", &self.person.as_ref().map(|_| "…"))
            .finish()
    }
}

/// What a consumer knows when it asks which methods a sign-in may use, and what is still
/// needed.
///
/// `Debug` shows words and counts, never the address or the person.
#[derive(Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct SignInContext {
    /// Why it is asked: one of [`purposes`](crate::purposes).
    pub purpose: String,
    /// The steps already passed; empty when the sign-in has not begun.
    pub done: Vec<Factor>,
    /// The client's IP address as the consumer determined it, text without a port; `None`
    /// when it could not be determined, and never a placeholder such as `0.0.0.0`.
    pub address: Option<String>,
    /// What kind of client: one of [`clients`](crate::clients), or the consumer's own word.
    pub client: Option<String>,
    /// The consumer's own word for the service signed in to, such as `portal`.
    pub service: Option<String>,
    /// When it is asked, in seconds since the Unix epoch.
    pub time_unix: Option<i64>,
    /// Who the passed steps named, once one did. `None` before that.
    pub person: Option<Person>,
    /// Anything else the consumer says about the sign-in; a policy reads what it knows.
    #[map(default, skip_serializing_if = BTreeMap::is_empty)]
    pub attributes: BTreeMap<String, Value>,
}

impl SignInContext {
    /// Says the [`client`](SignInContext::client) is one the consumer established itself (see
    /// [`attributes::CLIENT_VERIFIED`]); call it only when that is so.
    #[must_use]
    pub fn with_verified_client(mut self) -> Self {
        self.attributes
            .insert(attributes::CLIENT_VERIFIED.to_owned(), Value::from(true));
        self
    }

    /// Whether the consumer said the client is verified; a missing or non-boolean attribute is
    /// not.
    #[must_use]
    pub fn client_is_verified(&self) -> bool {
        self.attributes
            .get(attributes::CLIENT_VERIFIED)
            .is_some_and(|value| bool::try_from(value).unwrap_or(false))
    }
}

impl fmt::Debug for SignInContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignInContext")
            .field("purpose", &self.purpose)
            .field("done", &self.done.len())
            .field("address", &self.address.as_ref().map(|_| "…"))
            .field("client", &self.client)
            .field("service", &self.service)
            .field("time_unix", &self.time_unix.map(|_| "…"))
            .field("person", &self.person.as_ref().map(|_| "…"))
            .field("attributes", &self.attributes.len())
            .finish()
    }
}

/// A method a sign-in may use next.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Offer {
    /// The [`methods`](crate::methods) word.
    pub method: String,
    /// The providers to ask, in order; empty is any provider that takes the method.
    #[map(default, skip_serializing_if = Vec::is_empty)]
    pub providers: Vec<String>,
}

/// What a sign-in may do next.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct SignInDecision {
    /// Whether the steps passed are enough to open the session.
    pub complete: bool,
    /// The methods that may be presented next, empty when none may: with
    /// [`complete`](SignInDecision::complete) false that is a refusal.
    pub methods: Vec<Offer>,
    /// The rule that decided, by name or position, for a log.
    pub rule: Option<String>,
    /// Limits for the session, such as a lifetime; provider-defined.
    #[map(default, skip_serializing_if = BTreeMap::is_empty)]
    pub limits: BTreeMap<String, Value>,
}
