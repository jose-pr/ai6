//! `ai6ctl compartments`: need-to-know access across Operations, through the gateway's routes. A
//! refusal prints the gateway's reason.

use clap::Subcommand;
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};

use crate::access::{exchange, list, text, url};
use crate::plain;

#[derive(Debug, Subcommand)]
pub(crate) enum CompartmentsCommand {
    /// Show one compartment: what it grants, its covers and who is read in, leaving out what
    /// you may not see.
    Show {
        /// Its ID.
        id: String,
    },
    /// Make a compartment or change its settings (needs compartment.manage). What is not named
    /// stays as it is.
    Set {
        /// Its ID, a codeword by convention.
        id: String,
        /// What it is for.
        #[arg(long)]
        description: Option<String>,
        /// What it grants, comma separated: mission.status, mission.read, product.read,
        /// cable.send, cable.read, mission.propose.
        #[arg(long, value_delimiter = ',')]
        actions: Option<Vec<String>>,
        /// The clearance a Session needs for it to apply: desk, field or 00.
        #[arg(long)]
        min_clearance: Option<String>,
        /// The most recipients of one Cable, or `none`.
        #[arg(long)]
        max_fanout: Option<String>,
    },
    /// Cover an Operation or a Mission (needs compartment.manage, and compartment.cover on the
    /// Operation).
    Cover {
        /// Its ID.
        id: String,
        /// operation:ID or mission:ID.
        resource: String,
        /// Leave out the Operations under it.
        #[arg(long)]
        no_propagate: bool,
    },
    /// Stop covering it (needs compartment.manage, or compartment.cover on the Operation).
    Uncover {
        /// Its ID.
        id: String,
        /// operation:ID or mission:ID.
        resource: String,
    },
    /// Read a subject in (needs compartment.manage; a person, group or account also access.manage).
    ReadIn {
        /// Its ID.
        id: String,
        /// person:, group:, account:, operation:, mission: or agent:, then its name.
        subject: String,
        /// For an Operation: leave out the Operations under it.
        #[arg(long)]
        no_propagate: bool,
    },
    /// Read a subject out (needs compartment.manage).
    ReadOut {
        /// Its ID.
        id: String,
        /// The subject as it was read in.
        subject: String,
    },
    /// Delete a compartment (needs compartment.manage).
    Remove {
        /// Its ID.
        id: String,
    },
}

fn entries(view: &Value, key: &str) -> Vec<String> {
    view[key]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let reach = if entry["propagate"] == false {
                " (this one only)"
            } else {
                ""
            };
            format!("{}{reach}", text(&entry["entry"]))
        })
        .collect()
}

fn print_compartment(view: &Value) {
    println!("compartment\t{}", text(&view["id"]));
    println!("description\t{}", text(&view["description"]));
    println!("actions\t{}", list(&view["actions"]));
    if let Some(floor) = view["min_clearance"].as_str() {
        println!("min_clearance\t{}", plain(floor));
    }
    for (key, value) in view["limits"].as_object().into_iter().flatten() {
        println!("limit\t{}\t{}", plain(key), plain(&value.to_string()));
    }
    for weakening in view["weakenings"].as_array().into_iter().flatten() {
        println!("weakening\t{}", text(weakening));
    }
    for cover in entries(view, "covers") {
        println!("cover\t{cover}");
    }
    for read_in in entries(view, "read_in") {
        println!("read_in\t{read_in}");
    }
}

