//! `ai6ctl keys`: a person's own API keys, through the gateway's `/v1/user/keys` routes. A key's
//! value is shown once, at creation, on standard output or in a file only its owner can read; it
//! is printed nowhere else.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};

use crate::access::{exchange, list, text, url};

#[derive(Debug, Subcommand)]
pub(crate) enum KeysCommand {
    /// Make a key that acts as you, narrowed to the scopes named. Its value is printed once
    /// to standard output (the only thing printed there), or written with --output. Needs
    /// your own sign-in token, not another key.
    Create {
        /// What the key may do, as ACTION@SCOPE such as config.manage@config:plugin; repeat for
        /// more. Each must be something you may do now.
        #[arg(long = "scope", required = true, value_name = "ACTION@SCOPE")]
        scopes: Vec<String>,
        /// How long the key lasts: seconds, or a number with s, m, h or d (`30d`); `never`
        /// for no end. Default: the gateway's `key_ttl`.
        #[arg(long)]
        expires: Option<String>,
        /// What to call the key.
        #[arg(long, default_value = "key")]
        name: String,
        /// Write the value to this new file (mode 600) instead of printing it.
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// List your keys: name, scopes, created, expires, last used. Never the values.
    List {
        /// Everyone's keys (needs account.manage on config:accounts).
        #[arg(long)]
        all: bool,
    },
    /// End a key now. You revoke your own; an administrator anyone's.
    Revoke {
        /// The key's ID, from `keys list`.
        id: String,
    },
}

/// Seconds in `text`: a number of seconds, a number with `s`, `m`, `h` or `d`, or `never` (0).
pub(crate) fn parse_lifetime(text: &str) -> Result<f64, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("never") {
        return Ok(0.0);
    }
    let (number, unit) = match text.char_indices().last() {
        Some((at, unit)) if unit.is_ascii_alphabetic() => (&text[..at], Some(unit)),
        _ => (text, None),
    };
    let scale = match unit {
        None | Some('s') => 1.0,
        Some('m') => 60.0,
        Some('h') => 3600.0,
        Some('d') => 86_400.0,
        Some(other) => {
            return Err(format!(
                "{text:?} is not a duration: {other:?} is not a unit, use s, m, h or d"
            ));
        }
    };
    let value: f64 = number
        .trim()
        .parse()
        .map_err(|_| format!("{text:?} is not a duration such as 30d, 12h or never"))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{text:?} must be a positive duration, or never"));
    }
    Ok(value * scale)
}

/// Writes `value` and a newline to the new file `path`, readable by its owner alone.
fn write_private(path: &Path, value: &str) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("cannot create {}: {}", path.display(), error.kind()))?;
    file.write_all(format!("{value}\n").as_bytes())
        .map_err(|error| format!("cannot write {}: {}", path.display(), error.kind()))
}

fn when(value: &Value, none: &str) -> String {
    value
        .as_f64()
        .map_or_else(|| none.to_owned(), |seconds| format!("{}", seconds as i64))
}

pub(crate) async fn run(
    client: &Client,
    server: &Url,
    token: &str,
    command: KeysCommand,
) -> Result<(), String> {
    match command {
        KeysCommand::Create {
            scopes,
            expires,
            name,
            output,
        } => {
            let mut body = json!({ "name": name, "scopes": scopes });
            if let Some(expires) = expires {
                body["expires_in"] = json!(parse_lifetime(&expires)?);
            }
            // Refuse a file that is in the way before a key is made for it.
            if let Some(path) = &output
                && path.exists()
            {
                return Err(format!("{} exists; name a new file", path.display()));
            }
            let made = exchange(
                client,
                token,
                Method::POST,
                url(server, &["keys"])?,
                None,
                Some(body),
            )
            .await?;
            let secret = made
                .get("secret")
                .and_then(Value::as_str)
                .ok_or("the gateway sent no key")?;
            let id = text(&made["key"]["id"]);
            match output {
                Some(path) => {
                    write_private(&path, secret)?;
                    println!("key {id} written to {}", path.display());
                }
                None => {
                    println!("{secret}");
                    eprintln!("key {id} created; this is the only time its value is shown");
                }
            }
        }
        KeysCommand::List { all } => {
            let mut address = url(server, &["keys"])?;
            if all {
                address.query_pairs_mut().append_pair("all", "true");
            }
            let answer = exchange(client, token, Method::GET, address, None, None).await?;
            println!("ID\tNAME\tOWNER\tSCOPES\tCREATED\tEXPIRES\tLAST USED");
            for key in answer.as_array().into_iter().flatten() {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    text(&key["id"]),
                    text(&key["name"]),
                    text(&key["owner_name"]),
                    list(&key["scopes"]),
                    when(&key["created_at"], "-"),
                    when(&key["expires_at"], "never"),
                    when(&key["last_used_at"], "never"),
                );
            }
        }
        KeysCommand::Revoke { id } => {
            exchange(
                client,
                token,
                Method::DELETE,
                url(server, &["keys", &id])?,
                None,
                None,
            )
            .await?;
            println!("key {} revoked", crate::plain(&id));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lifetime_is_seconds_or_a_number_with_a_unit_or_never() {
        assert_eq!(parse_lifetime("90").unwrap(), 90.0);
        assert_eq!(parse_lifetime("45s").unwrap(), 45.0);
        assert_eq!(parse_lifetime("2m").unwrap(), 120.0);
        assert_eq!(parse_lifetime("12h").unwrap(), 43_200.0);
        assert_eq!(parse_lifetime("30d").unwrap(), 2_592_000.0);
        assert_eq!(parse_lifetime("never").unwrap(), 0.0);
        for bad in ["", "d", "-1d", "0", "5w", "abc", "NaN"] {
            assert!(parse_lifetime(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_key_file_is_new_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        write_private(&path, "ck_example").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ck_example\n");
        assert!(
            write_private(&path, "other").is_err(),
            "an existing file is not replaced"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
