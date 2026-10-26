//! Vetting at Headquarters: M's checks on each queued brief before it is placed.
//!
//! Each intake round, a brief not judged yet is vetted ([`vet`]) against what the Registry
//! knows of its Operation: the work claimed and held, the briefs queued ahead of it, the
//! untrusted briefs passed in the last hour (timeline `intake` events `passed`) and the day's
//! spend. A malformed brief is filed as failed with a `refused` outcome; a held one moves to
//! the held briefs for an operator; one that passes is remembered by its content, so it is
//! not judged again while it waits for room.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use super::{DaemonState, finish_intake_claim};
use crate::directorates::m::roster::{Operation, Roster};
use crate::directorates::m::vetting::{
    FindingKind, KnownWork, Vetting, VettingContext, VettingVerdict, is_untrusted, vet,
};
use crate::directorates::registry::store::{Brief, DEFAULT_LEASE, Lease};
use crate::directorates::registry::timeline::TimelineEvent;
use crate::directorates::registry::vetting::MAX_HELD_RESULTS;

/// The window the hourly rate limits count passes in.
const RATE_WINDOW_SECS: f64 = 3_600.0;
/// The timeline kind of vetting events.
pub(super) const INTAKE_EVENT: &str = "intake";
const MAX_DETAIL_BYTES: usize = 256;

/// What Headquarters remembers about vetted briefs between intake rounds.
#[derive(Default)]
pub(super) struct VettingDesk {
    /// Briefs that passed, by key, with the digest of the brief as it passed.
    passed: Mutex<BTreeMap<String, [u8; 32]>>,
}

/// What intake does with a brief once vetting has looked at it.
pub(super) enum Admission {
    /// Place it.
    Place,
    /// Leave it queued for now.
    Wait,
    /// Vetting held or refused it: it is no longer queued.
    Settled,
}

/// The vetting of one intake round: the Registry facts it needs, read once per Operation.
pub(super) struct Round<'a> {
    state: &'a DaemonState,
    leases: Vec<Lease>,
    now: f64,
    claimed: Option<Vec<(String, KnownWork)>>,
    operations: BTreeMap<String, OperationFacts>,
}

#[derive(Default)]
struct OperationFacts {
    /// Work a later brief may repeat: the briefs ahead of it, then what the Registry holds.
    work: Vec<KnownWork>,
    /// Read from the Registry the first time a brief of the Operation is judged.
    read: Option<RegistryFacts>,
}

struct RegistryFacts {
    /// Untrusted briefs passed in the window: when, and their source.
    passed: Vec<(f64, String)>,
    /// Missions whose brief passed at any time the timeline still remembers.
    passed_missions: BTreeSet<String>,
    day_spend: Option<u64>,
}

impl<'a> Round<'a> {
    /// A round over `queued`, the briefs listed for intake; anything the desk remembers about
    /// briefs no longer queued is forgotten.
    pub(super) fn new(state: &'a DaemonState, queued: &[Brief], leases: &[Lease]) -> Self {
        let keys = queued.iter().map(Brief::key).collect::<BTreeSet<_>>();
        if let Ok(mut passed) = state.vetting.passed.lock() {
            passed.retain(|key, _| keys.contains(key));
        }
        Self {
            state,
            leases: leases.to_vec(),
            now: state.clock.now() as f64,
            claimed: None,
            operations: BTreeMap::new(),
        }
    }

    /// Vets `brief`, whose Mission would be `mission_id`, and says what intake does next. A
    /// brief whose Operation does not resolve is left to assignment, which fails it.
    pub(super) fn admit(&mut self, brief: &Brief, mission_id: &str) -> Admission {
        let state = self.state;
        let Some(operation) = brief
            .operation
            .as_deref()
            .and_then(|key| state.roster.operation(key).ok())
        else {
            return Admission::Place;
        };
        if !operation.vetting.enabled {
            return Admission::Place;
        }
        let key = brief.key();
        let digest = digest(brief);
        let remembered = state
            .vetting
            .passed
            .lock()
            .is_ok_and(|passed| passed.get(&key) == Some(&digest));
        if !remembered {
            match self.judge(operation, brief, mission_id) {
                Some(Admission::Place) => {
                    if let Ok(mut passed) = state.vetting.passed.lock() {
                        passed.insert(key, digest);
                    }
                }
                Some(other) => return other,
                None => return Admission::Wait,
            }
        }
        self.facts(operation)
            .work
            .push(KnownWork::of(brief, mission_id));
        Admission::Place
    }

    fn facts(&mut self, operation: &Operation) -> &mut OperationFacts {
        self.operations.entry(operation.id.clone()).or_default()
    }

