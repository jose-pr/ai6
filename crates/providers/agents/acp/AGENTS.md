# `ai6-agent-acp` — API header

Header-file-style reference for the `ai6-agent-acp` crate. It ships inside the package and is
self-contained. Development documentation lives at <https://github.com/jose-pr/ai6>.

No features. MSRV: Rust 1.89. Depends on `ai6` (features `acp`, `gate`), the Agent Client
Protocol SDK (`agent-client-protocol`), `futures`, `serde`, `serde_json` and `toml`. Contains no
`unsafe` code. Also builds the test binary `ai6-acp-fake-agent`.

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
    pub limit_phrases: Vec<String>,             // "rate limited", "rate limit exceeded", "usage-exhausted", "usage limit", "quota exceeded"
}
pub struct AcpHandler;                           // AgentHandler provider id "acp"; stdio() = Json (pipe)
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
  event (`resets_at` and `window` unset), not `failed`.
- `assistant_scripts` and `assistant_skills` keep the trait defaults (none).
- A failed open appends the agent's last non-protocol output lines to the error; a connection
  lost later is `failed` with the agent's exit status and those lines.
- An interrupted turn the agent answers `cancelled` is `completed`; one it cancels by itself is
  `blocked` with the text `ai6`'s `TURN_CANCELLED`.