/// What `ai6ctl compartments` does: with no command, lists what the person may read.
pub(crate) async fn run(
    client: &Client,
    server: &Url,
    token: &str,
    command: Option<CompartmentsCommand>,
) -> Result<(), String> {
    let get = |path: Vec<String>| {
        let (client, server, token) = (client.clone(), server.clone(), token.to_owned());
        async move {
            let segments: Vec<&str> = path.iter().map(String::as_str).collect();
            exchange(
                &client,
                &token,
                Method::GET,
                url(&server, &segments)?,
                None,
                None,
            )
            .await
        }
    };
    let write = |method: Method, path: Vec<String>, body: Option<Value>| {
        let (client, server, token) = (client.clone(), server.clone(), token.to_owned());
        async move {
            let segments: Vec<&str> = path.iter().map(String::as_str).collect();
            exchange(
                &client,
                &token,
                method,
                url(&server, &segments)?,
                None,
                body,
            )
            .await
        }
    };
    let of = |id: &str, tail: &[&str]| {
        ["compartments", id]
            .iter()
            .chain(tail)
            .map(|segment| (*segment).to_owned())
            .collect::<Vec<_>>()
    };
    match command {
        None => {
            let listed = get(vec!["compartments".to_owned()]).await?;
            println!("COMPARTMENT\tACTIONS\tCOVERS\tREAD_IN");
            for view in listed.as_array().into_iter().flatten() {
                println!(
                    "{}\t{}\t{}\t{}",
                    text(&view["id"]),
                    list(&view["actions"]),
                    entries(view, "covers").join(","),
                    entries(view, "read_in").join(","),
                );
            }
        }
        Some(CompartmentsCommand::Show { id }) => {
            print_compartment(&get(of(&id, &[])).await?);
        }
        Some(CompartmentsCommand::Set {
            id,
            description,
            actions,
            min_clearance,
            max_fanout,
        }) => {
            // A new compartment has nothing to read; an edit keeps what is not named.
            let current = get(of(&id, &[])).await.unwrap_or(Value::Null);
            let mut limits = current["limits"].clone();
            if !limits.is_object() {
                limits = json!({});
            }
            if let Some(bound) = max_fanout {
                limits["max_fanout"] = match bound.parse::<u64>() {
                    Ok(count) => json!(count),
                    Err(_) if bound == "none" => json!("none"),
                    Err(_) => {
                        return Err("--max-fanout takes a number or none".to_owned());
                    }
                };
            }
            let mut body = json!({
                "description": description.unwrap_or_else(|| text(&current["description"])),
                "actions": actions.map_or_else(|| current["actions"].clone(), |actions| json!(actions)),
                "limits": limits,
            });
            if !body["actions"].is_array() {
                body["actions"] = json!([]);
            }
            if let Some(floor) =
                min_clearance.or_else(|| current["min_clearance"].as_str().map(str::to_owned))
            {
                body["min_clearance"] = json!(floor);
            }
            write(Method::PUT, of(&id, &[]), Some(body)).await?;
            println!("{}: saved", plain(&id));
        }
        Some(CompartmentsCommand::Cover {
            id,
            resource,
            no_propagate,
        }) => {
            write(
                Method::PUT,
                of(&id, &["covers", &resource]),
                Some(json!({"propagate": !no_propagate})),
            )
            .await?;
            println!("{}: covers {}", plain(&id), plain(&resource));
        }
        Some(CompartmentsCommand::Uncover { id, resource }) => {
            write(Method::DELETE, of(&id, &["covers", &resource]), None).await?;
            println!("{}: no longer covers {}", plain(&id), plain(&resource));
        }
        Some(CompartmentsCommand::ReadIn {
            id,
            subject,
            no_propagate,
        }) => {
            write(
                Method::PUT,
                of(&id, &["read-in", &subject]),
                Some(json!({"propagate": !no_propagate})),
            )
            .await?;
            println!("{}: {} read in", plain(&id), plain(&subject));
        }
        Some(CompartmentsCommand::ReadOut { id, subject }) => {
            write(Method::DELETE, of(&id, &["read-in", &subject]), None).await?;
            println!("{}: {} read out", plain(&id), plain(&subject));
        }
        Some(CompartmentsCommand::Remove { id }) => {
            write(Method::DELETE, of(&id, &[]), None).await?;
            println!("{}: removed", plain(&id));
        }
    }
    Ok(())
}

/// `ai6ctl missions products M`: what a Mission produced.
pub(crate) async fn products(
    client: &Client,
    server: &Url,
    token: &str,
    mission: &str,
) -> Result<(), String> {
    let mut address = server.clone();
    address
        .path_segments_mut()
        .map_err(|_| "Universal Exports URL cannot contain path segments".to_owned())?
        .extend(["v1", "missions", mission, "products"]);
    let answer = exchange(client, token, Method::GET, address, None, None).await?;
    for key in [
        "mission",
        "operation",
        "state",
        "role",
        "agent",
        "created",
        "blocked",
    ] {
        if let Some(value) = answer[key].as_str() {
            println!("{key}\t{}", plain(value));
        }
    }
    println!("attempts\t{}", answer["attempts"]);
    for (key, value) in answer["outcome_untrusted"]
        .as_object()
        .into_iter()
        .flatten()
    {
        println!("outcome.{}\t{}", plain(key), plain(&value.to_string()));
    }
    if let Some(report) = answer["cx_report_untrusted"].as_str() {
        println!("cx_report\t{}", plain(report));
    }
    for (key, value) in answer["refs"].as_object().into_iter().flatten() {
        println!("ref.{}\t{}", plain(key), plain(&value.to_string()));
    }
    Ok(())
}
