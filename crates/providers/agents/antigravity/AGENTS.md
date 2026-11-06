# `ai6-agent-antigravity` — API header

Header-file-style reference for the `ai6-agent-antigravity` crate: every public item with its
signature, contract and gotchas, so a consuming agent skips the source. It ships inside the package
and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No features. MSRV: Rust 1.89. The crate depends on `ai6`, `serde`, `serde_json` and `toml`. It
exposes `AntigravityHandler` and `AntigravityConfig` from the root, under the handler ID
`antigravity`. The crate contains no `unsafe` code.

The CLI is closed source and updates itself. Everything below was measured with **agy 1.4.12 on
x86-64 Linux**, unless it says otherwise, with a real login and a scripted loopback Gemini API
(`modelProvider: "gemini"`, `GOOGLE_GEMINI_BASE_URL`, a fake key, a private `HOME`). Nothing was
measured on Windows or macOS; the crate builds on Windows, and a mode bit it cannot set there is not
set. `tests/fixtures/agy-1.4.12/` holds the recorded lines the tests replay.

## `AntigravityConfig`

Every key is optional. Counts and bytes are plain numbers (a large number takes the bound away); a
key ending `_ms` is milliseconds. A Station sets them in `[providers.antigravity]` of its `station`
record.

| Key | Default | What |
| --- | --- | --- |
| `executable` | `agy` | Executable visible to the terminal environment |
| `json_line_bytes` | 67108864 | Bytes of one line of agy's output held before what there is is handed over |
| `csi_bytes` | 64 | Bytes of one terminal control sequence before it is dropped |
| `tail_lines` | 8 | Non-JSON lines a failed Session quotes |
| `tail_line_bytes` | 512 | Bytes of each of those lines |
| `close_wait_ms` | 2000 | How long `close` waits for agy to have exited |
| `exit_wait_ms` | 1000 | How long the end of the output waits for the exit status |
| `context_file_bytes` | 1048576 | Bytes of an assistant's `context_file`; a larger one is refused |
| `gate_timeout_secs` | 60 | agy's timeout for the green ink hook; past it the command does not run |
| `mcp_timeout_secs` | 0 | agy's `timeoutSeconds` for AI6's MCP servers: positive replaces its limits, negative turns them off, 0 leaves 300 s to connect and 180 s a call (its documentation) |
| `settings_toml` | empty | The Agent's `handler_settings` as TOML; the Station fills it in |

## `handler_settings`

Unknown keys, a malformed rule or a reserved `settings` key make the build fail with
`GUATIAO_ERR_BAD_VALUE`.

| Key | Default | What |
| --- | --- | --- |
| `allowed_tools` | `[]` | agy permission rules (`command(git status)`, `read_url(*)`) added to the allow list, or tool names (`search_web`) added to what a Mission's Session is offered |
| `denied_tools` | `[]` | Rules added to the deny list, or tool names taken out of what it is offered; a deny rule beats an allow rule |
| `settings` | `{}` | Further `settings.json` keys for this Session's `--settings` (`modelProvider`). `permissions`, `toolPermission`, `agentMode`, `trustedWorkspaces` and `allowNonWorkspaceAccess` are refused |
| `auto_update` | `false` | Lets agy's updater run at start |
| `limit_phrases` | see below | What marks a failed turn as a limit, without regard to case |

Answer-only and assistant Sessions ignore `allowed_tools` and `denied_tools`.
`AgentHandler::weakenings` answers one line per `allowed_tools` entry not also in `denied_tools`:
`allowed_tools adds RULE to what its desk Sessions may call` for `run_command` or a `command(…)`
rule, whatever the Agent's clearance, since it can come to work at `desk`; nothing for a default
tool; otherwise `… its field Sessions …`.

## How a launch is made

`stdio()` answers `HarnessStdio::Json`, and `program()` its `executable`, which the Station looks up
on the Session's own `PATH` before it starts it. Start and resume run, in the working directory:

