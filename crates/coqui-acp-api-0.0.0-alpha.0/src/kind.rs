//! The `acp_harness` kind: a provider that drives an ACP agent and speaks ACP itself, one JSON-RPC
//! message per channel message.

use alloc::string::String;
use alloc::vec::Vec;

use guatiao::Secret;
use guatiao::library::{Object, ProviderError};
use guatiao_channel::{Channel, ChannelSink};

use crate::account::{AccountCatalog, AccountRef};
use crate::environment::Environment;
use crate::models::{
    ExportPage, HarnessInfo, ImportReport, NonDefault, SessionExport, SessionImport, SessionQuery,
    SessionRecordFile, StartRequest,
};
use crate::requirement::Requirement;
use crate::spawner::ProcessSpawner;

/// Drives an Agent Client Protocol agent, and serves it as one ACP connection over a channel.
///
/// The host is the ACP client and the provider the agent: each message on the channel, either way,
/// is one whole JSON-RPC 2.0 message as UTF-8 JSON text. The kind never models ACP itself, so a
/// provider in any language implements ACP as the protocol defines it, and the version is agreed by
/// `initialize` on the channel.
///
/// Every method answers for the provider as configured: a host builds an instance from the
/// provider's settings and asks it. Every method is synchronous; a host calls them from a thread
/// of its own. The methods without a default are the ones a host acts on for every harness; a
/// default is kept only where it is a true answer for a harness that has nothing to say.
///
/// Every object a method is handed (`spawner`, `sink`) is the provider's
/// from then on: it destroys each, even when it refuses.
#[guatiao::kind(name = "acp_harness")]
pub trait AcpHarness: Send + Sync {
    /// Who the harness is and what its `initialize` will advertise, with nothing started: for a host
    /// listing its harnesses. Cheap, and the same on every call.
    fn describe(&self) -> Result<HarnessInfo, ProviderError>;

    /// The kinds of account the harness takes, the ways to sign in to make one, and every variable
    /// it reads for a credential. Cheap, the same on every call, and answered as configured. The
    /// kind [`HARNESS_LOGIN`](crate::HARNESS_LOGIN) is always listed.
    fn accounts(&self) -> Result<AccountCatalog, ProviderError>;

    /// Starts one ACP connection: messages the host sends are the client's, and the agent's come back
    /// through `sink`. The programs it drives start through `spawner`, or the provider ignores it.
    ///
    /// `spawner` and `sink` are handed over: the provider destroys both, even when it refuses, and
    /// the spawner by the time it calls `closed`. It returns once the channel takes messages; the
    /// program may start later, at `initialize` or `session/new`, and a failure then is answered in
    /// ACP before the provider closes.
    ///
    /// With `request.account` the provider applies the account (see
    /// [`AccountGrant`](crate::AccountGrant)): it removes every name in its catalog's
    /// `credential_variables` from what it starts, then sets the chosen kind's, writes the kind's
    /// files through the spawner and starts the harness on them. With none, the environment passes
    /// as it is and nothing is cleared.
    ///
    /// It refuses exactly when [`refusal`](Self::refusal) would for the request's requirements and
    /// environment, with the same status: `GUATIAO_ERR_UNSUPPORTED` for a requirement it cannot
    /// hold, `GUATIAO_ERR_BAD_VALUE` for a request it cannot read (a key it does not know, a
    /// session id that is not a UUID, a requirement's value, an account kind it does not list,
    /// settings its schema refuses, a credential that is missing, an account id that is not 1 to
    /// 128 bytes without a control character). It refuses `GUATIAO_ERR_UNSUPPORTED`, with the
    /// reason, an account kind it cannot apply through this spawner (one that needs a directory,
    /// given a spawner without files). It never starts with a requirement left out.
    fn start(
        &self,
        request: StartRequest,
        spawner: Object<dyn ProcessSpawner>,
        sink: Object<dyn ChannelSink>,
    ) -> Result<Object<dyn Channel>, ProviderError>;

    /// Why the harness could not hold a connection to all of `requirements` together, started with
    /// `environment` and running on `account`, in a sentence; `None` when it can. An empty
    /// `environment` asks about the harness's configuration alone, and is answered. A host asks
    /// before it starts a connection, and passes over a harness that would refuse.
    ///
    /// `GUATIAO_ERR_BAD_VALUE` for requirements `start` would refuse as unreadable: a name given
    /// twice, or a value its name does not take; and for an `account` of a kind the harness does
    /// not list. A name the harness does not know is answered with a refusal, never `None`.
    fn refusal(
        &self,
        requirements: Vec<Requirement>,
        environment: Secret<Environment>,
        account: Option<AccountRef>,
    ) -> Result<Option<String>, ProviderError>;

    /// The program the harness starts through the spawner (a path, or a name looked up on the
    /// `PATH` where it runs, the start's environment's `PATH` when that sets one), so a host can say
    /// it cannot be found before it starts a connection; `None` for a harness that starts none.
    fn program(&self) -> Result<Option<String>, ProviderError>;

    /// Seeds a session at the host's UUID from a history, so that a later connection's
    /// `session/resume` or `session/load` of that UUID opens it; an empty history seeds a fresh one.
    /// The provider maps the UUID to the harness's own id and keeps the mapping. `spawner` is handed
    /// over as `start`'s is, for a harness whose files live where its programs run.
    ///
    /// Answers what the seed was made from and what it lost. Refuses `GUATIAO_ERR_UNSUPPORTED`,
    /// with the reason, when the harness cannot import at all or does not read the history's
    /// version; `GUATIAO_ERR_BAD_VALUE` for an id that is not a UUID or already names a session, or
    /// a history it cannot read. Never a session left half seeded, never a silent fallback, and never
    /// an overwrite: a host moving a session back to a harness that still holds it imports at a new
    /// UUID or into a fresh store.
    fn import_session(
        &self,
        session: SessionImport,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ImportReport, ProviderError>;

    /// One page of the history of the session `export` names (by its UUID), in the standard
    /// session-history format, as it is: whole lines, never more than `max_bytes`, the header on
    /// the first page only. The pages, joined, are the export of one page as
    /// large as the whole. `spawner` is handed over as `import_session`'s is.
    /// Refuses `GUATIAO_ERR_NOT_FOUND` for a session the harness does not have,
    /// `GUATIAO_ERR_BAD_VALUE` for an `after` it did not give or that the session has changed
    /// since, and for a next line longer than `max_bytes`, as
    /// [`LINE_TOO_LARGE`](crate::LINE_TOO_LARGE) says, `GUATIAO_ERR_UNSUPPORTED`, with the reason,
    /// when it cannot export.
    fn export_session(
        &self,
        export: SessionExport,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ExportPage, ProviderError>;

    /// Where the harness keeps its own record of the session `query` names (by the session id it
    /// reported, a UUID), when it keeps one and the file is there. The default, for a harness that
    /// keeps none: none.
    fn session_record(
        &self,
        query: SessionQuery,
    ) -> Result<Option<SessionRecordFile>, ProviderError> {
        drop(query);
        Ok(None)
    }

    /// Each way this configured harness departs from its defaults that its settings' schema cannot
    /// show (the detail of a rule a setting holds as text), for the host to report beside the
    /// differences it reads from the schema itself. The default, for settings the schema shows in
    /// full: none.
    fn non_defaults(&self) -> Result<Vec<NonDefault>, ProviderError> {
        Ok(Vec::new())
    }
}