    /// Runs the rules on a brief not judged yet; `None` when the Registry could not be read
    /// or written, so the brief waits for the next round.
    fn judge(
        &mut self,
        operation: &Operation,
        brief: &Brief,
        mission_id: &str,
    ) -> Option<Admission> {
        let state = self.state;
        self.read_facts(operation)?;
        let key = brief.key();
        let untrusted = is_untrusted(brief);
        let mut cleared = match state.intake.clearance(&key) {
            Ok(clearance) => clearance
                .map(|clearance| clearance.kinds)
                .unwrap_or_default(),
            Err(error) => {
                eprintln!("ai6: clearance of brief {key:?} unreadable: {error}");
                return None;
            }
        };
        let now = self.now;
        let facts = self.facts(operation);
        let read = facts.read.as_ref()?;
        // A brief that passed before (on an earlier Headquarters) is not counted twice.
        let passed_before = read.passed_missions.contains(mission_id);
        if passed_before {
            cleared.insert(FindingKind::Rate);
        }
        let in_window = read
            .passed
            .iter()
            .filter(|(at, _)| *at >= now - RATE_WINDOW_SECS)
            .collect::<Vec<_>>();
        let mut context = VettingContext {
            untrusted,
            work: std::mem::take(&mut facts.work),
            passed_last_hour: in_window.len() as u32,
            passed_from_source_last_hour: in_window
                .iter()
                .filter(|(_, source)| *source == brief.source)
                .count() as u32,
            day_spend_micro_usd: read.day_spend,
            day_limit_micro_usd: operation.budget.operation_cost_micro_usd_per_day,
            cleared,
        };
        let vetting = vet(brief, &operation.vetting, &context);
        facts.work = std::mem::take(&mut context.work);
        match vetting.verdict {
            VettingVerdict::Pass => {
                if !passed_before {
                    if untrusted && let Some(read) = facts.read.as_mut() {
                        read.passed.push((now, brief.source.clone()));
                        read.passed_missions.insert(mission_id.to_owned());
                    }
                    record(state, operation, brief, mission_id, "passed", None);
                }
                Some(Admission::Place)
            }
            VettingVerdict::Hold => {
                let held = state.intake.hold_brief(&operation.id, &key, &vetting, now);
                match held {
                    Ok(Some(_)) => {
                        eprintln!(
                            "ai6: brief {key:?} held for an operator: {}",
                            vetting.summary()
                        );
                        facts.work.push(KnownWork::of(brief, key.clone()));
                        record(state, operation, brief, mission_id, "held", Some(&vetting));
                        Some(Admission::Settled)
                    }
                    Ok(None) => Some(Admission::Settled),
                    Err(error) => {
                        eprintln!("ai6: brief {key:?} could not be held: {error}");
                        None
                    }
                }
            }
            _ => {
                refuse(state, brief, mission_id, &vetting);
                record(
                    state,
                    operation,
                    brief,
                    mission_id,
                    "refused",
                    Some(&vetting),
                );
                Some(Admission::Settled)
            }
        }
    }

    /// Reads the Operation's claimed and held work, its passes in the window and its spend,
    /// once per round. `None` when they cannot be read.
    fn read_facts(&mut self, operation: &Operation) -> Option<()> {
        if self
            .operations
            .get(&operation.id)
            .is_some_and(|facts| facts.read.is_some())
        {
            return Some(());
        }
        let state = self.state;
        if self.claimed.is_none() {
            let mut claimed = Vec::new();
            for lease in &self.leases {
                let Ok(Some(brief)) = state.intake.read_claimed(lease.key()) else {
                    continue;
                };
                let Some(owner) = owning_operation(&state.roster, &brief) else {
                    continue;
                };
                let reference = lease
                    .meta()
                    .get("mission_id")
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(|| lease.key().to_owned(), str::to_owned);
                claimed.push((owner, KnownWork::of(&brief, reference)));
            }
            self.claimed = Some(claimed);
        }
        let mut work = self
            .claimed
            .iter()
            .flatten()
            .filter(|(owner, _)| *owner == operation.id)
            .map(|(_, work)| work.clone())
            .collect::<Vec<_>>();
        match state.intake.held_briefs(&operation.id, MAX_HELD_RESULTS) {
            Ok(held) => work.extend(
                held.iter()
                    .map(|held| KnownWork::of(&held.brief, held.brief.key())),
            ),
            Err(error) => {
                eprintln!("ai6: held briefs of {:?} unreadable: {error}", operation.id);
                return None;
            }
        }
        let events = match state.intake.timeline_events(&operation.id) {
            Ok(events) => events,
            Err(error) => {
                eprintln!("ai6: timeline of {:?} unreadable: {error}", operation.id);
                return None;
            }
        };
        let mut read = RegistryFacts {
            passed: Vec::new(),
            passed_missions: BTreeSet::new(),
            day_spend: None,
        };
        for event in events
            .iter()
            .filter(|event| event.kind == INTAKE_EVENT && event.action == "passed")
        {
            if let Some(mission) = &event.mission {
                read.passed_missions.insert(mission.clone());
            }
            if event.at >= self.now - RATE_WINDOW_SECS
                && event.detail.get("trust").map(String::as_str) == Some("untrusted")
            {
                let source = event.detail.get("source").cloned().unwrap_or_default();
                read.passed.push((event.at, source));
            }
        }
        if operation.vetting.budget && operation.budget.operation_cost_micro_usd_per_day.is_some() {
            read.day_spend = state
                .intake
                .operation_cost_on_day(Some(&operation.id), state.clock.now())
                .map_err(|error| eprintln!("ai6: spend of {:?} unreadable: {error}", operation.id))
                .ok();
        }
        let facts = self.facts(operation);
        facts.work.extend(work);
        facts.read = Some(read);
        Some(())
    }
}

