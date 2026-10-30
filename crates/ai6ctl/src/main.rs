//! Operator command-line client for the Universal Exports public API.

use std::net::IpAddr;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use reqwest::{Client, Method, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const DEFAULT_SERVER: &str = "http://127.0.0.1:7082";
const SERVER_ENV: &str = "UNEX_URL";
const ACCESS_TOKEN_ENV: &str = "UNEX_ACCESS_TOKEN";
const DEFAULT_CONTROL_WAIT_SECS: u64 = 10;
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Parser)]
#[command(
    name = "ai6ctl",
    about = "Inspect and control AI6 Stations, Agent roles, Missions and Sessions, and sign green ink, through the Universal Exports API",
    after_help = "The Universal Exports origin is --server, else UNEX_URL, else [unex] url of the ai6.cfg found as the ai6 command finds it (--config, AI6_CONFIG, the user's then the system's configuration directory), else http://127.0.0.1:7082. Commands other than health require a user bearer token in UNEX_ACCESS_TOKEN. Never use M or terminal-attachment credentials."
)]
struct Cli {
    /// Universal Exports API origin (default: UNEX_URL, else `[unex] url` of ai6.cfg, else
    /// http://127.0.0.1:7082).
    #[arg(long, env = SERVER_ENV, global = true)]
    server: Option<String>,
    /// The ai6.cfg to read `[unex] url` from, which must exist (default: AI6_CONFIG, else
    /// ai6.cfg in the user's, then the system's configuration directory).
    #[arg(long, value_name = "PATH", global = true)]
    config: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum OperationsCommand {
    /// Show one Operation: its parent and your actions, and with --effective its resolved
    /// policy and the Operation each key comes from (needs roster.read).
    Show {
        /// AI6 Operation ID.
        operation: String,
        /// Also show the resolved definition and where each key comes from.
        #[arg(long)]
        effective: bool,
    },
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check that the Universal Exports API is responding.
    Health,
    /// List visible Stations and their health and Headquarters status.
    Stations,
    /// Show one visible Station by its Station ID.
    Station {
        /// AI6 Station ID.
        station_id: String,
    },
    /// List the Agents of one Operation and their records there: Missions done, failed,
    /// blown, abandoned and stalled, incidents, and the model that occupies each codename
    /// (needs agent.read on the Operation).
    Agents {
        /// AI6 Operation ID.
        #[arg(long, global = true)]
        operation: Option<String>,
        #[command(subcommand)]
        command: Option<AgentCommand>,
    },
    /// List Agent roles available to one Operation.
    AgentRoles {
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
    },
    /// List registered Missions, optionally filtered by Operation.
    Missions {
        /// Keep Missions from this Operation.
        #[arg(long)]
        operation: Option<String>,
        /// With --operation, also keep Missions of every Operation under it.
        #[arg(long, requires = "operation")]
        subtree: bool,
    },
    /// List the Operations you may act on as a tree, each under its nearest Operation you
    /// may read, with your actions; or show one.
    Operations {
        #[command(subcommand)]
        command: Option<OperationsCommand>,
    },
    /// Show one Mission and its provider Sessions, or cancel, retry or resume it.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Mission {
        /// AI6 Mission ID.
        #[arg(required = true)]
        mission_id: Option<String>,
        /// Select an Operation when the Mission ID is not unique in the visible board.
        #[arg(long)]
        operation: Option<String>,
        #[command(subcommand)]
        control: Option<MissionCommand>,
    },
    /// List Sessions, optionally filtered by Operation, Mission or state.
    Sessions {
        /// Keep Sessions from this Operation.
        #[arg(long)]
        operation: Option<String>,
        /// Keep Sessions from this Mission.
        #[arg(long)]
        mission: Option<String>,
        /// Keep Sessions in this lifecycle state.
        #[arg(long)]
        state: Option<String>,
    },
    /// Show one provider Session by its Session ID, or interrupt it.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Session {
        /// Provider-issued Session ID.
        #[arg(required = true)]
        session_id: Option<String>,
        #[command(subcommand)]
        control: Option<SessionCommand>,
    },
    /// List the steps Agents wait to take, and approve, reject or revoke them (needs
    /// approval.decide on the Operation).
    GreenInk {
        #[command(subcommand)]
        command: GreenInkCommand,
    },
    /// List or revoke class authorisations, standing approvals of a kind of step (needs
    /// approval.class on the Operation).
    ClassAuthorizations {
        #[command(subcommand)]
        command: ClassCommand,
    },
    /// List the briefs vetting held, and release or refuse them (needs mission.create on the
    /// Operation).
    HeldBriefs {
        #[command(subcommand)]
        command: HeldCommand,
    },
    /// List the pool accounts an Operation may draw from: Sessions using each, quota used,
    /// until when it is limited and tokens today (needs pool.read on the Operation).
    Pools {
        /// AI6 Operation ID.
        #[arg(long, global = true)]
        operation: Option<String>,
        #[command(subcommand)]
        command: Option<PoolCommand>,
    },
}

/// Changes to an Agent's clearance in an Operation (need agent.configure; audited). Raising
/// one takes green ink on M's promotion request instead.
#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Hold an Agent at a clearance or below in the Operation.
    Lower {
        /// The Agent codename.
        agent: String,
        /// `desk`, `field` or `00`.
        clearance: String,
        /// Why, for the record.
        #[arg(long)]
        reason: String,
        /// Seconds it lasts; until restored when omitted.
        #[arg(long)]
        expires_in_secs: Option<u64>,
    },
    /// Remove the Agent's grant in the Operation, returning it to its configured clearance.
    Restore {
        /// The Agent codename.
        agent: String,
    },
}

/// Changes to a pool account.
#[derive(Debug, Subcommand)]
enum PoolCommand {
    /// Lift an account's provider limit so Sessions may use it again now (needs pool.manage
    /// on the Operation; audited).
    ClearLimit {
        /// The pool ID.
        pool: String,
        /// The account's name in the pool.
        account: String,
    },
}

