//! `ai6ctl secret`: secret values and the scope keys that encrypt them, through the gateway.
//!
//! The gateway holds no key, so each verb is a request a Station answers; `--wait` (seconds,
//! default 30, 0 to leave it) is how long this waits for the answer. A value is read from
//! standard input and never taken as an argument, and is never printed.

use std::io::Read as _;
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};

use crate::access::{exchange, list, text, url};
use crate::plain;

/// How long to wait for a Station to answer.
#[derive(Debug, Args, Clone)]
pub(crate) struct Wait {
    /// Seconds to wait for a Station's answer; 0 sends the request and prints its ID.
    #[arg(long, default_value_t = 30)]
    wait: u64,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SecretCommand {
    /// List the values and scope keys, with their scope, revision, key ID and Stations (never a
    /// value). Needs config.read on config:secrets.
    List,
    /// Write a value, read from standard input. A scope with no key gets one, wrapped for
    /// `--stations`. Needs config.manage on config:secrets.
    Put {
        /// The value's name.
        name: String,
        /// `global`, `operation:ID` or `compartment:ID`; the value's own when it exists, else global.
        #[arg(long)]
        scope: Option<String>,
        /// The Stations a new scope's key is wrapped for, comma separated.
        #[arg(long, value_delimiter = ',')]
        stations: Vec<String>,
        /// The fingerprint a Station's sealing key must be signed by, as STATION=FINGERPRINT.
        #[arg(long = "pin")]
        pins: Vec<String>,
        /// The revision this replaces; refused when the value is at another.
        #[arg(long)]
        expect: Option<u64>,
        #[command(flatten)]
        wait: Wait,
    },
    /// Write a new value of one that exists, from standard input, under its scope's key.
    Rotate {
        /// The value's name.
        name: String,
        /// The revision this replaces.
        #[arg(long)]
        expect: Option<u64>,
        #[command(flatten)]
        wait: Wait,
    },
    /// Replace a scope's key and encrypt its values again, keeping its Stations.
    RotateKey {
        /// The scope.
        scope: String,
        #[command(flatten)]
        wait: Wait,
    },
    /// Wrap a scope's key for one more Station.
    Grant {
        /// The scope.
        scope: String,
        /// The Station.
        station: String,
        /// The fingerprint its sealing key must be signed by.
        #[arg(long)]
        pin: Option<String>,
        #[command(flatten)]
        wait: Wait,
    },
    /// Take a Station out of a scope: its wrap is dropped and the key replaced. It keeps what it
    /// already opened, and until scope key records are signed its Registry credentials must be
    /// revoked too.
    Revoke {
        /// The scope.
        scope: String,
        /// The Station.
        station: String,
        #[command(flatten)]
        wait: Wait,
    },
    /// Remove a value of a scope.
    Remove {
        /// The value's name.
        name: String,
        /// The scope it is in.
        #[arg(long)]
        scope: String,
        #[command(flatten)]
        wait: Wait,
    },
    /// Where a request stands.
    Status {
        /// The request's ID.
        id: String,
    },
}

/// The value on standard input, without its last line break.
fn value_from_stdin() -> Result<String, String> {
    let mut value = String::new();
    std::io::stdin()
        .read_to_string(&mut value)
        .map_err(|_| "the value could not be read from standard input".to_owned())?;
    if let Some(stripped) = value.strip_suffix('\n') {
        value = stripped.strip_suffix('\r').unwrap_or(stripped).to_owned();
    }
    if value.is_empty() {
        return Err("give the value on standard input".to_owned());
    }
    Ok(value)
}

fn pins(pins: &[String]) -> Result<serde_json::Map<String, Value>, String> {
    pins.iter()
        .map(|pin| {
            pin.split_once('=')
                .filter(|(station, fingerprint)| !station.is_empty() && !fingerprint.is_empty())
                .map(|(station, fingerprint)| (station.to_owned(), json!(fingerprint)))
                .ok_or_else(|| "--pin takes STATION=FINGERPRINT".to_owned())
        })
        .collect()
}

pub(crate) async fn run(
    client: &Client,
    server: &Url,
    token: &str,
    command: SecretCommand,
) -> Result<(), String> {
    let get = |segments: Vec<String>| {
        let (client, server, token) = (client.clone(), server.clone(), token.to_owned());
        async move {
            let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
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
    let (body, wait) = match command {
        SecretCommand::List => {
            let listed = get(vec!["secrets".to_owned()]).await?;
            println!("SCOPE\tREVISION\tKEY\tSTATIONS");
            for scope in listed["scopes"].as_array().into_iter().flatten() {
                println!(
                    "{}\t{}\t{}\t{}{}",
                    text(&scope["scope"]),
                    scope["revision"],
                    text(&scope["key_id"]),
                    list(&scope["stations"]),
                    if scope["rotating"] == true {
                        "\t(rotation unfinished)"
                    } else {
                        ""
                    },
                );
            }
            println!("NAME\tSCOPE\tREVISION\tKEY");
            for value in listed["values"].as_array().into_iter().flatten() {
                println!(
                    "{}\t{}\t{}\t{}",
                    text(&value["name"]),
                    text(&value["scope"]),
                    value["revision"],
                    text(&value["key_id"]),
                );
            }
            return Ok(());
        }
        SecretCommand::Status { id } => {
            let answered = get(vec!["secrets".to_owned(), "requests".to_owned(), id]).await?;
            print_answer(&answered)?;
            return Ok(());
        }
        SecretCommand::Put {
            name,
            scope,
            stations,
            pins: pin_args,
            expect,
            wait,
        } => (
            json!({
                "verb": "put", "name": name, "scope": scope, "value": value_from_stdin()?,
                "expect": expect, "stations": stations, "pins": pins(&pin_args)?,
            }),
            wait,
        ),
        SecretCommand::Rotate { name, expect, wait } => (
            json!({"verb": "rotate", "name": name, "value": value_from_stdin()?, "expect": expect}),
            wait,
        ),
        SecretCommand::RotateKey { scope, wait } => {
            (json!({"verb": "rotate-key", "scope": scope}), wait)
        }
        SecretCommand::Grant {
            scope,
            station,
            pin,
            wait,
        } => (
            json!({"verb": "grant", "scope": scope, "station": station, "pin": pin}),
            wait,
        ),
        SecretCommand::Revoke {
            scope,
            station,
            wait,
        } => (
            json!({"verb": "revoke", "scope": scope, "station": station}),
            wait,
        ),
        SecretCommand::Remove { name, scope, wait } => (
            json!({"verb": "remove", "name": name, "scope": scope}),
            wait,
        ),
    };
    let accepted = exchange(
        client,
        token,
        Method::POST,
        url(server, &["secrets", "requests"])?,
        None,
        Some(body),
    )
    .await?;
    let id = text(&accepted["id"]);
    if wait.wait == 0 {
        println!("request\t{id}");
        return Ok(());
    }
    let until = Instant::now() + Duration::from_secs(wait.wait);
    loop {
        let answered = get(vec![
            "secrets".to_owned(),
            "requests".to_owned(),
            id.clone(),
        ])
        .await?;
        if answered["state"] != "pending" {
            return print_answer(&answered);
        }
        if Instant::now() >= until {
            return Err(format!(
                "no Station has answered request {id} yet: it stays; `ai6ctl secret status {id}` asks again"
            ));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn print_answer(answered: &Value) -> Result<(), String> {
    match answered["state"].as_str() {
        Some("pending") => {
            println!("pending");
            Ok(())
        }
        Some("done") => {
            let outcome = &answered["outcome"];
            println!(
                "{}\t{}\t{}",
                text(&outcome["verb"]),
                text(&outcome["scope"]),
                text(&outcome["name"]),
            );
            if let Some(revision) = outcome["revision"].as_u64() {
                println!("revision\t{revision}");
            }
            println!("key\t{}", text(&outcome["key_id"]));
            println!("stations\t{}", list(&outcome["stations"]));
            for note in outcome["notes"].as_array().into_iter().flatten() {
                println!("note\t{}", plain(&text(note)));
            }
            Ok(())
        }
        Some("failed") => Err(format!("refused: {}", text(&answered["error"]))),
        _ => Err("the gateway's answer was not one this knows".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_is_a_station_and_a_fingerprint() {
        let parsed = pins(&["a=sha256:00".to_owned()]).unwrap();
        assert_eq!(parsed["a"], "sha256:00");
        assert!(pins(&["nonsense".to_owned()]).is_err());
        assert!(pins(&["=x".to_owned()]).is_err());
    }
}
