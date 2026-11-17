//! `knowledge.json` and `knowledge_search.json`: the tools, their arguments, the files served, the
//! templates rendered, the result envelope, the hint and the checks on a search answer.

use serde_json::{Map, Value, json};

use super::support::decode;
use super::{Report, Runner};
use crate::{
    CallerFacts, Engine, ErrorClass, KnowledgeEntry, KnowledgeError, KnowledgeOverlay,
    KnowledgeSettings, KnowledgeTree, Limit, OverlayTree, Renderers, SearchMode, TemplateFacts,
    kb_tool_result, knowledge_arguments, knowledge_call, knowledge_plan, knowledge_tools,
    search_check, search_order,
};

fn settings_of(case: &Value) -> KnowledgeSettings {
    let mut settings = KnowledgeSettings::default();
    let Some(limits) = case.get("limits").and_then(Value::as_object) else {
        return settings;
    };
    for (key, value) in limits {
        // A null lifts the limit.
        let limit = value.as_u64().map_or(Limit::UNBOUNDED, Limit::at);
        settings = match key.as_str() {
            "max_file_bytes" => settings.max_file_bytes(limit),
            "max_read_bytes" => settings.max_read_bytes(limit),
            "max_rendered_bytes" => settings.max_rendered_bytes(limit),
            "max_query_characters" => settings.max_query_characters(limit),
            "max_top" => settings.max_top(limit),
            "max_context_bytes" => settings.max_context_bytes(limit),
            "max_context_depth" => settings.max_context_depth(limit),
            "max_include_depth" => settings.max_include_depth(limit),
            other => panic!("a limit this runner does not know: {other}"),
        };
    }
    settings
}

