//! `hooks.json`: which hooks an overlay has and whether they are on, what each event's hook is
//! told, what of its output applies, which calls a `match` selects, the order the hooks of an event
//! run in, and the environment each gets.

use std::collections::BTreeMap;

use super::support::{object, strings};
use super::{Report, Runner};
use crate::{
    EnvSet, Environment, Facts, Hook, HookEvent, HookFacts, HookRun, HookScope, Limit, OnFailure,
    OverlayProcess, OverlayTree, Settings, composition_order, fire_event_for_call,
    hook_environment, hook_matches, hook_order, hook_payload, hook_plan, hook_result,
    process_environment, read_overlay, resolve, validate,
};
use serde_json::{Map, Value, json};

fn event_of(case: &Value) -> HookEvent {
    HookEvent::named(case["event"].as_str().expect("event")).expect("a known event")
}

fn settings_of(limits: &Value) -> Settings {
    let mut settings = Settings::default();
    let limit = |key: &str| limits[key].as_u64().map(Limit::at);
    if let Some(value) = limit("timeout") {
        settings = settings.hook_timeout(value);
    }
    if let Some(value) = limit("event_timeout") {
        settings = settings.hook_event_timeout(value);
    }
    if let Some(value) = limit("max_output") {
        settings = settings.hook_output_bytes(value);
    }
    if let Some(value) = limit("max_payload") {
        settings = settings.hook_payload_bytes(value);
    }
    if let Some(value) = limit("context_bytes") {
        settings = settings.context_bytes(value);
    }
    settings
}

fn hook_json(hook: &Hook) -> Value {
    json!({"event": hook.event.as_str(), "path": hook.path, "interpreter": hook.interpreter,
           "runs": hook.runs, "match": hook.select, "sets": hook.sets, "timeout": hook.timeout,
           "on_failure": match hook.on_failure { OnFailure::Refuse => "refuse", _ => "report" },
           "payload": hook.payload})
}

fn plan_case(case: &Value) -> Result<(), String> {
    let facts = &case["facts"];
    let tree = match facts["files"].clone() {
        Value::Array(paths) => OverlayTree::from_paths(
            paths
                .iter()
                .map(|path| path.as_str().expect("a path").to_owned()),
        ),
        Value::Object(texts) => OverlayTree::from_texts(
            texts
                .iter()
                .map(|(path, text)| (path.clone(), text.as_str().expect("text").to_owned())),
        ),
        _ => OverlayTree::default(),
    };
    let settings = settings_of(&facts["limits"]);
    let manifest = match case.get("manifest") {
        Some(manifest) => object(manifest),
        None => {
            read_overlay(&tree, &Settings::default())
                .map_err(|error| error.to_string())?
                .manifest
        }
    };
    let mut known = HookFacts::new(
        facts["os"].as_str().expect("os"),
        facts["identity"].as_str().unwrap_or_default(),
    );
    known.interpreters = facts.get("interpreters").map(strings);
    known.scope = match facts["scope"].as_str() {
        Some("user") => HookScope::User,
        Some("project") => HookScope::Project,
        _ => HookScope::Overlay,
    };
    known.digest = facts["digest"].as_str().map(str::to_owned);
    known.allowed = facts["allowed"].as_str().map(str::to_owned);
    known.tree = facts["tree"].as_object().map(|tree| {
        tree.iter()
            .map(|(path, text)| {
                (
                    path.clone(),
                    text.as_str().expect("text").as_bytes().to_vec(),
                )
            })
            .collect()
    });
    let events = |key: &str| -> Option<Vec<HookEvent>> {
        facts.get(key).map(|list| {
            strings(list)
                .iter()
                .map(|name| HookEvent::named(name).expect("an event"))
                .collect()
        })
    };
    known.fires = events("fires");
    known.partial = events("partial").unwrap_or_default();
    known.timeout = facts["limits"]["timeout"].as_u64();
    let plan = hook_plan(&manifest, &tree, &known, &settings).map_err(|error| error.to_string())?;
    let pairs = |list: &[(String, String)]| -> Vec<Value> {
        list.iter().map(|(a, b)| json!([a, b])).collect()
    };
    let expect = &case["expect"];
    let mut got = json!({
        "hooks": plan.hooks.iter().map(hook_json).collect::<Vec<_>>(),
        "on": plan.on, "activates": plan.activates, "needs": plan.needs,
        "reports": pairs(&plan.reports), "non_defaults": pairs(&plan.non_defaults),
    });
    if expect.get("issues").is_some() {
        got["issues"] = json!(
            validate(&manifest, Some(&tree))
                .into_iter()
                .map(|issue| issue.at)
                .collect::<Vec<_>>()
        );
    }
    (got == *expect).then_some(()).ok_or(format!("{got}"))
}

