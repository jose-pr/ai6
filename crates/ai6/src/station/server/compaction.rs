//! How a long-lived conversation (a charge's, a standing Mission's, Moneypenny's) that has grown
//! to its size, or filled its model's context window, is shortened. Three steps, in this order:
//!
//! 1. the running harness compacts its own conversation;
//! 2. if it cannot, compaction fails or leaves the conversation at or past the size, the harness
//!    is cleared and the conversation goes on in the same process from a summary;
//! 3. only if the harness is gone or cannot clear does a fresh Run start with the summary.
//!
//! `[harness] compaction` sets where the order begins: `in_session` at the first step, `clear` at
//! the second, `fresh` at the third. [`shorten`] carries out the first two on a running session;
//! the third is the caller's, since it starts a Run.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use coqui_acp_provider::{
    AgentEvent, AgentMessage, AgentSession, BLOCKED_EVENT, CANCELLED_EVENT, COMPLETED_EVENT,
    FAILED_EVENT, LIMIT_EVENT,
};

use super::DaemonState;
use crate::directorates::m::roster::Agent;
use crate::directorates::medical::usage::UsageTotals;
use crate::directorates::registry::runs::RunRecord;
use crate::station::limits::{CompactionMode, HarnessLimits};
use crate::{
    CLEARED_EVENT, COMPACT_FAILED_EVENT, COMPACTED_EVENT, SessionChange, SessionCommand,
    SessionCommands,
};

/// How long one read of a harness's events waits.
const READ_SLICE_MS: i64 = 250;

/// The size at which a conversation compacts: the smaller of `[harness] compact_at_tokens` and
/// `[harness] compact_at_cap_percent` of the route's `token_cap`; none when neither is set.
pub(super) fn threshold(limits: &HarnessLimits, cap: Option<u64>) -> Option<u64> {
    let by_cap = cap
        .zip(limits.compact_at_cap_percent.get())
        .map(|(cap, percent)| (u128::from(cap) * u128::from(percent) / 100) as u64);
    match (limits.compact_at_tokens.get(), by_cap) {
        (Some(size), Some(cap)) => Some(size.min(cap)),
        (size, cap) => size.or(cap),
    }
}

/// The smallest `token_cap` among an Agent's routes: a conversation may land on any of them.
pub(super) fn smallest_cap(agent: &Agent) -> Option<u64> {
    agent
        .route_list()
        .iter()
        .filter_map(|route| route.token_cap)
        .min()
}

/// What a turn took into the conversation's context: its prompt tokens, cached or not. For a
/// turn of several requests this counts the context once per request, so it is an upper bound.
pub(super) fn context_of(delta: &UsageTotals) -> u64 {
    delta
        .input
        .saturating_add(delta.cache_read)
        .saturating_add(delta.cache_write)
}

/// The tokens a standing Mission's conversation has taken in across the Runs that resumed one
/// provider session, ending at `last`: what each Run added (its input and output, cache reads
/// not counted again). A Run that started a session of its own, or began after its session was
/// compacted or cleared, ends the chain.
pub(super) fn chain_tokens(
    state: &DaemonState,
    operation: Option<&str>,
    mission_id: &str,
    last: &RunRecord,
) -> u64 {
    let Some(usage) = state
        .intake
        .mission_usage(operation, mission_id)
        .ok()
        .flatten()
    else {
        return 0;
    };
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    let mut current = last.clone();
    while seen.insert(current.id.clone()) {
        total = total.saturating_add(usage.runs.get(&current.id).map_or(0, UsageTotals::tokens));
        // A Run that began after its session was compacted counts from what was left.
        if let Some(left) = current.compacted_to {
            total = total.saturating_add(left);
            break;
        }
        let Some(previous) = current
            .predecessor
            .as_deref()
            .and_then(|id| state.intake.run(id).ok().flatten())
            .filter(|previous| previous.provider_id.is_some())
            .filter(|previous| previous.provider_id == current.provider_id)
        else {
            break;
        };
        current = previous;
    }
    total
}

/// What shortened a conversation, in the order the Station tries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// The running harness compacted its own conversation.
    Compacted,
    /// The running harness was cleared and the conversation goes on from a summary.
    Cleared,
    /// A fresh Run replaces the harness.
    Fresh,
}

impl Step {
    /// The word a logbook or timeline entry carries for the step.
    pub(super) fn word(self) -> &'static str {
        match self {
            Self::Compacted => "compacted",
            Self::Cleared => "cleared",
            Self::Fresh => "renewed",
        }
    }
}