/// Green ink: a person's approval of one irreversible step an Agent asked to take.
#[derive(Debug, Subcommand)]
enum GreenInkCommand {
    /// List the steps waiting for a decision, oldest first.
    List {
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
    },
    /// Approve a waiting step, signed by you.
    Approve {
        /// Green ink request ID.
        request_id: String,
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
        /// Seconds the approval lasts; one hour when omitted.
        #[arg(long)]
        expires_in: Option<u64>,
        /// What to tell the Agent.
        #[arg(long)]
        note: Option<String>,
    },
    /// Refuse a waiting step.
    Reject {
        /// Green ink request ID.
        request_id: String,
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
        /// Why, for the Agent.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Withdraw an approval before it expires.
    Revoke {
        /// Green ink request ID.
        request_id: String,
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
    },
}

/// Class authorisations of one Operation.
#[derive(Debug, Subcommand)]
enum ClassCommand {
    /// List every class authorisation, oldest first, revoked and expired ones included.
    List {
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
    },
    /// Revoke a class authorisation.
    Revoke {
        /// Class authorisation ID.
        id: String,
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
    },
}

/// Briefs vetting held for an operator.
#[derive(Debug, Subcommand)]
enum HeldCommand {
    /// List the held briefs, oldest first, with what vetting found; never their text.
    List {
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
    },
    /// Return a held brief to the queue; vetting does not hold it again for the same findings.
    Release {
        /// The brief's Registry key.
        key: String,
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
    },
    /// File a held brief as failed without running it.
    Refuse {
        /// The brief's Registry key.
        key: String,
        /// AI6 Operation ID.
        #[arg(long)]
        operation: String,
        /// Why, for the record; never sent to the Source.
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct HeldBrief {
    key: String,
    brief: HeldBriefText,
    vetting: HeldVetting,
    held_at: f64,
}

#[derive(Debug, Deserialize)]
struct HeldBriefText {
    source: String,
    title: String,
}

#[derive(Debug, Deserialize)]
struct HeldVetting {
    findings: Vec<HeldFinding>,
}

#[derive(Debug, Deserialize)]
struct HeldFinding {
    kind: String,
    detail: String,
    related: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HeldDecision {
    key: String,
    state: String,
}

/// Mission lifecycle controls; the Station holding the Mission decides whether they apply.
#[derive(Debug, Subcommand)]
enum MissionCommand {
    /// Stop the Mission's live Session, if any, and abandon the Mission.
    Cancel {
        /// AI6 Mission ID.
        mission_id: String,
        #[command(flatten)]
        target: MissionTarget,
    },
    /// Queue a stalled or finished Mission again as a new attempt.
    Retry {
        /// AI6 Mission ID.
        mission_id: String,
        /// Give the new attempt to this qualified Agent instead of the one it had.
        #[arg(long)]
        agent: Option<String>,
        #[command(flatten)]
        target: MissionTarget,
    },
    /// Resume a parked, blocked or stalled Mission now.
    Resume {
        /// AI6 Mission ID.
        mission_id: String,
        /// Send this message to the Agent as it resumes.
        #[arg(long)]
        message: Option<String>,
        #[command(flatten)]
        target: MissionTarget,
    },
}

/// Session controls.
#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// Stop the Session's current turn; its Mission stays, blocked, until resumed.
    Interrupt {
        /// Provider-issued Session ID.
        session_id: String,
        #[command(flatten)]
        request: ControlOptions,
    },
}

#[derive(Debug, Args)]
struct MissionTarget {
    /// The Mission's Operation; found from the visible board when omitted.
    #[arg(long)]
    operation: Option<String>,
    #[command(flatten)]
    request: ControlOptions,
}

#[derive(Debug, Args)]
struct ControlOptions {
    /// Repeating a request with the same key returns the first one instead of asking again.
    #[arg(long)]
    idempotency_key: Option<String>,
    /// Seconds to wait for the Station's outcome; 0 returns once the control is queued.
    #[arg(long, default_value_t = DEFAULT_CONTROL_WAIT_SECS)]
    wait: u64,
}

#[derive(Debug, Deserialize)]
struct ControlStatus {
    control_id: String,
    operation_id: String,
    mission_id: String,
    action: String,
    station_id: String,
    status: String,
    state: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BoardSession {
    operation_id: String,
    mission_id: String,
    session_id: String,
    state: Option<String>,
    sequence: u64,
    #[serde(default)]
    exposure: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BoardMission {
    operation_id: String,
    mission_id: String,
    sessions: Vec<BoardSession>,
}

#[derive(Debug, Deserialize)]
struct StationBoard {
    stations: Vec<BoardStation>,
    headquarters_station: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BoardStation {
    station_id: String,
    healthy: bool,
    roles: Vec<String>,
    running: usize,
    capacity: usize,
    headquarters: bool,
    seen: f64,
    operations: Vec<String>,
    #[serde(default)]
    resources: Option<StationResources>,
}

/// The host figures a Station published; each is absent when the host did not give it.
#[derive(Debug, Deserialize)]
struct StationResources {
    cpus: Option<f64>,
    cpu_percent: Option<f64>,
    load_1m: Option<f64>,
    memory_total_mb: Option<u64>,
    memory_available_mb: Option<u64>,
    sampled_at: u64,
}

#[derive(Debug, Deserialize)]
struct Scope {
    kind: String,
    target: String,
}

#[derive(Debug, Deserialize)]
struct WaitingStep {
    id: String,
    mission: String,
    agent: String,
    scope: Scope,
    reason: String,
    expires_at: f64,
}

#[derive(Debug, Deserialize)]
struct Revocation {
    by: String,
}

#[derive(Debug, Deserialize)]
struct Decided {
    request: WaitingStep,
    decision: String,
    signer: String,
    expires_at: Option<f64>,
    revoked: Option<Revocation>,
}

#[derive(Debug, Deserialize)]
struct ClassAuthorization {
    id: String,
    kinds: Vec<String>,
    target_pattern: String,
    agents: Vec<String>,
    roles: Vec<String>,
    clearance_min: String,
    max_uses: Option<u32>,
    uses: u32,
    expires_at: f64,
    revoked: Option<Revocation>,
}

#[derive(Debug, Deserialize)]
struct AgentRole {
    id: String,
    purpose: String,
    policy: String,
    operation_override: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ai6ctl: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The origin to use: `server` (the flag or `UNEX_URL`), else `[unex] url` of the bootstrap
/// file the `ai6` command would read, else the loopback default. Nothing else of that file
/// is read, its Station token least of all.
fn origin(server: Option<&str>, config: Option<&std::path::Path>) -> Result<String, String> {
    if let Some(server) = server {
        return Ok(server.to_owned());
    }
    let located = ai6::bootstrap::Bootstrap::find(config).map_err(|error| error.to_string())?;
    Ok(located
        .value
        .unex
        .map_or_else(|| DEFAULT_SERVER.to_owned(), |unex| unex.url))
}

async fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let server = server_url(&origin(cli.server.as_deref(), cli.config.as_deref())?)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "could not initialize the HTTP client".to_owned())?;

    match cli.command {
        Command::Health => {
            let response = client
                .get(endpoint(&server, "/health"))
                .send()
                .await
                .map_err(|_| "could not contact Universal Exports".to_owned())?;
            if !response.status().is_success() {
                return Err(format!(
                    "Universal Exports returned HTTP {}",
                    response.status()
                ));
            }
            println!("Universal Exports is healthy");
        }
        Command::Stations => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let board = stations(&client, &server, &token).await?;
            print_stations(&board);
        }
        Command::Station { station_id } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let board = stations(&client, &server, &token).await?;
            let Some(station) = board
                .stations
                .iter()
                .find(|station| station.station_id == station_id)
            else {
                return Err("Station was not found".to_owned());
            };
            println!(
                "{}",
                station_row(station, board.headquarters_station.as_deref())
            );
        }
        Command::Agents { operation, command } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let operation = operation.ok_or("pass --operation")?;
            match command {
                None => {
                    let request = operation_request(
                        &client,
                        &server,
                        &token,
                        Method::GET,
                        &[&operation, "agents"],
                    )?;
                    let agents: Vec<AgentView> = send(request, "agent.read", "Agent list").await?;
                    print_agents(&agents);
                }
                Some(AgentCommand::Lower {
                    agent,
                    clearance,
                    reason,
                    expires_in_secs,
                }) => {
                    let request = operation_request(
                        &client,
                        &server,
                        &token,
                        Method::PUT,
                        &[&operation, "agents", &agent, "clearance"],
                    )?
                    .json(&serde_json::json!({
                        "clearance": clearance,
                        "reason": reason,
                        "expires_in_secs": expires_in_secs,
                    }));
                    let grant: ClearanceGrant =
                        send(request, "agent.configure", "clearance grant").await?;
                    println!(
                        "{}: held at {} in {}",
                        plain(&grant.agent),
                        plain(&grant.clearance),
                        plain(&operation)
                    );
                }
                Some(AgentCommand::Restore { agent }) => {
                    let request = operation_request(
                        &client,
                        &server,
                        &token,
                        Method::DELETE,
                        &[&operation, "agents", &agent, "clearance"],
                    )?;
                    let grant: ClearanceGrant =
                        send(request, "agent.configure", "clearance grant").await?;
                    println!(
                        "{}: grant to {} removed; configured clearance applies in {}",
                        plain(&grant.agent),
                        plain(&grant.clearance),
                        plain(&operation)
                    );
                }
            }
        }
        Command::AgentRoles { operation } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let roles = agent_roles(&client, &server, &token, &operation).await?;
            print_agent_roles(roles);
        }
        Command::Missions { operation, subtree } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let filter = operation.as_deref().map(|operation| (operation, subtree));
            print_missions(missions(&client, &server, &token, filter).await?);
        }
        Command::Operations { command } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let request = client
                .get(endpoint(&server, "/v1/user/profile"))
                .bearer_auth(&token);
            let profile: Profile = send(request, "a signed-in user", "profile").await?;
            match command {
                None => print_operation_tree(&profile.operations),
                Some(OperationsCommand::Show {
                    operation,
                    effective,
                }) => {
                    let access = profile
                        .operations
                        .iter()
                        .find(|access| access.operation_id == operation)
                        .ok_or_else(|| format!("no Operation {operation:?} you may act on"))?;
                    println!("operation\t{}", plain(&access.operation_id));
                    println!(
                        "parent\t{}",
                        access.parent.as_deref().map_or("-".to_owned(), plain)
                    );
                    println!("actions\t{}", access.actions.join(","));
                    if effective {
                        let mut url = server.clone();
                        url.path_segments_mut()
                            .map_err(|_| {
                                "Universal Exports URL cannot contain path segments".to_owned()
                            })?
                            .extend(["v1", "user", "roster", "operation", &operation, "effective"]);
                        let effective: EffectiveOperation = send(
                            client.get(url).bearer_auth(&token),
                            "roster.read on config:roster",
                            "effective Operation",
                        )
                        .await?;
                        print_effective(&effective);
                    }
                }
            }
        }
        Command::Mission {
            control: Some(command),
            ..
        } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let (mission_id, target, action, extra) = match command {
                MissionCommand::Cancel { mission_id, target } => {
                    (mission_id, target, "cancel", None)
                }
                MissionCommand::Retry {
                    mission_id,
                    agent,
                    target,
                } => (
                    mission_id,
                    target,
                    "retry",
                    agent.map(|agent| ("agent", agent)),
                ),
                MissionCommand::Resume {
                    mission_id,
                    message,
                    target,
                } => (
                    mission_id,
                    target,
                    "resume",
                    message.map(|message| ("message", message)),
                ),
            };
            let operation = match target.operation {
                Some(operation) => operation,
                None => mission_operation(&client, &server, &token, &mission_id).await?,
            };
            let mut body = serde_json::json!({ "action": action });
            if let Some((key, value)) = extra {
                body[key] = serde_json::Value::String(value);
            }
            let status = control(
                &client,
                &server,
                &token,
                (&operation, &mission_id),
                body,
                &target.request,
            )
            .await?;
            print_control(&status);
            control_verdict(&status)?;
        }
        Command::Session {
            control:
                Some(SessionCommand::Interrupt {
                    session_id,
                    request,
                }),
            ..
        } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let session = board(&client, &server, &token)
                .await?
                .into_iter()
                .find(|session| session.session_id == session_id)
                .ok_or_else(|| "Session was not found".to_owned())?;
            let body = serde_json::json!({ "action": "interrupt", "session_id": session_id });
            let status = control(
                &client,
                &server,
                &token,
                (&session.operation_id, &session.mission_id),
                body,
                &request,
            )
            .await?;
            print_control(&status);
            control_verdict(&status)?;
        }
        Command::Mission {
            mission_id,
            operation,
            control: None,
        } => {
            let mission_id = mission_id.ok_or_else(|| "a Mission ID is required".to_owned())?;
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let matches = missions(&client, &server, &token, None)
                .await?
                .into_iter()
                .filter(|mission| {
                    mission.mission_id == mission_id
                        && operation
                            .as_ref()
                            .is_none_or(|value| &mission.operation_id == value)
                })
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [] => return Err("Mission was not found".to_owned()),
                [mission] => print_mission(mission),
                _ => {
                    return Err(
                        "Mission ID is visible in multiple Stations; pass --operation".to_owned(),
                    );
                }
            }
        }
        Command::Sessions {
            operation,
            mission,
            state,
        } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let sessions = board(&client, &server, &token).await?;
            let selected = sessions.into_iter().filter(|session| {
                operation
                    .as_ref()
                    .is_none_or(|value| &session.operation_id == value)
                    && mission
                        .as_ref()
                        .is_none_or(|value| &session.mission_id == value)
                    && state
                        .as_ref()
                        .is_none_or(|value| session.state.as_ref() == Some(value))
            });
            print_sessions(selected);
        }
        Command::Pools { operation, command } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let operation = operation.ok_or("pass --operation")?;
            match command {
                None => {
                    let request = operation_request(
                        &client,
                        &server,
                        &token,
                        Method::GET,
                        &[&operation, "pools"],
                    )?;
                    let board: PoolBoard = send(request, "pool.read", "pool board").await?;
                    print_pools(&board, unix_now());
                }
                Some(PoolCommand::ClearLimit { pool, account }) => {
                    let request = operation_request(
                        &client,
                        &server,
                        &token,
                        Method::POST,
                        &[&operation, "pools", &pool, &account, "clear-limit"],
                    )?;
                    let response = request
                        .send()
                        .await
                        .map_err(|_| "could not contact Universal Exports".to_owned())?;
                    match response.status() {
                        status if status.is_success() => {
                            println!("{}/{}: limit cleared", plain(&pool), plain(&account));
                        }
                        StatusCode::FORBIDDEN => {
                            return Err("Universal Exports returned HTTP 403 Forbidden \
                                 (pool.manage on the Operation is required)"
                                .to_owned());
                        }
                        StatusCode::NOT_FOUND => {
                            return Err("Universal Exports returned HTTP 404 Not Found (no \
                                 such account declared for the Operation)"
                                .to_owned());
                        }
                        status => return Err(format!("Universal Exports returned HTTP {status}")),
                    }
                }
            }
        }
        Command::GreenInk { command } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            green_ink(&client, &server, &token, command).await?;
        }
        Command::ClassAuthorizations { command } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            class_authorizations(&client, &server, &token, command).await?;
        }
        Command::HeldBriefs { command } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            held_briefs(&client, &server, &token, command).await?;
        }
        Command::Session {
            session_id,
            control: None,
        } => {
            let session_id = session_id.ok_or_else(|| "a Session ID is required".to_owned())?;
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let sessions = board(&client, &server, &token).await?;
            let Some(session) = sessions
                .into_iter()
                .find(|session| session.session_id == session_id)
            else {
                return Err("Session was not found".to_owned());
            };
            print_sessions(std::iter::once(session));
        }
    }
    Ok(())
}

