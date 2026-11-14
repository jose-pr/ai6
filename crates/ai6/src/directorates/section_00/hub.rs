//! Attaching people to a live Run: what the Station serves and what the gateway relays.
//!
//! A person attached to a charge's Run (or a Mission's) is in the same position as one with the
//! harness running in tmux and a terminal connected to it: they get the Run's history, then its
//! live stream, may type into it, answer what it asks and interrupt it; several may be attached
//! at once and detaching leaves the Run running. [`RunHub`] is the Station's side of that, the
//! one trait the Station owns for it: the Station's own implementation keeps each Run's history
//! and attached clients in memory, and a shared Run hub can replace it by implementing the
//! trait. [`HubCommand`] and [`HubFrame`] are what Universal Exports and a Station exchange.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which Run a person attaches to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunTarget {
    /// The standing Run of the charge of `scope`: `m`, `operation:<codename>` or `station:<id>`.
    Charge {
        /// The charge's scope.
        scope: String,
    },
    /// The live Run of a Mission.
    Mission {
        /// The Mission's Operation.
        operation: String,
        /// The Mission's ID.
        mission: String,
    },
}

impl RunTarget {
    /// A key that names the target in a map or a log.
    pub fn key(&self) -> String {
        match self {
            Self::Charge { scope } => format!("charge:{scope}"),
            Self::Mission { mission, .. } => format!("mission:{mission}"),
        }
    }
}

impl fmt::Display for RunTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.key())
    }
}

/// One thing a client is shown, in order.
///
/// `kind` is an open vocabulary: `event` (`data` is an agent event as the harness reported it:
/// its `kind` (`text`, `tool`, `plan`, `completed`, ...), `text` and `tools`), `prompt` (`data`
/// has `from` and `text`: a person's message), `permission` and `answered` (an agent's request
/// and its answer), `attached` and `detached` (a person came or left), `interrupted`,
/// `ready` (the replay is over: `data` has the Run's `busy` state and who is attached),
/// `closed` (the Run is gone) and `error` (a command was refused: `data.reason`). A client
/// passes over a kind it does not know.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubFrame {
    /// The frame's place in its Run's history; only grows. A frame for one client alone has none.
    #[serde(default)]
    pub seq: u64,
    /// When it happened, as Unix seconds.
    pub at: f64,
    /// What it is.
    pub kind: String,
    /// The client it is for, when it is not for all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// What it carries.
    #[serde(default)]
    pub data: Value,
}

impl HubFrame {
    /// A frame of `kind` carrying `data`, with no place in a history yet.
    pub fn new(kind: impl Into<String>, at: f64, data: Value) -> Self {
        Self {
            seq: 0,
            at,
            kind: kind.into(),
            client: None,
            data,
        }
    }

    /// The same frame, for `client` alone.
    #[must_use]
    pub fn to(mut self, client: impl Into<String>) -> Self {
        self.client = Some(client.into());
        self
    }
}

/// What a client asks of a Run.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HubOp {
    /// Joins the Run: its history after `after` (all of it when absent) is replayed, then live
    /// frames follow.
    Attach {
        /// The last frame the client already has.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<u64>,
    },
    /// Leaves it; the Run goes on.
    Detach,
    /// Types `text` into the Run as its user.
    Prompt {
        /// The message.
        text: String,
    },
    /// Answers a permission request.
    Answer {
        /// The request's ID, as its `permission` frame gave it.
        request: u64,
        /// The option chosen, one of those the request offered.
        option: String,
    },
    /// Interrupts the turn under way.
    Interrupt,
}

/// A client's request, as the gateway hands it to the Station running the Run.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubCommand {
    /// The Run.
    pub target: RunTarget,
    /// The client's connection, named by the gateway.
    pub client: String,
    /// The person, as the gateway authenticated them; the Station records and stamps it.
    pub person: String,
    /// What is asked.
    #[serde(flatten)]
    pub op: HubOp,
}

