//! `ai6ctl memories`: the memory of a person, an Operation or everyone, through the gateway's
//! routes. What you may not see answers as not there; a refusal prints the gateway's reason.

use clap::Subcommand;
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};

use crate::access::{exchange, list, text, url};
use crate::plain;

#[derive(Debug, Subcommand)]
pub(crate) enum MemoriesCommand {
    /// Search what you may read, best first; with no words, the newest of one scope.
    Search {
        /// Words to look for.
        query: Option<String>,
        /// person:ID, operation:ID or global; everything you may read when left out.
        #[arg(long)]
        scope: Option<String>,
    },
    /// Show one entry with its content.
    Show {
        /// Its ID.
        id: String,
    },
    /// Add an entry (needs memory.write on the Operation, or on config:memory for global).
    Add {
        /// person:ID (your own), operation:ID or global.
        scope: String,
        /// One line saying what it is for.
        #[arg(long)]
        summary: String,
        /// Its text.
        #[arg(long)]
        content: String,
        /// A short name; it may repeat.
        #[arg(long)]
        name: Option<String>,
        /// Compartments that protect it, comma separated; you must be read into each.
        #[arg(long, value_delimiter = ',')]
        compartments: Option<Vec<String>>,
    },
    /// Change an entry from the revision you read; a person's edit clears its mark.
    Edit {
        /// Its ID.
        id: String,
        /// The revision you read.
        #[arg(long)]
        revision: u64,
        /// One line saying what it is for.
        #[arg(long)]
        summary: String,
        /// Its new text.
        #[arg(long)]
        content: String,
        /// A short name; it may repeat.
        #[arg(long)]
        name: Option<String>,
        /// Compartments that protect it, comma separated; left out, they stay.
        #[arg(long, value_delimiter = ',')]
        compartments: Option<Vec<String>>,
    },
    /// Forget an entry and its prior versions.
    Forget {
        /// Its ID.
        id: String,
        /// The revision you read, to refuse once it has moved on.
        #[arg(long)]
        revision: Option<u64>,
    },
    /// List what Agents proposed that you may decide.
    Proposals {
        /// person:ID, operation:ID or global; every scope when left out.
        #[arg(long)]
        scope: Option<String>,
    },
    /// Approve a proposal, or reject it with --reject.
    Decide {
        /// The proposal's ID.
        id: String,
        /// Drop it rather than carry it out.
        #[arg(long)]
        reject: bool,
    },
}

/// `source` in a line: who wrote it, and its mark.
fn source(entry: &Value) -> String {
    let by = text(&entry["source"]["by"]);
    if entry["source"]["exposed"] == true {
        format!("{by} (exposed: {})", list(&entry["source"]["exposures"]))
    } else {
        by
    }
}

fn print_entries(listed: &Value) {
    println!("ID\tSCOPE\tREVISION\tSOURCE\tSUMMARY");
    for entry in listed["entries"].as_array().into_iter().flatten() {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            text(&entry["id"]),
            text(&entry["scope"]),
            entry["revision"],
            source(entry),
            text(&entry["summary"]),
        );
    }
    let shown = listed["entries"].as_array().map_or(0, Vec::len);
    if listed["total"]
        .as_u64()
        .is_some_and(|total| total as usize > shown)
    {
        println!("({shown} of {})", listed["total"]);
    }
}

pub(crate) async fn run(
    client: &Client,
    server: &Url,
    token: &str,
    command: MemoriesCommand,
) -> Result<(), String> {
    let call = |method: Method,
                segments: Vec<String>,
                query: Vec<(&'static str, String)>,
                body: Option<Value>| {
        let (client, server, token) = (client.clone(), server.clone(), token.to_owned());
        async move {
            let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
            let mut address = url(&server, &segments)?;
            if !query.is_empty() {
                address.query_pairs_mut().extend_pairs(query);
            }
            exchange(&client, &token, method, address, None, body).await
        }
    };
    let path = |tail: &[&str]| {
        std::iter::once("memories")
            .chain(tail.iter().copied())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    match command {
        MemoriesCommand::Search { query, scope } => {
            let mut asked = Vec::new();
            if let Some(scope) = scope {
                asked.push(("scope", scope));
            }
            if let Some(query) = query {
                asked.push(("q", query));
            }
            print_entries(&call(Method::GET, path(&[]), asked, None).await?);
        }
        MemoriesCommand::Show { id } => {
            let entry = call(Method::GET, path(&[&id]), Vec::new(), None).await?;
            for key in ["id", "scope", "name", "summary", "updated_by"] {
                println!("{key}\t{}", text(&entry[key]));
            }
            println!("revision\t{}", entry["revision"]);
            println!("source\t{}", source(&entry));
            println!("compartments\t{}", list(&entry["compartments"]));
            println!();
            println!("{}", plain(entry["content"].as_str().unwrap_or_default()));
        }
        MemoriesCommand::Add {
            scope,
            summary,
            content,
            name,
            compartments,
        } => {
            let answer = call(
                Method::POST,
                path(&[]),
                Vec::new(),
                Some(json!({
                    "scope": scope,
                    "summary": summary,
                    "content": content,
                    "name": name,
                    "compartments": compartments,
                })),
            )
            .await?;
            println!("{}\t{}", text(&answer["id"]), answer["revision"]);
        }
        MemoriesCommand::Edit {
            id,
            revision,
            summary,
            content,
            name,
            compartments,
        } => {
            let answer = call(
                Method::PUT,
                path(&[&id]),
                Vec::new(),
                Some(json!({
                    "summary": summary,
                    "content": content,
                    "name": name,
                    "revision": revision,
                    "compartments": compartments,
                })),
            )
            .await?;
            println!("{}\t{}", text(&answer["id"]), answer["revision"]);
        }
        MemoriesCommand::Forget { id, revision } => {
            let asked = revision
                .map(|revision| vec![("revision", revision.to_string())])
                .unwrap_or_default();
            call(Method::DELETE, path(&[&id]), asked, None).await?;
            println!("forgot\t{}", plain(&id));
        }
        MemoriesCommand::Proposals { scope } => {
            let asked = scope
                .map(|scope| vec![("scope", scope)])
                .unwrap_or_default();
            let listed = call(Method::GET, path(&["proposals"]), asked, None).await?;
            println!("ID\tSCOPE\tENTRY\tFORGET\tSOURCE\tSUMMARY");
            for proposal in listed["proposals"].as_array().into_iter().flatten() {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    text(&proposal["id"]),
                    text(&proposal["scope"]),
                    text(&proposal["entry"]),
                    proposal["forget"],
                    source(proposal),
                    text(&proposal["summary"]),
                );
            }
        }
        MemoriesCommand::Decide { id, reject } => {
            let answer = call(
                Method::POST,
                path(&["proposals", &id]),
                Vec::new(),
                Some(json!({"approve": !reject})),
            )
            .await?;
            println!(
                "{}\t{}",
                if reject { "rejected" } else { "approved" },
                text(&answer["entry"]["id"])
            );
        }
    }
    Ok(())
}
