use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ai6",
    version,
    about = "The AI6 Mission daemon",
    after_help = "Optional Universal Exports publication reads AI6_EXPORTS_URL and AI6_EXPORTS_TOKEN from the environment."
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
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Local address for the daemon service.
    #[arg(long, default_value = "127.0.0.1:7081")]
    listen: SocketAddr,
    /// A directory or library path to scan for Guatiao AgentHandler providers.
    #[arg(long = "plugin-path")]
    plugin_paths: Vec<PathBuf>,
    /// AI6 configuration directory containing operations.toml and agents.toml.
    #[arg(long)]
    config_dir: Option<PathBuf>,
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
    /// Shared Registry identity for this daemon (defaults to host and process ID).
    #[arg(long)]
    station: Option<String>,
    /// Station roles: control elects Headquarters; execution runs assigned Missions.
    #[arg(long, value_parser = ["control", "execution"])]
    role: Vec<String>,
    /// Station placement label in KEY=VALUE form.
    #[arg(long = "label", value_name = "KEY=VALUE", value_parser = parse_station_label)]
    labels: Vec<(String, String)>,
    /// Pin the terminal provider agent harnesses run in (default: an available one, preferring `pty`).
    #[arg(long)]
    terminal: Option<String>,
}

#[derive(Debug, Subcommand)]
enum PeerCommand {
    /// Accept a certificate shown in the waiting peer list.
    Accept {
        /// Full certificate fingerprint printed by the connecting station.
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
                println!("{}", identity.id());
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
        Command::Serve(args) => {
            let ServeArgs {
                listen,
                plugin_paths,
                config_dir,
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
            } = *args;
            let publisher = match publisher_config() {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("ai6: Universal Exports configuration failed: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let mut station_config = ai6::StationConfig::default();
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
            if let Some(terminal) = terminal {
                station_config = station_config.with_terminal(terminal);
            }
            let mut config = ai6::ServeConfig::new(listen)
                .with_plugin_paths(plugin_paths)
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

fn station_identity() -> Result<ai6::StationIdentity, ai6::StationIdentityError> {
    let state_dir =
        ai6::station::safehouse::state_dir().map_err(|_| ai6::StationIdentityError::Path)?;
    ai6::StationIdentity::open_or_create(&state_dir)
}

fn peer_command(command: PeerCommand) -> Result<String, ai6::StationIdentityError> {
    let state_dir =
        ai6::station::safehouse::state_dir().map_err(|_| ai6::StationIdentityError::Path)?;
    let peers = ai6::PeerTrustStore::open(&state_dir)?;
    match command {
        PeerCommand::Accept { id } => {
            let id = id.parse::<ai6::StationFingerprint>()?;
            peers.accept(&id)?;
            Ok(format!("Accepted station {id}\n"))
        }
        PeerCommand::Waiting => {
            let ids = peers.waiting()?;
            if ids.is_empty() {
                Ok("No Stations are waiting for acceptance.\n".to_owned())
            } else {
                Ok(format!(
                    "Stations waiting for acceptance:\n{}\n",
                    ids.iter()
                        .map(|id| format!("  {id}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ))
            }
        }
    }
}

fn publisher_config() -> Result<Option<ai6::station::publisher::Config>, String> {
    let origin = environment_variable("AI6_EXPORTS_URL")?;
    let token = environment_variable("AI6_EXPORTS_TOKEN")?;
    match (origin, token) {
        (None, None) => Ok(None),
        (Some(origin), Some(token)) => ai6::station::publisher::Config::try_new(&origin, token)
            .map(Some)
            .map_err(|error| error.to_string()),
        _ => Err("AI6_EXPORTS_URL and AI6_EXPORTS_TOKEN must be set together".to_owned()),
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
