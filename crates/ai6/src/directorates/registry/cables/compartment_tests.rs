use std::sync::Mutex;

use crate::directorates::m::compartments::{
    Compartment, CompartmentAction, CompartmentCover, CompartmentLimits, Compartments, CoverTarget,
    ReadIn, ReadInSubject,
};
use crate::directorates::m::roster::{Operation, Roster};
use crate::directorates::registry::cables::*;
use crate::directorates::registry::store::{CableStore, FileRegistry};
use crate::{Limit, Rate};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Compartments as a Station answers for them: `A`, `B` and `C`, read fresh at each question.
struct Answers {
    held: Mutex<Option<Vec<Compartment>>>,
    roster: Roster,
    clearance: Mutex<&'static str>,
}

impl CableCompartments for Answers {
    fn current(&self) -> Result<CompiledCompartments, String> {
        let held = self.held.lock().unwrap().clone();
        let Some(held) = held else {
            return Err("unreadable".to_owned());
        };
        Compartments::of(held)
            .and_then(|compartments| compartments.compile(&self.roster))
            .map_err(|reason| reason.to_string())
    }

    fn clearance(&self, _agent: &str, _operation: &str) -> Option<String> {
        Some((*self.clearance.lock().unwrap()).to_owned())
    }
}

use crate::directorates::m::compartments::CompiledCompartments;

fn answers(held: Vec<Compartment>) -> Arc<Answers> {
    Arc::new(Answers {
        held: Mutex::new(Some(held)),
        roster: Roster::builder()
            .operation(Operation::new("A"))
            .operation(Operation::new("B"))
            .operation(Operation::new("C"))
            .build()
            .unwrap(),
        clearance: Mutex::new("field"),
    })
}

fn kestrel() -> Compartment {
    Compartment::new("K")
        .with_actions([CompartmentAction::CableSend])
        .with_cover(CompartmentCover::new(CoverTarget::Operation(
            "B".to_owned(),
        )))
        .with_read_in(ReadIn::new(ReadInSubject::Operation("A".to_owned())))
}

fn principal(agent: &str, operation: &str, session: &str) -> CablePrincipal {
    CablePrincipal::try_new(
        agent.into(),
        operation.into(),
        None,
        "station-a".into(),
        Some(session.into()),
        Some(format!("{operation}-{session}")),
    )
    .unwrap()
}

fn endpoint(agent: &str, operation: &str, session: &str) -> CableEndpoint {
    CableEndpoint::try_new(
        agent.into(),
        operation.into(),
        None,
        "station-a".into(),
        Some(session.into()),
        Some(format!("{operation}-{session}")),
    )
    .unwrap()
}

fn station_grants() -> CablePolicy {
    let station = CableTarget::try_new(CableScope::Station, Some("station-a".into())).unwrap();
    CablePolicy::try_new(
        vec![
            CableGrant::try_new("*".into(), CableAction::Send, station.clone()).unwrap(),
            CableGrant::try_new("*".into(), CableAction::Receive, station).unwrap(),
        ],
        BTreeMap::new(),
    )
    .unwrap()
}

type Records = Arc<Mutex<Vec<CableAuditRecord>>>;

/// A relay with the sender `a1` of `A`, `b1` and `b2` of `B`, `c1` of `C`, and a log.
fn relay(
    registry: &Arc<FileRegistry>,
    policy: CablePolicy,
    answers: &Arc<Answers>,
) -> (CableRelay, Records) {
    let endpoints = vec![
        endpoint("a1", "A", "s-a1"),
        endpoint("b1", "B", "s-b1"),
        endpoint("b2", "B", "s-b2"),
        endpoint("c1", "C", "s-c1"),
    ];
    let records: Records = Arc::default();
    let log = records.clone();
    let relay = CableRelay::new(
        registry.clone(),
        policy,
        move || endpoints.clone(),
        BTreeMap::new(),
    )
    .with_clock(|| 1_700_000_000.0)
    .with_durable_audit(move |record| {
        log.lock().unwrap().push(record.clone());
        Ok(())
    });
    relay.set_compartments(answers.clone());
    (relay, records)
}

fn to_compartment(name: &str) -> CableTarget {
    CableTarget::try_new(CableScope::Compartment, Some(name.into())).unwrap()
}

