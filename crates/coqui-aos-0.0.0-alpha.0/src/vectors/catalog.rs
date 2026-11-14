//! `catalog.json`: catalogs in both forms, several catalogs listed and chosen from, and icons.

use std::collections::BTreeMap;

use super::support::bytes_of;
use super::{Report, Runner};
use crate::{Catalog, CatalogEntry, CatalogSection, list_catalogs, read_catalog, read_icon};
use serde_json::{Value, json};

fn entry_json(entry: &CatalogEntry) -> Value {
    let mut out = json!({"source": super::support::source(&entry.source)});
    if let Some(pin) = &entry.pin {
        out["pin"] = json!(pin);
    }
    if let Some(description) = &entry.description {
        out["description"] = json!(description);
    }
    if let Some(about) = &entry.about {
        out["about"] = Value::Object(about.clone());
    }
    out
}

fn section_json(section: &CatalogSection) -> Value {
    let entries: BTreeMap<&String, Value> = section
        .entries
        .iter()
        .map(|(name, entry)| (name, entry_json(entry)))
        .collect();
    let refused: Vec<Value> = section
        .refused
        .iter()
        .map(|(name, why)| json!([name, why.as_str()]))
        .collect();
    json!({"entries": entries, "refused": refused})
}

fn catalog_json(catalog: &Catalog) -> Value {
    let mut out = serde_json::Map::new();
    out.insert(
        "form".into(),
        json!(match catalog.form {
            crate::CatalogForm::Minimal => "minimal",
            _ => "full",
        }),
    );
    for (key, value) in [
        ("name", &catalog.name),
        ("description", &catalog.description),
        ("homepage", &catalog.homepage),
    ] {
        if let Some(value) = value {
            out.insert(key.into(), json!(value));
        }
    }
    let overlays = section_json(&catalog.overlays);
    out.insert("entries".into(), overlays["entries"].clone());
    out.insert("refused".into(), overlays["refused"].clone());
    if let Some(skills) = &catalog.skills {
        out.insert("skills".into(), section_json(skills));
    }
    out.insert("ignored".into(), json!(catalog.ignored));
    Value::Object(out)
}

fn read(item: &Value, settings: &crate::Settings) -> Result<Catalog, crate::Error> {
    read_catalog(
        item["text"].as_str().expect("text").as_bytes(),
        item["name"].as_str().expect("name"),
        item["base"].as_str(),
        settings,
    )
}

fn catalogs_case(case: &Value) -> Result<(), String> {
    let settings = crate::Settings::default();
    let items = case["catalogs"].as_array().expect("catalogs");
    let read_all: Vec<(String, Catalog)> = items
        .iter()
        .map(|item| {
            (
                item["label"].as_str().expect("label").to_owned(),
                read(item, &settings).expect("a catalog"),
            )
        })
        .collect();
    let labelled: Vec<(&str, &Catalog)> = read_all
        .iter()
        .map(|(label, catalog)| (label.as_str(), catalog))
        .collect();
    let expect = &case["expect"];
    let listing = match list_catalogs(&labelled) {
        Ok(listing) => listing,
        Err(error) => {
            return (expect["error"] == error.class.as_str())
                .then_some(())
                .ok_or(format!("refused {error}, expected {expect}"));
        }
    };
    let entries: Vec<Value> = listing
        .entries
        .iter()
        .map(|entry| {
            json!({"catalog": entry.catalog, "name": entry.name,
                   "identity": entry.identity, "shown": entry.shown})
        })
        .collect();
    let mut got = json!({"entries": entries, "collisions": listing.collisions});
    if let Some(requests) = case["choose"].as_array().filter(|list| !list.is_empty()) {
        let mut chosen = serde_json::Map::new();
        for request in requests {
            let request = request.as_str().expect("a name");
            chosen.insert(
                request.to_owned(),
                match listing.choose(request) {
                    Ok(entry) => json!({"catalog": entry.catalog, "name": entry.name}),
                    Err(error) => json!({"error": error.class.as_str()}),
                },
            );
        }
        got["chosen"] = Value::Object(chosen);
    }
    (got == *expect).then_some(()).ok_or(format!("{got}"))
}

fn icon_case(case: &Value) -> Result<(), String> {
    let icon = &case["icon"];
    let mut settings = crate::Settings::default();
    let limit = |key: &str| icon["limits"][key].as_u64().map(crate::Limit::at);
    if let Some(bytes) = limit("max_bytes") {
        settings = settings.icon_bytes(bytes);
    }
    if let Some(width) = limit("max_width") {
        settings = settings.icon_width(width);
    }
    if let Some(height) = limit("max_height") {
        settings = settings.icon_height(height);
    }
    let data = bytes_of(icon);
    let expect = &case["expect"];
    match read_icon(&data, icon["name"].as_str().expect("name"), &settings) {
        Ok(read) => {
            let got =
                json!({"format": read.format.as_str(), "width": read.width, "height": read.height});
            (got == *expect).then_some(()).ok_or(format!("{got}"))
        }
        Err(error) => (expect["error"] == error.class.as_str())
            .then_some(())
            .ok_or(format!("refused {error}, expected {expect}")),
    }
}

pub(super) fn catalogs_and_icons(runner: &Runner) -> Report {
    runner.each("catalog.json", |case| {
        if case.get("icon").is_some() {
            return icon_case(case);
        }
        if case.get("catalogs").is_some() {
            return catalogs_case(case);
        }
        let item = &case["catalog"];
        let expect = &case["expect"];
        match read(item, &crate::Settings::default()) {
            Ok(catalog) => {
                let got = catalog_json(&catalog);
                (got == *expect).then_some(()).ok_or(format!("{got}"))
            }
            Err(error) => (expect["error"] == error.class.as_str())
                .then_some(())
                .ok_or(format!("refused {error}, expected {expect}")),
        }
    })
}