/// A conversation shortened in its running harness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Shortened {
    pub(super) step: Step,
    /// The conversation's tokens before, when known.
    pub(super) before: Option<u64>,
    /// The conversation's tokens after, when known.
    pub(super) after: Option<u64>,
    /// Why the steps tried before this one did not do.
    pub(super) skipped: Vec<String>,
}

impl Shortened {
    /// What happened, as a logbook or timeline entry says it.
    pub(super) fn says(&self) -> String {
        says(self.step, self.before, self.after, &self.skipped)
    }
}

/// `step`, the conversation's tokens before and after it, and why the steps before it did not do.
pub(super) fn says(
    step: Step,
    before: Option<u64>,
    after: Option<u64>,
    skipped: &[String],
) -> String {
    let count = |tokens: Option<u64>| {
        tokens.map_or("an unreported number of".to_owned(), |tokens| {
            tokens.to_string()
        })
    };
    let mut text = match step {
        Step::Compacted => format!(
            "the running session compacted its conversation: {} tokens before, {} after",
            count(before),
            count(after)
        ),
        Step::Cleared => format!(
            "the running session was cleared and carries on from a summary: {} tokens before, {} after",
            count(before),
            count(after)
        ),
        Step::Fresh => format!(
            "a fresh Run replaces the session, which held {} tokens, and carries on from a summary",
            count(before)
        ),
    };
    if !skipped.is_empty() {
        text.push_str(&format!(" ({})", skipped.join("; ")));
    }
    text
}

/// Why no step in the running harness shortened the conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Unshortened {
    pub(super) reasons: Vec<String>,
    /// The harness is gone or stopped answering: it must be replaced.
    pub(super) gone: bool,
}

/// How a command a harness was asked for ended.
enum Reply {
    Changed(SessionChange),
    /// The harness reported the command failing, or did not report it, for this reason.
    Failed(String),
}

/// Why a command the handler declares was not carried out.
enum Refusal {
    /// The handler refused the command when it was sent.
    Unsupported(String),
    /// The harness is gone, or the Station is stopping.
    Gone(String),
}

/// Sends `command` to `session` and reads until the turn it makes ends, or `wait` passes with no
/// report. The command's end is matched on its own report: while `pending_end` is set, the turn
/// that came before it (the one that filled the window) has not been seen to end, so the first end
/// event that arrives before any report is that turn's and is passed over, however late it comes.
/// A turn that ends with no `what` report, and a wait that passes without one, are failures of
/// the command, in those words.
fn ask(
    session: &dyn AgentSession,
    command: &SessionCommand,
    (what, wait): (&str, Option<Duration>),
    pending_end: &mut bool,
    seen: &mut dyn FnMut(&AgentEvent),
    stopped: &dyn Fn() -> bool,
) -> Result<Reply, Refusal> {
    session
        .send_message(AgentMessage {
            text: command.encode(),
        })
        .map_err(|error| Refusal::Unsupported(error.to_string()))?;
    let deadline = wait.map(|wait| Instant::now() + wait);
    let mut reply = None;
    loop {
        if stopped() {
            return Err(Refusal::Gone("the Station is stopping".to_owned()));
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            let seconds = wait.map_or(0.0, |wait| wait.as_secs_f64());
            return Ok(Reply::Failed(format!("no {what} report in {seconds} s")));
        }
        let event = match session.next_event(READ_SLICE_MS) {
            Ok(Some(event)) => event,
            Ok(None) => continue,
            Err(error) => return Err(Refusal::Gone(error.to_string())),
        };
        seen(&event);
        match event.kind.as_str() {
            COMPACTED_EVENT | CLEARED_EVENT => {
                reply = Some(Reply::Changed(
                    SessionChange::of(&event).unwrap_or_default(),
                ));
            }
            COMPACT_FAILED_EVENT => reply = Some(Reply::Failed(event.text.unwrap_or_default())),
            FAILED_EVENT => {
                return Err(Refusal::Gone(
                    event
                        .text
                        .unwrap_or_else(|| "the harness failed".to_owned()),
                ));
            }
            COMPLETED_EVENT | BLOCKED_EVENT | CANCELLED_EVENT | LIMIT_EVENT => {
                if reply.is_none() && std::mem::take(pending_end) {
                    continue;
                }
                return Ok(reply.unwrap_or_else(|| {
                    Reply::Failed(format!("the turn ended without a {what} report"))
                }));
            }
            _ => {}
        }
    }
}