fn to_agent(name: &str) -> CableTarget {
    CableTarget::try_new(CableScope::Agent, Some(name.into())).unwrap()
}

fn send(target: CableTarget, key: &str) -> CableSendRequest {
    CableSendRequest::new(target, "the release moved".into(), key.into())
}

fn recipients(cable: &crate::directorates::registry::cables::Cable) -> Vec<String> {
    cable
        .recipients()
        .iter()
        .map(|recipient| recipient.agent().to_owned())
        .collect()
}

#[test]
fn a_sender_read_in_reaches_what_the_compartment_covers_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let (relay, _) = relay(&registry, CablePolicy::default(), &answers(vec![kestrel()]));
    let cable = relay
        .send(
            &principal("a1", "A", "s-a1"),
            send(to_compartment("K"), "one"),
        )
        .unwrap();
    assert_eq!(recipients(&cable), ["b1", "b2"]);
    // Read in nowhere, in a compartment it does not hold, or in none that exists: nothing.
    assert!(
        relay
            .send(
                &principal("c1", "C", "s-c1"),
                send(to_compartment("K"), "two")
            )
            .is_err()
    );
    assert!(
        relay
            .send(
                &principal("a1", "A", "s-a1"),
                send(to_compartment("NONE"), "three")
            )
            .is_err()
    );
    let deliveries = relay.inbox(&principal("b1", "B", "s-b1"), 10).unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].from_operation, "A");
}

#[test]
fn a_compartment_cable_can_be_narrowed_to_one_operation_and_never_reaches_its_own_session() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let both = kestrel()
        .with_cover(CompartmentCover::new(CoverTarget::Operation(
            "C".to_owned(),
        )))
        .with_cover(CompartmentCover::new(CoverTarget::Operation(
            "A".to_owned(),
        )));
    let (relay, _) = relay(&registry, CablePolicy::default(), &answers(vec![both]));
    let sender = principal("a1", "A", "s-a1");
    let all = relay
        .send(&sender, send(to_compartment("K"), "all"))
        .unwrap();
    assert_eq!(recipients(&all), ["b1", "b2", "c1"]);
    let narrow = relay
        .send(
            &sender,
            send(to_compartment("K"), "narrow").with_operation("C"),
        )
        .unwrap();
    assert_eq!(recipients(&narrow), ["c1"]);
}

#[test]
fn a_station_granted_unicast_across_an_operation_needs_a_compartment_unless_weakened() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let (strict, _) = relay(&registry, station_grants(), &answers(Vec::new()));
    let sender = principal("a1", "A", "s-a1");
    assert_eq!(
        strict
            .send(&sender, send(to_agent("b1"), "refused"))
            .unwrap_err(),
        CableRelayError::Unavailable
    );
    // The same Station grant inside the Operation is untouched.
    let (inside, _) = relay(&registry, station_grants(), &answers(Vec::new()));
    let own = CableEndpoint::try_new(
        "a2".into(),
        "A".into(),
        None,
        "station-a".into(),
        Some("s-a2".into()),
        Some("A-s-a2".into()),
    )
    .unwrap();
    inside.register_endpoint(own).unwrap();
    assert!(inside.send(&sender, send(to_agent("a2"), "inside")).is_ok());

    let (read_in, log) = relay(&registry, station_grants(), &answers(vec![kestrel()]));
    let crossed = read_in
        .send(&sender, send(to_agent("b1"), "crossed"))
        .unwrap();
    assert_eq!(recipients(&crossed), ["b1"]);
    let written = log.lock().unwrap().clone();
    assert_eq!(written.len(), 1);
    let crossing = written[0].crossing.as_ref().unwrap();
    assert_eq!(crossing.operation, "B");
    assert_eq!(crossing.compartment.as_deref(), Some("K"));

    let (weak, log) = relay(&registry, station_grants(), &answers(Vec::new()));
    let weak = weak.with_cross_operation(CrossOperation::StationGrants);
    let delivered = weak.send(&sender, send(to_agent("c1"), "weak")).unwrap();
    assert_eq!(recipients(&delivered), ["c1"]);
    let written = log.lock().unwrap().clone();
    assert_eq!(written[0].crossing.as_ref().unwrap().compartment, None);
    assert_eq!(
        CableConfig::parse_toml("cross_operation = \"station_grants\"\n")
            .unwrap()
            .weakenings(),
        ["weakening: cables.cross_operation = \"station_grants\""]
    );
    assert!(CableConfig::default().weakenings().is_empty());
}

