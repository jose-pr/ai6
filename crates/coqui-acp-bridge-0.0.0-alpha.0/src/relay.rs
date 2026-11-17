//! Passes complete messages between a channel and an agent, unread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use coqui_transport::{Channel, Failure};

use crate::agent::{Agent, Output, WriteEnd};
use crate::linger::Backlog;

/// How often each direction looks up to see whether the other has ended.
const POLL: Duration = Duration::from_millis(50);
/// How long a line may wait on a full queue to a peer, after which the peer has stopped taking them.
const PEER_LIMIT: Duration = Duration::from_secs(30);

/// Why one link ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LinkEnd {
    /// The agent's output ended, which means it exited or closed it.
    AgentEnded,
    /// The agent stopped reading its input and a line waited too long for it.
    AgentStuck,
    /// The peer closed in order.
    PeerClosed,
    /// The channel failed. A dialing bridge may connect again.
    Failed(Failure),
    /// A message would have corrupted the line protocol or was over its limit.
    Protocol,
}

impl LinkEnd {
    /// Whether a link that ended like this leaves an agent worth keeping: the peer left, or the path went. A refusal, a peer that
    /// broke the wire format and an agent that ended or stopped reading leave nothing to come back to.
    pub(crate) const fn keeps_agent(self) -> bool {
        match self {
            Self::PeerClosed => true,
            Self::Failed(failure) => failure.is_path(),
            Self::AgentEnded | Self::AgentStuck | Self::Protocol => false,
        }
    }
}

/// Relays until the agent or the link ends. A line the peer could not take stays lost: ACP's own session
/// recovery is the only safe answer to that, so nothing here replays anything.
pub fn relay<C: Channel + ?Sized>(channel: &C, agent: &Agent) -> LinkEnd {
    relay_holding(channel, agent, &mut Backlog::new(0))
}

/// As [`relay`], sending what `backlog` holds before anything newer. What this link could not take stays in it, for the next.
pub(crate) fn relay_holding<C: Channel + ?Sized>(
    channel: &C,
    agent: &Agent,
    backlog: &mut Backlog,
) -> LinkEnd {
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let inbound = scope.spawn(|| {
            let ended = from_peer(channel, agent, &stop);
            stop.store(true, Ordering::SeqCst);
            ended
        });
        let outbound = to_peer(channel, agent, &stop, backlog);
        stop.store(true, Ordering::SeqCst);
        let inbound = inbound.join().unwrap_or(Some(LinkEnd::Protocol));
        match (outbound, inbound) {
            // A refusal closes the channel, so the other direction reports only a lost path: the refusal is why this link ended.
            (Some(out), Some(end)) if out.keeps_agent() && !end.keeps_agent() => end,
            (out, end) => out.or(end).unwrap_or(LinkEnd::PeerClosed),
        }
    })
}

fn from_peer<C: Channel + ?Sized>(
    channel: &C,
    agent: &Agent,
    stop: &AtomicBool,
) -> Option<LinkEnd> {
    while !stop.load(Ordering::SeqCst) {
        match channel.receive(Some(POLL)) {
            Ok(Some(message)) => {
                // One message is one line to the agent: a raw newline inside would forge a second one.
                if message.iter().any(|byte| matches!(byte, b'\n' | b'\r')) {
                    return Some(LinkEnd::Protocol);
                }
                match agent.write_line(&message, stop) {
                    Ok(()) => {}
                    Err(WriteEnd::Ended) => return Some(LinkEnd::AgentEnded),
                    Err(WriteEnd::Stuck) => return Some(LinkEnd::AgentStuck),
                    Err(WriteEnd::Stopped) => return None,
                }
            }
            Ok(None) => return Some(LinkEnd::PeerClosed),
            Err(Failure::Timeout) => {}
            Err(failure) => return Some(LinkEnd::Failed(failure)),
        }
    }
    None
}

fn to_peer<C: Channel + ?Sized>(
    channel: &C,
    agent: &Agent,
    stop: &AtomicBool,
    backlog: &mut Backlog,
) -> Option<LinkEnd> {
    while !stop.load(Ordering::SeqCst) {
        let Some(message) = backlog.next() else {
            break;
        };
        if let Some(end) = deliver(channel, &message, stop) {
            return Some(end);
        }
        drop(message);
        backlog.sent();
    }
    while !stop.load(Ordering::SeqCst) {
        match agent.next_output(POLL) {
            Ok(Output::Line(line)) => {
                if let Some(end) = deliver(channel, &line, stop) {
                    backlog.push(line);
                    return Some(end);
                }
            }
            Ok(Output::TooLong) => return Some(LinkEnd::Protocol),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Some(LinkEnd::AgentEnded),
        }
    }
    None
}

/// Sends one line, waiting on a full queue, so the agent feels a slow peer as back-pressure rather than losing the link.
fn deliver<C: Channel + ?Sized>(channel: &C, line: &[u8], stop: &AtomicBool) -> Option<LinkEnd> {
    let until = Instant::now() + PEER_LIMIT;
    loop {
        match channel.send(line) {
            Ok(()) => return None,
            Err(Failure::Protocol) => return Some(LinkEnd::Protocol),
            Err(Failure::NoProgress) if Instant::now() < until && !stop.load(Ordering::SeqCst) => {
                thread::sleep(POLL);
            }
            Err(failure) => return Some(LinkEnd::Failed(failure)),
        }
    }
}