/// What [`shorten`] is asked to do for one conversation.
pub(super) struct Plan<'a> {
    /// What the handler declares it carries out; a command it does not declare is never sent.
    pub(super) declared: &'a SessionCommands,
    pub(super) mode: CompactionMode,
    /// The conversation's tokens and the size it must come under, when known.
    pub(super) size: Option<u64>,
    pub(super) limit: Option<u64>,
    /// The turn before the command ended in an event the caller has not read to its end (a
    /// context-window limit is read as the turn's end, and the turn's own end may follow it). The
    /// first end that arrives before the command's report is then that turn's.
    pub(super) pending_end: bool,
    /// How long a command may go without its report (`[harness] compact_wait`); `None` waits
    /// for as long as the harness runs.
    pub(super) wait: Option<Duration>,
}

/// What a handler does not declare, in the words of a skipped step.
fn undeclared(declared: &SessionCommands, what: &str) -> String {
    match &declared.unsupported_because {
        Some(why) => format!("this handler declares no {what}: {why}"),
        None => format!("this handler declares no {what}"),
    }
}

/// Shortens the conversation of the running `session` as far as the plan's mode allows: compacts
/// it in place if the handler declares compaction; if that is not declared, is refused, fails,
/// reports nothing, or leaves the conversation at or past the limit, clears the session if the
/// handler declares that. The caller carries on from a summary after a clear, and starts a fresh
/// Run when this refuses. A command is sent only to a handler that declares it.
///
/// `seen` is given every event the harness reports meanwhile; `stopped` ends the wait.
pub(super) fn shorten(
    session: &dyn AgentSession,
    plan: &Plan<'_>,
    mut seen: impl FnMut(&AgentEvent),
    stopped: impl Fn() -> bool,
) -> Result<Shortened, Unshortened> {
    let Plan {
        declared,
        mode,
        size,
        limit,
        pending_end,
        wait,
    } = *plan;
    let mut pending_end = pending_end;
    if mode == CompactionMode::Fresh {
        return Err(Unshortened {
            reasons: vec!["[harness] compaction is fresh".to_owned()],
            gone: false,
        });
    }
    let mut skipped = Vec::new();
    let gone = |mut skipped: Vec<String>, why: String| {
        skipped.push(format!("the harness is gone: {why}"));
        Unshortened {
            reasons: skipped,
            gone: true,
        }
    };
    if mode == CompactionMode::InSession {
        if !declared.compact {
            skipped.push(undeclared(declared, "compaction"));
        } else {
            match ask(
                session,
                &SessionCommand::Compact { focus: None },
                ("compaction", wait),
                &mut pending_end,
                &mut seen,
                &stopped,
            ) {
                Ok(Reply::Changed(change)) => {
                    let over = change
                        .tokens_after
                        .zip(limit)
                        .filter(|(after, limit)| after >= limit);
                    match over {
                        Some((after, limit)) => skipped.push(format!(
                            "compaction left {after} tokens, at or past {limit}"
                        )),
                        None => {
                            return Ok(Shortened {
                                step: Step::Compacted,
                                before: change.tokens_before.or(size),
                                after: change.tokens_after,
                                skipped,
                            });
                        }
                    }
                }
                Ok(Reply::Failed(why)) if why.is_empty() => {
                    skipped.push("compaction failed".to_owned());
                }
                Ok(Reply::Failed(why)) => skipped.push(format!("compaction failed: {why}")),
                Err(Refusal::Unsupported(why)) => {
                    skipped.push(format!("the handler refused compaction: {why}"));
                }
                Err(Refusal::Gone(why)) => return Err(gone(skipped, why)),
            }
        }
    }
    if !declared.clear {
        skipped.push(undeclared(declared, "clear"));
        return Err(Unshortened {
            reasons: skipped,
            gone: false,
        });
    }
    match ask(
        session,
        &SessionCommand::Clear,
        ("clear", wait),
        &mut pending_end,
        &mut seen,
        &stopped,
    ) {
        Ok(Reply::Changed(change)) => Ok(Shortened {
            step: Step::Cleared,
            before: size,
            after: change.tokens_after.or(Some(0)),
            skipped,
        }),
        Ok(Reply::Failed(why)) => {
            skipped.push(format!("clearing failed: {why}"));
            Err(Unshortened {
                reasons: skipped,
                gone: false,
            })
        }
        Err(Refusal::Unsupported(why)) => {
            skipped.push(format!("the handler refused to clear: {why}"));
            Err(Unshortened {
                reasons: skipped,
                gone: false,
            })
        }
        Err(Refusal::Gone(why)) => Err(gone(skipped, why)),
    }
}

