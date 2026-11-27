//! What makes a harness one the stdio provider runs, beyond how it takes an account.

use alloc::string::String;
use alloc::vec::Vec;

use core::time::Duration;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::{
    Environment, ExportPage, ImportReport, LoginMethodInfo, LoginOutcome, LoginPrompter,
    LoginRequest, ProcessSpawner, SessionExport, SessionImport, SignedInCheck,
};

use super::StdioProvider;
use super::accounts::HarnessDef;

/// A harness's own sign-in: the program, the request, and the host's spawner and prompter.
pub type SignIn = fn(
    String,
    LoginRequest,
    Object<dyn ProcessSpawner>,
    Object<dyn LoginPrompter>,
) -> Result<LoginOutcome, ProviderError>;

/// Whether the harness's own login exists, from the variables it reads (the call's over this process's) and the places it
/// keeps one, looking no longer than the time given at a place that may be slow. Starts nothing and reaches no network.
pub type LoginCheck = fn(&Environment, Duration) -> SignedInCheck;

/// Reads a session the host holds into the harness's own store, for a harness that keeps one the provider can write.
/// `UNSUPPORTED` or `INTERNAL` falls back to a replay when the settings allow one.
pub type ImportSession = fn(
    &StdioProvider,
    &SessionImport,
    Object<dyn ProcessSpawner>,
) -> Result<ImportReport, ProviderError>;

/// Reads a session out of the harness's own store.
pub type ExportSession = fn(
    &StdioProvider,
    &SessionExport,
    Object<dyn ProcessSpawner>,
) -> Result<ExportPage, ProviderError>;

/// How a harness that keeps an imported session under an id made from the host's UUID spells that id.
#[derive(Debug, Clone, Copy)]
pub struct SessionIds {
    /// The id the harness keeps the session the host knows as this UUID under; `None` for text that is not a UUID.
    pub native: fn(&str) -> Option<String>,
    /// The UUID the host knows the session by, when the harness's id is one `native` made; `None` for every other.
    pub host: fn(&str) -> Option<String>,
}

/// The ways to sign in of a harness that lists none.
#[must_use]
pub fn no_logins() -> Vec<LoginMethodInfo> {
    Vec::new()
}

/// A harness the stdio provider runs: how it takes an account ([`HarnessDef`]) and what its account provider and its
/// sessions do beyond that.
#[derive(Debug)]
pub struct StdioDef {
    /// How it starts and what accounts it takes.
    pub accounts: &'static HarnessDef,
    /// What its account provider does, in a sentence.
    pub accounts_description: &'static str,
    /// The ways it signs a person in.
    pub logins: fn() -> Vec<LoginMethodInfo>,
    /// Its sign-in, when it has one; else the account's `login_unsupported` says why not.
    pub sign_in: Option<SignIn>,
    /// Whether its own login exists.
    pub signed_in: LoginCheck,
    /// How it spells the id of a session imported into its store, when it keeps one.
    pub session_ids: Option<SessionIds>,
    /// Writes a session into its store, when it takes one.
    pub import: Option<ImportSession>,
    /// Reads a session out of its store, when it can.
    pub export: Option<ExportSession>,
    /// Why a session cannot be seeded in it, said when no replay is allowed or it has no store the provider writes.
    pub import_unsupported: &'static str,
    /// Why a session cannot be read out of it.
    pub export_unsupported: &'static str,
}
