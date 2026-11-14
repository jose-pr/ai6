//! What the vector runners share: their comparison rules and the JSON shapes the vectors write
//! results in.

use std::collections::BTreeMap;

use crate::{
    Dependency, Error, Facts, LeftEnvironment, Needs, PackageConflict, PackageEnvironment,
    ScriptRun, SetupRecord, SetupRun, Source, SourceKind,
};
use base64::Engine as _;
use serde_json::{Map, Value, json};

pub fn decode(encoded: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("a vector's base64")
}

/// A case's document bytes: `base64`, `text`, or `parts` concatenated.
pub fn bytes_of(case: &Value) -> Vec<u8> {
    if let Some(encoded) = case.get("base64").and_then(Value::as_str) {
        return decode(encoded);
    }
    if let Some(parts) = case.get("parts").and_then(Value::as_array) {
        let mut bytes = Vec::new();
        for part in parts {
            bytes.extend(part_bytes(part));
        }
        return bytes;
    }
    case["text"].as_str().expect("text").as_bytes().to_vec()
}

fn part_bytes(part: &Value) -> Vec<u8> {
    let (text, times) = match part {
        Value::Array(pair) => (
            pair[0].as_str().expect("a part's text"),
            pair[1].as_u64().expect("a part's count"),
        ),
        other => (other.as_str().expect("a part is text"), 1),
    };
    text.as_bytes()
        .repeat(usize::try_from(times).expect("count"))
}

/// The settings a case's `limits` write.
pub fn settings_of(case: &Value) -> crate::Settings {
    let mut settings = crate::Settings::default();
    let limits = &case["limits"];
    if let Some(bytes) = limits["max_bytes"].as_u64() {
        settings.manifest_bytes = crate::Limit::at(bytes);
        settings.catalog_bytes = crate::Limit::at(bytes);
    }
    if let Some(depth) = limits["max_depth"].as_u64() {
        settings.manifest_depth = crate::Limit::at(depth);
    }
    if let Some(bytes) = limits["max_context_bytes"].as_u64() {
        settings.context_bytes = crate::Limit::at(bytes);
    }
    settings
}

pub fn expected_errors(expect: &Value) -> Vec<String> {
    match &expect["error"] {
        Value::String(class) => vec![class.clone()],
        Value::Array(classes) => classes
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// Compares a result with a case's `expect`: its `data`, or one of its error classes.
pub fn compare(got: Result<Value, Error>, expect: &Value) -> Result<(), String> {
    let errors = expected_errors(expect);
    match got {
        Ok(data) if expect.get("data").is_some() => (data == expect["data"])
            .then_some(())
            .ok_or(format!("read {data}, expected {}", expect["data"])),
        Ok(data) => Err(format!("read {data}, expected an error {errors:?}")),
        Err(error) if errors.iter().any(|class| class == error.class.as_str()) => Ok(()),
        Err(error) => Err(format!("refused {error}, expected {expect}")),
    }
}

pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| item.as_str().expect("a string").to_owned())
        .collect()
}

fn string_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, item)| Some((key.clone(), item.as_str()?.to_owned())))
        .collect()
}

/// The consumer's facts a case writes.
pub fn facts_of(raw: &Value) -> Facts {
    let mut facts = Facts::default();
    let text = |key: &str| raw.get(key).and_then(Value::as_str).map(str::to_owned);
    facts.os = text("os").unwrap_or_default();
    facts.root = text("root").unwrap_or_default();
    facts.identity = text("identity").unwrap_or_default();
    facts.state = text("state");
    facts.pinned = raw["pinned"].as_bool().unwrap_or(false);
    facts.base_env = string_map(&raw["base_env"]);
    facts.host_env = string_map(&raw["host_env"]);
    facts.grants = raw["grants"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(provider, keys)| (provider.clone(), strings(keys)))
        .collect();
    facts.secrets = string_map(&raw["secrets"]);
    facts.secret_files = raw
        .get("secret_files")
        .filter(|files| files.is_object())
        .map(string_map);
    facts.providers = raw.get("providers").filter(|p| p.is_array()).map(strings);
    facts.provider_values = raw["provider_values"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(provider, values)| (provider.clone(), string_map(values)))
        .collect();
    facts.enforces_network = raw["enforces_network"].as_bool();
    facts.env_scripts_enabled = raw["env_scripts_enabled"].as_bool();
    facts.gate_env_scripts = raw.get("allowed").is_some();
    facts.digest = text("digest");
    facts.allowed = text("allowed");
    facts.interpreters = raw
        .get("interpreters")
        .filter(|value| value.is_array())
        .map(strings);
    facts.packages = string_map(&raw["packages"]);
    facts
}

