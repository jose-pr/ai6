//! `reading.json`, `validation.json`, `names.json`, `source.json`, `digest.json`, `compose.json`
//! and `effective.json`.

use std::collections::BTreeMap;

use super::support::{bytes_of, compare, facts_of, object, settings_of, strings};
use super::{Report, Runner};
use crate::{
    Change, Document, Environment, Error, OverlayTree, Source, TreeEntry, applies, compose,
    composed_environment, digest_entries, environments, normalize_name, preparation,
    read_catalog_document, read_document, read_dotenv, read_env_script_output, root_variable,
    skill_flags, valid_overlay_name, validate,
};
use serde_json::{Value, json};

pub(super) fn reading(runner: &Runner) -> Report {
    runner.each("reading.json", |case| {
        let data = bytes_of(case);
        let settings = settings_of(case);
        let got: Result<Value, Error> = match case["format"].as_str().expect("format") {
            "aos" => read_document(&data, &settings).map(Value::Object),
            "dotenv" => {
                let windows = case["os"].as_str() == Some("windows");
                let base: Vec<(String, String)> = case["env"]
                    .as_object()
                    .map(|env| {
                        env.iter()
                            .map(|(name, value)| {
                                (name.clone(), value.as_str().expect("text").to_owned())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut env = Environment::new(base, windows);
                for name in case["secret_variables"].as_array().into_iter().flatten() {
                    let name = name.as_str().expect("a name");
                    let value = env.get(name).unwrap_or_default().to_owned();
                    env.set(name, value, true);
                }
                read_dotenv(&data, &mut env).map(|set| {
                    Value::Object(
                        set.into_iter()
                            .map(|(name, value)| (name, Value::String(value)))
                            .collect(),
                    )
                })
            }
            "env-script" => read_env_script_output(&data, &settings).map(|set| {
                Value::Object(
                    set.into_iter()
                        .map(|(name, value)| (name, value.map_or(Value::Null, Value::String)))
                        .collect(),
                )
            }),
            "catalog" => read_catalog_document(
                &data,
                case["name"].as_str().expect("a file name"),
                &settings,
            )
            .map(Value::Object),
            other => panic!("format {other}"),
        };
        compare(got, &case["expect"])
    })
}

pub(super) fn validation(runner: &Runner) -> Report {
    runner.each("validation.json", |case| {
        let document = object(&case["document"]);
        let tree = case["files"].as_array().map(|files| {
            OverlayTree::from_paths(
                files
                    .iter()
                    .map(|path| path.as_str().expect("a path").to_owned()),
            )
        });
        let issues = validate(&document, tree.as_ref());
        let expect = &case["expect"];
        if let (Some(flags), Some(tree)) = (expect.get("flags"), tree.as_ref()) {
            let got: Vec<Value> = skill_flags(&document, tree)
                .into_iter()
                .map(Value::from)
                .collect();
            if Value::Array(got.clone()) != *flags {
                return Err(format!("flags {got:?}, expected {flags}"));
            }
        }
        if let Some(applies_expected) = expect["applies"].as_bool()
            && applies(&issues) != applies_expected
        {
            return Err(format!("applies {}, issues {issues:?}", applies(&issues)));
        }
        if expect["valid"] == true {
            return issues.is_empty().then_some(()).ok_or(format!("{issues:?}"));
        }
        let at = expect["at"].as_str().expect("at");
        if let Some(class) = expect.get("class").and_then(Value::as_str)
            && !issues
                .iter()
                .any(|issue| issue.at == at && issue.code.as_str() == class)
        {
            return Err(format!("no {class} at {at}: {issues:?}"));
        }
        issues
            .iter()
            .any(|issue| {
                issue.at == at
                    || issue.at.starts_with(&format!("{at}/"))
                    || (!issue.at.is_empty() && at.starts_with(&format!("{}/", issue.at)))
            })
            .then_some(())
            .ok_or(format!("nothing at {at}: {issues:?}"))
    })
}

pub(super) fn names(runner: &Runner) -> Report {
    runner.each("names.json", |case| {
        let name = case["name"].as_str().expect("name");
        let expect = &case["expect"];
        let valid = valid_overlay_name(name);
        if expect["valid"] != valid {
            return Err(format!("valid {valid}"));
        }
        if valid
            && (expect["normalized"] != normalize_name(name)
                || expect["variable"] != root_variable(name))
        {
            return Err(format!("{} {}", normalize_name(name), root_variable(name)));
        }
        Ok(())
    })
}

pub(super) fn source(runner: &Runner) -> Report {
    runner.each("source.json", |case| {
        let parsed = Source::parse(case["source"].as_str().expect("source"));
        let expect = &case["expect"];
        match parsed {
            Ok(source) if expect.get("error").is_none() => {
                let got = super::support::source(&source);
                (got == *expect).then_some(()).ok_or(format!("{got}"))
            }
            Ok(source) => Err(format!("parsed {source:?}")),
            Err(_) if expect.get("error").is_some() => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    })
}

pub(super) fn digest(runner: &Runner) -> Report {
    runner.each("digest.json", |case| {
        let mut entries = Vec::new();
        for item in case["tree"].as_array().expect("tree") {
            let path = match item.get("path_base64").and_then(Value::as_str) {
                Some(encoded) => super::support::decode(encoded),
                None => item["path"].as_str().expect("path").as_bytes().to_vec(),
            };
            let entry = match item["type"].as_str().unwrap_or("file") {
                "dir" => TreeEntry::Directory,
                "symlink" => TreeEntry::Symlink,
                "special" => TreeEntry::Special,
                _ => TreeEntry::File(bytes_of(item)),
            };
            entries.push((path, entry));
        }
        let got =
            digest_entries(entries.iter().map(|(path, entry)| {
                (path.split(|byte| *byte == b'/').collect::<Vec<_>>(), entry)
            }));
        let expect = &case["expect"];
        match got {
            Ok(tree) => {
                let lines: Vec<String> = tree
                    .lines()
                    .into_iter()
                    .map(|line| String::from_utf8(line).expect("lines are UTF-8"))
                    .collect();
                let got = json!({"digest": tree.digest.to_string(), "lines": lines});
                (got == *expect).then_some(()).ok_or(format!("{got}"))
            }
            Err(error) => (expect["error"] == error.class.as_str())
                .then_some(())
                .ok_or(error.to_string()),
        }
    })
}

fn trees(case: &Value) -> BTreeMap<String, OverlayTree> {
    case["files"]
        .as_object()
        .map(|files| {
            files
                .iter()
                .map(|(identity, files)| {
                    let tree = match files {
                        Value::Array(paths) => OverlayTree::from_paths(
                            paths
                                .iter()
                                .map(|path| path.as_str().expect("a path").to_owned()),
                        ),
                        Value::Object(texts) => {
                            OverlayTree::from_texts(texts.iter().map(|(path, text)| {
                                (path.clone(), text.as_str().expect("text").to_owned())
                            }))
                        }
                        _ => OverlayTree::default(),
                    };
                    (identity.clone(), tree)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn env_data(env: &Environment) -> Value {
    Value::Object(
        env.values()
            .iter()
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect(),
    )
}

fn refusal_data(refusal: &Error) -> Value {
    json!({"error": refusal.class.as_str(), "subject": refusal.subject})
}

pub(super) fn compose_vectors(runner: &Runner) -> Report {
    runner.each("compose.json", |case| {
        let overlays: BTreeMap<String, Document> = case["overlays"]
            .as_object()
            .expect("overlays")
            .iter()
            .map(|(identity, manifest)| (identity.clone(), object(manifest)))
            .collect();
        let requested = strings(&case["requested"]);
        let trees = trees(case);
        let expect = &case["expect"];
        let composition = match compose(&overlays, &requested, &trees) {
            Ok(composition) => composition,
            Err(cycle) => {
                let got = json!({"error": "cycle", "chain": cycle.0});
                return (got == *expect).then_some(()).ok_or(format!("{got}"));
            }
        };
        let Some(facts) = case.get("facts") else {
            let pairs = |pairs: &[(String, String)]| -> Vec<Value> {
                pairs.iter().map(|(a, b)| json!([a, b])).collect()
            };
            let got = json!({
                "install_order": composition.install_order,
                "active": composition.active,
                "missing": pairs(&composition.missing),
                "root_variables": composition.root_variables,
                "routing": composition.routing,
                "rules": pairs(&composition.rules),
                "context": pairs(&composition.context),
                "skills": composition.skills,
                "skill_collisions": composition.skill_collisions,
            });
            return (got == *expect).then_some(()).ok_or(format!("{got}"));
        };
        let os = facts["os"].as_str().expect("os");
        let removed = strings(&facts["removed"]);
        let mut got = serde_json::Map::new();
        match environments(&overlays, &composition.active, &trees, os, &removed) {
            Err(lock) => {
                got.insert("active".into(), json!(composition.active));
                got.insert(
                    "env".into(),
                    json!({"error": lock.class.as_str(), "subject": lock.subject}),
                );
            }
            Ok(installed) => {
                let active: Vec<String> = composition
                    .active
                    .iter()
                    .filter(|identity| !installed.blocked.contains(identity))
                    .cloned()
                    .collect();
                got.insert("active".into(), json!(active));
                let environments: Vec<Value> = installed
                    .environments
                    .iter()
                    .map(super::support::package_environment)
                    .collect();
                let conflicts: Vec<Value> = installed
                    .conflicts
                    .iter()
                    .map(super::support::package_conflict)
                    .collect();
                let left: Vec<Value> = installed
                    .left
                    .iter()
                    .map(super::support::left_environment)
                    .collect();
                for (key, value) in [
                    ("environments", json!(environments)),
                    ("conflicts", json!(conflicts)),
                    ("blocked", json!(installed.blocked)),
                    ("left", json!(left)),
                ] {
                    if expect.get(key).is_some() {
                        got.insert(key.into(), value);
                    }
                }
                // Packages are given per overlay here; each overlay's own go in below.
                let shared = facts_of(facts);
                let roots = facts["roots"].as_object().expect("roots");
                let per: Vec<(&Document, crate::Facts)> = active
                    .iter()
                    .map(|identity| {
                        let mut own = shared.clone();
                        own.identity = identity.clone();
                        own.root = roots[identity].as_str().expect("a root").to_owned();
                        own.tree = trees.get(identity).cloned().unwrap_or_default();
                        own.packages = facts["packages"][identity]
                            .as_object()
                            .map(|packages| {
                                packages
                                    .iter()
                                    .map(|(runtime, dir)| {
                                        (runtime.clone(), dir.as_str().expect("a dir").to_owned())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        (&overlays[identity], own)
                    })
                    .collect();
                let base = Environment::new(
                    facts["base_env"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .map(|(name, value)| {
                            (name.clone(), value.as_str().expect("text").to_owned())
                        }),
                    os == "windows",
                );
                let pre_env: BTreeMap<String, crate::HookRun> = facts["pre_env"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(identity, run)| (identity.clone(), super::support::hook_run(run)))
                    .collect();
                let composed = composed_environment(&per, base, &pre_env, &mut Vec::new());
                got.insert(
                    "env".into(),
                    composed.env.as_ref().map_or_else(refusal_data, env_data),
                );
                if facts.get("pre_env").is_some() {
                    got.insert(
                        "hook_reports".into(),
                        json!(
                            composed
                                .hook_reports
                                .iter()
                                .map(|report| {
                                    let mut row = vec![json!(report.id)];
                                    row.extend(report.about.iter().map(|part| json!(part)));
                                    row
                                })
                                .collect::<Vec<_>>()
                        ),
                    );
                    got.insert(
                        "hook_non_defaults".into(),
                        json!(
                            composed
                                .hook_non_defaults
                                .iter()
                                .map(|(a, b)| json!([a, b]))
                                .collect::<Vec<_>>()
                        ),
                    );
                }
            }
        }
        let got = Value::Object(got);
        (got == *expect).then_some(()).ok_or(format!("{got}"))
    })
}

pub(super) fn effective_vectors(runner: &Runner) -> Report {
    runner.each(
        "effective.json",
        |case| {
            let manifest = object(&case["manifest"]);
            let raw = &case["facts"];
            let mut facts = facts_of(raw);
            facts.tree = raw["files"]
                .as_object()
                .map(|files| {
                    OverlayTree::from_texts(files.iter().map(|(path, text)| {
                        (path.clone(), text.as_str().expect("text").to_owned())
                    }))
                })
                .unwrap_or_default();
            let expect = &case["expect"];
            let got = match runner.effective_of(&manifest, &facts) {
                Err(refusal) => refusal_data(&refusal),
                Ok(result) => {
                    let scripts: Vec<Value> =
                        result.env_scripts.iter().map(super::support::script_run).collect();
                    let dependencies: Vec<Value> = result
                        .dependencies
                        .iter()
                        .map(super::support::dependency)
                        .collect();
                    let mut got = json!({
                        "ungranted": result.ungranted,
                        "supported": result.supported,
                        "env": env_data(&result.env),
                        "env_scripts": scripts,
                        "setup": result.setup.as_ref().map(super::support::setup_run),
                        "python": result.python,
                        "node": result.node,
                        "dependencies": dependencies,
                        "needs": super::support::needs(&result.needs),
                        "non_defaults": result.non_defaults.iter().map(|w| json!([w.id.as_str(), w.subject])).collect::<Vec<_>>(),
                    });
                    if !result.reports.is_empty() {
                        got["reports"] = json!(
                            result
                                .reports
                                .iter()
                                .map(|(id, subject)| json!([id, subject]))
                                .collect::<Vec<_>>()
                        );
                    }
                    if let Some(change) = raw.get("change") {
                        let change = match change.as_str().expect("change") {
                            "add" => Change::Add,
                            "sync" => Change::Sync,
                            "activate" => Change::Activate,
                            "remove" => Change::Remove,
                            other => panic!("change {other}"),
                        };
                        let record = raw.get("record").filter(|r| r.is_object()).map(|record| {
                            crate::SetupRecord::new(
                                record["digest"].as_str().expect("digest"),
                                crate::Phase::named(record["phase"].as_str().expect("phase"))
                                    .expect("a phase"),
                            )
                        });
                        let prepared = preparation(
                            change,
                            record.as_ref(),
                            raw["digest"].as_str(),
                            result.setup.as_ref(),
                            &result.dependencies,
                            raw["fails"].as_str().and_then(crate::FailStep::named),
                        );
                        got["setup_phase"] = json!(prepared.phase.map(|phase| phase.name()));
                        got["setup_arguments"] = json!(prepared.arguments);
                        got["steps"] = json!(
                            prepared
                                .steps
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                        );
                        got["record"] = json!(prepared.record.as_ref().map(super::support::setup_record));
                        got["activates"] = json!(prepared.activates);
                    }
                    got
                }
            };
            (got == *expect).then_some(()).ok_or(format!("{got}"))
        },
    )
}