```text
agy --input-format stream-json --output-format stream-json --print-timeout=0s
    --add-dir <SESSION DIR> [--agent=<NAME> | --conversation=<ID>]
    [--model=<MODEL>] [--effort=<EFFORT>] --settings <JSON>
```

and one user message is one line, `{"event":"user","message":{"content":"…"}}`. Without
`-p`, a process takes many turns and exits 0 when its standard input closes.

**The Session's directory.** agy has no flag for an agent, MCP servers or hooks: `--settings`
refuses `mcpServers` ("unknown setting") and does not read hooks, and the user's own files are shared
by every Session of the OS user. It does read `.agents/` of every workspace, `--add-dir`'s too. So
each launch makes `<working directory>.ai6-agy-<unique>`, mode 0700 on Unix, holding
`.agents/agents/ai6-<unique>/agent.md`, `.agents/mcp_config.json` (0600) and `.agents/hooks.json`,
and removes it at `close` or when dropped. It sits beside the working directory because agy keeps
workspaces in path order and runs commands in the first: an `--add-dir` that sorts first makes
commands run there, in either direction, so a name that extends the working directory's path is what
keeps it second. Its parent must be writable. It is measured on a local filesystem and is local to
the handler; a Session whose handler runs elsewhere than its harness is not supported.

**The agent** fixes the tools and is the system prompt: `tools: [...]`, `mainAgent: true`,
`subagent: false`, `inheritMcp`, and the body `# System Prompt\n<context>`. The context is
`initial_context` joined, then an assistant's `context_file`. agy turns each H1 into a tag and keeps
the rest as written, `---` lines included, and offers exactly the listed tools plus `manage_task`,
and, with `inheritMcp: true`, `call_mcp_tool`, `list_resources` and `read_resource`. (With `inheritMcp:
false` those last two are still offered, and `call_mcp_tool` is not.) A tool not listed answers
`unknown tool`, which the model sees. Under `--agent`, the working directory's `AGENTS.md`, `GEMINI.md`
and `.agents/rules` did not reach the request, so project instructions are not read in any mode. A
unique name keeps a checkout's `.agents/agents/` from shadowing it.

**A conversation keeps the agent of its first launch**: its tools and system prompt, under another
`--agent`, with no `--agent`, and with the agent's file gone. So resume passes no agent, and
`AgentResume::initial_context` is not used. `--settings` is read at each launch, so a clearance that
narrows is held by rule (below); one that widens gets no new tool.

**Effort.** `agy models` lists names that end in an effort (`gemini-3.8-flash-low`,
`claude-opus-5-5-high`, `gpt-oss-120b-medium`). `--model gemini-3.8-flash --effort low` is also taken,
as is the bare name, and `--model gemini-3.8-flash-low --effort high` is refused ("conflicts"), exit
code 1. So a model name ending in `-low`, `-medium`, `-high`, `-xhigh` or `-max` gets no `--effort`:
the route's name wins over the Agent's effort. An unknown model exits 1 and lists the models. With
none, agy uses its own default model (Gemini 4 Argon).

**The updater.** `AGY_CLI_DISABLE_AUTO_UPDATE=true` is added to the Session's environment; only the
value `true` works (`1` went on spawning the updater, seen in a fresh `HOME`, which checks at start and
then at most every 15 minutes). With it set, an update happens only when an operator runs `agy
update`, or an Agent sets `auto_update = true`.

**Environment and login.** A harness gets only the Session's environment, so an Agent's `env` sets
`HOME` and `PATH`. The login is a file under `$HOME/.gemini/antigravity-cli/`; with another `HOME`,
agy is not signed in (`ANTIGRAVITY_APP_DATA_DIR` does not move it). A pool's account of kind
`harness-login` hands over nothing and keeps count of its limits. Four processes sharing one `HOME`
at once each finished its turn, on the stand-in model.

