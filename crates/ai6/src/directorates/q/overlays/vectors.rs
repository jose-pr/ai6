//! The AOS conformance vectors (`tests/fixtures/aos-vectors/`), read as data. Each test runs
//! every case of one file, names each failing case, and fails when any does.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value as Data, json};

use super::*;

fn cases(file: &str) -> Vec<Data> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/aos-vectors")
        .join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let document: Data = serde_json::from_str(&text).unwrap();
    document["cases"].as_array().unwrap().clone()
}

fn base64(text: &str) -> Vec<u8> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        let value = alphabet.iter().position(|known| *known == byte).unwrap() as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    out
}

/// Runs `check` on every case, collecting the failures; `skip` names a case left out, with
/// the reason.
fn run(
    file: &str,
    check: impl Fn(&Data) -> Result<(), String>,
    skip: impl Fn(&Data) -> Option<&'static str>,
) {
    let (mut passed, mut skipped, mut failed) = (0, 0, Vec::new());
    for case in cases(file) {
        let id = case["id"].as_str().unwrap_or_default().to_owned();
        if let Some(reason) = skip(&case) {
            skipped += 1;
            eprintln!("{file}: skipped {id}: {reason}");
            continue;
        }
        match check(&case) {
            Ok(()) => passed += 1,
            Err(why) => failed.push(format!("{id}: {why}")),
        }
    }
    eprintln!(
        "{file}: {passed} passed, {skipped} skipped, {} failed",
        failed.len()
    );
    assert!(failed.is_empty(), "{file}:\n{}", failed.join("\n"));
}

fn none(_: &Data) -> Option<&'static str> {
    None
}

