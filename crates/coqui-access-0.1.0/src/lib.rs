//! Kinds and models for authentication, identity lookup, authorization and account
//! management, on [Guatiao](https://github.com/Boriken-Dev/guatiao).
//!
//! A host and every provider library compile against this crate and against nothing else
//! of each other. Providers name `guatiao` through [`guatiao`] (re-exported here), so the
//! two sides cannot drift to different versions.
//!
//! | Kind | Name | Answers |
//! | --- | --- | --- |
//! | [`Authenticator`] | `coqui_authenticator` | Are these credentials valid, and which [`Lookup`] do they name? |
//! | [`Directory`] | `coqui_directory` | Who is the [`Person`] behind a [`Lookup`]? |
//! | [`Authorizer`] | `coqui_authorizer` | May this [`Person`] do this action on this resource? |
//! | [`Accounts`] | `coqui_accounts` | Optional: list, put and remove accounts, list unaccepted peers. |
//! | [`SignInPolicy`] | `coqui_sign_in_policy` | Which methods may this sign-in use, and what does it still need? |
//!
//! Every method is synchronous; a host calls a provider from a blocking thread. Kind names
//! and their floor hashes are an ABI, pinned by this crate's contract test: a provider built
//! against one hash is refused by a host built against another. The hash covers every method
//! of a kind, optional ones included, so a kind that exists is never appended to: it is
//! extended by a new kind, and a model by an `Option` field.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

pub use guatiao;

pub mod error;
mod kind;
mod limit;
pub mod methods;
mod model;
mod subject;

pub use kind::{
    Accounts, AccountsVtable, Authenticator, AuthenticatorVtable, Authorizer, AuthorizerVtable,
    Directory, DirectoryVtable, SignInPolicy, SignInPolicyVtable,
};
pub use limit::Limit;
pub use model::{
    AccountInfo, AccountPut, Attempt, AuthorizationDecision, AuthorizationRequest, Claims,
    Description, Factor, Lookup, Offer, Person, PutOutcome, SignInContext, SignInDecision, Verdict,
    Waiting, clients, purposes, takes,
};
pub use subject::{Subject, SubjectError};

/// The prefix of every kind name in this crate. A host scanning a directory keeps the
/// libraries that declare a kind beginning with it.
pub const KIND_PREFIX: &str = "coqui_";