## A login kept in the Registry

A `harness-login` pool account with `files = [".gemini/antigravity-cli/antigravity-oauth-token"]`. Seen in
1.4.12's help and program text only, since the only binary here sits beside a live login: Go's `oauth2`
token shape (`token.access_token`, `refresh_token`, `expiry`) and a token source that saves what it
refreshes. `agy models` lists models; whether it renews is unmeasured.

Unmeasured, and assumed unsafe until seen with a throwaway login: its refresh threshold, whether the
provider rotates the token, whether a running harness re-reads the file, what a refused refresh does, and
how long an unused login lasts. Keep `max_concurrent = 1` and set no `renew_command`, which leaves it to
its next real use with a warning before it lapses.

**Overlays.** A launch with a skill whose `native` is set (`AgentStart::overlays`,
`AgentResume::overlays`) copies each one to `skills/<name>/` in the `.agents` directory of
`<working directory>.ai6-agy-skills-<unique>`, and passes that directory as a second `--add-dir`; it is removed with the Session's. Each overlay root
gets `read_file(<root>)` in `allow`. A skill without `native` is a routing line in the system prompt.
Measured with agy 1.4.12 on x86-64 Linux, stand-in model: a `skills/<name>/SKILL.md` in a workspace's
`.agents` is listed in the request under an `--agent` as without one, and its body is read with `read_file`, so
the skills sit in a workspace of their own (the Session's directory is denied to the file tools). A
`read_file` outside the workspaces under no rule ended the turn; with the rule it ran; a `write_file`
there under no rule ended the turn and wrote nothing.

## What a Session may do

Rules go in `--settings`' `permissions` (`allow`, `deny`), never saved by agy. Every launch also
denies `read_file` and `write_file` on the Session's directory, which holds the MCP tokens.

| Session | Tools offered at its first launch | Allow | Deny |
| --- | --- | --- | --- |
| `field`, `00` | `read_file`, `grep_search`, `find_by_name`, `list_dir`, `write_file`, `edit_file`, `run_command`, MCP | `write_file(<wd>)`, `command(*)`, `mcp(<server>/*)` | `write_file` on `<wd>/.git`, `<wd>/.agents`, `<wd>/.agent` |
| `desk`, any other clearance | the same without `run_command` | the same without `command(*)` | the same, and `command(*)` |
| remote-hands (`AI6_THEATER_MCP_*` present) | MCP only | `mcp(ai6-cables/*)`, `mcp(ai6-theater/*)` | `read_file(*)`, `write_file(*)`, `command(*)`; at `desk` also `mcp(ai6-theater/theater_run)`, `mcp(ai6-theater/theater_write)` |
| Assistant (Moneypenny) | `read_file`, MCP | `mcp(<her server>/*)` | `write_file(*)`, `command(*)`, `read_url(*)`, `execute_url(*)` |
| Answer-only | none | none | those, and `read_file(*)`, `mcp(*)` |

Not offered by default: `search_web` (which runs as soon as offered: no rule gates it), `invoke_subagent`,
`define_subagent`, `manage_subagents`, `schedule`, `send_message`, `ask_question`. A model learns a
lazily loaded MCP tool's arguments by reading `<app data>/mcp/<server>/<tool>.json`, which `read_file`
may do with no rule, so an assistant keeps `read_file`. Reading anywhere else outside the workspaces
is a soft denial. What this does not give: `run_command` runs as the Theater's OS user and can read the
Session's directory and its own environment; isolation is a container Theater's.

**A working directory that carries agy's own files.** `.agents/hooks.json` and `.agent/hooks.json` ran,
and `.agents/mcp_config.json` connected, with the directory outside `trustedWorkspaces`, in `-p`-less
stream mode, from the working directory and from an `--add-dir`. (`.gemini/hooks.json` did not run, nor
did `.agents/hooks.json` of a parent directory.) No flag keeps them out. So start and resume refuse
with `GUATIAO_ERR_UNSUPPORTED`, naming the file, when `.agents/hooks.json`, `.agent/hooks.json`,
`.agents/plugins` or `.agent/plugins` exists without `project_settings`, or `.agents/mcp_config.json` or
`.agent/mcp_config.json` exists without both `project_settings` and `project_mcp`. The plugin
directories and `.agent/mcp_config.json` are by analogy and agy's documentation, not measured.
`ai6::PROJECT_FILES` and `PROJECT_DIRECTORIES` hold them, so a pin covers them.

**Green ink.** A Session with a command tool and a Cable endpoint gets a `PreToolUse` hook on
`^run_command$` running, under `sh -c`, `'<PROGRAM>' green-ink gate --cable-url '<URL>'`, with
`gate_timeout_secs`. `PROGRAM` is `AI6_GREEN_INK_GATE` from the Session's environment when not blank,
else the executable hosting the handler. `ai6 green-ink gate` reads the hook's `toolCall.args.CommandLine`
and `workspacePaths[0]` and prints `{"decision":"deny","reason":"…"}` to hold it, nothing to let it go on.
Measured: a hook printing nothing lets the call go on, subject to the permission rules (`allow` does
not lift an Ask); `{}` refuses it; a hook that exits non-zero, is not found or times out refuses it
(`JSON hook … failed`), so the gate fails closed.

**An assistant's mode.** Refused with `GUATIAO_ERR_UNSUPPORTED`, in a sentence: `interpreters` (with
`unsandboxed` too), `files`, `skills`, and any network but one that reaches nothing. An MCP server with
no URL and token in the environment is refused with `GUATIAO_ERR_BAD_VALUE`. `assistant_scripts` and
`assistant_skills` answer the default refusal, so a Station offers neither.

