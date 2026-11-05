use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use ai6::bootstrap::{Bootstrap, Located, SecretRef, is_sql_location};
use ai6::station::identity;
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ai6",
    version,
    about = "The AI6 Mission daemon",
    after_help = "Settings needed before the configuration provider is reached come from ai6.cfg: --config, else AI6_CONFIG, else ai6.cfg in the user's then the system's configuration directory. Universal Exports publication is its [unex] url and station_token."
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
    Identity {
        #[command(flatten)]
        bootstrap: BootstrapArg,
    },
    /// Manage pinned station certificates.
    Peer {
        #[command(flatten)]
        bootstrap: BootstrapArg,
        #[command(subcommand)]
        command: PeerCommand,
    },
    /// Authorize or revoke standing orders (sleepers) of the configured Operations.
    Sleeper {
        #[command(flatten)]
        bootstrap: BootstrapArg,
        #[command(subcommand)]
        command: SleeperCommand,
    },
    /// List, add or release Agent quarantines in the shared Registry.
    Quarantine {
        #[command(flatten)]
        bootstrap: BootstrapArg,
        #[command(subcommand)]
        command: QuarantineCommand,
    },
    /// The bootstrap file this command reads.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
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
    /// Provider library keys, signatures and pins (`plugins.toml`).
    Plugins {
        #[command(subcommand)]
        command: PluginsCommand,
    },
    /// A checkout's harness configuration (`[operation.ID.harness]`).
    Harness {
        #[command(subcommand)]
        command: HarnessCommand,
    },
    /// Overlays loadouts and Operations issue (`[overlay.NAME]`).
    Overlay {
        #[command(subcommand)]
        command: OverlayCommand,
    },
}

/// Where the bootstrap file is.
#[derive(Debug, Args)]
struct BootstrapArg {
    /// The bootstrap file, which must exist (default: AI6_CONFIG, else ai6.cfg in the user's,
    /// then the system's configuration directory; with none, every setting at its default).
    #[arg(long, value_name = "PATH", global = true)]
    config: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print one section's JSON Schema, as its declaration states it: titles, descriptions,
    /// defaults, units and bounds. With no section, list every section and its title.
    Schema {
        /// A dotted path: `provider`, `registry.limits.sessions`, `oo.chat`, `record.station`,
        /// `record.unex.gateway`, `record.operation.issues`.
        section: Option<String>,
    },
    /// Print the bootstrap file in use, or `none (defaults)`.
    Path {
        #[command(flatten)]
        bootstrap: BootstrapArg,
    },
    /// Print one family's records at a scope as TOML, or the record KEY alone.
    Show {
        /// The family: operation, agent, role, assistant, cover, theater, loadout, sandbox,
        /// mcp, secret, pool, cables, plugin, station, tls, access or unex.
        family: String,
        /// One record's key.
        key: Option<String>,
        /// `station:<id>` for one Station's own records (default: the global ones).
        #[arg(long, default_value = "")]
        scope: String,
        #[command(flatten)]
        bootstrap: BootstrapArg,
    },
    /// Load a configuration directory's family files into the configuration provider, which
    /// must take changes (SQL); checked whole and refused whole.
    Import {
        /// The directory, laid out as a configuration directory.
        dir: PathBuf,
        /// Load its records at `station:<id>` (default: global).
        #[arg(long, default_value = "")]
        scope: String,
        #[command(flatten)]
        bootstrap: BootstrapArg,
    },
    /// Remove one record. Refused, listing them, while another record still names it; with
    /// --force it goes anyway, and the output and the audit record say what still names it.
    /// A file provider's comments are kept in the common shape of a TOML file.
    Rm {
        /// The family, as for show.
        family: String,
        /// The record's key.
        key: String,
        /// `station:<id>` for one Station's own record (default: the global one).
        #[arg(long, default_value = "")]
        scope: String,
        /// Remove it though another record names it. The provider's own checks still apply.
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        bootstrap: BootstrapArg,
    },
    /// Write the public ai6.cfg a web page reads beside index.html, from this host's file: the
    /// gateway's address ([unex] public_origin, else url) and what the page needs of [oo], and
    /// nothing else.
    Public {
        /// Where to write it (default: standard output); this host's own file is refused.
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        #[command(flatten)]
        bootstrap: BootstrapArg,
    },
    /// Write every family's global records, and each Station's own, as TOML family files.
    Export {
        /// The directory to write; files in it are replaced.
        dir: PathBuf,
        #[command(flatten)]
        bootstrap: BootstrapArg,
    },
}