/// The engines a case's server renders, and the option values it declares it lacks.
fn engines_of(case: &Value) -> Renderers {
    let engines: Vec<Engine> = match case.get("engines").and_then(Value::as_array) {
        None => Engine::ALL.to_vec(),
        Some(names) => Engine::ALL
            .into_iter()
            .filter(|engine| names.iter().any(|name| name == engine.as_str()))
            .collect(),
    };
    let lacks: Vec<&str> = case
        .get("lacks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    Renderers::none().engines(&engines).lacking(&lacks)
}

fn modes_of(case: &Value) -> Vec<SearchMode> {
    case.get("modes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|name| SearchMode::parse(name.as_str().expect("a mode")).expect("a known mode"))
        .collect()
}

fn text_of(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn overlay_of(raw: &Value) -> KnowledgeOverlay {
    let mut tree = KnowledgeTree::new();
    for item in raw["tree"].as_array().expect("a tree") {
        let path = item["path"].as_str().expect("a path");
        let entry = match item.get("type").and_then(Value::as_str).unwrap_or("file") {
            "symlink" => KnowledgeEntry::Symlink,
            "special" => KnowledgeEntry::Special,
            "dir" => KnowledgeEntry::Directory,
            _ => KnowledgeEntry::File(match item.get("base64").and_then(Value::as_str) {
                Some(encoded) => decode(encoded),
                None => item["text"]
                    .as_str()
                    .unwrap_or_default()
                    .as_bytes()
                    .to_vec(),
            }),
        };
        tree.insert(path, entry);
    }
    let facts = &raw["facts"];
    let mut template = TemplateFacts::new();
    template.root = text_of(facts, "root");
    template.state = text_of(facts, "state");
    template.scope = text_of(facts, "scope");
    template.project = text_of(facts, "project");
    if let Some(options) = facts.get("options").and_then(Value::as_object) {
        template.options = options.clone();
    }
    KnowledgeOverlay::new(
        raw["identity"].as_str().expect("an identity"),
        raw["manifest"].as_object().cloned().unwrap_or_default(),
        tree,
    )
    .served(raw["served"].as_bool().unwrap_or(false))
    .facts(template)
}

fn caller_of(case: &Value) -> CallerFacts {
    let mut caller = CallerFacts::new();
    let Some(facts) = case.get("facts") else {
        return caller;
    };
    caller.role = text_of(&facts["caller"], "role");
    caller.profile = text_of(&facts["caller"], "profile");
    if let Some(env) = facts.get("env").and_then(Value::as_object) {
        for (name, value) in env {
            if let Some(value) = value.as_str() {
                caller = caller.env(name.clone(), value);
            }
        }
    }
    for name in facts
        .get("env_allow")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        caller = caller.allow(name.as_str().expect("a name"));
    }
    caller
}

fn same(got: &Value, expect: &Value) -> Result<(), String> {
    (got == expect)
        .then_some(())
        .ok_or_else(|| format!("got {got}, expected {expect}"))
}

fn envelope(outcome: &Value) -> Result<Value, String> {
    // The vector's outcome is `{"result": ..}` or `{"error": {"class", "message"}}`.
    let handled = match outcome.get("error") {
        Some(error) => Err(KnowledgeError::new(
            ErrorClass::parse(error["class"].as_str().unwrap_or_default())
                .ok_or("an error class")?,
            error["message"].as_str().unwrap_or_default(),
        )),
        None => Ok(outcome["result"].clone()),
    };
    let mcp = kb_tool_result(handled).to_mcp();
    let text: Value = serde_json::from_str(mcp["content"][0]["text"].as_str().unwrap_or(""))
        .map_err(|error| error.to_string())?;
    same(&text, &mcp["structuredContent"])?;
    Ok(json!({"structuredContent": mcp["structuredContent"], "isError": mcp["isError"]}))
}

fn check(case: &Value) -> Result<(), String> {
    let id = case["id"].as_str().expect("an id");
    let settings = settings_of(case);
    let expect = &case["expect"];
    if id.starts_with("tools-") {
        let got = json!({"tools": knowledge_tools(&modes_of(case), &settings)});
        return same(&got, expect);
    }
    if case.get("kind").and_then(Value::as_str) == Some("arguments") {
        let got = knowledge_arguments(
            case["tool"].as_str().expect("a tool"),
            &case["arguments"],
            &modes_of(case),
            &settings,
        );
        return match got {
            Ok(arguments) => same(&json!({"arguments": arguments}), expect),
            Err(error) => same(&json!({"error": error.class.as_str()}), expect),
        };
    }
    if let Some(outcome) = case.get("outcome") {
        return same(&envelope(outcome)?, expect);
    }
    if case.get("manifest").is_some() && case.get("files").is_some() {
        let files = case["files"].as_object().expect("files");
        let tree = OverlayTree::from_texts(
            files
                .iter()
                .map(|(path, text)| (path.clone(), text.as_str().unwrap_or_default())),
        );
        let manifest: Map<String, Value> =
            case["manifest"].as_object().cloned().unwrap_or_default();
        let user = case.get("user").and_then(Value::as_bool);
        let plan = knowledge_plan(&manifest, &tree, user);
        return same(
            &json!({"served": plan.served, "left_out": plan.left_out}),
            expect,
        );
    }
    let overlays: Vec<KnowledgeOverlay> = case["overlays"]
        .as_array()
        .expect("overlays")
        .iter()
        .map(overlay_of)
        .collect();
    let got = knowledge_call(
        case["tool"].as_str().expect("a tool"),
        &overlays,
        &case["arguments"],
        &modes_of(case),
        &engines_of(case),
        &settings,
        &caller_of(case),
    );
    match got {
        Ok(result) => same(&json!({"result": result}), expect),
        Err(error) => same(&json!({"error": error.class.as_str()}), expect),
    }
}

pub(super) fn knowledge(runner: &Runner) -> Report {
    runner.each("knowledge.json", check)
}

fn check_search(case: &Value) -> Result<(), String> {
    let id = case["id"].as_str().expect("an id");
    let expect = &case["expect"];
    if id.starts_with("order-") {
        let passages = case["passages"].as_array().expect("passages");
        return same(&json!({"order": search_order(passages)}), expect);
    }
    let overlays: Vec<KnowledgeOverlay> = case["overlays"]
        .as_array()
        .expect("overlays")
        .iter()
        .map(overlay_of)
        .collect();
    let broken = search_check(
        &overlays,
        &case["arguments"],
        &case["response"],
        &modes_of(case),
        &settings_of(case),
        &caller_of(case),
        &engines_of(case),
    )
    .map_err(|error| error.to_string())?;
    same(&json!({"violations": broken}), expect)
}

pub(super) fn knowledge_search(runner: &Runner) -> Report {
    runner.each("knowledge_search.json", check_search)
}
