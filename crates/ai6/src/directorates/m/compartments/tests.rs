use serde_json::json;

use super::*;
use crate::directorates::m::config::{FileProvider, MemoryProvider, Settings};
use crate::directorates::m::roster::Operation;

/// `A`, and the tree `P` over `C` over `D`, and `Q`.
fn roster() -> Roster {
    roster_with(RosterSettings::default())
}

fn roster_with(settings: RosterSettings) -> Roster {
    Roster::builder()
        .settings(settings)
        .operation(Operation::new("A"))
        .operation(Operation::new("P"))
        .operation(Operation::new("C").with_parent("P"))
        .operation(Operation::new("D").with_parent("C"))
        .operation(Operation::new("Q"))
        .build()
        .unwrap()
}

fn operation(id: &str) -> CompartmentCover {
    CompartmentCover::new(CoverTarget::Operation(id.to_owned()))
}

fn subject(text: &str) -> ReadIn {
    ReadIn::new(ReadInSubject::parse(text).unwrap())
}

fn compile(compartments: impl IntoIterator<Item = Compartment>) -> CompiledCompartments {
    Compartments::of(compartments)
        .unwrap()
        .compile(&roster())
        .unwrap()
}

fn session(agent: &str, operation: &str) -> FieldPrincipal {
    FieldPrincipal::new(agent, operation, Some(format!("{operation}-1")), "field")
}

fn mission(id: &str, operation: &str) -> CompartmentResource {
    CompartmentResource::Mission {
        id: id.to_owned(),
        operation: operation.to_owned(),
    }
}

fn on(operation: &str) -> CompartmentResource {
    CompartmentResource::Operation(operation.to_owned())
}

use CompartmentAction::{MissionStatus, ProductRead};

fn kestrel() -> Compartment {
    Compartment::new("KESTREL").with_actions([MissionStatus])
}

#[test]
fn a_cover_reaches_a_child_s_missions_unless_it_does_not_propagate() {
    let down = compile([kestrel()
        .with_cover(operation("P"))
        .with_read_in(subject("operation:A"))]);
    let reader = session("007", "A");
    assert_eq!(
        down.field_allows(&reader, MissionStatus, &mission("C-1", "C")),
        Some("KESTREL")
    );
    assert_eq!(
        down.field_allows(&reader, MissionStatus, &on("D")),
        Some("KESTREL")
    );
    let only = compile([kestrel()
        .with_cover(operation("P").with_propagate(false))
        .with_read_in(subject("operation:A"))]);
    assert_eq!(
        only.field_allows(&reader, MissionStatus, &mission("C-1", "C")),
        None
    );
    assert_eq!(
        only.field_allows(&reader, MissionStatus, &mission("P-1", "P")),
        Some("KESTREL"),
        "a covered Operation always includes its own Missions"
    );
    assert_eq!(
        down.field_allows(&session("007", "Q"), MissionStatus, &on("P")),
        None
    );
    assert_eq!(
        down.field_allows(&reader, MissionStatus, &on("Q")),
        None,
        "nothing propagates upward or sideways"
    );
}

#[test]
fn a_read_in_reaches_a_child_s_session_unless_it_does_not_propagate() {
    let cover_q =
        |read_in: ReadIn| compile([kestrel().with_cover(operation("Q")).with_read_in(read_in)]);
    let tree = cover_q(subject("operation:P"));
    assert_eq!(
        tree.field_allows(&session("007", "C"), MissionStatus, &on("Q")),
        Some("KESTREL")
    );
    let only = cover_q(subject("operation:P").with_propagate(false));
    assert_eq!(
        only.field_allows(&session("007", "C"), MissionStatus, &on("Q")),
        None
    );
    assert_eq!(
        only.field_allows(&session("007", "P"), MissionStatus, &on("Q")),
        Some("KESTREL")
    );
}

#[test]
fn a_mission_read_in_reaches_only_that_mission_s_sessions() {
    let compiled = compile([kestrel()
        .with_cover(operation("Q"))
        .with_read_in(subject("mission:A-1"))]);
    assert_eq!(
        compiled.field_allows(&session("007", "A"), MissionStatus, &on("Q")),
        Some("KESTREL")
    );
    let other = FieldPrincipal::new("007", "A", Some("A-2".to_owned()), "field");
    assert_eq!(compiled.field_allows(&other, MissionStatus, &on("Q")), None);
}

