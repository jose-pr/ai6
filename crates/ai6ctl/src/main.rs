//! Read-only command-line client for the Universal Exports public API.

use std::net::IpAddr;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use reqwest::{Client, Url};
use serde::Deserialize;

const DEFAULT_SERVER: &str = "http://127.0.0.1:7082";
const SERVER_ENV: &str = "AI6_EXPORTS_URL";
const ACCESS_TOKEN_ENV: &str = "AI6_ACCESS_TOKEN";

#[derive(Debug, Parser)]
#[command(
    name = "ai6ctl",
    about = "Inspect AI6 Stations, Agent roles, Missions and Sessions through the Universal Exports API",
    after_help = "Read commands require a user bearer token in AI6_ACCESS_TOKEN. Never use M or terminal-attachment credentials."
)]
struct Cli {
    /// Universal Exports API origin.
    #[arg(long, env = SERVER_ENV, default_value = DEFAULT_SERVER, global = true)]
    server: String,
    #[command(subcommand)]
    command: Command,
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
    },
    /// Show one Mission and its registered provider Sessions.
    Mission {
        /// AI6 Mission ID.
        mission_id: String,
        /// Select a Operation when the Mission ID is not unique in the visible board.
        #[arg(long)]
        operation: Option<String>,
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
    /// Show one provider Session by its Session ID.
    Session {
        /// Provider-issued Session ID.
        session_id: String,
    },
}

#[derive(Debug, Deserialize)]
struct BoardSession {
    operation_id: String,
    mission_id: String,
    session_id: String,
    state: Option<String>,
    sequence: u64,
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

async fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let server = server_url(&cli.server)?;
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
            print_station(station, board.headquarters_station.as_deref());
        }
        Command::AgentRoles { operation } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let roles = agent_roles(&client, &server, &token, &operation).await?;
            print_agent_roles(roles);
        }
        Command::Missions { operation } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let missions = missions(&client, &server, &token).await?;
            print_missions(missions.into_iter().filter(|mission| {
                operation
                    .as_ref()
                    .is_none_or(|value| &mission.operation_id == value)
            }));
        }
        Command::Mission {
            mission_id,
            operation,
        } => {
            let token = access_token(std::env::var(ACCESS_TOKEN_ENV).ok())?;
            let matches = missions(&client, &server, &token)
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
        Command::Session { session_id } => {
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
) -> Result<Vec<BoardMission>, String> {
    let response = missions_request(client, server, access_token)
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

fn board_request(client: &Client, server: &Url, access_token: &str) -> reqwest::RequestBuilder {
    client
        .get(endpoint(server, "/v1/board"))
        .bearer_auth(access_token)
}

fn missions_request(client: &Client, server: &Url, access_token: &str) -> reqwest::RequestBuilder {
    client
        .get(endpoint(server, "/v1/missions"))
        .bearer_auth(access_token)
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
    println!("STATION\tHEALTH\tHQS\tRUNNING/CAPACITY\tROLES\tOPERATIONS\tSEEN_UNIX");
    for station in &board.stations {
        print_station(station, board.headquarters_station.as_deref());
    }
}

fn print_station(station: &BoardStation, headquarters_station: Option<&str>) {
    println!(
        "{}\t{}\t{}\t{}/{}\t{}\t{}\t{:.0}",
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
    );
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
    println!("SESSION\tSTATE\tSEQUENCE");
    for session in &mission.sessions {
        println!(
            "{}\t{}\t{}",
            plain(&session.session_id),
            session
                .state
                .as_deref()
                .map(plain)
                .unwrap_or_else(|| "awaiting lifecycle".to_owned()),
            session.sequence
        );
    }
}

fn print_sessions(sessions: impl IntoIterator<Item = BoardSession>) {
    println!("STATION\tMISSION\tSESSION\tSTATE\tSEQUENCE");
    for session in sessions {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            plain(&session.operation_id),
            plain(&session.mission_id),
            plain(&session.session_id),
            session
                .state
                .as_deref()
                .map(plain)
                .unwrap_or_else(|| "awaiting lifecycle".to_owned()),
            session.sequence
        );
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
        let request = missions_request(&Client::new(), &server, "user-token")
            .build()
            .unwrap();

        assert_eq!(request.url().path(), "/v1/missions");
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
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }
}
