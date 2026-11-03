//! The record of a Mission's reviews: one per Mission and gate, the panel, each round's votes and
//! where the review stands.
//!
//! Any Station may write one: a vote comes from wherever its reviewer ran, so every write is
//! compared with the revision it read. What decides stays in
//! [`review`](crate::directorates::m::review); this is what a restart carries on from.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::store::{FileRegistry, safe_component};
use crate::directorates::m::review::{Ballot, ReviewGate, ReviewPolicy, Reviewer, Vote};

/// Where a review stands.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// The panel is being asked or has not answered.
    Reviewing,
    /// It cannot go on: no panel can be filled, or no policy read. `why` says what is missing.
    Waiting,
    /// A person decides; `why` says what brought it to them.
    Person,
    /// Sent back; the author's Session is to be resumed with the reasons.
    SendingBack,
    /// The author is working on what it was sent back with.
    Redoing,
    /// It passed. For the plan gate the Mission goes on; for the output gate it is done.
    Passed,
    /// It ended the Mission as failed.
    Refused,
}

impl ReviewStatus {
    /// The name used in records and on the board.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reviewing => "reviewing",
            Self::Waiting => "waiting",
            Self::Person => "person",
            Self::SendingBack => "sending_back",
            Self::Redoing => "redoing",
            Self::Passed => "passed",
            Self::Refused => "refused",
        }
    }
}

/// What a reviewer judges, kept so every round reads the same, and so a restart can ask again.
/// All of it is data: it was written by the author, or by whatever the author read.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewMaterial {
    /// The goals: the Mission's brief as its Agent got it.
    #[serde(default)]
    pub goals: String,
    /// The Agent's plan (plan gate) or final message, its own account of the work and its tests
    /// (output gate).
    #[serde(default)]
    pub submitted: String,
    /// The branch the work is on, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The revision it started from, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// One line per commit since the base.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    /// The change since the base, as far as `[review] diff_bytes` allows.
    #[serde(default)]
    pub diff: String,
    /// Why there is no diff, or that it was cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_note: Option<String>,
}

/// A reviewer's turn the Station asked for.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskedReviewer {
    /// Which time the work was put up, the first being 1.
    pub iteration: u32,
    /// The round.
    pub round: u32,
    /// The seat.
    pub seat: usize,
    /// The review brief's ID.
    pub review_id: String,
    /// When it was asked, as Unix seconds.
    pub at: f64,
}

/// A seat's vote in a round.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedVote {
    /// Which time the work was put up, the first being 1.
    pub iteration: u32,
    /// The round.
    pub round: u32,
    /// The seat.
    pub seat: usize,
    /// The Agent in it.
    pub agent: String,
    /// The model it counted as.
    pub model: String,
    /// The seat's weight.
    pub weight: u32,
    /// The vote.
    pub vote: Vote,
    /// Its reasons: the reviewer's own words, as data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    /// When it was recorded, as Unix seconds.
    #[serde(default)]
    pub at: f64,
}

/// A person's decision over the panel's.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overrule {
    /// Who, as Universal Exports authenticated them.
    pub by: String,
    /// What they decided: `pass`, `redo` or `refuse`.
    pub decision: String,
    /// Why, in their words.
    pub reason: String,
    /// When, as Unix seconds.
    pub at: f64,
}

impl Overrule {
    /// `by`'s decision `decision` (`pass`, `redo` or `refuse`) for `reason`, at `at`.
    pub fn new(
        by: impl Into<String>,
        decision: impl Into<String>,
        reason: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            by: by.into(),
            decision: decision.into(),
            reason: reason.into(),
            at,
        }
    }
}

/// One gate's review of one Mission.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissionReview {
    /// The Mission reviewed.
    pub mission_id: String,
    /// Its Operation.
    pub operation: String,
    /// The gate.
    pub gate: ReviewGate,
    /// What this record's write counts as; 0 before it is first stored.
    #[serde(default)]
    pub revision: u64,
    /// The policy's name.
    pub policy_name: String,
    /// The policy as it read when this work was put up: a change to it reaches the next review.
    pub policy: ReviewPolicy,
    /// The Agent who wrote the work.
    pub author: String,
    /// Where it stands.
    pub status: ReviewStatus,
    /// Why it waits, for a person to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Which time the work is up, the first being 1.
    pub iteration: u32,
    /// How many times the author has been sent back.
    #[serde(default)]
    pub sent_back: u32,
    /// The round of this iteration, the first being 1.
    pub round: u32,
    /// Who sits on the panel; empty until it can be filled.
    #[serde(default)]
    pub panel: Vec<Reviewer>,
    /// Every reviewer asked, in order.
    #[serde(default)]
    pub asked: Vec<AskedReviewer>,
    /// Every vote, in order.
    #[serde(default)]
    pub votes: Vec<RecordedVote>,
    /// What the panel judges.
    #[serde(default)]
    pub material: ReviewMaterial,
    /// Tokens the reviewers' turns have spent.
    #[serde(default)]
    pub tokens: u64,
    /// A person's decision, when one was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrule: Option<Overrule>,
    /// When it opened, as Unix seconds.
    pub opened_at: f64,
}