#[test]
fn a_hold_inbound_mode_holds_a_compartment_cable_and_release_asks_the_grant_again() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let target = to_compartment("K");
    let policy = CablePolicy::try_new(
        vec![CableGrant::try_new("c1".into(), CableAction::Release, target.clone()).unwrap()],
        BTreeMap::from([("b1".to_owned(), CableInboundMode::Hold)]),
    )
    .unwrap();
    let held = answers(vec![kestrel()]);
    let (relay, _) = relay(&registry, policy, &held);
    let cable = relay
        .send(&principal("a1", "A", "s-a1"), send(target, "held"))
        .unwrap();
    let state_of = |cable: &crate::directorates::registry::cables::Cable, agent: &str| {
        cable
            .recipients()
            .iter()
            .find(|recipient| recipient.agent() == agent)
            .unwrap()
            .state()
    };
    assert_eq!(state_of(&cable, "b1"), CableState::Held);
    assert_eq!(state_of(&cable, "b2"), CableState::Queued);
    assert!(
        relay
            .inbox(&principal("b1", "B", "s-b1"), 10)
            .unwrap()
            .is_empty()
    );
    // Read out before anyone releases it: the release refuses the recipient.
    *held.held.lock().unwrap() = Some(Vec::new());
    let released = relay
        .release(&principal("c1", "C", "s-c1"), cable.id())
        .unwrap();
    assert_eq!((released.delivered, released.refused), (0, 1));

    // What cannot be read leaves it held.
    *held.held.lock().unwrap() = Some(vec![kestrel()]);
    let waits = relay
        .send(
            &principal("a1", "A", "s-a1"),
            send(to_compartment("K"), "held-waits"),
        )
        .unwrap();
    *held.held.lock().unwrap() = None;
    let released = relay
        .release(&principal("c1", "C", "s-c1"), waits.id())
        .unwrap();
    assert_eq!((released.delivered, released.refused), (0, 0));

    // Held and read in: it is delivered.
    *held.held.lock().unwrap() = Some(vec![kestrel()]);
    let again = relay
        .send(
            &principal("a1", "A", "s-a1"),
            send(to_compartment("K"), "held-again"),
        )
        .unwrap();
    let released = relay
        .release(&principal("c1", "C", "s-c1"), again.id())
        .unwrap();
    assert_eq!((released.delivered, released.refused), (1, 0));
}

#[test]
fn a_cable_released_across_an_operation_is_in_the_log_and_a_failing_log_keeps_it_held() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let target = to_compartment("K");
    let policy = CablePolicy::try_new(
        vec![CableGrant::try_new("c1".into(), CableAction::Release, target.clone()).unwrap()],
        BTreeMap::from([("b1".to_owned(), CableInboundMode::Hold)]),
    )
    .unwrap();
    let endpoints = vec![endpoint("b1", "B", "s-b1")];
    let log_fails = Arc::new(Mutex::new(true));
    let (fails, records): (_, Records) = (log_fails.clone(), Arc::default());
    let written = records.clone();
    let relay = CableRelay::new(
        registry.clone(),
        policy,
        move || endpoints.clone(),
        BTreeMap::new(),
    )
    .with_clock(|| 1_700_000_000.0)
    .with_durable_audit(move |record| {
        if record.event == "cable_read" && *fails.lock().unwrap() {
            return Err("the log is full".to_owned());
        }
        written.lock().unwrap().push(record.clone());
        Ok(())
    });
    relay.set_compartments(answers(vec![kestrel()]));
    let cable = relay
        .send(&principal("a1", "A", "s-a1"), send(target, "held"))
        .unwrap();
    let approver = principal("c1", "C", "s-c1");
    let released = relay.release(&approver, cable.id()).unwrap();
    assert_eq!((released.delivered, released.refused), (0, 0));
    *log_fails.lock().unwrap() = false;
    let released = relay.release(&approver, cable.id()).unwrap();
    assert_eq!((released.delivered, released.refused), (1, 0));
    assert!(
        records
            .lock()
            .unwrap()
            .iter()
            .any(|record| record.event == "cable_read")
    );
}