impl HubCommand {
    /// A request of `person` through `client` for `target`.
    pub fn new(
        target: RunTarget,
        client: impl Into<String>,
        person: impl Into<String>,
        op: HubOp,
    ) -> Self {
        Self {
            target,
            client: client.into(),
            person: person.into(),
            op,
        }
    }
}

/// A batch of frames a Station sends the gateway for one Run.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubDelivery {
    /// The sending Station.
    pub station: String,
    /// The Run.
    pub target: RunTarget,
    /// The frames, in order; each names its client or is for every attached one.
    pub frames: Vec<HubFrame>,
}

impl HubDelivery {
    /// `frames` of `target`, sent by `station`.
    pub fn new(station: impl Into<String>, target: RunTarget, frames: Vec<HubFrame>) -> Self {
        Self {
            station: station.into(),
            target,
            frames,
        }
    }
}

/// Why a hub did not do what was asked.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubError {
    /// This Station runs no such Run now.
    NoRun,
    /// The Run has as many clients as `[attach] clients` allows.
    TooManyClients,
    /// As many prompts wait for the Run's turn as `[attach] waiting_prompts` allows.
    QueueFull,
    /// The client is not attached.
    NotAttached,
    /// No such permission request is open: it was answered, or it ended.
    NoRequest,
    /// The answer is not one of the options the request offered.
    NotAnOption,
    /// The Run refused: its harness is gone or would not take it.
    Failed(String),
}

impl fmt::Display for HubError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRun => formatter.write_str("this Station runs no such Run now"),
            Self::TooManyClients => formatter.write_str("the Run has all the clients it allows"),
            Self::QueueFull => formatter.write_str("as many prompts wait as it allows"),
            Self::NotAttached => formatter.write_str("not attached to the Run"),
            Self::NoRequest => formatter.write_str("no such request is open"),
            Self::NotAnOption => formatter.write_str("not an option the request offered"),
            Self::Failed(reason) => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for HubError {}

struct Queue {
    frames: VecDeque<HubFrame>,
    /// The sender is gone: what is queued is the last there will be.
    ended: bool,
    /// The stream is gone: nothing sent is read.
    abandoned: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
    /// Frames the queue holds; `None` for no bound.
    bound: Option<usize>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The sending end of a client's bounded queue of frames.
pub struct FrameSender(Arc<Shared>);

/// The reading end of a client's queue of frames: what a client is shown after its replay.
pub struct FrameStream(Arc<Shared>);

/// Why a frame was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotQueued {
    /// The client is as far behind as the queue allows.
    Full,
    /// Nobody reads the queue any more.
    Gone,
}

/// A queue of frames for one client holding at most `bound` of them (`None` for no bound), so a
/// client that does not read cannot grow without end.
pub fn frame_queue(bound: Option<usize>) -> (FrameSender, FrameStream) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue {
            frames: VecDeque::new(),
            ended: false,
            abandoned: false,
        }),
        ready: Condvar::new(),
        bound,
    });
    (FrameSender(shared.clone()), FrameStream(shared))
}

impl FrameSender {
    /// Queues `frame`, or says why it could not be.
    ///
    /// # Errors
    ///
    /// [`NotQueued::Full`] when the queue holds as many as it may, [`NotQueued::Gone`] when
    /// its stream was dropped.
    pub fn try_send(&self, frame: HubFrame) -> Result<(), NotQueued> {
        let mut queue = self.0.lock();
        if queue.abandoned {
            return Err(NotQueued::Gone);
        }
        if self
            .0
            .bound
            .is_some_and(|bound| queue.frames.len() >= bound)
        {
            return Err(NotQueued::Full);
        }
        queue.frames.push_back(frame);
        drop(queue);
        self.0.ready.notify_all();
        Ok(())
    }
}

impl Drop for FrameSender {
    fn drop(&mut self) {
        self.0.lock().ended = true;
        self.0.ready.notify_all();
    }
}