fn server_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "Universal Exports URL is invalid".to_owned())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(
            "Universal Exports URL must be an origin without credentials or a path".to_owned(),
        );
    }
    let local_http = url.scheme() == "http"
        && url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .strip_prefix('[')
                    .and_then(|host| host.strip_suffix(']'))
                    .unwrap_or(host)
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    if url.scheme() != "https" && !local_http {
        return Err(
            "Universal Exports URL must use HTTPS (HTTP is allowed for loopback)".to_owned(),
        );
    }
    Ok(url)
}

fn endpoint(server: &Url, path: &str) -> Url {
    let mut url = server.clone();
    url.set_path(path);
    url
}

fn access_token(value: Option<String>) -> Result<String, String> {
    value
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| format!("set {ACCESS_TOKEN_ENV} to a Universal Exports user bearer token"))
}

async fn board(
    client: &Client,
    server: &Url,
    access_token: &str,
) -> Result<Vec<BoardSession>, String> {
    let response = board_request(client, server, access_token)
        .send()
        .await
        .map_err(|_| "could not contact Universal Exports".to_owned())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Universal Exports returned HTTP {status}"));
    }
    response
        .json()
        .await
        .map_err(|_| "Universal Exports returned an invalid Session list".to_owned())
}

async fn missions(
    client: &Client,
    server: &Url,
    access_token: &str,
    filter: Option<(&str, bool)>,
) -> Result<Vec<BoardMission>, String> {
    let response = missions_request(client, server, access_token, filter)
        .send()
        .await
        .map_err(|_| "could not contact Universal Exports".to_owned())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Universal Exports returned HTTP {status}"));
    }
    response
        .json()
        .await
        .map_err(|_| "Universal Exports returned an invalid Mission list".to_owned())
}

