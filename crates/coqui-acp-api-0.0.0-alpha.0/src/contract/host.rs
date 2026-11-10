//! The host's side of a connection under test: the sink and spawner handed to `start`, seen from
//! the suite.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use guatiao::Status;
use guatiao::library::{Object, ProviderError};
use guatiao_channel::{Channel, ChannelSink};
use serde_json::{Value, json};

use crate::spawner::{ProcessSpawner, SpawnSpec, SpawnedProcess, SpawnerInfo};
use crate::{AcpHarness, StartRequest};

/// How long an awaited call may take: generous, for an interpreter under load.
pub(super) const PATIENCE: Duration = Duration::from_secs(30);
/// How long to watch for something that must not happen.
pub(super) const GRACE: Duration = Duration::from_millis(500);

#[derive(Default)]
struct Log {
    received: Vec<Vec<u8>>,
    closed: Option<u32>,
    /// The reason `closed` gave.
    reason: String,
    held: bool,
    away: bool,
    sink_destroyed: bool,
    spawner_destroyed: bool,
    /// Whether the spawner was destroyed when `closed` came.
    spawner_gone_at_close: Option<bool>,
    broken: Vec<String>,
}

pub(super) struct Shared {
    log: Mutex<Log>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Sink(Arc<Shared>);

impl ChannelSink for Sink {
    fn receive(&mut self, bytes: &[u8]) -> Result<(), ProviderError> {
        let mut log = self.0.lock();
        if log.closed.is_some() {
            log.broken.push("receive came after closed".into());
        }
        while log.held {
            log = self
                .0
                .changed
                .wait(log)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let answer = if log.away {
            Err(ProviderError::new(
                Status::GUATIAO_ERR_GONE,
                "the host takes nothing more",
            ))
        } else {
            log.received.push(bytes.to_vec());
            Ok(())
        };
        drop(log);
        self.0.changed.notify_all();
        answer
    }

    fn closed(&mut self, status: u32, reason: &str) {
        let mut log = self.0.lock();
        if log.closed.is_some() {
            log.broken.push(format!("closed({status}) came twice"));
        }
        log.closed = Some(status);
        log.reason = reason.to_owned();
        log.spawner_gone_at_close = Some(log.spawner_destroyed);
        drop(log);
        self.0.changed.notify_all();
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        self.0.lock().sink_destroyed = true;
        self.0.changed.notify_all();
    }
}

/// A spawner that starts nothing, and says when it is destroyed.
struct Spawner(Arc<Shared>);

impl ProcessSpawner for Spawner {
    fn describe(&self) -> Result<SpawnerInfo, ProviderError> {
        Ok(SpawnerInfo::new(
            std::env::consts::OS,
            std::env::consts::ARCH,
            true,
        ))
    }

