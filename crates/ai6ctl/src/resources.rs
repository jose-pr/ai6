//! `ai6ctl get|apply|edit|set|unset|delete|api-resources`: the records of the configuration
//! provider, managed through the gateway under the person's actions. A kind is a family's name
//! (`watch`, `agent`, `theater`), or its plural.

use std::collections::BTreeSet;
use std::io::{IsTerminal as _, Read as _, Write as _};
use std::path::{Path, PathBuf};

use ai6::bootstrap::format::{EXTENSIONS, Format};
use ai6::directorates::m::config::{document_tables, family_of_kind, parse_document};
use clap::{Args, ValueEnum};
use reqwest::{Client, Method, StatusCode, Url};
use serde_json::{Map, Value, json};

use crate::access::url;
use crate::plain;

/// How `get` and `edit` write a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Output {
    /// The `.cfg` format, which is TOML unless its first line names another.
    Cfg,
    /// TOML.
    Toml,
    /// JSON.
    Json,
    /// YAML.
    Yaml,
}

impl Output {
    fn format(self) -> Format {
        match self {
            Self::Cfg | Self::Toml => Format::Toml,
            Self::Json => Format::Json,
            Self::Yaml => Format::Yaml,
        }
    }
}

/// Which format a document is read as when `--format` names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Input {
    /// TOML.
    Toml,
    /// JSON.
    Json,
    /// YAML.
    Yaml,
}

impl Input {
    fn format(self) -> Format {
        match self {
            Self::Toml => Format::Toml,
            Self::Json => Format::Json,
            Self::Yaml => Format::Yaml,
        }
    }
}

/// Where a command acts.
#[derive(Debug, Args)]
pub(crate) struct Scope {
    /// Act on this Station's own records, for the kinds that have them (default: the global
    /// records every Station reads).
    #[arg(long, value_name = "ID")]
    station: Option<String>,
}

/// `ai6ctl get KIND [KEY] [FIELD]`.
#[derive(Debug, Args)]
pub(crate) struct Get {
    /// The kind of record: watch, agent, operation, ...; `ai6ctl api-resources` lists them.
    kind: String,
    /// One record's key; a kind with a single record is keyed `settings`.
    key: Option<String>,
    /// A field of that record, dotted for a nested table (`green_ink.kinds`).
    field: Option<String>,
    /// How to write what is printed.
    #[arg(short = 'o', long, value_enum, default_value = "cfg")]
    output: Output,
    /// Print each record's key and revision instead of its fields.
    #[arg(long)]
    revisions: bool,
    #[command(flatten)]
    scope: Scope,
}

/// `ai6ctl apply -f FILE|DIR|-`.
#[derive(Debug, Args)]
pub(crate) struct Apply {
    /// A document, or a directory of them (but `ai6.cfg`, the bootstrap), or `-` for standard
    /// input read as a `.cfg` is. Each holds the tables of any kinds: `[watch.KEY]`,
    /// `[agent.KEY]`, `[assistant]`, ...
    #[arg(
        short = 'f',
        long = "filename",
        value_name = "FILE|DIR|-",
        required = true
    )]
    files: Vec<String>,
    /// Read every document as this format whatever its name or first line says.
    #[arg(long, value_enum)]
    format: Option<Input>,
    /// Check the documents against the stored records and say what would change; store nothing.
    /// There is no `--if-revision`: the records are stored together, not against one revision.
    #[arg(long)]
    dry_run: bool,
    /// Store though the records loosen what is in force, without asking.
    #[arg(short = 'y', long)]
    yes: bool,
    #[command(flatten)]
    scope: Scope,
}

/// `ai6ctl edit KIND KEY`.
#[derive(Debug, Args)]
pub(crate) struct Edit {
    /// The kind of record.
    kind: String,
    /// The record's key.
    key: String,
    /// How the record is written for the editor.
    #[arg(short = 'o', long, value_enum, default_value = "cfg")]
    output: Output,
    /// Refuse the save unless the record is at this revision (default: the one read).
    #[arg(long, value_name = "REVISION")]
    if_revision: Option<String>,
    /// Store though the record loosens what is in force, without asking.
    #[arg(short = 'y', long)]
    yes: bool,
    #[command(flatten)]
    scope: Scope,
}

