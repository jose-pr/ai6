//! The plan limits of the login Codex keeps for itself, read from its `auth.json` with no `codex` process.
//!
//! Codex keeps a ChatGPT sign-in in `auth.json` of its home (`CODEX_HOME`, else `.codex` below the user's home). The access
//! token in it is what Codex's own `account/rateLimits/read` sends to the ChatGPT backend, so reading the same document
//! with it ([`read_limits`]) answers the same buckets without starting `codex app-server`.
//!
//! Nothing is ever written, and no token is renewed here: ChatGPT replaces a refresh token each time it is used and refuses
//! the old one, so a renewal whose new token is not stored in the file would end the person's sign-in. A login whose access
//! token has expired is left to Codex, which renews it and stores the result.

use alloc::string::String;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::{AccountGrant, AccountUsage, Credentials, Environment, Source};
use serde_json::Value;

use crate::account_usage::{ReadOptions, read_limits};
use crate::accounts::{AUTH_FILE, CHATGPT, FILE_KEY};

/// What the stored login says of the account's usage.
#[derive(Debug)]
pub(crate) enum Stored {
    /// The usage the login's access token read.
    Read(AccountUsage),
    /// There is no login in a file Codex's own ChatGPT backend would take: no file (a login kept in the keyring is one), a
    /// file that is not a ChatGPT sign-in, or an access token that expired or was refused. Codex itself can read these.
    Unavailable,
}

/// The folder Codex keeps its login in: `CODEX_HOME` of the call's environment, then of this process, then `.codex` below the
/// user's home.
fn home_of(environment: &Environment) -> Option<PathBuf> {
    let named = |value: Option<&str>| value.filter(|text| !text.is_empty()).map(PathBuf::from);
    if let Some(home) = named(environment.get("CODEX_HOME")) {
        return Some(home);
    }
    let process = std::env::var("CODEX_HOME").ok();
    if let Some(home) = named(process.as_deref()) {
        return Some(home);
    }
    let user = ["USERPROFILE", "HOME"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|text| !text.is_empty()))?;
    Some(PathBuf::from(user).join(".codex"))
}

/// `auth.json` of the home, when it is a file of at most `limit` bytes.
fn read_file(home: &Path, limit: u64) -> Option<String> {
    let path = home.join(AUTH_FILE);
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit.saturating_add(1))
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= limit).then_some(text)
}

/// The usage of the account `id` as the stored login reads it.
///
/// A login that holds an API key and no ChatGPT sign-in has no plan limits and answers with no buckets, as Codex does.
pub(crate) fn read(
    id: &str,
    environment: &Environment,
    options: &ReadOptions,
    limit: u64,
) -> Stored {
    let Some(home) = home_of(environment) else {
        return Stored::Unavailable;
    };
    let Some(text) = read_file(&home, limit) else {
        return Stored::Unavailable;
    };
    let Ok(file) = serde_json::from_str::<Value>(&text) else {
        return Stored::Unavailable;
    };
    let signed_in = file
        .get("tokens")
        .and_then(|tokens| tokens.get("access_token"))
        .and_then(Value::as_str)
        .is_some_and(|token| !token.is_empty());
    if !signed_in {
        let has_key = file
            .get("OPENAI_API_KEY")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.is_empty());
        return if has_key {
            Stored::Read(empty(id))
        } else {
            Stored::Unavailable
        };
    }
    let grant = AccountGrant::new(id, CHATGPT)
        .credentials(Credentials::from_iter([(FILE_KEY, text.as_str())]))
        .state_dir(home.to_string_lossy());
    match read_limits(&grant, options) {
        Ok(report) if report.supported => Stored::Read(AccountUsage::from_report(report)),
        Ok(_) => Stored::Read(empty(id)),
        Err(error) if error.status == Status::GUATIAO_ERR_GONE => Stored::Unavailable,
        // The backend could not be reached or answered something else: Codex would meet the same, so no process is started.
        Err(_) => Stored::Read(empty(id)),
    }
}

fn empty(id: &str) -> AccountUsage {
    let mut usage = AccountUsage::new();
    usage.sources = alloc::vec![Source::Reported];
    usage.account = Some(String::from(id));
    usage
}
