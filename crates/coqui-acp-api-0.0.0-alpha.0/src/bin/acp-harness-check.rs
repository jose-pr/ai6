//! `acp-harness-check`: runs the `acp_harness` contract on a provider in a library file, or in a
//! source file any Guatiao source loads (a Python, JavaScript or Lua provider), with no Rust of the
//! provider's own.
//!
//! ```text
//! acp-harness-check --library FILE [OPTIONS]
//! acp-harness-check --source SOURCE_LIBRARY --source-config JSON [OPTIONS]
//!
//! --provider KEY        the provider, when the file offers more than one acp_harness
//! --settings JSON       build an instance from these settings instead of the default instance
//! --version N           the ACP version initialize asks for (1)
//! --holds NAME[=JSON]   a requirement it holds, repeated; the default holds read_only and no_commands
//! --message-limit N     the largest message it takes, when it has a limit
//! --no-imports          it imports no session, and must refuse import_session with a reason
//! ```
//!
//! Exits 0 when the provider keeps every rule, 1 with each rule it broke, 2 when it could not be
//! loaded or the command line is wrong.
#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::print_stderr,
    reason = "a program names what it uses from std and reports on its own streams"
)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use coqui_acp_api::contract::Contract;
use coqui_acp_api::guatiao::library::Registry;
use coqui_acp_api::guatiao::{List, Map, Number, Value};
use coqui_acp_api::{AcpHarness, Requirement};

#[derive(Default)]
struct Args {
    library: Option<PathBuf>,
    source: Option<PathBuf>,
    source_config: Option<String>,
    provider: Option<String>,
    settings: Option<String>,
    version: Option<u16>,
    holds: Vec<Requirement>,
    message_limit: Option<usize>,
    no_imports: bool,
}

fn parse() -> Result<Args, String> {
    let mut args = Args::default();
    let mut given = std::env::args().skip(1);
    while let Some(flag) = given.next() {
        let mut value = || given.next().ok_or_else(|| format!("{flag} takes a value"));
        match flag.as_str() {
            "--library" => args.library = Some(value()?.into()),
            "--source" => args.source = Some(value()?.into()),
            "--source-config" => args.source_config = Some(value()?),
            "--provider" => args.provider = Some(value()?),
            "--settings" => args.settings = Some(value()?),
            "--version" => {
                args.version = Some(value()?.parse().map_err(|_| "--version takes a number")?);
            }
            "--message-limit" => {
                args.message_limit = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--message-limit takes a number")?,
                );
            }
            "--no-imports" => args.no_imports = true,
            "--holds" => {
                let held = value()?;
                args.holds.push(match held.split_once('=') {
                    Some((name, json)) => Requirement::with_value(name, json_value(json)?),
                    None => Requirement::new(held),
                });
            }
            _ => return Err(format!("{flag} is not an option")),
        }
    }
    if args.library.is_some() == args.source.is_some() {
        return Err("name one of --library and --source".into());
    }
    Ok(args)
}

/// JSON text as a Guatiao value.
fn json_value(text: &str) -> Result<Value, String> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    convert(&json)
}

fn convert(json: &serde_json::Value) -> Result<Value, String> {
    Ok(match json {
        serde_json::Value::Null => Value::null(),
        serde_json::Value::Bool(b) => Value::from(*b),
        serde_json::Value::Number(n) => {
            Value::from(Number::new(&n.to_string()).map_err(|e| e.to_string())?)
        }
        serde_json::Value::String(s) => Value::from(s.as_str()),
        serde_json::Value::Array(items) => {
            let mut list = List::new();
            for item in items {
                list.push(convert(item)?);
            }
            Value::from(list)
        }
        serde_json::Value::Object(entries) => Value::from(map(entries)?),
    })
}

fn map(entries: &serde_json::Map<String, serde_json::Value>) -> Result<Map, String> {
    let mut map = Map::new();
    for (key, value) in entries {
        map.set(key, convert(value)?);
    }
    Ok(map)
}

fn load(args: &Args) -> Result<(Registry, Arc<dyn AcpHarness>), String> {
    let mut registry = Registry::new("acp-harness-check", env!("CARGO_PKG_VERSION"));
    if let Some(library) = &args.library {
        registry
            .load_file(library)
            .map_err(|e| format!("{} did not load: {e}", library.display()))?;
    }
    if let Some(source) = &args.source {
        let config = match &args.source_config {
            Some(text) => match serde_json::from_str(text) {
                Ok(serde_json::Value::Object(entries)) => map(&entries)?,
                _ => return Err("--source-config takes a JSON object".into()),
            },
            None => Map::new(),
        };
        registry
            .add_source_file(source, &config)
            .map_err(|e| format!("{} was refused: {e}", source.display()))?;
        let refreshed = registry.refresh();
        if !refreshed.refused.is_empty() {
            return Err(format!("the source refused: {refreshed:?}"));
        }
    }
    let offer = match &args.provider {
        Some(key) => registry
            .offer::<dyn AcpHarness>(key)
            .ok_or_else(|| format!("nothing answers to {key}"))?
            .map_err(|e| format!("{key} is not this acp_harness: {e:?}"))?,
        None => {
            let mut offers = registry.offers::<dyn AcpHarness>();
            let first = offers.next().ok_or("no acp_harness provider was found")?;
            if offers.next().is_some() {
                return Err("more than one acp_harness: name one with --provider".into());
            }
            first
        }
    };
    let harness: Arc<dyn AcpHarness> = match &args.settings {
        Some(text) => Arc::new(
            offer
                .instantiate(&json_value(text)?)
                .map_err(|e| format!("the settings were refused: {e:?}"))?,
        ),
        None => offer
            .shared()
            .ok_or("the provider has no default instance: give --settings")?,
    };
    Ok((registry, harness))
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(args) => args,
        Err(why) => {
            eprintln!("acp-harness-check: {why}");
            return ExitCode::from(2);
        }
    };
    let (registry, harness) = match load(&args) {
        Ok(loaded) => loaded,
        Err(why) => {
            eprintln!("acp-harness-check: {why}");
            return ExitCode::from(2);
        }
    };
    let name = args
        .provider
        .clone()
        .unwrap_or_else(|| "the provider".into());
    let mut contract = Contract::new(name.clone());
    if let Some(version) = args.version {
        contract = contract.version(version);
    }
    if !args.holds.is_empty() {
        contract = contract.holds(args.holds.clone());
    }
    if let Some(limit) = args.message_limit {
        contract = contract.message_limit(limit);
    }
    if args.no_imports {
        contract = contract.imports(false);
    }
    let outcome = contract.run(&*harness);
    drop(harness);
    drop(registry);
    match outcome {
        Ok(()) => {
            println!("{name} keeps every rule of the acp_harness contract");
            ExitCode::SUCCESS
        }
        Err(failures) => {
            eprintln!("{failures}");
            ExitCode::from(1)
        }
    }
}
