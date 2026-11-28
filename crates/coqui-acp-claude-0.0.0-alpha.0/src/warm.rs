//! Claude Code processes kept started and initialised ahead of `session/new`.

use alloc::string::ToString;
use alloc::sync::Arc;
use std::sync::LazyLock;

use coqui_acp_harness::{Event, Maker, Relay, Spawn, WarmKey, WarmPolicy, WarmPool, Warmed};

use crate::launch::{ClaudeLaunch, SessionStart, mcp_config};
use crate::session::{ClaudeSession, ClaudeSettings};

/// Every provider instance of the process shares these.
static POOL: LazyLock<WarmPool<WarmSession>> = LazyLock::new(WarmPool::new);

/// A Claude Code that has answered `initialize` and has no turn: its events wait in `relay` for the session that takes it.
pub(crate) struct WarmSession {
    session: ClaudeSession,
    relay: Relay,
}

impl Warmed for WarmSession {
    fn ready(&self) -> bool {
        self.session.alive() && !self.relay.overflowed()
    }

    fn stop(&self) {
        self.session.close();
    }
}

impl WarmSession {
    /// The session, with its events from now on (and those it held) going to `sink`.
    pub(crate) fn adopt(
        self,
        sink: impl Fn(Event) -> bool + Send + Sync + 'static,
    ) -> ClaudeSession {
        self.relay.connect(sink);
        self.session
    }
}

/// What fixes a process as `launch` starts it: everything but the session's own id, which the process takes with it. The MCP
/// configuration and the appended prompt are named by their contents, as the files they are written to are made for each
/// process.
pub(crate) fn key_of(policy: &WarmPolicy, launch: &ClaudeLaunch) -> WarmKey {
    let mut fixed = launch.clone();
    fixed.session = SessionStart::Chosen;
    let mut key = WarmKey::new(policy, &fixed.launch());
    if fixed.mcp_config_file.is_none() && !fixed.mcp_servers.is_empty() {
        key = key.part(
            [
                b"mcp:".as_slice(),
                mcp_config(&fixed.mcp_servers).to_string().as_bytes(),
            ]
            .concat(),
        );
    }
    if fixed.append_system_prompt_file.is_none()
        && let Some(prompt) = fixed
            .append_system_prompt
            .as_deref()
            .filter(|text| !text.is_empty())
    {
        key = key.part([b"prompt:".as_slice(), prompt.as_bytes()].concat());
    }
    key
}

/// Starts a warm process as `launch` says, under a session id of its own, through `spawn`.
fn maker(
    launch: &ClaudeLaunch,
    spawn: Arc<dyn Spawn>,
    settings: ClaudeSettings,
) -> Maker<WarmSession> {
    let launch = launch.clone();
    Arc::new(move || {
        let mut launch = launch.clone();
        launch.session = SessionStart::New(uuid::Uuid::new_v4().to_string());
        let relay = Relay::new();
        let session =
            ClaudeSession::start(&launch, spawn.as_ref(), settings.clone(), relay.sink()).ok()?;
        Some(WarmSession { session, relay })
    })
}

/// A warm process for `launch` when there is one; `None` with warming off.
pub(crate) fn take(launch: &ClaudeLaunch, settings: &ClaudeSettings) -> Option<WarmSession> {
    let policy = &settings.warm;
    POOL.take(&key_of(policy, launch), policy)
}

/// Starts warm processes for `launch` until the policy's number are ready.
pub(crate) fn fill(launch: &ClaudeLaunch, spawn: &Arc<dyn Spawn>, settings: &ClaudeSettings) {
    let policy = &settings.warm;
    POOL.fill(
        &key_of(policy, launch),
        policy,
        &maker(launch, Arc::clone(spawn), settings.clone()),
    );
}

/// How many warm processes are ready for `launch`.
pub(crate) fn ready(launch: &ClaudeLaunch, settings: &ClaudeSettings) -> usize {
    POOL.ready(&key_of(&settings.warm, launch))
}