#[test]
fn an_agent_read_in_applies_in_every_operation_it_works_in() {
    let compiled = compile([kestrel()
        .with_cover(operation("Q"))
        .with_read_in(subject("agent:007"))]);
    for operation in ["A", "C"] {
        assert_eq!(
            compiled.field_allows(&session("007", operation), MissionStatus, &on("Q")),
            Some("KESTREL")
        );
    }
    assert_eq!(
        compiled.field_allows(&session("008", "A"), MissionStatus, &on("Q")),
        None
    );
}

#[test]
fn a_clearance_floor_skips_a_session_below_it() {
    let compiled = compile([kestrel()
        .with_cover(operation("Q"))
        .with_read_in(subject("operation:A"))
        .with_min_clearance("00")]);
    let at = |clearance: &str| FieldPrincipal::new("007", "A", None, clearance);
    assert_eq!(
        compiled.field_allows(&at("field"), MissionStatus, &on("Q")),
        None
    );
    assert_eq!(
        compiled.field_allows(&at("no such clearance"), MissionStatus, &on("Q")),
        None,
        "an unknown clearance counts as the lowest"
    );
    assert_eq!(
        compiled.field_allows(&at("00"), MissionStatus, &on("Q")),
        Some("KESTREL")
    );
    assert!(compiled.reachable(&at("field"), MissionStatus).is_empty());
    assert!(compiled.held(&at("field")).is_empty());
}

#[test]
fn two_compartments_give_their_union() {
    let compiled = compile([
        kestrel()
            .with_cover(operation("Q"))
            .with_read_in(subject("operation:A")),
        Compartment::new("LAPWING")
            .with_actions([ProductRead])
            .with_cover(operation("Q"))
            .with_read_in(subject("operation:A")),
    ]);
    let reader = session("007", "A");
    assert_eq!(
        compiled.field_allows(&reader, MissionStatus, &on("Q")),
        Some("KESTREL")
    );
    assert_eq!(
        compiled.field_allows(&reader, ProductRead, &on("Q")),
        Some("LAPWING")
    );
    assert_eq!(
        compiled.field_allows(&reader, CompartmentAction::CableSend, &on("Q")),
        None
    );
}

#[test]
fn reading_a_brief_implies_its_status_and_nothing_else() {
    let compiled = compile([Compartment::new("KESTREL")
        .with_actions([CompartmentAction::MissionRead])
        .with_cover(operation("Q"))
        .with_read_in(subject("operation:A"))]);
    let reader = session("007", "A");
    assert!(
        compiled
            .field_allows(&reader, MissionStatus, &on("Q"))
            .is_some()
    );
    assert!(
        compiled
            .field_allows(&reader, ProductRead, &on("Q"))
            .is_none()
    );
}

#[test]
fn an_action_for_one_kind_of_subject_alone_is_valid_and_grants_the_other_nothing() {
    let proposing = compile([Compartment::new("K")
        .with_actions([CompartmentAction::MissionPropose])
        .with_cover(operation("Q"))
        .with_read_in(subject("operation:A"))
        .with_read_in(subject("group:release"))]);
    assert_eq!(
        proposing.field_allows(
            &session("007", "A"),
            CompartmentAction::MissionPropose,
            &on("Q")
        ),
        Some("K")
    );
    let person = Person {
        groups: vec!["release".to_owned()],
        ..Person::default()
    };
    assert_eq!(
        proposing.person_compartments(&person, "Q"),
        Vec::<&str>::new()
    );
    let reading = compile([Compartment::new("K")
        .with_actions([CompartmentAction::CableRead])
        .with_cover(operation("Q"))
        .with_read_in(subject("operation:A"))
        .with_read_in(subject("group:release"))]);
    assert!(reading.held(&session("007", "A")).is_empty());
    assert_eq!(reading.person_compartments(&person, "Q"), vec!["K"]);
}

