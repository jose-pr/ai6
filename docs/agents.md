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
which the Agents view in Moneypenny and `ai6ctl agents` show. Changing `model` and clearing
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
and policy per Operation in Moneypenny. An Operation's local `.ai6/agents.toml` may refine
Agent entries but cannot change role policy or grant permissions.

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
  `AI6_CODEX_TOOL_CHECK` ([below](#codex-tool-check));
- is exposed from the start (`brief:source`), and its usage counts against the Operation.

Answer-only is the `answer_only` field of the handler's start request. Claude Code gets
`--restricted` (2.1.248 or later), `--tools=` (no built-in tool), `--disallowedTools` naming
its shell, file, web, sub-agent and every MCP tool, `--setting-sources user` and
`--strict-mcp-config` with an empty `--mcp-config`. Codex gets its command execution off
(`features.shell_tool = false`), local image attachment off (`tools.view_image = false`), web
search, sub-agents (`features.multi_agent`, `agents.enabled`) and app connectors off, an empty
skills catalog (`skills.max_context_tokens = 1`), every other feature it enables switched
off, the user's MCP servers replaced by none and the working directory's project layer
skipped, and its thread in the `read-only` sandbox with approval policy `never`; it starts
only after the [tool check](#codex-tool-check) finds it offers no tool at all. A Claude Code
too old for one of its flags refuses to start, which ends the review without an answer.

When a resumed Session continues, the Station gives the handler the standing part of its
start-up context again (the Agent's `context`, its role's purpose and policy):
Claude Code does not keep `--append-system-prompt` in the Session it records, so the handler
passes it again; Codex keeps a thread's instructions.

### Codex tool check

A Codex release can offer its model tools no setting this handler passes switches off, and
each release adds some: Codex 0.164 enables code mode (`functions.exec`, a JavaScript runtime
whose nested tools include `apply_patch` and `view_image`), computer and browser use, a
plugin's `mcp__cua_repl` tools, collaboration tools, `request_user_input`, `clock.sleep`,
plugins, hooks and skills by default, and ignores a user's `features.unified_exec = false`.
So before an answer-only or assistant Codex thread starts or resumes, the handler runs the
same Codex with the same settings, environment and working directory:

1. `codex features list`, then switches off every enabled feature except
   `secret_auth_storage`, `enable_request_compression`, `system_proxy_fallback` and
   `unbounded_connection_retries` (`ai6_agent_codex::KEPT_FEATURES`: how it stores its login
   and reaches its service);
2. `codex debug prompt-input` with those settings and the thread's model, which renders what
   the model would be given without asking it anything, and reads the tools offered.

The thread starts only when the tools offered are exactly those of its MCP servers
(`mcp__<server>.<tool>`; none for answer-only) and every server offers at least one. A
passed check is reused for 15 minutes for the same Codex executable, `--version`, settings,
model and servers. With Codex 0.164.0 an answer-only thread passes with no tool at all, while
Moneypenny does not start: that release offers its models MCP tools only inside
`functions.exec`, and with code mode off it offers them none.

| `AI6_CODEX_TOOL_CHECK` in the Agent's environment | When the tools cannot be read, or are not exactly the servers' |
| --- | --- |
| `strict` (the default, also when unset) | The thread does not start, and the error names what was found |
| `warn` | The thread starts; what was found is logged on the Station as a weakening and is the Session's first event (kind `weakened`) |

Any other value refuses to start. The features are switched off in both modes.

The check is only as good as `codex debug prompt-input`. With Codex 0.164.0 its tools match
the request `codex exec` sends for the same model with Codex's default features, but not
with every feature off: there `codex exec` still sends `functions.exec`, `functions.wait`
and `functions.request_user_input`, while `prompt-input` renders none. The app server's own
feature state (`experimentalFeature/list`) matches `codex features list` under the same
settings; whether its threads are sent what `prompt-input` renders is not established.

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

Each managed Session receives a separate loopback MCP endpoint when AI6 is built with
`cables-mcp`. The endpoint returns no tools until AI6 binds the provider-issued Session
ID. Claude and Codex receive bearer credentials through their launch environment, never
in process arguments or persisted configuration. Remote SSH tunneling is not implemented.

Incoming message text is untrusted input. It cannot grant tools, clearance, secrets or
authority. A2A remains an external interoperability boundary; it cannot bypass AI6's
relay policy. See [Agent Cables](architecture.md#agent-cables) for the message and
permission contract.

## Clearance

| Clearance | May |
| --- | --- |
| `desk` | Read, plan and propose; nothing leaves the worktree |
| `field` | Change its worktree and run tests; irreversible actions require [green ink](missions.md#green-ink) |
| `00` | Act unattended within the configured Operation policy (by default it still asks for green ink; `preset = "licensed"` lets it push and merge) |

An Operation refuses an Agent above its clearance or outside its roster. M retains
authorization authority; Agent role policy is guidance only. Q Branch controls which
credentials are issued.

### Track record and promotion

The Station that runs a Session counts its outcome in the Agent's **record**: Missions done,
failed, blown, abandoned and stalled, and the incidents charged to it (quarantines, budget
stops, green ink refused), per Operation and by clearance, under the model the Session was
launched with. A Session lost with its Station or runner counts as `lost`, never as the
Agent's failure; Chief of Staff reviews are not counted. Moneypenny's Agents view and `ai6ctl
agents --operation ID` show an Operation's records (`agent.read`), nothing of other
Operations'. Promotion counts only the Missions of the model that occupies the codename now.

Autonomy is earned per Operation and signed by a person. When an Agent's record in an
Operation meets the next step, M files a [green ink](missions.md#green-ink) request of kind
`clearance` for it, `007:field@G` for example, under Mission `promotion-007`; nothing is
raised until a person with `approval.decide` **and** `agent.configure` approves it. The
approval is spent and becomes a clearance grant in that Operation only, never above the
Operation's own clearance, for the model and configured clearance the Agent has: a new model
in the codename, or another configured clearance, does not inherit it and earns its clearance
afresh. Removing the grant, or revoking its approval, is final. A refused or expired request is asked
again once the record has as many Missions more.

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
lower clearance in an Operation (Moneypenny, `ai6ctl agents lower`), which only ever lowers
it, and may remove the grant to return it to its configured clearance (`ai6ctl agents
restore`). A quarantine takes back a raised grant the moment it is stored. While a person's
lower grant stands, M files no promotion for that Agent there, and a promotion approved
meanwhile does not overwrite it. A grant a Station cannot read counts as `desk`, and shows as
`desk` with a note.

Clearance is enforced at the harness as well as by policy. The Claude handler passes Claude
Code a `PreToolUse` hook (through `--settings`) that runs `ai6 green-ink gate` before each
`Bash` or `PowerShell` call, and the Codex handler answers Codex's command approval requests
the same way: a command that pushes to a protected ref, deletes a ref, merges a pull request
or creates a release without a live approval is refused, and the Agent is told what to ask
for ([What holds a step](missions.md#what-holds-a-step)). A gated Codex thread runs with
`approvalPolicy = "untrusted"`, so it asks before every command that is not a safe read, and
input to a running command is declined. In remote-hands mode the Station checks every
`theater_run` command itself. The hook is the `ai6` program hosting the handler unless the
Agent's roster environment names another in `AI6_GREEN_INK_GATE` (reported as a weakening; a
start or resume request cannot set it), and it is given the Session's Cable URL in its own
arguments. It needs Claude Code 2.1.295 or later, which blocks the call when the hook cannot
run.

## Credentials

Q issues each Session the credentials its Agent's loadout names, as leases, and nothing
else of its own: harness keys or tokens, git and forge tokens. A Station reads loadouts from
`loadouts.toml` in its configuration directory:

```toml
[loadout.007]
secrets = ["anthropic", "forge", "release"]

[secret.anthropic]
env = "ANTHROPIC_API_KEY"                    # default: the name in upper case, - and . as _
                                             # read from secrets/anthropic in this directory

[secret.forge]
env = "GH_TOKEN"
from = { file = "/etc/ai6/secrets/forge" }   # or { env = "VAR" }: weaker, see below

[secret.release]                             # a credential for protected targets
env = "RELEASE_TOKEN"
green_ink = true
kinds = ["release", "push"]                  # approvals that unlock it; default: any
targets = ["tag v*", "origin:refs/tags/*"]   # globs on the approval's target; default: any
```

- When a Session starts or resumes, the Station leases the Agent's secrets from its secret
  backend and puts each value in the harness's environment under `env`, replacing what the
  Mission's earlier Session held. A value is never written to disk: not in the safe house,
  the launch record, the Registry, nor on the Theater. The [detached
  wrapper](theaters.md#the-detached-wrapper) hands the environment over through a FIFO; a
  runner keeps it in memory. A Session refuses to start when its backend refuses an
  ordinary secret.
- The Station's own backend reads each value when it is leased, from a file on the Station
  (`secrets/<name>` in its configuration directory by default; a relative `from = { file }`
  is relative to that directory) or, with `from = { env = "VAR" }`, from the Station's own
  environment. The environment is the weaker choice, logged as a weakening at start: every
  process the Station starts on its own host inherits it, a harness among them, green ink
  or not. `StationConfig::with_secret_backend` puts another backend in its place, such as a
  vault client whose leases expire on their own. Leases last at most an hour and are
  revoked when the Mission ends or its Session stops on that Station; a value already in a
  harness's environment stays there until the harness ends.
- A harness on the Station's own host runs as the Station's user, so it can read what that
  user can, the secret files among them. Keep green ink credentials out of reach of local
  harnesses: give them to Missions on other Theaters, or use a backend the harness cannot
  query.
- A `green_ink` secret is leased only while the Mission holds a live [green
  ink](missions.md#green-ink) approval of one of its `kinds` whose target matches one of its
  `targets`: once per approval, ever (the Registry records the use, so another Station or a
  restart does not lease it again), for no longer than the approval lasts. A green ink secret
  without `targets`, and one served from the Station's own files or environment (whose value a
  revocation cannot take back), is logged as a weakening at start. The Agent asks with `request_approval`, and the
  credential is in its environment from the next Session start or resume after the
  operator signs. When the approval expires or is revoked, the Station revokes the lease at
  its backend within a scheduler pass (`credential` / `revoked` on the timeline); a backend
  that issues credentials of its own stops honouring it, while a value already in the
  harness's environment stays there.
- A Session the Operation's [exposure](medical.md) policy restricts with `secrets` gets no
  new leases.
- Each lease is on the Mission's timeline (`credential` / `leased`), naming the secrets and
  the approval, never a value.

Credentials already on a Theater outside these leases (a git credential helper, an SSH
agent, a CLI login) are reported as weakenings; see [Fitting a Theater
out](theaters.md#fitting-a-theater-out).

### Account pools

Several provider accounts can be shared by every Station, each Session drawing one, so
work spreads over the quota that is left and a limit on one account does not stop the
others. Pools are declared beside the loadouts, in `loadouts.toml`:

```toml
[pool.claude-team]
handler = "claude"              # the AgentHandler whose Sessions draw from it
operations = ["ai6-*"]          # globs on Operation IDs; default: every Operation
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

`env = "VAR"` puts the value (or directory) in another variable; a handler other than
`claude` or `codex` needs it for `config-dir`. Each account's secret is an ordinary loadout
secret: it is leased from the same backend, never written to the Registry, the safe house or
the timeline. A `green_ink` secret cannot back an account: the load fails, since a pool hands
its accounts to every Session that draws them while a green ink credential is only for a step
a person approved.

- **Who may draw from a pool**: its `handler` must be the Agent's, its `operations` must admit
  the Operation, and the Operation's `pools` (default: every pool) must name it. An Agent may
  pin one with `pool = "<id>"`. An Agent whose handler has pools, none of which may be used,
  does not start, and the reason names each pool and why; an Agent whose handler has no pool
  keeps its loadout alone.
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
  reported nothing counts as half used; a window past its reset counts as unused), then the
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
  (`StationConfig::with_account_limit_horizon`), whatever the provider reports. An operator
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

## Commands

Run `ai6 serve --help` to configure a Station and `ai6ctl --help` for the operator
commands. The sample Operation and roster are in `examples/config/`.
