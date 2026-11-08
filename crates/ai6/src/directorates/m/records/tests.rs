use guatiao::schema::read::SchemaRef;
use guatiao::schema::validate::validate_map;
use serde_json::{Value, json};

use super::*;
use crate::directorates::m::config::{StationRuntime, UnexRecord};

const KINDS: [ConfigRecord; 9] = [
    ConfigRecord::Operation,
    ConfigRecord::Agent,
    ConfigRecord::Role,
    ConfigRecord::Cover,
    ConfigRecord::Assistant,
    ConfigRecord::Roster,
    ConfigRecord::Compartment,
    ConfigRecord::Review,
    ConfigRecord::Watch,
];

/// Every key of an Operation's `issues`, apart, since a single `json!` has a recursion limit.
fn issue_policy_example() -> Value {
    json!({
        "preset": "licensed",
        "updates": "all",
        "max_body_bytes": 4096,
        "posts_per_hour": 3,
        "post_burst": 2,
        "posts_per_mission": 10,
        "operation_posts_per_hour": 30,
        "tracker_posts_per_hour": 100,
        "status_interval_secs": 120,
        "reply_ttl_secs": 3600,
        "on_timeout": "fail",
        "on_secret": "redact",
        "links": "any",
        "create": "off",
        "max_created": 5,
        "max_created_per_day": 20,
        "max_split_depth": 1,
        "max_split_children": 3,
        "duplicate_window_secs": 86400,
        "creation_approval_secs": 600,
    })
}