#[test]
fn people_and_sessions_are_asked_of_different_rules() {
    let compiled = compile([Compartment::new("KESTREL")
        .with_actions(CompartmentAction::ALL)
        .with_cover(operation("Q"))
        .with_read_in(subject("group:release"))
        .with_read_in(subject("agent:007"))]);
    let person = Person {
        id: "person-1".to_owned(),
        name: "m".to_owned(),
        groups: vec!["release".to_owned()],
        ..Person::default()
    };
    assert_eq!(
        compiled.person_allows(&person, CompartmentAction::CableRead, &on("Q")),
        Some("KESTREL")
    );
    assert_eq!(
        compiled.person_allows(&person, CompartmentAction::MissionPropose, &on("Q")),
        None,
        "proposing is for Agents"
    );
    let reader = session("007", "A");
    assert_eq!(
        compiled.field_allows(&reader, CompartmentAction::CableRead, &on("Q")),
        None,
        "cable.read is for people"
    );
    assert!(
        compiled
            .field_allows(&reader, CompartmentAction::MissionPropose, &on("Q"))
            .is_some()
    );
    // A person named like an Agent, or in a group named like an Operation, holds nothing here.
    let lookalike = Person {
        id: "agent:007".to_owned(),
        name: "007".to_owned(),
        groups: vec!["subtree:A".to_owned(), "operation:A".to_owned()],
        ..Person::default()
    };
    assert_eq!(
        compiled.person_allows(&lookalike, MissionStatus, &on("Q")),
        None
    );
    assert_eq!(compiled.person_compartments(&person, "Q"), vec!["KESTREL"]);
}

#[test]
fn a_covered_mission_is_reachable_and_its_operation_is_not() {
    let compiled = compile([kestrel()
        .with_cover(CompartmentCover::new(CoverTarget::Mission(
            "Q-7".to_owned(),
        )))
        .with_read_in(subject("operation:A"))]);
    let reader = session("007", "A");
    assert_eq!(
        compiled.field_allows(&reader, MissionStatus, &mission("Q-7", "Q")),
        Some("KESTREL")
    );
    assert_eq!(
        compiled.field_allows(&reader, MissionStatus, &mission("Q-8", "Q")),
        None
    );
    assert_eq!(
        compiled.field_allows(&reader, MissionStatus, &on("Q")),
        None
    );
    assert_eq!(
        compiled.reachable(&reader, MissionStatus),
        vec![CompartmentResource::Mission {
            id: "Q-7".to_owned(),
            operation: String::new()
        }]
    );
}

#[test]
fn reachable_expands_the_tree_and_held_says_what_a_session_holds() {
    let compiled = compile([kestrel()
        .with_description("Release work")
        .with_cover(operation("P"))
        .with_read_in(subject("operation:A"))]);
    let reader = session("007", "A");
    assert_eq!(
        compiled.reachable(&reader, MissionStatus),
        vec![on("C"), on("D"), on("P")]
    );
    let held = compiled.held(&reader);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, "KESTREL");
    assert_eq!(held[0].description, "Release work");
    assert_eq!(held[0].actions, vec![MissionStatus]);
    assert_eq!(held[0].operations, vec!["C", "D", "P"]);
    assert!(compiled.held(&session("007", "Q")).is_empty());
}

#[test]
fn an_action_outside_the_vocabulary_is_invalid_and_named() {
    let refused = Compartment::from_definition(
        "KESTREL",
        &json!({"actions": ["roster.manage"], "covers": [{"operation": "Q"}]}),
    )
    .unwrap_err();
    assert_eq!(refused.kind(), RosterErrorKind::Invalid);
    assert!(refused.to_string().contains("roster.manage"), "{refused}");
    assert!(
        refused.to_string().contains("no policy authority"),
        "{refused}"
    );
    assert!(
        CompartmentSettings::from_json(&json!({"actions": ["access.manage"]}))
            .unwrap_err()
            .contains("access.manage")
    );
}

#[test]
fn a_record_must_grant_something_on_something() {
    for (definition, reason) in [
        (json!({"covers": [{"operation": "Q"}]}), "no action"),
        (
            json!({"actions": ["mission.status"], "covers": [{"operation": "Q", "mission": "m"}]}),
            "one Operation or one Mission",
        ),
        (
            json!({"actions": ["mission.status"], "covers": [{"mission": "m", "propagate": false}]}),
            "propagate",
        ),
        (
            json!({"actions": ["mission.status"], "covers": [{"operation": "Q"}], "read_in": [{"subject": "root:all"}]}),
            "read_in",
        ),
        (
            json!({"actions": ["mission.status"], "covers": [{"operation": "Q"}], "read_in": [{"subject": "agent:007", "propagate": false}]}),
            "propagate",
        ),
        (
            json!({"actions": ["mission.status"], "covers": [{"operation": "Q"}], "limits": {"rate": 0}}),
            "limits.rate",
        ),
    ] {
        let refused = Compartment::from_definition("KESTREL", &definition)
            .unwrap_err()
            .to_string();
        assert!(refused.contains(reason), "{reason}: {refused}");
    }
    assert!(
        Compartment::from_definition(
            "not one word",
            &json!({"actions": ["mission.status"], "covers": [{"operation": "Q"}]})
        )
        .is_err()
    );
}

