//! `front_matter.json`: the split, the reading of metadata, a context file's text and an
//! overlay's two manifests, context and problems.

use super::support::{bytes_of, settings_of};
use super::{Report, Runner};
use crate::{
    Case, OverlayTree, TreeEntry, applies, context_file_text, digest_entries, read_front_matter,
    read_overlay, split_front_matter, validate,
};
use serde_json::Value;

fn case_name(case: Case) -> &'static str {
    match case {
        Case::None => "none",
        Case::Malformed => "malformed",
        Case::Unterminated => "unterminated",
        Case::TooLarge => "too-large",
        Case::Empty => "empty",
        Case::UnknownFormat => "unknown-format",
        Case::DoesNotParse => "does-not-parse",
        Case::NotAMapping => "not-a-mapping",
        Case::Mapping => "mapping",
    }
}

fn file_bytes(value: &Value) -> Vec<u8> {
    match value.get("base64").and_then(Value::as_str) {
        Some(encoded) => super::support::decode(encoded),
        None => value.as_str().expect("a file is text").as_bytes().to_vec(),
    }
}

fn check_bytes(
    got: &[u8],
    expect: &Value,
    key: &str,
    base64_key: &str,
    size_key: &str,
) -> Result<(), String> {
    if let Some(text) = expect.get(key).and_then(Value::as_str)
        && got != text.as_bytes()
    {
        return Err(format!(
            "{key} {:?}, expected {text:?}",
            String::from_utf8_lossy(got)
        ));
    }
    if let Some(encoded) = expect.get(base64_key).and_then(Value::as_str)
        && got != super::support::decode(encoded)
    {
        return Err(format!("{base64_key} differs"));
    }
    if let Some(size) = expect.get(size_key).and_then(Value::as_u64)
        && got.len() as u64 != size
    {
        return Err(format!("{size_key} {}, expected {size}", got.len()));
    }
    Ok(())
}

fn document_case(case: &Value) -> Result<(), String> {
    let data = bytes_of(case);
    let settings = settings_of(case);
    let expect = &case["expect"];
    let read = read_front_matter(&data, &settings);
    let split = split_front_matter(&data, &settings);
    let name = case_name(read.case);
    if expect["case"] != name {
        return Err(format!("case {name}, expected {}", expect["case"]));
    }
    for (key, got) in [
        ("body_offset", split.body_offset),
        ("body_line", split.body_line),
        ("metadata_offset", split.metadata_offset),
        ("metadata_line", split.metadata_line),
    ] {
        if let Some(want) = expect.get(key).and_then(Value::as_u64)
            && got != Some(usize::try_from(want).expect("offset"))
        {
            return Err(format!("{key} {got:?}, expected {want}"));
        }
    }
    check_bytes(
        split.metadata.unwrap_or_default(),
        expect,
        "metadata",
        "metadata_base64",
        "metadata_size",
    )?;
    check_bytes(
        split.body(&data),
        expect,
        "body",
        "body_base64",
        "body_size",
    )?;
    if let Some(format) = expect.get("format").and_then(Value::as_str)
        && read.format.map(crate::Format::as_str) != Some(format)
    {
        return Err(format!("format {:?}, expected {format}", read.format));
    }
    if let Some(want) = expect.get("data") {
        let got = read.data.clone().map(Value::Object);
        if got.as_ref() != Some(want) {
            return Err(format!("data {got:?}, expected {want}"));
        }
    }
    let classes = super::support::expected_errors(expect);
    match (classes.is_empty(), &read.error) {
        (false, Some(error)) if classes.iter().any(|class| class == error.class.as_str()) => {
            if let Some(line) = expect.get("line").and_then(Value::as_u64)
                && error.line != Some(usize::try_from(line).expect("line"))
            {
                return Err(format!("line {:?}, expected {line}", error.line));
            }
        }
        (true, None) => {}
        (_, got) => return Err(format!("error {got:?}, expected {classes:?}")),
    }
    Ok(())
}

fn files_case(case: &Value) -> Result<(), String> {
    let files = case["files"].as_object().expect("files");
    let mut tree = OverlayTree::default();
    for (path, content) in files {
        tree = tree.with_bytes(path, file_bytes(content));
    }
    let settings = settings_of(case);
    let expect = &case["expect"];
    let read = match read_overlay(&tree, &settings) {
        Ok(read) => read,
        Err(error) => {
            let Some(class) = expect.get("error").and_then(Value::as_str) else {
                return Err(format!("refused {error}"));
            };
            if error.class.as_str() != class {
                return Err(format!("refused {error}, expected {class}"));
            }
            if let Some(line) = expect.get("line").and_then(Value::as_u64)
                && error.line != Some(usize::try_from(line).expect("line"))
            {
                return Err(format!("line {:?}, expected {line}", error.line));
            }
            return Ok(());
        }
    };
    if expect.get("error").is_some() {
        return Err("read, expected an error".to_owned());
    }
    let got = |key: &str| -> Value {
        match key {
            "manifest_file" => read
                .manifest_file
                .map_or(Value::Null, |file| Value::from(file.as_str())),
            "manifest" => Value::Object(read.manifest.clone()),
            "context" => Value::from(read.context.clone()),
            "context_ignored" => Value::from(read.context_ignored.clone()),
            "issues" => Value::from(read.issues.clone()),
            "harness_context" => serde_json::to_value(read.harness_context.clone()).expect("map"),
            "applies" => Value::Bool(applies(&validate(&read.manifest, Some(&tree)))),
            "digest" => {
                let entries: Vec<(Vec<u8>, TreeEntry)> = files
                    .iter()
                    .map(|(path, content)| {
                        (
                            path.as_bytes().to_vec(),
                            TreeEntry::File(file_bytes(content)),
                        )
                    })
                    .collect();
                let digest = digest_entries(entries.iter().map(|(path, entry)| {
                    (path.split(|byte| *byte == b'/').collect::<Vec<_>>(), entry)
                }))
                .expect("a tree of files");
                Value::String(digest.digest.to_string())
            }
            other => panic!("unknown key {other}"),
        }
    };
    for (key, want) in expect.as_object().expect("expect") {
        let value = got(key);
        let value = if want.is_object() || want.is_array() || value != Value::Null {
            value
        } else {
            Value::Null
        };
        if &value != want {
            return Err(format!("{key} {value}, expected {want}"));
        }
    }
    Ok(())
}

pub(super) fn front_matter(runner: &Runner) -> Report {
    runner.each("front_matter.json", |case| {
        if let Some(text) = case.get("context_file").and_then(Value::as_str) {
            let settings = settings_of(case);
            let expect = &case["expect"];
            return match context_file_text(text.as_bytes(), &settings) {
                Ok(got) => (expect
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::as_bytes)
                    == Some(got))
                .then_some(())
                .ok_or(format!("text {:?}", String::from_utf8_lossy(got))),
                Err(error) => (expect["error"] == error.class.as_str())
                    .then_some(())
                    .ok_or(format!("refused {error}")),
            };
        }
        if case.get("files").is_some() {
            return files_case(case);
        }
        document_case(case)
    })
}
