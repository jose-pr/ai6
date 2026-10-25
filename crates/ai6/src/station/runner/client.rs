//! The Station's side of a runner: how it reaches one, and the [`AgentSession`] through which
//! it drives a Session the runner supervises.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use guatiao::Bytes;
use guatiao::Status;
use guatiao::library::ProviderError;
use serde::Deserialize;

use super::{EventsPage, Launch, SessionReport, Spooled, StartRequest, Upstream};
use crate::directorates::m::identity::SessionId;
use crate::directorates::m::theaters::{RunnerSettings, Theater, TheaterAccess};
use crate::directorates::q::transport::{RunSpec, TransportClient, describe};
use crate::directorates::section_00::agent::{AgentEvent, AgentMessage, AgentSession};

/// How long one read of a runner's events may wait for a new one.
const READ_WAIT_MS: u64 = 1_000;
/// Output a relayed answer may have; a page of events is well under it.
const MAX_CALL_OUTPUT: u64 = 32 * 1024 * 1024;

/// How the Station reaches the runner.
enum Reach {
    /// On the Station's own host: requests go straight to the runner's loopback port.
    Local {
        state_dir: PathBuf,
        command: PathBuf,
    },
    /// On a host a transport reaches: each request is one `ai6 runner call` run there.
    Transport {
        client: Arc<TransportClient>,
        command: String,
        state_dir: Option<String>,
    },
}

/// One runner a Station talks to.
pub(crate) struct RunnerLink {
    reach: Reach,
    plugin_paths: Vec<String>,
    terminal: Option<String>,
}

/// Why a request got no answer from the runner: the link is down, or no runner serves.
#[derive(Debug, Clone)]
pub(crate) struct Unreachable(pub(crate) String);

#[derive(Deserialize)]
struct Envelope {
    status: u16,
    #[serde(default)]
    body: serde_json::Value,
}

impl RunnerLink {
    /// The runner of `theater`, reached through `transport` unless the Theater is the
    /// Station's own host. `None` when the Theater declares no runner.
    pub(crate) fn for_theater(
        theater: &Theater,
        transport: Option<Arc<TransportClient>>,
    ) -> Result<Option<Self>, String> {
        let Some(settings) = &theater.runner else {
            return Ok(None);
        };
        let reach = match &theater.access {
            TheaterAccess::Local => Reach::Local {
                state_dir: match &settings.state_dir {
                    Some(dir) => PathBuf::from(dir),
                    None => super::default_state_dir().map_err(|error| error.to_string())?,
                },
                command: match &settings.command {
                    Some(command) => PathBuf::from(command),
                    None => std::env::current_exe()
                        .map_err(|error| format!("this Station's own binary: {error}"))?,
                },
            },
            _ => Reach::Transport {
                client: transport.ok_or_else(|| {
                    format!(
                        "this Station has no transport to Theater {:?}, so it cannot reach its runner",
                        theater.id
                    )
                })?,
                command: settings.command.clone().unwrap_or_else(|| "ai6".to_owned()),
                state_dir: settings.state_dir.clone(),
            },
        };
        Ok(Some(Self::with_reach(reach, settings)))
    }

    /// A runner on this host serving `state_dir`, started (if ever) with `command`.
    #[cfg(test)]
    pub(crate) fn local(state_dir: PathBuf, command: PathBuf) -> Self {
        Self::with_reach(
            Reach::Local { state_dir, command },
            &RunnerSettings::default(),
        )
    }

    fn with_reach(reach: Reach, settings: &RunnerSettings) -> Self {
        Self {
            reach,
            plugin_paths: settings.plugin_paths.clone(),
            terminal: settings.terminal.clone(),
        }
    }

    /// The terminal provider pinned for the runner, if any.
    pub(crate) fn terminal(&self) -> Option<&str> {
        self.terminal.as_deref()
    }

