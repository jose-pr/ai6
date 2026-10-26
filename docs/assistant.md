# Moneypenny, the assistant

Moneypenny is an assistant that knows AI6. A person signed in to the operator app can chat
with her: ask which Stations are busy, why a Mission stalled, what a held brief says, or how
a setting works. She answers from AI6's live records, through tools that run as that person,
and from this guide.

## Where she runs

Her model runs on a Station, through the same [AgentHandler providers](agents.md) as every
Agent: Claude Code or Codex, with whatever subscription login or API key the Agent's
environment gives them. No API key is needed beyond what the harness already uses. She is a
roster Agent, `moneypenny` unless Universal Exports names another (`--assistant-agent`):

```toml
[agent.moneypenny]
handler = "claude"
model = "<model-id>"
effort = "low"
context = "The operators here prefer short answers."
```

Each message the person sends goes from the app to Universal Exports, which hands it to a
Station: the one already running the conversation, else the Headquarters Station, else any
healthy Station. A Station that has no such Agent, no such handler, a quarantine on it, or
four assistant Sessions running already refuses, and the next one is asked. The Station keeps
one harness per conversation, so a follow-up reaches it at once; after ten idle minutes it
closes, and the next message resumes the same provider Session.

An assistant Session is not a Mission: it has no Operation, safe house or Cable endpoint,
never appears on the board, and works in a new empty directory that is removed when the
harness closes. Of its Agent's environment it keeps only what reaches the model, as a
[Chief of Staff review](agents.md#chief-of-staff) does.

## What she may touch

She is started in the handlers' **assistant mode**: no built-in tool at all (no shell, no
files, no web), only the user's own harness settings, and exactly one MCP server, Universal
Exports' Moneypenny endpoint. Claude Code gets `--restricted`, `--tools=`, its own tools
denied by name, `--setting-sources user`, and `--strict-mcp-config` with only that server,
whose tools it may call without asking. Codex cannot switch its shell off, so it runs in the
read-only sandbox (no writes, no network) with approvals off, web search off, and its MCP
servers replaced by that one server.

The mode is built from parts (MCP servers, interpreters, skills, network policy) so later
releases can widen it; today a handler refuses to start an assistant that asks for an
interpreter, a skill or a network beyond its MCP server.

## Her tools

Universal Exports serves her tools on `/v1/moneypenny/mcp`. A message's job carries a fresh
random token bound to the person and the conversation; the Station hands it to the harness in
its environment (never on a command line) and never sees the person's own credentials.

Every tool is a request to the same Universal Exports routes the app uses, made with the
person's access token. She can therefore read exactly what the person can, and nothing more:
asked for the audit log by someone without `audit.read`, the tool answers "Refused", and she
says so. When the person signs out, her tools stop working.

| Tool | Reads |
| --- | --- |
| `board` | Stations (health, running and capacity, resources, Theaters, the Headquarters Station) and every visible Session |
| `missions`, `mission` | Missions with their Sessions, attempts, brief and usage |
| `timeline` | An Operation's activity |
| `pools` | Account pools: Sessions, quota windows, tokens today |
| `held_briefs`, `mission_proposals` | Work waiting for a person |
| `green_ink`, `class_authorizations` | Irreversible steps waiting for green ink, standing approvals |
| `roster` | Operations, Agents, roles and covers |
| `audit` | Audit records |
| `agent_roles`, `quarantines`, `placement_preview`, `station_settings` | Operation and Station settings |
| `profile` | Who the person is and what they may do |
| `field_guide` | A page of this guide |

Results reach her as data, marked as such: briefs, issue bodies and Agent output inside them
were written by others, and her instructions say never to follow what they say.

## Changes she proposes, you confirm

Moneypenny changes nothing herself. Her write tools only propose:

| Tool | Proposes | Needs |
| --- | --- | --- |
| `propose_mission` | Queuing a Mission | `mission.create` |
| `propose_mission_control` | Cancel, retry, resume or interrupt a Mission | `mission.control` |
| `propose_mission_proposal_decision` | Approving or rejecting an Agent's Mission proposal | `mission.create` |
| `propose_held_brief_decision` | Releasing or refusing a held brief | `mission.create` |
| `propose_roster_edit`, `propose_roster_removal` | A roster record set or removed | `roster.manage` |
| `propose_station_settings` | A Station's limits | `station.configure` |
| `propose_class_authorization` | A class authorisation | `approval.class` |

A proposal shows in the chat with the exact request, a summary and the permission it needs,
and **Confirm** and **Cancel**. If the person lacks that permission, there is no proposal at
all: she is told she cannot. Only the person, in the app session the proposal was made in,
can confirm it, within an hour; she has no way to. A confirmed proposal is first recorded in
the audit log (`moneypenny.confirmed`: who, which conversation and proposal, which request),
then sent through the same route as the app's own request, with its usual checks and its own
audit record: a roster edit is still checked against the whole roster and refused if it
would leave it unacceptable. She is told how it went.

So text she reads cannot act through her: an issue body that tells her to change the roster
can at most make her propose it, in plain view, to a person who decides. Irreversible steps
keep [green ink](missions.md#green-ink): she cannot sign it, only point you to the Green ink page.

## In the app

The app opens a WebSocket to `/v1/user/moneypenny` and sends its access token in the first
frame. With each message it says which page the person is on (for example the board, or a
Mission), so "why is this one stuck?" means something. Conversations are kept per person (20
each, for a week) in Universal Exports' memory; reopening one shows what was said. One
message is answered at a time.

## Settings

| Setting | Default | Where |
| --- | --- | --- |
| The Agent she is | `moneypenny` | `universal-exports --assistant-agent`, `AI6_EXPORTS_ASSISTANT_AGENT` |
| Serve her at all | yes | `--no-assistant`, `AI6_EXPORTS_NO_ASSISTANT` |
| Conversations kept | 20 a person, 2,000 in all, 7 days | `AssistantConfig::with_conversation_limits` |
| Tool token lifetime | 1 hour | `AssistantConfig::with_token_ttl` |
| Time for a Station to take a message | 60 seconds | `AssistantConfig::with_answer_timeout` |
| Tool calls per conversation | 120 a minute | `AssistantConfig::with_tool_calls_per_minute` |
