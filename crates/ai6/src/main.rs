use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use ai6::station::identity;
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ai6",
    version,
    about = "The AI6 Mission daemon",
    after_help = "Optional Universal Exports publication reads AI6_EXPORTS_URL and AI6_EXPORTS_STATION_TOKEN from the environment."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the daemon's private local service.
    Serve(Box<ServeArgs>),
    /// Print or create this station's certificate identity.
    Identity,
    /// Manage pinned station certificates.
    Peer {
        #[command(subcommand)]
        command: PeerCommand,
    },
    /// Authorize or revoke standing orders (sleepers) configured in operations.toml.
    Sleeper {
        /// AI6 configuration directory containing operations.toml.
        #[arg(long, global = true)]
        config_dir: Option<PathBuf>,
        #[command(subcommand)]
        command: SleeperCommand,
    },
    /// List, add or release Agent quarantines in the shared Registry.
    Quarantine {
        #[command(subcommand)]
        command: QuarantineCommand,
    },
    /// The field runner: supervise agent harnesses on this host so Sessions outlive their Station.
    Runner {
        #[command(subcommand)]
        command: RunnerCommand,
    },
    /// Fit out the Theaters declared in theaters.toml.
    Theater {
        #[command(subcommand)]
        command: TheaterCommand,
    },
    /// Green ink on the harness side.
    GreenInk {
        #[command(subcommand)]
        command: GreenInkCommand,
    },
    /// Moneypenny's harness hooks and settings (`[assistant]` in agents.toml).
    Assistant {
        #[command(subcommand)]
        command: AssistantCommand,
    },
    /// Provider library pins (`plugins.toml`).
    Plugins {
        #[command(subcommand)]
        command: PluginsCommand,
    },
    /// A checkout's harness configuration (`[operation.ID.harness]`).
    Harness {
        #[command(subcommand)]
        command: HarnessCommand,
    },
}

