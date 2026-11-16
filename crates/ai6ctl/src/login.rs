//! `ai6ctl login` and `ai6ctl factors`: signing in to the gateway with a password and, when the
//! sign-in policy asks, a one-time code; and a person's second factor. The token, the
//! provisioning link and the recovery codes are shown once, on standard output only; prompts and
//! the methods the gateway offers go to standard error.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use clap::{Args, Subcommand};
use reqwest::{Client, Method, StatusCode, Url};
use serde_json::{Value, json};

use crate::access::{exchange, list, text, url};
use crate::plain;

/// The environment variable that holds the password, instead of a prompt.
pub(crate) const PASSWORD_ENV: &str = "UNEX_PASSWORD";
/// The environment variable that holds the first one-time code, instead of a prompt.
pub(crate) const CODE_ENV: &str = "UNEX_CODE";

#[derive(Debug, Args)]
pub(crate) struct LoginArgs {
    /// The account name to sign in as.
    #[arg(long)]
    name: String,
    /// Read the password from this file's first line. Without it the password is
    /// `UNEX_PASSWORD`, else the first line of standard input.
    #[arg(long, value_name = "FILE")]
    password_file: Option<PathBuf>,
    /// Present a recovery code, not a one-time code, at the next step.
    #[arg(long)]
    recovery: bool,
    /// Write the token to this new file (mode 600) instead of printing it.
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum FactorsCommand {
    /// Whether you have a second factor, and whether the sign-in policy lets you set one up.
    Status,
    /// Begin setting up a one-time code app, or replace the one you have: prints the
    /// provisioning link and the recovery codes, once.
    Enrol,
    /// Confirm the app with the code it shows now; until then the old factor, if any, stands.
    Confirm {
        /// The code from the app.
        code: String,
    },
    /// Remove a person's second factor so that they enrol again (needs account.manage).
    Reset {
        /// The person's ID, not their account name.
        person: String,
    },
}

/// The line `prompt` asks for, after showing it, from standard input.
fn read_line(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|_| "could not read standard input".to_owned())?;
    let line = line.trim_end_matches(['\r', '\n']).to_owned();
    if line.is_empty() {
        return Err("nothing was entered".to_owned());
    }
    Ok(line)
}

async fn post(client: &Client, server: &Url, body: &Value) -> Result<(StatusCode, Value), String> {
    let mut url = server.clone();
    url.set_path("/v1/auth/login");
    let response = client
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|_| "could not contact Universal Exports".to_owned())?;
    let status = response.status();
    let value: Value = response.json().await.unwrap_or(Value::Null);
    Ok((status, value))
}

fn refused(status: StatusCode, value: &Value) -> String {
    let reason = value
        .get("error")
        .and_then(Value::as_str)
        .map_or_else(String::new, |reason| format!(": {}", plain(reason)));
    format!("Universal Exports returned HTTP {status}{reason}")
}

pub(crate) async fn login(client: &Client, server: &Url, args: LoginArgs) -> Result<(), String> {
    let secret = match (&args.password_file, std::env::var(PASSWORD_ENV)) {
        (Some(path), _) => std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {}", path.display(), error.kind()))?
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned(),
        (None, Ok(secret)) if !secret.is_empty() => secret,
        _ => read_line("Password: ")?,
    };
    let (mut status, mut value) =
        post(client, server, &json!({"name": args.name, "secret": secret})).await?;
    let mut first_code = std::env::var(CODE_ENV).ok().filter(|code| !code.is_empty());
    while status == StatusCode::ACCEPTED {
        let pending = value["pending"]
            .as_str()
            .ok_or("the gateway asked for another step without a pending sign-in")?
            .to_owned();
        let offered = list(&value["methods"]);
        let method = if args.recovery {
            "recovery_code"
        } else {
            "totp"
        };
        if !offered.split(',').any(|offer| offer == method) {
            return Err(format!(
                "the gateway offers {offered} as the next step, not {method}"
            ));
        }
        let code = match first_code.take() {
            Some(code) => code,
            None => read_line(if args.recovery {
                "Recovery code: "
            } else {
                "Code: "
            })?,
        };
        (status, value) = post(
            client,
            server,
            &json!({"pending": pending, "method": method, "secret": code}),
        )
        .await?;
    }
    if status != StatusCode::CREATED {
        return Err(refused(status, &value));
    }
    let token = value["token"]
        .as_str()
        .ok_or("the gateway issued no token")?;
    match args.output {
        Some(path) => {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options
                .open(&path)
                .map_err(|error| format!("cannot create {}: {}", path.display(), error.kind()))?;
            file.write_all(format!("{token}\n").as_bytes())
                .map_err(|error| format!("cannot write {}: {}", path.display(), error.kind()))?;
            eprintln!("signed in; token written to {}", path.display());
        }
        None => println!("{token}"),
    }
    Ok(())
}

pub(crate) async fn run(
    client: &Client,
    server: &Url,
    token: &str,
    command: FactorsCommand,
) -> Result<(), String> {
    match command {
        FactorsCommand::Status => {
            let view = exchange(
                client,
                token,
                Method::GET,
                url(server, &["factors"])?,
                None,
                None,
            )
            .await?;
            println!("configured\t{}", view["configured"]);
            println!("enrolled\t{}", view["enrolled"]);
            println!("enrolling\t{}", view["enrolling"]);
            println!("may enrol\t{}", view["may_enrol"]);
        }
        FactorsCommand::Enrol => {
            let begun = exchange(
                client,
                token,
                Method::POST,
                url(server, &["factors", "totp"])?,
                None,
                None,
            )
            .await?;
            println!("{}", text(&begun["uri"]));
            for code in begun["recovery_codes"].as_array().into_iter().flatten() {
                println!("recovery\t{}", text(code));
            }
            eprintln!("shown once; confirm with `ai6ctl factors confirm CODE`");
        }
        FactorsCommand::Confirm { code } => {
            exchange(
                client,
                token,
                Method::POST,
                url(server, &["factors", "totp", "confirm"])?,
                None,
                Some(json!({"code": code})),
            )
            .await?;
            println!("second factor enrolled");
        }
        FactorsCommand::Reset { person } => {
            exchange(
                client,
                token,
                Method::DELETE,
                url(server, &["access", "factors", &person])?,
                None,
                None,
            )
            .await?;
            println!("{}: second factor removed", plain(&person));
        }
    }
    Ok(())
}