#[test]
fn a_recipient_with_no_session_is_not_in_a_compartment_s_audience() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let endpoints = vec![
        CableEndpoint::try_new(
            "b9".into(),
            "B".into(),
            None,
            "station-a".into(),
            None,
            None,
        )
        .unwrap(),
    ];
    let relay = CableRelay::new(
        registry.clone(),
        CablePolicy::default(),
        move || endpoints.clone(),
        BTreeMap::new(),
    )
    .with_clock(|| 1_700_000_000.0)
    .with_durable_audit(|_| Ok(()));
    relay.set_compartments(answers(vec![kestrel()]));
    assert!(
        relay
            .send(
                &principal("a1", "A", "s-a1"),
                send(to_compartment("K"), "one")
            )
            .is_err()
    );
}

#[test]
fn a_compartment_s_bucket_is_shared_by_two_relays_over_one_registry() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let limits = CompartmentLimits {
        rate: Rate::at(0.001),
        burst: Limit::at(2),
        ..CompartmentLimits::default()
    };
    let held = answers(vec![kestrel().with_limits(limits)]);
    let (first, _) = relay(&registry, CablePolicy::default(), &held);
    let (second, _) = relay(&registry, CablePolicy::default(), &held);
    let sender = principal("a1", "A", "s-a1");
    assert!(
        first
            .send(&sender, send(to_compartment("K"), "one"))
            .is_ok()
    );
    assert!(
        second
            .send(&sender, send(to_compartment("K"), "two"))
            .is_ok()
    );
    assert!(
        first
            .send(&sender, send(to_compartment("K"), "three"))
            .is_err()
    );
    assert!(
        second
            .send(&sender, send(to_compartment("K"), "four"))
            .is_err()
    );
    assert_eq!(registry.find_cable("a1", "three").unwrap(), None);
}

#[test]
fn a_per_sender_bucket_counts_one_sender_alone() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let limits = CompartmentLimits {
        sender_rate: Rate::at(0.001),
        sender_burst: Limit::at(1),
        ..CompartmentLimits::default()
    };
    let wide = kestrel()
        .with_limits(limits)
        .with_read_in(ReadIn::new(ReadInSubject::Operation("C".to_owned())));
    let (relay, _) = relay(&registry, CablePolicy::default(), &answers(vec![wide]));
    let (a1, c1) = (principal("a1", "A", "s-a1"), principal("c1", "C", "s-c1"));
    assert!(relay.send(&a1, send(to_compartment("K"), "one")).is_ok());
    assert!(relay.send(&a1, send(to_compartment("K"), "two")).is_err());
    assert!(relay.send(&c1, send(to_compartment("K"), "three")).is_ok());
}

#[test]
fn max_fanout_refuses_a_third_recipient() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let limits = CompartmentLimits {
        max_fanout: Limit::at(2),
        ..CompartmentLimits::default()
    };
    let wide = kestrel()
        .with_limits(limits)
        .with_cover(CompartmentCover::new(CoverTarget::Operation(
            "C".to_owned(),
        )));
    let (relay, _) = relay(&registry, CablePolicy::default(), &answers(vec![wide]));
    let sender = principal("a1", "A", "s-a1");
    assert_eq!(
        relay
            .send(&sender, send(to_compartment("K"), "wide"))
            .unwrap_err(),
        CableRelayError::Unavailable
    );
    assert!(
        relay
            .send(
                &sender,
                send(to_compartment("K"), "narrow").with_operation("B")
            )
            .is_ok()
    );
}

