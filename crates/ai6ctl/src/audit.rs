//! `ai6ctl audit`: the audit log through the gateway's `GET /v1/user/audit`, which shows each
//! person only the records they hold `audit.read` for.

use clap::Subcommand;
use reqwest::{Client, Method, Url};
use serde::Deserialize;

use crate::{plain, send};

#[derive(Debug, Subcommand)]
pub(crate) enum AuditCommand {
    /// List audit records, newest first, as AT (Unix seconds), ACTOR, ACTION, RESOURCE and
    /// TARGET. Needs audit.read on what each record is about; a record you may not read is not
    /// listed.
    List {
        /// Only records of this Operation (needs audit.read on it).
        #[arg(long, conflicts_with = "station")]
        operation: Option<String>,
        /// Only records of this Station (needs audit.read on it).
        #[arg(long)]
        station: Option<String>,
        /// Only records whose action starts with this, such as `run.context` for what was read
        /// of a Run's transcript or `internal.read` for reads through a Station's API.
        #[arg(long, value_name = "PREFIX")]
        action: Option<String>,
        /// The most records to list (the gateway's page size when left out).
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(Debug, Deserialize)]
struct AuditRow {
    #[serde(default)]
    at: f64,
    #[serde(default)]
    actor: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    target: String,
    #[serde(default)]
    operation: String,
}

pub(crate) async fn run(
    client: &Client,
    server: &Url,
    token: &str,
    command: AuditCommand,
) -> Result<(), String> {
    let AuditCommand::List {
        operation,
        station,
        action,
        limit,
    } = command;
    let mut url = server.clone();
    url.path_segments_mut()
        .map_err(|_| "Universal Exports URL cannot contain path segments".to_owned())?
        .extend(["v1", "user", "audit"]);
    {
        let mut query = url.query_pairs_mut();
        if let Some(operation) = &operation {
            query.append_pair("operation_id", operation);
        }
        if let Some(station) = &station {
            query.append_pair("station_id", station);
        }
        if let Some(action) = &action {
            query.append_pair("action", action);
        }
        if let Some(limit) = limit {
            query.append_pair("limit", &limit.to_string());
        }
    }
    let request = client.request(Method::GET, url).bearer_auth(token);
    let rows: Vec<AuditRow> = send(request, "audit.read", "audit log").await?;
    println!("AT\tACTOR\tACTION\tRESOURCE\tTARGET");
    for row in &rows {
        let resource = row
            .resource
            .clone()
            .unwrap_or_else(|| format!("operation:{}", row.operation));
        println!(
            "{}\t{}\t{}\t{}\t{}",
            row.at as i64,
            plain(&row.actor),
            plain(&row.action),
            plain(&resource),
            plain(&row.target),
        );
    }
    Ok(())
}