    /// Sends one request; an answer of any status is `Ok`.
    pub(crate) fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, serde_json::Value), Unreachable> {
        match &self.reach {
            Reach::Local { state_dir, .. } => {
                super::call(state_dir, method, path, body).map_err(Unreachable)
            }
            Reach::Transport {
                client,
                command,
                state_dir,
            } => {
                let mut argv = vec![command.clone(), "runner".to_owned(), "call".to_owned()];
                if let Some(dir) = state_dir {
                    argv.extend(["--state-dir".to_owned(), dir.clone()]);
                }
                argv.extend([method.to_owned(), path.to_owned()]);
                let stdin = body
                    .map(serde_json::to_vec)
                    .transpose()
                    .map_err(|error| Unreachable(error.to_string()))?;
                let output = client
                    .run(&RunSpec {
                        argv,
                        stdin: stdin.map(Bytes),
                        timeout_ms: super::MAX_WAIT.as_millis() as u64 + 30_000,
                        max_output: MAX_CALL_OUTPUT,
                        ..RunSpec::default()
                    })
                    .map_err(|error| Unreachable(describe(&error)))?;
                let stderr = String::from_utf8_lossy(&output.stderr.0).trim().to_owned();
                match output.code {
                    Some(0) => {
                        let envelope: Envelope =
                            serde_json::from_slice(&output.stdout.0).map_err(|error| {
                                Unreachable(format!(
                                    "`{command} runner call` answered badly: {error}"
                                ))
                            })?;
                        Ok((envelope.status, envelope.body))
                    }
                    Some(code) if code == i64::from(super::CALL_NOT_RUNNING) => {
                        Err(Unreachable(stderr))
                    }
                    code => Err(Unreachable(format!(
                        "`{command} runner call` failed ({}): {stderr}",
                        code.map_or_else(|| "no exit status".to_owned(), |code| code.to_string())
                    ))),
                }
            }
        }
    }

    /// Starts the runner unless one already answers.
    pub(crate) fn ensure(&self) -> Result<(), String> {
        let not_running = match self.call("GET", "/v1/health", None) {
            Ok((200, _)) => return Ok(()),
            Ok((status, body)) => return Err(api_error(status, &body)),
            Err(Unreachable(why)) => why,
        };
        let mut arguments: Vec<String> = Vec::new();
        for path in &self.plugin_paths {
            arguments.extend(["--plugin-path".to_owned(), path.clone()]);
        }
        if let Some(terminal) = &self.terminal {
            arguments.extend(["--terminal".to_owned(), terminal.clone()]);
        }
        match &self.reach {
            Reach::Local { state_dir, command } => {
                let mut arguments: Vec<std::ffi::OsString> =
                    arguments.into_iter().map(Into::into).collect();
                arguments.extend(["--state-dir".into(), state_dir.as_os_str().to_owned()]);
                super::spawn_detached(command, &arguments, state_dir).map_err(|error| {
                    format!("the runner could not start ({not_running}): {error}")
                })?;
            }
            Reach::Transport {
                client,
                command,
                state_dir,
            } => {
                let mut argv = vec![
                    command.clone(),
                    "runner".to_owned(),
                    "serve".to_owned(),
                    "--detach".to_owned(),
                ];
                if let Some(dir) = state_dir {
                    argv.extend(["--state-dir".to_owned(), dir.clone()]);
                }
                argv.extend(arguments);
                let output = client
                    .run(&RunSpec {
                        argv,
                        timeout_ms: 60_000,
                        ..RunSpec::default()
                    })
                    .map_err(|error| describe(&error))?;
                if output.code != Some(0) {
                    return Err(format!(
                        "`{command} runner serve --detach` failed ({not_running}): {}",
                        String::from_utf8_lossy(&output.stderr.0).trim()
                    ));
                }
            }
        }
        match self.call("GET", "/v1/health", None) {
            Ok((200, _)) => Ok(()),
            Ok((status, body)) => Err(api_error(status, &body)),
            Err(Unreachable(why)) => Err(format!("the runner started but does not answer: {why}")),
        }
    }

    /// Launches a harness on the runner.
    pub(crate) fn launch(
        &self,
        key: &str,
        handler: &str,
        launch: Launch,
    ) -> Result<SessionReport, String> {
        let request = StartRequest {
            key: key.to_owned(),
            handler: handler.to_owned(),
            terminal: self.terminal.clone(),
            launch,
        };
        let body = serde_json::to_value(&request).map_err(|error| error.to_string())?;
        match self.call("POST", "/v1/sessions", Some(&body)) {
            Ok((201, body)) => serde_json::from_value(body).map_err(|error| error.to_string()),
            Ok((status, body)) => Err(api_error(status, &body)),
            Err(Unreachable(why)) => Err(why),
        }
    }

    /// What the runner knows of Session `key`; `None` when it has no such Session.
    pub(crate) fn status(&self, key: &str) -> Result<Option<SessionReport>, String> {
        match self.call("GET", &format!("/v1/sessions/{key}"), None) {
            Ok((200, body)) => serde_json::from_value(body)
                .map(Some)
                .map_err(|error| error.to_string()),
            Ok((404, _)) => Ok(None),
            Ok((status, body)) => Err(api_error(status, &body)),
            Err(Unreachable(why)) => Err(why),
        }
    }

    /// Points Session `key`'s Cable relay at the Station's endpoint.
    #[cfg_attr(
        not(feature = "cables-mcp"),
        allow(
            dead_code,
            reason = "only a Station with Cable tools repoints the relay"
        )
    )]
    pub(crate) fn set_cable(&self, key: &str, upstream: &Upstream) -> Result<(), String> {
        let body = serde_json::to_value(upstream).map_err(|error| error.to_string())?;
        match self.call("PUT", &format!("/v1/sessions/{key}/cable"), Some(&body)) {
            Ok((204, _)) => Ok(()),
            Ok((status, body)) => Err(api_error(status, &body)),
            Err(Unreachable(why)) => Err(why),
        }
    }
}

fn api_error(status: u16, body: &serde_json::Value) -> String {
    match body.get("error").and_then(serde_json::Value::as_str) {
        Some(error) => format!("the runner refused ({status}): {error}"),
        None => format!("the runner answered {status}"),
    }
}

