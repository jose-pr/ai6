//! The accounts Codex takes, as the provider's catalog lists them.
//!
//! Measured on Codex 0.166.0. A `rotates` nobody measured is `true`, a `history_portable` nobody measured `false`, and
//! a kind whose success path was not run is taken to need a directory. `renews` is `false` for every kind: the provider
//! has no `renew` yet.

use alloc::string::ToString;
use alloc::vec;

use coqui_acp_api::{AUTH, AccountCatalog, AccountKindInfo, EndpointSlot, MODEL_API};
use coqui_acp_harness::provider::{credential, schema};
use serde_json::{Value, json};

/// The kind of a ChatGPT login made by `codex login`, kept in `auth.json` of `CODEX_HOME`.
pub const CHATGPT: &str = "codex-chatgpt";
/// The kind of an API key, made by `codex login --with-api-key` or given in `CODEX_API_KEY`.
pub const API_KEY: &str = "codex-api-key";
/// The kind of an access token, made by `codex login --with-access-token` or given in `CODEX_ACCESS_TOKEN`.
pub const ACCESS_TOKEN: &str = "codex-access-token";

/// Every variable Codex reads for a credential, so a start on an account removes them all.
const VARIABLES: [&str; 3] = ["CODEX_API_KEY", "OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"];

fn settings(secret: &str, endpoint: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            secret: credential(),
            endpoint: {"type": "string", "format": "uri"},
        },
        "required": [secret],
        "additionalProperties": false,
    })
}

fn kind(id: &str, title: &str, description: &str, secret: &str, endpoint: &str) -> AccountKindInfo {
    let mut kind = AccountKindInfo::new(id, title);
    kind.description = description.to_string();
    kind.settings = schema(&settings(secret, endpoint));
    kind.state_dir = true;
    kind
}

/// What the provider takes as an account.
#[must_use]
pub fn catalog() -> AccountCatalog {
    let mut catalog = AccountCatalog::new();

    let mut chatgpt = kind(
        CHATGPT,
        "ChatGPT login",
        "A login made by `codex login`, kept in `auth.json` of Codex's home directory.",
        "auth_file",
        "chatgpt_base_url",
    );
    chatgpt.endpoints = vec![
        EndpointSlot::new(MODEL_API)
            .key("chatgpt_base_url")
            .default_url("https://chatgpt.com/backend-api/codex"),
        EndpointSlot::new(AUTH).default_url("https://auth.openai.com"),
    ];
    chatgpt.rotates = true;
    chatgpt.notices =
        vec!["the login is kept in a file that what the agent runs can read".to_string()];

    let mut key = kind(
        API_KEY,
        "OpenAI API key",
        "A key Codex saves in `auth.json` of its home directory, or reads from `CODEX_API_KEY`.",
        "api_key",
        "openai_base_url",
    );
    key.endpoints = vec![
        EndpointSlot::new(MODEL_API)
            .key("openai_base_url")
            .default_url("https://api.openai.com/v1"),
    ];
    key.notices = vec!["the key is kept in a file that what the agent runs can read".to_string()];

    let mut token = kind(
        ACCESS_TOKEN,
        "Codex access token",
        "A token Codex checks offline when it is given, from `codex login --with-access-token` or `CODEX_ACCESS_TOKEN`.",
        "access_token",
        "chatgpt_base_url",
    );
    token.endpoints = vec![
        EndpointSlot::new(MODEL_API)
            .key("chatgpt_base_url")
            .default_url("https://chatgpt.com/backend-api/codex"),
    ];
    token.rotates = true;

    catalog.kinds.extend([chatgpt, key, token]);
    catalog.credential_variables = VARIABLES.iter().map(|v| (*v).to_string()).collect();
    catalog
}
