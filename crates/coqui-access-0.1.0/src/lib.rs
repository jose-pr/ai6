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
//!
//! Every method is synchronous; a host calls a provider from a blocking thread. Kind names
//! and the hashes of their required methods are an ABI, pinned by this crate's contract
//! test: a provider built against one hash is refused by a host built against another.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

pub use guatiao;

pub mod error;
mod kind;
mod limit;
mod model;
mod subject;

pub use kind::{
    Accounts, AccountsVtable, Authenticator, AuthenticatorVtable, Authorizer, AuthorizerVtable,
    Directory, DirectoryVtable,
};
pub use limit::Limit;
pub use model::{
    AccountInfo, AccountPut, Attempt, AuthorizationDecision, AuthorizationRequest, Description,
    Lookup, Person, PutOutcome, Verdict, Waiting, takes,
};
pub use subject::{Subject, SubjectError};

/// The prefix of every kind name in this crate. A host scanning a directory keeps the
/// libraries that declare a kind beginning with it.
pub const KIND_PREFIX: &str = "coqui_";