/// What the handler `handler`, built with its defaults or `settings`, declares it carries out on a
/// live session; nothing when it cannot be built or does not say.
pub(super) fn declared(
    state: &DaemonState,
    handler: &str,
    settings: Option<&serde_json::Map<String, serde_json::Value>>,
) -> SessionCommands {
    let built = match settings {
        Some(settings) => state.plugins.handler_with(handler, settings),
        None => state.plugins.handler(handler, None),
    };
    built
        .ok()
        .and_then(|handler| handler.session_commands().ok())
        .unwrap_or_else(SessionCommands::none)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds::Limit;
    use guatiao::Status;
    use guatiao::library::ProviderError;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    fn limits(tokens: Limit, percent: Limit) -> HarnessLimits {
        HarnessLimits {
            compact_at_tokens: tokens,
            compact_at_cap_percent: percent,
            ..HarnessLimits::default()
        }
    }

    #[test]
    fn the_smaller_of_the_size_and_the_share_of_the_cap_decides() {
        let share = limits(Limit::UNBOUNDED, Limit::at(50));
        assert_eq!(threshold(&share, Some(300_000)), Some(150_000));
        assert_eq!(threshold(&share, None), None, "no cap, no size: no bound");
        let both = limits(Limit::at(100_000), Limit::at(50));
        assert_eq!(threshold(&both, Some(300_000)), Some(100_000));
        assert_eq!(threshold(&both, None), Some(100_000));
        let size = limits(Limit::at(100_000), Limit::UNBOUNDED);
        assert_eq!(threshold(&size, Some(10)), Some(100_000));
        let neither = limits(Limit::UNBOUNDED, Limit::UNBOUNDED);
        assert_eq!(threshold(&neither, Some(10)), None);
    }

    #[test]
    fn a_turns_context_counts_cached_prompt_tokens_and_not_its_output() {
        let delta = UsageTotals {
            input: 10,
            output: 1_000,
            cache_read: 200,
            cache_write: 30,
            cost_micro_usd: 0,
        };
        assert_eq!(context_of(&delta), 240);
    }

    /// What a stand-in session answers a command with: the events it reports, or a refusal.
    type Answer = Result<Vec<AgentEvent>, &'static str>;

    /// A session that answers each command from a script and keeps what it was sent.
    struct Scripted {
        compact: Answer,
        clear: Answer,
        /// Events with the time each becomes readable.
        events: Mutex<VecDeque<(Instant, AgentEvent)>>,
        /// How long after a command its answer becomes readable.
        answer_after: Duration,
        sent: Mutex<Vec<SessionCommand>>,
    }

    impl Scripted {
        fn new(compact: Answer, clear: Answer) -> Self {
            Self {
                compact,
                clear,
                events: Mutex::new(VecDeque::new()),
                answer_after: Duration::ZERO,
                sent: Mutex::new(Vec::new()),
            }
        }

        /// An event that becomes readable `after` from now.
        fn queue_after(&self, after: Duration, event: AgentEvent) {
            self.events
                .lock()
                .unwrap()
                .push_back((Instant::now() + after, event));
        }

        fn sent(&self) -> Vec<SessionCommand> {
            self.sent.lock().unwrap().clone()
        }
    }

    fn change(kind: &str, before: Option<u64>, after: Option<u64>) -> AgentEvent {
        SessionChange {
            tokens_before: before,
            tokens_after: after,
            ..SessionChange::default()
        }
        .event(kind, None)
    }

    fn completed() -> AgentEvent {
        AgentEvent::new(COMPLETED_EVENT, None)
    }

    impl AgentSession for Scripted {
        fn session_id(&self) -> Result<Option<String>, ProviderError> {
            Ok(Some("s".to_owned()))
        }

        fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
            let command = SessionCommand::parse(&message.text).unwrap().unwrap();
            self.sent.lock().unwrap().push(command.clone());
            let answer = match command {
                SessionCommand::Compact { .. } => &self.compact,
                SessionCommand::Clear => &self.clear,
            };
            match answer {
                Ok(events) => {
                    for event in events.clone() {
                        self.queue_after(self.answer_after, event);
                    }
                    Ok(())
                }
                Err(why) => Err(ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, why)),
            }
        }

        fn next_event(&self, _timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
            let mut events = self.events.lock().unwrap();
            if events.front().is_some_and(|(at, _)| *at <= Instant::now()) {
                return Ok(events.pop_front().map(|(_, event)| event));
            }
            drop(events);
            std::thread::sleep(Duration::from_millis(5));
            Ok(None)
        }

        fn interrupt(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn close(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    fn both() -> SessionCommands {
        SessionCommands {
            compact: true,
            clear: true,
            unsupported_because: None,
        }
    }

    fn run_with(
        session: &Scripted,
        declared: &SessionCommands,
        mode: CompactionMode,
        limit: Option<u64>,
    ) -> Result<Shortened, Unshortened> {
        shorten(
            session,
            &Plan {
                declared,
                mode,
                size: Some(50_000),
                limit,
                pending_end: false,
                wait: Some(Duration::from_millis(400)),
            },
            |_| {},
            || false,
        )
    }

    fn run(
        session: &Scripted,
        mode: CompactionMode,
        limit: Option<u64>,
    ) -> Result<Shortened, Unshortened> {
        run_with(session, &both(), mode, limit)
    }

    fn compacts(before: u64, after: u64) -> Answer {
        Ok(vec![
            change(COMPACTED_EVENT, Some(before), Some(after)),
            completed(),
        ])
    }

    fn clears() -> Answer {
        Ok(vec![change(CLEARED_EVENT, None, None), completed()])
    }

    #[test]
    fn a_compaction_that_brings_the_conversation_under_the_size_is_the_first_step_and_the_last() {
        let session = Scripted::new(compacts(60_000, 9_000), Ok(vec![]));
        let done = run(&session, CompactionMode::InSession, Some(40_000)).unwrap();
        assert_eq!(
            (done.step, done.before, done.after),
            (Step::Compacted, Some(60_000), Some(9_000))
        );
        assert!(done.skipped.is_empty());
        assert_eq!(session.sent(), [SessionCommand::Compact { focus: None }]);
        assert!(done.says().contains("60000 tokens before, 9000 after"));
    }

    #[test]
    fn a_handler_that_refuses_a_declared_compaction_is_cleared_in_the_same_session() {
        let session = Scripted::new(Err("no compaction here"), clears());
        let done = run(&session, CompactionMode::InSession, Some(40_000)).unwrap();
        assert_eq!(
            (done.step, done.before, done.after),
            (Step::Cleared, Some(50_000), Some(0))
        );
        assert!(done.skipped[0].contains("no compaction here"), "{done:?}");
        assert_eq!(
            session.sent(),
            [
                SessionCommand::Compact { focus: None },
                SessionCommand::Clear
            ]
        );
    }

    #[test]
    fn a_compaction_that_fails_is_followed_by_a_clear() {
        let failed = AgentEvent::new(
            COMPACT_FAILED_EVENT,
            Some("not enough room to summarise".to_owned()),
        );
        let session = Scripted::new(Ok(vec![failed, completed()]), clears());
        let done = run(&session, CompactionMode::InSession, None).unwrap();
        assert_eq!(done.step, Step::Cleared);
        assert!(
            done.skipped[0].contains("compaction failed: not enough room"),
            "{done:?}"
        );
    }

    #[test]
    fn a_compaction_that_leaves_the_conversation_over_the_size_is_followed_by_a_clear() {
        let session = Scripted::new(compacts(60_000, 45_000), clears());
        let done = run(&session, CompactionMode::InSession, Some(40_000)).unwrap();
        assert_eq!(done.step, Step::Cleared);
        assert!(
            done.skipped[0].contains("left 45000 tokens, at or past 40000"),
            "{done:?}"
        );
    }

    /// A harness that declares compaction and ends the turn with no boundary has failed it.
    #[test]
    fn a_declared_compaction_whose_turn_ends_with_no_boundary_is_a_failure_then_a_clear() {
        let session = Scripted::new(Ok(vec![completed()]), clears());
        let done = run(&session, CompactionMode::InSession, None).unwrap();
        assert_eq!(done.step, Step::Cleared);
        assert!(
            done.skipped[0].contains("the turn ended without a compaction report"),
            "{done:?}"
        );
        assert_eq!(session.sent().len(), 2, "the clear step was taken");
    }

    /// One that never reports at all is cut off at the wait, and the clear step follows.
    #[test]
    fn a_declared_compaction_that_never_reports_is_a_failure_at_the_wait_then_a_clear() {
        let session = Scripted::new(Ok(vec![]), clears());
        let done = run(&session, CompactionMode::InSession, None).unwrap();
        assert_eq!(done.step, Step::Cleared);
        assert!(
            done.skipped[0].contains("no compaction report in 0.4 s"),
            "{done:?}"
        );
    }

    /// A harness that declares no compaction is never sent one: the clear step comes directly.
    #[test]
    fn an_undeclared_compaction_is_never_sent_and_the_clear_step_comes_directly() {
        let session = Scripted::new(compacts(60_000, 9_000), clears());
        let declared = SessionCommands {
            compact: false,
            clear: true,
            unsupported_because: Some("it has no /compact".to_owned()),
        };
        let done = run_with(&session, &declared, CompactionMode::InSession, None).unwrap();
        assert_eq!(done.step, Step::Cleared);
        assert_eq!(session.sent(), [SessionCommand::Clear]);
        assert!(
            done.skipped[0].contains("declares no compaction: it has no /compact"),
            "{done:?}"
        );
    }

    #[test]
    fn a_handler_that_declares_neither_is_sent_nothing_and_leaves_the_fresh_run_to_the_caller() {
        let session = Scripted::new(compacts(60_000, 9_000), clears());
        let refused = run_with(
            &session,
            &SessionCommands::none(),
            CompactionMode::InSession,
            None,
        )
        .unwrap_err();
        assert!(!refused.gone, "{refused:?}");
        assert!(session.sent().is_empty());
        assert_eq!(refused.reasons.len(), 2, "{refused:?}");
    }

    #[test]
    fn the_clear_setting_does_not_compact_and_the_fresh_setting_touches_nothing() {
        let session = Scripted::new(compacts(60_000, 9_000), clears());
        let done = run(&session, CompactionMode::Clear, None).unwrap();
        assert_eq!(done.step, Step::Cleared);
        assert_eq!(session.sent(), [SessionCommand::Clear]);

        let untouched = Scripted::new(Ok(vec![]), Ok(vec![]));
        let refused = run(&untouched, CompactionMode::Fresh, None).unwrap_err();
        assert!(!refused.gone);
        assert!(untouched.sent().is_empty());
    }

    #[test]
    fn a_session_that_dies_is_gone_and_no_clear_is_asked_of_it() {
        let session = Scripted::new(
            Ok(vec![AgentEvent::new(
                FAILED_EVENT,
                Some("pipe closed".to_owned()),
            )]),
            Ok(vec![]),
        );
        let refused = run(&session, CompactionMode::InSession, None).unwrap_err();
        assert!(refused.gone, "{refused:?}");
        assert_eq!(session.sent().len(), 1);
    }

    fn run_after_a_limit(session: &Scripted) -> Result<Shortened, Unshortened> {
        shorten(
            session,
            &Plan {
                declared: &both(),
                mode: CompactionMode::InSession,
                size: Some(50_000),
                limit: None,
                pending_end: true,
                wait: Some(Duration::from_secs(5)),
            },
            |_| {},
            || false,
        )
    }

    /// The end of the turn that filled the window arrives after the command is sent, and before
    /// the command's own report: it is not the compaction's end, and the report is attributed to
    /// the compaction.
    #[test]
    fn the_late_end_of_the_previous_turn_is_not_the_end_of_the_compaction() {
        let mut session = Scripted::new(compacts(60_000, 9_000), Ok(vec![]));
        session.answer_after = Duration::from_millis(300);
        session.queue_after(Duration::from_millis(150), completed());
        let done = run_after_a_limit(&session).unwrap();
        assert_eq!(done.step, Step::Compacted);
        assert_eq!(done.after, Some(9_000));
        assert!(done.skipped.is_empty(), "{done:?}");
        assert!(
            session.events.lock().unwrap().is_empty(),
            "the command's own end was read, so the next turn starts clean"
        );
    }

    /// With the previous turn's end already read, a turn that ends with no report is the
    /// command's own, and fails it.
    #[test]
    fn an_end_with_no_report_is_the_commands_when_the_previous_turn_already_ended() {
        let session = Scripted::new(Ok(vec![completed()]), clears());
        let done = run(&session, CompactionMode::InSession, None).unwrap();
        assert_eq!(done.step, Step::Cleared);
        assert!(done.skipped[0].contains("ended without a compaction report"));
    }
}
