# The Agent Client Protocol

AI6 drives every Agent Session through the [Agent Client Protocol](https://agentclientprotocol.com)
(ACP): a JSON-RPC 2.0 protocol, one message per line, between a client (the Station) and an
agent. The handler contract (`AgentHandler`, `AgentSession`) stays the Guatiao provider kind;
ACP is the wire between the Station and the harness.

- A Session the Station supervises itself (a local Mission, Moneypenny's harness, a harness
  under the detached wrapper) is an ACP client connection to an in-process **facade** of the
  Agent's handler. The native `claude` and `codex` handlers keep their restricted modes,
  green-ink hooks and limit and quota detection; the facade carries every event they emit.
- A Session on a [field runner](theaters.md#the-field-runner) is an ACP connection to the runner,
  over its loopback port or through `ai6 runner connect` on the Theater's transport.
- The `acp` handler runs **any ACP agent** as an Agent's handler.

AI6 implements ACP version 1 (stable). It does not use ACP's HTTP or WebSocket transports,
proxies or unstable features, and speaks no `fs/*` or `terminal/*` methods: an agent works in its
own working directory.

## The `acp` handler

An Agent selects it with `handler = "acp"` and `handler_settings`:

```toml
[agent.009]
handler = "acp"
model = "sonnet"
clearance = "field"
roles = ["analyst"]
[agent.009.handler_settings]
command = "npx"
args = ["-y", "@agentclientprotocol/claude-agent-acp@0.99.1"]
env = { CLAUDE_CODE_EXECUTABLE = "/usr/local/bin/claude" }
```

AI6 installs no adapter. These exist and are pinned by you:

| Agent | `command` and `args` |
| --- | --- |
| Claude (Node 22 or later) | `npx -y @agentclientprotocol/claude-agent-acp@0.99.1`; set `CLAUDE_CODE_EXECUTABLE` to the `claude` it drives |
| Codex (Node 22 or later) | `npx -y @agentclientprotocol/codex-acp@2.5.0` |
| Gemini | `gemini --acp` |

### Settings

Unknown keys are refused when the handler loads; every number is a setting.

| Key | Default | Meaning |
| --- | --- | --- |
| `command` | required | The program that speaks ACP on standard input and output. |
| `args` | `[]` | Its arguments. |
| `env` | `{}` | Environment added under the Session's own; the Session wins. |
| `meta` | `{}` | A table passed beside AI6's launch data as `session/new._meta`, for an adapter's options (`claudeCode.options`). |
| `model_option`, `effort_option` | the option of category `model` / `thought_level`, else the id `model` / `effort` | The config option ids that carry the Agent's model and effort. |
| `protocol` | `"v1"` | The only version. |
| `initialize_timeout_ms` | `20000` | How long `initialize` may take. |
| `request_timeout_ms` | `30000` | How long a request other than a prompt may take. |
| `start_timeout_ms` | `60000` | How long a facade-style start may take. |
| `close_grace_ms` | `2000` | How long close waits before the process is ended. |
| `diagnostic_lines` | `20` | Non-protocol output lines kept to explain the agent's death. |
| `poll_ms` | `200` | How often a silent agent's output is checked for a stop request. |

### Environment and accounts

The agent process is started through the `pipe` terminal with the Session's full environment,
exactly as a native handler gets it: the roster's `agent.env`, the `AI6_*` variables, credentials
the Station issues, and the pool account's variables. A pool account's `CLAUDE_CODE_OAUTH_TOKEN`
or `CLAUDE_CONFIG_DIR` therefore reaches the Claude adapter, and the Codex adapter reads
`CODEX_HOME` and `CODEX_API_KEY`. ACP has no standard field for this, so AI6 passes nothing
else: the model and effort are set through the agent's config options, and the rest of AI6's
launch data (mission, clearance, environment) travels only to AI6's own facade and runner, in
`_meta["dev.coqui.ai6/launch"]`, where values in `environment` are never logged.

### Permissions and restricted modes

An agent's `session/request_permission` is answered by the green-ink gate, the same check a Claude
`PreToolUse` hook calls: a shell command that pushes to a protected ref, merges or releases is
refused until green ink covers it, and the refusal appears as a `permission` event. Other tool
kinds are allowed. In a restricted Session (answer-only review, Moneypenny) every request for a
tool that edits, deletes, moves or executes is refused without asking, and a tool of those kinds
that runs without a request ends the Session. Remote-hands mode, which replaces the harness's own
tools, is refused: an ACP agent cannot be made to give them up.

### Resume

The handler resumes with `session/resume` when the agent advertises it, else `session/load` (the
agent's replay of history is not shown on the board), else the resume fails and the Station starts
afresh with the recap, as it does for any handler that cannot resume.

### What an external agent cannot give

ACP v1 carries no token counts, no usage-limit or quota report and no system prompt. An ACP
agent's Sessions therefore show cost only when the agent reports it (`usage_update` with a cost,
converted to millionths of a dollar when it is in USD), AI6 cannot park a Session at a usage limit
or read an account's quota windows from it, and the Agent's context goes in front of the first
prompt as text. The agent also decides how it ends a turn: `end_turn`, `max_tokens` and
`max_turn_requests` complete the Mission's turn, `refusal` and an error fail it, and `cancelled`
blocks it.

## Field runners

The runner serves ACP on its loopback port. A connection authenticates with the
`ai6-runner-token` method (the token in the runner's state directory), starts or resumes a Session
under a key, and attaches to a running one with `session/load`, which replays the events it
missed. A newer connection to a Session closes the older. See [the field runner](theaters.md#the-field-runner).