async fn stations(
    client: &Client,
    server: &Url,
    access_token: &str,
) -> Result<StationBoard, String> {
    let response = stations_request(client, server, access_token)
        .send()
        .await
        .map_err(|_| "could not contact Universal Exports".to_owned())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Universal Exports returned HTTP {status}"));
    }
    response
        .json()
        .await
        .map_err(|_| "Universal Exports returned an invalid Station list".to_owned())
}

async fn agent_roles(
    client: &Client,
    server: &Url,
    access_token: &str,
    operation: &str,
) -> Result<Vec<AgentRole>, String> {
    let response = agent_roles_request(client, server, access_token, operation)?
        .send()
        .await
        .map_err(|_| "could not contact Universal Exports".to_owned())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Universal Exports returned HTTP {status}"));
    }
    response
        .json()
        .await
        .map_err(|_| "Universal Exports returned an invalid Agent role list".to_owned())
}

async fn green_ink(
    client: &Client,
    server: &Url,
    token: &str,
    command: GreenInkCommand,
) -> Result<(), String> {
    const PERMISSION: &str = "approval.decide";
    let (operation, request_id, action, body) = match command {
        GreenInkCommand::List { operation } => {
            let request = operation_request(
                client,
                server,
                token,
                Method::GET,
                &[&operation, "green-ink"],
            )?;
            let steps: Vec<WaitingStep> = send(request, PERMISSION, "green ink list").await?;
            print_waiting(&steps);
            return Ok(());
        }
        GreenInkCommand::Approve {
            request_id,
            operation,
            expires_in,
            note,
        } => {
            let mut body = serde_json::json!({});
            if let Some(seconds) = expires_in {
                body["expires_in_secs"] = seconds.into();
            }
            if let Some(note) = note {
                body["note"] = note.into();
            }
            (operation, request_id, "approve", body)
        }
        GreenInkCommand::Reject {
            request_id,
            operation,
            reason,
        } => {
            let mut body = serde_json::json!({});
            if let Some(reason) = reason {
                body["reason"] = reason.into();
            }
            (operation, request_id, "reject", body)
        }
        GreenInkCommand::Revoke {
            request_id,
            operation,
        } => (operation, request_id, "revoke", serde_json::json!({})),
    };
    let request = operation_request(
        client,
        server,
        token,
        Method::POST,
        &[&operation, "green-ink", &request_id, action],
    )?
    .json(&body);
    let decided: Decided = send(request, PERMISSION, "green ink decision").await?;
    print_decided(&decided);
    Ok(())
}

async fn class_authorizations(
    client: &Client,
    server: &Url,
    token: &str,
    command: ClassCommand,
) -> Result<(), String> {
    const PERMISSION: &str = "approval.class";
    let records = match command {
        ClassCommand::List { operation } => {
            let request = operation_request(
                client,
                server,
                token,
                Method::GET,
                &[&operation, "class-authorizations"],
            )?;
            send(request, PERMISSION, "class authorisation list").await?
        }
        ClassCommand::Revoke { id, operation } => {
            let request = operation_request(
                client,
                server,
                token,
                Method::DELETE,
                &[&operation, "class-authorizations", &id],
            )?;
            vec![send(request, PERMISSION, "class authorisation").await?]
        }
    };
    print_classes(&records, unix_now());
    Ok(())
}

async fn held_briefs(
    client: &Client,
    server: &Url,
    token: &str,
    command: HeldCommand,
) -> Result<(), String> {
    const PERMISSION: &str = "mission.create";
    let (operation, key, action, body) = match command {
        HeldCommand::List { operation } => {
            let request = operation_request(
                client,
                server,
                token,
                Method::GET,
                &[&operation, "held-briefs"],
            )?;
            let held: Vec<HeldBrief> = send(request, PERMISSION, "held brief list").await?;
            print_held(&held);
            return Ok(());
        }
        HeldCommand::Release { key, operation } => {
            (operation, key, "release", serde_json::json!({}))
        }
        HeldCommand::Refuse {
            key,
            operation,
            reason,
        } => {
            let mut body = serde_json::json!({});
            if let Some(reason) = reason {
                body["reason"] = reason.into();
            }
            (operation, key, "refuse", body)
        }
    };
    let request = operation_request(
        client,
        server,
        token,
        Method::POST,
        &[&operation, "held-briefs", &key, action],
    )?
    .json(&body);
    let decided: HeldDecision = send(request, PERMISSION, "held brief decision").await?;
    println!("{}\t{}", plain(&decided.key), plain(&decided.state));
    Ok(())
}

fn print_held(held: &[HeldBrief]) {
    println!("KEY\tSOURCE\tTITLE\tHELD_UNIX\tFINDINGS");
    for entry in held {
        let findings = entry
            .vetting
            .findings
            .iter()
            .map(|finding| match &finding.related {
                Some(related) => format!("{}: {} ({related})", finding.kind, finding.detail),
                None => format!("{}: {}", finding.kind, finding.detail),
            })
            .collect::<Vec<_>>()
            .join("; ");
        println!("{}", held_line(entry, &findings));
    }
}

/// One held brief's line. Its title and source are shown with invisible characters made
/// visible, since hidden text is what vetting most often holds a brief for.
fn held_line(entry: &HeldBrief, findings: &str) -> String {
    format!(
        "{}\t{}\t{}\t{:.0}\t{}",
        plain(&entry.key),
        plain(&reveal(&entry.brief.source)),
        plain(&reveal(&entry.brief.title)),
        entry.held_at,
        plain(findings),
    )
}

/// `text` with the characters AI6's vetting reports as hidden made visible: each as
/// `<U+XXXX>`, and a run of tag characters as the text it spells, `<tags: ...>` (tags U+E0020
/// to U+E007E stand for printable ASCII).
fn reveal(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut tags = String::new();
    for character in text.chars() {
        if ('\u{E0000}'..='\u{E007F}').contains(&character) {
            let code = u32::from(character) - 0xE0000;
            tags.push(
                char::from_u32(code)
                    .filter(|_| (0x20..=0x7E).contains(&code))
                    .unwrap_or('?'),
            );
            continue;
        }
        if !tags.is_empty() {
            shown.push_str(&format!("<tags: {}>", std::mem::take(&mut tags)));
        }
        if is_invisible(character) {
            shown.push_str(&format!("<U+{:04X}>", u32::from(character)));
        } else {
            shown.push(character);
        }
    }
    if !tags.is_empty() {
        shown.push_str(&format!("<tags: {tags}>"));
    }
    shown
}