/// `ai6ctl set KIND KEY field=value ...`.
#[derive(Debug, Args)]
pub(crate) struct Set {
    /// The kind of record.
    kind: String,
    /// The record's key; it is made when there is none.
    key: String,
    /// `field=value` pairs; the field is dotted for a nested table and the value is read as the
    /// field's type, a list as `a,b` or JSON.
    #[arg(required = true, value_name = "FIELD=VALUE")]
    assignments: Vec<String>,
    /// Refuse the change unless the record is at this revision (default: the one read).
    #[arg(long, value_name = "REVISION")]
    if_revision: Option<String>,
    /// Store though the record loosens what is in force, without asking.
    #[arg(short = 'y', long)]
    yes: bool,
    #[command(flatten)]
    scope: Scope,
}

/// `ai6ctl unset KIND KEY FIELD`.
#[derive(Debug, Args)]
pub(crate) struct Unset {
    /// The kind of record.
    kind: String,
    /// The record's key.
    key: String,
    /// The field to remove, dotted for a nested table; it then has its default.
    field: String,
    /// Refuse the change unless the record is at this revision (default: the one read).
    #[arg(long, value_name = "REVISION")]
    if_revision: Option<String>,
    /// Store though the record loosens what is in force, without asking.
    #[arg(short = 'y', long)]
    yes: bool,
    #[command(flatten)]
    scope: Scope,
}

/// `ai6ctl delete KIND KEY`.
#[derive(Debug, Args)]
pub(crate) struct Delete {
    /// The kind of record.
    kind: String,
    /// The record's key.
    key: String,
    /// Delete it though other records name it.
    #[arg(long)]
    force: bool,
    /// Refuse the deletion unless the record is at this revision.
    #[arg(long, value_name = "REVISION")]
    if_revision: Option<String>,
    #[command(flatten)]
    scope: Scope,
}

/// The gateway, as these commands reach it.
pub(crate) struct Gateway<'a> {
    client: &'a Client,
    server: &'a Url,
    token: &'a str,
}

/// A stored record, as the gateway lists it.
struct Stored {
    definition: Value,
    revision: String,
}

impl<'a> Gateway<'a> {
    pub(crate) fn new(client: &'a Client, server: &'a Url, token: &'a str) -> Self {
        Self {
            client,
            server,
            token,
        }
    }

    /// One request. A refusal comes back as the gateway's reason; `changed` and `refused` say
    /// what they are.
    async fn send(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, &str)],
        revision: Option<&str>,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let mut address = url(self.server, segments)?;
        if !query.is_empty() {
            address.query_pairs_mut().extend_pairs(query);
        }
        let mut request = self.client.request(method, address).bearer_auth(self.token);
        if let Some(revision) = revision {
            request = request.header("if-match", format!("\"{revision}\""));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "could not contact Universal Exports".to_owned())?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(value);
        }
        if let Some(reason) = value.get("refused").and_then(Value::as_str) {
            return Err(format!("refused: {}", plain(reason)));
        }
        if status == StatusCode::CONFLICT
            && let Some(now) = value.get("changed").and_then(Value::as_str)
        {
            return Err(format!(
                "the record changed meanwhile and is at revision {}",
                plain(now)
            ));
        }
        if status == StatusCode::CONFLICT {
            return Err("the configuration provider takes no changes".to_owned());
        }
        // A refusal is the gateway's reason as text, or as an `error` field.
        let reason = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(body.trim());
        let reason = if reason.is_empty() {
            String::new()
        } else {
            format!(": {}", plain(reason))
        };
        Err(format!("Universal Exports returned HTTP {status}{reason}"))
    }

    /// The records of `kind`, by key.
    async fn list(
        &self,
        kind: &str,
        station: Option<&str>,
    ) -> Result<Vec<(String, Stored)>, String> {
        let query: Vec<(&str, &str)> = station.map(|id| ("station", id)).into_iter().collect();
        let listed = self
            .send(Method::GET, &["roster", kind], &query, None, None)
            .await?;
        let mut records = Vec::new();
        for record in listed["records"].as_array().into_iter().flatten() {
            records.push((
                record["key"].as_str().unwrap_or_default().to_owned(),
                Stored {
                    definition: record["definition"].clone(),
                    revision: record["revision"].as_str().unwrap_or("0").to_owned(),
                },
            ));
        }
        Ok(records)
    }

    /// The record's JSON Schema, `null` for a kind that has no form yet.
    async fn schema(&self, kind: &str) -> Result<Value, String> {
        match self
            .send(Method::GET, &["roster", kind, "schema"], &[], None, None)
            .await
        {
            Ok(form) => Ok(form["schema"].clone()),
            Err(reason) if reason.contains("HTTP 404") => Ok(Value::Null),
            Err(reason) => Err(reason),
        }
    }

    /// Stores one record over the one read (`revision`, `"0"`: none), and prints the new
    /// revision and what the record loosens.
    async fn put(
        &self,
        (kind, key): (&str, &str),
        station: Option<&str>,
        revision: &str,
        definition: Value,
        yes: bool,
    ) -> Result<(), String> {
        let mut query: Vec<(&str, &str)> = station.map(|id| ("station", id)).into_iter().collect();
        if yes {
            query.push(("confirm", "true"));
        }
        let mut changed = self
            .send(
                Method::PUT,
                &["roster", kind, key],
                &query,
                Some(revision),
                Some(definition.clone()),
            )
            .await?;
        // A record that loosens what is in force waits for a person to say so.
        if changed["applied"] == false {
            let weakened = changed["weakened"].as_array().cloned().unwrap_or_default();
            if !confirmed(&weakened)? {
                return Err("nothing stored".to_owned());
            }
            query.push(("confirm", "true"));
            changed = self
                .send(
                    Method::PUT,
                    &["roster", kind, key],
                    &query,
                    Some(revision),
                    Some(definition),
                )
                .await?;
        }
        println!("{}/{} configured", plain(kind), plain(key));
        print_change(&changed);
        Ok(())
    }
}