fn env_json(env: &[(String, Option<String>)]) -> Value {
    Value::Object(
        env.iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.clone().map_or(Value::Null, Value::String),
                )
            })
            .collect::<Map<_, _>>(),
    )
}

fn output_case(case: &Value) -> Result<(), String> {
    let limits = &case["limits"];
    let hook = super::support::hook_run(&case["run"]);
    let expect = &case["expect"];
    match hook_result(event_of(case), &hook, &settings_of(limits)) {
        Ok(output) => {
            let mut got = Map::new();
            if let Some(decision) = output.decision {
                got.insert("decision".into(), json!(decision.as_str()));
            }
            if let Some(reason) = &output.reason {
                got.insert("reason".into(), json!(reason));
            }
            if let Some(context) = &output.context {
                got.insert("context".into(), json!(context));
            }
            if let Some(input) = &output.input {
                got.insert("input".into(), Value::Object(input.clone()));
            }
            if let Some(env) = &output.env {
                got.insert("env".into(), env_json(env));
            }
            let got = json!({"output": got});
            (got == *expect).then_some(()).ok_or(format!("{got}"))
        }
        Err(failure) => (expect["failed"] == failure.as_str())
            .then_some(())
            .ok_or(format!("failed {}, expected {expect}", failure.as_str())),
    }
}

