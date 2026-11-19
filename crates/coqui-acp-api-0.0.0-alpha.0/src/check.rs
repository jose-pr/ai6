//! The contract suites run on a provider in a library file, or in a source file any Guatiao source loads (a Python,
//! JavaScript or Lua provider), with no Rust of the provider's own: [`HarnessCheck`] for an `acp_harness`,
//! [`AccountCheck`] for an `account_provider`. Each is a `clap::Args` type with a `run` function; a program that people
//! run is the host's, and `examples/check.rs` is one for this crate's tests.
//!
//! A run ends with status 0 when the provider keeps every rule, or 1 with each rule it broke on standard error. An
//! `Err` is a provider that could not be loaded or a command line that is wrong; a program says it and ends with 2.
#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "a program's commands name what they use from std and report on its own streams"
)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use guatiao::library::Registry;
use guatiao::{List, Map, Number, Value};

use crate::contract::{AccountContract, Contract, HOME_VARIABLE};
use crate::{AccountProvider, AcpHarness, Requirement};

/// Where the provider is and how an instance of it is made.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct Provider {
    /// A library file that offers the provider.
    #[arg(long, value_name = "FILE")]
    pub library: Option<PathBuf>,
    /// A Guatiao source's library file, which loads the provider from `--source-config`.
    #[arg(long, value_name = "SOURCE_LIBRARY")]
    pub source: Option<PathBuf>,
    /// The source's configuration, a JSON object.
    #[arg(long, value_name = "JSON")]
    pub source_config: Option<String>,
    /// The provider, when the file offers more than one of the kind.
    #[arg(long, value_name = "KEY")]
    pub provider: Option<String>,
    /// Build an instance from these settings, JSON, instead of the default instance.
    #[arg(long, value_name = "JSON")]
    pub settings: Option<String>,
    /// Run the account checks on this kind of account alone.
    #[arg(long, value_name = "KIND")]
    pub account: Option<String>,
}

/// A program that runs a check again, for a provider whose harness starts a program (`--program-agent`).
///
/// The run needs a home of its own and no configuration directory of a command line in its environment, so that neither
/// the provider nor the program it starts can reach the person's home; the process that runs it cannot give itself that,
/// so the host names the program that does, with the arguments that make it run this same check.
#[derive(Debug, Clone)]
pub struct Again {
    /// The program, usually the host's own.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
}

/// Runs the `acp_harness` contract on a provider.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct HarnessCheck {
    /// Where the provider is.
    #[command(flatten)]
    pub provider: Provider,
    /// The ACP version `initialize` asks for.
    #[arg(long, value_name = "N")]
    pub version: Option<u16>,
    /// A requirement it holds as `NAME` or `NAME=JSON`, repeated; by default it holds `read_only` and `no_commands`. `none`
    /// says it holds none.
    #[arg(long, value_name = "NAME[=JSON]")]
    pub holds: Vec<String>,
    /// The largest message it takes, when it has a limit.
    #[arg(long, value_name = "N")]
    pub message_limit: Option<usize>,
    /// It imports no session, and must refuse `import_session` with a reason.
    #[arg(long)]
    pub no_imports: bool,
    /// The provider starts its harness's own program, which the checker's spawner then starts for real: name a stand-in
    /// for it in `--settings`.
    #[arg(long)]
    pub program_agent: bool,
    /// The program that runs this check again in a home of its own, which `--program-agent` needs unless the process
    /// already runs in one (`ACP_HARNESS_CHECK_HOME`). Set by the host, not by an argument.
    #[arg(skip)]
    pub again: Option<Again>,
}

/// Runs the `account_provider` contract on a provider.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct AccountCheck {
    /// Where the provider is.
    #[command(flatten)]
    pub provider: Provider,
    /// It offers no login method, lists none, and must refuse login with a reason.
    #[arg(long)]
    pub no_logins: bool,
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

