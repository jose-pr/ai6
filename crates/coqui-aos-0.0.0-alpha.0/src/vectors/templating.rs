//! `templating.json`: `${NAME}` in a context file and in a bundled skill's body.

use super::support::{bytes_of, expected_errors};
use super::{Report, Runner};
use crate::{Document, Settings, TemplateFacts, context_text, skill_text, template_values};
use serde_json::Value;

fn text_of(facts: &Value, key: &str) -> Option<String> {
    facts.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn facts_of(case: &Value) -> TemplateFacts {
    let facts = &case["facts"];
    let mut out = TemplateFacts::new();
    out.root = text_of(facts, "root");
    out.state = text_of(facts, "state");
    out.name = text_of(facts, "name");
    out.scope = text_of(facts, "scope");
    out.project = text_of(facts, "project");
    if let Some(options) = facts.get("options").and_then(Value::as_object) {
        out.options = options.clone();
    }
    out
}

fn template_case(case: &Value) -> Result<(), String> {
    let manifest: Document = case
        .get("manifest")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let data = bytes_of(case);
    let settings = Settings::default();
    let served = template_values(&manifest, &facts_of(case)).and_then(|values| match case["kind"]
        .as_str()
        .expect("kind")
    {
        "context" => context_text(&data, &values, &settings),
        _ => skill_text(&data, &values, &settings),
    });
    let expect = &case["expect"];
    let classes = expected_errors(expect);
    match served {
        Ok(text) if classes.is_empty() => {
            let want = expect["text"].as_str().expect("expected text");
            (text == want)
                .then_some(())
                .ok_or(format!("text {text:?}, expected {want:?}"))
        }
        Err(error) if classes.iter().any(|class| class == error.class.as_str()) => Ok(()),
        other => Err(format!("{other:?}, expected {expect}")),
    }
}

pub(super) fn templating(runner: &Runner) -> Report {
    runner.each("templating.json", template_case)
}