impl MissionReview {
    /// A review opened now, for `iteration` 1.
    pub fn open(
        (mission_id, operation, gate): (&str, &str, ReviewGate),
        (policy_name, policy): (&str, &ReviewPolicy),
        author: &str,
        material: ReviewMaterial,
        now: f64,
    ) -> Self {
        Self {
            mission_id: mission_id.to_owned(),
            operation: operation.to_owned(),
            gate,
            revision: 0,
            policy_name: policy_name.to_owned(),
            policy: policy.clone(),
            author: author.to_owned(),
            status: ReviewStatus::Reviewing,
            why: None,
            iteration: 1,
            sent_back: 0,
            round: 1,
            panel: Vec::new(),
            asked: Vec::new(),
            votes: Vec::new(),
            material,
            tokens: 0,
            overrule: None,
            opened_at: now,
        }
    }

    /// The seat's vote in the current iteration and round.
    pub fn vote_of(&self, seat: usize) -> Option<&RecordedVote> {
        self.votes.iter().find(|vote| {
            vote.iteration == self.iteration && vote.round == self.round && vote.seat == seat
        })
    }

    /// When the seat was asked in the current iteration and round.
    pub fn asked_of(&self, seat: usize) -> Option<&AskedReviewer> {
        self.asked.iter().find(|asked| {
            asked.iteration == self.iteration && asked.round == self.round && asked.seat == seat
        })
    }

    /// Records a seat's vote in the round it was asked in. A seat has one vote a round, whatever
    /// answers again: `false` when it had voted, or when the review has moved past that round.
    pub fn record_vote(&mut self, asked: (u32, u32), vote: RecordedVote) -> bool {
        if (self.iteration, self.round) != asked
            || self.status != ReviewStatus::Reviewing
            || self.vote_of(vote.seat).is_some()
        {
            return false;
        }
        self.votes.push(vote);
        true
    }

    /// The current round's votes, as [`decide`](crate::directorates::m::review::decide) reads them.
    pub fn ballots(&self) -> Vec<Ballot> {
        self.panel
            .iter()
            .filter_map(|reviewer| {
                let vote = self.vote_of(reviewer.seat)?;
                Some(Ballot::of(reviewer, vote.vote))
            })
            .collect()
    }

    /// Reviewer turns asked for so far, over every round of every iteration.
    pub fn turns(&self) -> u64 {
        self.asked.len() as u64
    }

    /// The reasons of the round before this one that seat `seat` did not write, for the next round
    /// to read: `(agent, vote, reason)`.
    pub fn others_reasons(&self, seat: usize) -> Vec<(String, Vote, String)> {
        self.votes
            .iter()
            .filter(|vote| {
                vote.iteration == self.iteration
                    && vote.round + 1 == self.round
                    && vote.seat != seat
            })
            .flat_map(|vote| {
                let mut said = vote
                    .reasons
                    .iter()
                    .map(|reason| (vote.agent.clone(), vote.vote, reason.clone()))
                    .collect::<Vec<_>>();
                if said.is_empty() {
                    said.push((vote.agent.clone(), vote.vote, String::new()));
                }
                said
            })
            .collect()
    }

    /// The reasons that sent the current iteration back, for the author to read.
    pub fn redo_reasons(&self) -> Vec<String> {
        self.votes
            .iter()
            .filter(|vote| {
                vote.iteration == self.iteration
                    && vote.round == self.round
                    && vote.vote == Vote::Redo
            })
            .flat_map(|vote| vote.reasons.iter().cloned())
            .collect()
    }
}

/// What reviews say of one Agent, for its scorecard: as an author, how often its work was put up
/// and sent back; as a reviewer, how often it voted and how often against what its panel decided.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewTally {
    /// Times its work was put up for review.
    pub put_up: u64,
    /// Times its work was sent back.
    pub sent_back: u64,
    /// Votes it cast that judged (`pass` or `redo`).
    pub votes: u64,
    /// Of those, votes against what the panel decided in that round.
    pub disagreed: u64,
    /// Turns it was asked for and gave no answer that could be read.
    pub absent: u64,
}