#[test]
fn an_unknown_operation_refuses_the_snapshot_and_an_unknown_mission_does_not() {
    for compartment in [
        kestrel().with_cover(operation("nowhere")),
        kestrel()
            .with_cover(operation("Q"))
            .with_read_in(subject("operation:nowhere")),
    ] {
        let refused = Compartments::of([compartment])
            .unwrap()
            .compile(&roster())
            .unwrap_err();
        assert!(refused.to_string().contains("nowhere"), "{refused}");
    }
    assert!(
        Compartments::of([kestrel()
            .with_cover(CompartmentCover::new(CoverTarget::Mission(
                "gone".to_owned()
            )))
            .with_read_in(subject("mission:also-gone"))])
        .unwrap()
        .compile(&roster())
        .is_ok()
    );
}

fn with_pairs(pairs: u64) -> Compartment {
    let mut compartment = kestrel().with_cover(operation("Q"));
    for agent in 0..pairs {
        compartment = compartment.with_read_in(subject(&format!("agent:a{agent}")));
    }
    compartment
}

#[test]
fn past_ten_thousand_rules_is_refused_by_default_and_accepted_with_none() {
    let at_bound = Compartments::of([with_pairs(10_000)]).unwrap();
    assert!(at_bound.compile(&roster()).is_ok());
    let past = Compartments::of([with_pairs(10_001)]).unwrap();
    let refused = past.compile(&roster()).unwrap_err();
    assert!(
        refused.to_string().contains("max_compartment_rules"),
        "{refused}"
    );
    let unbounded = roster_with(RosterSettings::new().with_max_compartment_rules(Limit::UNBOUNDED));
    assert!(past.compile(&unbounded).is_ok());
}

#[test]
fn weakenings_name_each_limit_looser_than_its_default() {
    let plain = kestrel().with_cover(operation("Q"));
    assert!(plain.weakenings().is_empty());
    let limits = CompartmentLimits {
        max_fanout: Limit::UNBOUNDED,
        rate: Rate::at(20.0),
        burst: Limit::at(10),
        ..CompartmentLimits::default()
    };
    let weakened = plain.with_limits(limits).weakenings();
    assert_eq!(weakened.len(), 2, "{weakened:?}");
    assert!(
        weakened.iter().any(|line| line.contains("max_fanout")),
        "{weakened:?}"
    );
    assert!(
        weakened.iter().any(|line| line.contains("rate")),
        "{weakened:?}"
    );
    assert!(
        !weakened.iter().any(|line| line.contains("burst")),
        "{weakened:?}"
    );
}

fn definition() -> serde_json::Value {
    json!({
        "description": "Release work",
        "actions": ["mission.status", "product.read", "cable.send"],
        "covers": [{"operation": "Q"}, {"mission": "A-41"}],
        "read_in": [{"subject": "operation:A"}, {"subject": "group:release"}, {"subject": "agent:007"}],
        "min_clearance": "field",
        "limits": {"max_fanout": "none", "rate": 2.5}
    })
}

#[test]
fn a_record_keeps_its_shape_through_its_definition() {
    let read = Compartment::from_definition("KESTREL", &definition()).unwrap();
    assert_eq!(read.limits.max_fanout, Limit::UNBOUNDED);
    assert_eq!(read.limits.rate, Rate::at(2.5));
    assert_eq!(read.limits.burst, Limit::at(50));
    assert_eq!(read.min_clearance.as_deref(), Some("field"));
    let again = Compartment::from_definition("KESTREL", &read.to_definition()).unwrap();
    assert_eq!(read, again);
}

const OPERATIONS: &str =
    "[operation.A]\n[operation.P]\n[operation.C]\nparent = \"P\"\n[operation.Q]\n";

