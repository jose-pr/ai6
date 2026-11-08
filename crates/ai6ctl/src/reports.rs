//! `ai6ctl reports`: file, list and show Reports through the gateway's `/v1/reports` routes. A
//! Report you are not read in to, or not cleared for, lists by its ID and markings alone.

use clap::{Args, Subcommand};
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};

use crate::access::{exchange, list, text};
use crate::plain;

#[derive(Debug, Subcommand)]
pub(crate) enum ReportsCommand {
    /// File a Report (needs mission.create on the Operation, or station.configure on the
    /// Station or on config:reports for m). It creates no Mission.
    File(Box<FileArgs>),
    /// List Reports of a scope, or those tied to a Mission, most urgent first.
    List {
        /// operation:ID, station:ID or m.
        #[arg(long, required_unless_present = "mission")]
        scope: Option<String>,
        /// Only those tied to this Mission.
        #[arg(long)]
        mission: Option<String>,
        /// open, assessed or closed.
        #[arg(long)]
        state: Option<String>,
    },
    /// Show one Report as far as you may read it.
    Show {
        /// Its ID.
        id: String,
    },
    /// Who is cleared for which classification: list, grant or revoke (access.read, and
    /// access.manage to change, on config:reports). A change holds from the next request.
    Clearance {
        #[command(subcommand)]
        command: ClearanceCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ClearanceCommand {
    /// List each subject and the classification it reads up to.
    List,
    /// Clear a subject up to a classification, replacing what it had.
    Grant {
        /// person:ID, group:NAME or agent:CODENAME.
        subject: String,
        /// One of the deployment's classifications.
        classification: String,
    },
    /// Take a subject's clearance away.
    Revoke {
        /// person:ID, group:NAME or agent:CODENAME.
        subject: String,
    },
}

/// What `ai6ctl reports file` takes.
#[derive(Debug, Args)]
pub(crate) struct FileArgs {
    /// operation:ID, station:ID or m.
    scope: String,
    /// One line naming it.
    #[arg(long)]
    title: String,
    /// What was observed.
    #[arg(long)]
    text: String,
    /// A short label; finding by default.
    #[arg(long)]
    kind: Option<String>,
    /// info, warning or critical; info by default.
    #[arg(long)]
    severity: Option<String>,
    /// From the deployment's priorities (critical, high, normal, low by default).
    #[arg(long)]
    priority: Option<String>,
    /// When it needs an answer by, as Unix seconds.
    #[arg(long)]
    deadline: Option<f64>,
    /// From the deployment's classifications; unset is the lowest.
    #[arg(long)]
    classification: Option<String>,
    /// From the deployment's handling marks.
    #[arg(long)]
    handling: Option<String>,
    /// Compartments a reader must each be read in to, comma separated.
    #[arg(long, value_delimiter = ',')]
    compartments: Vec<String>,
    /// A sanitised part, shown in its place to those cleared only for its marking.
    #[arg(long)]
    tearline: Option<String>,
    /// The tearline's classification, no higher than the Report's.
    #[arg(long, requires = "tearline")]
    tearline_classification: Option<String>,
    /// What it is tied to, as KIND:ID (mission, report, decision or evidence); repeatable.
    #[arg(long = "link")]
    links: Vec<String>,
}

fn address(server: &Url, segments: &[&str]) -> Result<Url, String> {
    let mut url = server.clone();
    url.path_segments_mut()
        .map_err(|_| "Universal Exports URL cannot contain path segments".to_owned())?
        .push("v1")
        .extend(segments);
    Ok(url)
}

fn link_of(text: &str) -> Result<Value, String> {
    let (kind, id) = text
        .split_once(':')
        .filter(|(kind, id)| !kind.is_empty() && !id.is_empty())
        .ok_or_else(|| format!("{text:?} is not a link: use KIND:ID"))?;
    Ok(json!({"kind": kind, "id": id}))
}

fn print_listed(listed: &Value) {
    println!("ID\tSCOPE\tPRIORITY\tSTATE\tSEVERITY\tMARKINGS\tTITLE");
    for report in listed["reports"].as_array().into_iter().flatten() {
        let title = match report["sight"].as_str() {
            Some("content") => text(&report["title"]),
            Some("tearline") => "(tearline)".to_owned(),
            _ => "(withheld)".to_owned(),
        };
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            text(&report["id"]),
            text(&report["scope"]),
            text(&report["priority"]),
            text(&report["state"]),
            text(&report["severity"]),
            markings(report),
            title,
        );
    }
}

fn markings(report: &Value) -> String {
    let mut marks = Vec::new();
    for key in ["classification", "handling"] {
        if let Some(mark) = report[key].as_str() {
            marks.push(plain(mark));
        }
    }
    let compartments = list(&report["compartments"]);
    if !compartments.is_empty() {
        marks.push(compartments);
    }
    marks.join("/")
}

pub(crate) async fn run(
    client: &Client,
    server: &Url,
    token: &str,
    command: ReportsCommand,
) -> Result<(), String> {
    match command {
        ReportsCommand::File(args) => {
            let FileArgs {
                scope,
                title,
                text: body,
                kind,
                severity,
                priority,
                deadline,
                classification,
                handling,
                compartments,
                tearline,
                tearline_classification,
                links,
            } = *args;
            let links = links
                .iter()
                .map(|link| link_of(link))
                .collect::<Result<Vec<_>, _>>()?;
            let mut filing = json!({
                "scope": scope,
                "title": title,
                "text": body,
                "kind": kind,
                "severity": severity,
                "priority": priority,
                "deadline": deadline,
                "classification": classification,
                "handling": handling,
                "compartments": compartments,
                "links": links,
            });
            if let Some(tearline) = tearline {
                filing["tearline"] =
                    json!({"text": tearline, "classification": tearline_classification});
            }
            let filed = exchange(
                client,
                token,
                Method::POST,
                address(server, &["reports"])?,
                None,
                Some(filing),
            )
            .await?;
            println!("{}\t{}", text(&filed["id"]), text(&filed["scope"]));
        }
        ReportsCommand::List {
            scope,
            mission,
            state,
        } => {
            let mut url = address(server, &["reports"])?;
            {
                let mut query = url.query_pairs_mut();
                for (key, value) in [("scope", scope), ("mission", mission), ("state", state)] {
                    if let Some(value) = value {
                        query.append_pair(key, &value);
                    }
                }
            }
            print_listed(&exchange(client, token, Method::GET, url, None, None).await?);
        }
        ReportsCommand::Clearance { command } => {
            run_clearance(client, server, token, command).await?;
        }
        ReportsCommand::Show { id } => {
            let report = exchange(
                client,
                token,
                Method::GET,
                address(server, &["reports", &id])?,
                None,
                None,
            )
            .await?;
            for key in [
                "id", "scope", "kind", "severity", "state", "priority", "source",
            ] {
                println!("{key}\t{}", text(&report[key]));
            }
            println!("markings\t{}", markings(&report));
            println!("reliability\t{}", text(&report["reliability"]));
            if report["untrusted"] == true {
                println!("untrusted\ttrue");
            }
            if report["sight"] == "tearline" {
                println!("shown\ttearline");
            } else {
                println!("title\t{}", text(&report["title"]));
            }
            println!();
            println!("{}", plain(report["text"].as_str().unwrap_or_default()));
        }
    }
    Ok(())
}

async fn run_clearance(
    client: &Client,
    server: &Url,
    token: &str,
    command: ClearanceCommand,
) -> Result<(), String> {
    match command {
        ClearanceCommand::List => {
            let listed = exchange(
                client,
                token,
                Method::GET,
                address(server, &["reports", "clearances"])?,
                None,
                None,
            )
            .await?;
            println!("SUBJECT\tCLASSIFICATION\tBY");
            for clearance in listed["clearances"].as_array().into_iter().flatten() {
                println!(
                    "{}\t{}\t{}",
                    text(&clearance["subject"]),
                    text(&clearance["classification"]),
                    text(&clearance["by"]),
                );
            }
        }
        ClearanceCommand::Grant {
            subject,
            classification,
        } => {
            let granted = exchange(
                client,
                token,
                Method::PUT,
                address(server, &["reports", "clearances", &subject])?,
                None,
                Some(json!({"classification": classification})),
            )
            .await?;
            println!(
                "granted\t{}\t{}",
                text(&granted["subject"]),
                text(&granted["classification"])
            );
        }
        ClearanceCommand::Revoke { subject } => {
            let revoked = exchange(
                client,
                token,
                Method::DELETE,
                address(server, &["reports", "clearances", &subject])?,
                None,
                None,
            )
            .await?;
            let word = if revoked["revoked"] == true {
                "revoked"
            } else {
                "none held"
            };
            println!("{word}\t{}", plain(&subject));
        }
    }
    Ok(())
}