/// A record of each kind holding every key a person can write.
fn full_example(record: ConfigRecord) -> Value {
    match record {
        ConfigRecord::Operation => {
            let mut operation = json!({
                "name": "alpha",
                "description": "The alpha project",
                "parent": "PARENT",
                "inherit": false,
                "path": "/srv/alpha",
                "theater": "lab",
                "workspace": {
                    "provider": "git",
                    "source": "/srv/alpha.git",
                    "base": "main",
                    "branch": "{id}",
                    "isolate": false,
                    "checkpoint_remote": "origin",
                    "checkpoint_secs": 60
                },
                "config": "/etc/alpha",
                "roster": ["007"],
                "clearance": "00",
                "stations": ["station-a"],
                "station_labels": {"os": "linux"},
                "placement": "pack",
                "pools": ["main"],
                "cover": "covert",
                "cover_identity": {"name": "Field", "email": "field@example.invalid"},
                "on_blown": "fail",
                "max_concurrent": 3,
                "on_limit": "wait",
                "limit_fallback_secs": 60,
                "on_lost": "fail",
                "lost_after_secs": 90,
                "budget": {
                    "preset": "strict",
                    "session_tokens": 10,
                    "mission_tokens": 20,
                    "mission_cost_usd": 1.5,
                    "operation_cost_usd_per_day": 3,
                    "warn_at": 0.5
                },
                "exposure": {"preset": "observe", "restrict": ["cables"], "tools": ["WebFetch"]},
                "quarantine": {"window_hours": 2.5, "stalls": 1, "budget_stops": 1, "blown": false},
                "green_ink": {
                    "preset": "licensed",
                    "kinds": ["deploy"],
                    "requesters": ["field"],
                    "unattended": {"00": ["push"]},
                    "protected": ["main"],
                    "request_ttl_secs": 60
                },
                "vetting": {
                    "enabled": true,
                    "max_brief_bytes": 1000,
                    "max_per_hour": 5,
                    "max_per_source_per_hour": 5,
                    "duplicates": false,
                    "budget": false,
                    "screening": "all",
                    "phrases": ["deploy now"],
                    "html_comments": false,
                    "review": "off",
                    "rate_hold_hours": 2,
                    "max_proposal_reviews_per_hour": 4,
                    "max_proposal_reviews_per_session_per_hour": 2
                },
                "harness": {"project_settings": "ignore"},
                "promotion": {
                    "field": {
                        "missions": 1,
                        "max_failure_rate": 0.5,
                        "quiet_days": 2,
                        "quiet_on": ["blown"]
                    },
                    "00": {"missions": 2}
                },
                "sources": {"tracker": {"kind": "github", "repository": "a/b", "every": 60}},
                "sleepers": {
                    "round": {
                        "schedule": "0 6 * * 1",
                        "title": "Weekly round",
                        "prompt": "Look around",
                        "agent": "007",
                        "role": "case-officer",
                        "priority": 2,
                        "stations": ["station-a"],
                        "each_station": true
                    }
                }
            });
            operation["issues"] = issue_policy_example();
            // Set apart: one more key in the literal above passes `json!`'s recursion limit.
            operation["memory_write"] = json!("field");
            operation["overlays"] = json!(["engineering"]);
            operation["scorecard"] = json!({
                "window_days": 7,
                "small_sample": 3,
                "propose": true,
                "hold_above": 0.6,
                "reorder_gap": 0.25,
                "auto_hold": false,
                "never_prefer": ["claude-fable-5-1"]
            });
            operation["review"] = json!({"plan": "none", "output": "two-models"});
            operation
        }
        ConfigRecord::Agent => json!({
            "description": "A case officer",
            "aliases": ["bond"],
            "extends": "base",
            "handler": "claude",
            "model": "opus",
            "effort": "high",
            "handler_settings": {"command": "agent", "args": ["--acp"], "request_timeout_ms": 5},
            "route": [{
                "handler": "codex",
                "model": "another-name",
                "effort": "low",
                "handler_settings": {"command": "agent"},
                "pool": "team",
                "env": {"GATEWAY": "https://llm.example"},
                "secrets": ["litellm"],
                "priority": 2
            }],
            "balance": "round_robin",
            "roles": ["case-officer"],
            "clearance": "field",
            "priority": 5,
            "tags": ["linux"],
            "context": "Be careful.",
            "env": {"LANG": "C"},
            "max_concurrent": 2,
            "pool": "main",
            "limits": {
                "stall_timeout": 45.5,
                "loop_timeout": 10,
                "loop_min_lines": 7,
                "blocked_after": 1.25,
                "max_runtime": 100,
                "history": 20,
                "normalize_digits": true
            },
            "budget": {"session_tokens": 10},
            "retire_on": "2030-01-01",
            "on_retire": "retired",
            "retire_warning_days": 3,
            "wake_on_cable": false,
            "wakes_per_hour": 5
        }),
        ConfigRecord::Role => json!({
            "purpose": "Runs cases",
            "require_approval_before_done": true,
            "deliverables": ["branch", "file:report.md"],
            "policy": "Follow the brief",
            "review": {"plan": "none", "output": "two-models"}
        }),
        ConfigRecord::Cover => json!({
            "description": "Named",
            "agent_author": true,
            "model": true,
            "announce": true,
            "trailers": ["Co-Authored-By"],
            "suggest": "Co-Authored-By: {agent}",
            "footer": "Filed.",
            "forbid": ["secret"]
        }),
        ConfigRecord::Assistant => json!({
            "personal_repo": "/srv/mp/{person_digest}.git",
            "shared": {"ops": {"repo": "/srv/ops.git", "write": true}},
            "org": {"repo": "/srv/org.git", "write": false},
            "allow_elevated": true,
            "allow_beside_local_missions": true,
            "interpreters": ["python3"],
            "network": "limited",
            "allowed_hosts": ["a.example"],
            "denied_networks": ["10.0.0.0/8"],
            "unsandboxed_scripts": true,
            "skills": {"pdf": {"path": "/srv/skills/pdf"}},
            "skills_unpinned": true,
            "skills_allow_tools": true,
            "group": {"ops": {
                "interpreters": ["python3"],
                "network": "denied",
                "allowed_hosts": ["a.example"],
                "denied_networks": ["10.0.0.0/8"],
                "unsandboxed_scripts": false,
                "skills": ["pdf"]
            }},
            "person": {"person-1": {"network": "denied"}},
            "views": {"blocks": 1},
            "limits": {"message_bytes": "none", "events": 3},
            "desk": {"idle": "none", "live": 2, "job_wait": 0.5},
            "gateway": {"conversations": 3, "conversation_age": 60, "token_ttl": 60},
            "pages": {"max_pages": 3, "network": "own", "scripts": false, "open_ttl": 60}
        }),
        ConfigRecord::Roster => json!({
            "max_operation_depth": 3,
            "max_compartment_rules": 100,
            "reports": {
                "classifications": ["open", "secret"],
                "handlings": ["here"],
                "priorities": ["now", "later"]
            }
        }),
        ConfigRecord::Review => json!({
            "panel": [
                {"role": "reviewer", "weight": 2, "veto": true},
                {"agent": "Tanner"}
            ],
            "pass_at": 0.66,
            "distinct_models": 2,
            "rounds": 2,
            "redo_limit": 2,
            "on_no_agreement": "refuse",
            "max_turns": 12,
            "max_tokens": 100000,
            "reviewer_timeout_secs": 600
        }),
        ConfigRecord::Watch => json!({
            "agent": "M",
            "operation": "G",
            "wake_every": "none",
            "max_turns_per_hour": 12,
            "max_events_per_turn": 20,
            "max_waiting_events": "none",
            "turn_timeout": 600,
            "lease": 30
        }),
        ConfigRecord::Compartment => json!({
            "description": "Release work",
            "actions": ["mission.status", "product.read"],
            "min_clearance": "field",
            "limits": {
                "max_fanout": 10,
                "rate": 1.5,
                "burst": 5,
                "sender_rate": "none",
                "sender_burst": 2,
                "max_product_bytes": 1024
            }
        }),
        other => panic!("no example for {other:?}"),
    }
}