#[derive(Debug, Subcommand)]
enum OverlayCommand {
    /// Read an overlay directory as a Station reads it, running nothing: print its files, each
    /// with its SHA-256, and the `pin` its `[overlay.NAME]` needs, the AOS tree digest. Exits
    /// non-zero without a pin when its overlay.cfg does not read or validate, or a skill fails
    /// the SKILL.md checks.
    Pin {
        /// The overlay's root directory.
        dir: PathBuf,
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
    /// Make a publisher's Ed25519 key in a new file, and print its `publisher` line and nothing
    /// of the key itself. It belongs on the host that builds libraries, never on a Station.
    Keygen {
        /// The file to write, which must not exist.
        #[arg(long, value_name = "PATH")]
        out: PathBuf,
    },
    /// Write FILE.sig beside each AI6 provider library: the key's signature of its file name and
    /// SHA-256, valid for that name alone.
    Sign {
        /// The file holding the private key (PKCS#8, DER or PEM).
        #[arg(long, value_name = "PATH")]
        key: PathBuf,
        /// Library files to sign.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Print the `plugins.toml` entry that pins each AI6 provider library by its SHA-256, or
    /// accepts what a publisher signed for its name.
    Pin {
        /// Accept whatever this key signed instead of one digest: `ed25519:HEX` or the private
        /// key's file. Anything it signed for that name then loads, older builds included.
        #[arg(long, value_name = "KEY")]
        publisher: Option<String>,
        /// Store the entries as `plugin` records of the configuration provider, which must take
        /// changes (SQL), in place of printing them: each replaces the record of its file name,
        /// and the other records stay. Prints each library's change.
        #[arg(long)]
        import: bool,
        /// With --import, `station:<id>` for one Station's own records (default: the global ones).
        #[arg(long, requires = "import")]
        scope: Option<String>,
        #[command(flatten)]
        bootstrap: BootstrapArg,
        /// Library files to pin.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Print what a strict scan would say about each library under the provider's `plugin`
    /// records, loading nothing and writing no copy. Exits 1 when one would be refused, or
    /// there is none to check.
    Check {
        #[command(flatten)]
        bootstrap: BootstrapArg,
        /// Libraries or directories to check (default: the bootstrap file's `[station.plugins] paths`).
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
        #[command(flatten)]
        bootstrap: BootstrapArg,
        /// A directory or library path to scan for the Theater's transport provider, besides
        /// the bootstrap file's `[station.plugins] paths`.
        #[arg(long = "plugin-path")]
        plugin_paths: Vec<PathBuf>,
        /// How provider libraries are checked: strict, report or off (default: `[station.plugins]
        /// profile`, else strict with `plugin` records and report without).
        #[arg(long, value_name = "PROFILE")]
        plugin_profile: Option<ai6::PluginProfile>,
    },
}

/// How a runner checks provider libraries before they load.
#[derive(Debug, Args)]
struct PinArgs {
    /// The pin set, as `[[library]]` entries; a file named here must exist (default:
    /// plugins.toml in the user's configuration directory, when there is one). A Station
    /// passes the pins it resolved.
    #[arg(long = "pins", value_name = "PATH")]
    plugin_pins: Option<PathBuf>,
    /// How provider libraries are checked: strict (only pinned or signed ones load, from a
    /// verified copy), report (all load; unpinned ones are reported) or off (default: strict
    /// with a pin set and report without).
    #[arg(long, value_name = "PROFILE")]
    plugin_profile: Option<ai6::PluginProfile>,
    /// Which verified copies a strict scan keeps besides those it loaded: a count, 0 for none of
    /// them, or `none` for no limit (default: 8). A Station passes its own `[station.plugins]
    /// keep_verified`.
    #[arg(long, value_name = "N|none")]
    keep_verified: Option<ai6::KeepVerified>,
}

#[derive(Debug, Subcommand)]
enum RunnerCommand {
    /// Serve the runner until interrupted; the harnesses it supervises end with it.
    Serve(RunnerServeArgs),
    /// Relay this process's standard input and output to the runner on this host over ACP,
    /// authenticated with the runner's token.
    ///
    /// A Station that reaches this host through a transport speaks ACP to the runner this way.
    /// A terminal is put in raw mode and `ai6-runner-connect-ready` is printed first. Exits 3
    /// when no runner answers.
    Connect {
        /// The runner's state directory (default: `runner` in AI6's state directory).
        #[arg(long)]
        state_dir: Option<PathBuf>,
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
    /// The settings a Station hands its runners, as JSON: the `[runner]` table and the
    /// `[providers.<id>]` tables of its `station` record; `-` reads them from standard input
    /// (default: every key at its default). They are recorded in the state directory, where
    /// `runner connect` reads them.
    #[arg(long, value_name = "PATH")]
    settings: Option<PathBuf>,
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
    #[command(flatten)]
    bootstrap: BootstrapArg,
    /// Where the configuration provider is: a configuration directory, `sqlite:PATH` or a
    /// `postgres://` URL (default: the bootstrap file's `[provider] location`, else the
    /// directory holding it, else the user's configuration directory).
    #[arg(long, value_name = "LOCATION")]
    provider: Option<String>,
    /// Where the shared Registry is: a directory, `sqlite:PATH` or a `postgres://` URL
    /// (default: `[registry] location`, else the provider's database when it is SQL, else
    /// `registry` in the state directory).
    #[arg(long, value_name = "LOCATION")]
    registry: Option<String>,
    /// Where the Station keeps its state (default: `state_dir`, else the platform's state
    /// directory).
    #[arg(long, value_name = "PATH")]
    state_dir: Option<PathBuf>,
    /// Station identity in the shared Registry (default: `station`, else kept in the state
    /// directory across restarts).
    #[arg(long)]
    station: Option<String>,
    /// Local address for the daemon service (default: `listen`, else 127.0.0.1:7081).
    #[arg(long)]
    listen: Option<SocketAddr>,
    /// A directory or library path to scan for Guatiao AgentHandler providers, besides the
    /// bootstrap file's `[station.plugins] paths`.
    #[arg(long = "plugin-path")]
    plugin_paths: Vec<PathBuf>,
    /// How provider libraries are checked: strict (only pinned or signed ones load, from a
    /// verified copy), report (all load; unpinned ones are reported) or off (default:
    /// `[station.plugins] profile`, else strict with `plugin` records and report without).
    #[arg(long, value_name = "PROFILE")]
    plugin_profile: Option<ai6::PluginProfile>,
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
        Command::Config {
            command:
                command @ (ConfigCommand::Schema { .. }
                | ConfigCommand::Show { .. }
                | ConfigCommand::Import { .. }
                | ConfigCommand::Rm { .. }
                | ConfigCommand::Public { .. }
                | ConfigCommand::Export { .. }),
        } => match config_command(command) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Config {
            command: ConfigCommand::Path { bootstrap },
        } => match ai6::bootstrap::SearchPath::new(
            ai6::bootstrap::FILE_NAME,
            ai6::bootstrap::CONFIG_ENV,
            bootstrap.config.as_deref(),
        )
        .locate()
        {
            Ok(Some(path)) => {
                println!("{}", path.display());
                ExitCode::SUCCESS
            }
            Ok(None) => {
                println!("none (defaults)");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Identity { bootstrap } => match station_identity(&bootstrap) {
            Ok(identity) => {
                println!("{}", identity.fingerprint());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: could not load station identity: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Peer { bootstrap, command } => match peer_command(&bootstrap, command) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: peer operation failed: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Sleeper { bootstrap, command } => match sleeper_command(&bootstrap, command) {
            Ok(output) => {
                print!("{output}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("ai6: sleeper operation failed: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Quarantine { bootstrap, command } => match quarantine_command(&bootstrap, command)
        {
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
        Command::Plugins { command } => {
            match tokio::task::spawn_blocking(move || plugins_command(command)).await {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("ai6: plugins command failed: {error}");
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
        Command::Overlay {
            command: OverlayCommand::Pin { dir },
        } => overlay_pin(&dir),
        Command::Theater { command } => {
            match tokio::task::spawn_blocking(move || theater_command(command)).await {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("ai6: theater command failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Serve(args) => serve(*args).await,
    }
}

/// Runs `ai6 serve`: the bootstrap file with the command line over it.
async fn serve(args: ServeArgs) -> ExitCode {
    let ServeArgs {
        bootstrap,
        provider,
        registry,
        state_dir,
        station,
        listen,
        plugin_paths,
        plugin_profile,
    } = args;
    let mut located = match load_bootstrap(&bootstrap) {
        Ok(located) => located,
        Err(error) => {
            eprintln!("ai6: {error}");
            return ExitCode::FAILURE;
        }
    };
    let settings = &mut located.value;
    settings.provider.location = provider.or(settings.provider.location.take());
    settings.registry.location = registry.or(settings.registry.location.take());
    let own = &mut settings.station;
    own.state_dir = state_dir.or(own.state_dir.take());
    own.id = station.or(own.id.take());
    own.listen = listen.or(own.listen);
    own.plugins.paths.extend(plugin_paths);
    own.plugins.profile = plugin_profile.or(own.plugins.profile);
    let opened = match open_station(&located) {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("ai6: {error}");
            return ExitCode::FAILURE;
        }
    };
    let publisher = match publisher_config(&located) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("ai6: Universal Exports configuration failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let weakenings = located.weakenings();
    for weakening in &weakenings {
        eprintln!("ai6: warning: {weakening}");
    }
    let station_config = ai6::StationConfig::default()
        .with_state_dir(&opened.state_dir)
        .with_protected_path(&opened.identity_dir)
        .with_sealing_dir(&opened.identity_dir)
        .with_protected_path(&opened.config_dir)
        .with_bootstrap_weakenings(weakenings)
        .with_config_poll(located.value.provider.poll())
        .with_headquarters_lease(opened.lease);
    let mut config = ai6::ServeConfig::new(located.listen())
        .with_plugin_paths(located.value.station.plugins.paths.clone())
        .with_station_config(station_config)
        .with_config_dir(&opened.config_dir)
        .with_secrets_dir(&opened.secrets_dir)
        .with_config_provider(opened.provider)
        .with_registry(opened.registry);
    if let Some(profile) = located.value.station.plugins.profile {
        config = config.with_plugin_profile(profile);
    }
    config = config.with_keep_verified(located.value.station.plugins.keep_verified());
    if let Some(station) = &located.value.station.id {
        config = config.with_station_id(station);
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

/// The bootstrap file `--config` names, else the one the search finds, else the defaults.
fn load_bootstrap(bootstrap: &BootstrapArg) -> Result<Located<Bootstrap>, String> {
    Bootstrap::find(bootstrap.config.as_deref()).map_err(|error| error.to_string())
}

/// What a Station opens from its bootstrap file before it serves.
struct Opened {
    provider: Arc<dyn ai6::ConfigProvider>,
    registry: Arc<dyn ai6::RegistryBackend>,
    /// Where Theaters, loadouts, Cables and pins are read until they are provider records:
    /// the provider's directory, else the bootstrap file's, else the user's.
    config_dir: PathBuf,
    state_dir: PathBuf,
    identity_dir: PathBuf,
    secrets_dir: PathBuf,
    lease: Duration,
}

fn open_station(located: &Located<Bootstrap>) -> Result<Opened, String> {
    let text = |error: ai6::bootstrap::BootstrapError| error.to_string();
    let lease = located.value.registry.headquarters_lease();
    if lease.is_zero() {
        return Err("[registry] headquarters_lease must be positive".to_owned());
    }
    let provider_location = located.provider_location().map_err(text)?;
    let provider = open_provider(&provider_location, &located.value)?;
    let registry = open_registry(located)?;
    let config_dir = if is_sql_location(&provider_location) {
        match located.dir() {
            Some(dir) => dir.to_path_buf(),
            None => ai6::bootstrap::platform_config_dir().map_err(text)?,
        }
    } else {
        PathBuf::from(&provider_location)
    };
    Ok(Opened {
        provider,
        registry,
        config_dir,
        state_dir: located.state_dir().map_err(text)?,
        identity_dir: located.identity_dir().map_err(text)?,
        secrets_dir: located.secrets_dir().map_err(text)?,
        lease,
    })
}

/// Runs `ai6 config rm`. Everything is read first, so a record that cannot be judged stays, and
/// the audit record is written before the removal, so no removal goes unrecorded; a refusal
/// after that is recorded too.
fn remove_config_record(
    (family, key, scope): (&str, &str, &ai6::ConfigScope),
    force: bool,
    bootstrap: &BootstrapArg,
) -> Result<String, String> {
    let record = ai6::ConfigRecord::from_name(family)
        .ok_or_else(|| format!("{family:?} is not a configuration family"))?;
    let located = load_bootstrap(bootstrap)?;
    let location = located
        .provider_location()
        .map_err(|error| error.to_string())?;
    let provider = open_provider(&location, &located.value)?;
    let names = ai6::directorates::m::config::references_to(provider.as_ref(), record, scope, key)
        .map_err(|error| error.to_string())?;
    let registry = open_registry(&located)?;
    let audit = |action: &str, target: String| {
        registry
            .record_audit(&cli_audit(action, target))
            .map_err(|error| format!("the audit record could not be written: {error}"))
    };
    let what = format!("{family} {key} at {scope}");
    let listed = names
        .iter()
        .map(|name| format!("  {name}"))
        .collect::<Vec<_>>()
        .join("\n");
    let joined = names
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if !names.is_empty() && !force {
        audit("roster.refused", format!("{what}: still named by {joined}"))?;
        return Err(format!(
            "{what} is still named by:\n{listed}\nchange those first, or pass --force"
        ));
    }
    let target = if names.is_empty() {
        what.clone()
    } else {
        format!("{what}, forced: still named by {joined}")
    };
    audit("roster.remove", target)?;
    match provider.remove_on_host(record, scope, key) {
        Ok(Some(_)) if names.is_empty() => Ok(format!("Removed {what}\n")),
        Ok(Some(_)) => {
            eprintln!("ai6: weakening: {what} removed by --force though still named");
            Ok(format!(
                "Removed {what}, by --force, though these still name it:\n{listed}\n"
            ))
        }
        Ok(None) => {
            audit("roster.refused", format!("{what}: gone already"))?;
            Err(format!("no {what}"))
        }
        Err(error) => {
            let reason = error.to_string();
            audit("roster.refused", format!("{what}: {reason}"))?;
            Err(reason)
        }
    }
}

/// An audit record of a roster edit made on this host, by the user running the command.
fn cli_audit(action: &str, target: String) -> ai6::AuditRecord {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
    ai6::AuditRecord {
        id: uuid::Uuid::new_v4().to_string(),
        actor: format!("cli:{user}"),
        action: action.to_owned(),
        operation: String::new(),
        resource: Some("config:roster".to_owned()),
        target: bounded_line(
            &target,
            ai6::station::limits::AuditLimits::default()
                .target_bytes
                .or_max(),
        ),
        at,
    }
}

/// `text` on one line, cut at a character boundary to fit `bytes`.
fn bounded_line(text: &str, bytes: usize) -> String {
    let mut line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if line.len() > bytes {
        let mut end = bytes - '…'.len_utf8();
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    line
}

/// Runs `ai6 config show|import|rm|export`.
fn config_command(command: ConfigCommand) -> Result<String, String> {
    use ai6::{ConfigProvider as _, ConfigRecord, ConfigScope};
    let scope_of = |text: &str| {
        ConfigScope::parse(text).ok_or_else(|| format!("--scope {text:?}: \"\" or station:<id>"))
    };
    let open = |bootstrap: &BootstrapArg| -> Result<Arc<dyn ai6::ConfigProvider>, String> {
        let located = load_bootstrap(bootstrap)?;
        let location = located
            .provider_location()
            .map_err(|error| error.to_string())?;
        open_provider(&location, &located.value)
    };
    match command {
        ConfigCommand::Schema { section } => match section {
            None => Ok(ai6::settings::sections()
                .into_iter()
                .map(|(name, title)| format!("{name}\t{title}\n"))
                .collect()),
            Some(section) => {
                let schema = ai6::settings::schema(&section).ok_or_else(|| {
                    format!("no section {section:?}; `ai6 config schema` lists them")
                })?;
                serde_json::to_string_pretty(&schema)
                    .map(|text| format!("{text}\n"))
                    .map_err(|error| error.to_string())
            }
        },
        ConfigCommand::Path { .. } => Ok(String::new()),
        ConfigCommand::Public { output, bootstrap } => public_config(output.as_deref(), &bootstrap),
        ConfigCommand::Rm {
            family,
            key,
            scope,
            force,
            bootstrap,
        } => remove_config_record((&family, &key, &scope_of(&scope)?), force, &bootstrap),
        ConfigCommand::Show {
            family,
            key,
            scope,
            bootstrap,
        } => {
            let record = ConfigRecord::from_name(&family)
                .ok_or_else(|| format!("{family:?} is not a configuration family"))?;
            let records = open(&bootstrap)?
                .definitions(record, &scope_of(&scope)?)
                .map_err(|error| error.to_string())?
                .records;
            let records: Vec<_> = match &key {
                Some(key) => records
                    .into_iter()
                    .filter(|(found, _)| found == key)
                    .collect(),
                None => records,
            };
            if key.is_some() && records.is_empty() {
                return Err(format!("no {family} record {:?}", key.unwrap_or_default()));
            }
            ai6::directorates::m::config::family_document(record, records)
                .map_err(|error| error.to_string())
        }
        ConfigCommand::Import {
            dir,
            scope,
            bootstrap,
        } => {
            let scope = scope_of(&scope)?;
            let station = match &scope {
                ConfigScope::Station(id) => id.clone(),
                _ => String::new(),
            };
            let source = ai6::FileProvider::new(&dir);
            // The directory must be acceptable on its own before anything is written.
            source.load("").map_err(|error| error.to_string())?;
            let target = open(&bootstrap)?;
            let mut records = Vec::new();
            for record in ai6::directorates::m::config::IMPORT_ORDER {
                let read = ai6::FileProvider::new(&dir)
                    .definitions(record, &ConfigScope::Global)
                    .map_err(|error| error.to_string())?;
                records.extend(
                    read.records
                        .into_iter()
                        .map(|(key, value)| (record, key, value)),
                );
            }
            target
                .put_all(&scope, &records)
                .map_err(|error| error.to_string())?;
            target.load(&station).map_err(|error| error.to_string())?;
            Ok(format!("Imported {} records at {scope}\n", records.len()))
        }
        ConfigCommand::Export { dir, bootstrap } => {
            let provider = open(&bootstrap)?;
            std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
            let written = ai6::directorates::m::config::export(provider.as_ref(), &dir)
                .map_err(|error| error.to_string())?;
            Ok(format!("Wrote {written} files to {}\n", dir.display()))
        }
    }
}

/// Runs `ai6 config public`. A key the allow list refused is named on standard error, so a person
/// who expected it to reach the page finds out; its value is not.
fn public_config(output: Option<&Path>, bootstrap: &BootstrapArg) -> Result<String, String> {
    let located = load_bootstrap(bootstrap)?;
    let public =
        ai6::bootstrap::public::public_file(&located.value).map_err(|error| error.to_string())?;
    for key in &public.left_out {
        eprintln!("ai6: {key} is not on the public allow list, so it is left out");
    }
    let Some(output) = output else {
        return Ok(public.text);
    };
    let same = located.path.as_deref().is_some_and(|own| {
        let canonical = |path: &Path| std::fs::canonicalize(path).ok();
        canonical(own).is_some() && canonical(own) == canonical(output)
    });
    if same {
        return Err(format!(
            "{} is this host's own file, which a public one must not replace",
            output.display()
        ));
    }
    std::fs::write(output, public.text)
        .map_err(|error| format!("{}: {error}", output.display()))?;
    Ok(format!("Wrote {}\n", output.display()))
}

/// The Station this command acts for: the bootstrap file's `[station] id`, else the ID a Station
/// keeps in its state directory, else none (`""`, the global records).
fn station_of(located: &Located<Bootstrap>, opened: &Opened) -> String {
    station_in(located, &opened.state_dir)
}

/// [`station_of`], given the state directory.
fn station_in(located: &Located<Bootstrap>, state_dir: &Path) -> String {
    if let Some(station) = &located.value.station.id {
        return station.clone();
    }
    std::fs::read_to_string(state_dir.join("station-id"))
        .map(|text| text.trim().to_owned())
        .unwrap_or_default()
}

/// Opens the configuration provider at `location` within the bootstrap file's `[provider]`
/// limits, connecting a SQL one with its `[registry]` timeouts and `[provider]` password.
fn open_provider(
    location: &str,
    bootstrap: &Bootstrap,
) -> Result<Arc<dyn ai6::ConfigProvider>, String> {
    let options = sql_options(
        &bootstrap.registry,
        bootstrap.provider.password.as_ref(),
        "provider",
    )?;
    let shown = ai6::directorates::registry::sql::redact(location);
    let provider =
        ai6::directorates::m::config::open_with(location, &options, bootstrap.provider.limits())
            .map_err(|error| {
                format!("the configuration provider at {shown} cannot be opened: {error}")
            })?;
    warn_weakenings("configuration provider", location);
    Ok(provider)
}

/// Opens the Registry the bootstrap file names.
fn open_registry(located: &Located<Bootstrap>) -> Result<Arc<dyn ai6::RegistryBackend>, String> {
    let location = located
        .registry_location()
        .map_err(|error| error.to_string())?;
    let options = sql_options(
        &located.value.registry,
        located.registry_password(),
        "Registry",
    )?
    .with_limits(located.value.registry.limits.clone());
    let registry =
        ai6::directorates::registry::backend::open_with(&location, &options).map_err(|error| {
            let location = ai6::directorates::registry::sql::redact(&location);
            format!("the Registry at {location} cannot be opened: {error}")
        })?;
    warn_weakenings("Registry", &location);
    Ok(registry)
}

/// The provider library policy of this host for `snapshot`: its `plugin` records, under
/// `[station.plugins] profile` when set; verified copies under the state directory's
/// `plugins/verified`.
fn plugin_policy(
    located: &Located<Bootstrap>,
    opened: &Opened,
    snapshot: &ai6::ConfigSnapshot,
) -> ai6::station::plugins::PluginPolicy {
    let policy = ai6::station::plugins::PluginPolicy::new(
        snapshot.settings.pins.clone(),
        opened.state_dir.join("plugins").join("verified"),
    )
    .with_keep_verified(located.value.station.plugins.keep_verified());
    match located.value.station.plugins.profile {
        Some(profile) => policy.with_profile(profile),
        None => policy,
    }
}

fn load_plugin_policy(
    pins: Option<&Path>,
    default_pins: Option<&Path>,
    profile: Option<ai6::PluginProfile>,
    state_dir: &Path,
) -> Result<ai6::station::plugins::PluginPolicy, String> {
    ai6::station::plugins::PluginPolicy::load(
        pins,
        default_pins,
        profile,
        state_dir.join("plugins").join("verified"),
    )
    .map_err(|error| error.to_string())
}

/// Runs `ai6 plugins keygen|sign|pin|check`. Neither a private key nor a command-line `KEY` that
/// turned out not to be one is ever printed.
fn plugins_command(command: PluginsCommand) -> ExitCode {
    use ai6::station::plugins::{self, PublisherSigner};
    let failed = |subject: &Path, error: std::io::Error| {
        eprintln!("ai6: {}: {error}", subject.display());
        ExitCode::FAILURE
    };
    match command {
        PluginsCommand::Keygen { out } => match PublisherSigner::generate(&out) {
            Ok(signer) => {
                println!("publisher = \"{}\"", signer.public_key());
                if cfg!(windows) {
                    eprintln!(
                        "ai6: {} takes the access list of its directory: keep it where only you can read it",
                        out.display()
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => failed(&out, error),
        },
        PluginsCommand::Sign { key, paths } => {
            let signer = match PublisherSigner::read(&key) {
                Ok(signer) => signer,
                Err(error) => return failed(&key, error),
            };
            for path in &paths {
                match signer.sign(path) {
                    Ok(signature) => println!("{}", signature.display()),
                    Err(error) => return failed(path, error),
                }
            }
            ExitCode::SUCCESS
        }
        PluginsCommand::Pin {
            publisher,
            import,
            scope,
            bootstrap,
            paths,
        } => {
            let publisher = match publisher.as_deref().map(publisher_of).transpose() {
                Ok(publisher) => publisher,
                Err(error) => {
                    eprintln!("ai6: --publisher: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let mut pins = ai6::PinSet::new();
            for path in &paths {
                let entry = match publisher {
                    Some(key) => plugins::publisher_entry(path, key),
                    None => plugins::pin_entry(path),
                };
                match entry {
                    Ok(entry) => pins = pins.with_library(entry),
                    Err(error) => return failed(path, error),
                }
            }
            for weakening in pins.weakenings() {
                eprintln!("ai6: warning: {weakening}");
            }
            if import {
                return match import_pins(&pins, scope.as_deref().unwrap_or_default(), &bootstrap) {
                    Ok(changes) => {
                        print!("{changes}");
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("ai6: {error}");
                        ExitCode::FAILURE
                    }
                };
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
        PluginsCommand::Check { bootstrap, paths } => match plugins_check(&bootstrap, paths) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("ai6: {error}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Runs `ai6 plugins pin --import`: stores each entry of `pins` as the `plugin` record of its file
/// name at `scope`, as one change, and returns a line per library saying whether it was added,
/// changed or already so. A record that already says the same is not written again.
fn import_pins(
    pins: &ai6::PinSet,
    scope: &str,
    bootstrap: &BootstrapArg,
) -> Result<String, String> {
    use ai6::{ConfigRecord, ConfigScope};
    let scope = ConfigScope::parse(scope)
        .ok_or_else(|| format!("--scope {scope:?}: \"\" or station:<id>"))?;
    let mut files: Vec<&str> = pins
        .libraries
        .iter()
        .map(|library| library.file.as_str())
        .collect();
    files.sort_unstable();
    if let Some(twice) = files.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(format!("two libraries are named {:?}", twice[0]));
    }
    let located = load_bootstrap(bootstrap)?;
    let location = located
        .provider_location()
        .map_err(|error| error.to_string())?;
    let provider = open_provider(&location, &located.value)?;
    let held = provider
        .definitions(ConfigRecord::Plugin, &scope)
        .map_err(|error| error.to_string())?;
    if !held.writable {
        return Err(format!(
            "the configuration provider at {} takes no changes: add the entries `ai6 plugins pin` \
             prints to its plugins file instead",
            ai6::directorates::registry::sql::redact(&location)
        ));
    }
    let mut report = String::new();
    let mut records = Vec::new();
    for library in &pins.libraries {
        let stored = held
            .records
            .iter()
            .find(|(file, _)| *file == library.file)
            .map(|(_, definition)| {
                let mut entry = definition.clone();
                if let Some(table) = entry.as_object_mut() {
                    table.insert("file".to_owned(), library.file.clone().into());
                }
                serde_json::from_value::<ai6::LibraryPin>(entry).map_or_else(
                    |_| "an entry it cannot read".to_owned(),
                    |old| pin_terms(&old),
                )
            });
        let now = pin_terms(library);
        match stored {
            Some(before) if before == now => {
                report.push_str(&format!("unchanged {}: {now}\n", library.file));
                continue;
            }
            Some(before) => {
                report.push_str(&format!("changed {}: {before} -> {now}\n", library.file));
            }
            None => report.push_str(&format!("added {}: {now}\n", library.file)),
        }
        let mut definition = serde_json::to_value(library).map_err(|error| error.to_string())?;
        if let Some(table) = definition.as_object_mut() {
            table.remove("file");
        }
        records.push((ConfigRecord::Plugin, library.file.clone(), definition));
    }
    if !records.is_empty() {
        provider
            .put_all(&scope, &records)
            .map_err(|error| error.to_string())?;
        let station = match &scope {
            ConfigScope::Station(id) => id.clone(),
            _ => String::new(),
        };
        provider.load(&station).map_err(|error| error.to_string())?;
    }
    report.push_str(&format!(
        "{} of {} plugin records written at {scope}; a Station applies them at restart\n",
        records.len(),
        pins.libraries.len()
    ));
    Ok(report)
}

/// What a pin-set entry accepts, in words: `pin sha256:..`, `publisher ed25519:..`, or both
/// joined by `or`, or by `and` under `require = "both"`.
fn pin_terms(library: &ai6::LibraryPin) -> String {
    let terms: Vec<String> = [
        library.pin.map(|pin| format!("pin {pin}")),
        library.publisher.map(|key| format!("publisher {key}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let joiner = if library.require == ai6::Require::Both {
        " and "
    } else {
        " or "
    };
    terms.join(joiner)
}

/// The publisher `--publisher` names: `ed25519:HEX`, else the public half of the key in that file.
fn publisher_of(text: &str) -> Result<ai6::PublisherKey, String> {
    if let Ok(key) = text.parse() {
        return Ok(key);
    }
    ai6::station::plugins::PublisherSigner::read(Path::new(text))
        .map(|signer| signer.public_key())
        .map_err(|error| format!("not ed25519:HEX, nor a key file ({})", error.kind()))
}

/// Prints the verdict on each library named, or on `[station.plugins] paths`, against the `plugin` records
/// of this host's Station. Opens the configuration provider and nothing else.
fn plugins_check(bootstrap: &BootstrapArg, paths: Vec<PathBuf>) -> Result<ExitCode, String> {
    let located = load_bootstrap(bootstrap)?;
    let location = located
        .provider_location()
        .map_err(|error| error.to_string())?;
    let provider = open_provider(&location, &located.value)?;
    let state_dir = located.state_dir().map_err(|error| error.to_string())?;
    let snapshot = provider
        .load(&station_in(&located, &state_dir))
        .map_err(|error| error.to_string())?;
    let paths = if paths.is_empty() {
        located.value.station.plugins.paths.clone()
    } else {
        paths
    };
    let summary =
        ai6::station::plugins::check_libraries(&paths, &snapshot.settings.pins.unwrap_or_default());
    for library in &summary.checked {
        println!("{}  {}  {}", library.path, library.digest, library.verdict);
    }
    for skipped in &summary.skipped {
        println!("skipped: {skipped}");
    }
    for problem in summary.failed.iter().chain(&summary.unreadable) {
        eprintln!("ai6: {problem}");
    }
    for weakening in &summary.weakened {
        eprintln!("ai6: warning: {weakening}");
    }
    if summary.checked.is_empty() {
        eprintln!("ai6: no AI6 provider library to check");
    }
    let refused = summary
        .checked
        .iter()
        .any(|library| !library.verdict.starts_with("accepted"));
    Ok(
        if refused
            || summary.checked.is_empty()
            || !summary.failed.is_empty()
            || !summary.unreadable.is_empty()
        {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        },
    )
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
            let settings = match args.settings.as_deref() {
                None => ai6::station::runner::RunnerHandoff::default(),
                Some(path) => {
                    let read = if path == std::path::Path::new("-") {
                        let mut bytes = Vec::new();
                        std::io::Read::read_to_end(&mut std::io::stdin(), &mut bytes)
                            .and_then(|_| ai6::station::runner::RunnerHandoff::parse(&bytes))
                    } else {
                        ai6::station::runner::RunnerHandoff::read(path)
                    };
                    match read {
                        Ok(settings) => settings,
                        Err(error) => {
                            eprintln!("ai6: runner settings {}: {error}", path.display());
                            return ExitCode::FAILURE;
                        }
                    }
                }
            };
            if args.detach {
                let mut arguments: Vec<std::ffi::OsString> =
                    vec!["--state-dir".into(), state_dir.clone().into()];
                if args.settings.is_some() {
                    // Settings read from standard input reach the detached runner as a file.
                    let path = state_dir.join(ai6::station::runner::SETTINGS_FILE);
                    let written = settings.to_json_bytes().and_then(|bytes| {
                        std::fs::create_dir_all(&state_dir)?;
                        std::fs::write(&path, bytes)
                    });
                    if let Err(error) = written {
                        eprintln!("ai6: runner settings {}: {error}", path.display());
                        return ExitCode::FAILURE;
                    }
                    arguments.extend(["--settings".into(), path.into()]);
                }
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
                    arguments.extend(["--pins".into(), pins.into()]);
                }
                if let Some(profile) = args.pins.plugin_profile {
                    arguments.extend(["--plugin-profile".into(), profile.as_str().into()]);
                }
                if let Some(keep) = args.pins.keep_verified {
                    arguments.extend(["--keep-verified".into(), keep.to_string().into()]);
                }
                let started = std::env::current_exe().and_then(|program| {
                    ai6::station::runner::spawn_detached(
                        &program,
                        &arguments,
                        &state_dir,
                        &settings.runner,
                    )
                });
                return match started {
                    Ok(info) => {
                        println!(
                            "{}",
                            serde_json::to_string(&info).unwrap_or_else(|_| info.address.clone())
                        );
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprintln!("ai6: runner did not start: {error}");
                        ExitCode::FAILURE
                    }
                };
            }
            let default_pins = ai6::bootstrap::platform_config_dir()
                .ok()
                .map(|dir| dir.join("plugins.toml"));
            let policy = match load_plugin_policy(
                args.pins.plugin_pins.as_deref(),
                default_pins.as_deref(),
                args.pins.plugin_profile,
                &state_dir,
            ) {
                Ok(policy) => policy,
                Err(error) => {
                    eprintln!("ai6: provider library pins: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let policy = match args.pins.keep_verified {
                Some(keep) => policy.with_keep_verified(keep),
                None => policy,
            };
            let mut config = ai6::station::runner::RunnerConfig::new(&state_dir)
                .with_plugin_paths(args.plugin_paths)
                .with_plugin_policy(policy)
                .with_limits(settings.runner)
                .with_provider_configs(settings.providers);
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
        RunnerCommand::Connect { state_dir } => {
            let state_dir = match resolve(state_dir) {
                Ok(dir) => dir,
                Err(error) => {
                    eprintln!("ai6: runner state directory: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let settings = match ai6::station::runner::RunnerHandoff::in_state_dir(&state_dir) {
                Ok(settings) => settings,
                Err(error) => {
                    eprintln!("ai6: runner settings: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let relayed = tokio::task::spawn_blocking(move || {
                ai6::station::runner::connect_stdio(&state_dir, &settings.runner)
            })
            .await;
            match relayed {
                Ok(Ok(())) => ExitCode::SUCCESS,
                Ok(Err(error)) => {
                    eprintln!("ai6: {error}");
                    ExitCode::from(ai6::station::runner::CONNECT_NOT_RUNNING)
                }
                Err(error) => {
                    eprintln!("ai6: runner connect failed: {error}");
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
        bootstrap,
        plugin_paths,
        plugin_profile,
    } = command;
    let mut located = match load_bootstrap(&bootstrap) {
        Ok(located) => located,
        Err(error) => {
            eprintln!("ai6: {error}");
            return ExitCode::FAILURE;
        }
    };
    let plugins = &mut located.value.station.plugins;
    plugins.paths.extend(plugin_paths);
    plugins.profile = plugin_profile.or(plugins.profile);
    let opened = match open_station(&located) {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("ai6: {error}");
            return ExitCode::FAILURE;
        }
    };
    let station = station_of(&located, &opened);
    let snapshot = match opened.provider.load(&station) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("ai6: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(declared) = snapshot.settings.theaters.find(&theater) else {
        eprintln!("ai6: the configuration declares no Theater {theater:?}");
        return ExitCode::FAILURE;
    };
    let policy = plugin_policy(&located, &opened, &snapshot);
    let plugins =
        ai6::station::plugins::Plugins::scan_with(&located.value.station.plugins.paths, &policy);
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
    let steps = ai6::directorates::q::provision::provision(
        &client,
        &declared,
        check,
        &snapshot.settings.station.limits().provision,
    );
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

fn station_identity(bootstrap: &BootstrapArg) -> Result<identity::Identity, String> {
    let located = load_bootstrap(bootstrap)?;
    let dir = located.identity_dir().map_err(|error| error.to_string())?;
    identity::open_identity(&dir).map_err(|error| error.to_string())
}

fn sleeper_command(bootstrap: &BootstrapArg, command: SleeperCommand) -> Result<String, String> {
    let located = load_bootstrap(bootstrap)?;
    let location = located
        .provider_location()
        .map_err(|error| error.to_string())?;
    let roster = open_provider(&location, &located.value)?
        .load("")
        .map_err(|e| e.to_string())?
        .roster;
    let registry = open_registry(&located)?;
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

fn quarantine_command(
    bootstrap: &BootstrapArg,
    command: QuarantineCommand,
) -> Result<String, String> {
    let registry = open_registry(&load_bootstrap(bootstrap)?)?;
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

fn peer_command(bootstrap: &BootstrapArg, command: PeerCommand) -> Result<String, String> {
    let located = load_bootstrap(bootstrap)?;
    let dir = located.identity_dir().map_err(|error| error.to_string())?;
    let opened = open_station(&located)?;
    let policy = opened
        .provider
        .load(&station_of(&located, &opened))
        .map_err(|error| error.to_string())?
        .settings
        .tls_policy()?;
    let peers = identity::open_peers(&dir, &policy).map_err(|error| error.to_string())?;
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

/// Publication to Universal Exports: `[unex] url` with `station_token`, trusting `ca_file` as well
/// as the public roots. Either alone is another program's and publishes nothing; a bad `ca_file` is
/// refused either way.
fn publisher_config(
    located: &Located<Bootstrap>,
) -> Result<Option<ai6::station::publisher::Config>, String> {
    let unex = &located.value.unex;
    let certificates = unex
        .ca_file
        .as_deref()
        .map(ai6::bootstrap::read_ca_file)
        .transpose()
        .map_err(|error| format!("[unex] ca_file: {error}"))?;
    let (Some(url), Some(token)) = (&unex.url, &unex.station_token) else {
        if unex.station_token.is_some() {
            eprintln!("ai6: [unex] station_token is set and url is not, so nothing is published");
        }
        return Ok(None);
    };
    let token = token
        .read()
        .map_err(|error| format!("[unex] station_token: {error}"))?;
    let config = ai6::station::publisher::Config::try_new(url, token.as_str())
        .map_err(|error| error.to_string())?;
    Ok(Some(match certificates {
        Some(certificates) => config.with_ca_certificates(&certificates),
        None => config,
    }))
}

/// Options for a SQL Registry or configuration provider: timeouts from `[registry]`, and the
/// password `password` refers to.
fn sql_options(
    registry: &ai6::bootstrap::RegistrySettings,
    password: Option<&SecretRef>,
    what: &str,
) -> Result<ai6::SqlOptions, String> {
    let options = ai6::SqlOptions::for_lease(registry.headquarters_lease())
        .with_busy_timeout(registry.sqlite_busy_timeout());
    match password {
        Some(password) => {
            let value = password
                .read()
                .map_err(|error| format!("the {what} password: {error}"))?;
            Ok(options.with_password(value.as_str()))
        }
        None => Ok(options),
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

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("ai6: could not install the Ctrl-C handler: {error}");
    }
}

/// `ai6 overlay pin DIR`.
fn overlay_pin(dir: &std::path::Path) -> ExitCode {
    let read = match ai6::directorates::q::overlays::read_overlay(dir, &Default::default()) {
        Ok(read) => read,
        Err(error) => {
            eprintln!("ai6: {}: {error}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    for (path, digest) in &read.digest.files {
        println!("# {digest}  {path}");
    }
    let mut usable = true;
    for issue in &read.issues {
        let blocks = !ai6::directorates::q::overlays::applies(std::slice::from_ref(issue));
        usable &= !blocks;
        eprintln!(
            "ai6: {}: overlay.cfg {}: {}{}",
            dir.display(),
            issue.at,
            issue.code,
            if blocks {
                ""
            } else {
                " (informational; it still applies)"
            }
        );
    }
    for (name, check) in &read.skills {
        if let Err(reason) = check {
            usable = false;
            eprintln!("ai6: {}: skill {name}: {reason}", dir.display());
        }
    }
    if !usable {
        return ExitCode::FAILURE;
    }
    println!("pin = \"{}\"", read.digest.digest);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }
}