fn refuse(message: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &message.to_string())
}

/// A Session on a runner, read from its spool.
///
/// A read that cannot reach the runner answers "no event yet", so a dropped link costs only
/// time: the next read that gets through continues from the last event the Station took.
pub(crate) struct RunnerSession {
    link: Arc<RunnerLink>,
    key: String,
    reading: Mutex<Reading>,
    detached: Arc<AtomicBool>,
    relink: Option<Box<dyn Fn() + Send + Sync>>,
}

#[derive(Default)]
struct Reading {
    /// The last event handed to the Station; `None` asks from the runner's cursor.
    after: Option<u64>,
    buffered: VecDeque<Spooled>,
    complete: bool,
    session_id: Option<SessionId>,
    unreachable: bool,
}

impl RunnerSession {
    /// Reads Session `key` from the event after the last one a Station acknowledged.
    pub(crate) fn attach(
        link: Arc<RunnerLink>,
        key: impl Into<String>,
        session_id: Option<SessionId>,
        detached: Arc<AtomicBool>,
    ) -> Self {
        Self {
            link,
            key: key.into(),
            reading: Mutex::new(Reading {
                session_id,
                ..Reading::default()
            }),
            detached,
            relink: None,
        }
    }

    /// Runs `relink` after a read gets through again following one that could not.
    #[cfg_attr(
        not(feature = "cables-mcp"),
        allow(dead_code, reason = "only the Cable forward needs re-opening")
    )]
    pub(crate) fn on_relink(mut self, relink: impl Fn() + Send + Sync + 'static) -> Self {
        self.relink = Some(Box::new(relink));
        self
    }

    fn reading(&self) -> std::sync::MutexGuard<'_, Reading> {
        self.reading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn post(&self, action: &str, body: Option<&serde_json::Value>) -> Result<(), ProviderError> {
        match self
            .link
            .call("POST", &format!("/v1/sessions/{}/{action}", self.key), body)
        {
            Ok((status, _)) if (200..300).contains(&status) => Ok(()),
            Ok((status, body)) => Err(refuse(api_error(status, &body))),
            Err(Unreachable(why)) => Err(refuse(format!("the runner is unreachable: {why}"))),
        }
    }
}

impl AgentSession for RunnerSession {
    fn session_id(&self) -> Result<Option<SessionId>, ProviderError> {
        Ok(self.reading().session_id.clone())
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        let body = serde_json::to_value(&message).map_err(refuse)?;
        self.post("messages", Some(&body))
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let wait = u64::try_from(timeout_ms).unwrap_or(0).min(READ_WAIT_MS);
        let after = {
            let mut reading = self.reading();
            if let Some(spooled) = reading.buffered.pop_front() {
                reading.after = Some(spooled.seq);
                return Ok(Some(spooled.event));
            }
            if reading.complete {
                drop(reading);
                std::thread::sleep(Duration::from_millis(wait));
                return Ok(None);
            }
            reading.after
        };
        let mut path = format!("/v1/sessions/{}/events?timeout_ms={wait}", self.key);
        if let Some(after) = after {
            path.push_str(&format!("&after={after}"));
        }
        match self.link.call("GET", &path, None) {
            Ok((200, body)) => {
                let page: EventsPage = serde_json::from_value(body).map_err(refuse)?;
                let relinked = {
                    let mut reading = self.reading();
                    let relinked = std::mem::take(&mut reading.unreachable);
                    if page.session_id.is_some() {
                        reading.session_id = page.session_id;
                    }
                    reading.complete = page.complete;
                    reading.buffered.extend(page.events);
                    relinked
                };
                if relinked {
                    eprintln!("ai6: runner Session {:?} is reachable again", self.key);
                    if let Some(relink) = &self.relink {
                        relink();
                    }
                }
                let mut reading = self.reading();
                Ok(reading.buffered.pop_front().map(|spooled| {
                    reading.after = Some(spooled.seq);
                    spooled.event
                }))
            }
            Ok((404, _)) => Err(refuse(format!(
                "the runner no longer has Session {:?}",
                self.key
            ))),
            Ok((status, body)) => Err(refuse(api_error(status, &body))),
            Err(Unreachable(why)) => {
                let mut reading = self.reading();
                if !reading.unreachable {
                    reading.unreachable = true;
                    eprintln!(
                        "ai6: runner Session {:?} is unreachable; still waiting: {why}",
                        self.key
                    );
                }
                drop(reading);
                std::thread::sleep(Duration::from_millis(wait.max(100)));
                Ok(None)
            }
        }
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        if self.detached.load(Ordering::Acquire) {
            return Ok(());
        }
        self.post("interrupt", None)
    }

    fn close(&self) -> Result<(), ProviderError> {
        if self.detached.load(Ordering::Acquire) {
            return Ok(());
        }
        // What the Station took is acknowledged, so nothing is left to replay.
        match self.reading().after {
            Some(after) => self.post(&format!("close?after={after}"), None),
            None => self.post("close", None),
        }
    }
}
