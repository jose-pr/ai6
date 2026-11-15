use guatiao::library::ProviderError;

use crate::model::{
    AccountInfo, AccountPut, Attempt, AuthorizationDecision, AuthorizationRequest, Description,
    Lookup, Person, PutOutcome, SignInContext, SignInDecision, Verdict, Waiting,
};

/// Checks credentials and returns only a [`Lookup`] reference.
#[guatiao::kind(name = "coqui_authenticator")]
pub trait Authenticator: Send + Sync {
    /// Checks one attempt without returning person details or credential material.
    ///
    /// An error means the authenticator could not find out; it is never a verdict.
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError>;

    /// What this authenticator is and what it takes. Defaults to nothing said.
    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description::default())
    }
}

/// Resolves an authenticated reference into the authoritative [`Person`] record.
#[guatiao::kind(name = "coqui_directory")]
pub trait Directory: Send + Sync {
    /// Returns the person for `lookup`, or `None` when the reference is unknown or the
    /// account is disabled.
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError>;

    /// What this directory is. Defaults to nothing said.
    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description::default())
    }
}

/// Authorizes one action on one resource for a [`Person`] a directory returned.
#[guatiao::kind(name = "coqui_authorizer")]
pub trait Authorizer: Send + Sync {
    /// Evaluates the request using the supplied authoritative person record.
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError>;
}

/// Account management for a provider that keeps its own accounts.
///
/// A separate kind, so authentication providers that keep nothing (an LDAP client, a
/// reverse-proxy header reader) do not implement it. A provider offering this kind is
/// usually also an [`Authenticator`] and a [`Directory`] over the same store.
#[guatiao::kind(name = "coqui_accounts")]
pub trait Accounts: Send + Sync {
    /// The accounts kept, without anything that proves one.
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError>;

    /// Makes an account, or changes the one of that name; a field not said is left as it
    /// is. A provider that makes secrets itself answers with the one it made.
    fn put(&self, account: AccountPut) -> Result<PutOutcome, ProviderError>;

    /// Takes an account away. `GUATIAO_ERR_NOT_FOUND` when none has that name.
    fn remove(&self, name: String) -> Result<(), ProviderError>;

    /// What was shown lately and is nobody's yet, and could be accepted as somebody's with
    /// [`put`](Accounts::put): a peer certificate nobody has accepted. Empty for a
    /// provider that keeps no such thing.
    fn waiting(&self) -> Result<Vec<Waiting>, ProviderError> {
        Ok(Vec::new())
    }
}

/// Decides which methods a sign-in may use and what it still needs, from where and how it
/// is made.
///
/// The consumer asks before any credential is checked (what to offer), again for each
/// method presented, and again once a step has named someone. `Err` means the provider
/// could not find out; a consumer then offers nothing and refuses, never allowing.
#[guatiao::kind(name = "coqui_sign_in_policy")]
pub trait SignInPolicy: Send + Sync {
    /// The methods that may be presented next, or that the sign-in is complete.
    fn decide(&self, context: SignInContext) -> Result<SignInDecision, ProviderError>;
}
