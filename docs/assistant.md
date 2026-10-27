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
closes, and the next message resumes the same provider Session. A harness that exits or
fails is closed at once and the conversation shows why, with the harness's exit status and
its last words; so is one that writes nothing for ten minutes while answering. A Session that
cannot be resumed is noted in the Station's log, and the conversation starts afresh with a
recap.

An assistant Session is not a Mission: it has no Operation or Cable endpoint, and never
appears on the board. It works in a directory of its own, the same one each time its
conversation's harness starts on that Station, because Claude Code resumes a Session only
from the directory that recorded it; the directory is removed when the harness closes. With
[personal repositories](#her-workspace-and-memory) the directory is her workspace. A
harness that starts afresh is given her instructions and a recap of the conversation; a
resumed one only her instructions, which Claude Code uses once the prompt it recorded with
the Session is gone (after compaction). Of its Agent's environment it keeps only what reaches the model, as a
[Chief of Staff review](agents.md#chief-of-staff) does.

## What she may touch

She is started in the handlers' **assistant mode**: no built-in tool at all (no shell, no
files, no web), only the user's own harness settings, and exactly one MCP server, Universal
Exports' Moneypenny endpoint. Claude Code gets `--restricted`, `--tools=`, its own tools
denied by name, `--setting-sources user`, and `--strict-mcp-config` with only that server,
whose tools it may call without asking, and runs with `CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1`:
Claude Code otherwise attaches its server-side advisor tool, which none of those flags
removes. Codex gets its command execution switched off
(`features.shell_tool = false`), web search, sub-agents, app connectors and questions to the
user off, an empty skills catalog, every other feature it enables switched off but those
that only store its login and reach its service and the one its tool check reads through,
that one server added and each of the user's own MCP servers switched off by name, and, as a
further line, the read-only sandbox (no writes, no network) with approvals off. Then the
[tool check](agents.md#codex-tool-check) renders what Codex would offer the model and starts
her only if that is exactly her server's tools. Codex 0.164 gives every model its code-mode
JavaScript tools (`functions.exec`, `functions.wait`) and offers MCP tools only through
them, which this check refuses, so with that release she does not start on Codex: the safe
outcome, since `exec` reaches nested tools such as `apply_patch`. Run her on Claude Code.
Either harness that calls a tool outside her server anyway is ended at once ([restricted
Sessions](agents.md#chief-of-staff)).

The mode is built from parts (MCP servers, interpreters, skills, network policy, the
directories she may write, a context file) so later releases can widen it; today a handler
refuses to start an assistant that asks for an interpreter, a skill or a network beyond its
MCP server. With a [workspace](#her-workspace-and-memory), Claude Code also gets its file
tools (`Read`, `Write`, `Edit`, `Glob`, `Grep`) under `--permission-mode dontAsk`: she may
read anything in her workspace (`--restricted` keeps the file tools inside it) and write
only under `personal/` and `scratch/` (`Edit(/personal/**)`, `Edit(/scratch/**)`); every
other write, including to `.git` and `.claude`, is denied without a prompt. Her standing
context goes in with `--append-system-prompt-file`, from a file beside, not inside, her
workspace. Every assistant Session runs with `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, so Claude
Code keeps no memory of its own beside hers. Codex refuses a workspace for now.

A Station running elevated (as root, or on Windows with an administrator's elevated token)
refuses every assistant message unless the roster's `[assistant]` sets `allow_elevated =
true`. That setting is a weakening: each harness started under it logs `ai6: weakening:
assistant.allow_elevated = true: …`, and the Station reports it at check-in
(`assistant_weakened`).

## Her tools

Universal Exports serves her tools on `/v1/moneypenny/mcp`. A message's job carries a random
token bound to the person, the conversation and the Station that runs it; the Station hands
it to the harness in its environment (never on a command line) and never sees the person's
own credentials. Later messages to the same Station carry the same token while it lasts, so a
running harness keeps working; when the conversation moves to another Station, the old
Station's token stops working, and a Station whose job moved on closes the harness it
started.

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
| `propose_shared_change` | One file of a group's shared repository ([workspace](#her-workspace-and-memory)) | membership of the group |

A proposal shows in the chat with the exact request, a summary and the permission it needs,
and **Confirm** and **Cancel**. If the person lacks that permission, there is no proposal at
all: she is told she cannot. A proposal whose text holds a character that renders as nothing
(a direction control, a zero-width or tag character) is refused, and the app shows any such
character in a request as `<U+XXXX>`, so what is confirmed is what is seen. Only the person, in the app session the proposal was made in,
can confirm it, within an hour; she has no way to. A confirmed proposal is first recorded in
the audit log (`moneypenny.confirmed`: who, which conversation and proposal, which request),
then sent through the same route as the app's own request, with its usual checks and its own
audit record: a roster edit is still checked against the whole roster and refused if it
would leave it unacceptable. She is told how it went.

So text she reads cannot act through her: an issue body that tells her to change the roster
can at most make her propose it, in plain view, to a person who decides. Irreversible steps
keep [green ink](missions.md#green-ink): she cannot sign it, only point you to the Green ink page.

## Her workspace and memory

When the roster gives people personal repositories, Moneypenny keeps her work and her
memory of each person in that person's own repository, so both outlive the conversation and
the Station. In the global `agents.toml` (the roster a configuration directory holds; the SQL
roster has no `[assistant]` table yet):

```toml
[assistant]
personal_repo = "/srv/moneypenny/{person}.git"   # or {person_digest}
allow_elevated = false

[assistant.shared.ops]          # a directory group
repo = "ssh://git@git.example/ops/handbook.git"
write = false

[assistant.org]                 # everyone
repo = "https://git.example/org/handbook.git"
```

`{person}` is the person's identifier when it is 1 to 64 letters, digits, `.`, `_`, `@` and
`-` not starting with `.`; any other identifier is refused, and `{person_digest}` (16 hex
digits of its SHA-256) always works. A local path or `file://` repository is created, empty
and bare, the first time it is used; `ssh://`, `https://` and `user@host:` repositories are
reached with the Station user's Git credentials and must exist. Universal Exports reads the
table from the roster it serves (`--roster`) and sends each message with the person's
workspace; without `personal_repo` nothing changes from a plain conversation.

Her workspace on the Station holds:

| Path | What |
| --- | --- |
| `personal/` | A clone of the person's repository on `main`: her working directory for them |
| `personal/memory/` | Her memory of the person, indexed by `memory/MEMORY.md` (written on first use) |
| `shared/<group>/`, `shared/org/` | The repositories of the person's directory groups and the organisation's, cloned afresh at each start, read-only for her |
| `scratch/` | Anything temporary, gone with the workspace |
| `CLAUDE.md`, `AGENTS.md` | Where things are and how her memory is kept; `CLAUDE.md` imports `@personal/memory/MEMORY.md`, `AGENTS.md` holds it |

Claude Code started with `--restricted` does not load a working directory's `CLAUDE.md`, so
the same text, `MEMORY.md` included (at most 32 KiB), reaches her system prompt through a
context file beside the workspace.

After each answer, and when her harness closes, the Station saves her work: whatever she
changed in `personal/` outside `memory/` is committed to `main`, authored by the person, with
the message `moneypenny: update <paths>` and the trailer `Via: Moneypenny (conversation
<id>)`. If `main` moved on meanwhile her commit is replayed on it once; if that conflicts, it
is pushed to `moneypenny/<conversation digest>` instead and both of you are told.

Her memory is never written unseen. At the start of each answer the Station copies
`memory/`; at the end it puts that copy back and sends Universal Exports what she changed
there, with a diff. In a conversation that has read nothing other people wrote, it is
committed at once and the conversation shows what she kept. Once one of her tools has
returned text others wrote (`mission`, `missions`, `held_briefs`, `mission_proposals`,
`timeline` or `audit`), the change becomes a **proposal** with the diff: **Confirm** commits
it (audited `moneypenny.confirmed`), **Cancel** discards it. Either way she is told.

A change to a shared repository is a proposal too, through `propose_shared_change` (the
group, a file path, its new text and a commit subject), and only for a member of that group;
anyone else's request is refused. Confirmed, it is committed under the person's name with the
same trailer. With `write = true` for that group (or for `org`) it is committed at once and
audited `moneypenny.shared.write`; that setting is a weakening, as are `allow_elevated` and
any `write = true`, and each is logged when a Station starts or its roster changes.

## Views she shows

When a table or a chart says it better than prose, she shows a **view** with her `show` tool.
A view is data in a strict, versioned schema that the app renders with its own widgets:

| Block | Shows |
| --- | --- |
| `text` | Markdown: headings, lists, bold, italic, code, and links into the app only |
| `table` | Columns and rows of plain values |
| `key_values` | A list of names and values |
| `chart` | A bar or line chart of up to eight series |
| `card` | A Mission, Station or Theater, opening its page |
| `timeline` | Dated events |
| `diff` | Two texts, line by line |
| `form` | A draft of one of her `propose_*` tools for the person to fill in |

Nothing in a view runs: there is no HTML, no script and no outside link. Universal Exports
refuses a view that is not exactly the schema, within its bounds, and tells her why, so the
app never receives it; the app checks again and shows a refused view as "View refused" with
the reason, never as a guess. Submitting a form records a proposal of its tool, which the
person still confirms.

## In the app

**Ask Moneypenny** (top right on every page) opens the chat. The app opens a WebSocket to
`/v1/user/moneypenny` and sends its access token in the first frame. With each message it
says which page the person is on (for example the Stations page, or the Session open beside
the board), so "why is this one stuck?" means something. Her answers, the tools she used,
her views and her proposals (with **Confirm** and **Cancel**) appear in the panel.
Conversations are kept per person (20 each, for a week) in Universal Exports' memory;
reopening one shows what was said. One message is answered at a time.

While she answers, **Stop** takes the place of Send. It sends `{"type":"interrupt"}` on the
socket, which only ever reaches the person's own conversation. A message no Station has
taken up yet is simply withdrawn; otherwise Universal Exports asks the Station running her
harness to interrupt it, and that Station's handler stops the turn the way the harness
expects (a stream-json interrupt request for Claude Code, `turn/interrupt` for Codex). The
conversation then shows "You stopped Moneypenny's answer.", her harness stays up for the next
message, and she is told with that message that she was stopped. Stop with nothing being
answered does nothing.

## Settings

| Setting | Default | Where |
| --- | --- | --- |
| The Agent she is | `moneypenny` | `universal-exports --assistant-agent`, `AI6_EXPORTS_ASSISTANT_AGENT` |
| Serve her at all | yes | `--no-assistant`, `AI6_EXPORTS_NO_ASSISTANT` |
| Conversations kept | 20 a person, 2,000 in all, 7 days | `AssistantConfig::with_conversation_limits` |
| Tool token lifetime | 1 hour | `AssistantConfig::with_token_ttl` |
| Time for a Station to take a message | 60 seconds | `AssistantConfig::with_answer_timeout` |
| Tool calls per conversation | 120 a minute | `AssistantConfig::with_tool_calls_per_minute` |
| Conversations of one person with a running harness or an answer pending | 2 (the least recently used idle harness is closed to make room) | `AssistantConfig::with_live_conversations_per_person` |
| How long an answer may go without word from its Station | 10 minutes, then it is given up | `AssistantConfig::with_answer_deadline` |
| Memory kept | 1 MiB per conversation, 4 MiB per person (oldest views go first); the app is sent the newest 100 entries | `AssistantConfig::with_memory_limits` |
| Personal repositories | none: no workspace | `[assistant] personal_repo` in the global `agents.toml` |
| Shared repositories | none | `[assistant.shared.<group>]`, `[assistant.org]` (`write = false`) |
| Run on an elevated Station | no | `[assistant] allow_elevated` |