fn print_change(change: &Value) {
    if let Some(revision) = change["revision"].as_str() {
        println!("revision\t{}", plain(revision));
    }
    for weakening in change["weakened"].as_array().into_iter().flatten() {
        println!(
            "weakening\t{}",
            plain(weakening.as_str().unwrap_or_default())
        );
    }
}

fn station_of(scope: &Scope) -> Option<&str> {
    scope.station.as_deref()
}

/// `value` written as `format`, a table for TOML.
fn written(format: Format, value: Map<String, Value>) -> Result<String, String> {
    ai6::bootstrap::format::to_string(format, &Value::Object(value))
        .map_err(|error| error.to_string())
}

/// The record `key` of `kind` as a document the other commands read: `[kind.key]`.
fn document(
    kind: &str,
    records: impl IntoIterator<Item = (String, Value)>,
    format: Format,
) -> Result<String, String> {
    let record = family_of_kind(kind).ok_or_else(|| format!("{kind:?} is not a kind of record"))?;
    written(format, document_tables(record, records))
}

/// A value at a dotted path of a record.
fn at<'a>(definition: &'a Value, field: &str) -> Option<&'a Value> {
    field
        .split('.')
        .try_fold(definition, |value, segment| value.get(segment))
}

pub(crate) async fn get(gateway: &Gateway<'_>, command: Get) -> Result<(), String> {
    let mut records = gateway
        .list(&command.kind, station_of(&command.scope))
        .await?;
    let single = command.key.is_some();
    if let Some(key) = &command.key {
        records.retain(|(found, _)| found == key);
        if records.is_empty() {
            return Err(format!("no {} record {}", plain(&command.kind), plain(key)));
        }
    }
    if command.revisions {
        for (key, stored) in &records {
            println!("{}\t{}", plain(key), plain(&stored.revision));
        }
        return Ok(());
    }
    if let (Some(field), Some((_, stored))) = (&command.field, records.first()) {
        let value = at(&stored.definition, field)
            .ok_or_else(|| format!("{} has no field {}", plain(&command.kind), plain(field)))?;
        match value {
            Value::String(text) => println!("{}", plain(text)),
            Value::Bool(_) | Value::Number(_) => println!("{value}"),
            other => {
                let name = field.rsplit('.').next().unwrap_or(field).to_owned();
                let mut table = Map::new();
                table.insert(name, other.clone());
                print!("{}", written(command.output.format(), table)?);
            }
        }
        return Ok(());
    }
    if command.field.is_some() {
        return Err("a field is read from one record: name its key".to_owned());
    }
    if records.is_empty() && !single {
        eprintln!("no {} records", plain(&command.kind));
        return Ok(());
    }
    print!(
        "{}",
        document(
            &command.kind,
            records
                .into_iter()
                .map(|(key, stored)| (key, stored.definition)),
            command.output.format(),
        )?
    );
    Ok(())
}