pub fn script_run(run: &ScriptRun) -> Value {
    json!({"interpreter": run.interpreter, "path": run.path, "args": run.args, "timeout": run.timeout})
}

pub fn setup_run(run: &SetupRun) -> Value {
    json!({"interpreter": run.interpreter, "path": run.path, "args": run.args,
           "timeout": run.timeout, "network": run.network})
}

pub fn needs(needs: &Needs) -> Value {
    json!({"interpreters": needs.interpreters, "commands": needs.commands})
}

pub fn dependency(dependency: &Dependency) -> Value {
    json!({"runtime": dependency.runtime.as_str(), "file": dependency.file, "lock": dependency.lock,
           "environment": dependency.environment})
}

pub fn package_environment(environment: &PackageEnvironment) -> Value {
    json!({"runtime": environment.runtime.as_str(), "environment": environment.environment,
           "overlays": environment.overlays, "files": environment.files,
           "locked": environment.locked})
}

pub fn package_conflict(conflict: &PackageConflict) -> Value {
    json!({"runtime": conflict.runtime.as_str(), "environment": conflict.environment,
           "package": conflict.package, "overlays": conflict.overlays})
}

pub fn left_environment(left: &LeftEnvironment) -> Value {
    json!({"overlay": left.overlay, "runtime": left.runtime.as_str(), "environment": left.environment,
           "then": left.then.as_str()})
}

pub fn setup_record(record: &SetupRecord) -> Value {
    json!({"digest": record.digest, "phase": record.phase.name()})
}

pub fn source(source: &Source) -> Value {
    let kind = match source.kind {
        SourceKind::Git => "git",
        SourceKind::Url => "url",
        SourceKind::Directory => "directory",
    };
    json!({"kind": kind, "location": source.location, "ref": source.reference, "path": source.path})
}

pub fn object(value: &Value) -> Map<String, Value> {
    value.as_object().expect("an object").clone()
}

pub fn hook_run(value: &Value) -> crate::HookRun {
    let stdout = if let Some(encoded) = value.get("stdout_base64").and_then(Value::as_str) {
        decode(encoded)
    } else if let Some(parts) = value.get("stdout_parts").and_then(Value::as_array) {
        parts
            .iter()
            .flat_map(|part| {
                let text = part[0].as_str().expect("text");
                text.as_bytes()
                    .repeat(usize::try_from(part[1].as_u64().expect("times")).expect("times"))
            })
            .collect()
    } else {
        value["stdout"]
            .as_str()
            .unwrap_or_default()
            .as_bytes()
            .to_vec()
    };
    let mut hook = crate::HookRun::new(value["source"].as_str().unwrap_or_default(), stdout);
    hook.on_failure = match value["on_failure"].as_str() {
        Some("refuse") => Some(crate::OnFailure::Refuse),
        _ => Some(crate::OnFailure::Report),
    };
    hook.seconds = value["seconds"].as_u64().unwrap_or(0);
    hook.not_run = value["not_run"].as_bool().unwrap_or(false);
    hook.payload_size = value["payload_size"].as_u64().unwrap_or(0);
    hook.timed_out = value["timed_out"].as_bool().unwrap_or(false);
    hook.exit = i32::try_from(value["exit"].as_i64().unwrap_or(0)).expect("an exit status");
    hook.sets = strings(&value["sets"]);
    hook.select = value.get("match").map(strings);
    hook
}