/// Characters that render as nothing: direction controls and marks, zero-width characters,
/// variation selectors, fillers, invisible operators and the soft hyphen.
fn is_invisible(character: char) -> bool {
    matches!(
        character,
        '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200D}'
            | '\u{2060}'..='\u{2064}'
            | '\u{FEFF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{E0100}'..='\u{E01EF}'
            | '\u{180E}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{3164}'
            | '\u{FFA0}'
            | '\u{00AD}'
    )
}

/// A request under `/v1/user/operations/`, each of `segments` encoded as one path segment.
fn operation_request(
    client: &Client,
    server: &Url,
    access_token: &str,
    method: Method,
    segments: &[&str],
) -> Result<reqwest::RequestBuilder, String> {
    let mut url = server.clone();
    url.path_segments_mut()
        .map_err(|_| "Universal Exports URL cannot contain path segments".to_owned())?
        .extend(["v1", "user", "operations"])
        .extend(segments);
    Ok(client.request(method, url).bearer_auth(access_token))
}

/// Sends `request` and reads its JSON answer, naming what the refusals usually mean.
async fn send<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    permission: &str,
    what: &str,
) -> Result<T, String> {
    let response = request
        .send()
        .await
        .map_err(|_| "could not contact Universal Exports".to_owned())?;
    let status = response.status();
    if !status.is_success() {
        let detail = match status {
            StatusCode::FORBIDDEN => format!(" ({permission} on the Operation is required)"),
            StatusCode::NOT_FOUND => " (no such request or class authorisation)".to_owned(),
            StatusCode::CONFLICT => {
                " (it was already decided another way, expired, or is not an approval)".to_owned()
            }
            StatusCode::BAD_REQUEST => " (a value is out of bounds)".to_owned(),
            StatusCode::SERVICE_UNAVAILABLE => " (the gateway has no Registry)".to_owned(),
            _ => String::new(),
        };
        return Err(format!("Universal Exports returned HTTP {status}{detail}"));
    }
    response
        .json()
        .await
        .map_err(|_| format!("Universal Exports returned an invalid {what}"))
}

/// The pool board of one Operation, as Universal Exports serves it.
#[derive(Debug, Deserialize)]
struct PoolBoard {
    accounts: Vec<PoolAccount>,
}

#[derive(Debug, Deserialize)]
struct PoolAccount {
    pool: String,
    account: String,
    kind: String,
    max_concurrent: Option<usize>,
    active: Vec<serde_json::Value>,
    #[serde(default)]
    other_sessions: usize,
    windows: Vec<PoolWindow>,
    limited_until: Option<f64>,
    tokens_today: u64,
}

#[derive(Debug, Deserialize)]
struct PoolWindow {
    name: String,
    used_permille: Option<u16>,
}

/// One Agent of an Operation and its record there, as Universal Exports serves it.
#[derive(Debug, Deserialize)]
struct AgentView {
    agent: String,
    posted: bool,
    operation: AgentTally,
    #[serde(default)]
    every_operation: AgentTally,
    last_mission_at: Option<f64>,
    #[serde(default)]
    model_history: Vec<ModelTenure>,
    retirement: Option<RetirementNotice>,
    clearance: Option<String>,
    configured_clearance: Option<String>,
    promotion: Option<WaitingStep>,
}

/// An Agent's clearance grant in an Operation.
#[derive(Debug, Deserialize)]
struct ClearanceGrant {
    agent: String,
    clearance: String,
}

#[derive(Debug, Deserialize)]
struct RetirementNotice {
    retire_on: String,
    status: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AgentTally {
    done: u64,
    failed: u64,
    blown: u64,
    abandoned: u64,
    stalled: u64,
    lost: u64,
    quarantined: u64,
    budget_stops: u64,
    green_ink_rejected: u64,
}

#[derive(Debug, Deserialize)]
struct ModelTenure {
    model: Option<String>,
}

fn print_agents(agents: &[AgentView]) {
    println!(
        "AGENT\tPOSTED\tDONE\tFAILED\tBLOWN\tABANDONED\tSTALLED\tLOST\tQUARANTINED\tBUDGET_STOPS\tGREEN_INK_REJECTED\tLAST_MISSION_UNIX\tMODEL\tRETIREMENT\tCLEARANCE\tPROMOTION"
    );
    for view in agents {
        let tally = &view.operation;
        let clearance = match (&view.clearance, &view.configured_clearance) {
            (Some(clearance), Some(configured)) if clearance != configured => {
                format!("{}(configured:{})", plain(clearance), plain(configured))
            }
            (Some(clearance), _) => plain(clearance),
            (None, _) => "-".to_owned(),
        };
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            plain(&view.agent),
            view.posted,
            tally.done,
            tally.failed,
            tally.blown,
            tally.abandoned,
            tally.stalled,
            tally.lost,
            tally.quarantined + view.every_operation.quarantined,
            tally.budget_stops,
            tally.green_ink_rejected,
            view.last_mission_at
                .map_or_else(|| "-".to_owned(), |at| format!("{at:.0}")),
            view.model_history.last().map_or("-".to_owned(), |tenure| {
                tenure.model.as_deref().map_or("default".to_owned(), plain)
            }),
            view.retirement.as_ref().map_or("-".to_owned(), |notice| {
                format!("{}:{}", plain(&notice.status), plain(&notice.retire_on))
            }),
            clearance,
            view.promotion.as_ref().map_or("-".to_owned(), |request| {
                format!("{}:{}", plain(&request.id), plain(&request.scope.target))
            }),
        );
    }
}

fn print_pools(board: &PoolBoard, now: f64) {
    println!("POOL\tACCOUNT\tKIND\tSESSIONS\tWINDOWS\tLIMITED_UNTIL_UNIX\tTOKENS_TODAY");
    for account in &board.accounts {
        let using = account.active.len() + account.other_sessions;
        let sessions = match account.max_concurrent {
            Some(max) => format!("{using}/{max}"),
            None => using.to_string(),
        };
        let windows = account
            .windows
            .iter()
            .map(|window| match window.used_permille {
                Some(used) => format!("{}={}.{}%", plain(&window.name), used / 10, used % 10),
                None => format!("{}=?", plain(&window.name)),
            })
            .collect::<Vec<_>>();
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            plain(&account.pool),
            plain(&account.account),
            plain(&account.kind),
            sessions,
            if windows.is_empty() {
                "-".to_owned()
            } else {
                windows.join(",")
            },
            account
                .limited_until
                .filter(|until| *until > now)
                .map_or_else(|| "-".to_owned(), |until| format!("{until:.0}")),
            account.tokens_today,
        );
    }
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

fn print_waiting(steps: &[WaitingStep]) {
    println!("REQUEST\tMISSION\tAGENT\tSTEP\tTARGET\tEXPIRES_UNIX\tREASON");
    for step in steps {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{:.0}\t{}",
            plain(&step.id),
            plain(&step.mission),
            plain(&step.agent),
            plain(&step.scope.kind),
            plain(&step.scope.target),
            step.expires_at,
            plain(&step.reason),
        );
    }
}