/// What the argument `-f` names, read: a file, a directory's documents in name order, or
/// standard input.
fn read_documents(
    files: &[String],
    format: Option<Format>,
) -> Result<Vec<(String, String, Value)>, String> {
    let mut found: Vec<(String, String, Value)> = Vec::new();
    let mut add = |path: &Path, text: &str| -> Result<(), String> {
        for (record, key, definition) in
            parse_document(path, format, text).map_err(|error| error.to_string())?
        {
            let kind = record.name().to_owned();
            if found
                .iter()
                .any(|(other, held, _)| *other == kind && *held == key)
            {
                return Err(format!("{kind} {key} is given by two documents"));
            }
            found.push((kind, key, definition));
        }
        Ok(())
    };
    for file in files {
        if file == "-" {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| format!("standard input: {error}"))?;
            add(Path::new("stdin.cfg"), &text)?;
            continue;
        }
        let path = PathBuf::from(file);
        let metadata =
            std::fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut documents = Vec::new();
        if metadata.is_dir() {
            for entry in
                std::fs::read_dir(&path).map_err(|error| format!("{}: {error}", path.display()))?
            {
                let entry = entry.map_err(|error| format!("{}: {error}", path.display()))?;
                let inner = entry.path();
                let known = inner
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
                    });
                // `ai6.cfg` is the bootstrap, which a directory of records may sit beside.
                let bootstrap = inner.file_stem().is_some_and(|stem| stem == "ai6");
                if known && !bootstrap && inner.is_file() {
                    documents.push(inner);
                }
            }
            documents.sort();
            if documents.is_empty() {
                return Err(format!(
                    "{} holds no {} document",
                    path.display(),
                    EXTENSIONS.join(", ")
                ));
            }
        } else {
            documents.push(path);
        }
        for document in documents {
            let text = std::fs::read_to_string(&document)
                .map_err(|error| format!("{}: {error}", document.display()))?;
            add(&document, &text)?;
        }
    }
    if found.is_empty() {
        return Err("the documents hold no records".to_owned());
    }
    Ok(found)
}

fn confirmed(weakened: &[Value]) -> Result<bool, String> {
    for weakening in weakened {
        println!(
            "weakening\t{}",
            plain(weakening.as_str().unwrap_or_default())
        );
    }
    if !std::io::stdin().is_terminal() {
        return Err(
            "these records loosen what is in force; repeat with --yes to store them".to_owned(),
        );
    }
    print!("Store them? [y/N] ");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    Ok(answer.trim().eq_ignore_ascii_case("y"))
}

pub(crate) async fn apply(gateway: &Gateway<'_>, command: Apply) -> Result<(), String> {
    let documents = read_documents(&command.files, command.format.map(Input::format))?;
    let records: Vec<Value> = documents
        .iter()
        .map(|(kind, key, definition)| json!({"kind": kind, "key": key, "definition": definition}))
        .collect();
    let mut request =
        json!({"records": records, "dry_run": command.dry_run, "confirm": command.yes});
    if let Some(station) = station_of(&command.scope) {
        request["station"] = json!(station);
    }
    let mut answer = gateway
        .send(
            Method::POST,
            &["resources", "apply"],
            &[],
            None,
            Some(request.clone()),
        )
        .await?;
    if answer["applied"] == false && !command.dry_run {
        let weakened = answer["weakened"].as_array().cloned().unwrap_or_default();
        if !confirmed(&weakened)? {
            return Err("nothing stored".to_owned());
        }
        request["confirm"] = json!(true);
        answer = gateway
            .send(
                Method::POST,
                &["resources", "apply"],
                &[],
                None,
                Some(request),
            )
            .await?;
    }
    for record in answer["records"].as_array().into_iter().flatten() {
        let verb = if command.dry_run {
            format!(
                "would be {}",
                plain(record["change"].as_str().unwrap_or_default())
            )
        } else {
            plain(record["change"].as_str().unwrap_or_default())
        };
        println!(
            "{}/{} {verb}",
            plain(record["kind"].as_str().unwrap_or_default()),
            plain(record["key"].as_str().unwrap_or_default()),
        );
    }
    if command.dry_run {
        for weakening in answer["weakened"].as_array().into_iter().flatten() {
            println!(
                "weakening\t{}",
                plain(weakening.as_str().unwrap_or_default())
            );
        }
        println!("dry run: nothing stored");
    } else {
        print_change(&answer);
    }
    Ok(())
}

