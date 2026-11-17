//! What a standing Sleeper's window is told of its source issue: the comments made since the
//! last window that ended done, and no others.
//!
//! A window that ended any other way leaves the mark where it was, so its comments are told
//! again by the next one.

use super::DaemonState;
use super::issues::fenced;
use crate::directorates::registry::mission_issues::IssueReply;
use crate::directorates::registry::runs::RunRecord;

/// What a window is to be briefed with.
pub(super) struct Delta {
    /// The new comments, fenced as outside text, when there are any.
    pub(super) text: Option<String>,
    /// The mark the window's Run record keeps: past it, a later window briefs.
    pub(super) through: Option<String>,
}

/// The place of a comment in the tracker's order.
fn mark(reply: &IssueReply) -> String {
    format!("{}\t{}", reply.created, reply.id)
}

/// The latest mark of a run of `mission_id` that ended done.
fn settled_mark(runs: &[RunRecord], mission_id: &str) -> Option<String> {
    runs.iter()
        .filter(|run| run.mission_id == mission_id)
        .filter(|run| {
            run.result
                .as_ref()
                .is_some_and(|result| result.state == "done")
        })
        .filter_map(|run| Some((run.number, run.comments_through.clone()?)))
        .max_by_key(|(number, _)| *number)
        .map(|(_, mark)| mark)
}

/// The comments of `mission_id`'s linked issues past `settled`, oldest first, at most `shown` of
/// the newest, and the mark that reaches the newest of them.
fn since(
    replies: &[IssueReply],
    settled: Option<&str>,
    shown: usize,
) -> (Vec<IssueReply>, Option<String>) {
    let mut fresh: Vec<&IssueReply> = replies
        .iter()
        .filter(|reply| settled.is_none_or(|settled| mark(reply).as_str() > settled))
        .collect();
    fresh.sort_by_key(|reply| mark(reply));
    let through = fresh
        .last()
        .map(|reply| mark(reply))
        .or_else(|| settled.map(str::to_owned));
    let skip = fresh.len().saturating_sub(shown);
    (fresh.into_iter().skip(skip).cloned().collect(), through)
}

/// The delta for the window `mission_id` is starting; `None` when it has no source comments.
pub(super) fn of(state: &DaemonState, mission_id: &str) -> Option<Delta> {
    let replies = state.intake.issue_replies(mission_id).ok()?;
    let runs = state.intake.runs().ok()?;
    let settled = settled_mark(&runs, mission_id);
    if replies.is_empty() && settled.is_none() {
        return None;
    }
    let shown = state.station.limits().sources.replies_read.or_max();
    let (fresh, through) = since(&replies, settled.as_deref(), shown);
    let text = (!fresh.is_empty()).then(|| {
        let mut text = format!(
            "{} comment{} on the source issue since your last finished window; earlier ones \
             were handled then:",
            fresh.len(),
            if fresh.len() == 1 { "" } else { "s" }
        );
        for reply in &fresh {
            text.push_str(&format!(
                "\n\nComment {} at {}:\n{}",
                reply.id,
                reply.created,
                fenced(&reply.tracker, &reply.item, &reply.author, &reply.body)
            ));
        }
        text
    });
    Some(Delta { text, through })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::mission_issues::ReplyState;
    use crate::directorates::registry::runs::RunResult;

    fn reply(id: &str, created: &str) -> IssueReply {
        IssueReply {
            id: id.to_owned(),
            mission_id: "m".to_owned(),
            tracker: "gh".to_owned(),
            item: "o/r#1".to_owned(),
            author_id: "1".to_owned(),
            author: "ann".to_owned(),
            body: format!("body {id}"),
            created: created.to_owned(),
            state: ReplyState::Ignored,
            operator: false,
        }
    }

    fn run(number: u32, state: &str, through: Option<&str>) -> RunRecord {
        let mut run = crate::directorates::registry::runs::opened_for_test("m", number);
        run.result = Some(RunResult::new(state));
        run.comments_through = through.map(str::to_owned);
        run
    }

    #[test]
    fn a_window_is_told_only_what_is_newer_than_the_last_one_that_ended_done() {
        let all = [
            reply("c1", "2026-11-01T00:00:00Z"),
            reply("c2", "2026-11-02T00:00:00Z"),
            reply("c3", "2026-11-03T00:00:00Z"),
        ];
        let (first, through) = since(&all, None, 10);
        assert_eq!(first.len(), 3, "the first window is told all of them");
        let through = through.unwrap();
        assert!(through.ends_with("c3"));

        let runs = [run(1, "done", Some(&through))];
        let mut later = all.to_vec();
        later.push(reply("c4", "2026-11-04T00:00:00Z"));
        let settled = settled_mark(&runs, "m");
        let (second, next) = since(&later, settled.as_deref(), 10);
        assert_eq!(
            second.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c4"]
        );
        assert!(next.unwrap().ends_with("c4"));

        // A window that failed leaves the mark where the last done one put it.
        let runs = [
            run(1, "done", Some(&through)),
            run(2, "failed", Some("2026-11-04T00:00:00Z\tc4")),
        ];
        assert_eq!(settled_mark(&runs, "m").as_deref(), Some(through.as_str()));
        let (third, _) = since(&later, settled_mark(&runs, "m").as_deref(), 10);
        assert_eq!(third.len(), 1, "c4 is told again");
    }
}