fn print_decided(decided: &Decided) {
    println!("REQUEST\tSTEP\tTARGET\tDECISION\tSIGNER\tEXPIRES_UNIX\tREVOKED_BY");
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        plain(&decided.request.id),
        plain(&decided.request.scope.kind),
        plain(&decided.request.scope.target),
        plain(&decided.decision),
        plain(&decided.signer),
        decided
            .expires_at
            .map_or_else(|| "-".to_owned(), |at| format!("{at:.0}")),
        decided
            .revoked
            .as_ref()
            .map_or_else(|| "-".to_owned(), |revoked| plain(&revoked.by)),
    );
}

fn print_classes(records: &[ClassAuthorization], now: f64) {
    println!("CLASS\tKINDS\tTARGET\tAGENTS\tROLES\tCLEARANCE\tUSES\tEXPIRES_UNIX\tSTATE");
    for record in records {
        let uses = match record.max_uses {
            Some(max) => format!("{}/{max}", record.uses),
            None => record.uses.to_string(),
        };
        let state = if record.revoked.is_some() {
            "revoked"
        } else if record.expires_at <= now {
            "expired"
        } else if record.max_uses.is_some_and(|max| record.uses >= max) {
            "used up"
        } else {
            "live"
        };
        let list = |values: &[String]| {
            if values.is_empty() {
                "-".to_owned()
            } else {
                plain(&values.join(","))
            }
        };
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.0}\t{}",
            plain(&record.id),
            list(&record.kinds),
            plain(&record.target_pattern),
            list(&record.agents),
            list(&record.roles),
            plain(&record.clearance_min),
            uses,
            record.expires_at,
            state,
        );
    }
}

/// The Operation of a Mission on the visible board.
async fn mission_operation(
    client: &Client,
    server: &Url,
    access_token: &str,
    mission_id: &str,
) -> Result<String, String> {
    let mut operations = missions(client, server, access_token, None)
        .await?
        .into_iter()
        .filter(|mission| mission.mission_id == mission_id)
        .map(|mission| mission.operation_id)
        .collect::<Vec<_>>();
    operations.dedup();
    match operations.as_slice() {
        [operation] => Ok(operation.clone()),
        [] => Err("Mission is not on the visible board; pass --operation".to_owned()),
        _ => Err("Mission ID is visible in multiple Operations; pass --operation".to_owned()),
    }
}

/// Asks for a control and, unless told not to wait, follows it until the Station reports.
async fn control(
    client: &Client,
    server: &Url,
    access_token: &str,
    (operation, mission): (&str, &str),
    mut body: serde_json::Value,
    options: &ControlOptions,
) -> Result<ControlStatus, String> {
    body["idempotency_key"] = serde_json::Value::String(
        options
            .idempotency_key
            .clone()
            .unwrap_or_else(idempotency_key),
    );
    let response = control_request(client, server, access_token, operation, mission)?
        .json(&body)
        .send()
        .await
        .map_err(|_| "could not contact Universal Exports".to_owned())?;
    let mut status = control_response(response).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(options.wait);
    while matches!(status.status.as_str(), "queued" | "delivered")
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(CONTROL_POLL_INTERVAL).await;
        let response = control_status_request(client, server, access_token, &status)?
            .send()
            .await
            .map_err(|_| "could not contact Universal Exports".to_owned())?;
        status = control_response(response).await?;
    }
    Ok(status)
}

async fn control_response(response: reqwest::Response) -> Result<ControlStatus, String> {
    let status = response.status();
    if !status.is_success() {
        let detail = match status {
            StatusCode::FORBIDDEN => " (mission.control is required)",
            StatusCode::CONFLICT => {
                " (the idempotency key was used for another request, or the Session moved on)"
            }
            StatusCode::SERVICE_UNAVAILABLE => " (no Station can take the control now)",
            _ => "",
        };
        return Err(format!("Universal Exports returned HTTP {status}{detail}"));
    }
    response
        .json()
        .await
        .map_err(|_| "Universal Exports returned an invalid control status".to_owned())
}

/// A key unique to this invocation; pass `--idempotency-key` to make a repeat safe.
fn idempotency_key() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("ai6ctl-{}-{nanos}", std::process::id())
}

fn control_request(
    client: &Client,
    server: &Url,
    access_token: &str,
    operation: &str,
    mission: &str,
) -> Result<reqwest::RequestBuilder, String> {
    let mut url = server.clone();
    url.path_segments_mut()
        .map_err(|_| "Universal Exports URL cannot contain path segments".to_owned())?
        .extend([
            "v1",
            "user",
            "operations",
            operation,
            "missions",
            mission,
            "actions",
        ]);
    Ok(client.post(url).bearer_auth(access_token))
}

fn control_status_request(
    client: &Client,
    server: &Url,
    access_token: &str,
    status: &ControlStatus,
) -> Result<reqwest::RequestBuilder, String> {
    let mut url = server.clone();
    url.path_segments_mut()
        .map_err(|_| "Universal Exports URL cannot contain path segments".to_owned())?
        .extend([
            "v1",
            "user",
            "operations",
            &status.operation_id,
            "missions",
            &status.mission_id,
            "actions",
            &status.control_id,
        ]);
    Ok(client.get(url).bearer_auth(access_token))
}

fn print_control(status: &ControlStatus) {
    println!("CONTROL\tACTION\tMISSION\tSTATION\tSTATUS\tSTATE\tREASON");
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        plain(&status.control_id),
        plain(&status.action),
        plain(&status.mission_id),
        plain(&status.station_id),
        plain(&status.status),
        status
            .state
            .as_deref()
            .map_or_else(|| "-".to_owned(), plain),
        status
            .reason
            .as_deref()
            .map_or_else(|| "-".to_owned(), plain),
    );
}

/// Fails the command when the Station rejected the control or no Station took it in time.
fn control_verdict(status: &ControlStatus) -> Result<(), String> {
    match status.status.as_str() {
        "rejected" => Err(format!(
            "the Station rejected the control: {}",
            status.reason.as_deref().map_or_else(String::new, plain)
        )),
        "expired" => Err("no Station acted on the control before it expired".to_owned()),
        _ => Ok(()),
    }
}

fn board_request(client: &Client, server: &Url, access_token: &str) -> reqwest::RequestBuilder {
    client
        .get(endpoint(server, "/v1/board"))
        .bearer_auth(access_token)
}

/// `GET /v1/missions`, kept to one Operation (and with `true` the Operations under it).
fn missions_request(
    client: &Client,
    server: &Url,
    access_token: &str,
    filter: Option<(&str, bool)>,
) -> reqwest::RequestBuilder {
    let mut url = endpoint(server, "/v1/missions");
    if let Some((operation, subtree)) = filter {
        let mut query = url.query_pairs_mut();
        query.append_pair("operation", operation);
        if subtree {
            query.append_pair("subtree", "true");
        }
    }
    client.get(url).bearer_auth(access_token)
}

fn stations_request(client: &Client, server: &Url, access_token: &str) -> reqwest::RequestBuilder {
    client
        .get(endpoint(server, "/v1/stations"))
        .bearer_auth(access_token)
}

fn agent_roles_request(
    client: &Client,
    server: &Url,
    access_token: &str,
    operation: &str,
) -> Result<reqwest::RequestBuilder, String> {
    let mut url = server.clone();
    url.path_segments_mut()
        .map_err(|_| "Universal Exports URL cannot contain path segments".to_owned())?
        .extend(["v1", "user", "operations", operation, "agent-roles"]);
    Ok(client.get(url).bearer_auth(access_token))
}