    fn spawn(&self, _spec: SpawnSpec) -> Result<Object<dyn SpawnedProcess>, ProviderError> {
        Err(ProviderError::new(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "the contract's spawner starts nothing",
        ))
    }
}

impl Drop for Spawner {
    fn drop(&mut self) {
        self.0.lock().spawner_destroyed = true;
        self.0.changed.notify_all();
    }
}

/// One connection under test: what the provider sent, read in order.
pub(super) struct Conn {
    shared: Arc<Shared>,
    pub(super) channel: Option<Arc<Object<dyn Channel>>>,
    read: usize,
}

/// The pieces `start` is handed, and the log they write to.
pub(super) fn handed() -> (
    Arc<Shared>,
    Object<dyn ProcessSpawner>,
    Object<dyn ChannelSink>,
) {
    let shared = Arc::new(Shared {
        log: Mutex::new(Log::default()),
        changed: Condvar::new(),
    });
    let spawner = Spawner(Arc::clone(&shared)).into_object();
    let sink = Sink(Arc::clone(&shared)).into_object();
    (shared, spawner, sink)
}

/// Whether a spawner handed over on its own has been destroyed.
pub(super) struct SpawnerWatch(Arc<Shared>);

impl SpawnerWatch {
    pub(super) fn destroyed(&self) -> bool {
        self.0.lock().spawner_destroyed
    }
}

/// A spawner to hand to one call, and its watch.
pub(super) fn spawner() -> (SpawnerWatch, Object<dyn ProcessSpawner>) {
    let (shared, spawner, sink) = handed();
    drop(sink);
    (SpawnerWatch(shared), spawner)
}

/// Starts a connection, or answers the refusal and whether both handed objects were destroyed.
pub(super) fn start(
    harness: &dyn AcpHarness,
    request: StartRequest,
) -> Result<Conn, (ProviderError, Conn)> {
    let (shared, spawner, sink) = handed();
    match harness.start(request, spawner, sink) {
        Ok(channel) => Ok(Conn {
            shared,
            channel: Some(Arc::new(channel)),
            read: 0,
        }),
        Err(refused) => Err((
            refused,
            Conn {
                shared,
                channel: None,
                read: 0,
            },
        )),
    }
}

/// Starts a connection that must start.
pub(super) fn open(harness: &dyn AcpHarness, request: StartRequest) -> Result<Conn, String> {
    start(harness, request).map_err(|(e, _)| format!("start refused: {}", e.message()))
}

pub(super) fn status(error: &ProviderError) -> u32 {
    error.status.into()
}

impl Conn {
    fn wait_for<T>(
        &self,
        what: &str,
        patience: Duration,
        mut done: impl FnMut(&Log) -> Option<T>,
    ) -> Result<Option<T>, String> {
        let deadline = Instant::now() + patience;
        let mut log = self.shared.lock();
        loop {
            if !log.broken.is_empty() {
                return Err(format!("the channel's rules broke: {:?}", log.broken));
            }
            if let Some(found) = done(&log) {
                return Ok(Some(found));
            }
            let now = Instant::now();
            if now >= deadline {
                let _ = what;
                return Ok(None);
            }
            log = self
                .shared
                .changed
                .wait_timeout(log, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn must<T>(&self, what: &str, done: impl FnMut(&Log) -> Option<T>) -> Result<T, String> {
        self.wait_for(what, PATIENCE, done)?
            .ok_or_else(|| format!("{what} did not come within {PATIENCE:?}"))
    }

    pub(super) fn channel(&self) -> &Object<dyn Channel> {
        self.channel
            .as_deref()
            .expect("a started connection has a channel")
    }

    pub(super) fn shared_channel(&self) -> Arc<Object<dyn Channel>> {
        Arc::clone(
            self.channel
                .as_ref()
                .expect("a started connection has a channel"),
        )
    }

    /// Sends one message, which must be taken.
    pub(super) fn send(&self, message: &Value) -> Result<(), String> {
        self.channel()
            .send(message.to_string().as_bytes())
            .map_err(|e| format!("a send was refused: {}", e.message()))
    }

    /// Sends bytes as they are, which must be taken.
    pub(super) fn send_bytes(&self, bytes: &[u8]) -> Result<(), String> {
        self.channel()
            .send(bytes)
            .map_err(|e| format!("a send was refused: {}", e.message()))
    }

    /// The next message the provider sent, as JSON.
    pub(super) fn next(&mut self) -> Result<Value, String> {
        let at = self.read;
        let bytes = self.must("a message from the provider", |log| {
            log.received.get(at).cloned()
        })?;
        self.read += 1;
        serde_json::from_slice(&bytes).map_err(|_| {
            format!(
                "a message is not one JSON value: {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    }

    /// The next message that answers request `id`, skipping notifications; an agent's request in
    /// between is an error.
    pub(super) fn answer(&mut self, id: u64) -> Result<Value, String> {
        loop {
            let message = self.next()?;
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return Ok(message);
            }
            if message.get("method").is_some() && message.get("id").is_some() {
                return Err(format!(
                    "the agent asked {} while request {id} waited",
                    message["method"]
                ));
            }
        }
    }

    /// Whether nothing more arrives within [`GRACE`].
    pub(super) fn quiet(&self) -> Result<bool, String> {
        let at = self.read;
        Ok(self
            .wait_for("nothing", GRACE, |log| {
                (log.received.len() > at).then_some(())
            })?
            .is_none())
    }

    /// The status of the provider's `closed`.
    pub(super) fn closed(&self) -> Result<u32, String> {
        self.must("closed", |log| log.closed)
    }

    /// The reason the provider's `closed` gave, once it has come.
    pub(super) fn reason(&self) -> Result<String, String> {
        self.closed()?;
        Ok(self.shared.lock().reason.clone())
    }

    /// Whether the spawner was destroyed by the time `closed` came.
    pub(super) fn spawner_gone_at_close(&self) -> Result<bool, String> {
        self.must("closed", |log| log.spawner_gone_at_close)
    }

    /// Waits for both handed objects to be destroyed.
    pub(super) fn destroyed(&self) -> Result<(), String> {
        self.must("the sink's destruction", |log| {
            log.sink_destroyed.then_some(())
        })?;
        self.must("the spawner's destruction", |log| {
            log.spawner_destroyed.then_some(())
        })
    }

    /// `receive` waits from now until [`release`](Self::release).
    pub(super) fn hold(&self) {
        self.shared.lock().held = true;
    }

    pub(super) fn release(&self) {
        self.shared.lock().held = false;
        self.shared.changed.notify_all();
    }

    /// `receive` answers GONE from now on.
    pub(super) fn go_away(&self) {
        self.shared.lock().away = true;
    }

    pub(super) fn received(&self) -> usize {
        self.shared.lock().received.len()
    }

    /// Every message received so far that is a request of the agent's, by method.
    pub(super) fn asked(&self, method: &str) -> bool {
        self.shared.lock().received.iter().any(|bytes| {
            serde_json::from_slice::<Value>(bytes)
                .is_ok_and(|m| m.get("method") == Some(&json!(method)))
        })
    }

    /// Closes from the host's side with `code`, and checks the provider's `closed` follows with it
    /// and that both objects go.
    pub(super) fn close_with(&self, code: u32) -> Result<(), String> {
        self.channel().close(
            code,
            if code == 0 {
                ""
            } else {
                "the contract gives up"
            },
        );
        let closed = self.closed()?;
        if closed != code {
            return Err(format!(
                "closed({closed}) after the host closed with {code}"
            ));
        }
        if !self.spawner_gone_at_close()? {
            return Err("the spawner was still alive when the provider called closed".to_owned());
        }
        self.destroyed()
    }
}