/// The registry that holds the provider's file.
fn registry(provider: &Provider) -> Result<Registry, String> {
    if provider.library.is_some() == provider.source.is_some() {
        return Err("name one of --library and --source".into());
    }
    let mut registry = Registry::new("coqui-acp-api", env!("CARGO_PKG_VERSION"));
    if let Some(library) = &provider.library {
        registry
            .load_file(library)
            .map_err(|e| format!("{} did not load: {e}", library.display()))?;
    }
    if let Some(source) = &provider.source {
        let config = match &provider.source_config {
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
    Ok(registry)
}

fn load_harness(provider: &Provider) -> Result<(Registry, Arc<dyn AcpHarness>), String> {
    let registry = registry(provider)?;
    let offer = match &provider.provider {
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
    let harness: Arc<dyn AcpHarness> = match &provider.settings {
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

fn load_account(provider: &Provider) -> Result<(Registry, Arc<dyn AccountProvider>), String> {
    let registry = registry(provider)?;
    let offer = match &provider.provider {
        Some(key) => registry
            .offer::<dyn AccountProvider>(key)
            .ok_or_else(|| format!("nothing answers to {key}"))?
            .map_err(|e| format!("{key} is not this account_provider: {e:?}"))?,
        None => {
            let mut offers = registry.offers::<dyn AccountProvider>();
            let first = offers
                .next()
                .ok_or("no account_provider provider was found")?;
            if offers.next().is_some() {
                return Err("more than one account_provider: name one with --provider".into());
            }
            first
        }
    };
    let account: Arc<dyn AccountProvider> = match &provider.settings {
        Some(text) => Arc::new(
            offer
                .instantiate(&json_value(text)?)
                .map_err(|e| format!("the settings were refused: {e:?}"))?,
        ),
        None => offer
            .shared()
            .ok_or("the provider has no default instance: give --settings")?,
    };
    Ok((registry, account))
}

/// Runs the check again with a home of its own and no configuration directory of a command line in its environment, and
/// answers how it ended.
fn homed(again: &Again) -> Result<u8, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let home = std::env::temp_dir().join(format!(
        "acp-harness-check-home-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&home).map_err(|why| format!("no home to run in: {why}"))?;
    let mut command = std::process::Command::new(&again.program);
    command.args(&again.args);
    for name in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ] {
        command.env(name, &home);
    }
    for name in ["CLAUDE_CONFIG_DIR", "CODEX_HOME"] {
        command.env_remove(name);
    }
    command.env(HOME_VARIABLE, &home);
    let status = command.status();
    let _ = std::fs::remove_dir_all(&home);
    status
        .map(|status| u8::try_from(status.code().unwrap_or(2)).unwrap_or(2))
        .map_err(|why| format!("could not run again: {why}"))
}

/// Runs the `acp_harness` contract on the provider.
///
/// # Errors
///
/// The command line is wrong, or the provider could not be loaded or made.
pub fn run_harness(args: &HarnessCheck) -> Result<u8, String> {
    let mut holds = Vec::new();
    let mut holds_none = false;
    for held in &args.holds {
        if held == "none" {
            holds_none = true;
        } else {
            holds.push(match held.split_once('=') {
                Some((name, json)) => Requirement::with_value(name, json_value(json)?),
                None => Requirement::new(held.as_str()),
            });
        }
    }
    if holds_none && !holds.is_empty() {
        return Err("--holds none holds nothing: name no other requirement".into());
    }
    if args.program_agent && args.provider.settings.is_none() {
        return Err(
            "--program-agent needs --settings, naming the program the provider starts".into(),
        );
    }
    if args.provider.library.is_some() == args.provider.source.is_some() {
        return Err("name one of --library and --source".into());
    }
    if args.program_agent && std::env::var_os(HOME_VARIABLE).is_none() {
        let again = args.again.as_ref().ok_or_else(|| {
            format!(
                "--program-agent runs the suite in a home of its own: start this with {HOME_VARIABLE} set, or name the program that runs it again"
            )
        })?;
        return homed(again);
    }
    let (registry, harness) = load_harness(&args.provider)?;
    let name = args
        .provider
        .provider
        .clone()
        .unwrap_or_else(|| "the provider".into());
    let mut contract = Contract::new(name.clone());
    if let Some(version) = args.version {
        contract = contract.version(version);
    }
    if holds_none {
        contract = contract.holds(Vec::new());
    } else if !holds.is_empty() {
        contract = contract.holds(holds);
    }
    if let Some(limit) = args.message_limit {
        contract = contract.message_limit(limit);
    }
    if args.no_imports {
        contract = contract.imports(false);
    }
    if let Some(kind) = &args.provider.account {
        contract = contract.account(kind.clone());
    }
    if args.program_agent {
        contract = contract.program_agent(true);
    }
    let outcome = contract.run(&*harness);
    drop(harness);
    drop(registry);
    Ok(match outcome {
        Ok(()) => {
            println!("{name} keeps every rule of the acp_harness contract");
            0
        }
        Err(failures) => {
            eprintln!("{failures}");
            1
        }
    })
}

/// Runs the `account_provider` contract on the provider.
///
/// # Errors
///
/// The provider could not be loaded or made.
pub fn run_account(args: &AccountCheck) -> Result<u8, String> {
    let (registry, provider) = load_account(&args.provider)?;
    let name = args
        .provider
        .provider
        .clone()
        .unwrap_or_else(|| "the provider".into());
    let mut contract = AccountContract::new(name.clone());
    if args.no_logins {
        contract = contract.logins(false);
    }
    if let Some(kind) = &args.provider.account {
        contract = contract.account(kind.clone());
    }
    let outcome = contract.run(&*provider);
    drop(provider);
    drop(registry);
    Ok(match outcome {
        Ok(()) => {
            println!("{name} keeps every rule of the account_provider contract");
            0
        }
        Err(failures) => {
            eprintln!("{failures}");
            1
        }
    })
}
