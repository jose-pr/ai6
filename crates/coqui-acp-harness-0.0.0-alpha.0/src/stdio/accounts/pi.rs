//! Pi (`pi-acp`), read from Pi 1.5.0's source; `pi-acp` itself and a real sign-in were not run.
//!
//! `PI_CODING_AGENT_DIR` places `auth.json`, the settings and the sessions. Pi's own sign-in (`/login`) exists only inside
//! its terminal interface, so this provider lists no login method: an API key account is made from its settings.

use super::{Credential, HarnessDef, KindDef, Setting};

/// The kind of a key read from one of the variables below.
pub const API_KEY: &str = "pi-api-key";
/// The kind of a sign-in kept in `auth.json`.
pub const OAUTH: &str = "pi-oauth";

/// The variables Pi reads a provider's key from. A key account names one.
const KEY_VARIABLES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_OAUTH_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
    "GROQ_API_KEY",
    "CEREBRAS_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "HF_TOKEN",
    "COPILOT_GITHUB_TOKEN",
];

/// Every variable Pi reads for a credential, a cloud identity, an endpoint or for where it keeps them, so a start on an
/// account removes them all. From Pi 1.5.0's source and `docs/providers.md`; a provider that Pi's dependencies add is not
/// in it.
const VARIABLES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_OAUTH_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
    "GROQ_API_KEY",
    "CEREBRAS_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "HF_TOKEN",
    "COPILOT_GITHUB_TOKEN",
    "PI_CODING_AGENT_DIR",
    "PI_CODING_AGENT_SESSION_DIR",
    // Provider keys beyond the ones an account can name.
    "AI_GATEWAY_API_KEY",
    "ANT_LING_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "BASETEN_API_KEY",
    "CLOUDFLARE_API_KEY",
    "FIREWORKS_API_KEY",
    "GOOGLE_CLOUD_API_KEY",
    "KIMI_API_KEY",
    "META_API_KEY",
    "MINIMAX_API_KEY",
    "MINIMAX_CN_API_KEY",
    "MOONSHOT_API_KEY",
    "NVIDIA_API_KEY",
    "OPENCODE_API_KEY",
    "QWEN_TOKEN_PLAN_API_KEY",
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "RADIUS_API_KEY",
    "TOGETHER_API_KEY",
    "TYPESAFE_API_KEY",
    "XIAOMI_API_KEY",
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    "ZAI_API_KEY",
    "ZAI_CODING_CN_API_KEY",
    // Endpoints.
    "ANTHROPIC_BASE_URL",
    "OPENAI_BASE_URL",
    "OPENROUTER_BASE_URL",
    "LLAMA_BASE_URL",
    "AZURE_OPENAI_BASE_URL",
    "AZURE_OPENAI_ENDPOINT",
    "AZURE_OPENAI_RESOURCE_NAME",
    "AZURE_OPENAI_API_VERSION",
    "AZURE_OPENAI_DEPLOYMENT_NAME_MAP",
    "VERTEX_BASE_URL",
    "CLOUDFLARE_ACCOUNT_ID",
    "CLOUDFLARE_GATEWAY_ID",
    "GCE_METADATA_HOST",
    "AWS_EC2_METADATA_SERVICE_ENDPOINT",
    "PI_INSTALLER_API_BASE",
    "PI_SHARE_VIEWER_URL",
    // Cloud identities.
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    "GOOGLE_CLOUD_QUOTA_PROJECT",
    "GCLOUD_PROJECT",
    "CLOUDSDK_CONFIG",
    "ANTHROPIC_FEDERATION_RULE_ID",
    "ANTHROPIC_IDENTITY_TOKEN_FILE",
    "ANTHROPIC_ORGANIZATION_ID",
    "ANTHROPIC_SERVICE_ACCOUNT_ID",
    "ANTHROPIC_WORKSPACE_ID",
];

/// What Pi takes as an account.
pub static DEF: HarnessDef = HarnessDef {
    name: "Pi",
    program: "pi-acp",
    args: &[],
    kinds: &[
        KindDef {
            id: API_KEY,
            title: "Pi provider key",
            description: "A key for one provider, read from the variable `variable` names, with Pi started in a directory of its own so that no key in `auth.json` and no `baseUrl` in the person's own `models.json` overrides it.",
            credentials: &[Credential {
                key: "api_key",
                required: true,
            }],
            settings: &[Setting {
                name: "variable",
                required: true,
                url: false,
                choices: KEY_VARIABLES,
            }],
            state_dir: true,
            rotates: false,
            history_portable: false,
            renews: false,
            endpoints: &[],
            notices: &[
                "the credential is in the environment of every program the agent runs",
                "the account has a Pi directory of its own, without the person's settings",
            ],
            credential_variables: &[],
            setting_variables: &[],
            named_variable: Some(("variable", "api_key")),
            home_variable: Some("PI_CODING_AGENT_DIR"),
            file: None,
            renew_unsupported: "a key is not renewed",
        },
        KindDef {
            id: OAUTH,
            title: "Pi sign-in",
            description: "The `auth.json` Pi's own sign-in (`/login`) wrote, kept in Pi's directory.",
            credentials: &[Credential {
                key: "auth_json",
                required: true,
            }],
            settings: &[],
            state_dir: true,
            rotates: true,
            history_portable: false,
            renews: false,
            endpoints: &[],
            notices: &["the login is kept in a file that what the agent runs can read"],
            credential_variables: &[],
            setting_variables: &[],
            named_variable: None,
            home_variable: Some("PI_CODING_AGENT_DIR"),
            file: Some(("auth.json", "auth_json")),
            renew_unsupported: "Pi refreshes a sign-in itself, under a lock, when it needs one; it has no command that does so without a model turn",
        },
    ],
    variables: VARIABLES,
    renew_args: None,
    login_unsupported: "Pi signs in only inside its terminal interface (`/login`), which this provider does not drive; a key account is made from its settings",
};