/// The tally of every Agent in `reviews`, by codename. A round's decision is what came of it: a
/// redo when the work was sent back after it, a pass when the review passed in it; a round that
/// ended otherwise counts no vote as disagreeing.
pub fn tally(reviews: &[MissionReview]) -> BTreeMap<String, ReviewTally> {
    let mut tallies = BTreeMap::<String, ReviewTally>::new();
    for review in reviews {
        let author = tallies.entry(review.author.clone()).or_default();
        author.put_up += u64::from(review.iteration);
        author.sent_back += u64::from(review.sent_back);
        for vote in &review.votes {
            let decided = if vote.iteration < review.iteration {
                // Put up again after this round: the panel's last word on it was a redo, unless an
                // earlier round of the same iteration went on to another.
                let last_round = review
                    .votes
                    .iter()
                    .filter(|other| other.iteration == vote.iteration)
                    .map(|other| other.round)
                    .max();
                (last_round == Some(vote.round)).then_some(Vote::Redo)
            } else if vote.round == review.round && review.status == ReviewStatus::Passed {
                Some(Vote::Pass)
            } else {
                None
            };
            let reviewer = tallies.entry(vote.agent.clone()).or_default();
            match vote.vote {
                Vote::Pass | Vote::Redo => {
                    reviewer.votes += 1;
                    if decided.is_some_and(|decided| decided != vote.vote) {
                        reviewer.disagreed += 1;
                    }
                }
                Vote::Absent => reviewer.absent += 1,
                Vote::Abstain => {}
            }
        }
    }
    tallies
}

/// Reads and writes Mission reviews.
pub trait MissionReviewStore {
    /// The review of `mission_id` at `gate`, as last written.
    ///
    /// # Errors
    ///
    /// When the Registry cannot be read.
    fn mission_review(
        &self,
        mission_id: &str,
        gate: ReviewGate,
    ) -> io::Result<Option<MissionReview>>;

    /// Every review of `mission_id`.
    ///
    /// # Errors
    ///
    /// When the Registry cannot be read.
    fn mission_reviews(&self, mission_id: &str) -> io::Result<Vec<MissionReview>>;

    /// Writes `review` if what is stored is still at `review.revision`, and returns it at its new
    /// revision; `None` when another Station wrote first, so the caller reads again and decides
    /// again.
    ///
    /// # Errors
    ///
    /// When the Registry cannot be written.
    fn put_mission_review(&self, review: &MissionReview) -> io::Result<Option<MissionReview>>;
}

/// Applies `change` to what is stored until it lands, for a write that does not depend on anything
/// but the record itself. `change` returns `false` to write nothing. A write that keeps losing to
/// others is an error after `tries`.
///
/// # Errors
///
/// When the Registry fails, when there is no such review or when the tries run out.
pub fn update_mission_review(
    store: &dyn MissionReviewStore,
    (mission_id, gate): (&str, ReviewGate),
    tries: usize,
    mut change: impl FnMut(&mut MissionReview) -> bool,
) -> io::Result<Option<MissionReview>> {
    for _ in 0..tries.max(1) {
        let Some(mut review) = store.mission_review(mission_id, gate)? else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Mission {mission_id:?} has no {gate} review"),
            ));
        };
        if !change(&mut review) {
            return Ok(None);
        }
        if let Some(written) = store.put_mission_review(&review)? {
            return Ok(Some(written));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        format!("the {gate} review of Mission {mission_id:?} kept changing as it was written"),
    ))
}

impl FileRegistry {
    fn review_dir(&self) -> PathBuf {
        self.root().join("mission-reviews")
    }

    fn review_path(&self, mission_id: &str, gate: ReviewGate) -> PathBuf {
        self.review_dir().join(format!(
            "{}.{}.json",
            safe_component(mission_id),
            gate.as_str()
        ))
    }
}