**The tripwire.** A restricted (answer-only or assistant) Session whose stream shows a tool step other
than `read_file`, `call_mcp_tool`, `list_resources`, `read_resource` or `manage_task` (answer-only: only
`manage_task`), or an MCP call to another server, is killed, ends with one `failed` event naming the
tool, and nothing read after that becomes an event. A hallucinated tool ends it too, since the step
appears before agy answers `unknown tool`.

## The stream

`next_event` decodes `--output-format stream-json`:

| Line | Event |
| --- | --- |
| `init` | kind `init`, raw line in `data`. Its `conversation_id` is `session_id()`, `None` until then. agy writes it at start, before reading a message, at no cost |
| `step_update`, `agent_response` | kind `text` at the step's end, its deltas joined, with `usage` when reported; dropped when both are empty |
| `step_update`, `tool` | kind `tool` on a step's first update, `tools` holding `run_command` or, through `call_mcp_tool`, `mcp__<server>__<tool>` |
| `result` with `status` `SUCCESS` | kind `completed`, text `response` |
| `result` otherwise, or with `finished_reason` | kind `limit` when `error` holds a limit phrase, else `failed` with `error` |
| another `event` | passed through under its name, raw line in `data` |
| not JSON, or JSON with no `event` | kind `output`; the last lines are kept for a failure |

**Session ID.** `--conversation` with an ID agy does not hold starts a new conversation, exit code 0,
with `warning: conversation "…" not found` on standard error only. So a resume holds its message until
`init`, and an `init` or `result` naming another conversation kills agy, sends nothing and makes
`next_event` fail ("agy resumed a different Session than requested"), `session_id()` holding what agy
said. A resume finds a conversation from any working directory.

**A refused tool.** Three shapes, each a step with `state` `ERROR` and `tool_info.error.type`
`TOOL_ERROR`:

- no rule (Ask): `permission check failed for … user denied permission …`. **The turn ends there**: no
  further model call, `status` `SUCCESS`, `response` empty, exit code 0, `denied_actions`
  `[{"action":"command","display_name":"RunCommand"}]`, and on standard error `jetski: no output
  produced — a tool required the "command" permission …`. The process stays up for another message;