fn event_case(case: &Value) -> Result<(), String> {
    let runs: Vec<HookRun> = case["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .map(super::support::hook_run)
        .collect();
    let applies = case.get("applies").map(strings);
    let applies: Option<Vec<&str>> = applies
        .as_ref()
        .map(|list| list.iter().map(String::as_str).collect());
    let windows = case["os"].as_str() == Some("windows");
    let fired = fire_event_for_call(
        event_of(case),
        &runs,
        applies.as_deref(),
        &settings_of(&case["limits"]),
        windows,
        usize::try_from(case["context_used"].as_u64().unwrap_or(0)).expect("a size"),
        case.get("input"),
        case["kind"].as_str(),
    );
    let mut got = json!({
        "ran": fired.ran,
        "decision": fired.decision.map(|decision| decision.as_str()),
        "by": fired.by,
        "reason": fired.reason,
        "context": fired.context.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
        "env": env_json(&fired.env),
        "reports": fired.reports.iter().map(|report| {
            let mut row = vec![json!(report.id)];
            row.extend(report.about.iter().map(|part| json!(part)));
            Value::Array(row)
        }).collect::<Vec<_>>(),
        "non_defaults": fired.non_defaults.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
    });
    if let Some(seen) = &fired.seen {
        got["seen"] = seen.iter().map(|(a, b)| json!([a, b])).collect();
        got["input"] = fired.input.clone().unwrap_or(Value::Null);
    }
    (got == case["expect"])
        .then_some(())
        .ok_or(format!("{got}"))
}

fn overlays_of(case: &Value) -> BTreeMap<String, crate::Document> {
    case["overlays"]
        .as_object()
        .expect("overlays")
        .iter()
        .map(|(name, manifest)| (name.clone(), object(manifest)))
        .collect()
}

fn order_case(case: &Value) -> Result<(), String> {
    let scopes: Vec<HookScope> = strings(&case["scopes"])
        .iter()
        .map(|name| match name.as_str() {
            "user" => HookScope::User,
            "project" => HookScope::Project,
            other => panic!("scope {other}"),
        })
        .collect();
    let order = hook_order(
        &overlays_of(case),
        &strings(&case["requested"]),
        &strings(&case["has"]),
        &scopes,
    )
    .map_err(|cycle| cycle.to_string())?;
    let got = json!({"order": order.iter().map(|(scope, source)| {
        let scope = match scope {
            HookScope::User => "user",
            HookScope::Project => "project",
            _ => "overlay",
        };
        json!([scope, source])
    }).collect::<Vec<_>>()});
    (got == case["expect"])
        .then_some(())
        .ok_or(format!("{got}"))
}

fn environment_case(case: &Value) -> Result<(), String> {
    let raw = &case["facts"];
    let overlays = overlays_of(case);
    let names = composition_order(
        &overlays,
        &resolve(&overlays, &strings(&case["requested"]))
            .map_err(|cycle| cycle.to_string())?
            .install_order,
    );
    let shared = super::support::facts_of(raw);
    let active: Vec<(&crate::Document, Facts)> = names
        .iter()
        .map(|name| {
            let mut own = shared.clone();
            own.identity.clone_from(name);
            own.root = raw["roots"][name].as_str().expect("a root").to_owned();
            own.state = raw["states"][name].as_str().map(str::to_owned);
            own.tree = case["files"][name]
                .as_object()
                .map(|files| {
                    OverlayTree::from_texts(files.iter().map(|(path, text)| {
                        (path.clone(), text.as_str().expect("text").to_owned())
                    }))
                })
                .unwrap_or_default();
            (&overlays[name], own)
        })
        .collect();
    let base = Environment::new(
        shared.base_env.clone(),
        raw["os"].as_str() == Some("windows"),
    );
    let pre_env: BTreeMap<String, HookRun> = raw["pre_env"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, run)| (name.clone(), super::support::hook_run(run)))
        .collect();
    let identity = match case["scope"].as_str() {
        Some("user" | "project") => None,
        _ => Some(case["identity"].as_str().expect("an identity")),
    };
    let env = if let Some(process) = case["process"].as_str() {
        let process = match process {
            "setup" => OverlayProcess::Setup,
            "server" => OverlayProcess::Server,
            "command" => OverlayProcess::Command,
            "env-script" => OverlayProcess::EnvScript,
            other => panic!("process {other}"),
        };
        process_environment(
            process,
            &active,
            &base,
            &pre_env,
            identity.expect("an identity"),
        )
    } else {
        let earlier: Vec<EnvSet> = raw["earlier"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|set| {
                set.as_object()
                    .expect("an object")
                    .iter()
                    .map(|(name, value)| (name.clone(), value.as_str().map(str::to_owned)))
                    .collect()
            })
            .collect();
        hook_environment(event_of(case), &active, &base, &pre_env, identity, &earlier)
    }
    .map_err(|error| error.to_string())?;
    let got = json!({"env": Value::Object(
        env.values()
            .iter()
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect(),
    )});
    (got == case["expect"])
        .then_some(())
        .ok_or(format!("{got}"))
}

pub(super) fn hooks(runner: &Runner) -> Report {
    runner.each("hooks.json", |case| {
        let id = case["id"].as_str().expect("id");
        if id.starts_with("order-") {
            order_case(case)
        } else if id.starts_with("environment-") || id.starts_with("process-") {
            environment_case(case)
        } else if id.starts_with("plan-") {
            plan_case(case)
        } else if id.starts_with("payload-") {
            let got = Value::Object(hook_payload(event_of(case), &object(&case["known"])));
            (got == case["expect"])
                .then_some(())
                .ok_or(format!("{got}"))
        } else if id.starts_with("output-") {
            output_case(case)
        } else if id.starts_with("event-") {
            event_case(case)
        } else {
            let select = case["match"].as_array().map(|_| strings(&case["match"]));
            let got = hook_matches(
                event_of(case),
                select.as_deref(),
                case["value"].as_str().expect("value"),
            );
            (json!({"runs": got}) == case["expect"])
                .then_some(())
                .ok_or(format!("{got}"))
        }
    })
}
