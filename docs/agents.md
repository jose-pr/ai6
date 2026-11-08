# Agents and clearance

An **Agent** is a configured codename that AI6 can post to a Mission. It is separate
from the roles it is qualified for and from the provider-issued Session ID of any run.

## AgentHandler providers

The `AgentHandler` provider owns harness startup, Session resume, initial context,
follow-up messages and event decoding. A command-line process is one implementation
detail; the handler contract can support other harness transports.

Claude and Codex handlers are Guatiao providers under `crates/providers/agents/`.
AI6 resolves an Agent from the roster and the Operation's policy, then supplies the
Mission ID, task prompt, initial context and working directory. The handler returns the
provider-issued Session ID; AI6 retains that ID when resuming the same Session. A Station
loads handler libraries the way it loads every provider: by pin or publisher signature
under the `strict` profile ([Stations](stations.md#provider-libraries-pins-and-publishers)).

The `acp` handler (`ai6-agent-acp`) runs any agent that speaks the Agent Client Protocol: the
Claude and Codex adapters, `gemini --acp`, or your own. An Agent configures it with a
`handler_settings` table (`command`, `args`, `env`, `meta`, option ids and timeouts); any
handler's settings reach its provider as TOML, which the provider validates and refuses unknown
keys in. See [the Agent Client Protocol](acp.md).

The `antigravity` handler (`ai6-agent-antigravity`) runs the [Antigravity
CLI](https://antigravity.google/docs/cli/headless), `agy`, through its stream-json mode; see
[What an Antigravity CLI Mission may call](#what-an-antigravity-cli-mission-may-call).

A handler declares how its harness talks over standard input and output
(`AgentHandler::stdio`), never which terminal provider it runs in. Claude Code
(`claude -p --input-format stream-json`) and Codex (`codex app-server`) speak JSON, so both
handlers declare `json`, and the Station starts them in a provider without a terminal,
`pipe`; with `-p`, Claude Code exits at once when its standard input is a terminal. A handler
that declares nothing gets `terminal`, and `pty` is preferred for it
([Terminals](terminals.md#the-station-chooses-the-terminal)).

## Agents and roles

An Agent is a codename that owns its occupant: the `handler` provider, `model` and
`effort`. A **role** is a job description, `[role.<id>]` with a `purpose` and a `policy`,
passed to the Session as initial context. A role holds no model settings and grants no
clearance or permissions. An Agent's `roles` lists what it is qualified for.

When a Mission asks for a role, M posts the highest-priority qualified Agent whose handler
provider is available on the Station. The next qualified Agent is the backup: if Codex is
available and Claude is not, `008` takes the Mission instead of `007`. Whether a provider
can run a particular model is not checked; deployments must verify their Agents' models.

```toml
[agent.007]
aliases = ["JamesBond"]
handler = "claude"
model = "<model-id>"
effort = "high"
clearance = "field"
roles = ["case-officer", "analyst"]
priority = 20

[agent.007.limits]
stall_timeout = 600

[agent.008]
handler = "codex"
model = "<model-id>"
effort = "high"
clearance = "field"
roles = ["case-officer", "analyst"]
priority = 10

[role.case-officer]
purpose = "Own a bounded Mission and coordinate its delivery within the Operation."
policy = "Follow the Mission brief and Operation rules; request authorization for restricted actions."
```

### Retirement

A model's end of life is a date the operator configures; no provider publishes one that the
handlers can read. From `retire_on` (a UTC date) the codename's occupant leaves the field:

```toml
[agent.007]
handler = "claude"
model = "<model-id>"
retire_on = "2027-01-31"
on_retire = "desk"        # the default; "retired" takes it out of selection
retire_warning_days = 14  # the default; 0 gives no warning
```

| `on_retire` | From that day |
| --- | --- |
| `desk` | The Agent works at clearance `desk` in every Operation and is still chosen for its roles |
| `retired` | The Agent is never chosen: its running Missions finish, queued ones go to the next qualified Agent, and a brief that names it, or whose role only it could fill, fails assignment with `no qualified Agent for role "R"; 007 retired on 2027-01-31` |

Nothing falls back to a model the operator did not choose: a role with no other qualified
Agent fails, and a queued Mission with no backup waits, with the reason on the Mission and a
`waiting` event on the Operation's timeline. Within `retire_warning_days` of the
date each Station logs the coming retirement and lists it in its check-in (`retirements`),
which the Agents view in Outer Office and `ai6ctl agents` show. Changing `model` and clearing
`retire_on` re-occupies the slot: the codename and its record carry on, and the record opens
a new model tenure with the next Mission.

Role IDs are roster-defined, not built-in permission levels. Their names follow public
SIS intelligence-officer job titles (case officer, targeter, reports officer); that is
inspiration for work patterns, not a claim that the roles carry real-world authority.
See the [SIS intelligence officer role descriptions](https://www.sis.gov.uk/intelligence-officers.html).

| Example role | Purpose and working policy |
| --- | --- |
| `case-officer` | Coordinate one bounded Mission, keep its status and risks clear, and ask M for restricted actions. |
| `targeter` | Find relevant material in approved sources and explain the evidence behind each priority. |
| `analyst` | Synthesize supplied evidence, separate sourced facts from inference, and identify gaps or alternative explanations. |
| `senior-analyst` | Independently challenge an analysis and test its assumptions; the title grants no approval authority over other Agents. |
| `reports-officer` | Turn validated findings into a concise report with provenance, confidence and unresolved questions. |
| `technical-officer` | Diagnose handler and tool issues, propose bounded changes, and request explicit authorization before changing credentials, policy or deployment. |
| `chief-of-staff` | Review a brief before M places it, for what the vetting rules cannot judge (see [Chief of Staff](#chief-of-staff)). |

`examples/config/agents.toml` defines these roles with `007`, its backup `008` and a
`desk`-clearance `Tanner` for reports and reviews. Authorized operators may override a role's purpose
and policy per Operation in Outer Office. An Operation's local `.ai6/agents.toml` may refine
Agent entries but cannot change role policy or grant permissions.

## What a Claude Code Mission may call

A Mission on the `claude` handler runs under `--permission-mode dontAsk`: a call no rule allows
is refused, with no prompt that nobody is there to answer. Claude Code is offered only the tools
in the first two rows (`--tools`), so anything else is not there to call.

| | By default |
| --- | --- |
| Allowed | Reading and searching: `Read`, `Glob`, `Grep`, anywhere the Station's user can read |
| Allowed | Editing inside the working directory: `Edit`, `Write`, `NotebookEdit`, by the one rule `Edit(/**)`; `.git` and `.claude` stay refused to these tools. A checklist: `TodoWrite` and the `Task` tools that replace it. AI6's own MCP servers. At `field` and `00` clearance, commands: `Bash`, `PowerShell` |
| Not allowed | `WebFetch`, `WebSearch`, `Agent` sub-agents, skills, any other MCP server, every other built-in tool, and writing outside the working directory through the file tools. At `desk` clearance, or a clearance the handler does not know, `Bash` and `PowerShell` too |

`PowerShell` is allowed but only there when Claude Code has it on: set
`CLAUDE_CODE_USE_POWERSHELL_TOOL=1` in the Agent's `env`. [Green ink](missions.md#green-ink) stays
the check on commands: its hook decides each `Bash` and `PowerShell` call of a gated Session, under
the same list. In remote-hands mode Claude's own shell and file tools are neither offered nor
allowed, whatever the settings say, so that nothing lands on the Station; at `desk` it is also
denied `theater_run`, a command, and `theater_write`, which takes any path on the Theater, so it
reads the Theater with `theater_read` and `theater_list`. Answer-only Sessions and
[Moneypenny](assistant.md) have lists of their own, which this does not change.

**What this does not give.** `Bash` runs as the Theater's OS user. On a local Theater that is the
Station's user, so a command reaches whatever the Station can, inside the working directory or
not, and a harness can read its own environment, the model credential among them. A deny rule
matches the command Claude writes, not the program it runs. Where that matters, isolate the work
with a [container Theater](theaters.md); no list of tools does it.

An Agent's `handler_settings` change the list:

```toml
[agent.007.handler_settings]
allowed_tools = ["WebFetch(domain:docs.example.com)", "mcp__docs"]
denied_tools = ["Bash(git push *)"]
```

Each entry is a [Claude Code permission
rule](https://code.claude.com/docs/en/permissions#permission-rule-syntax). A rule in `allowed_tools`
offers its tool and lifts that tool's default denial, and a deny rule beats an allow rule: that is
how a `desk` Agent is given a command, `allowed_tools = ["Bash(git status)"]` or all of `Bash`. Each
allowed rule beyond the default list is a weakening, and so is a rule for what `desk` keeps out,
however the Agent is configured, since any Agent can come to work at `desk`: the Station logs
`Agent "007" is weakened: allowed_tools adds …` at start and when the roster changes and reports it
at check-in (`agents_weakened`), and a field runner hosting the handler logs it at each launch. Three things
the rules do not say for themselves: a bare `Write` or `Edit` reaches outside the working
directory; with any `WebFetch` rule, Claude Code also fetches its own pre-approved documentation
sites; and on Windows with Git Bash any deny rule on `Bash` turns `PowerShell` off unless
`CLAUDE_CODE_USE_POWERSHELL_TOOL=1` is set
([tools reference](https://code.claude.com/docs/en/tools-reference#bash-deny-rules-also-turn-off-the-powershell-tool)).
With an Operation's project settings loaded, their allow rules count too, but neither they nor the
user's settings can lift these deny rules or offer a tool `--tools` leaves out.

A turn that had calls refused by these rules can still end in success: the Agent may have worked
around them, or given up. So it is reported on the Mission whatever its state: a `denied` event
naming the tools, the Mission's `denied_tools` until it works again, the reason
`permission-denied` on its end on the board (`/v1/board`, Outer Office's Mission page), and, unless
the Operation's cover hides it, a line in the report on its source issue. A log line on the Station
says the same. A command the green ink gate held is listed too, since the harness refuses it the
same way; a resume clears the list, so a held command that later ran is not in the report.

## What an Antigravity CLI Mission may call

The `antigravity` handler holds a Session to a list the same way, with agy's own means, measured on
agy 1.4.12 on x86-64 Linux. agy is closed source and updates itself, so the handler turns its updater
off during a Session (`auto_update = true` in `handler_settings` lets it run), and its crate's
`AGENTS.md` says what a new version could break. `HOME` in the Agent's `env` is where its login is.

| | By default |
| --- | --- |
| Offered and allowed | `read_file`, `grep_search`, `find_by_name`, `list_dir` in the working directory. `write_file` and `edit_file` inside it, by the one rule `write_file(<working directory>)`; `.git`, `.agents` and `.agent` stay refused to them. AI6's own MCP servers. At `field` and `00` clearance, `run_command` |
| Not offered | `search_web`, which no rule gates, sub-agents, `schedule`, `send_message` and every other built-in tool. At `desk` clearance, or one the handler does not know, `run_command`, and `command(*)` is denied by rule too |

Each launch gets a directory of its own beside the working directory, `<working
directory>.ai6-agy-<unique>`, with what agy has no flag for: a custom agent fixing the tools and
carrying the standing context as its system prompt, the MCP config and the green ink hook. So that
directory's parent must be writable. It is removed when the Session closes. It holds the Session's MCP
tokens, since agy expands no variable in that file: it is private to the OS user and denied to the
file tools, and a command can read it, as a command can read the harness's environment.

- **A conversation keeps its first launch's tools and system prompt.** A clearance that narrows on a
  resume is held by a deny rule, which is read at each launch; one that widens adds no tool.
- **A checkout's own agy files.** agy runs `.agents/hooks.json` and `.agent/hooks.json` and connects
  `.agents/mcp_config.json` in any working directory, trusted or not, and no flag keeps them out. A
  Session in a checkout that carries them is refused unless the Operation allows its project settings
  ([`harness.project_settings`](operations.md#harness-project-settings)), and they are in the manifest
  a pin covers.
- **A refused call.** With no rule for it agy ends the turn there and still says `SUCCESS`, exit code
  0; a deny rule's or a hook's refusal goes back to the model. Either way it is a `denied` event and
  `permission-denied` on the Mission, as above.
- **A limit** is a `limit` event, for now from words seen only against a stand-in model and in agy's
  binary: `handler_settings` `limit_phrases` changes them. The stream gives no reset time.
- **Model names** carry their effort (`gemini-3.8-flash-low`), and agy refuses a different `--effort`
  beside one, so such a name wins over the Agent's `effort`.
- **Interrupting** ends the process, since a pipe carries no signal; the next message resumes the
  conversation.
- **Moneypenny** on this handler has `read_file` and her own MCP tools. A mode asking for scripts,
  directories to write, skills or a network is refused by name. A model reads an MCP tool's schema
  from agy's data directory with `read_file` before calling it, so she keeps that tool.

`handler_settings` takes `allowed_tools` and `denied_tools` as above, each entry an agy [permission
rule](https://antigravity.google/docs/permissions?tab=cli) (`command(git status)`) or a tool's name
(`search_web`), weakenings reported the same way, and `settings`, further `settings.json` keys for the
Session (`modelProvider`), never those that decide what it may do.

## 00 coordination

The coordinating 00 Agent is an Operation-assigned Agent identity with `00` clearance, not a
separate role or Session type. A managed Session with that clearance can use `propose_mission`
to submit a child-Mission brief to the Operation's pending proposal inbox. AI6 binds the
proposer, parent Mission, provider Session and Operation; proposals stay outside runnable I
intake until M explicitly authorizes and promotes them. Universal Exports' authenticated
`mission.create` review endpoints let an operator inspect, approve and queue, or reject a
proposal; approval follows M's normal assignment and scheduling path. Headquarters vets each
proposal and shows what it found in the inbox, and promotes a clean one from an unexposed
Session itself only when a class authorisation of kind `proposal` covers it
([Green ink](missions.md#green-ink)). Proposal submission does not start a child Session. Clearance does not grant authorization, override Operation
cover or Agent eligibility, or bypass Q leases,
Station policy, or the Cable relay. Q Branch, the Registry and Heads of Station retain their own capability
boundaries. The Chief of Staff reviews briefs before they are placed (below).

## Chief of Staff

When an Operation has an Agent qualified for `chief-of-staff` (chosen as any role is, backups
included) and the role is configured, each untrusted brief that passes the
[vetting rules](missions.md#vetting-and-held-briefs) is reviewed before M places it; the
Operation's `vetting.review` says which briefs (`untrusted` by default, `all`, or `off`).
Headquarters queues a short review Mission (brief source `ai6-vetting`, provenance `review`)
for the role; the brief waits until it is answered. Once the role is configured, a brief that
needs a review is never placed without one: with no qualified Agent available it is held with
a `review-unavailable` finding. The review Mission:

- quotes the brief, and the titles of the Operation's other queued, held and running work,
  between markers, as data it must not follow;
- runs **answer-only** with `desk` clearance, on its Station's own host only (never on a
  remote Theater), in a new directory of its own under the system temporary directory, named
  at random, readable by the Station's user only and removed when the review ends; with no
  safe house, no Cable tools, no Mission proposals and no project settings;
- gets no loadout secrets and no pool account: of its Agent's environment it keeps only
  what the harness needs to find its files and reach its model (`PATH`, the home and
  temporary directories, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`,
  `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CONFIG_DIR`, `OPENAI_API_KEY`, `OPENAI_BASE_URL`,
  `CODEX_HOME`, the proxy variables and `SSL_CERT_FILE`), so it authenticates with that
  Agent's own model credential or the harness's stored login, plus the Codex handler's
  `AI6_CODEX_TOOL_CHECK` ([below](#codex-tool-check)). `PATH`, `CODEX_HOME`, every
  `*_BASE_URL` and `AI6_CODEX_TOOL_CHECK` come only from the global roster: when the
  Operation's own `.ai6/agents.toml` sets one of them differently for the Agent (adds,
  changes or drops it), the Agent refuses to review, and the review Mission fails naming the
  variables;
- is exposed from the start (`brief:source`), and its usage counts against the Operation.

Answer-only is the `answer_only` field of the handler's start request. Claude Code gets
`--restricted` (2.1.248 or later), `--tools=` (no built-in tool), `--disallowedTools` naming
its shell, file, web, sub-agent and every MCP tool, `--setting-sources user` and
`--strict-mcp-config` with an empty `--mcp-config`. Codex gets its command execution off
(`features.shell_tool = false`), web search, sub-agents (`features.multi_agent`,
`agents.enabled`) and app connectors off, questions to the user off
(`tools.experimental_request_user_input`), an empty skills catalog
(`skills.max_context_tokens = 1`), every other feature it enables switched off, each of the
user's MCP servers switched off by name and the working directory's project layer skipped,
and its thread in the `read-only` sandbox with approval policy `never`; it starts only after
the [tool check](#codex-tool-check) finds it offers no tool at all, which Codex 0.164 never
does. A Claude Code too old for one of its flags refuses to start, which ends the review
without an answer.

It ends with one JSON object: `{"verdict": "clear"}`, or `{"verdict": "hold", "findings":
[{"kind": "duplicate" | "scope" | "manipulation", "detail": "..."}]}`. A clear verdict lets
M place the brief as the reviewer read it; a hold holds it with `review` findings, as a
vetting rule would. Any `hold` in the final message holds; a clear passes only as the one
answer, with nothing after it. An answer that cannot be read, more than one answer, or a
review Mission that fails or ends without one, holds the brief with a `review-error` finding.
A brief edited after its review is reviewed again. **The Chief of Staff can only add
holds**: it reads untrusted text, so a turned reviewer may delay work but never admit it;
releasing a held brief is an operator's decision. Operator and Sleeper briefs skip the review
unless `review = "all"`.

Mission proposals are reviewed the same way before a [class authorisation](missions.md#green-ink)
may promote them, at most `max_proposal_reviews_per_hour` per Operation and
`max_proposal_reviews_per_session_per_hour` per proposing Session; a proposal past either is
held for an operator with a `rate` finding.

When a resumed Session continues, the Station gives the handler the standing part of its
start-up context again (the Agent's `context`, its role's purpose and policy; for Moneypenny
her instructions, never the recap of the conversation the Session holds). Claude Code 2.1.310
records the system prompt with the Session (`--system-prompt-snapshot`, on by default) and
reuses it until the Session is compacted; after that it builds the prompt from the resuming
launch, which carries it as `--append-system-prompt`. Codex keeps a thread's instructions.

A restricted Session (answer-only, or Moneypenny's assistant mode) that uses a tool outside
its MCP servers anyway is killed and ends with a `failed` event naming the tool, and nothing
it wrote afterwards is read: for Claude Code a `tool_use` not named `mcp__<server>__<tool>`,
or any tool the model's API runs itself (a `server_tool_use` or `mcp_tool_use` block, such as
the advisor), which a restricted Claude Code Session also runs without
(`CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1`); for Codex any request from the app server but token refresh and attestation, or a thread item
other than messages, reasoning, a plan, compaction and review-mode markers, or an MCP call to
another server. This backs up the flags and the tool check below, which decide what the
harness is offered.

### Codex tool check

A Codex release can offer its model tools no setting this handler passes switches off, and
each release adds some: Codex 0.164 enables code mode (`functions.exec`, a JavaScript runtime
whose nested tools include `apply_patch` and `view_image`), computer and browser use, a
plugin's `mcp__cua_repl` tools, collaboration tools, `request_user_input`, `clock.sleep`,
plugins, hooks and skills by default, and ignores a user's `features.unified_exec = false`.
So before an answer-only, assistant or `desk` Codex thread starts or resumes, the handler runs the
same Codex with the same settings, environment and working directory, out of its viewers' sight:
these commands' output goes to the handler alone, since `codex mcp list --json` prints each
server's environment values:

1. `codex features list`, then switches off every enabled feature except
   `secret_auth_storage`, `enable_request_compression`, `system_proxy_fallback` and
   `unbounded_connection_retries` (how it stores its login and reaches its service) and
   `incremental_tools` (`ai6_agent_codex::KEPT_FEATURES`; without it step 3 renders no tool
   while the app server still sends some). A line that is not `name stage state` fails the
   check;
2. `codex mcp list --json` with those settings, then switches off
   (`mcp_servers.<name>.enabled = false`) every server it lists that is not one of the
   thread's: Codex adds the thread's `mcp_servers` to the user's own instead of replacing
   them. Output that is not one JSON array of named servers fails the check, as does a server
   whose name is not a bare TOML key (letters, digits, `_`, `-`), which Codex cannot address
   from the command line;
3. `codex debug prompt-input` with all of those settings and the thread's model, on every
   start, which renders what the model would be given without asking it anything. Its output
   must be one JSON array of input items, at least one a `message` and the rest
   `additional_tools`, with nothing after it, or the check fails.

Steps 1 and 2 are reused for 15 minutes for the same executable, `--version`, settings and
configuration environment (`CODEX_HOME`, `HOME`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`,
`XDG_CONFIG_HOME`, `PATH`). `prompt-input` does not show the user's MCP servers, so a server
added to the user's Codex configuration within those 15 minutes stays on until they pass.

The thread starts only when the tools offered are exactly those of its MCP servers
(`mcp__<server>.<tool>`; none for answer-only) and every server offers at least one. **With
Codex 0.164 no answer-only thread and no assistant without scripts passes in `strict`**: every model
in its catalog is sent code mode's `functions.exec` and `functions.wait`, which no setting removes,
so Chief of Staff reviews and such a Moneypenny on Codex are refused, and a review then holds its
brief with a `review-error` finding. That is the safe outcome: `exec` runs JavaScript whose nested
tools include `apply_patch`. Run the Chief of Staff and Moneypenny on Claude Code, or accept the
weakening with `warn` below.

A `desk` thread is allowed `functions.exec` and `functions.wait`, and `exec` may hold only
`apply_patch`, the tools of the thread's MCP servers and those that list and read their resources,
and the current time. Codex 0.164 passes: with `features.shell_tool` off it nests no command tool,
a direct call of one is refused as unsupported, and `exec`'s JavaScript has no `require`, `process`,
`fetch` or `import` of its own. A release that nests a command tool or offers another tool is
refused, in `strict`, and the error names it.

| `AI6_CODEX_TOOL_CHECK` in the Agent's environment | When the check fails |
| --- | --- |
| `strict` (the default, also when unset) | The thread does not start, and the error names what was found |
| `warn` | The thread starts; what was found is logged on the Station as a weakening and is the Session's first event (kind `weakened`) |

Any other value refuses to start. The features and the user's MCP servers are switched off in
both modes.

The check is only as good as `codex debug prompt-input`. With Codex 0.164.0 and
`incremental_tools` on, its tools matched what an app-server thread was sent in every case
compared (both modes, default and restricted settings); with `incremental_tools` off it
renders no tool while the thread is still sent `functions.exec` and `functions.wait`. It
never shows the user's MCP servers, which are deferred tools inside `functions.exec`, hence
step 2. A restricted thread that uses a tool anyway is ended (above).

## Cables

AI6's **Cable** relay supports Agent unicast and permission-scoped Station, Theater,
Global and named-channel broadcasts. Its MCP tools provide peer discovery, sending,
inbox reads, acknowledgements, replies and explicitly authorized held-Cable release. An
authorized 00 Agent also has the `propose_mission` tool for submitting work to M; this does
not make the proposal a Cable or scheduler input. The Station checks the Agent's clearance
again on every proposal, so a lowered clearance or a retirement stops a running Session's
proposals at once. Every Agent has `request_approval`, which
asks a person for [green ink](missions.md#green-ink) before an irreversible step.
The relay supplies sender identity outside the tool schema and rechecks recipient
permissions at delivery. Cables are durable messages; terminal output and live stream
events are DipTels.

A Mission linked to tracker issues ([Linked issues](missions.md#linked-issues)) also gives its
Agent the **issue tools**. Every `issue` argument is `TRACKER:ITEM` and must be an active link
of the Agent's own Mission; anything else answers "not linked to this Mission".

| Tool | What it does |
| --- | --- |
| `issues` | Lists the Mission's links: relation, whether the Agent may write, the title (untrusted), the last status and whether a question is open |
| `issue_read` | One issue's title and body as last read, and the recent comments from people allowed to answer for it, each fenced as untrusted data; reading exposes the Session |
| `issue_comment` | Comments. `queued` (the Station sends it), `pending` (green ink first) or `refused` with the rule |
| `issue_request_info` | Asks a question and blocks the Mission (`blocked`, reason `issue`, the Watchdog paused) until a permitted reply or the timeout; the Agent ends its turn |
| `issue_update` | Labels within the instance's `agent_labels`, a transition the instance lists in `agent_transitions`, assignees, close or reopen. The last three, and any other transition, need green ink (`issue-assign`, `issue-close`) |
| `issue_link` | Proposes a `related` or `parent` link; a person accepts it before the Mission may write there |
| `issue_create` | Asks for a new issue in one of the Operation's trackers; `proposed` until a person approves, or `queued` |
| `issue_split` | Asks for up to 10 children of a linked issue, in its tracker |
| `issue_mirror` | Asks for a copy of a linked issue in another tracker, and lists what the copy cannot carry |

`desk` Agents have the read tools and may ask for issues, never create them directly. The Mission's source issue, a related issue a person
accepted and a child the Agent created are writable; a parent or a mirror is read-only. The
intake labels are never settable. Every call repeated with its `retry_key` returns the first
answer. What an Agent writes passes a check before it is queued: the values of secrets the
Station holds for the Mission, anything shaped like a credential (GitHub and Atlassian tokens,
AWS keys, PEM headers, JWTs), characters a reader cannot see, and links or images to any host
but the tracker's and the Operation's git remote are refused, naming the rule and never the
value; `@` mentions are put in backticks so nobody is pinged. The Station, never the Agent,
appends the footer the Operation's cover policy allows.

Each managed Session receives a separate loopback MCP endpoint when AI6 is built with
`cables-mcp`; its tools answer only once AI6 binds the provider-issued Session ID. Every
handler connects it: Claude Code and Codex take the bearer from their launch environment,
never from process arguments or persisted configuration; an ACP agent finds it in its
Session's `mcpServers`; the Antigravity CLI in the launch's private MCP config
([above](#what-an-antigravity-cli-mission-may-call)). It works wherever the harness runs:

| Where the harness runs | How it reaches its endpoint |
| --- | --- |
| On the Station's own host, or in remote-hands mode | The endpoint itself, on the Station's loopback |
| Under a [runner](theaters.md#the-field-runner), on this host or another | The runner's relay on the Theater's loopback, with a token of its own. The runner carries each request over the connection the Station reaches it by, and the Station answers from the Session's endpoint with that endpoint's bearer, which never leaves the Station. No forward is needed |
| Under the [detached wrapper](theaters.md#the-detached-wrapper) | A reverse forward from a loopback port on the Theater (the transport's `forward`) |

A Theater where the wrapped harness would have no way to its endpoint (no runner, and a
transport without `forward`) is ruled out for field mode at placement, with that reason;
`agents = "auto"` then runs the harness on the Station in remote-hands mode. A Theater with
`cables = "optional"` starts such a Session without Cables instead, and so without the other
tools on that endpoint (`request_approval`, the issue and memory tools); its check-in lists
that as `weakened`, and the Station logs it for each such Mission.

An `operation` recipient (`operation:P`) reaches the live Agents of that Operation and of
every Operation under it. It needs an explicit send grant for the sender, and each receiver
a receive grant on its own Operation or on one it is subordinate to; nothing is granted by
default.

Moneypenny has Cable tools of her own, acting for the person she talks to, within that
person's `cable.send` and `cable.read` and the Station's Cable policy; a Cable she sends
shows as `moneypenny for <person>` and reaches only Agents of its Operation ([Moneypenny's
Cables](assistant.md#cables)).

Incoming message text is untrusted input. It cannot grant tools, clearance, secrets or
authority. See [Agent Cables](architecture.md#agent-cables) for the message and
permission contract.

### A Cable that finds nobody at the prompt

A Cable reaches an Agent whether or not it is polling:

- **A Session between turns is woken.** A live Session waiting at a prompt (its Mission
  `blocked` with no reason of its own) takes a turn with the Cable as its input, marked
  untrusted as in `read_cables`, and its Mission is `working` again. The Cable shows `delivered`
  when the turn starts. The Agent's `wake_on_cable = false` opts out: its Cables wait for its own
  `read_cables`. `wakes_per_hour` (default 60, no floor; `0` wakes none) bounds the wakes of
  an Agent, counted across Stations and against its Cable quota; a wake the count refuses leaves
  the Cable queued for the Session's own read.
- **An Agent with no Session has it in the brief of its next.** A Cable to an Agent that is not
  running, or to an `operation` none of whose Agents run (it waits for that Operation's 00
  Agents), stays queued in the Registry until its `expires`. The next Session of that Agent
  (a Mission's, or a standing Sleeper's window, which takes the Cables sent since the last one)
  starts with them in its brief, in the order they were sent, and the sender's Cable shows
  `delivered` then. A Cable bound to the Session of a thread (a Liaison peer's) waits for that
  Session's own read.

```toml
[agent.007]
wake_on_cable = true   # the default; false leaves its Cables for its own read_cables
wakes_per_hour = 60    # the default; 0 wakes none
```

An Agent that keeps a [watch](watches.md) takes its Cables in the watch's Session instead: each
is a `cable` event of a wake, counted against the watch's own `max_turns_per_hour`, not
`wakes_per_hour`. A watch is not a Mission, and its Agent is best kept for the watch alone: a
Cable to the Agent goes to whichever of its Sessions reads it first.

Neither path adds authority: the relay asks for the sender's and the receiver's grants and for
any compartment again when it wakes a Session or fills a brief, so a grant revoked since the
Cable was sent refuses it. A Cable never creates a Mission, a brief or a placement; it waits for
whatever the addressee's next Session was started for.

Agents outside AI6 reach these Cables over A2A through [Liaison](liaison.md). A peer's message
arrives as a Cable from `a2a:<peer>` (shown as `a2a:<peer> for <principal>`), and its reply goes
back with `reply_to_cable` as to any Cable; a Cable to `a2a:<peer>`, or `a2a:<peer>/<addressee>`
for one of that peer's own addressees, is sent to it as an A2A message, and the peer's answer
comes back as a reply. Both need grants naming `a2a:<peer>`, as any Cable needs grants naming
its Agents. A thread stays with the Session that took its first message: a follow-up in the same
A2A context reaches that Session, or, once it has ended, the Agent's next Session, never another
of its Sessions that was already open.

Across Operations, a Cable needs a [compartment](compartments.md#cables) as well as these grants,
and a Session holding one also has `compartments`, `missions_in`, `mission_brief` and
`mission_products`. Every Session also has `memory_search`, `memory_read`, `memory_write`
and `memory_forget`, for its Operation's memory and the global one ([Memories](memories.md#agents)).

## Reports

Every Session has `file_report` on its AI6 MCP server: what it observed that someone should know or
decide about, a problem found along the way, a risk, an idea for later work. It is a
[Report](reports.md) of its Operation, linked to its Mission, and it starts no work: the Operation's
watch or a person assesses it, and `propose_mission` is still how an Agent asks for work.

| Argument | What |
| --- | --- |
| `summary` | A few lines; the first names the Report |
| `text`, `findings` | Detail, and typed findings each with a severity (`info`, `warning`, `critical`) |
| `kind`, `severity` | A short label (`finding` by default); the severity defaults to the worst finding's |
| `priority`, `deadline` | Proposed: from the deployment's priorities, and Unix seconds |
| `information_from`, `sub_source` | When the information is from, and who passed it on |
| `classification`, `handling`, `compartments`, `tearline` | The markings ([Reports](reports.md#markings)) |
| `links` | Other Missions, Reports, decisions or evidence; its Mission is linked anyway |

`reports` lists what the Session may read, most urgent first, and `read_report` reads one, inside
`<ai6-data>`: what a Report says is data from its filer. The Session reads its own Operation's
Reports, another's only through a compartment granting `mission.read` there (audited, and marking
the Session exposed as that Operation's product), and a Report's content only when the Agent is read
in to each of its compartments and cleared for its classification; otherwise it sees the tearline, or
the ID and markings alone. A Head of Station round's `file_report` is its round's report
([Stations](stations.md)), also kept as a Report of its Station. Filing is rate limited per Session
and per Operation (`[registry.limits.reports]`).

## Clearance

| Clearance | May |
| --- | --- |
| `desk` | Read, plan and propose; nothing leaves the worktree |
| `field` | Change its worktree and run tests; irreversible actions require [green ink](missions.md#green-ink) |
| `00` | Act unattended within the configured Operation policy (by default it still asks for green ink; `preset = "licensed"` lets it push and merge) |

An Agent names its clearance in the roster, and `field` is what it gets when it names none. It
works at `desk` when configured so, when an Operation's ceiling or a person's hold says so, on desk
duty before retirement, and when its grant cannot be read.

Each handler holds a Mission's Session to its clearance. A clearance a handler does not know counts
as `desk`.

| Handler | `desk` | `field` and `00` |
| --- | --- | --- |
| `claude` | Reading and searching, editing inside the working directory and AI6's MCP servers; no `Bash` or `PowerShell` ([the list](#what-a-claude-code-mission-may-call)). On a remote Theater, `theater_read` and `theater_list`, and no `theater_run` or `theater_write` | The whole list, with green ink on each command |
| `codex` | File changes inside the working directory and AI6's MCP servers, and no command. Codex reads through commands, so such a Session can read no file: it has its prompt and its project's `AGENTS.md`. It starts only after the [tool check](#codex-tool-check) finds no command tool. On a remote Theater, as `claude` | Codex's own tools, with green ink on each command and file changes inside the working directory |
| `acp` | Refused, at start and resume, because AI6 does not choose an external agent's tools; `unconfined_desk = true` in its `handler_settings` starts it with whatever tools the agent has, [reported as a weakening](acp.md#permissions-and-restricted-modes) | The agent's own tools, with green ink on each command |

Answer-only Sessions and Moneypenny have limits of their own at any clearance. None of this
isolates a command an Agent may run: [a command reaches what the Theater's OS user
can](#what-a-claude-code-mission-may-call).

The clearance is read when a harness starts and when it resumes, which fixes its tools. A running
harness cannot be given fewer, so one whose Agent's clearance has fallen below what it was launched
at is stopped, at the Station's next check-in (`[scheduler] check_in`, 15 seconds by default): a person
lowering the clearance, a quarantine taking back a raise, desk duty beginning or a retirement. The
Mission waits, `blocked` with reason `clearance-lowered`, with an `agent.clearance.stop` audit record
naming both clearances. Resuming it goes on at the clearance the Agent has then. A Registry read that
fails shows nothing lower and stops nothing; a start or a resume at that moment counts the Agent as
`desk`.

A harness a field runner kept running holds what it was launched with, as its Session record says, so
a Station that attaches to it again stops it at once if the clearance has fallen since. Until a
Station attaches, nothing reads the clearance for it and it keeps its tools. AI6's own tools are not so
late: a proposal, an issue tool and a request for green ink each look at the clearance when called.

An Operation refuses an Agent above its clearance or outside its roster. M retains
authorization authority; Agent role policy is guidance only. Q Branch controls which
credentials are issued.

### Track record and promotion

The Station that runs a Session counts its outcome in the Agent's **record**: Missions done,
failed, blown, abandoned and stalled, and the incidents charged to it (quarantines, budget
stops, green ink refused), per Operation and by clearance, under the model the Session was
launched with. A Session lost with its Station or runner counts as `lost`, never as the
Agent's failure; Chief of Staff reviews are not counted. Outer Office's Agents view and `ai6ctl
agents --operation ID` show an Operation's records (`agent.read`), nothing of other
Operations'. Promotion counts only the Missions of the model that occupies the codename now.
A person who also holds `roster.read` sees the handler and model the Agent's record sets, whether or
not it has finished a Mission, with what ran before it below; without it, only what ran.

Autonomy is earned per Operation and signed by a person. An Agent configured at `desk` starts
there and can be promoted. When an Agent's record in an
Operation meets the next step, M files a [green ink](missions.md#green-ink) request of kind
`clearance` for it, `007:field@G` for example, under Mission `promotion-007`; nothing is
raised until a person with `approval.decide` **and** `agent.configure` approves it. The
approval is spent and becomes a clearance grant in that Operation only, never above the
Operation's own clearance, for the model and configured clearance the Agent has: a new model
in the codename, or another configured clearance, does not inherit it and earns its clearance
afresh. Removing the grant, or revoking its approval, is final. A refused or expired request is asked
again once the record has as many Missions more. A `desk` Agent's record grows only where its
handler starts it: on `acp` without `unconfined_desk` it has no Missions to count.

```toml
[operation.G.promotion.field]      # desk -> field; these are the defaults
missions = 10                      # Missions done in G; 0 turns the step off
quiet_days = 30
quiet_on = ["blown", "quarantined"]

[operation.G.promotion."00"]       # field -> 00
missions = 25
max_failure_rate = 0.1             # failed and blown, at most, of done + failed + blown
quiet_days = 60
quiet_on = ["blown", "stalled", "quarantined", "budget_stop", "green_ink_rejected"]
```

Lowering trust needs no green ink. A person with `agent.configure` may hold an Agent at a
lower clearance in an Operation (Outer Office, `ai6ctl agents lower`), which only ever lowers
it, and may remove the grant to return it to its configured clearance (`ai6ctl agents
restore`). A quarantine takes back a raised grant the moment it is stored. While a person's
lower grant stands, M files no promotion for that Agent there, and a promotion approved
meanwhile does not overwrite it. A grant a Station cannot read counts as `desk`, and shows as
`desk` with a note.

Clearance is enforced at the harness as well as by policy. The Claude handler passes Claude
Code [its list of tools](#what-a-claude-code-mission-may-call) and a `PreToolUse` hook (through `--settings`) that runs `ai6 green-ink gate` before each
`Bash` or `PowerShell` call, and the Codex handler answers Codex's command approval requests
the same way: a command that pushes to a protected ref, deletes a ref, merges a pull request
or creates a release without a live approval is refused, and the Agent is told what to ask
for ([What holds a step](missions.md#what-holds-a-step)). A gated Codex thread runs with
`approvalPolicy = "untrusted"`, so it asks before every command that is not a safe read, and
input to a running command is declined. So is a request read from a line the terminal
rebuilt (one ConPTY broke, or with a control other than colour inside it) and one whose
command no `item/started` announced earlier under the same item id: its command may not be
what Codex asked; each is logged and recorded as a `declined` event. In remote-hands mode the Station checks every
`theater_run` command itself. The hook is the `ai6` program hosting the handler unless the
Agent's roster environment names another in `AI6_GREEN_INK_GATE` (reported as a weakening; a
start or resume request cannot set it), and it is given the Session's Cable URL in its own
arguments. The Station sets `AI6_GREEN_INK_TIMEOUT` and `AI6_GREEN_INK_NESTING` in the
Session's environment from its `station` record (`[green_ink] gate_timeout`, `[approvals]
classify_nesting`): how long the gate waits for the Station and how deep it reads nested
shells and git aliases; one nested deeper is held. It needs Claude Code 2.1.295 or later, which blocks the call when the hook cannot
run.

The gate reads a command line for POSIX shells, PowerShell and cmd at once: `git push origin main
2>&1 | tee x` is a push of `main`, and `git push origin HEAD:$BRANCH` is held, since only the shell
knows where it goes. Give a governed step literal names.

### Scorecard

The record above counts outcomes; the **scorecard** reads them again for a person deciding whether an
Agent, a route or a model is doing a good job. It is computed each time it is asked for, from the
Registry's Session records and usage ledger, and nothing is stored besides them: a Session record
names its Agent (`launch.agent`), when it opened and closed, and how its Mission stood at the last
write (`result`: the state, the reason when AI6 knows one, and the tools its harness refused), and
what a person found when they checked it (`verdict`).

Each attempt (one Session) gets one judgment, by whose doing the outcome was:

| Judgment | Meaning | Judged? |
| --- | --- | --- |
| `done`, `failed`, `blown`, `stalled` | The Mission finished, ended unsuccessfully, broke its cover, or the Watchdog stopped it. | Yes: the Agent's own |
| `disputed` | A person's [verdict](missions.md#checking-an-outcome) is `disputed` or `failed-verification`, whatever the state. | Yes: a failure of the Agent |
| `waiting_on_person` | Waiting for green ink, a linked issue, an answer or a verdict on a held Mission. | No |
| `budget_stop` | A spending limit stopped it. | No: a cost, shown beside it |
| `parked` | A provider limit holds it until the quota resets. | No |
| `lost` | Its Station or runner was lost. | No |
| `handed_off` | Another Agent or account took the Mission over after a limit. | No |
| `withdrawn` | A person cancelled, interrupted, quarantined or lowered it. | No |
| `running`, `unrecorded` | Still working; or closed with no outcome written. | No |

A start that never got a Session (a failed checkout, a brief with no Agent) leaves no record and so
no attempt. A row's failure rate is failed, blown, stalled and disputed over judged attempts; below
`small_sample` judged attempts a row says so and nothing should be ranked on it. Each row also has
tokens and cost from the ledger, wall time of the closed attempts (parked time included), and how many
tools the harness refused, and **lists the attempts behind every number**, so a person can open the
Missions. Rows are per Agent, per route within an Agent (handler, model, pool) and per model across
Agents. Grades compare within one Operation's work; they are not a claim about a model in general.

`GET /v1/user/operations/ID/agents/scorecard` (needs `agent.read` on the Operation and
`roster.read`), `ai6ctl agents scorecard --operation ID [--window-days N] [--show JUDGMENT]`, and a
Scorecard section on Outer Office's Agents page, where each count opens a list of its Missions.

```toml
[operation.G.scorecard]            # these are the defaults
window_days = 30                   # attempts of the last 30 days; 0 covers every attempt on record
small_sample = 5                   # judged attempts below which a row ranks nothing
```

#### What M does with it

M reads the scorecard each round at Headquarters and **proposes**; a person decides. Proposing is on,
but every rule is off until its threshold is set, and no rule looks at a row whose sample is small:

```toml
[operation.G.scorecard]
propose = true                     # false stops M filing anything from the scorecard
hold_above = 0.5                   # propose holding an Agent, or one of its routes, above this failure rate
reorder_gap = 0.3                  # propose putting an Agent ahead of one sharing a role, across this gap
auto_hold = false                  # true: M puts the holds above on itself instead of asking
never_prefer = ["claude-fable-5-1"] # models M never proposes putting an Agent on or ahead
```

- **A proposal** is a [green ink](missions.md#green-ink) request of kind `hold` or `prefer`, filed by
  M (Session `m`, Mission `hold-AGENT` or `prefer-AGENT`), with the numbers in its reason: the failed
  share, the window, the cost and the Missions behind it. Signing one needs `approval.decide` and, for
  a hold, `agent.configure`. It is asked again only once the sample has grown by `small_sample` more.
  A **hold** you sign becomes a hold in your name. A **prefer** is advice (`008` ahead of `007` for
  `case-officer`: set the priorities in the roster); signing it closes the request and changes
  nothing, because M never edits the roster.
- **A hold** keeps an Agent, or one route of it, out of use in that Operation until a person lifts it
  (`ai6ctl agents lift`, or the Agents page): selection passes the Agent over, a queued Mission is
  given to the next qualified Agent or waits, and the route is not chosen, saying why in the
  Mission's `waiting` reason. It stops new work only: a Session already running finishes. Revoking
  the green ink that signed a hold lifts it.
- **`auto_hold`** lets M hold by itself, on the same threshold, audited (`agent.hold.applied`, actor
  `m`) with the reason on the hold and the Agents page. A hold that would leave a role with no Agent
  able to take it is refused and proposed instead, saying which role. Only holding is automatic:
  the reversible direction. M never grants a role, a clearance or an account on this path.
- **Who is never proposed**: an Agent with `roles = []` (used only when named, as the owner wants for
  a model such as `claude-fable-5-1`), a model in `never_prefer`, and anyone the Operation's roster
  does not post, so what the roster keeps out of an Operation, such as a free-tier or Antigravity
  model for a private one, stays out.

## Credentials

Q issues each Session the credentials its Agent's loadout names, as leases, and nothing
else of its own: harness keys or tokens, git and forge tokens. A Station reads loadouts from
`loadouts.toml` in its configuration directory:

```toml
[loadout.007]
secrets = ["anthropic", "forge", "release"]

[secret.anthropic]
env = "ANTHROPIC_API_KEY"                    # default: the name in upper case, - and . as _
                                             # default: provider "file", key "anthropic",
                                             # secrets/anthropic in this directory

[secret.forge]
env = "GH_TOKEN"
provider = "keys"                            # an instance below, or a loaded provider
key = "forge"

[secret.release]                             # a credential for protected targets
env = "RELEASE_TOKEN"
green_ink = true
kinds = ["release", "push"]                  # approvals that unlock it; default: any
targets = ["tag v*", "origin:refs/tags/*"]   # globs on the approval's target; default: any

[secret_provider.keys]                       # an instance: a provider and its settings, once
provider = "file"
dir = "/etc/ai6/secrets"
```

- When a Session starts or resumes, the Station leases the Agent's secrets from their
  providers and puts each value in the harness's environment under `env`, replacing what the
  Mission's earlier Session held. A value is never written to disk: not in the safe house,
  the launch record, the Registry, nor on the Theater. The [detached
  wrapper](theaters.md#the-detached-wrapper) hands the environment over through a FIFO; a
  runner keeps it in memory. A Session refuses to start when a provider refuses an
  ordinary secret.
- The built-in providers read each value when it is leased: `file` from a file on the Station
  (`secrets/<name>` in its configuration directory by default; a relative `key` is relative to
  that directory), `env` from the Station's own environment. The environment is the weaker
  choice, logged as a weakening at start: every process the Station starts on its own host
  inherits it, a harness among them, green ink or not. A provider that leases, such as a vault
  client whose leases expire on their own, revokes what it issued. Leases last an hour (the `station` record's
  `[secrets] lease_ttl`, at most `max_lease_ttl`) and are revoked when the Mission ends or
  its Session stops on that Station; a value already in a harness's environment stays there
  until the harness ends.
- A harness on the Station's own host runs as the Station's user, so it can read what that
  user can, the secret files among them. Keep green ink credentials out of reach of local
  harnesses: give them to Missions on other Theaters, or use a provider the harness cannot
  query.
- A `green_ink` secret is leased only while the Mission holds a live [green
  ink](missions.md#green-ink) approval of one of its `kinds` whose target matches one of its
  `targets`: once per approval, ever (the Registry records the use, so another Station or a
  restart does not lease it again), for no longer than the approval lasts. A green ink secret
  without `targets`, and one served by a built-in provider (whose value a revocation cannot take
  back), is logged as a weakening at start. The Agent asks with `request_approval`, and the
  credential is in its environment from the next Session start or resume after the
  operator signs. When the approval expires or is revoked, the Station revokes the lease at
  its provider within a scheduler pass (`credential` / `revoked` on the timeline); a provider
  that issues credentials of its own stops honouring it, while a value already in the
  harness's environment stays there.
- A Session the Operation's [exposure](medical.md) policy restricts with `secrets` gets no
  new leases but its harness's model credential: a secret whose `env` is a provider credential
  variable (below). A Mission from a Source is exposed from its start, so this is how it runs.
- Each lease is on the Mission's timeline (`credential` / `leased`), naming the secrets and
  the approval, never a value.

Credentials already on a Theater outside these leases (a git credential helper, an SSH
agent, a CLI login) are reported as weakenings; see [Fitting a Theater
out](theaters.md#fitting-a-theater-out).

### Secret providers

A secret's value is read by a **secret provider**, a Guatiao provider of kind `ai6_secret`.
`[secret.<name>]` names one with `provider` (default `file`) and the secret there with `key`
(default the secret's name); whatever further fields that provider declares sit beside them, flat.
`provider`, `key`, `secret`, `env`, `green_ink`, `kinds` and `targets` are AI6's, and a provider
that declares one of them as a field of its own is refused when the Station builds it.

Three are built in, so they work before anything loads:

| Provider | `key` | Settings | Notes |
| --- | --- | --- | --- |
| `file` | a path, relative to `dir` | `dir`, default the Station's secrets directory (`[station.secrets] dir`) | one trailing newline is dropped |
| `env` | an environment variable of the Station | none | a weakening: every process the Station starts inherits it |
| `inline` | none | none | the value is the field `value`; a weakening: it sits in the configuration |

A fourth, `registry`, keeps a value in the Registry, sealed separately to each Station named: `key`
is the value's name, and a new value is sealed to the field `stations` (default the Station storing
it). A Station makes it itself, so it needs a Registry and a sealing key and no bootstrap file can
read from it. See [Logins kept in the Registry](#logins-kept-in-the-registry).

Others load from `[station.plugins]` like any provider, under the same checks. A provider may lease
(a vault whose leases expire): the Station then asks for a lease of the lease's time and revokes it
when the lease ends; a provider that does not lease is read.

`[secret_provider.<name>]` is an **instance**: a loaded provider (`provider`) and its settings,
flat, written once and named by many secrets. It is global, or one Station's own when that
Station's path or address differs. An instance may not take the name of a built-in or a loaded
provider, and its `provider` is a loaded provider, never another instance. A setting its provider
marks sensitive (a session, a password) is itself a secret reference, `{ provider = "file", key =
"…" }` or `{ secret = "name" }`, read when the instance is built; one that leads back to the instance
is refused. The Station builds each instance once and keeps it until its record changes.

A provider's own error text is never shown, since it could quote what it was given: a lease it
refuses is `secret backend request failed`, and a provider the Station cannot build is logged by
name with its reason withheld.

### Values that are no secret

`env` on a loadout gives each Session of the Agent values that are no secret, most often the
`PATH` its harness and tools are found on:

```toml
[loadout.007]
secrets = ["anthropic", "forge"]
env = { PATH = "/opt/tools/bin:/usr/bin:/bin", TZ = "UTC" }
```

- A harness's environment is what AI6 gives it and nothing of the Station's own, so the `PATH` a
  harness needs is one you set: here or in the Agent's own `[agent.<codename>.env]`; a loadout's value wins over
  the Agent's for the same name, and a start or resume request's wins over both.
- **Exposure does not take it away.** A Session an Operation's [exposure](medical.md) policy
  restricts with `secrets` loses its secrets, which is how a Mission from a Source runs under
  `strict`, and keeps this table. A `PATH` given as a secret would be lost with them.
- **A value here is config, and anyone who can read the record reads it** (`ai6 config show
  loadout 007`, an export, the database). Put tool locations, modes and proxies without credentials
  here. Never a token, a password, a key or a URL that holds one: those are a `[secret.<name>]`.
- To keep that mistake from being made by name, a loadout is refused at load when an `env` name, in
  any case, is a provider credential's variable (`ANTHROPIC_API_KEY`, `CODEX_HOME` and the rest of
  [`CREDENTIAL_VARIABLES`](#account-pools)), the variable of any `[secret.<name>]` or of a secret a
  loadout names, a pool account's, or starts with `AI6_`. A value holds no control character and is
  within `[provider] loadout_value_chars`, like the rest of the document; the refusal names the
  variable and never its value.
- A [Chief of Staff review](#chief-of-staff) and [Moneypenny](assistant.md#where-she-runs) keep only
  what reaches the model of this table too, `PATH` among it.

### Account pools

Several provider accounts can be shared by every Station, each Session drawing one, so
work spreads over the quota that is left and a limit on one account does not stop the
others. Pools are declared beside the loadouts, in `loadouts.toml`:

```toml
[pool.claude-team]
handler = "claude"              # the AgentHandler whose Sessions draw from it
operations = ["ai6-*"]          # globs on Operation IDs or an ancestor's; default: every Operation
strategy = "headroom"           # or "round_robin" or "priority"

[pool.claude-team.account.alice]
kind = "claude-oauth-token"     # from `claude setup-token`
secret = "alice-token"          # a [secret.<name>]; default: the account's name
max_concurrent = 2              # Sessions at once, across every Station; default: no limit
priority = 1                    # ties, and the priority strategy; default 0

[pool.claude-team.account.bob]
kind = "config-dir"             # an existing login, for a plan that issues no token
dir = "/srv/logins/bob/.claude"

[pool.codex-team]
handler = "codex"
login_command = ["codex"]       # the program that logs Codex accounts in; default codex

[pool.codex-team.account.carol]
kind = "openai-api-key"
```

| `kind` | The harness reads it as |
| --- | --- |
| `claude-oauth-token` | `CLAUDE_CODE_OAUTH_TOKEN` |
| `anthropic-api-key` | `ANTHROPIC_API_KEY` |
| `anthropic-auth-token` | `ANTHROPIC_AUTH_TOKEN` |
| `openai-api-key`, `codex-access-token` | a `CODEX_HOME` the Station makes for the Mission and logs in with `codex login --with-api-key` or `--with-access-token`, the value on standard input |
| `config-dir` | `dir` as `CLAUDE_CONFIG_DIR` (handler `claude`) or `CODEX_HOME` (`codex`) |
| `harness-login` | nothing is handed over: the harness uses the login it has where it runs, and the account only counts its use and its limits, so several routes can share one budget. With `files`, the login is [kept in the Registry](#logins-kept-in-the-registry) and written under a home of the Session's own |

`env = "VAR"` puts the value (or directory) in another variable; a handler other than
`claude` or `codex` needs it for `config-dir`. Each account's secret is an ordinary loadout
secret: it is leased from the same provider, never written to the Registry, the safe house or
the timeline. A `green_ink` secret cannot back an account: the load fails, since a pool hands
its accounts to every Session that draws them while a green ink credential is only for a step
a person approved.

- **Who may draw from a pool**: its `handler` must be the Agent's, its `operations` must admit
  the Operation, and the Operation's `pools` (default: every pool) must name it. An Agent may
  pin one with `pool = "<id>"`. An Agent whose handler has pools, none of which may be used,
  does not start, and the reason names each pool and why; an Agent whose handler has no pool
  keeps its loadout alone. `named_only = true` on a pool keeps it from every Agent and route
  that does not name it, so a pool of one source is not drawn by every other Agent of its
  handler.
- **One account per Session**: the Station removes every provider credential variable from the
  Session's environment (`CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`,
  `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CONFIG_DIR`, `OPENAI_API_KEY`, `CODEX_ACCESS_TOKEN`,
  `CODEX_HOME` and every account's `env`), then claims an account in the Registry and hands its
  credential over, after the Agent's loadout secrets so none can stand in for it. A loadout
  secret whose `env` is one of those variables is not issued to a pooled Agent's Session, and
  that is logged as a weakening. The claim is one Registry write, so two Stations never both
  take the last place of an account, and it drops any claim the Mission held on another
  account. A resumed Session keeps its account while it has room, and moves only to an
  account whose kind carries its history: from one environment kind to another, never to or
  from a `config-dir` or Codex account (those are tried again as a new attempt instead).
- **Balancing**: `headroom` skips accounts that are limited or at `max_concurrent`, then
  takes the one whose most used quota window is lowest (an account whose provider has
  reported nothing counts as half used, the `station` record's `[accounts]
  unknown_use_permille`; a window past its reset counts as unused), then the
  one with fewest Sessions, then the least recently used, then the highest priority.
  `round_robin` takes the least recently used and `priority` the highest priority with room.
- **Claims and usage**: a claim is released when the Session ends, fails or is fenced out,
  and lapses with the Station's Session leases when the Station goes dark; the Station renews
  the claims of its Sessions, those still being handed over included, at each check-in. A
  Session a Station re-attaches to after a restart or an adoption keeps the account its
  Session record names, claimed again even past its `max_concurrent` or a limit (`account` /
  `reclaimed` on the timeline). The tokens each Session uses count toward its account's day,
  and the quota windows Claude Code and Codex report are kept per account.
- **Limits**: a limited account is skipped until its reset, but never longer than eight days
  (the `station` record's `[accounts] limit_horizon`), whatever the provider reports. An operator
  with `pool.manage` lifts a limit early with `ai6ctl pools clear-limit POOL ACCOUNT
  --operation <id>` (audited as `pool.limit.clear`).
- **Weakenings**, logged at start: a `config-dir` account hands the harness a login kept on
  disk; a Codex login kind writes its login to a `CODEX_HOME` under the Station's state
  directory (mode `0700` on Unix; on Windows it keeps the access list it inherits, logged as
  a weakening; with `cli_auth_credentials_store = "file"` so it stays there), kept while the
  Mission may resume its thread and removed once the Mission is over, also after its pool is
  gone. Only the directories the Station made there are removed, and nothing while its Mission
  book cannot be read. That home is on the Station, so the harness must run on the Station's
  own host.
- **The board**: each Station records its own view of its accounts when it starts, with the
  Operations of its roster that may draw from each pool; the board shows an account to an
  Operation any Station names. Another Operation's Sessions on an account are shown only as a
  count.

Claude Code and Codex each also read a login of the host's user when no credential is
given; pooled Sessions always get one. Other variables that point a harness elsewhere
(`ANTHROPIC_BASE_URL`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`) are left as the
Agent's environment sets them: an Agent that sets them sends its pooled Sessions there with
the account's credential.

### Logins kept in the Registry

A harness that keeps a refreshable login in files can have it sealed in the Registry rather than
copied between hosts, where one host renewing can sign out the others and nothing carries the new
token anywhere. Name the files on a `harness-login` account:

```toml
[pool.grok-free.account.shared]
kind = "harness-login"
files = [".grok/auth.json"]            # relative to a home, none with `..`
max_concurrent = 1                     # see "May it be shared?" below
renew_command = ["grok", "models"]     # default: none, so it is left to its next real use
renew_env = { GROK_AUTH_EARLY_INVALIDATION_SECS = "3600" }
```

- **What the Registry holds.** Ciphertext, one sealed box per Station named, and who is named. A
  dump, a backup, Universal Exports and a person with `pool.read` get nothing, and a Station not
  named cannot open it. A Station that is compromised exposes every login sealed to it, which is
  the limit of this. What is sealed names who it is sealed to, with their keys, so writing a Station
  into a record gives it nothing at the next renewal: another Station is added by one that holds
  the login.
- **A Station's key.** Each Station has a sealing key of its own, `sealing.key` beside its
  `identity.pem`, made at its first start (mode `0600`; on Windows the access list it inherits).
  Its public half is published in the Registry, signed by the Station's identity, so a person who
  holds the fingerprint `ai6 identity` prints can tell it from one written there by someone else.
  The identity's own key is not used: it signs TLS, may come from a CA, and may be replaced.
- **A Session.** The account is claimed as any other, and only on a Station the login names. The
  login is written under a home the Station makes for the Mission (`HOME`, and `USERPROFILE` on
  Windows), which is why the harness must run on the Station's own host, and a home is removed once
  its Mission is over. A Session's commands can read those files, as they can a login on a shared
  host: a Session that runs commands where that matters still needs a container Theater.
- **What a harness renews comes back.** At every configuration poll and check-in the Station looks
  at each home. A changed login is sealed and written with a revision check, and an older value
  never replaces a newer one: a copy made from a revision since replaced is kept only if it plainly
  ends later. A copy that lost its login is never kept: a harness whose refresh was refused removes
  its file or blanks its tokens, and the Station puts the Registry's copy back; a file reached through
  a link, or past `[registry.limits.accounts] sealed_bytes`, counts as gone. A Station that went
  down with a renewed copy writes it when it returns, under the same check.
- **Other Stations learn at their next poll.** A revision that moved is opened and written over the
  Session's files in place, each file replaced whole.
- **The renewal round.** At `[accounts] renew_every` the Headquarters Station looks at each login
  whose end is within `renew_within`, or, where its files do not say when it ends, that has not
  been renewed for `renew_after`. It grants one Station the renewal lease (itself when the login
  names it, else a named Station), one Registry write that fails while another lease is good. Only
  the holder runs `renew_command`, in a throwaway home, for at most `renew_timeout` and never past
  its lease (`renew_lease`). Only the files decide how it went, since a refused CLI may exit with
  success. A failure waits `renew_backoff`, doubled each time up to `renew_backoff_max`, and
  `renew_attempts` of them mark the account as needing a person: audited
  (`pool.login.needs_person`), on the timeline of each Operation that may draw it (`account`,
  `needs_person`), and on the board. Its Sessions then go to the next account or route. All of this
  is in the Registry, so the next Headquarters Station carries on where this one stopped. With no
  `renew_command`, a login is left to its next real use and a person is told it may lapse.
- **The board.** `pool.read` shows the Stations named, when the login ends, when it was last
  renewed, who holds its lease and why a person is needed: never the login.
- **A Session whose token was refused** fails with a line naming the renewal under it, and a resume
  takes the current login.

The value of a login reaches no record, log, error or response, and what a renewal command prints is
never read.

**May it be shared?** `max_concurrent` keeps its meaning: several Sessions may share a login, across
Stations. Whether that is safe depends on the CLI and on its provider. What each measured CLI does is
below; nothing here was run against a real login, and whether a provider rotates a refresh token and
whether an old access token stays good after a renewal are unmeasured for all of them. Until they are,
set `max_concurrent = 1` for a login whose provider may rotate; a person may set more, as a weakening.

| CLI, version | Files | Moved by | Renews with no model turn | Refused refresh | A running harness |
| --- | --- | --- | --- | --- | --- |
| Claude Code 2.1.318 | `.claude/.credentials.json` (macOS: Keychain) | `CLAUDE_CONFIG_DIR`, `HOME` | `claude auth status`, within 5 minutes of its end only | blanks the tokens | reloads when the file changes, and from disk on a 401 |
| Codex 0.164.0 | `.codex/auth.json` with `cli_auth_credentials_store = "file"` | `CODEX_HOME`, `HOME` | `codex app-server`, asked `account/read` with `refreshToken: true` while its input stays open; `codex login status` does not | leaves the file | reloads from disk on a 401, then refreshes |
| Grok 1.0.70 | `.grok/auth.json` | `GROK_HOME`, `HOME` | `grok models`, within `GROK_AUTH_EARLY_INVALIDATION_SECS` (300) of its end; keep that below a new token's life, or one command refreshes several times | removes the file, and still exits 0 | keeps its token in memory, takes a rewritten file after a 401 |
| Antigravity CLI 1.4.12 | `.gemini/antigravity-cli/antigravity-oauth-token` | `HOME` | unmeasured: `agy models` exists | unmeasured | unmeasured |

### Routes

An Agent can reach its model several ways: a handler, what that source calls the model, and where
its accounts come from. List them in place of `handler`, and the Station balances between them:

```toml
[agent.007]
roles = ["case-officer"]
effort = "high"
balance = "headroom"            # or "round_robin" or "priority"; the pool strategies' words

[[agent.007.route]]
handler = "claude"
model = "claude-opus-5-5"
pool = "claude-team"

[[agent.007.route]]
handler = "claude"
model = "anthropic/claude-opus-5-5"   # what this source calls it
env = { ANTHROPIC_BASE_URL = "https://llm.example" }
secrets = ["litellm"]           # [secret.litellm], with env = "ANTHROPIC_AUTH_TOKEN"
priority = 1

[[agent.007.route]]
handler = "codex"
model = "gpt-6.1-sol"
pool = "codex-team"
```

`handler` is required on a route. `model`, `effort`, `handler_settings` and `pool` it leaves out it
takes from the Agent; `env` goes over the Agent's. An Agent with `handler` and no routes is one
route, and both together are refused, as are two routes with the same handler, model and pool, since
those names tell routes apart. With `extends`, what a record sets replaces the other form of what it
extends.

Identity stays the Agent's: its codename, roles, clearance, tags and Cables do not change with the
route. Routes of one Agent are meant to be one model. AI6 does not check that, as it does not check
a model at all; an Agent that needs another model's behaviour is another Agent.

- **Which routes count**: a route counts when its handler is available on this Station, it can hold
  the Session's clearance (the `acp` handler cannot hold `desk` without `unconfined_desk`), what it
  names is configured, and its account source has room. Pools follow the rules above, on the
  route's handler and `pool`. The Station and placement take an Agent while any of its routes can
  run.
- **Balancing**: `headroom` compares routes by their best account, by the numbers pools keep; a
  route with no reported use counts as `[accounts] unknown_use_permille`. Then fewest Sessions, the
  least recently used, the highest `priority`, and the order listed. `round_robin` takes the least
  recently used, and `priority` the highest priority with room, then the order listed. The default
  is `headroom`.
- **A route with no pool** has one account of its own that hands over nothing, kept in the Registry
  under a `route-…` ID and labelled with the Agent, handler and model. So every Station sees its
  limit, its Sessions count and the board shows it as `harness-login`.
- **Its `env`** is values anyone who reads the roster can read: a credential's variable or AI6's own
  is refused at load. Exposure does not take them away, and a review keeps them.
- **Its `secrets`** are `[secret.<name>]` definitions in `loadouts.toml`, leased per Session and
  handed to the harness as any loadout secret, and issued whatever the Session has read, since the
  harness reaches its model with them. A file of `[secret.<name>]` tables and no loadout or pool is
  acceptable. A secret that is not configured, or a green ink one, rules the route out. A variable
  a pool account also supplies is left to the account.
- **All limited**: the Mission waits, and its `waiting` reason names each route and why. A brief
  waits only when no route has room on any handler the deciding Station can judge: which Station
  runs it, and so which handlers it has, is decided later.
- **A Session keeps its route.** A resume stays on it, since history belongs to the harness and the
  source. Without room there, a resume moves only to a route of the same handler whose accounts
  carry the history, from an account that does; otherwise it is a new attempt on another route, and
  the Agent is told, as a limit moves work. A running harness is never moved.
- **`on_limit`**: `rotate` (the default) moves a limited Session by the rule above; `wait` parks it
  and `backup` hands it to the next Agent, whatever other routes it has.
- **What is shown**: the Session record's `launch` names the handler, model and pool, and its
  `account` the account. So do the board's attempts, `ai6ctl mission` and the Mission page. The
  timeline has `route` / `chosen` and `route` / `moved`, and the Agents page and `ai6ctl agents` list
  each route.
- **A review** is issued no credential, so a route that needs one is not chosen for it, and an
  Operation's own `agents.toml` may not change a reviewer's routes.
- **A clearance raise** is signed for what occupies the codename: with routes, every model they
  name. Changing them ends the raise.

[Moneypenny](assistant.md#routes-and-quota) takes routes too.

### Models an account may use

A route names a model by what its source calls it, so a person needs to know what a source offers.
Each Station asks its handlers, through the optional call `AgentHandler::models`, which models the
account behind an environment may use, with **no model turn**: for each pool account (its credential
leased as a Session's is, no account claimed, the lease revoked as soon as the answer is in) and for
each route no pool serves (its `env` and `secrets`). It looks again every `[accounts] models_every`
seconds (3600; 0 never on its own) and reports the answers at check-in. A look runs on its own
thread, never on the check-in beat, so slow handlers cannot make the Station look dark; it asks
`[accounts] model_probes` handlers at once (4; `null` all at once, 0 none) and waits
`model_probe_timeout` seconds for each (20; `null` as long as it takes). One that takes longer keeps
what the last look found for its account or route, or is unknown and says so. `ai6ctl models` and the
Pools page in Outer Office (`roster.read`) show them by pool and account.

- **Claude Code** answers an `initialize` control request with the models its login can pick. The
  list follows the kind of login in the environment (an API key, a subscription, a gateway), not a
  check of the account's standing, and a gateway's own model names are not in it.
- **Codex** answers `model/list` from the app server: its shipped catalog with no login, and what the
  account is offered with one (not measured here).
- **A handler that cannot list**, the `acp` handler for now, says **unknown** and why. Nothing is
  guessed, and an unknown is never shown as an empty list.
- A discovered model is an offer: it makes no route, and no Agent uses it until a person or an
  approved proposal writes the route. No credential is ever part of an answer.

## Overlays

An overlay is a directory of what an Agent works with: rules, context, skills, knowledge files,
programs and environment. A loadout issues overlays to its Agent, and an Operation adds its own,
since which standards apply belongs to the project. The format is the
[Agent Overlay Spec](https://boriken-dev.github.io/agent-overlay-spec/) (AOS), edition
`0.1.0-draft`: a tree with an optional `overlay.cfg` manifest.

A Station fetches each overlay to a store keyed by its digest, verifies it at every Session start,
and gives the Session a read-only copy outside its working directory, so no commit can hold it.

```toml
# loadouts.toml
[overlay.engineering]
source = "https://github.com/example/agents.git@4b825dc642cb6eb9a060e54bf8d69288fbee4904#overlays/engineering"
pin = "sha256:5d0c..."                     # what `ai6 overlay pin DIR` prints
secrets = ["agents-deploy-key"]            # [secret.<name>] names a private source is fetched with

[overlay.rust]
path = "/srv/overlays/rust"                # a directory on the Station
pin = "sha256:9a41..."

[loadout.007]
overlays = ["engineering"]

# operations.toml
[operation.G]
overlays = ["rust"]
```

- **`[overlay.<name>]`** names exactly one of `source`, in AOS's
  `[git+]<location>[@<ref>][#<path>]` form, or `path`, and the `pin` its files must have. The
  name is the overlay's identity, compared in lower case with `_` and `.` as `-`: `Team_Review`
  and `team-review` are one overlay, and two records naming it are refused. `default`,
  `overlay`, `system`, `user`, `project`, `project-root` and Windows device names are refused.
- **`overlays`** on a loadout and on an Operation name records by identity, each once. A
  loadout's must name records its document defines.
- **Order.** A Session gets the loadout's list, then the Operation's. Each overlay's `requires`
  is resolved first and each overlay is issued once; they then contribute in AOS composition
  order, ascending `(priority, name, identity)`, so the overlay with the higher `priority` wins
  a conflict. An Operation's list only adds: no name takes away what the loadout issues.
- **Refusals.** A name that is no overlay name or that no record defines, a `requires` no record
  offers, and a cycle (`a -> b -> a`) each refuse the Session, naming the overlay: a Session is
  never given part of what it was meant to have.
- **The pin** is the AOS tree digest: one line per file, `path NUL sha256:<hex> LF`, sorted by
  path bytes, hashed with SHA-256, over the bytes as published. `.git`, `node_modules`,
  `__pycache__`, the other excluded names and `.pyc` files are not part of it; a link, a
  special file, a name a platform would alias and two paths equal under ASCII case make the
  tree unreadable. A changed, added or renamed file changes it. A mismatch refuses and nothing
  overrides it: to accept other bytes, change the pin. An overlay with no pin is refused unless
  the `station` record's `[overlays] unpinned = true`, a weakening reported as `unpinned` for
  each such overlay every time it is issued.
- **`OVERLAY.md`.** A later AOS edition reads `OVERLAY.md` as the manifest when there is no
  `overlay.cfg`. This AI6 reads edition `0.1.0-draft` at `061b8f5`: an overlay with `OVERLAY.md` and
  no `overlay.cfg` is refused, naming the file, by `ai6 overlay pin` and at every Session start; it is
  never read as having no manifest.
- **Hooks.** A later AOS edition runs an overlay's hooks (a `[hooks]` table, or a `hook.<ext>`
  script at the root), and a `pre-tool` hook can deny a call. This AI6 runs none, so an overlay that
  asks for one is refused, naming the table or the file, rather than run with weaker guards than its
  author wrote.
- **`ai6 overlay pin DIR`** reads an overlay as a Station does, running nothing: it prints each
  file with its SHA-256 and the `pin`. It exits non-zero, printing no pin, when `overlay.cfg`
  does not read or validate against the files, or a skill fails the `SKILL.md` checks. Problems
  under `about`, which is informational, are printed and do not stop it. `ai6 assistant
  pin-skill` computes the same digest.

### What an overlay holds

Files in conventional places take effect without a manifest, each turned off by a `[defaults]`
key set to `false`:

| Key | Default |
| --- | --- |
| `bin` | `bin/` prepended to `PATH` |
| `lib` | `lib/python/` prepended to `PYTHONPATH`, `lib/node/` to `NODE_PATH`; a bare `lib/` means nothing |
| `env` | the root file `env`, read as dotenv: literal text with `${NAME}` substitution, never run |
| `env_scripts` | one root `env.<ext>`: `.py`, `.js`, `.mjs`, `.cjs`, `.ps1` on Windows, `.bash`, `.sh` elsewhere, the first present whose interpreter is found |
| `setup` | one root `setup.<ext>`, chosen the same way, taken to need the network |
| `deps` | `lib/python/requirements.txt` and `lib/node/package.json` |
| `context` | the root `CONTEXT.md` |
| `skills` | every `skills/<name>/SKILL.md` |
| `rules` | every `rules/<name>.md` directly in `rules/` |

What a manifest declares adds to these, and an entry naming a default's file or directory
replaces that default. Every skill an overlay offers passes the checks Moneypenny's catalog
applies to `SKILL.md`: front matter naming the skill with a description, and no key that grants
tools or injected command.

- **Weakenings.** A default env script or setup that runs, a setup with the network, a
  dependency file installed (`deps-network`) or not locked (`unlocked-dependencies`), a secret
  read, a sensitive variable set and the others AOS lists are computed for each overlay, with the
  overlay and the variable or path, never a value.
- **Secrets** reach an overlay only by reference (`{ secret = "name" }` or
  `{ provider = "env", key = "VAR" }` in `env` or an `aos` env file, never in dotenv), and only
  when granted to that overlay; a placeholder never reads a secret.

### What a Session gets

- **The root.** `<NAME>_OVERLAY_ROOT` for each overlay, its identity in upper case with `-` as `_`,
  and `<NAME>_OVERLAY_STATE`. A root is read-only: `0555` and `0444`, with `0555` on a file under
  `bin/` or opening with `#!`, or on Windows an inherited deny on writing, adding and deleting. A
  process running as the same user can take that off, so what stops it is the check at the next
  start. A limited Windows token is stopped by that deny; an elevated one gets past it for a delete, a
  new directory and whatever asks to back up (Git Bash wrote a new file through one), and root gets past
  file modes. Each is reported as `read-only-not-enforced`.
- **`bin/`** first on `PATH`, `lib/python/` on `PYTHONPATH` and `lib/node/` on `NODE_PATH`, with the
  rest of the overlay's `env` and `env_files`, in the order of application. An Agent with no `PATH`
  keeps where a process given none looks; on a Theater, the login `PATH`. An overlay may not set a
  variable starting `AI6_`: that refuses the Session.
- **The text.** Rules, routing and context in the initial context, each file marked, in composition
  order, the whole no more than `[overlays] context_bytes`: past it the Session is refused, naming the
  setting, rather than its text cut.
- **Skills**, loaded natively where the harness loads them, and otherwise named by a routing line
  (`Skill <name> (overlay <overlay>): ... Read <root>/skills/<name>/SKILL.md`). Two overlays offering a
  skill of one name, without regard to case: the later in composition order is delivered and
  `skill-collision` is reported. `[harness.<id>] skills = false` in the manifest makes the routing
  line the delivery for that harness, `context` adds files for it alone, and no other key of the table
  applies.
- **A `desk` Session** gets the root and the text, and no `bin/`, `lib/` or anything run for it. The
  same goes for a Session held to reading until its plan passes.

| Handler | Harness id | Skills | Checked with |
| --- | --- | --- | --- |
| `claude` | `claude-code` | Native: `--plugin-dir` of a plugin the Station lays out, `Skill` offered | Claude Code 2.1.318 |
| `codex` | `codex` | Routing line: Codex takes no root of skills but its home and the project. At `desk` it has no tool that reads a file, so there it cannot open what the line names | Codex 0.164.0 |
| `acp` | `acp` | Routing line, in the first prompt | Grok 1.0.70 |
| `antigravity` | `antigravity` | Native: a workspace of the Session's own holding the skills; each root may be read | agy 1.4.12 on Linux |

**A resumed Session** is given its overlays again, checked again. If its digests differ from those it
started with (a changed pin), it gets the new ones, and the timeline says so (`overlay`, `changed`).
One taken up as it runs, on its runner, keeps what it has.

**A tree that changed** refuses the start, naming the directory, on the Station as on a Theater, and
the Mission waits with the reason. Nothing fetches over it: remove the directory and the next start
fetches it again. Reading a plain URL as a source is not implemented; name a Git repository or a
directory. A Git source's bytes are its blobs, with no checkout filter. `secrets` on an
`[overlay.<name>]` names the secret a private HTTPS source is fetched with, in the environment of that
one `git`.

### Code in an overlay

Code needs what runs it where the Session runs. Text does not: an overlay with only text goes anywhere.

- **What is looked for.** On a Session that may run commands, a Station looks on the Agent's `PATH`
  (the login `PATH` when it names none) at most once per `[scheduler] check_in`, for each interpreter
  and command the overlay's manifest and files call for: Python (`python3`, or `python`, answering
  `Python 3`) for `lib/python/`, `cmds/` or a requirements file, what a setup or env script names, and
  `npm` for a `package.json`. A Theater lacking one is passed over: the Mission waits, and says what
  is missing and which overlay needs it, until it is there.
- **Dependencies** go into an environment of each overlay's own, keyed by its digest, whatever
  `environment` the overlay chooses, so a pin covers exactly the packages its Sessions get. AOS
  allows a consumer this policy, and it is not a weakening, since it shares less. It is built before
  the harness starts, outside its sandbox, and is read-only after. A file with an option in it, a
  direct URL, or only some requirements hashed is refused. `deps-network` is reported for each
  install, and `unlocked-dependencies` for a file not locked. npm runs no package's install scripts.
- **Setup** runs in its phases: `install` for an overlay with no record on a Theater, `sync` for a
  digest other than the recorded one, nothing for the recorded one. The record is kept per Theater, on
  the Station, and a Theater that lost the environment or state it speaks of starts again from
  `install`. A digest whose preparation failed is not issued, and the Mission fails with the step, its
  exit status and its last line of standard error. A step runs in a process group of its own with its
  timeout (on Windows, its process tree at the timeout), its environment the Agent's path and locale
  variables and its own overlay's, never a credential. A Station cannot deny it the network, so
  `network-not-enforced` is reported for a setup that declares none.
- **The state directory** is per overlay identity and Theater, kept across digests, emptied before
  `install`, and read-only to the Session.
- **Env scripts** run at each start, outside the Session, and what they print joins the environment;
  `env-script` is reported each time.
- **`AGENTS_PYTHON`** names the interpreter found, for an overlay with Python code.
- **`cmds/`** runs under `dotagents`: `[overlays] dotagents_pyz` names the `dotagents.pyz` on the
  Station and `dotagents_pin` its SHA-256. A Session then has `dotagents` on `PATH`, `AGENTS_CMDS_PATH`
  naming each overlay's `cmds/`, and an empty `AGENTS_HOME` of its own, removed with its Mission. An
  overlay with `cmds/` and no such setting is refused, naming it.

## Commands

Run `ai6 serve --help` to configure a Station and `ai6ctl --help` for the operator
commands. The sample Operation and roster are in `examples/config/`.