#[derive(Debug, Subcommand)]
enum HarnessCommand {
    /// Print the files of a working directory's harness configuration, each with its SHA-256,
    /// and the `pin` that loads it under `project_settings = "pinned"`.
    Pin {
        /// The working directory (a checkout).
        dir: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum PluginsCommand {
    /// Print the `plugins.toml` entry that pins each AI6 provider library by its SHA-256.
    Pin {
        /// Library files to pin.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum GreenInkCommand {
    /// Claude Code's PreToolUse hook for shell commands. Reads the hook's JSON on standard
    /// input; for a command that pushes to a protected ref, deletes a ref, merges a pull
    /// request or creates a release without a live approval, prints a decision that denies it
    /// and tells the Agent to call request_approval, and so it does for input it cannot read.
    /// Prints nothing otherwise. Asks the Station at --cable-url (else AI6_CABLE_MCP_URL) with
    /// AI6_CABLE_MCP_TOKEN; always exits 0.
    Gate {
        /// The Session's Cable endpoint, as the handler saw it.
        #[arg(long)]
        cable_url: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum AssistantCommand {
    /// Claude Code's PreToolUse hook for Moneypenny's WebFetch. Reads the hook's JSON on
    /// standard input; denies a fetch whose host, or an address it resolves to, is in --deny,
    /// or, when --hosts is given, whose host is not one of them (`*.example.com` covers the
    /// names under it), and denies input it cannot read. Prints nothing otherwise; always
    /// exits 0.
    WebGate {
        /// The hosts she may fetch from, separated by commas; none allows any host --deny does
        /// not name.
        #[arg(long, value_delimiter = ',')]
        hosts: Vec<String>,
        /// What she may never fetch, separated by commas, whatever --hosts says: categories
        /// (loopback, private, link-local, metadata), networks (10.0.0.0/8), addresses and
        /// host names.
        #[arg(long, value_delimiter = ',')]
        deny: Vec<String>,
    },
    /// Print the files of a skill directory, each with its SHA-256, and the `pin` its
    /// `[assistant.skills.NAME]` entry needs: the digest a Station checks its copy against.
    PinSkill {
        /// The skill's directory, holding SKILL.md.
        dir: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum TheaterCommand {
    /// Install what a Theater lacks over its transport: the programs its `requires` names and
    /// its runner. Each step prints ok, changed or failed; a second run on a fit Theater
    /// changes nothing. Exits non-zero when a step failed, or with --check when one would
    /// change.
    Provision {
        /// The Theater ID in theaters.toml.
        theater: String,
        /// Look only: report what would change and write nothing on the Theater.
        #[arg(long)]
        check: bool,
        /// AI6 configuration directory containing theaters.toml.
        #[arg(long)]
        config_dir: Option<PathBuf>,
        /// A directory or library path to scan for the Theater's transport provider.
        #[arg(long = "plugin-path")]
        plugin_paths: Vec<PathBuf>,
        #[command(flatten)]
        pins: PinArgs,
    },
}

/// How provider libraries are checked before they load.
#[derive(Debug, Args)]
struct PinArgs {
    /// The provider library pin set (default: plugins.toml in the configuration directory);
    /// a file named here must exist.
    #[arg(long, value_name = "PATH")]
    plugin_pins: Option<PathBuf>,
    /// How provider libraries are checked: strict (only pinned or signed ones load, from a
    /// verified copy), report (all load; unpinned ones are reported) or off (default: the
    /// pin set's profile, else strict with a pin set and report without).
    #[arg(long, value_name = "PROFILE")]
    plugin_profile: Option<ai6::PluginProfile>,
}

#[derive(Debug, Subcommand)]
enum RunnerCommand {
    /// Serve the runner until interrupted; the harnesses it supervises end with it.
    Serve(RunnerServeArgs),
    /// Send one request to the runner on this host and print its status and JSON answer.
    ///
    /// A Station that reaches this host through a transport talks to the runner this way. A
    /// request body is read from standard input. Exits 3 when no runner answers.
    Call {
        /// The runner's state directory (default: `runner` in AI6's state directory).
        #[arg(long)]
        state_dir: Option<PathBuf>,
        /// HTTP method.
        method: String,
        /// Request path, such as `/v1/sessions`.
        path: String,
    },
}

#[derive(Debug, Args)]
struct RunnerServeArgs {
    /// State directory for the token, the address and spooled events (default: `runner` in
    /// AI6's state directory).
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Loopback address to listen on (default: a port the host chooses).
    #[arg(long)]
    listen: Option<SocketAddr>,
    /// A directory or library path to scan for AgentHandler and Terminal providers.
    #[arg(long = "plugin-path")]
    plugin_paths: Vec<PathBuf>,
    /// Pin the terminal provider harnesses run in (default: an available one that suits the
    /// handler: without a terminal, preferring `pipe`, for a JSON harness; else preferring `pty`).
    #[arg(long)]
    terminal: Option<String>,
    #[command(flatten)]
    pins: PinArgs,
    /// Start in the background, detached from this terminal, and print where it listens.
    #[arg(long)]
    detach: bool,
}

#[derive(Debug, Subcommand)]
enum QuarantineCommand {
    /// List every quarantined Agent, with who raised it and why.
    List,
    /// Quarantine an Agent: its work stops and no new work goes to it.
    Add {
        /// Agent codename.
        agent: String,
        /// Why; recorded with the quarantine.
        reason: String,
        /// Quarantine it in this Operation only (default: every Operation).
        #[arg(long, conflicts_with = "all")]
        operation: Option<String>,
        /// Quarantine it in every Operation.
        #[arg(long)]
        all: bool,
    },
    /// Release an Agent; its stopped Missions return to the queue.
    Release {
        /// Agent codename.
        agent: String,
        /// Release the quarantine of this Operation.
        #[arg(long, conflicts_with = "all")]
        operation: Option<String>,
        /// Release the quarantine that covers every Operation.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SleeperCommand {
    /// List every sleeper and whether its current definition is authorized.
    List,
    /// Authorize the sleeper as it is written now; editing it later withdraws this.
    Authorize {
        /// Operation ID or name.
        operation: String,
        /// Sleeper name.
        name: String,
    },
    /// Withdraw a sleeper's authorization.
    Revoke {
        /// Operation ID or name.
        operation: String,
        /// Sleeper name.
        name: String,
    },
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Local address for the daemon service.
    #[arg(long, default_value = "127.0.0.1:7081")]
    listen: SocketAddr,
    /// A directory or library path to scan for Guatiao AgentHandler providers.
    #[arg(long = "plugin-path")]
    plugin_paths: Vec<PathBuf>,
    #[command(flatten)]
    pins: PinArgs,
    /// AI6 configuration directory containing operations.toml and agents.toml.
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// Where the roster (Operations, Agents, roles, covers) comes from: a configuration
    /// directory, `sqlite:PATH` or a `postgres://` URL (default: the configuration directory).
    #[arg(long, value_name = "LOCATION")]
    config: Option<String>,
    /// Where the shared Registry is: a directory, `sqlite:PATH` or a `postgres://` URL
    /// (default: AI6_REGISTRY, else the state directory).
    #[arg(long, value_name = "LOCATION")]
    registry: Option<String>,
    /// Maximum number of Agent Sessions this daemon may run at once.
    #[arg(long, default_value_t = ai6::DEFAULT_MAX_CONCURRENT_MISSIONS)]
    max_concurrent_missions: usize,
    /// Seconds without provider or terminal activity before a Session stalls.
    #[arg(long, allow_hyphen_values = true, value_parser = parse_duration, default_value = "300")]
    stall_timeout: Duration,
    /// Seconds without novel terminal output before repeated output stalls a Session.
    #[arg(long, allow_hyphen_values = true, value_parser = parse_duration, default_value = "300")]
    loop_timeout: Duration,
    /// Repeated terminal lines required before loop detection applies.
    #[arg(long, default_value_t = ai6::directorates::medical::watchdog::DEFAULT_LOOP_MIN_LINES)]
    loop_min_lines: usize,
    /// Quiet seconds before a prompt-shaped terminal tail marks a Session blocked.
    #[arg(long, allow_hyphen_values = true, value_parser = parse_duration, default_value = "10")]
    blocked_after: Duration,
    /// Optional maximum Session runtime in seconds.
    #[arg(long, allow_hyphen_values = true, value_parser = parse_duration)]
    max_runtime: Option<Duration>,
    /// Distinct terminal lines retained for repeated-output detection.
    #[arg(long, default_value_t = ai6::directorates::medical::watchdog::DEFAULT_LINE_HISTORY)]
    history: usize,
    /// Normalize digit runs when comparing terminal lines for loops.
    #[arg(long, default_value_t = false)]
    normalize_digits: bool,
    /// Station identity in the shared Registry (default: kept in the state directory across restarts).
    #[arg(long)]
    station: Option<String>,
    /// Station roles: control elects Headquarters; execution runs assigned Missions.
    #[arg(long, value_parser = ["control", "execution"])]
    role: Vec<String>,
    /// Station placement label in KEY=VALUE form.
    #[arg(long = "label", value_name = "KEY=VALUE", value_parser = parse_station_label)]
    labels: Vec<(String, String)>,
    /// Pin the terminal provider agent harnesses run in (default: an available one that suits the
    /// handler: without a terminal, preferring `pipe`, for a JSON harness; else preferring `pty`).
    #[arg(long)]
    terminal: Option<String>,
    /// Seconds the Headquarters lease lasts between renewals: how long a standby control
    /// Station waits before taking over from a Headquarters that went dark.
    #[arg(long, value_parser = parse_duration, default_value = "60")]
    headquarters_lease: Duration,
    /// Seconds between samples of each remote Theater's CPUs, load and memory; 0 leaves only
    /// the 300-second requirements probe.
    #[arg(long, value_parser = parse_duration, default_value = "60")]
    resources_every: Duration,
    /// Seconds between checks of the roster source and loadouts.toml; a change that passes
    /// the configuration checks applies without a restart. 0 keeps what the Station started with.
    #[arg(long, value_parser = parse_duration, default_value = "5")]
    config_poll: Duration,
    /// A directory name the Head of Station may remove from the safe house of a Mission that
    /// is over, as a rebuildable cache; repeat for each. Replaces the default list (target,
    /// node_modules, .dart_tool).
    #[arg(long = "safe-house-cache", value_name = "NAME")]
    safe_house_caches: Vec<String>,
    /// Seconds the safe house of a Mission that is over is left alone before its caches may go.
    #[arg(long, value_parser = parse_duration, default_value = "604800")]
    safe_house_cache_age: Duration,
}

#[derive(Debug, Subcommand)]
enum PeerCommand {
    /// Accept a certificate shown in the waiting peer list.
    Accept {
        /// Full certificate fingerprint (sha256:HEX, bare hex or grouped base32) printed by the
        /// connecting station.
        id: String,
    },
    /// List certificates waiting for acceptance.
    Waiting,
}

#[tokio::main]
async fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Identity => match station_identity() {
            Ok(identity) => {
                println!("{}", identity.fingerprint());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: could not load station identity: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Peer { command } => match peer_command(command) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: peer operation failed: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Sleeper {
            config_dir,
            command,
        } => match sleeper_command(config_dir, command) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: sleeper operation failed: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Quarantine { command } => match quarantine_command(command) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: quarantine operation failed: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Runner { command } => runner_command(command).await,
        Command::GreenInk {
            command: GreenInkCommand::Gate { cable_url },
        } => green_ink_gate(cable_url).await,
        Command::Assistant { command } => match command {
            AssistantCommand::WebGate { hosts, deny } => assistant_web_gate(&hosts, &deny),
            AssistantCommand::PinSkill { dir } => match ai6::ProjectManifest::read_tree(&dir) {
                Ok(manifest) => {
                    for (path, digest) in &manifest.files {
                        println!("# {digest}  {path}");
                    }
                    println!("pin = \"{}\"", manifest.digest());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("ai6: {}: {error}", dir.display());
                    ExitCode::FAILURE
                }
            },
        },
        Command::Plugins {
            command: PluginsCommand::Pin { paths },
        } => {
            let mut pins = ai6::PinSet::new();
            for path in &paths {
                match ai6::station::plugins::pin_entry(path) {
                    Ok(entry) => pins = pins.with_library(entry),
                    Err(error) => {
                        eprintln!("ai6: {}: {error}", path.display());
                        return ExitCode::FAILURE;
                    }
                }
            }
            match toml::to_string(&pins) {
                Ok(text) => {
                    print!("{text}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("ai6: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Harness {
            command: HarnessCommand::Pin { dir },
        } => match ai6::ProjectManifest::read(&dir) {
            Ok(manifest) => {
                for (path, digest) in &manifest.files {
                    println!("# {digest}  {path}");
                }
                println!("pin = \"{}\"", manifest.digest());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: {}: {error}", dir.display());
                ExitCode::FAILURE
            }
        },
        Command::Theater { command } => {
            match tokio::task::spawn_blocking(move || theater_command(command)).await {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("ai6: theater command failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Serve(args) => {
            let ServeArgs {
                listen,
                plugin_paths,
                pins,
                config_dir,
                config: roster_location,
                registry,
                max_concurrent_missions,
                stall_timeout,
                loop_timeout,
                loop_min_lines,
                blocked_after,
                max_runtime,
                history,
                normalize_digits,
                station,
                role,
                labels,
                terminal,
                headquarters_lease,
                resources_every,
                config_poll,
                safe_house_caches,
                safe_house_cache_age,
            } = *args;
            let mut hygiene = ai6::station::safehouse::SafeHouseHygiene::default()
                .with_cache_age(safe_house_cache_age);
            if !safe_house_caches.is_empty() {
                hygiene = match hygiene.with_caches(safe_house_caches) {
                    Ok(hygiene) => hygiene,
                    Err(error) => {
                        eprintln!("ai6: --safe-house-cache: {error}");
                        return ExitCode::FAILURE;
                    }
                };
            }
            let publisher = match publisher_config() {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("ai6: Universal Exports configuration failed: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let mut station_config = ai6::StationConfig::default()
                .with_resources_every(resources_every)
                .with_config_poll(config_poll)
                .with_safe_house_hygiene(hygiene);
            if !role.is_empty() {
                station_config =
                    station_config.with_roles(role.iter().filter_map(|role| match role.as_str() {
                        "control" => Some(ai6::StationRole::Control),
                        "execution" => Some(ai6::StationRole::Execution),
                        _ => None,
                    }));
            }
            for (key, value) in labels {
                station_config = station_config.with_label(key, value);
            }
            station_config = station_config.with_headquarters_lease(headquarters_lease);
            if let Some(terminal) = terminal {
                station_config = station_config.with_terminal(terminal);
            }
            let plugin_policy = match plugin_policy(&pins, config_dir.as_deref(), None) {
                Ok(policy) => policy,
                Err(error) => {
                    eprintln!("ai6: provider library pins: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let mut config = ai6::ServeConfig::new(listen)
                .with_plugin_paths(plugin_paths)
                .with_plugin_policy(plugin_policy)
                .with_max_concurrent_missions(max_concurrent_missions)
                .with_station_config(station_config)
                .with_watchdog(
                    ai6::WatchdogConfig::default()
                        .with_stall_timeout(stall_timeout)
                        .with_loop_timeout(loop_timeout)
                        .with_loop_min_lines(loop_min_lines)
                        .with_blocked_after(blocked_after)
                        .with_max_runtime(max_runtime)
                        .with_history(history)
                        .with_normalize_digits(normalize_digits),
                );
            if let Some(station) = station {
                config = config.with_station_id(station);
            }
            if let Some(config_dir) = config_dir {
                config = config.with_config_dir(config_dir);
            }
            if let Some(location) = roster_location {
                let options = sql_options(headquarters_lease, "AI6_CONFIG_PASSWORD");
                match ai6::directorates::m::config::open_with(&location, &options) {
                    Ok(source) => {
                        warn_weakenings("roster", &location);
                        config = config.with_config_source(source);
                    }
                    Err(error) => {
                        let location = ai6::directorates::registry::sql::redact(&location);
                        eprintln!("ai6: the roster at {location} cannot be opened: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            // A SQL Registry named only in the environment still gets timeouts derived from
            // this Station's Headquarters lease.
            let registry = registry.or_else(|| {
                environment_variable("AI6_REGISTRY")
                    .ok()
                    .flatten()
                    .filter(|location| {
                        ["sqlite:", "postgres://", "postgresql://"]
                            .iter()
                            .any(|scheme| location.starts_with(scheme))
                    })
            });
            if let Some(location) = registry {
                let options = sql_options(headquarters_lease, "AI6_REGISTRY_PASSWORD");
                match ai6::directorates::registry::backend::open_with(&location, &options) {
                    Ok(registry) => {
                        warn_weakenings("Registry", &location);
                        config = config.with_registry(registry);
                    }
                    Err(error) => {
                        let location = ai6::directorates::registry::sql::redact(&location);
                        eprintln!("ai6: the Registry at {location} cannot be opened: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            if let Some(publisher) = publisher {
                config = config.with_publisher(publisher);
            }
            match ai6::serve_with_options(config, shutdown_signal()).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("ai6: daemon service failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// The provider library policy: the pin set `--plugin-pins` names (which must exist), else
/// `plugins.toml` in the configuration directory; verified copies under `state_dir` (default:
/// the state directory)'s `plugins/verified`.
fn plugin_policy(
    args: &PinArgs,
    config_dir: Option<&std::path::Path>,
    state_dir: Option<&std::path::Path>,
) -> Result<ai6::station::plugins::PluginPolicy, String> {
    let (pins, profile) = (args.plugin_pins.clone(), args.plugin_profile);
    let default_pins = match config_dir {
        Some(dir) => Some(dir.join("plugins.toml")),
        None => ai6::directorates::m::roster::config_dir()
            .ok()
            .map(|dir| dir.join("plugins.toml")),
    };
    let state = match state_dir {
        Some(dir) => dir.to_path_buf(),
        None => ai6::station::safehouse::state_dir().map_err(|error| error.to_string())?,
    };
    ai6::station::plugins::PluginPolicy::load(
        pins.as_deref(),
        default_pins.as_deref(),
        profile,
        state.join("plugins").join("verified"),
    )
    .map_err(|error| error.to_string())
}

/// Reads the hook input, asks the gate, and prints a denial when it holds the command; a
/// failure to read the input or to decide denies too.
async fn green_ink_gate(cable_url: Option<String>) -> ExitCode {
    use ai6::directorates::section_00::gate::{GateClient, deny, pre_tool_use};
    use std::io::Read;
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        println!(
            "{}",
            deny("Green ink: the hook input could not be read, so the command is held.")
        );
        return ExitCode::SUCCESS;
    }
    let decision = tokio::task::spawn_blocking(move || {
        let client = GateClient::from_process_environment(cable_url);
        pre_tool_use(&input, client.as_ref())
    })
    .await
    .unwrap_or_else(|_| {
        Some(deny(
            "Green ink: the gate failed while deciding, so the command is held.",
        ))
    });
    if let Some(decision) = decision {
        println!("{decision}");
    }
    ExitCode::SUCCESS
}

/// Reads the hook input and prints a denial when the fetch's host is one of `deny`, or not one
/// of `hosts` when it is given; a failure to read the input denies too.
fn assistant_web_gate(hosts: &[String], deny: &[String]) -> ExitCode {
    use ai6::directorates::section_00::gate::web_fetch_gate;
    use std::io::Read;
    let mut input = String::new();
    // Unreadable input is not hook input, which the gate denies.
    if std::io::stdin().read_to_string(&mut input).is_err() {
        input.clear();
    }
    if let Some(decision) = web_fetch_gate(&input, hosts, deny) {
        println!("{decision}");
    }
    ExitCode::SUCCESS
}

async fn runner_command(command: RunnerCommand) -> ExitCode {
    let resolve = |state_dir: Option<PathBuf>| match state_dir {
        Some(dir) => Ok(dir),
        None => ai6::station::runner::default_state_dir(),
    };
    match command {
        RunnerCommand::Serve(args) => {
            let state_dir = match resolve(args.state_dir) {
                Ok(dir) => dir,
                Err(error) => {
                    eprintln!("ai6: runner state directory: {error}");
                    return ExitCode::FAILURE;
                }
            };
            if args.detach {
                let mut arguments: Vec<std::ffi::OsString> =
                    vec!["--state-dir".into(), state_dir.clone().into()];
                if let Some(listen) = args.listen {
                    arguments.extend(["--listen".into(), listen.to_string().into()]);
                }
                for path in &args.plugin_paths {
                    arguments.extend(["--plugin-path".into(), path.into()]);
                }
                if let Some(terminal) = &args.terminal {
                    arguments.extend(["--terminal".into(), terminal.into()]);
                }
                if let Some(pins) = &args.pins.plugin_pins {
                    arguments.extend(["--plugin-pins".into(), pins.into()]);
                }
                if let Some(profile) = args.pins.plugin_profile {
                    arguments.extend(["--plugin-profile".into(), profile.as_str().into()]);
                }
                let started = std::env::current_exe().and_then(|program| {
                    ai6::station::runner::spawn_detached(&program, &arguments, &state_dir)
                });
                return match started {
                    Ok(info) => {
                        println!(
                            "{}",
                            serde_json::to_string(&info).unwrap_or_else(|_| info.url.clone())
                        );
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("ai6: runner did not start: {error}");
                        ExitCode::FAILURE
                    }
                };
            }
            let policy = match plugin_policy(&args.pins, None, Some(&state_dir)) {
                Ok(policy) => policy,
                Err(error) => {
                    eprintln!("ai6: provider library pins: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let mut config = ai6::station::runner::RunnerConfig::new(&state_dir)
                .with_plugin_paths(args.plugin_paths)
                .with_plugin_policy(policy);
            if let Some(listen) = args.listen {
                config = config.with_listen(listen);
            }
            if let Some(terminal) = args.terminal {
                config = config.with_terminal(terminal);
            }
            match ai6::station::runner::serve(config, termination_signal()).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("ai6: runner failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        RunnerCommand::Call {
            state_dir,
            method,
            path,
        } => {
            let state_dir = match resolve(state_dir) {
                Ok(dir) => dir,
                Err(error) => {
                    eprintln!("ai6: runner state directory: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let body = match runner_request_body() {
                Ok(body) => body,
                Err(error) => {
                    eprintln!("ai6: runner request body: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let answered = tokio::task::spawn_blocking(move || {
                ai6::station::runner::call(&state_dir, &method, &path, body.as_ref())
            })
            .await;
            match answered {
                Ok(Ok((status, body))) => {
                    println!("{}", serde_json::json!({ "status": status, "body": body }));
                    ExitCode::SUCCESS
                }
                Ok(Err(error)) => {
                    eprintln!("ai6: {error}");
                    ExitCode::from(ai6::station::runner::CALL_NOT_RUNNING)
                }
                Err(error) => {
                    eprintln!("ai6: runner request failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// Runs `ai6 theater …`. Blocks on the Theater's transport.
fn theater_command(command: TheaterCommand) -> ExitCode {
    let TheaterCommand::Provision {
        theater,
        check,
        config_dir,
        plugin_paths,
        pins,
    } = command;
    let config_dir = match config_dir {
        Some(path) => path,
        None => match ai6::directorates::m::roster::config_dir() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("ai6: AI6 configuration: {error}");
                return ExitCode::FAILURE;
            }
        },
    };
    let found = ai6::Theaters::load(&config_dir)
        .map_err(|error| error.to_string())
        .and_then(|theaters| {
            theaters
                .find(&theater)
                .ok_or_else(|| format!("theaters.toml declares no Theater {theater:?}"))
        });
    let declared = match found {
        Ok(declared) => declared,
        Err(error) => {
            eprintln!("ai6: {error}");
            return ExitCode::FAILURE;
        }
    };
    let policy = match plugin_policy(&pins, Some(&config_dir), None) {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("ai6: provider library pins: {error}");
            return ExitCode::FAILURE;
        }
    };
    let plugins = ai6::station::plugins::Plugins::scan_with(&plugin_paths, &policy);
    ai6::station::plugins::log_scan(plugins.scan_summary());
    let client = match plugins.theater_transport(&declared.access) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("ai6: Theater {theater:?}: {error}");
            let scan = plugins.scan_summary();
            for problem in scan.failed.iter().chain(&scan.unreadable) {
                eprintln!("ai6: plugin path: {problem}");
            }
            return ExitCode::FAILURE;
        }
    };
    let steps = ai6::directorates::q::provision::provision(&client, &declared, check);
    let mut unfit = false;
    for step in &steps {
        println!(
            "{:<12} {:<18} {}",
            step.status.as_str(),
            step.name,
            step.detail
        );
        unfit |= match step.status {
            ai6::StepStatus::Failed => true,
            ai6::StepStatus::WouldChange => check,
            _ => false,
        };
    }
    if unfit {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The JSON body on standard input, if any. An interactive terminal is never read.
fn runner_request_body() -> Result<Option<serde_json::Value>, String> {
    use std::io::{IsTerminal, Read};
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut text = String::new();
    stdin
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("not JSON: {error}"))
}

/// Resolves on Ctrl-C, and on SIGTERM where there is one, so a service manager's stop is a
/// clean shutdown.
async fn termination_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = terminate.recv() => {}
                    _ = shutdown_signal() => {}
                }
            }
            Err(_) => shutdown_signal().await,
        }
    }
    #[cfg(not(unix))]
    shutdown_signal().await;
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| "duration must be a number of seconds".to_owned())?;
    Duration::try_from_secs_f64(seconds)
        .map_err(|_| "duration must be finite and non-negative".to_owned())
}

fn parse_station_label(value: &str) -> Result<(String, String), String> {
    let (key, value) = value
        .split_once('=')
        .ok_or_else(|| "station labels use KEY=VALUE".to_owned())?;
    if key.trim().is_empty() || value.trim().is_empty() {
        return Err("station label keys and values must be non-empty".to_owned());
    }
    Ok((key.to_owned(), value.to_owned()))
}

fn station_identity() -> Result<identity::Identity, String> {
    let state_dir = ai6::station::safehouse::state_dir().map_err(|error| error.to_string())?;
    identity::open_identity(&state_dir).map_err(|error| error.to_string())
}

fn sleeper_command(config_dir: Option<PathBuf>, command: SleeperCommand) -> Result<String, String> {
    let config_dir = match config_dir {
        Some(path) => path,
        None => ai6::directorates::m::roster::config_dir().map_err(|e| e.to_string())?,
    };
    let roster = ai6::Roster::load(&config_dir).map_err(|e| e.to_string())?;
    let registry =
        ai6::directorates::registry::backend::from_environment().map_err(|e| e.to_string())?;
    let find = |operation: &str, name: &str| -> Result<(String, ai6::Sleeper), String> {
        let operation = roster.operation(operation).map_err(|e| e.to_string())?;
        let sleeper = operation
            .sleepers
            .get(name)
            .ok_or_else(|| format!("Operation {} has no sleeper {name:?}", operation.id))?;
        Ok((operation.id.clone(), sleeper.clone()))
    };
    match command {
        SleeperCommand::List => {
            let mut output = String::new();
            for operation in roster.operations() {
                for (name, sleeper) in &operation.sleepers {
                    let state = match registry
                        .sleeper_authorization(&operation.id, name)
                        .map_err(|e| e.to_string())?
                    {
                        Some(approval) if approval.fingerprint == sleeper.fingerprint() => {
                            format!("authorized by {}", approval.authorized_by)
                        }
                        Some(_) => "changed since it was authorized".to_owned(),
                        None => "waiting for authorization".to_owned(),
                    };
                    output.push_str(&format!(
                        "{} {name}  [{}]  {}  {state}\n",
                        operation.id, sleeper.schedule, sleeper.title
                    ));
                }
            }
            if output.is_empty() {
                output.push_str("No sleepers are configured.\n");
            }
            Ok(output)
        }
        SleeperCommand::Authorize { operation, name } => {
            let (operation, sleeper) = find(&operation, &name)?;
            let by = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "operator".to_owned());
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs() as i64);
            registry
                .authorize_sleeper(&operation, &name, &sleeper, &by, now)
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "Authorized sleeper {name} of Operation {operation}: [{}] {}\n",
                sleeper.schedule, sleeper.title
            ))
        }
        SleeperCommand::Revoke { operation, name } => {
            let (operation, _) = find(&operation, &name)?;
            let had = registry
                .revoke_sleeper(&operation, &name)
                .map_err(|e| e.to_string())?;
            Ok(if had {
                format!("Revoked sleeper {name} of Operation {operation}\n")
            } else {
                format!("Sleeper {name} of Operation {operation} was not authorized\n")
            })
        }
    }
}

fn quarantine_command(command: QuarantineCommand) -> Result<String, String> {
    let registry =
        ai6::directorates::registry::backend::from_environment().map_err(|e| e.to_string())?;
    let scope = |operation: Option<String>, all: bool| -> Result<Option<String>, String> {
        match (operation, all) {
            (Some(operation), false) => Ok(Some(operation)),
            (None, true) => Ok(None),
            _ => Err("name an Operation with --operation, or pass --all".to_owned()),
        }
    };
    match command {
        QuarantineCommand::List => {
            let records = registry.quarantines().map_err(|e| e.to_string())?;
            if records.is_empty() {
                return Ok("No Agents are quarantined.\n".to_owned());
            }
            let mut output = String::from("AGENT\tOPERATION\tBY\tREASON\n");
            for record in records {
                output.push_str(&format!(
                    "{}\t{}\t{}\t{}\n",
                    record.agent,
                    record.operation.as_deref().unwrap_or("(all)"),
                    record.by,
                    record.reason
                ));
            }
            Ok(output)
        }
        QuarantineCommand::Add {
            agent,
            reason,
            operation,
            all,
        } => {
            let scope = if operation.is_none() && !all {
                None
            } else {
                scope(operation, all)?
            };
            let by = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "operator".to_owned());
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |elapsed| elapsed.as_secs_f64());
            let record = ai6::Quarantine::new(&agent, scope.clone(), reason, by, now);
            registry
                .quarantine_agent(&record)
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "Quarantined {agent} in {}\n",
                scope.as_deref().unwrap_or("every Operation")
            ))
        }
        QuarantineCommand::Release {
            agent,
            operation,
            all,
        } => {
            let scope = scope(operation, all)?;
            let had = registry
                .release_quarantine(&agent, scope.as_deref())
                .map_err(|e| e.to_string())?;
            Ok(if had {
                format!("Released {agent}\n")
            } else {
                format!("{agent} was not quarantined there\n")
            })
        }
    }
}

fn peer_command(command: PeerCommand) -> Result<String, String> {
    let state_dir = ai6::station::safehouse::state_dir().map_err(|error| error.to_string())?;
    let peers = identity::open_peers(&state_dir, &identity::Policy::modern())
        .map_err(|error| error.to_string())?;
    match command {
        PeerCommand::Accept { id } => {
            let id = id
                .parse::<identity::PeerId>()
                .map_err(|error| error.to_string())?;
            peers.accept(&id).map_err(|error| error.to_string())?;
            Ok(format!("Accepted station {id}\n"))
        }
        PeerCommand::Waiting => {
            let waiting = peers.waiting().map_err(|error| error.to_string())?;
            if waiting.is_empty() {
                Ok("No Stations are waiting for acceptance.\n".to_owned())
            } else {
                Ok(format!(
                    "Stations waiting for acceptance:\n{}\n",
                    waiting
                        .iter()
                        .map(|entry| match entry.source {
                            Some(source) => format!("  {}  from {source}", entry.id),
                            None => format!("  {}", entry.id),
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                ))
            }
        }
    }
}

fn publisher_config() -> Result<Option<ai6::station::publisher::Config>, String> {
    let origin = environment_variable("AI6_EXPORTS_URL")?;
    let token = environment_variable("AI6_EXPORTS_STATION_TOKEN")?;
    match (origin, token) {
        (None, None) => Ok(None),
        (Some(origin), Some(token)) => ai6::station::publisher::Config::try_new(&origin, token)
            .map(Some)
            .map_err(|error| error.to_string()),
        _ => Err("AI6_EXPORTS_URL and AI6_EXPORTS_STATION_TOKEN must be set together".to_owned()),
    }
}

/// Options for a SQL Registry or roster: timeouts from this Station's Headquarters lease,
/// and the password in `password_variable` when it is set.
fn sql_options(headquarters_lease: Duration, password_variable: &str) -> ai6::SqlOptions {
    let options = ai6::SqlOptions::for_lease(headquarters_lease);
    match environment_variable(password_variable) {
        Ok(Some(password)) if !password.is_empty() => options.with_password(password),
        _ => options,
    }
}

/// Says on standard error what a SQL location gives up against the defaults.
fn warn_weakenings(what: &str, location: &str) {
    for weakening in ai6::directorates::registry::sql::weakenings(location) {
        eprintln!(
            "ai6: warning: the {what} at {}: {weakening}",
            ai6::directorates::registry::sql::redact(location)
        );
    }
}

fn environment_variable(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} must contain valid text")),
    }
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("ai6: could not install the Ctrl-C handler: {error}");
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command, ServeArgs, parse_duration};

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn duration_parser_accepts_fractional_seconds_and_rejects_invalid_values() {
        assert_eq!(parse_duration("0.025").unwrap().as_millis(), 25);
        assert!(parse_duration("-1").is_err());
        assert!(parse_duration("NaN").is_err());
        assert!(parse_duration("invalid").is_err());
    }

    #[test]
    fn serve_cli_parses_watchdog_limits() {
        let cli = Cli::try_parse_from([
            "ai6",
            "serve",
            "--stall-timeout",
            "12.5",
            "--loop-timeout",
            "8",
            "--loop-min-lines",
            "7",
            "--blocked-after",
            "1.5",
            "--max-runtime",
            "60",
            "--history",
            "120",
            "--normalize-digits",
        ])
        .unwrap();
        let Command::Serve(args) = cli.command else {
            panic!("serve arguments should select the serve command");
        };
        let ServeArgs {
            stall_timeout,
            loop_timeout,
            loop_min_lines,
            blocked_after,
            max_runtime,
            history,
            normalize_digits,
            ..
        } = *args;
        assert_eq!(stall_timeout.as_secs_f64(), 12.5);
        assert_eq!(loop_timeout.as_secs(), 8);
        assert_eq!(loop_min_lines, 7);
        assert_eq!(blocked_after.as_millis(), 1500);
        assert_eq!(max_runtime.unwrap().as_secs(), 60);
        assert_eq!(history, 120);
        assert!(normalize_digits);
    }
}