#[test]
fn the_same_compartment_loads_through_every_provider() {
    let expected = Compartment::from_definition("KESTREL", &definition()).unwrap();
    let toml = "[compartment.KESTREL]\ndescription = \"Release work\"\n\
        actions = [\"mission.status\", \"product.read\", \"cable.send\"]\n\
        covers = [{ operation = \"Q\" }, { mission = \"A-41\" }]\n\
        read_in = [{ subject = \"operation:A\" }, { subject = \"group:release\" }, { subject = \"agent:007\" }]\n\
        min_clearance = \"field\"\nlimits = { max_fanout = \"none\", rate = 2.5 }\n";
    let yaml = "compartment:\n  KESTREL:\n    description: Release work\n    \
        actions: [mission.status, product.read, cable.send]\n    \
        covers: [{operation: Q}, {mission: A-41}]\n    \
        read_in: [{subject: 'operation:A'}, {subject: 'group:release'}, {subject: 'agent:007'}]\n    \
        min_clearance: field\n    limits: {max_fanout: none, rate: 2.5}\n";
    let json_text =
        serde_json::to_string(&json!({"compartment": {"KESTREL": definition()}})).unwrap();
    for (name, text) in [
        ("compartments.toml", toml),
        ("compartments.yaml", yaml),
        ("compartments.json", json_text.as_str()),
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("operations.toml"), OPERATIONS).unwrap();
        std::fs::write(root.path().join(name), text).unwrap();
        let provider = FileProvider::new(root.path());
        let snapshot = provider.load("").unwrap();
        assert_eq!(
            snapshot.settings.compartments.get("KESTREL"),
            Some(&expected),
            "{name}"
        );
        assert_eq!(
            current(&provider).unwrap().get("KESTREL"),
            Some(&expected),
            "{name}"
        );
    }
    let memory = MemoryProvider::new(roster()).with_settings(Settings {
        compartments: Compartments::of([expected.clone()]).unwrap(),
        ..Settings::default()
    });
    assert_eq!(current(&memory).unwrap().get("KESTREL"), Some(&expected));
    #[cfg(feature = "sqlite")]
    {
        use crate::directorates::m::config::SqlProvider;
        let root = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", root.path().join("config.db").display());
        let provider = SqlProvider::open(&location).unwrap();
        let global = ConfigScope::Global;
        for id in ["A", "Q"] {
            provider
                .put(ConfigRecord::Operation, &global, id, &json!({}))
                .unwrap();
        }
        provider
            .put(ConfigRecord::Compartment, &global, "KESTREL", &definition())
            .unwrap();
        assert_eq!(
            provider
                .load("")
                .unwrap()
                .settings
                .compartments
                .get("KESTREL"),
            Some(&expected)
        );
        assert_eq!(current(&provider).unwrap().get("KESTREL"), Some(&expected));
    }
}

#[cfg(feature = "sqlite")]
mod sql {
    use super::*;
    use crate::directorates::m::config::SqlProvider;

    fn provider(root: &std::path::Path) -> SqlProvider {
        let location = format!("sqlite:{}", root.join("config.db").display());
        let provider = SqlProvider::open(&location).unwrap();
        for id in ["A", "Q"] {
            provider
                .put(
                    ConfigRecord::Operation,
                    &ConfigScope::Global,
                    id,
                    &json!({}),
                )
                .unwrap();
        }
        provider
    }

    #[test]
    fn a_provider_refuses_what_leaves_a_compartment_invalid_and_keeps_the_last_good() {
        let root = tempfile::tempdir().unwrap();
        let provider = provider(root.path());
        let global = ConfigScope::Global;
        let refused = provider
            .put(
                ConfigRecord::Compartment,
                &global,
                "KESTREL",
                &json!({"actions": ["roster.manage"], "covers": [{"operation": "Q"}]}),
            )
            .unwrap_err();
        assert_eq!(refused.kind(), RosterErrorKind::Invalid);
        let refused = provider
            .put(
                ConfigRecord::Compartment,
                &global,
                "KESTREL",
                &json!({"actions": ["mission.status"], "covers": [{"operation": "nowhere"}]}),
            )
            .unwrap_err();
        assert_eq!(refused.kind(), RosterErrorKind::Invalid);
        assert!(provider.load("").unwrap().settings.compartments.is_empty());
        provider
            .put(
                ConfigRecord::Compartment,
                &global,
                "KESTREL",
                &json!({"actions": ["mission.status"], "covers": [{"operation": "Q"}]}),
            )
            .unwrap();
        let refused = provider
            .remove(ConfigRecord::Operation, &global, "Q")
            .unwrap_err();
        assert_eq!(refused.kind(), RosterErrorKind::Invalid, "{refused}");
        assert!(provider.load("").unwrap().roster.operation("Q").is_ok());
    }