- a deny rule: `… Matches user-configured deny rule.`, not in `denied_actions`, and the model goes on;
- a hook: `tool call denied by pre-tool hook: <reason>` or `JSON hook … failed`, likewise.

Each comes out as one event of kind `denied` (`DENIED_EVENT`) before the turn's closing event, text the
tools once each, `run_command, write_file`, no `tools` or `data`. `denied_actions`' `display_name` is
used only if no step was seen. `unknown tool` is not a denial: nothing was refused.

**A limit.** Measured only against a stand-in answering 429 to an API key: `status` `ERROR`, `error`
`agent executor error: generating and executing: Error 429, Message: …, Status: RESOURCE_EXHAUSTED,
Details: […]`, exit code 3, standard error adding `AGY_ERROR: {"short_error":…,"status":
"RESOURCE_EXHAUSTED","error_code":429,"code_kind":"http","retryable":true,"error_id":…}`. With a
`QUOTA_EXHAUSTED` reason in the details it did not retry; without one, once. **What a signed-in
account's quota says has not been seen**: its binary holds `You have exhausted your quota on this
model.`, `individual quota reached` and `MODEL_CAPACITY_EXHAUSTED`, and its changelog says reaching an
individual quota is not retried. `limit_phrases` defaults to `RESOURCE_EXHAUSTED`, `Error 429`,
`QUOTA_EXHAUSTED`, `exhausted your quota` and `individual quota reached`; `MODEL_CAPACITY_EXHAUSTED`,
a server's capacity and not this account's quota, is left out. `limit` carries `resets_at` and `window`
as `None`: the stream gives neither, and the windows the `/usage` panel shows are not in it. No event
carries a `QuotaReport`.

**Usage.** A response step's `DONE` update has `usage`, per model call: `input_tokens` without
`cache_read_tokens`, `output_tokens` with `thinking_tokens`, so it is reported as is with `cumulative`
false. `result.usage` is left alone: it runs across every launch of the conversation. agy reports no
cost. `max_cost_micro_usd` has nowhere to go; AI6 enforces its budgets itself.

**End of output.** After an `ERROR` result the exit adds nothing. Otherwise it is one `failed` event,
naming the exit and quoting the tail, or `limit` when the tail holds a limit phrase.

**`interrupt`.** With a turn running it kills agy: a pipe carries no signal, and a `control_request` line
is an `ERROR` result with exit code 2. The end of the output is then a `completed` event, "the turn was
interrupted", which the Station reads as the interrupted turn. The next `send_message` runs agy again
with `--conversation`, holding the message until `init`. A conversation killed in a model call
resumed, with a system message in its next request saying its background work was stopped. With no turn
running it does nothing.

`close` kills the terminal, waits up to `close_wait_ms` and removes the Session's directory.

## What an update could break

The tests would notice a change in the lines recorded; a real run is what notices the rest. In order of
consequence:

- the workspace order that keeps commands in the checkout, and `.agents/` being read from `--add-dir`;
- the rule syntax (an unparseable rule is ignored, so a renamed action fails open), `run_command`'s and
  the file tools' names, and a custom agent's `tools` being an allow list;
- hooks failing closed, and `decision` as the hook's answer;
- `init` arriving before a message, `conversation_id`, and a `result` per turn;
- the words `limit_phrases` and the denial check look for, and `AGY_CLI_DISABLE_AUTO_UPDATE`.

## Gotchas

`agy`'s `--help` lists `--effort` up to `max`; its documentation lists to `high`. Its documentation says
`--print-timeout` defaults to 5 minutes and its `--help` to 0: the handler passes `0s`. Its
documentation says workspace writes are allowed by default; 1.4.12 refused them under no rule, hence
the `write_file` rule. Tests use recorded lines and a fake CLI; no account, CLI or network is needed.