#[test]
fn discovery_shows_who_a_compartment_reaches_and_never_where_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let (relay, _) = relay(&registry, station_grants(), &answers(vec![kestrel()]));
    let sender = principal("a1", "A", "s-a1");
    let peers = relay.discover_through_compartments(&sender).unwrap();
    assert_eq!(peers.len(), 2);
    assert_eq!(peers[0].agent, "b1");
    assert_eq!(peers[0].operation, "B");
    assert_eq!(peers[0].mission_id.as_deref(), Some("B-s-b1"));
    assert_eq!(peers[0].compartment, "K");
    let text = serde_json::to_string(&peers).unwrap();
    assert!(!text.contains("station-a"), "{text}");
    assert!(!text.contains("\"s-b1\""), "{text}");
    assert!(
        relay
            .discover_through_compartments(&principal("c1", "C", "s-c1"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_discover_grant_shows_no_other_operation_without_a_compartment() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let station = CableTarget::try_new(CableScope::Station, Some("station-a".into())).unwrap();
    let policy = CablePolicy::try_new(
        vec![
            CableGrant::try_new("*".into(), CableAction::Discover, station.clone()).unwrap(),
            CableGrant::try_new("*".into(), CableAction::Receive, station).unwrap(),
        ],
        BTreeMap::new(),
    )
    .unwrap();
    let (relay, _) = relay(&registry, policy, &answers(Vec::new()));
    let peers = relay.discover(&principal("a1", "A", "s-a1")).unwrap();
    assert!(peers.is_empty(), "{peers:?}");
    let weak = relay.with_cross_operation(CrossOperation::StationGrants);
    assert_eq!(
        weak.discover(&principal("a1", "A", "s-a1")).unwrap().len(),
        3
    );
}

#[test]
fn a_failing_audit_write_refuses_the_send_and_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let endpoints = vec![endpoint("b1", "B", "s-b1")];
    let relay = CableRelay::new(
        registry.clone(),
        CablePolicy::default(),
        move || endpoints.clone(),
        BTreeMap::new(),
    )
    .with_clock(|| 1_700_000_000.0)
    .with_durable_audit(|_| Err("the log is full".to_owned()));
    relay.set_compartments(answers(vec![kestrel()]));
    let sender = principal("a1", "A", "s-a1");
    assert_eq!(
        relay
            .send(&sender, send(to_compartment("K"), "lost"))
            .unwrap_err(),
        CableRelayError::Unavailable
    );
    assert_eq!(registry.find_cable("a1", "lost").unwrap(), None);
}

#[test]
fn no_log_at_all_refuses_a_crossing() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let endpoints = vec![endpoint("b1", "B", "s-b1")];
    let relay = CableRelay::new(
        registry.clone(),
        CablePolicy::default(),
        move || endpoints.clone(),
        BTreeMap::new(),
    )
    .with_clock(|| 1_700_000_000.0);
    relay.set_compartments(answers(vec![kestrel()]));
    assert!(
        relay
            .send(
                &principal("a1", "A", "s-a1"),
                send(to_compartment("K"), "lost")
            )
            .is_err()
    );
}

#[test]
fn a_grant_scope_of_compartment_takes_only_release() {
    for (action, valid) in [
        ("send", false),
        ("receive", false),
        ("discover", false),
        ("release", true),
    ] {
        let parsed = CableConfig::parse_toml(&format!(
            "[[grants]]\nagent = \"*\"\naction = \"{action}\"\nscope = \"compartment\"\nname = \"K\"\n"
        ));
        assert_eq!(parsed.is_ok(), valid, "{action}");
    }
}