/// Every place below `path` a person writes a value, as `(path, its schema)`.
fn writable<'a>(path: String, node: &'a Value, found: &mut Vec<(String, &'a Value)>) {
    if let Some(properties) = node.get("properties").and_then(Value::as_object) {
        for (key, field) in properties {
            let at = format!("{path}.{key}");
            found.push((at.clone(), field));
            writable(at, field, found);
        }
    }
    for (keyword, step) in [("additionalProperties", "[*]"), ("items", "[]")] {
        if let Some(inner) = node.get(keyword).filter(|inner| inner.is_object()) {
            writable(format!("{path}{step}"), inner, found);
        }
    }
    if let Some(arms) = node.get("anyOf").and_then(Value::as_array) {
        for arm in arms {
            writable(path.clone(), arm, found);
        }
    }
}

fn untitled(schema: &Value) -> Vec<String> {
    let mut found = Vec::new();
    writable(String::new(), schema, &mut found);
    found
        .into_iter()
        .filter(|(_, field)| field.get("title").and_then(Value::as_str).is_none())
        .map(|(path, _)| path)
        .collect()
}

#[test]
fn every_kind_has_a_form_that_fits_its_schema() {
    for record in KINDS {
        let form = record.form().unwrap();
        assert!(form.is_some(), "{record:?}");
    }
    assert_eq!(ConfigRecord::Theater.form(), Ok(None));
}

#[test]
fn every_field_a_person_can_write_has_a_title() {
    for record in KINDS {
        let form = record.form().unwrap().unwrap();
        assert_eq!(untitled(&form.schema), Vec::<String>::new(), "{record:?}");
    }
    #[derive(guatiao::Schema)]
    struct Nameless {
        #[schema(title = "Has one")]
        #[expect(dead_code, reason = "only its schema is read")]
        named: String,
        #[expect(dead_code, reason = "only its schema is read")]
        nameless: String,
    }
    let schema = to_plain(&<Nameless as guatiao::Schema>::schema().unwrap());
    assert_eq!(untitled(&schema), vec![".nameless".to_owned()]);
}

#[test]
fn every_field_has_words_for_the_person_reading_it() {
    for record in KINDS {
        let form = record.form().unwrap().unwrap();
        let mut found = Vec::new();
        writable(String::new(), &form.schema, &mut found);
        for (path, field) in found {
            let words = field.get("description").and_then(Value::as_str);
            assert!(
                words.is_some_and(|words| !words.is_empty()),
                "{record:?}{path} says nothing"
            );
        }
    }
}

fn to_plain(value: &guatiao::Value) -> Value {
    serde_json::to_value(Serializable::from(value)).unwrap()
}

fn all_keys_of(schema: &Value, example: &Value, path: &str, missing: &mut Vec<String>) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (key, field) in properties {
        let at = format!("{path}.{key}");
        match example.get(key) {
            Some(value) => all_keys_of(field, value, &at, missing),
            None => missing.push(at),
        }
    }
}