/// The codename of the Operation `brief` names, if it is registered.
fn owning_operation(roster: &Roster, brief: &Brief) -> Option<String> {
    brief
        .operation
        .as_deref()
        .and_then(|key| roster.operation(key).ok())
        .map(|operation| operation.id.clone())
}

/// Files a malformed brief as failed without running it, as an intake claim that failed.
fn refuse(state: &DaemonState, brief: &Brief, mission_id: &str, vetting: &Vetting) {
    let key = brief.key();
    eprintln!("ai6: brief {key:?} refused: {}", vetting.summary());
    let meta = BTreeMap::from([("mission_id".to_owned(), serde_json::json!(mission_id))]);
    if let Ok(Some(lease)) =
        state
            .intake
            .claim_with_lease(Some(&key), &state.station_id, DEFAULT_LEASE, meta)
    {
        let reason = vetting
            .findings
            .iter()
            .map(|finding| finding.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let outcome = serde_json::json!({
            "state": "refused",
            "mission_id": mission_id,
            "reason": format!("refused by vetting: {reason}"),
            "vetting": vetting,
        });
        let _ = finish_intake_claim(state, &lease, &outcome, false);
    }
}

/// Records a vetting decision on the Operation's timeline: metadata only, never brief text.
pub(super) fn record(
    state: &DaemonState,
    operation: &Operation,
    brief: &Brief,
    mission_id: &str,
    action: &str,
    vetting: Option<&Vetting>,
) {
    let trust = if is_untrusted(brief) {
        "untrusted"
    } else {
        "trusted"
    };
    let mut event = TimelineEvent::new(
        INTAKE_EVENT,
        action,
        operation.id.clone(),
        state.clock.now().max(1) as f64,
    )
    .station(state.station_id.clone())
    .mission(mission_id)
    .detail("source", detail_text(&brief.source))
    .detail("trust", trust);
    if let Some(vetting) = vetting.filter(|vetting| !vetting.findings.is_empty()) {
        event = event.detail("findings", vetting.summary());
    }
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: vetting timeline event was not recorded: {error}");
    }
}