pub(crate) async fn edit(gateway: &Gateway<'_>, command: Edit) -> Result<(), String> {
    let station = station_of(&command.scope);
    let record = family_of_kind(&command.kind)
        .ok_or_else(|| format!("{:?} is not a kind of record", command.kind))?;
    let stored = gateway
        .list(&command.kind, station)
        .await?
        .into_iter()
        .find(|(key, _)| *key == command.key);
    let (original, revision) = match stored {
        Some((_, stored)) => (stored.definition, stored.revision),
        None => (json!({}), "0".to_owned()),
    };
    let format = command.output.format();
    let text = document(
        &command.kind,
        [(command.key.clone(), original.clone())],
        format,
    )?;
    let extension = match format {
        Format::Json => "json",
        Format::Yaml => "yaml",
        _ => "cfg",
    };
    let path = std::env::temp_dir().join(format!(
        "ai6ctl-{}-{}.{extension}",
        std::process::id(),
        record.name()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    drop(file);
    let edited = run_editor(&path).and_then(|()| {
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
    });
    let _ = std::fs::remove_file(&path);
    let edited = edited?;
    let mut records = parse_document(&path, None, &edited).map_err(|error| error.to_string())?;
    let Some((found, key, definition)) = records.pop().filter(|_| records.is_empty()) else {
        return Err(format!(
            "the saved document must hold exactly {}.{}",
            record.name(),
            plain(&command.key)
        ));
    };
    if found != record || key != command.key {
        return Err(format!(
            "the saved document must hold {}.{}, not {}.{}",
            record.name(),
            plain(&command.key),
            found.name(),
            plain(&key)
        ));
    }
    if definition == original {
        println!("{}/{} unchanged", record.name(), plain(&key));
        return Ok(());
    }
    gateway
        .put(
            (&command.kind, &key),
            station,
            command.if_revision.as_deref().unwrap_or(&revision),
            definition,
            command.yes,
        )
        .await
}

/// Runs `$EDITOR` (else `$VISUAL`) on `path`, as the command words it names.
fn run_editor(path: &Path) -> Result<(), String> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map_err(|_| "EDITOR names no editor".to_owned())?;
    let mut words = editor.split_whitespace();
    let program = words
        .next()
        .ok_or_else(|| "EDITOR names no editor".to_owned())?;
    let status = std::process::Command::new(program)
        .args(words)
        .arg(path)
        .status()
        .map_err(|error| format!("{program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

/// The schema node of `name` in the object `node` describes, through references and
/// alternatives; `None` when the schema does not say.
fn property(root: &Value, node: &Value, name: &str, depth: usize) -> Lookup {
    if depth > 16 {
        return Lookup::Free;
    }
    if let Some(reference) = node["$ref"].as_str() {
        return match root.pointer(reference.trim_start_matches('#')) {
            Some(target) => property(root, target, name, depth + 1),
            None => Lookup::Free,
        };
    }
    let mut missing = None;
    for choice in ["anyOf", "oneOf", "allOf"] {
        for alternative in node[choice].as_array().into_iter().flatten() {
            match property(root, alternative, name, depth + 1) {
                Lookup::Found(found) => return Lookup::Found(found),
                Lookup::Free => return Lookup::Free,
                Lookup::Missing(fields) => missing = Some(fields),
            }
        }
    }
    if let Some(properties) = node["properties"].as_object() {
        if let Some(found) = properties.get(name) {
            return Lookup::Found(found.clone());
        }
        return match &node["additionalProperties"] {
            Value::Object(_) => Lookup::Found(node["additionalProperties"].clone()),
            Value::Bool(false) => Lookup::Missing(properties.keys().cloned().collect()),
            _ => Lookup::Free,
        };
    }
    if let Value::Object(_) = &node["additionalProperties"] {
        return Lookup::Found(node["additionalProperties"].clone());
    }
    missing.map_or(Lookup::Free, Lookup::Missing)
}

/// What a schema says of a field.
enum Lookup {
    Found(Value),
    /// The schema allows any field here, or does not say.
    Free,
    /// The object has these fields and not this one.
    Missing(Vec<String>),
}

/// The JSON types a schema node allows.
fn types(root: &Value, node: &Value, found: &mut BTreeSet<String>, depth: usize) {
    if depth > 16 {
        return;
    }
    if let Some(reference) = node["$ref"].as_str()
        && let Some(target) = root.pointer(reference.trim_start_matches('#'))
    {
        types(root, target, found, depth + 1);
    }
    match &node["type"] {
        Value::String(name) => {
            found.insert(name.clone());
        }
        Value::Array(names) => {
            found.extend(names.iter().filter_map(Value::as_str).map(str::to_owned))
        }
        _ => {}
    }
    for choice in ["anyOf", "oneOf", "allOf"] {
        for alternative in node[choice].as_array().into_iter().flatten() {
            types(root, alternative, found, depth + 1);
        }
    }
    for listed in node["enum"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(node.get("const"))
    {
        found.insert(
            match listed {
                Value::Bool(_) => "boolean",
                Value::Number(_) => "integer",
                _ => "string",
            }
            .to_owned(),
        );
    }
    if node["properties"].is_object() {
        found.insert("object".to_owned());
    }
}

/// The items schema of an array node.
fn items(root: &Value, node: &Value, depth: usize) -> Option<Value> {
    if depth > 16 {
        return None;
    }
    if let Some(reference) = node["$ref"].as_str() {
        return items(
            root,
            root.pointer(reference.trim_start_matches('#'))?,
            depth + 1,
        );
    }
    if node["items"].is_object() {
        return Some(node["items"].clone());
    }
    ["anyOf", "oneOf", "allOf"]
        .iter()
        .flat_map(|choice| node[*choice].as_array().into_iter().flatten())
        .find_map(|alternative| items(root, alternative, depth + 1))
}

/// `text` as a value of the field `node` describes; `None` (no schema) guesses from the text.
fn parse_value(root: &Value, node: Option<&Value>, text: &str) -> Result<Value, String> {
    let mut allowed = BTreeSet::new();
    if let Some(node) = node {
        types(root, node, &mut allowed, 0);
    }
    let has = |name: &str| allowed.contains(name);
    let guess = allowed.is_empty();
    // With no schema to say, a JSON string forces text: "3" is the text 3.
    if guess && text.starts_with('"') {
        return serde_json::from_str::<String>(text)
            .map(Value::String)
            .map_err(|_| "a quoted value is a JSON string".to_owned());
    }
    if (has("boolean") || guess) && (text == "true" || text == "false") {
        return Ok(Value::Bool(text == "true"));
    }
    // A guess keeps a codename like 007 as text: only a number written as it prints is one.
    if has("integer") || has("number") || guess {
        if let Ok(number) = text.parse::<i64>()
            && (!guess || number.to_string() == text)
        {
            return Ok(json!(number));
        }
        if let Ok(number) = text.parse::<u64>()
            && (!guess || number.to_string() == text)
        {
            return Ok(json!(number));
        }
    }
    if (has("number") || guess)
        && let Ok(number) = text.parse::<f64>()
        && number.is_finite()
        && (!guess || number.to_string() == text)
    {
        return Ok(json!(number));
    }
    if has("array") || (guess && text.starts_with('[')) {
        if text.starts_with('[') {
            return serde_json::from_str(text).map_err(|_| "a list is [..] JSON or a,b".to_owned());
        }
        let inner = node.and_then(|node| items(root, node, 0));
        return text
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| parse_value(root, inner.as_ref(), part))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    if has("object") || (guess && text.starts_with('{')) {
        let parsed: Value =
            serde_json::from_str(text).map_err(|_| "a table is given as JSON".to_owned())?;
        return if parsed.is_object() {
            Ok(parsed)
        } else {
            Err("a table is given as JSON".to_owned())
        };
    }
    if has("string") || guess {
        // A string field takes a quoted value as the string inside the quotes (TOML's basic
        // `"3"` or literal `'3'`), and any other text as it is.
        if has("string") {
            if text.starts_with('"')
                && let Ok(parsed) = serde_json::from_str::<String>(text)
            {
                return Ok(Value::String(parsed));
            }
            if text.len() >= 2
                && text.starts_with('\'')
                && text.ends_with('\'')
                && !text[1..text.len() - 1].contains('\'')
            {
                return Ok(Value::String(text[1..text.len() - 1].to_owned()));
            }
        }
        return Ok(Value::String(text.to_owned()));
    }
    Err(format!(
        "takes {}",
        allowed.iter().cloned().collect::<Vec<_>>().join(" or ")
    ))
}

/// `value` as the field `path` of the record's schema takes it.
fn typed(root: &Value, path: &str, text: &str) -> Result<Value, String> {
    let mut node = Some(root.clone());
    for segment in path.split('.') {
        node = match node {
            Some(parent) => match property(root, &parent, segment, 0) {
                Lookup::Found(child) => Some(child),
                Lookup::Free => None,
                Lookup::Missing(fields) => {
                    return Err(format!(
                        "no field {}; its fields are {}",
                        plain(segment),
                        fields.join(", ")
                    ));
                }
            },
            None => None,
        };
    }
    parse_value(root, node.as_ref(), text).map_err(|reason| format!("{}: {reason}", plain(path)))
}

fn put_path(definition: &mut Value, path: &str, value: Value) -> Result<(), String> {
    let mut table = definition;
    let mut segments = path.split('.').peekable();
    while let Some(segment) = segments.next() {
        let object = table
            .as_object_mut()
            .ok_or_else(|| format!("{} is not a table", plain(path)))?;
        if segments.peek().is_none() {
            object.insert(segment.to_owned(), value);
            return Ok(());
        }
        table = object.entry(segment).or_insert_with(|| json!({}));
    }
    Ok(())
}

fn remove_path(definition: &mut Value, path: &str) -> bool {
    let (parents, last) = path.rsplit_once('.').unwrap_or(("", path));
    let mut table = definition;
    if !parents.is_empty() {
        for segment in parents.split('.') {
            match table.get_mut(segment) {
                Some(inner) => table = inner,
                None => return false,
            }
        }
    }
    table
        .as_object_mut()
        .is_some_and(|object| object.remove(last).is_some())
}

/// The record `key` of `kind` and its revision (`"0"` and an empty table when there is none).
async fn stored(
    gateway: &Gateway<'_>,
    kind: &str,
    key: &str,
    station: Option<&str>,
) -> Result<Stored, String> {
    Ok(gateway
        .list(kind, station)
        .await?
        .into_iter()
        .find(|(found, _)| found == key)
        .map_or_else(
            || Stored {
                definition: json!({}),
                revision: "0".to_owned(),
            },
            |(_, stored)| stored,
        ))
}

pub(crate) async fn set(gateway: &Gateway<'_>, command: Set) -> Result<(), String> {
    let station = station_of(&command.scope);
    let schema = gateway.schema(&command.kind).await?;
    let mut held = stored(gateway, &command.kind, &command.key, station).await?;
    for assignment in &command.assignments {
        let (field, text) = assignment
            .split_once('=')
            .filter(|(field, _)| !field.is_empty())
            .ok_or_else(|| format!("{} is not field=value", plain(assignment)))?;
        let value = typed(&schema, field, text)?;
        put_path(&mut held.definition, field, value)?;
    }
    gateway
        .put(
            (&command.kind, &command.key),
            station,
            command.if_revision.as_deref().unwrap_or(&held.revision),
            held.definition,
            command.yes,
        )
        .await
}

pub(crate) async fn unset(gateway: &Gateway<'_>, command: Unset) -> Result<(), String> {
    let station = station_of(&command.scope);
    let mut held = stored(gateway, &command.kind, &command.key, station).await?;
    if held.revision == "0" {
        return Err(format!(
            "no {} record {}",
            plain(&command.kind),
            plain(&command.key)
        ));
    }
    if !remove_path(&mut held.definition, &command.field) {
        return Err(format!(
            "{} {} does not set {}",
            plain(&command.kind),
            plain(&command.key),
            plain(&command.field)
        ));
    }
    gateway
        .put(
            (&command.kind, &command.key),
            station,
            command.if_revision.as_deref().unwrap_or(&held.revision),
            held.definition,
            command.yes,
        )
        .await
}

pub(crate) async fn delete(gateway: &Gateway<'_>, command: Delete) -> Result<(), String> {
    let mut query: Vec<(&str, &str)> = Vec::new();
    if let Some(station) = station_of(&command.scope) {
        query.push(("station", station));
    }
    if command.force {
        query.push(("force", "true"));
    }
    let changed = gateway
        .send(
            Method::DELETE,
            &["roster", &command.kind, &command.key],
            &query,
            command.if_revision.as_deref(),
            None,
        )
        .await?;
    println!("{}/{} deleted", plain(&command.kind), plain(&command.key));
    print_change(&changed);
    Ok(())
}

pub(crate) async fn api_resources(gateway: &Gateway<'_>) -> Result<(), String> {
    let listed = gateway
        .send(Method::GET, &["resources"], &[], None, None)
        .await?;
    println!("KIND\tROUTE\tREAD\tMANAGE\tRESOURCE\tSTATION\tSINGLETON\tLIVE\tALLOWED");
    for resource in listed.as_array().into_iter().flatten() {
        let text = |key: &str| plain(resource[key].as_str().unwrap_or_default());
        let allowed: Vec<String> = resource["allowed"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|action| plain(action.as_str().unwrap_or_default()))
            .collect();
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            text("kind"),
            text("route"),
            text("read"),
            text("manage"),
            text("resource"),
            resource["station_scoped"],
            resource["singleton"],
            resource["live"],
            allowed.join(","),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent": {"type": "string"},
                "count": {"type": "integer"},
                "ratio": {"type": "number"},
                "on": {"type": "boolean"},
                "limit": {"anyOf": [{"type": "integer"}, {"const": "none"}]},
                "names": {"type": "array", "items": {"type": "string"}},
                "sizes": {"type": "array", "items": {"$ref": "#/$defs/Size"}},
                "inner": {"$ref": "#/$defs/Inner"},
                "free": {"type": "object", "additionalProperties": {"type": "integer"}}
            },
            "additionalProperties": false,
            "$defs": {
                "Size": {"type": "integer"},
                "Inner": {"type": "object", "properties": {"kinds": {"type": "array", "items": {"type": "string"}}}, "additionalProperties": false}
            }
        })
    }

    #[test]
    fn a_value_is_read_as_the_type_its_field_declares() {
        let schema = schema();
        for (field, text, expected) in [
            ("agent", "007", json!("007")),
            ("count", "007", json!(7)),
            ("ratio", "0.5", json!(0.5)),
            ("on", "true", json!(true)),
            ("limit", "4", json!(4)),
            ("limit", "none", json!("none")),
            ("names", "a, b,c", json!(["a", "b", "c"])),
            ("names", "[\"a,b\"]", json!(["a,b"])),
            ("names", "", json!([])),
            ("sizes", "1,2", json!([1, 2])),
            ("inner.kinds", "x", json!(["x"])),
            ("free.a", "3", json!(3)),
        ] {
            assert_eq!(
                typed(&schema, field, text).unwrap(),
                expected,
                "{field}={text}"
            );
        }
        for (field, text, reason) in [
            ("count", "many", "takes integer"),
            ("on", "yes", "takes boolean"),
            ("nope", "1", "no field nope; its fields are "),
            ("inner.nope", "1", "no field nope; its fields are kinds"),
        ] {
            let error = typed(&schema, field, text).unwrap_err();
            assert!(error.contains(reason), "{field}={text}: {error}");
        }
    }

    #[test]
    fn with_no_schema_a_number_as_written_is_a_number_and_a_codename_is_text() {
        for (text, expected) in [
            ("4", json!(4)),
            ("007", json!("007")),
            ("true", json!(true)),
            ("1.5", json!(1.5)),
            ("nan", json!("nan")),
            ("\"3\"", json!("3")),
            ("a b", json!("a b")),
            ("[1, 2]", json!([1, 2])),
            ("{\"a\": 1}", json!({"a": 1})),
        ] {
            assert_eq!(
                typed(&Value::Null, "x.y", text).unwrap(),
                expected,
                "{text}"
            );
        }
    }

    #[test]
    fn fields_are_set_and_removed_by_their_dotted_path() {
        let mut record = json!({"a": {"b": 1}, "c": 2});
        put_path(&mut record, "a.d.e", json!(3)).unwrap();
        put_path(&mut record, "c", json!(4)).unwrap();
        assert_eq!(record, json!({"a": {"b": 1, "d": {"e": 3}}, "c": 4}));
        assert!(put_path(&mut record, "c.x", json!(1)).is_err());
        assert!(remove_path(&mut record, "a.d.e"));
        assert!(!remove_path(&mut record, "a.d.e"));
        assert!(!remove_path(&mut record, "z.y"));
        assert!(remove_path(&mut record, "c"));
        assert_eq!(record, json!({"a": {"b": 1, "d": {}}}));
    }
}
