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
provider-issued Session ID; AI6 retains that ID when resuming the same Session.

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

`examples/config/agents.toml` defines these roles with `007`, its backup `008` and a
`desk`-clearance `Tanner` for reports. Authorized operators may override a role's purpose
and policy per Operation in Moneypenny. An Operation's local `.ai6/agents.toml` may refine
Agent entries but cannot change role policy or grant permissions.

## 00 coordination

The coordinating 00 Agent is an Operation-assigned Agent identity with `00` clearance, not a
separate role or Session type. A managed Session with that clearance can use `propose_mission`
to submit a child-Mission brief to the Operation's pending proposal inbox. AI6 binds the
proposer, parent Mission, provider Session and Operation; proposals stay outside runnable I
intake until M explicitly authorizes and promotes them. Universal Exports' authenticated
`mission.create` review endpoints let an operator inspect, approve and queue, or reject a
proposal; approval follows M's normal assignment and scheduling path. Proposal submission
does not start a child Session. Clearance does not grant authorization, override Operation
cover or Agent eligibility, or bypass Q leases,
Station policy, or the Cable relay. Q Branch, the Registry and Heads of Station retain their own capability
boundaries. The Chief of Staff is a separate planned M adviser for vetting briefs and budgets.

## Cables

AI6's **Cable** relay supports Agent unicast and permission-scoped Station, Theater,
Global and named-channel broadcasts. Its MCP tools provide peer discovery, sending,
inbox reads, acknowledgements, replies and explicitly authorized held-Cable release. An
authorized 00 Agent also has the `propose_mission` tool for submitting work to M; this does
not make the proposal a Cable or scheduler input. Every Agent has `request_approval`, which
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
  ink](missions.md#green-ink) approval of one of its `kinds`: once per approval, for no
  longer than the approval lasts. The Agent asks with `request_approval`, and the
  credential is in its environment from the next Session start or resume after the
  operator signs.
- A Session the Operation's [exposure](medical.md) policy restricts with `secrets` gets no
  new leases.
- Each lease is on the Mission's timeline (`credential` / `leased`), naming the secrets and
  the approval, never a value.

Credentials already on a Theater outside these leases (a git credential helper, an SSH
agent, a CLI login) are reported as weakenings; see [Fitting a Theater
out](theaters.md#fitting-a-theater-out).

## Commands

Run `ai6 serve --help` to configure a Station and `ai6ctl --help` for the operator
commands. The sample Operation and roster are in `examples/config/`.
