//! What the vector tests share: the vector files, their comparison rules, and the JSON shapes
//! the vectors write results in.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use coqui_aos::{
    Dependency, Error, Facts, LeftEnvironment, Needs, PackageConflict, PackageEnvironment, Phase,
    ScriptRun, SetupRecord, SetupRun, Source, SourceKind,
};
use serde_json::{Map, Value, json};

/// The vectors' directory, copied from the specification's `tests/vectors/`.
pub fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aos-vectors")
}

/// The cases of one vector file.
pub fn cases(file: &str) -> Vec<Value> {
    let path = vector_dir().join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let document: Value = serde_json::from_str(&text).expect("a vector file is JSON");
    document["cases"]
        .as_array()
        .expect("a vector file has cases")
        .clone()
}

pub fn decode(encoded: &str) -> Vec<u8> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut bits, mut count, mut out) = (0u32, 0, Vec::new());
    for byte in encoded.bytes().filter(|byte| *byte != b'=') {
        let value = alphabet
            .iter()
            .position(|known| *known == byte)
            .expect("a vector's base64");
        bits = (bits << 6) | u32::try_from(value).expect("six bits");
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push(u8::try_from(bits >> count).expect("a byte"));
            bits &= (1 << count) - 1;
        }
    }
    out
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
pub fn settings_of(case: &Value) -> coqui_aos::Settings {
    let mut settings = coqui_aos::Settings::default();
    let limits = &case["limits"];
    if let Some(bytes) = limits["max_bytes"].as_u64() {
        settings.manifest_bytes = coqui_aos::Limit::at(bytes);
        settings.catalog_bytes = coqui_aos::Limit::at(bytes);
    }
    if let Some(depth) = limits["max_depth"].as_u64() {
        settings.manifest_depth = coqui_aos::Limit::at(depth);
    }
    if let Some(bytes) = limits["max_context_bytes"].as_u64() {
        settings.context_bytes = coqui_aos::Limit::at(bytes);
    }
    settings
}

/// Runs `check` on every case, collecting the failures; `skip` names a case left out with the
/// reason. Prints the counts, and fails when any case fails.
pub fn run(
    file: &str,
    check: impl Fn(&Value) -> Result<(), String>,
    skip: impl Fn(&Value) -> Option<&'static str>,
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

pub fn none(_: &Value) -> Option<&'static str> {
    None
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
    facts.interpreters = raw
        .get("interpreters")
        .filter(|value| value.is_array())
        .map(strings);
    facts.packages = string_map(&raw["packages"]);
    facts
}

pub fn phase_name(phase: Phase) -> &'static str {
    phase.name()
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
        _ => unreachable!("a kind the vectors do not know"),
    };
    json!({"kind": kind, "location": source.location, "ref": source.reference, "path": source.path})
}

pub fn object(value: &Value) -> Map<String, Value> {
    value.as_object().expect("an object").clone()
}

pub fn hook_run(value: &Value) -> coqui_aos::HookRun {
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
    let mut hook = coqui_aos::HookRun::new(value["source"].as_str().unwrap_or_default(), stdout);
    hook.on_failure = match value["on_failure"].as_str() {
        Some("refuse") => Some(coqui_aos::OnFailure::Refuse),
        _ => Some(coqui_aos::OnFailure::Report),
    };
    hook.seconds = value["seconds"].as_u64().unwrap_or(0);
    hook.not_run = value["not_run"].as_bool().unwrap_or(false);
    hook.payload_size = value["payload_size"].as_u64().unwrap_or(0);
    hook.timed_out = value["timed_out"].as_bool().unwrap_or(false);
    hook.exit = i32::try_from(value["exit"].as_i64().unwrap_or(0)).expect("an exit status");
    hook.sets = strings(&value["sets"]);
    hook
}