fn print_stations(board: &StationBoard) {
    println!(
        "STATION\tHEALTH\tHQS\tRUNNING/CAPACITY\tROLES\tOPERATIONS\tSEEN_UNIX\t\
         CPU_PERCENT\tCPUS\tLOAD_1M\tMEM_FREE_MB\tMEM_TOTAL_MB\tSAMPLED_UNIX"
    );
    for station in &board.stations {
        println!(
            "{}",
            station_row(station, board.headquarters_station.as_deref())
        );
    }
}

/// One Station as a table row; a figure it did not report reads `unknown`.
fn station_row(station: &BoardStation, headquarters_station: Option<&str>) -> String {
    fn known<T: std::fmt::Display>(value: Option<T>) -> String {
        value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
    }
    let resources = station.resources.as_ref();
    format!(
        "{}\t{}\t{}\t{}/{}\t{}\t{}\t{:.0}\t{}\t{}\t{}\t{}\t{}\t{}",
        plain(&station.station_id),
        if station.healthy {
            "healthy"
        } else {
            "offline"
        },
        if station.headquarters || headquarters_station == Some(station.station_id.as_str()) {
            "yes"
        } else {
            "no"
        },
        station.running,
        station.capacity,
        plain(&station.roles.join(",")),
        plain(&station.operations.join(",")),
        station.seen,
        known(resources.and_then(|r| r.cpu_percent)),
        known(resources.and_then(|r| r.cpus)),
        known(resources.and_then(|r| r.load_1m)),
        known(resources.and_then(|r| r.memory_available_mb)),
        known(resources.and_then(|r| r.memory_total_mb)),
        known(resources.map(|r| r.sampled_at)),
    )
}

fn print_agent_roles(roles: impl IntoIterator<Item = AgentRole>) {
    println!("ROLE\tOPERATION_OVERRIDE\tPURPOSE\tPOLICY");
    for role in roles {
        println!(
            "{}\t{}\t{}\t{}",
            plain(&role.id),
            role.operation_override,
            plain(&role.purpose),
            plain(&role.policy),
        );
    }
}

/// The signed-in person's profile, as far as `operations` needs it.
#[derive(Debug, Deserialize)]
struct Profile {
    operations: Vec<OperationAccess>,
}

#[derive(Debug, Deserialize)]
struct OperationAccess {
    operation_id: String,
    #[serde(default)]
    parent: Option<String>,
    actions: Vec<String>,
}

/// An Operation as its Stations resolve it.
#[derive(Debug, Deserialize)]
struct EffectiveOperation {
    operation_id: String,
    lineage: Vec<String>,
    inherit: bool,
    definition: serde_json::Map<String, serde_json::Value>,
    origins: std::collections::BTreeMap<String, String>,
}

/// Each Operation indented under its parent, roots in ID order, with the person's actions.
fn print_operation_tree(operations: &[OperationAccess]) {
    fn print(operations: &[OperationAccess], parent: Option<&str>, depth: usize) {
        let mut children: Vec<&OperationAccess> = operations
            .iter()
            .filter(|access| {
                let listed = access
                    .parent
                    .as_deref()
                    .filter(|id| operations.iter().any(|other| other.operation_id == *id));
                listed == parent
            })
            .collect();
        children.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        for access in children {
            println!(
                "{}{}\t{}",
                "  ".repeat(depth),
                plain(&access.operation_id),
                access.actions.join(",")
            );
            // The profile names each Operation once, so the depth is bounded by its length.
            if depth < operations.len() {
                print(operations, Some(&access.operation_id), depth + 1);
            }
        }
    }
    println!("OPERATION\tACTIONS");
    print(operations, None, 0);
}

/// The lineage and each written key of `effective`, with the Operation it comes from.
fn print_effective(effective: &EffectiveOperation) {
    println!("lineage\t{}", effective.lineage.join(" > "));
    println!("inherit\t{}", effective.inherit);
    for (key, writer) in &effective.origins {
        let mut value = Some(&effective.definition);
        let mut leaf = None;
        for part in key.split('.') {
            leaf = value.and_then(|table| table.get(part));
            value = leaf.and_then(serde_json::Value::as_object);
        }
        let Some(leaf) = leaf else {
            continue;
        };
        let source = if *writer == effective.operation_id {
            "own".to_owned()
        } else {
            format!("from {}", plain(writer))
        };
        println!("{} = {}\t{source}", plain(key), plain(&leaf.to_string()));
    }
}

fn print_missions(missions: impl IntoIterator<Item = BoardMission>) {
    println!("STATION\tMISSION\tSESSIONS");
    for mission in missions {
        println!(
            "{}\t{}\t{}",
            plain(&mission.operation_id),
            plain(&mission.mission_id),
            mission.sessions.len()
        );
    }
}

fn print_mission(mission: &BoardMission) {
    println!("STATION\tMISSION\tSESSIONS");
    println!(
        "{}\t{}\t{}",
        plain(&mission.operation_id),
        plain(&mission.mission_id),
        mission.sessions.len()
    );
    println!("SESSION\tSTATE\tSEQUENCE\tEXPOSURE");
    for session in &mission.sessions {
        println!(
            "{}\t{}\t{}\t{}",
            plain(&session.session_id),
            session
                .state
                .as_deref()
                .map(plain)
                .unwrap_or_else(|| "awaiting lifecycle".to_owned()),
            session.sequence,
            exposure_column(&session.exposure)
        );
    }
}

fn print_sessions(sessions: impl IntoIterator<Item = BoardSession>) {
    println!("STATION\tMISSION\tSESSION\tSTATE\tSEQUENCE\tEXPOSURE");
    for session in sessions {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            plain(&session.operation_id),
            plain(&session.mission_id),
            plain(&session.session_id),
            session
                .state
                .as_deref()
                .map(plain)
                .unwrap_or_else(|| "awaiting lifecycle".to_owned()),
            session.sequence,
            exposure_column(&session.exposure)
        );
    }
}

