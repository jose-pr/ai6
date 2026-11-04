//! A route's limit before a turn did anything: the same message goes to her next route.
//!
//! The pump of the harness that met the limit closes it and hands a [`Step`] to the desk, which
//! starts the next route that has room through [`Desk::begin`], the path a harness that fails to
//! start already takes. A conversation with a move under way is in [`Moves`]: a message that
//! arrives waits for the move, and a Stop or a close stops the walk.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;

/// The conversations with a move under way, by conversation.
#[derive(Default)]
pub(super) struct Moves(Mutex<BTreeMap<String, Arc<Move>>>);

/// What the walks of one conversation share until the last of them is done.
#[derive(Default)]
struct Move {
    walks: AtomicUsize,
    interrupted: AtomicBool,
    closed: AtomicBool,
}

/// What the person asked for while a conversation's move was under way.
#[derive(Default, Clone, Copy)]
pub(super) struct Stops {
    pub interrupted: bool,
    pub closed: bool,
}

impl Moves {
    /// Joins `conversation`'s move, begun if none is under way. A harness that meets a limit
    /// meanwhile joins the same one, so a Stop reaches the whole chain.
    pub(super) fn enter(self: &Arc<Self>, conversation: &str) -> Walk {
        let mut moves = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let shared = moves.entry(conversation.to_owned()).or_default().clone();
        shared.walks.fetch_add(1, Ordering::AcqRel);
        Walk {
            moves: self.clone(),
            conversation: conversation.to_owned(),
            shared,
            left: false,
        }
    }

    /// Whether a move of `conversation` is under way.
    pub(super) fn under_way(&self, conversation: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(conversation)
    }

    /// Stops `conversation`'s move for a Stop; `false` when none is under way.
    pub(super) fn interrupt(&self, conversation: &str) -> bool {
        self.mark(conversation, |shared| &shared.interrupted)
    }

    /// Stops `conversation`'s move for a close; `false` when none is under way.
    pub(super) fn close(&self, conversation: &str) -> bool {
        self.mark(conversation, |shared| &shared.closed)
    }

    fn mark(&self, conversation: &str, flag: fn(&Move) -> &AtomicBool) -> bool {
        let moves = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        moves.get(conversation).is_some_and(|shared| {
            flag(shared).store(true, Ordering::Release);
            true
        })
    }
}

/// One walk's place in its conversation's move; leaving it ends the move when it is the last.
pub(super) struct Walk {
    moves: Arc<Moves>,
    conversation: String,
    shared: Arc<Move>,
    left: bool,
}

impl Walk {
    pub(super) fn stops(&self) -> Stops {
        Stops {
            interrupted: self.shared.interrupted.load(Ordering::Acquire),
            closed: self.shared.closed.load(Ordering::Acquire),
        }
    }

    pub(super) fn stopped(&self) -> bool {
        let stops = self.stops();
        stops.interrupted || stops.closed
    }

    /// Leaves the move. What was asked meanwhile comes back when this was the last walk, which
    /// then owes it to the harness it started.
    pub(super) fn finish(mut self) -> Stops {
        self.leave()
    }

    fn leave(&mut self) -> Stops {
        if std::mem::replace(&mut self.left, true) {
            return Stops::default();
        }
        // The count and the map change together, so a Stop is never taken between the two.
        let mut moves = self
            .moves
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shared.walks.fetch_sub(1, Ordering::AcqRel) > 1 {
            return Stops::default();
        }
        moves.remove(&self.conversation);
        self.stops()
    }
}

impl Drop for Walk {
    fn drop(&mut self) {
        self.leave();
    }
}

/// What a pump hands the desk when its harness met a limit before the turn did anything.
pub(super) struct Step {
    pub desk: Arc<Desk>,
    pub runtime: tokio::runtime::Handle,
    /// The message the harness was answering.
    pub job: AssistantJob,
    /// The routes already tried or refused for this message, with why.
    pub failed: Vec<(RouteRef, String)>,
    /// The route that met the limit.
    pub route: RouteRef,
    /// What is told if no route can take the message.
    pub held: Vec<AssistantEvent>,
    pub session_id: Option<String>,
    pub walk: Walk,
}

impl Step {
    pub(super) fn spawn(self) {
        let runtime = self.runtime.clone();
        drop(runtime.spawn(self.run()));
    }

    async fn run(self) {
        let Self {
            desk,
            mut job,
            mut failed,
            route,
            held,
            session_id,
            walk,
            ..
        } = self;
        let conversation = job.conversation_id.clone();
        // A provider Session belongs to the source it ran on.
        job.session_id = None;
        job.route = Some(route.clone());
        failed.push((route.clone(), "it reported a limit".to_owned()));
        let lead = format!(
            "Moneypenny's route {route} is out of quota: she is trying this message on another \
             route."
        );
        match desk.begin(&job, failed, vec![lead], Some(&walk)).await {
            Ok(()) => {
                let stops = walk.finish();
                if stops.closed {
                    desk.close(&conversation);
                } else if stops.interrupted
                    && let Err(reason) = desk.interrupt(&conversation).await
                {
                    (desk.log)(&format!(
                        "assistant conversation {}: {reason}",
                        conversation_digest(&conversation)
                    ));
                }
            }
            Err(reason) => {
                let stops = walk.stops();
                let events = if stops.closed {
                    vec![AssistantEvent::new(EVENT_CLOSED, None)]
                } else if stops.interrupted {
                    vec![
                        AssistantEvent::new(EVENT_INTERRUPTED, None),
                        AssistantEvent::new(EVENT_CLOSED, None),
                    ]
                } else {
                    (desk.log)(&format!(
                        "assistant conversation {}: no route took the message after route \
                         {route}'s limit: {reason}",
                        conversation_digest(&conversation)
                    ));
                    let mut events = held;
                    events.push(AssistantEvent::new(EVENT_CLOSED, None));
                    events
                };
                desk.outbox.send(AssistantReport {
                    route: session_id.as_ref().map(|_| route),
                    run: None,
                    station_id: desk.state.station_id.clone(),
                    conversation_id: conversation,
                    job_id: None,
                    session_id,
                    events,
                });
                drop(walk);
            }
        }
    }
}