#[test]
fn a_full_example_reads_through_the_type_and_through_its_schema() {
    for record in KINDS {
        let example = full_example(record);
        let form = record.form().unwrap().unwrap();
        // What the example leaves out would not be held to this test.
        let mut missing = Vec::new();
        all_keys_of(&form.schema, &example, "", &mut missing);
        missing.retain(|key| {
            // A key holding another arm of what the record already sets.
            !matches!(
                key.as_str(),
                ".harness.pin" | ".skills.*" | ".promotion.00.max_failure_rate"
            ) && (record == ConfigRecord::Operation || !key.starts_with(".budget."))
                && !key.starts_with(".views.")
                && !key.starts_with(".limits.")
                && !key.starts_with(".desk.")
                && !key.starts_with(".gateway.")
                && !key.starts_with(".pages.")
                && !key.starts_with(".promotion.00.")
        });
        assert_eq!(missing, Vec::<String>::new(), "{record:?} example");

        let schema_value = from_serde(&form.schema).unwrap();
        let schema = SchemaRef::new(&schema_value).unwrap();
        let value = from_serde(&example).unwrap();
        validate_map(schema, &value).unwrap_or_else(|error| panic!("{record:?} schema: {error}"));
        assert!(
            records_parse(record, &example).is_ok(),
            "{record:?} type: {:?}",
            records_parse(record, &example).err()
        );
    }
}

/// The record read through its type.
fn records_parse(record: ConfigRecord, definition: &Value) -> Result<(), String> {
    match record {
        ConfigRecord::Operation => from_json::<OperationDefinition>(definition).map(drop),
        ConfigRecord::Agent => from_json::<AgentDefinition>(definition).map(drop),
        ConfigRecord::Role => from_json::<AgentRoleDefinition>(definition).map(drop),
        ConfigRecord::Cover => from_json::<CoverDefinition>(definition).map(drop),
        ConfigRecord::Assistant => from_json::<AssistantSettings>(definition).map(drop),
        ConfigRecord::Roster => from_json::<RosterSettings>(definition).map(drop),
        ConfigRecord::Compartment => from_json::<CompartmentSettings>(definition).map(drop),
        ConfigRecord::Review => {
            from_json::<crate::directorates::m::review::ReviewPolicy>(definition).map(drop)
        }
        ConfigRecord::Watch => {
            from_json::<crate::directorates::m::watches::WatchDefinition>(definition).map(drop)
        }
        other => panic!("no type for {other:?}"),
    }
}

#[test]
fn a_key_nothing_declares_is_refused_by_the_type_and_the_schema_alike() {
    for record in KINDS {
        let mut example = full_example(record);
        example["no_such_key"] = json!("the value must not show");
        let refused = records_parse(record, &example).unwrap_err();
        assert!(refused.contains("no_such_key"), "{record:?}: {refused}");
        assert!(
            !refused.contains("the value must not show"),
            "{record:?}: {refused}"
        );
        let form = record.form().unwrap().unwrap();
        let schema_value = from_serde(&form.schema).unwrap();
        let value = from_serde(&example).unwrap();
        assert!(validate_map(SchemaRef::new(&schema_value).unwrap(), &value).is_err());
    }
}

#[test]
fn a_refusal_names_a_nested_key_and_not_its_value() {
    let refused =
        from_json::<AgentDefinition>(&json!({"budget": {"session_tokens": "plenty"}})).unwrap_err();
    assert!(refused.contains("budget.session_tokens"), "{refused}");
    assert!(!refused.contains("plenty"), "{refused}");
}

#[test]
fn a_default_reads_what_the_type_defaults_to() {
    let form = ConfigRecord::Operation.form().unwrap().unwrap();
    let properties = &form.schema["properties"];
    let empty: OperationDefinition = from_json(&json!({})).unwrap();
    assert_eq!(properties["cover"]["default"], empty.cover());
    assert_eq!(properties["clearance"]["default"], "field");
    assert_eq!(properties["inherit"]["default"], true);
    let form = ConfigRecord::Assistant.form().unwrap().unwrap();
    assert_eq!(
        form.schema["properties"]["desk"]["properties"]["live"]["default"],
        json!(crate::AssistantDeskLimits::default().live.get().unwrap())
    );
    assert_eq!(
        form.schema["properties"]["views"]["properties"]["blocks"]["default"],
        json!(24)
    );
}

#[test]
fn a_field_that_names_records_says_which() {
    let form = ConfigRecord::Agent.form().unwrap().unwrap();
    assert_eq!(form.schema["properties"]["roles"]["format"], "record:roles");
    assert_eq!(
        form.schema["properties"]["extends"]["format"],
        "record:agents"
    );
    let form = ConfigRecord::Operation.form().unwrap().unwrap();
    assert_eq!(
        form.schema["properties"]["roster"]["format"],
        "record:agents"
    );
    assert_eq!(
        form.schema["properties"]["parent"]["format"],
        "record:operations"
    );
}

