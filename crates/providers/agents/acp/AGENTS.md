# `ai6-agent-acp` — API header

Header-file-style reference for the `ai6-agent-acp` crate. It ships inside the package and is
self-contained. Development documentation lives at <https://github.com/jose-pr/ai6>.

No features. MSRV: Rust 1.89. Depends on `ai6` (features `acp`, `gate`), `coqui-acp`, the Agent
Client Protocol SDK (`agent-client-protocol`), `futures`, `serde`, `serde_json` and `toml`.
Contains no `unsafe` code. Also builds the test binary `ai6-acp-fake-agent`.

```rust
pub struct AcpConfig { pub settings_toml: String }   // Guatiao config; "" until an Agent sets handler_settings
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct AcpSettings {
    pub command: String, pub args: Vec<String>, pub env: BTreeMap<String, String>,
    pub meta: Map<String, Value>, pub model_option: Option<String>, pub effort_option: Option<String>,
    pub protocol: String,                       // "v1" only
    pub initialize_timeout_ms: u64,             // 20000
    pub request_timeout_ms: u64,                // 30000
    pub start_timeout_ms: u64,                  // 60000
    pub close_grace_ms: u64,                    // 2000
    pub diagnostic_lines: usize,                // 20
    pub exit_wait_ms: i64,                      // 1000
    pub poll_ms: i64,                           // 200
    pub unconfined_desk: bool,                  // false
    pub limit_phrases: Vec<String>,             // "rate limited", "rate limit exceeded", "usage-exhausted", "usage limit", "quota exceeded", "spending-limit", "run out of credits", "402 payment required"
    pub text_flush_bytes: usize,                // 65536: agent text delivered mid-message
    pub max_tool_kinds: usize,                  // 4096: tool calls whose kind is kept
    pub describe_options: usize,                // 64: options `models` keeps of the agent's answer
    pub describe_values: usize,                 // 1024: values per option
    pub describe_modes: usize,                  // 64: modes
    pub describe_sessions: usize,               // 500: sessions, across pages
    pub describe_text_bytes: usize,             // 4096: identifier and text bytes
    pub describe_pages: usize,                  // 10: session list pages (coqui_acp DescribeLimits; any value)
}
pub struct AcpHandler;                           // AgentHandler provider id "acp"; stdio() = Json (pipe);
                                                 // program() = the settings' `command`, None without settings
```

- `TryFrom<AcpConfig>`: empty settings build a handler whose `start` fails naming
  `handler_settings`; otherwise the TOML is parsed (unknown keys, an empty `command` and a
  `protocol` other than `v1` are refused).
- `start`/`resume` spawn `command args` through the given terminal with the settings' `env` under
  the Session's environment, then `AcpSession::connect` (see `ai6::directorates::section_00::acp`).
  The first prompt is `initial_context`, a read-only notice for answer-only/assistant Sessions,
  and the task; `resume` uses `session/resume`, else `session/load`, else fails.
- MCP servers: `AI6_CABLE_MCP_{URL,TOKEN}` and an assistant's `<prefix>_{URL,TOKEN}` become HTTP
  `mcpServers`; `AI6_THEATER_MCP_URL` (remote hands) is refused.
- Permissions: `GateDecider` (green ink); restricted Sessions refuse all but reading and the tools
  of their own MCP servers. A tool the agent runs without asking is out of reach of this: on Grok
  an agent profile takes those away (`docs/acp.md`).
- Clearance: a Mission's `start` or `resume` at a clearance `ai6::clearance_runs_commands` denies
  (`desk`, anything else) fails with `GUATIAO_ERR_UNSUPPORTED`, before the agent starts, unless
  `unconfined_desk` is set; answer-only and assistant Sessions are not refused. `weakenings` answers
  one line when `unconfined_desk` is set, whatever the Agent's clearance.
- `clearance_refusal` answers why a clearance `clearance_runs_commands` denies is refused, unless
  `unconfined_desk` is set, so a route of this handler is passed over before a Session starts.
- A prompt whose error contains one of `limit_phrases`, without regard to case, ends as a limit
  event (`resets_at` and `window` unset), not `failed`. The error's text is its message and the
  agent's reason, from a string `data` or an object `data`'s `message` (Grok out of credits:
  `Internal error: API error (status 402 Payment Required): ...`). A lost connection is never a
  limit, whatever the agent last wrote.
- `models(environment)` starts `command args` itself, with exactly the settings' `env` under
  `environment`, asks what the agent offers before any Session (`coqui_acp`'s `describe`, bounded by
  the `describe_*` settings, under `initialize_timeout_ms` and `request_timeout_ms`), and ends it.
  The values of the option of category `model` are the listing; an agent that advertises no such
  answer, or offers no model option, is unknown with the reason.
- An overlay's skill (`AgentStart::overlays`) is a routing line naming its `SKILL.md` in the first
  prompt, after `initial_context`: an ACP agent is any program, and the protocol has no skills.
- `assistant_scripts` and `assistant_skills` keep the trait defaults (none).
- A failed open appends the agent's last non-protocol output lines to the error; a connection
  lost later is `failed` with the agent's exit status and those lines.
- An interrupted turn the agent answers `cancelled` is `completed`; one it cancels by itself is
  `cancelled` with the text `TURN_CANCELLED` (`coqui_acp_provider`'s).

## Grok's login kept in the Registry

Grok runs under this handler. A `harness-login` pool account with `files = [".grok/auth.json"]`. Measured on
1.0.70 with a fake login and a loopback stand-in, never a real one:

- The file is `$GROK_HOME/auth.json`, else `$HOME/.grok/`; a map keyed by issuer and client, with `key`,
  `refresh_token` and an RFC 3339 `expires_at`. `auth.json.lock` beside it holds `pid:time` and is no login.
- `grok models` renews with no model turn within `GROK_AUTH_EARLY_INVALIDATION_SECS` (default 300) of the
  end, and stores the rotated token. At 15 minutes left it does nothing. A round needs
  `renew_env = { GROK_AUTH_EARLY_INVALIDATION_SECS = "3600" }` at `renew_within`, kept well below a new
  token's life: with a window past it, one command refreshed five times, since each reader judged the fresh
  token near its end.
- A refused refresh removes `auth.json` and still exits 0, printing that it is not authenticated.
- A running harness keeps its token in memory: a rewritten file is ignored on its next request, despite the
  documented hot reload, and taken after a 401. So a Session rewritten in place goes on with its old access
  token until that is refused.
- Its guide gives 7-day tokens, 30 days where the server gives no end.
- Unmeasured: whether the provider rotates the refresh token, and whether an old access token stays good
  after a renewal, which is what sharing rests on. Keep `max_concurrent = 1` until the live run across two
  hosts says otherwise.
