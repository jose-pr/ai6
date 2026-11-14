//! `skills.json`: where the skills of the active overlays come from, and the check of a skill that
//! was fetched; and the caller-supplied source a plan is fetched through.

use std::collections::BTreeMap;

use super::support::{object, strings};
use super::{Report, Runner};
use crate::{
    Catalog, Document, OverlayTree, Settings, read_catalog, read_fetched_skill, resolve_skills,
};
use serde_json::{Value, json};

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

fn pairs(list: &[(String, String)]) -> Vec<Value> {
    list.iter().map(|(a, b)| json!([a, b])).collect()
}

fn resolve_case(case: &Value) -> Result<(), String> {
    let settings = Settings::default();
    let overlays: BTreeMap<String, Document> = case["overlays"]
        .as_object()
        .expect("overlays")
        .iter()
        .map(|(identity, manifest)| (identity.clone(), object(manifest)))
        .collect();
    let read: Vec<(String, Catalog)> = case["catalogs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            (
                item["label"].as_str().expect("label").to_owned(),
                read_catalog(
                    item["text"].as_str().expect("text").as_bytes(),
                    item["name"].as_str().expect("name"),
                    item["base"].as_str(),
                    &settings,
                )
                .expect("a catalog"),
            )
        })
        .collect();
    let catalogs: Vec<(&str, &Catalog)> = read.iter().map(|(l, c)| (l.as_str(), c)).collect();
    let order = strings(&case["order"]);
    let order: Vec<&str> = order.iter().map(String::as_str).collect();
    let fetches = case["fetches"].as_bool().unwrap_or(true);
    let plan = resolve_skills(
        &overlays,
        &strings(&case["requested"]),
        &trees(case),
        &catalogs,
        &order,
        fetches,
        &settings,
    )
    .map_err(|error| error.to_string())?;
    let skills: Vec<Value> = plan
        .skills
        .iter()
        .map(|skill| {
            let mut out = json!({"name": skill.name, "origin": skill.origin.as_str(),
                                 "overlay": skill.overlay});
            for (key, value) in [
                ("source", &skill.source),
                ("pin", &skill.pin),
                ("catalog", &skill.catalog),
            ] {
                if let Some(value) = value {
                    out[key] = json!(value);
                }
            }
            if let Some(labels) = &skill.catalogs {
                out["catalogs"] = json!(labels);
            }
            out
        })
        .collect();
    let got = json!({
        "skills": skills,
        "missing": pairs(&plan.missing),
        "ambiguous": plan.ambiguous.iter().map(|(a, b, c)| json!([a, b, c])).collect::<Vec<_>>(),
        "unpinned": pairs(&plan.unpinned),
        "non_defaults": plan.non_defaults.iter().map(|(a, b, c)| json!([a, b, c])).collect::<Vec<_>>(),
        "not_fetched": pairs(&plan.not_fetched),
        "name_mismatch": pairs(&plan.name_mismatch),
        "shadowing": pairs(&plan.shadowing),
        "delivered": plan.delivered,
        "collisions": plan.collisions,
    });
    (got == case["expect"])
        .then_some(())
        .ok_or(format!("{got}"))
}

fn fetched_case(case: &Value) -> Result<(), String> {
    let files: Vec<(String, Vec<u8>)> = case["fetched"]
        .as_object()
        .expect("fetched")
        .iter()
        .map(|(path, content)| {
            let bytes = match content.get("base64").and_then(Value::as_str) {
                Some(encoded) => super::support::decode(encoded),
                None => content.as_str().expect("text").as_bytes().to_vec(),
            };
            (path.clone(), bytes)
        })
        .collect();
    let expect = &case["expect"];
    match read_fetched_skill(
        case["name"].as_str().expect("name"),
        &files,
        case["pin"].as_str(),
        &super::support::settings_of(case),
    ) {
        Ok(read) => {
            let got = json!({"digest": read.digest.to_string(), "reported": read.reported, "files": read.files});
            (got == *expect).then_some(()).ok_or(format!("{got}"))
        }
        Err(error) => (super::support::expected_errors(expect)
            .iter()
            .any(|class| class == error.class.as_str()))
        .then_some(())
        .ok_or(format!("refused {error}, expected {expect}")),
    }
}

pub(super) fn skills_from_elsewhere(runner: &Runner) -> Report {
    runner.each("skills.json", |case| {
        if case.get("fetched").is_some() {
            fetched_case(case)
        } else {
            resolve_case(case)
        }
    })
}