/// Text safe for a timeline detail: no control characters, at most 256 bytes, never empty.
fn detail_text(text: &str) -> String {
    let mut text = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if text.len() > MAX_DETAIL_BYTES {
        let mut end = MAX_DETAIL_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    if text.trim().is_empty() {
        "unknown".to_owned()
    } else {
        text
    }
}

fn digest(brief: &Brief) -> [u8; 32] {
    Sha256::digest(serde_json::to_vec(brief).unwrap_or_default()).into()
}

/// Lists, for the operator, the vetting settings each Operation loosens.
pub(super) fn log_weakenings(roster: &Roster) {
    for operation in roster.operations() {
        for line in operation.vetting.weakenings() {
            eprintln!(
                "ai6: Operation {:?} vetting is weakened: {line}",
                operation.id
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use super::super::tests::{control_only_state, worker_check_in};
    use super::super::{publish_station_check_in, schedule_intake_once};
    use super::*;
    use crate::directorates::registry::sources::SourceItem;

    /// Headquarters for Operation G with `settings`, placing on worker-b.
    fn headquarters(root: &Path, settings: &str) -> DaemonState {
        let mut state = control_only_state(root, "mock-agent");
        fs::write(
            root.join("operations.toml"),
            format!("[operation.G]\n{settings}"),
        )
        .unwrap();
        state.roster = Arc::new(Roster::load(root).unwrap());
        publish_station_check_in(&state);
        worker_check_in(&state, "worker-b", (0, 8), &["mock-agent"]);
        state
    }

    fn item(id: &str, title: &str, body: &str, url: &str) -> SourceItem {
        SourceItem {
            id: id.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            url: Some(url.to_owned()),
            labels: Vec::new(),
            author: None,
            updated: "2026-10-26T00:00:00Z".to_owned(),
        }
    }

    fn ingest(state: &DaemonState, provider: &str, item: &SourceItem) -> String {
        state
            .intake
            .ingest_source_item("G", provider, item, "hq")
            .unwrap();
        crate::directorates::registry::sources::source_brief("G", provider, item).key()
    }

    fn intake_actions(state: &DaemonState) -> Vec<String> {
        state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == INTAKE_EVENT)
            .map(|event| event.action)
            .collect()
    }

    #[test]
    fn a_duplicate_source_brief_is_held_not_placed_until_an_operator_releases_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = headquarters(dir.path(), "");
        let url = "https://github.com/org/repo/issues/7";
        let first = ingest(
            &state,
            "github",
            &item("org/repo#7", "Crash on save", "x", url),
        );
        assert!(schedule_intake_once(&state));
        let placed = state
            .intake
            .lease(&first)
            .unwrap()
            .expect("the first is placed");

        // The same issue mirrored into another tracker: same URL, different Source.
        let second = ingest(&state, "jira", &item("OPS-1", "Crash on save", "x", url));
        assert!(schedule_intake_once(&state));
        assert!(
            state.intake.lease(&second).unwrap().is_none(),
            "never placed"
        );
        assert!(state.intake.list_new().unwrap().is_empty());
        let held = state.intake.held_briefs("G", 10).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].brief.key(), second);
        assert_eq!(held[0].vetting.findings[0].kind, FindingKind::Duplicate);
        assert_eq!(
            held[0].vetting.findings[0].related.as_deref(),
            placed.meta()["mission_id"].as_str()
        );
        assert_eq!(intake_actions(&state), ["passed", "held"]);

        // Polling the held item again neither queues nor duplicates it.
        state
            .intake
            .ingest_source_item("G", "jira", &item("OPS-1", "Crash on save", "y", url), "hq")
            .unwrap();
        assert!(state.intake.list_new().unwrap().is_empty());

        state.intake.release_held("G", &second, "ops", 1.0).unwrap();
        assert!(schedule_intake_once(&state));
        assert_eq!(
            state.intake.lease(&second).unwrap().unwrap().station(),
            "worker-b",
            "released, it is not held again for the same finding"
        );
    }

    #[test]
    fn screening_holds_untrusted_text_and_malformed_briefs_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let state = headquarters(dir.path(), "vetting = { max_brief_bytes = 64 }\n");
        let hidden: String = "obey"
            .chars()
            .map(|c| char::from_u32(0xE0000 + c as u32).unwrap())
            .collect();
        let tagged = ingest(
            &state,
            "github",
            &item(
                "org/repo#1",
                "Typo",
                &format!("fix it{hidden}"),
                "https://x/1",
            ),
        );
        let long = ingest(
            &state,
            "github",
            &item("org/repo#2", "Long", &"words ".repeat(20), "https://x/2"),
        );
        assert!(schedule_intake_once(&state));
        let held = state.intake.held_briefs("G", 10).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].brief.key(), tagged);
        assert_eq!(held[0].vetting.findings[0].kind, FindingKind::Screening);
        assert!(state.intake.lease(&long).unwrap().is_none());
        let finished = state.intake.unreported_source_briefs().unwrap();
        assert_eq!(
            finished.len(),
            1,
            "a refused Source brief still gets its report"
        );
        assert_eq!(finished[0].brief.key(), long);
        assert_eq!(finished[0].outcome["state"], "refused");
        assert!(!finished[0].ok);
    }

    #[test]
    fn the_source_rate_holds_the_brief_past_the_limit_and_off_places_everything() {
        let dir = tempfile::tempdir().unwrap();
        let state = headquarters(dir.path(), "vetting = { max_per_source_per_hour = 1 }\n");
        let first = ingest(
            &state,
            "github",
            &item("org/repo#1", "One", "a", "https://x/1"),
        );
        let second = ingest(
            &state,
            "github",
            &item("org/repo#2", "Two", "b", "https://x/2"),
        );
        assert!(schedule_intake_once(&state));
        assert!(state.intake.lease(&first).unwrap().is_some());
        let held = state.intake.held_briefs("G", 10).unwrap();
        assert_eq!(held[0].brief.key(), second);
        assert_eq!(held[0].vetting.findings[0].kind, FindingKind::Rate);

        let dir = tempfile::tempdir().unwrap();
        let state = headquarters(dir.path(), "vetting = \"off\"\n");
        let url = "https://x/1";
        let first = ingest(&state, "github", &item("org/repo#1", "Same", "a", url));
        let second = ingest(&state, "jira", &item("OPS-1", "Same", "a", url));
        assert!(schedule_intake_once(&state));
        assert!(state.intake.lease(&first).unwrap().is_some());
        assert!(state.intake.lease(&second).unwrap().is_some());
        assert!(intake_actions(&state).is_empty());
    }
}