impl FrameStream {
    /// The next frame, waiting up to `wait` for one.
    ///
    /// # Errors
    ///
    /// [`RecvTimeoutError::Timeout`] when none came, [`RecvTimeoutError::Disconnected`] when the
    /// sender is gone and everything it queued was read.
    pub fn recv_timeout(&self, wait: Duration) -> Result<HubFrame, RecvTimeoutError> {
        let deadline = Instant::now() + wait;
        let mut queue = self.0.lock();
        loop {
            if let Some(frame) = queue.frames.pop_front() {
                return Ok(frame);
            }
            if queue.ended {
                return Err(RecvTimeoutError::Disconnected);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(RecvTimeoutError::Timeout);
            }
            queue = self
                .0
                .ready
                .wait_timeout(queue, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The next frame if one is queued.
    ///
    /// # Errors
    ///
    /// [`TryRecvError::Empty`], or [`TryRecvError::Disconnected`] once the sender is gone and
    /// everything it queued was read.
    pub fn try_recv(&self) -> Result<HubFrame, TryRecvError> {
        let mut queue = self.0.lock();
        match queue.frames.pop_front() {
            Some(frame) => Ok(frame),
            None if queue.ended => Err(TryRecvError::Disconnected),
            None => Err(TryRecvError::Empty),
        }
    }

    /// Everything queued now.
    pub fn drain(&self) -> Vec<HubFrame> {
        self.0.lock().frames.drain(..).collect()
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        self.0.lock().abandoned = true;
    }
}

/// What a client gets on attaching.
#[non_exhaustive]
pub struct Attachment {
    /// The history the client lacks, oldest first.
    pub replay: Vec<HubFrame>,
    /// Frames from here on. None is lost or repeated between `replay` and these.
    pub live: FrameStream,
    /// The persons attached now, by client.
    pub attached: BTreeMap<String, String>,
}

impl Attachment {
    /// An attachment of `replay` then `live`.
    pub fn new(
        replay: Vec<HubFrame>,
        live: FrameStream,
        attached: BTreeMap<String, String>,
    ) -> Self {
        Self {
            replay,
            live,
            attached,
        }
    }
}

/// The Station's side of attaching: one live Run served to any number of clients.
///
/// Every call names the person the gateway authenticated and the connection they came by; the
/// hub trusts neither beyond stamping what they do. Authorization is the gateway's.
pub trait RunHub: Send + Sync {
    /// Joins `client` of `person` to `target`.
    ///
    /// # Errors
    ///
    /// [`HubError::NoRun`] when this Station runs no such Run, [`HubError::TooManyClients`] when
    /// it has all the clients it allows.
    fn attach(
        &self,
        target: &RunTarget,
        client: &str,
        person: &str,
        after: Option<u64>,
    ) -> Result<Attachment, HubError>;

    /// Takes `client` off `target`; the Run goes on. Detaching one that is not attached is fine.
    fn detach(&self, target: &RunTarget, client: &str);

    /// Types `text` into the Run as `person`. It is one turn of the Run's own provider session,
    /// begun when the Run is free: a prompt made while a turn runs waits behind it.
    ///
    /// # Errors
    ///
    /// [`HubError::NotAttached`], [`HubError::QueueFull`] or [`HubError::Failed`].
    fn prompt(
        &self,
        target: &RunTarget,
        client: &str,
        person: &str,
        text: &str,
    ) -> Result<(), HubError>;

    /// Answers the permission request `request` with `option`. The first answer to a request
    /// decides it.
    ///
    /// # Errors
    ///
    /// [`HubError::NotAttached`], [`HubError::NoRequest`] when it was answered already or ended,
    /// [`HubError::NotAnOption`].
    fn answer(
        &self,
        target: &RunTarget,
        client: &str,
        person: &str,
        request: u64,
        option: &str,
    ) -> Result<(), HubError>;

    /// Interrupts the Run's turn under way.
    ///
    /// # Errors
    ///
    /// [`HubError::NotAttached`] or [`HubError::Failed`].
    fn interrupt(&self, target: &RunTarget, client: &str, person: &str) -> Result<(), HubError>;
}
