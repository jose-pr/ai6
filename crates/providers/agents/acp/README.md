# ai6-agent-acp

`ai6-agent-acp` implements AI6's Guatiao `AgentHandler` for any agent that speaks the
[Agent Client Protocol](https://agentclientprotocol.com) (ACP) on its standard input and
output: the Claude and Codex adapters, `gemini --acp`, or an agent of your own. AI6 starts the
agent through its terminal provider with the Session's full environment (roster `env`, `AI6_*`,
credentials, the pool account) and drives it as an ACP client.

An Agent selects it with `handler = "acp"` and `handler_settings`:

```toml
[agent.009]
handler = "acp"
model = "sonnet"
handler_settings = { command = "npx", args = ["-y", "@agentclientprotocol/claude-agent-acp@0.99.1"] }
```

## Settings

`command` (required), `args`, `env` (under the Session's own), `meta` (a table passed as
`session/new._meta`, for an adapter's options), `model_option` and `effort_option` (config option
ids), `protocol` (`"v1"`), and the timeouts `initialize_timeout_ms` (20000), `request_timeout_ms`
(30000), `start_timeout_ms` (60000) and `close_grace_ms` (2000), `diagnostic_lines` (20) and
`poll_ms` (200). Unknown keys are refused when the handler loads.

## Behaviour

- Permission requests go through the green-ink gate; restricted Sessions (answer only, assistant)
  refuse every request for a tool that edits, deletes, moves or executes.
- Cable and assistant MCP servers are passed as HTTP `mcpServers`; an agent without HTTP MCP
  support is refused when the Session needs it. Remote-hands mode is refused.
- The Agent's context and a restricted Session's limits go in front of the first prompt (ACP v1
  has no system prompt).
- No token counts and no limit or quota detection: cost is reported when the agent sends it.

## Development

Run `cargo test -p ai6-agent-acp` from the AI6 workspace root. The tests start a scripted fake agent
(`src/bin/fake_agent.rs`); no Node or model is needed.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
