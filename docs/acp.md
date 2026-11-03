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
own working directory. The client is the [`coqui-acp`](https://github.com/Boriken-Dev/coqui-acp)
library, which refuses an agent that answers `initialize` with another protocol version; AI6 adds
its launch extension, the event stream of its own facade and runner, and what a restricted Session
may use. A line an agent writes is not capped.

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
| Grok | `grok agent stdio`; read [Grok](#grok) before giving it a restricted Session |

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
| `exit_wait_ms` | `1000` | How long the report of a lost agent waits for its exit status. |
| `poll_ms` | `200` | How often a silent agent's output is checked for a stop request. |
| `unconfined_desk` | `false` | Starts a `desk` Session with whatever tools the agent has, in place of refusing it. A weakening. |
| `limit_phrases` | Grok's words and those other agents commonly use | What marks a failed turn as a limit; see [Grok](#grok). |
| `text_flush_bytes` | `65536` | UTF-8 bytes of agent text gathered before they are delivered mid-message. |
| `max_tool_kinds` | `4096` | Tool calls whose kind is kept, by which a later request that does not say its kind is judged. Past it the oldest is forgotten, said once as a `notice`, and such a request is of unknown kind. |
| `describe_options`, `describe_values`, `describe_modes` | `64`, `1024`, `64` | What the model listing keeps of the agent's answer before any Session: options, values per option, modes. |
| `describe_sessions`, `describe_pages` | `500`, `10` | Sessions kept of the agent's session list, and pages of it asked for. |
| `describe_text_bytes` | `4096` | UTF-8 bytes past which an identifier in that answer is dropped and other text cut. |

### Models

The handler lists an account's models without a Session: it starts the agent with that
environment, asks what a Session can start with (the `_coqui_acp/session_options` request, sent only
to an agent that advertises `dev.boriken.coqui.acp/session_options`, such as an agent served by
`coqui-acp-provider`'s facade), and ends it. The values of its option of category `model` are the
listing. An agent that does not advertise it, or offers no model option, is listed as unknown,
with the reason.

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
kinds are allowed. In a restricted Session (answer-only review, Moneypenny) only a request for
reading (`read`, `search`, `think`) or for a tool of the Session's own MCP servers is put to the
gate; every other is refused without asking, whatever kind the agent gives it, and a tool that
edits, deletes, moves or executes and runs without a request ends the Session, after it has run.
None of this binds a tool the agent runs without asking: only the agent's own settings can take
those away. Remote-hands mode, which replaces the harness's own tools, is refused: an ACP agent
cannot be made to give them up.

A request that does not parse, or that cannot be decided or answered for any other reason, is answered
with outcome `cancelled` and a `blocked` event, never with a JSON-RPC error: the Claude ACP adapter
0.99.1 has been seen to go ahead with the tool on an error answer.

A Mission's Session at [`desk` clearance](agents.md#clearance), or a clearance the handler does not
know, is refused at start and resume: `desk` allows no command tool, and AI6 does not choose an
external agent's tools. Give the Agent `field` clearance, or set `unconfined_desk = true`, which
starts it with whatever tools the agent has and is reported as a weakening (`Agent "009" is
weakened: …`, `agents_weakened`). Green ink still decides what the agent asks permission for; a tool
it runs without asking is not stopped.

### Resume

The handler resumes with `session/resume` when the agent advertises it, else `session/load` (the
agent's replay of history is not shown on the board), else the resume fails and the Station starts
afresh with the recap, as it does for any handler that cannot resume. A resume gets the Agent's
`handler_settings`, as a start does.

### When an agent dies

A connection that ends without AI6 closing it fails the Session with `the agent closed its
connection`, how the process ended (`it exited with code 1`, `it was ended by signal 15`) and the
last `diagnostic_lines` lines it wrote outside the protocol. On the `pipe` terminal that is the
last of its standard error, which is what a person sees in the chat.

### Grok

Grok CLI 1.0.70 speaks ACP version 1 as `grok agent stdio`. It advertises `session/resume`,
`session/load`, `session/close` and MCP over HTTP; AI6 uses `session/resume`. It sets
`PR_SET_PDEATHSIG` on Linux, so it ends when the thread that started it does; the `pipe` terminal
starts every program on a thread that lasts as long as its session. Refusing a tool ends Grok's
turn as `cancelled`, so a refused tool costs the whole answer rather than one step of it: a
Mission is blocked, and Moneypenny's answer ends with a notice.

A restricted Session is not restricted on Grok without a profile. These run without asking, as
the Station's user and outside the working directory: `read_file`, `list_dir`, `grep`, `todo_write`,
`enter_plan_mode`, `search_tool`, and `run_terminal_command` with a command Grok takes for read-only
(`ls`, `cat`, `head`, `git log`), which AI6 notices only to end the Session after it ran. `~/.grok/auth.json`
and whatever else that user can read are within reach. Asked, and refused in a restricted Session:
`run_terminal_command` otherwise, `write`, `search_replace`, and, since the kind `other` it gives them
says nothing, `monitor` (which runs any command), `spawn_subagent`, `scheduler_create`, `send_feedback`,
`image_gen` and the other media tools.

The flags `--tools`, `--disallowed-tools`, `--deny` and `--no-subagents` are accepted before `agent`
and change nothing under `agent stdio`. What does is an agent profile, whose `tools` are the
whole of what the model is offered and can call, on `session/resume` too:

```markdown
---
name: moneypenny
description: The AI6 assistant, on her MCP tools alone
tools: search_tool, use_tool
---
You are an assistant. Your only tools are the MCP tools reached through search_tool and use_tool.
```

```toml
[agent.moneypenny]
handler = "acp"
model = "grok-4.7"
[agent.moneypenny.handler_settings]
command = "/var/lib/ai6/.grok/bin/grok"
args = ["--no-auto-update", "--disable-web-search", "agent", "--agent-profile", "/etc/ai6/moneypenny-grok.md", "stdio"]
env = { HOME = "/var/lib/ai6", GROK_DISABLE_AUTOUPDATER = "1", GROK_MEMORY = "0", DISABLE_TELEMETRY = "1", DISABLE_ERROR_REPORTING = "1" }
```

Past its free quota Grok 1.0.70 answers `session/prompt` with JSON-RPC error `-32003`, `Rate limited`,
with xAI's text beside it (`subscription:free-usage-exhausted: You've used all the included free usage
...`) and no reset time. Out of credits, Grok answers `-32603`, `Internal error`, with its reason in an
object (`data.message`: `API error (status 402 Payment Required): personal-team-blocked:spending-limit:
You have run out of credits ...`). The handler reads the error as `Internal error: API error (status 402
Payment Required): ...`, the message with that reason after it, so such a turn stops as limited.
The handler's `limit_phrases` setting says what words mark a failed turn as a limit, found in
that error without regard to case; its default names Grok's and those other agents commonly use. Such
a Session stops as limited rather than failed, so [its route](agents.md#routes) or account is passed
over. An agent that dies is never limited, whatever it last wrote: its Session fails.

`--no-auto-update` and `--disable-web-search` go before `agent`. AI6 cannot tell whether a profile
is set, so nothing warns of an Agent without one. xAI's server-side search for this model is not in
that list and is not settled here: a turn's text goes to xAI either way.

### What an external agent cannot give

ACP v1 carries no token counts, no usage-limit or quota report and no system prompt. An ACP
agent's Sessions therefore show cost only when the agent reports it (`usage_update` with a cost,
converted to millionths of a dollar when it is in USD), AI6 cannot park a Session at a usage limit
or read an account's quota windows from it, and the Agent's context goes in front of the first
prompt as text. The agent also decides how it ends a turn: `end_turn`, `max_tokens` and
`max_turn_requests` complete the Mission's turn, `refusal` and an error fail it, and `cancelled`
completes a turn AI6 asked to interrupt and is otherwise a `cancelled` event, which blocks the
Mission. When a bridge that kept the agent through a lost connection says it dropped some of the
agent's output meanwhile, the turn fails, naming how many lines and bytes were lost.

## Field runners

The runner serves ACP on its loopback port. A connection authenticates with the
`ai6-runner-token` method (the token in the runner's state directory), starts or resumes a Session
under a key, and attaches to a running one with `session/load`, which replays the events it
missed. A newer connection to a Session closes the older. See [the field runner](theaters.md#the-field-runner).