fn expected_errors(expect: &Data) -> Vec<String> {
    match &expect["error"] {
        Data::String(class) => vec![class.clone()],
        Data::Array(classes) => classes
            .iter()
            .filter_map(Data::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn compare(got: Result<Data, String>, expect: &Data) -> Result<(), String> {
    let errors = expected_errors(expect);
    match got {
        Ok(data) if expect.get("data").is_some() => (data == expect["data"])
            .then_some(())
            .ok_or(format!("read {data}, expected {}", expect["data"])),
        Ok(data) => Err(format!("read {data}, expected an error {errors:?}")),
        Err(class) if errors.contains(&class) => Ok(()),
        Err(class) => Err(format!("refused {class}, expected {expect}")),
    }
}

#[test]
fn overlay_vectors_reading() {
    run(
        "reading.json",
        |case| {
            let data = match case.get("base64").and_then(Data::as_str) {
                Some(encoded) => base64(encoded),
                None => case["text"].as_str().unwrap().as_bytes().to_vec(),
            };
            let mut limits = ReadLimits::default();
            if let Some(bytes) = case["limits"]["max_bytes"].as_u64() {
                limits.bytes = crate::bounds::Limit::at(bytes);
            }
            if let Some(depth) = case["limits"]["max_depth"].as_u64() {
                limits.depth = crate::bounds::Limit::at(depth);
            }
            let got = match case["format"].as_str().unwrap() {
                "aos" => read_document(&data, &limits).map(Data::Object),
                "dotenv" => {
                    let windows = case["os"].as_str() == Some("windows");
                    let base: Vec<(String, String)> = case["env"]
                        .as_object()
                        .map(|env| {
                            env.iter()
                                .map(|(name, value)| {
                                    (name.clone(), value.as_str().unwrap().to_owned())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut env = Environment::new(base, windows);
                    for name in case["secret_variables"].as_array().into_iter().flatten() {
                        let name = name.as_str().unwrap();
                        let value = env.get(name).unwrap_or_default().to_owned();
                        env.set(name, value, true);
                    }
                    read_dotenv(&data, &mut env).map(|set| {
                        Data::Object(
                            set.into_iter()
                                .map(|(name, value)| (name, Data::String(value)))
                                .collect(),
                        )
                    })
                }
                "env-script" => read_env_script_output(&data).map(|set| {
                    Data::Object(
                        set.into_iter()
                            .map(|(name, value)| (name, value.map_or(Data::Null, Data::String)))
                            .collect(),
                    )
                }),
                other => panic!("format {other}"),
            };
            compare(
                got.map_err(|error| error.class().to_owned()),
                &case["expect"],
            )
        },
        |case| {
            (case["format"] == "catalog")
                .then_some("catalogs are the Store class, which AI6 does not implement")
        },
    );
}

#[test]
fn overlay_vectors_validation() {
    run(
        "validation.json",
        |case| {
            let document = case["document"].as_object().unwrap();
            let tree = case["files"].as_array().map(|files| {
                OverlayTree::from_paths(files.iter().map(|path| path.as_str().unwrap().to_owned()))
            });
            let issues = validate(document, tree.as_ref());
            let expect = &case["expect"];
            if let Some(applies_expected) = expect["applies"].as_bool()
                && applies(&issues) != applies_expected
            {
                return Err(format!("applies {}, issues {issues:?}", applies(&issues)));
            }
            if expect["valid"] == true {
                return issues.is_empty().then_some(()).ok_or(format!("{issues:?}"));
            }
            let at = expect["at"].as_str().unwrap();
            issues
                .iter()
                .any(|issue| {
                    issue.at == at
                        || issue.at.starts_with(&format!("{at}/"))
                        || (!issue.at.is_empty() && at.starts_with(&format!("{}/", issue.at)))
                })
                .then_some(())
                .ok_or(format!("nothing at {at}: {issues:?}"))
        },
        none,
    );
}

#[test]
fn overlay_vectors_names() {
    run(
        "names.json",
        |case| {
            let name = case["name"].as_str().unwrap();
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
        },
        none,
    );
}

#[test]
fn overlay_vectors_source() {
    run(
        "source.json",
        |case| {
            let parsed = Source::parse(case["source"].as_str().unwrap());
            let expect = &case["expect"];
            match parsed {
                Ok(source) if expect.get("error").is_none() => {
                    let got = serde_json::to_value(&source).unwrap();
                    (got == *expect).then_some(()).ok_or(format!("{got}"))
                }
                Ok(source) => Err(format!("parsed {source:?}")),
                Err(_) if expect.get("error").is_some() => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        },
        none,
    );
}

#[test]
fn overlay_vectors_digest() {
    run(
        "digest.json",
        |case| {
            let mut entries = Vec::new();
            for item in case["tree"].as_array().unwrap() {
                let path = match item.get("path_base64").and_then(Data::as_str) {
                    Some(encoded) => base64(encoded),
                    None => item["path"].as_str().unwrap().as_bytes().to_vec(),
                };
                let entry = match item["type"].as_str().unwrap_or("file") {
                    "dir" => TreeEntry::Directory,
                    "symlink" => TreeEntry::Symlink,
                    "special" => TreeEntry::Special,
                    _ => TreeEntry::File(match item.get("base64").and_then(Data::as_str) {
                        Some(encoded) => base64(encoded),
                        None => item["text"].as_str().unwrap().as_bytes().to_vec(),
                    }),
                };
                entries.push((path, entry));
            }
            let got = digest_entries(entries.iter().map(|(path, entry)| {
                (path.split(|byte| *byte == b'/').collect::<Vec<_>>(), entry)
            }));
            let expect = &case["expect"];
            match got {
                Ok(tree) => {
                    let lines: Vec<String> = tree
                        .lines()
                        .into_iter()
                        .map(|line| String::from_utf8(line).unwrap())
                        .collect();
                    let got = json!({"digest": tree.digest.to_string(), "lines": lines});
                    (got == *expect).then_some(()).ok_or(format!("{got}"))
                }
                Err(error) => (expect["error"] == error.kind.class())
                    .then_some(())
                    .ok_or(error.to_string()),
            }
        },
        none,
    );
}

fn trees(case: &Data) -> BTreeMap<String, OverlayTree> {
    case["files"]
        .as_object()
        .map(|files| {
            files
                .iter()
                .map(|(identity, files)| {
                    let tree = match files {
                        Data::Array(paths) => OverlayTree::from_paths(
                            paths.iter().map(|path| path.as_str().unwrap().to_owned()),
                        ),
                        Data::Object(texts) => {
                            OverlayTree::from_texts(texts.iter().map(|(path, text)| {
                                (path.clone(), text.as_str().unwrap().to_owned())
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

fn env_data(env: &Environment) -> Data {
    Data::Object(
        env.values()
            .iter()
            .map(|(name, value)| (name.clone(), Data::String(value.clone())))
            .collect(),
    )
}

fn refusal_data(refusal: &Refusal) -> Data {
    json!({"error": refusal.class, "subject": refusal.subject})
}

#[test]
fn overlay_vectors_compose() {
    run(
        "compose.json",
        |case| {
            let overlays: BTreeMap<String, Document> = case["overlays"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(identity, manifest)| {
                    (identity.clone(), manifest.as_object().unwrap().clone())
                })
                .collect();
            let requested: Vec<String> = case["requested"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| name.as_str().unwrap().to_owned())
                .collect();
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
                let got = json!({
                    "install_order": composition.install_order,
                    "active": composition.active,
                    "missing": composition.missing.iter().map(|(by, name)| json!([by, name])).collect::<Vec<_>>(),
                    "root_variables": composition.root_variables,
                    "routing": composition.routing,
                    "rules": composition.rules.iter().map(|(i, p)| json!([i, p])).collect::<Vec<_>>(),
                    "context": composition.context.iter().map(|(i, p)| json!([i, p])).collect::<Vec<_>>(),
                    "skills": composition.skills,
                    "skill_collisions": composition.skill_collisions,
                });
                return (got == *expect).then_some(()).ok_or(format!("{got}"));
            };
            let os = facts["os"].as_str().unwrap();
            let removed: Vec<String> = facts["removed"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|name| name.as_str().unwrap().to_owned())
                .collect();
            let mut got = serde_json::Map::new();
            match environments(&overlays, &composition.active, &trees, os, &removed) {
                Err(lock) => {
                    got.insert("active".into(), json!(composition.active));
                    got.insert(
                        "env".into(),
                        json!({"error": "partial-lock", "subject": lock}),
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
                    for (key, value) in [
                        ("environments", json!(installed.environments)),
                        ("conflicts", json!(installed.conflicts)),
                        ("blocked", json!(installed.blocked)),
                        ("left", json!(installed.left)),
                    ] {
                        if expect.get(key).is_some() {
                            got.insert(key.into(), value);
                        }
                    }
                    // Packages are given per overlay here; each overlay's own go in below.
                    let mut common = facts.clone();
                    common.as_object_mut().unwrap().remove("packages");
                    let shared: Facts = serde_json::from_value(common).unwrap();
                    let roots = facts["roots"].as_object().unwrap();
                    let per: Vec<(&Document, Facts)> = active
                        .iter()
                        .map(|identity| {
                            let mut own = shared.clone();
                            own.identity = identity.clone();
                            own.root = roots[identity].as_str().unwrap().to_owned();
                            own.tree = trees.get(identity).cloned().unwrap_or_default();
                            own.packages = facts["packages"][identity]
                                .as_object()
                                .map(|packages| {
                                    packages
                                        .iter()
                                        .map(|(runtime, dir)| {
                                            (runtime.clone(), dir.as_str().unwrap().to_owned())
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            (&overlays[identity], own)
                        })
                        .collect();
                    let base =
                        Environment::new(
                            facts["base_env"].as_object().into_iter().flatten().map(
                                |(name, value)| (name.clone(), value.as_str().unwrap().to_owned()),
                            ),
                            os == "windows",
                        );
                    let env = composed_environment(&per, base, &mut Vec::new());
                    got.insert(
                        "env".into(),
                        env.map_or_else(|refusal| refusal_data(&refusal), |env| env_data(&env)),
                    );
                }
            }
            let got = Data::Object(got);
            (got == *expect).then_some(()).ok_or(format!("{got}"))
        },
        none,
    );
}

#[test]
fn overlay_vectors_effective() {
    run(
        "effective.json",
        |case| {
            let manifest = case["manifest"].as_object().unwrap();
            let raw = &case["facts"];
            let mut facts: Facts =
                serde_json::from_value(raw.clone()).map_err(|error| error.to_string())?;
            facts.tree = raw["files"]
                .as_object()
                .map(|files| {
                    OverlayTree::from_texts(
                        files
                            .iter()
                            .map(|(path, text)| (path.clone(), text.as_str().unwrap().to_owned())),
                    )
                })
                .unwrap_or_default();
            let expect = &case["expect"];
            let got = match effective(manifest, &facts) {
                Err(refusal) => refusal_data(&refusal),
                Ok(result) => {
                    let mut got = json!({
                        "ungranted": result.ungranted,
                        "supported": result.supported,
                        "env": env_data(&result.env),
                        "env_scripts": result.env_scripts,
                        "setup": result.setup,
                        "python": result.python,
                        "node": result.node,
                        "dependencies": result.dependencies,
                        "needs": result.needs,
                        "weakenings": result.weakenings.iter().map(|w| json!([w.id, w.subject])).collect::<Vec<_>>(),
                    });
                    if let Some(change) = raw.get("change") {
                        let change: Change = serde_json::from_value(change.clone()).unwrap();
                        let record: Option<SetupRecord> = raw
                            .get("record")
                            .map(|record| serde_json::from_value(record.clone()).unwrap());
                        let prepared = preparation(
                            change,
                            record.as_ref(),
                            raw["digest"].as_str(),
                            result.setup.as_ref(),
                            &result.dependencies,
                            raw["fails"].as_str(),
                        );
                        got["setup_phase"] = json!(prepared.phase);
                        got["setup_arguments"] = json!(prepared.arguments);
                        got["steps"] = json!(prepared.steps);
                        got["record"] = json!(prepared.record);
                        got["activates"] = json!(prepared.activates);
                    }
                    got
                }
            };
            (got == *expect).then_some(()).ok_or(format!("{got}"))
        },
        none,
    );
}

#[test]
fn overlay_vectors_catalog_is_the_store_class() {
    // Catalogs and icons are the Store class, which AI6 does not implement: every case is
    // skipped and counted, so a new vector file is noticed.
    run(
        "catalog.json",
        |_| Ok(()),
        |_| Some("the Store class, which AI6 does not implement"),
    );
    let files: Vec<String> = std::fs::read_dir(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aos-vectors"),
    )
    .unwrap()
    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
    .filter(|name| name.ends_with(".json"))
    .collect();
    let mut files = files;
    files.sort();
    assert_eq!(
        files,
        [
            "catalog.json",
            "compose.json",
            "digest.json",
            "effective.json",
            "names.json",
            "reading.json",
            "source.json",
            "validation.json"
        ],
        "every vector file is run"
    );
}