    #[test]
    fn a_removed_grant_holds_from_the_next_read() {
        let root = tempfile::tempdir().unwrap();
        let provider = provider(root.path());
        let global = ConfigScope::Global;
        let reader = session("007", "A");
        assert_eq!(
            current(&provider)
                .unwrap()
                .field_allows(&reader, MissionStatus, &on("Q")),
            None
        );
        provider
            .put(
                ConfigRecord::Compartment,
                &global,
                "KESTREL",
                &json!({
                    "actions": ["mission.status"],
                    "covers": [{"operation": "Q"}],
                    "read_in": [{"subject": "operation:A"}]
                }),
            )
            .unwrap();
        assert_eq!(
            current(&provider)
                .unwrap()
                .field_allows(&reader, MissionStatus, &on("Q")),
            Some("KESTREL")
        );
        provider
            .remove(ConfigRecord::Compartment, &global, "KESTREL")
            .unwrap();
        assert_eq!(
            current(&provider)
                .unwrap()
                .field_allows(&reader, MissionStatus, &on("Q")),
            None
        );
    }
}

/// A provider that fails every read, as a database that is down does.
struct Down;

impl ConfigProvider for Down {
    fn load(&self, _station: &str) -> Result<crate::ConfigSnapshot, RosterError> {
        Err(crate::directorates::m::roster::error_of(
            RosterErrorKind::Unavailable,
            "down",
        ))
    }
    fn definitions(
        &self,
        _record: ConfigRecord,
        _scope: &ConfigScope,
    ) -> Result<crate::ConfigDefinitions, RosterError> {
        Err(crate::directorates::m::roster::error_of(
            RosterErrorKind::Unavailable,
            "down",
        ))
    }
}

#[test]
fn a_provider_that_cannot_be_read_answers_nothing() {
    assert_eq!(
        current(&Down).unwrap_err().kind(),
        RosterErrorKind::Unavailable
    );
}

#[test]
fn a_record_that_cannot_be_read_grants_nothing_and_the_rest_still_answer() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("operations.toml"), OPERATIONS).unwrap();
    std::fs::write(
        root.path().join("compartments.toml"),
        "[compartment.GOOD]\nactions = [\"mission.status\"]\ncovers = [{ operation = \"Q\" }]\n\
         read_in = [{ subject = \"operation:A\" }]\n\
         [compartment.BROKEN]\nactions = [\"roster.manage\"]\ncovers = [{ operation = \"Q\" }]\n\
         read_in = [{ subject = \"operation:A\" }]\n",
    )
    .unwrap();
    let provider = FileProvider::new(root.path());
    assert!(provider.load("").is_err(), "a snapshot refuses it whole");
    let compiled = current(&provider).unwrap();
    let reader = session("007", "A");
    assert_eq!(
        compiled.field_allows(&reader, MissionStatus, &on("Q")),
        Some("GOOD")
    );
    assert!(compiled.get("BROKEN").is_none());
    assert!(compiled.refused().contains_key("BROKEN"));
    assert_eq!(
        compiled.field_allows(&reader, CompartmentAction::CableSend, &on("Q")),
        None
    );
}

#[test]
fn a_tree_moved_under_a_cover_or_out_of_it_is_read_at_once() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("operations.toml"), OPERATIONS).unwrap();
    std::fs::write(
        root.path().join("compartments.toml"),
        "[compartment.K]\nactions = [\"mission.status\"]\ncovers = [{ operation = \"P\" }]\n\
         read_in = [{ subject = \"operation:A\" }]\n",
    )
    .unwrap();
    let provider = FileProvider::new(root.path());
    let reader = session("007", "A");
    assert!(
        current(&provider)
            .unwrap()
            .field_allows(&reader, MissionStatus, &on("C"))
            .is_some()
    );
    std::fs::write(
        root.path().join("operations.toml"),
        "[operation.A]\n[operation.P]\n[operation.C]\n[operation.Q]\n",
    )
    .unwrap();
    assert!(
        current(&provider)
            .unwrap()
            .field_allows(&reader, MissionStatus, &on("C"))
            .is_none()
    );
}

#[test]
fn the_tree_from_records_stays_narrow_on_a_cycle() {
    let lineage = lineage_of(&[
        ("A".to_owned(), json!({"parent": "B"})),
        ("B".to_owned(), json!({"parent": "A"})),
        ("C".to_owned(), json!({"parent": "gone"})),
    ]);
    assert_eq!(lineage.ancestors("A"), ["B"]);
    assert_eq!(lineage.ancestors("C"), [] as [String; 0]);
}
