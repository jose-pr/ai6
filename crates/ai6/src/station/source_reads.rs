//! Live reads of an Operation's trackers for Universal Exports: the Headquarters Station, which
//! alone holds tracker tokens, answers each [`SourceRead`] through the Operation's own Source
//! and Tracker providers.

use crate::directorates::m::roster::Roster;
use crate::directorates::registry::sources::SourcePoll;
use crate::directorates::registry::tracker::{
    ReplyPoll, SourceRead, SourceReadAnswer, SourceReadComment, SourceReadItem, Tracker,
};
use crate::station::issues::TrackerBuilder;
use crate::station::limits::SourceLimits;
use crate::station::sources::{SourceBuilder, source_setups};

/// Answers `read` through the Operation's providers, which `build` and `build_tracker` make
/// from each instance's settings. What cannot be read is the answer's `error`, never a panic.
pub(crate) fn answer(
    read: &SourceRead,
    roster: &Roster,
    limits: &SourceLimits,
    build: &SourceBuilder<'_>,
    build_tracker: &TrackerBuilder<'_>,
) -> SourceReadAnswer {
    let refused = |error: String| SourceReadAnswer {
        error: Some(error),
        ..SourceReadAnswer::default()
    };
    let Ok(operation) = roster.operation(&read.operation) else {
        return refused(format!("there is no Operation {}", read.operation));
    };
    let setups: Vec<_> = source_setups(operation, limits)
        .into_iter()
        .flatten()
        .filter(|setup| read.tracker.as_ref().is_none_or(|name| *name == setup.name))
        .collect();
    if setups.is_empty() {
        return refused(match &read.tracker {
            Some(name) => format!("Operation {} has no source {name}", read.operation),
            None => format!("Operation {} has no source", read.operation),
        });
    }
    match &read.item {
        Some(item) => {
            let setup = &setups[0];
            match build_tracker(&setup.provider, &setup.settings) {
                Ok(tracker) => one(tracker.as_ref(), &setup.name, item, read.comments),
                Err(error) => refused(format!("source {}: {error}", setup.name)),
            }
        }
        None => {
            let mut answer = SourceReadAnswer::default();
            let wanted = read.query.as_deref().map(str::to_lowercase);
            for setup in setups.iter().filter(|setup| setup.intake) {
                let found = build(&setup.provider, &setup.settings).and_then(|source| {
                    source
                        .poll(SourcePoll { cursor: None })
                        .map_err(|error| error.to_string())
                });
                let batch = match found {
                    Ok(batch) => batch,
                    Err(error) => {
                        answer.error = Some(format!("source {}: {error}", setup.name));
                        continue;
                    }
                };
                for item in batch.items {
                    if answer.items.len() >= read.items as usize {
                        return answer;
                    }
                    if wanted
                        .as_ref()
                        .is_some_and(|wanted| !item.title.to_lowercase().contains(wanted))
                    {
                        continue;
                    }
                    answer.items.push(SourceReadItem {
                        tracker: setup.name.clone(),
                        id: item.id,
                        title: item.title,
                        labels: item.labels,
                        url: item.url,
                        updated: item.updated,
                        author: item.author,
                        ..SourceReadItem::default()
                    });
                }
            }
            answer
        }
    }
}

/// One item of instance `name` and its latest `most` comments.
fn one(tracker: &dyn Tracker, name: &str, item: &str, most: u32) -> SourceReadAnswer {
    let issue = match tracker.issue(item.to_owned()) {
        Ok(issue) => issue,
        Err(error) => {
            return SourceReadAnswer {
                error: Some(format!("{name}:{item}: {error}")),
                ..SourceReadAnswer::default()
            };
        }
    };
    let mut comments: Vec<SourceReadComment> = Vec::new();
    let mut error = None;
    if most > 0 {
        match tracker.replies(ReplyPoll {
            items: vec![item.to_owned()],
            since: None,
        }) {
            Ok(batch) => {
                let skip = batch.replies.len().saturating_sub(most as usize);
                comments = batch
                    .replies
                    .into_iter()
                    .skip(skip)
                    .map(SourceReadComment::of)
                    .collect();
            }
            Err(error_read) => {
                error = Some(format!("its comments could not be read: {error_read}"))
            }
        }
    }
    SourceReadAnswer {
        item: Some(SourceReadItem {
            tracker: name.to_owned(),
            id: issue.id,
            title: issue.title,
            state: Some(issue.state),
            labels: issue.labels,
            assignees: issue.assignees,
            url: issue.url,
            updated: issue.updated,
            author: issue.author_id,
            body: Some(issue.body),
        }),
        comments,
        items: Vec::new(),
        error,
    }
}

#[cfg(test)]
mod tests;