impl MissionReviewStore for FileRegistry {
    fn mission_review(
        &self,
        mission_id: &str,
        gate: ReviewGate,
    ) -> io::Result<Option<MissionReview>> {
        match fs::read(self.review_path(mission_id, gate)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn mission_reviews(&self, mission_id: &str) -> io::Result<Vec<MissionReview>> {
        let mut reviews = Vec::new();
        for gate in [ReviewGate::Plan, ReviewGate::Output] {
            reviews.extend(self.mission_review(mission_id, gate)?);
        }
        Ok(reviews)
    }

    fn put_mission_review(&self, review: &MissionReview) -> io::Result<Option<MissionReview>> {
        let _lock = self.lock()?;
        let stored = self.mission_review(&review.mission_id, review.gate)?;
        if stored.map_or(0, |stored| stored.revision) != review.revision {
            return Ok(None);
        }
        let mut written = review.clone();
        written.revision += 1;
        fs::create_dir_all(self.review_dir())?;
        let bytes = serde_json::to_vec_pretty(&written)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.write_atomic(&self.review_path(&review.mission_id, review.gate), &bytes)?;
        Ok(Some(written))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::m::review::PanelSeat;
    use crate::directorates::registry::suite::{Store, backend_tests};

    #[test]
    fn a_tally_counts_work_sent_back_and_votes_against_their_panel() {
        let vote = |iteration, round, agent: &str, vote| RecordedVote {
            iteration,
            round,
            seat: 0,
            agent: agent.to_owned(),
            model: "m".to_owned(),
            weight: 1,
            vote,
            reasons: Vec::new(),
            at: 1.0,
        };
        let mut review = review("m", ReviewGate::Output);
        review.iteration = 2;
        review.sent_back = 1;
        review.status = ReviewStatus::Passed;
        review.votes = vec![
            vote(1, 1, "r1", Vote::Redo),
            vote(1, 1, "r2", Vote::Pass),
            vote(2, 1, "r1", Vote::Pass),
            vote(2, 1, "r2", Vote::Absent),
        ];
        let tallies = tally(&[review]);
        let author = &tallies["007"];
        assert_eq!((author.put_up, author.sent_back), (2, 1));
        let r1 = &tallies["r1"];
        assert_eq!((r1.votes, r1.disagreed, r1.absent), (2, 0, 0));
        let r2 = &tallies["r2"];
        assert_eq!(
            (r2.votes, r2.disagreed, r2.absent),
            (1, 1, 1),
            "it passed what was sent back, then gave no answer"
        );
    }

    backend_tests!(
        a_review_is_kept_per_gate_and_a_write_over_an_old_revision_loses,
        an_update_lands_over_whatever_was_written_meanwhile,
    );

    fn review(mission: &str, gate: ReviewGate) -> MissionReview {
        MissionReview::open(
            (mission, "G", gate),
            ("one", &ReviewPolicy::new(vec![PanelSeat::agent("r1")])),
            "007",
            ReviewMaterial {
                goals: "the goals".to_owned(),
                ..ReviewMaterial::default()
            },
            1.0,
        )
    }

    fn a_review_is_kept_per_gate_and_a_write_over_an_old_revision_loses(store: &dyn Store) {
        let registry = store.handle();
        assert!(
            registry
                .mission_review("m", ReviewGate::Output)
                .unwrap()
                .is_none()
        );
        assert!(registry.mission_reviews("m").unwrap().is_empty());

        let first = registry
            .put_mission_review(&review("m", ReviewGate::Output))
            .unwrap()
            .expect("nothing was there");
        assert_eq!(first.revision, 1);
        registry
            .put_mission_review(&review("m", ReviewGate::Plan))
            .unwrap()
            .expect("the other gate is its own record");
        assert_eq!(registry.mission_reviews("m").unwrap().len(), 2);
        assert!(registry.mission_reviews("other").unwrap().is_empty());

        assert!(
            registry
                .put_mission_review(&review("m", ReviewGate::Output))
                .unwrap()
                .is_none(),
            "a record read before it existed writes nothing"
        );
        let mut changed = first.clone();
        changed.status = ReviewStatus::Passed;
        let second = registry.put_mission_review(&changed).unwrap().unwrap();
        assert_eq!(second.revision, 2);
        assert!(
            registry.put_mission_review(&changed).unwrap().is_none(),
            "the same revision writes once"
        );
        let read = registry
            .mission_review("m", ReviewGate::Output)
            .unwrap()
            .unwrap();
        assert_eq!(read.status, ReviewStatus::Passed);
        assert_eq!(read.material.goals, "the goals");
    }

    fn an_update_lands_over_whatever_was_written_meanwhile(store: &dyn Store) {
        let registry = store.handle();
        registry
            .put_mission_review(&review("m", ReviewGate::Output))
            .unwrap();
        let written =
            update_mission_review(registry.as_ref(), ("m", ReviewGate::Output), 3, |review| {
                review.tokens += 5;
                true
            })
            .unwrap()
            .unwrap();
        assert_eq!(written.tokens, 5);
        assert_eq!(written.revision, 2);
        assert!(
            update_mission_review(registry.as_ref(), ("m", ReviewGate::Output), 3, |_| false)
                .unwrap()
                .is_none(),
            "nothing to change, nothing written"
        );
        let missing =
            update_mission_review(registry.as_ref(), ("none", ReviewGate::Output), 3, |_| true)
                .unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }
}