#[test]
fn the_covers_an_operation_is_offered_are_the_built_in_ones() {
    let form = ConfigRecord::Operation.form().unwrap().unwrap();
    let offered = form.schema["properties"]["cover"]["examples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let built_in = crate::Covers::default();
    for name in &offered {
        assert!(built_in.get(name).is_ok(), "{name}");
    }
    assert_eq!(offered.len(), 3);
}

#[test]
fn a_family_with_one_record_fixes_its_key_and_others_ask_for_one() {
    let fixed = ConfigRecord::Assistant.form().unwrap().unwrap();
    assert_eq!(fixed.key.fixed.as_deref(), Some("settings"));
    let asked = ConfigRecord::Agent.form().unwrap().unwrap();
    assert_eq!(asked.key.fixed, None);
    assert!(!asked.key.title.is_empty());
}

#[test]
fn a_record_written_by_toml_reads_as_the_json_record_does() {
    let from_file: AgentDefinition = from_toml(
        "handler = \"claude\"\npriority = 2\n[limits]\nstall_timeout = 4.5\n[handler_settings]\nargs = [\"a\"]\n",
    )
    .unwrap();
    let from_record: AgentDefinition = from_json(&json!({
        "handler": "claude",
        "priority": 2,
        "limits": {"stall_timeout": 4.5},
        "handler_settings": {"args": ["a"]}
    }))
    .unwrap();
    assert_eq!(to_json(&from_file), to_json(&from_record));
}

/// A record as a provider holds it: every key, `null` for an unset option and for a lifted bound,
/// seconds as floats.
fn stored(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// `definitions` as the one record of each singleton family, through the checks a provider runs.
fn loaded(definitions: Vec<(ConfigRecord, Value)>) -> crate::ConfigSnapshot {
    let layer: crate::directorates::m::config::Layer = definitions
        .into_iter()
        .map(|(family, definition)| {
            (
                family,
                [(ConfigRecord::SINGLETON_KEY.to_owned(), definition)].into(),
            )
        })
        .collect();
    crate::directorates::m::config::assemble(
        crate::ConfigRevision::new(1),
        &layer,
        None,
        &"the fixture",
        &crate::ConfigLimits::default(),
    )
    .unwrap()
}

#[test]
fn stored_records_at_their_defaults_read_as_the_defaults() {
    let snapshot = loaded(vec![
        (ConfigRecord::Station, stored("station_record_bare.json")),
        (ConfigRecord::Unex, stored("unex_record_bare.json")),
    ]);
    assert_eq!(snapshot.settings.station, StationRuntime::default());
    assert_eq!(snapshot.settings.unex, UnexRecord::default());
}

#[test]
fn stored_records_keep_their_values_nulls_and_lifted_bounds() {
    let snapshot = loaded(vec![
        (
            ConfigRecord::Station,
            stored("station_record_as_stored.json"),
        ),
        (ConfigRecord::Unex, stored("unex_record_as_stored.json")),
    ]);
    let station = &snapshot.settings.station;
    assert_eq!(station.scheduler.event_queue, crate::Limit::UNBOUNDED);
    assert_eq!(station.secrets.max_lease_ttl, crate::TimeLimit::UNBOUNDED);
    assert_eq!(station.stall_timeout, Some(600.0));
    assert_eq!(station.blocked_after, Some(12.5));
    assert_eq!(station.partial_line_bytes, None);
    assert_eq!(
        station.roles.as_deref(),
        Some(&["execution".to_owned()][..])
    );
    assert_eq!(station.labels["site"], "west");
    assert_eq!(
        station.providers["pty"],
        serde_json::json!({
            "scrollback": 10, "shell": null, "ratio": 1.5,
            "nested": {"list": [1, "two", true]}
        })
    );
    let unex = &snapshot.settings.unex;
    assert_eq!(unex.authenticator.as_deref(), Some("password"));
    assert_eq!(
        Value::Object(unex.authenticator_config.clone().unwrap().0),
        serde_json::json!({"file": "accounts.toml", "rounds": 12, "ratio": 0.5, "note": null})
    );
    assert!(unex.directory_config.as_ref().unwrap().is_empty());
    assert_eq!(unex.accounts_config, None);
    assert_eq!(unex.access_cache_ttl, Some(5.0));
    assert_eq!(unex.gateway.frame_bytes, crate::Limit::UNBOUNDED);
    assert_eq!(unex.gateway.session_max, crate::TimeLimit::UNBOUNDED);
    assert_eq!(unex.provider_config_depth, crate::Limit::UNBOUNDED);
    assert!(
        unex.weakenings()
            .iter()
            .any(|line| line.starts_with("unex.gateway.frame_bytes none")),
        "{:?}",
        unex.weakenings()
    );
}

#[test]
fn what_a_record_writes_reads_back_as_itself() {
    let snapshot = loaded(vec![
        (
            ConfigRecord::Station,
            stored("station_record_as_stored.json"),
        ),
        (ConfigRecord::Unex, stored("unex_record_as_stored.json")),
    ]);
    let station = &snapshot.settings.station;
    let written = crate::directorates::m::records::to_json(station).unwrap();
    assert_eq!(&from_json::<StationRuntime>(&written).unwrap(), station);
    let unex = &snapshot.settings.unex;
    let written = crate::directorates::m::records::to_json(unex).unwrap();
    assert_eq!(&from_json::<UnexRecord>(&written).unwrap(), unex);
}

#[test]
fn a_refused_setting_names_its_key_and_never_its_value() {
    for (document, key) in [
        (
            serde_json::json!({"gateway": {"sessions": "hunter2"}}),
            "gateway.sessions",
        ),
        (serde_json::json!({"gateway": {"hunter2": 1}}), "hunter2"),
        (serde_json::json!({"directory": ["hunter2"]}), "directory"),
        (
            serde_json::json!({"authenticator_config": "hunter2"}),
            "authenticator_config",
        ),
        (
            serde_json::json!({"login": {"name_window": "hunter2"}}),
            "login.name_window",
        ),
    ] {
        let refusal = from_json::<UnexRecord>(&document).unwrap_err();
        assert!(refusal.contains(key), "{document}: {refusal}");
        if key != "hunter2" {
            assert!(!refusal.contains("hunter2"), "{document}: {refusal}");
        }
    }
    for (document, key) in [
        (
            serde_json::json!({"scheduler": {"interval": "hunter2"}}),
            "scheduler.interval",
        ),
        (
            serde_json::json!({"secrets": {"lease_ttl": -1}}),
            "secrets.lease_ttl",
        ),
        (serde_json::json!({"labels": {"site": 12345}}), "labels"),
        (
            serde_json::json!({"stall_timeout": "hunter2"}),
            "stall_timeout",
        ),
    ] {
        let refusal = from_json::<StationRuntime>(&document).unwrap_err();
        assert!(refusal.contains(key), "{document}: {refusal}");
        assert!(!refusal.contains("hunter2"), "{document}: {refusal}");
        assert!(!refusal.contains("12345"), "{document}: {refusal}");
    }
}

/// The forms as the app's preview and tests read them: the routes' answers, generated here.
const APP_COPY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../clients/outer-office/assets/record_forms.json"
);

#[test]
fn the_app_s_copy_of_the_forms_is_current() {
    let forms: serde_json::Map<String, Value> = [
        ("operations", ConfigRecord::Operation),
        ("agents", ConfigRecord::Agent),
        ("roles", ConfigRecord::Role),
        ("covers", ConfigRecord::Cover),
        ("assistant", ConfigRecord::Assistant),
        ("roster", ConfigRecord::Roster),
        ("compartments", ConfigRecord::Compartment),
        ("reviews", ConfigRecord::Review),
    ]
    .into_iter()
    .map(|(kind, record)| {
        (
            kind.to_owned(),
            serde_json::to_value(record.form().unwrap().unwrap()).unwrap(),
        )
    })
    .collect();
    let text = serde_json::to_string_pretty(&forms).unwrap()
        + "
";
    if std::env::var_os("AI6_UPDATE_RECORD_FORMS").is_some() {
        std::fs::write(APP_COPY, &text).unwrap();
    }
    let held = std::fs::read_to_string(APP_COPY).unwrap_or_default();
    assert!(
        held == text,
        "the app's copy of the forms is stale: run with AI6_UPDATE_RECORD_FORMS=1"
    );
}

#[test]
fn a_limit_keeps_its_three_spellings() {
    use crate::Limit;
    for (written, limit) in [
        (json!(3), Limit::at(3)),
        (json!("none"), Limit::UNBOUNDED),
        (Value::Null, Limit::UNBOUNDED),
    ] {
        let settings: RosterSettings = from_json(&json!({"max_operation_depth": written})).unwrap();
        assert_eq!(settings.max_operation_depth, limit);
    }
    assert!(from_json::<RosterSettings>(&json!({"max_operation_depth": "deep"})).is_err());
    assert!(from_json::<RosterSettings>(&json!({"max_operation_depth": -1})).is_err());
}
