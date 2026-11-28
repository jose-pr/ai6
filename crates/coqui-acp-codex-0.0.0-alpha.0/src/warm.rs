//! Codex app servers kept started and initialised ahead of `session/new`.

use alloc::format;
use alloc::sync::Arc;
use std::sync::LazyLock;

use coqui_acp_harness::{Event, Maker, Relay, Spawn, WarmKey, WarmPolicy, WarmPool, Warmed};

use crate::Error;
use crate::launch::{CodexLaunch, ThreadStart};
use crate::session::{CodexSession, CodexSettings};

/// Every provider instance of the process shares these.
static POOL: LazyLock<WarmPool<WarmServer>> = LazyLock::new(WarmPool::new);

/// A Codex app server that has answered `initialize` and listed its models, and has no thread: its events wait in `relay`
/// for the session that takes it.
pub(crate) struct WarmServer {
    session: CodexSession,
    relay: Relay,
}

impl Warmed for WarmServer {
    fn ready(&self) -> bool {
        self.session.alive() && !self.relay.overflowed()
    }

    fn stop(&self) {
        self.session.close();
    }
}

impl WarmServer {
    /// The session of the thread `launch` says, on this app server, its events from now on (and those held) going to `sink`.
    pub(crate) fn adopt(
        self,
        launch: &CodexLaunch,
        sink: impl Fn(Event) -> bool + Send + Sync + 'static,
    ) -> Result<CodexSession, Error> {
        self.relay.connect(sink);
        self.session.adopt(launch)
    }
}

/// What fixes an app server as `launch` starts it: the program, its arguments and environment, and how it names itself in
/// `initialize`. The directory is not in it: a thread names its own, which is what Codex works in.
pub(crate) fn key_of(policy: &WarmPolicy, launch: &CodexLaunch) -> WarmKey {
    let client = &launch.client_info;
    WarmKey::new(policy, &launch.launch())
        .without_cwd()
        .part(format!(
            "client:{}\0{}\0{}",
            client.name, client.title, client.version
        ))
}

/// Starts a warm app server as `launch` says, in `cwd`, through `spawn`.
fn maker(
    launch: &CodexLaunch,
    cwd: &std::path::Path,
    spawn: Arc<dyn Spawn>,
    settings: CodexSettings,
) -> Maker<WarmServer> {
    let mut launch = launch.clone();
    launch.cwd = cwd.to_path_buf();
    launch.thread = ThreadStart::New;
    Arc::new(move || {
        let relay = Relay::new();
        let session =
            CodexSession::warm_up(&launch, spawn.as_ref(), settings.clone(), relay.sink()).ok()?;
        Some(WarmServer { session, relay })
    })
}

/// A warm app server for `launch` when there is one; `None` with warming off.
pub(crate) fn take(launch: &CodexLaunch, settings: &CodexSettings) -> Option<WarmServer> {
    let policy = &settings.warm;
    POOL.take(&key_of(policy, launch), policy)
}

/// Starts warm app servers for `launch` until the policy's number are ready, each in `cwd`.
pub(crate) fn fill(
    launch: &CodexLaunch,
    cwd: &std::path::Path,
    spawn: &Arc<dyn Spawn>,
    settings: &CodexSettings,
) {
    let policy = &settings.warm;
    POOL.fill(
        &key_of(policy, launch),
        policy,
        &maker(launch, cwd, Arc::clone(spawn), settings.clone()),
    );
}

/// How many warm app servers are ready for `launch`.
pub(crate) fn ready(launch: &CodexLaunch, settings: &CodexSettings) -> usize {
    POOL.ready(&key_of(&settings.warm, launch))
}