/// The untrusted input a Session read, comma separated; `-` when it read none.
fn exposure_column(exposure: &[String]) -> String {
    if exposure.is_empty() {
        "-".to_owned()
    } else {
        exposure
            .iter()
            .map(|source| plain(source))
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn plain(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn server_urls_allow_https_and_loopback_http_only() {
        assert!(server_url("https://exports.example").is_ok());
        assert!(server_url("http://127.0.0.1:7082").is_ok());
        assert!(server_url("http://[::1]:7082").is_ok());
        assert!(server_url("http://exports.example").is_err());
        assert!(server_url("https://user:password@exports.example").is_err());
        assert!(server_url("https://exports.example/path").is_err());
    }

    #[test]
    fn a_station_row_prints_cpu_and_memory_and_unknown_for_what_was_not_reported() {
        let mut station: BoardStation = serde_json::from_value(serde_json::json!({
            "station_id": "worker", "healthy": true, "roles": ["execution"], "running": 1,
            "capacity": 4, "headquarters": false, "seen": 1_700_000_005.0, "operations": ["G"],
            "resources": {
                "cpus": 8.0, "cpu_percent": 23.5, "memory_total_mb": 16384,
                "memory_available_mb": 9000, "sampled_at": 1_700_000_000
            }
        }))
        .unwrap();
        assert_eq!(
            station_row(&station, None),
            "worker\thealthy\tno\t1/4\texecution\tG\t1700000005\t23.5\t8\tunknown\t9000\t16384\t1700000000"
        );
        station.resources = None;
        assert!(
            station_row(&station, Some("worker"))
                .ends_with("\tunknown\tunknown\tunknown\tunknown\tunknown\tunknown"),
            "a Station that reports no resources"
        );
    }

    #[test]
    fn terminal_controls_are_removed_from_table_values() {
        assert_eq!(plain("operation\u{1b}[31m"), "operation\u{fffd}[31m");
        assert_eq!(plain("mission\tvalue"), "mission\u{fffd}value");
    }

    #[test]
    fn access_token_must_be_present_and_nonempty() {
        assert!(access_token(None).is_err());
        assert!(access_token(Some(String::new())).is_err());
        assert!(access_token(Some(" \t ".to_owned())).is_err());
        assert_eq!(
            access_token(Some("user-token".to_owned())).unwrap(),
            "user-token"
        );
    }

    #[test]
    fn board_request_uses_the_authorized_api_with_a_user_bearer() {
        let server = server_url("https://exports.example").unwrap();
        let request = board_request(&Client::new(), &server, "user-token")
            .build()
            .unwrap();

        assert_eq!(request.url().path(), "/v1/board");
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .unwrap(),
            "Bearer user-token"
        );
    }

    #[test]
    fn missions_request_uses_the_authorized_api_with_a_user_bearer() {
        let server = server_url("https://exports.example").unwrap();
        let request = missions_request(&Client::new(), &server, "user-token", None)
            .build()
            .unwrap();

        assert_eq!(request.url().path(), "/v1/missions");
        let subtree = missions_request(&Client::new(), &server, "t", Some(("P", true)))
            .build()
            .unwrap();
        assert_eq!(subtree.url().query(), Some("operation=P&subtree=true"));
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .unwrap(),
            "Bearer user-token"
        );
    }

    #[test]
    fn stations_request_uses_the_authorized_api_with_a_user_bearer() {
        let server = server_url("https://exports.example").unwrap();
        let request = stations_request(&Client::new(), &server, "user-token")
            .build()
            .unwrap();

        assert_eq!(request.url().path(), "/v1/stations");
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .unwrap(),
            "Bearer user-token"
        );
    }

    #[test]
    fn agent_role_request_encodes_the_operation_and_uses_a_user_bearer() {
        let server = server_url("https://exports.example").unwrap();
        let request = agent_roles_request(&Client::new(), &server, "user-token", "op/alpha")
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            request.url().path(),
            "/v1/user/operations/op%2Falpha/agent-roles"
        );
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .unwrap(),
            "Bearer user-token"
        );
    }

    #[test]
    fn control_requests_encode_the_mission_path_and_use_a_user_bearer() {
        let server = server_url("https://exports.example").unwrap();
        let request = control_request(&Client::new(), &server, "user-token", "op/alpha", "m 1")
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().path(),
            "/v1/user/operations/op%2Falpha/missions/m%201/actions"
        );
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .unwrap(),
            "Bearer user-token"
        );
    }

    #[test]
    fn mission_and_session_keep_their_show_form_beside_their_controls() {
        let show = Cli::try_parse_from(["ai6ctl", "mission", "mission-007"]).unwrap();
        assert!(matches!(
            show.command,
            Command::Mission {
                mission_id: Some(_),
                control: None,
                ..
            }
        ));
        let cancel =
            Cli::try_parse_from(["ai6ctl", "mission", "cancel", "mission-007", "--wait", "0"])
                .unwrap();
        assert!(matches!(
            cancel.command,
            Command::Mission {
                control: Some(MissionCommand::Cancel { .. }),
                ..
            }
        ));
        let interrupt =
            Cli::try_parse_from(["ai6ctl", "session", "interrupt", "provider-session-007"])
                .unwrap();
        assert!(matches!(
            interrupt.command,
            Command::Session {
                control: Some(SessionCommand::Interrupt { .. }),
                ..
            }
        ));
        assert!(Cli::try_parse_from(["ai6ctl", "mission"]).is_err());
    }

    #[test]
    fn green_ink_requests_encode_each_segment_and_use_a_user_bearer() {
        let server = server_url("https://exports.example").unwrap();
        let request = operation_request(
            &Client::new(),
            &server,
            "user-token",
            Method::POST,
            &["op/alpha", "green-ink", "green-ink-1 2", "approve"],
        )
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().path(),
            "/v1/user/operations/op%2Falpha/green-ink/green-ink-1%202/approve"
        );
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .unwrap(),
            "Bearer user-token"
        );
        let approve = Cli::try_parse_from([
            "ai6ctl",
            "green-ink",
            "approve",
            "green-ink-1",
            "--operation",
            "G",
            "--expires-in",
            "600",
        ])
        .unwrap();
        assert!(matches!(
            approve.command,
            Command::GreenInk {
                command: GreenInkCommand::Approve {
                    expires_in: Some(600),
                    ..
                }
            }
        ));
        assert!(Cli::try_parse_from(["ai6ctl", "green-ink", "list"]).is_err());
        assert!(
            Cli::try_parse_from([
                "ai6ctl",
                "class-authorizations",
                "revoke",
                "class-1",
                "--operation",
                "G"
            ])
            .is_ok()
        );
    }

    #[test]
    fn held_brief_commands_name_the_operation_and_print_no_brief_text() {
        let refuse = Cli::try_parse_from([
            "ai6ctl",
            "held-briefs",
            "refuse",
            "source_github--org_repo_7",
            "--operation",
            "G",
            "--reason",
            "a mirror",
        ])
        .unwrap();
        assert!(matches!(
            refuse.command,
            Command::HeldBriefs {
                command: HeldCommand::Refuse {
                    reason: Some(_),
                    ..
                }
            }
        ));
        assert!(Cli::try_parse_from(["ai6ctl", "held-briefs", "list"]).is_err());
        let request = operation_request(
            &Client::new(),
            &server_url("https://exports.example").unwrap(),
            "user-token",
            Method::POST,
            &["G", "held-briefs", "source_github--org_repo_7", "release"],
        )
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(
            request.url().path(),
            "/v1/user/operations/G/held-briefs/source_github--org_repo_7/release"
        );
        let held: Vec<HeldBrief> = serde_json::from_value(serde_json::json!([{
            "key": "source_github--org_repo_7",
            "operation": "G",
            "brief": {"title": "Crash", "body": "secret words", "source": "source:github"},
            "vetting": {"verdict": "hold", "findings": [
                {"kind": "duplicate", "detail": "the same URL as other work", "related": "m-1"}
            ]},
            "held_at": 10.0
        }]))
        .unwrap();
        assert_eq!(held[0].vetting.findings[0].related.as_deref(), Some("m-1"));

        // A title hiding characters shows them by code; tag characters spell their text.
        let mut hidden: Vec<HeldBrief> = serde_json::from_value(serde_json::json!([{
            "key": "source_github--org_repo_8",
            "brief": {"title": "Fix\u{200B}it\u{E006F}\u{E006B}", "source": "source:github"},
            "vetting": {"findings": []},
            "held_at": 11.0
        }]))
        .unwrap();
        let line = held_line(&hidden.remove(0), "");
        assert!(line.contains("Fix<U+200B>it<tags: ok>"), "{line}");
        assert_eq!(
            reveal("caf\u{E9} \u{202E}x\u{00AD}\u{E006F}"),
            "caf\u{E9} <U+202E>x<U+00AD><tags: o>"
        );
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }
}