#[test]
fn reading_a_crossing_cable_is_audited_and_its_grant_asked_again() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let held = answers(vec![kestrel()]);
    let (relay, log) = relay(&registry, CablePolicy::default(), &held);
    let sender = principal("a1", "A", "s-a1");
    relay
        .send(&sender, send(to_compartment("K"), "one"))
        .unwrap();
    let b1 = principal("b1", "B", "s-b1");
    assert_eq!(relay.inbox(&b1, 10).unwrap().len(), 1);
    let read = log
        .lock()
        .unwrap()
        .iter()
        .find(|record| record.event == "cable_read")
        .cloned()
        .unwrap();
    assert_eq!(read.operation, "B");
    assert_eq!(
        read.crossing.unwrap().operation,
        "A",
        "the source Operation"
    );
    // A delivered Cable is not shown again once the grant is gone.
    *held.held.lock().unwrap() = Some(Vec::new());
    assert!(relay.inbox(&b1, 10).unwrap().is_empty());
    // A Cable that waits for a read out is refused when it would be delivered.
    *held.held.lock().unwrap() = Some(vec![kestrel()]);
    relay
        .send(&sender, send(to_compartment("K"), "two"))
        .unwrap();
    *held.held.lock().unwrap() = Some(Vec::new());
    let b2 = principal("b2", "B", "s-b2");
    assert!(relay.inbox(&b2, 10).unwrap().is_empty());
    let stored = registry.find_cable("a1", "two").unwrap().unwrap();
    let state = stored
        .recipients()
        .iter()
        .find(|recipient| recipient.agent() == "b2")
        .unwrap()
        .state();
    assert_eq!(state, CableState::Refused);
    // What cannot be read leaves a waiting Cable where it is.
    *held.held.lock().unwrap() = Some(vec![kestrel()]);
    relay
        .send(&sender, send(to_compartment("K"), "three"))
        .unwrap();
    *held.held.lock().unwrap() = None;
    assert!(relay.inbox(&b2, 10).unwrap().is_empty());
    let stored = registry.find_cable("a1", "three").unwrap().unwrap();
    let state = stored
        .recipients()
        .iter()
        .find(|recipient| recipient.agent() == "b2")
        .unwrap()
        .state();
    assert_eq!(state, CableState::Queued);
}

#[test]
fn a_reply_stays_on_the_grant_that_opened_the_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let held = answers(vec![kestrel()]);
    let (relay, _) = relay(&registry, CablePolicy::default(), &held);
    let (a1, b1) = (principal("a1", "A", "s-a1"), principal("b1", "B", "s-b1"));
    let opened = relay
        .send(&a1, send(to_compartment("K"), "opened"))
        .unwrap();
    assert_eq!(relay.inbox(&b1, 10).unwrap().len(), 1);
    // b1 holds no cable.send on A, but a1's grant opened this: the reply goes back to a1 alone.
    let reply = relay
        .reply(&b1, opened.id(), "answered".into(), "reply-1".into())
        .unwrap();
    assert_eq!(recipients(&reply), ["a1"]);
    let answers_read = relay.inbox(&a1, 10).unwrap();
    assert_eq!(answers_read.len(), 1);
    assert_eq!(answers_read[0].body, "answered");
    // Unprompted, b1 reaches nobody through K.
    assert!(
        relay
            .send(&b1, send(to_compartment("K"), "unprompted"))
            .is_err()
    );
    // Read out, the reply is refused as well.
    *held.held.lock().unwrap() = Some(Vec::new());
    assert!(
        relay
            .reply(&b1, opened.id(), "too late".into(), "reply-2".into())
            .is_err()
    );
}

#[test]
fn a_person_s_assistant_has_no_compartment_audience() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let (relay, _) = relay(&registry, CablePolicy::default(), &answers(vec![kestrel()]));
    let assistant = principal("moneypenny", "A", "s-m")
        .with_on_behalf_of("m".to_owned())
        .unwrap();
    assert_eq!(
        relay
            .send(&assistant, send(to_compartment("K"), "broadcast"))
            .unwrap_err(),
        CableRelayError::Unavailable
    );
}

#[test]
fn what_cannot_be_read_grants_no_crossing() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let held = answers(vec![kestrel()]);
    *held.held.lock().unwrap() = None;
    let (relay, _) = relay(&registry, station_grants(), &held);
    let sender = principal("a1", "A", "s-a1");
    assert!(
        relay
            .send(&sender, send(to_compartment("K"), "one"))
            .is_err()
    );
    assert!(relay.send(&sender, send(to_agent("b1"), "two")).is_err());
    assert_eq!(registry.find_cable("a1", "two").unwrap(), None);
}

#[test]
fn a_clearance_floor_stops_a_send_from_a_session_below_it() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(FileRegistry::open(dir.path()));
    let held = answers(vec![kestrel().with_min_clearance("00")]);
    let (relay, _) = relay(&registry, CablePolicy::default(), &held);
    let sender = principal("a1", "A", "s-a1");
    assert!(
        relay
            .send(&sender, send(to_compartment("K"), "one"))
            .is_err()
    );
    *held.clearance.lock().unwrap() = "00";
    assert!(
        relay
            .send(&sender, send(to_compartment("K"), "two"))
            .is_ok()
    );
}
