//! A stand-in for `codex login --with-api-key` and `--with-access-token` that the account tests run: it reads the
//! credential from standard input and keeps it in `auth.json` of `CODEX_HOME`, as Codex does, and logs what it was given
//! (never the credential) to `fake-codex.log` beside it. A credential that starts with `refused` is refused. It signs
//! in to nothing and reaches no network.

use std::io::{Read, Write};
use std::path::PathBuf;

fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [login, flag] = arguments.as_slice() else {
        eprintln!(
            "this stand-in answers `login --with-api-key` and `login --with-access-token` only"
        );
        return std::process::ExitCode::from(2);
    };
    if login != "login" || !matches!(flag.as_str(), "--with-api-key" | "--with-access-token") {
        eprintln!(
            "this stand-in answers `login --with-api-key` and `login --with-access-token` only"
        );
        return std::process::ExitCode::from(2);
    }
    let Some(home) = std::env::var_os("CODEX_HOME").map(PathBuf::from) else {
        eprintln!("no home");
        return std::process::ExitCode::from(1);
    };
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return std::process::ExitCode::from(1);
    }
    let credential = input.trim();
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("fake-codex.log"))
    {
        let names: Vec<String> = std::env::vars_os()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        let _ = writeln!(
            log,
            "{}",
            serde_json::json!({"args": arguments, "env_names": names})
        );
    }
    if credential.is_empty() || credential.starts_with("refused") {
        eprintln!("Error: the credential was not accepted");
        return std::process::ExitCode::from(1);
    }
    let stored = if flag == "--with-api-key" {
        serde_json::json!({"OPENAI_API_KEY": credential})
    } else {
        serde_json::json!({"tokens": {"access_token": credential}})
    };
    if std::fs::write(home.join("auth.json"), stored.to_string()).is_err() {
        return std::process::ExitCode::from(1);
    }
    std::process::ExitCode::SUCCESS
}
