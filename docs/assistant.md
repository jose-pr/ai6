# Moneypenny, the assistant

Moneypenny is an assistant that knows AI6. A person signed in to Outer Office, the operator app, can chat
with her: ask which Stations are busy, why a Mission stalled, what a held brief says, or how
a setting works. She answers from AI6's live records, through tools that run as that person,
and from this guide.

## Where she runs

Her model runs on a Station, through the same [AgentHandler providers](agents.md) as every
Agent: Claude Code or Codex, with whatever subscription login or API key the Agent's
environment gives them. No API key is needed beyond what the harness already uses. She is a
roster Agent, `moneypenny` unless Universal Exports names another (the `unex` record's
`assistant_agent`):

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
four assistant Sessions running or starting already (`[assistant.desk] live`) refuses, and the
next one is asked. A
Station takes each conversation's messages in order and different conversations' at the same
time, so one whose repositories are slow to clone holds up no one else. The Station keeps
one harness per conversation, so a follow-up reaches it at once; after ten idle minutes
(`[assistant.desk] idle`) it
closes, and the next message resumes the same provider Session. A harness that exits or
fails is closed at once and the conversation shows why, with the harness's exit status and
its last words; so is one that writes nothing for ten minutes while answering. A Session that
cannot be resumed is noted in the Station's log, and the conversation starts afresh with a
recap.

An assistant Session is not a Mission: it has no Operation, and never appears on the board;
it reaches Agents only through [Cables](#cables), for the person. It works in a directory of
its own, the same one each time its conversation's harness starts on that Station, because
Claude Code resumes a Session only from the directory that recorded it; the directory is
removed when the harness closes. With [personal repositories](#her-workspace-and-memory) the
directory is her workspace. A
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
user off, an empty skills catalog, no project instructions file (`project_doc_max_bytes =
0`), every other feature it enables switched off but those that only store its login and
reach its service and the one its tool check reads through, that one server added with its
tools approved without asking (`default_tools_approval_mode = "approve"`: Codex otherwise
fails every MCP call it would ask about) and each of the user's own MCP servers switched off
by name, and, as a further line, the read-only sandbox (no writes, no network) with approvals
off. Then the [tool check](agents.md#codex-tool-check) renders what Codex would offer the
model and starts her only if that is exactly her server's tools. Codex 0.164 gives every
model its code-mode JavaScript tools (`functions.exec`, `functions.wait`) and offers MCP
tools only through them, which this check refuses, so with that release she starts on Codex
only [with scripts](#her-scripts-on-codex), where those two tools are admitted inside Codex's
sandbox as a reported weakening. Either harness that calls a tool outside what her mode
allows is ended at once ([restricted Sessions](agents.md#chief-of-staff)).

The mode is built from parts (MCP servers, interpreters, skills, network policy, the
directories she may write, a context file), and Claude Code gets exactly the built-in tools
those parts call for: [scripts and the web](#her-scripts-and-the-web) as the roster allows,
and [skills](#her-skills) from the roster's catalog. Codex gets the tools of
[its sandbox](#her-scripts-on-codex) with interpreters and none without, and refuses skills. With a
[workspace](#her-workspace-and-memory), Claude Code also gets its file
tools (`Read`, `Write`, `Edit`, `Glob`, `Grep`) under `--permission-mode dontAsk`: she may
read anything in her workspace (`--restricted` keeps the file tools inside it) and write
only under `personal/` and `scratch/` (`Edit(/personal/**)`, `Edit(/scratch/**)`); every
other write is denied without a prompt, and so is any path with a name starting with `.`
(`.git`, `.gitattributes`, `.claude`), case and Windows short-name variants included. Her
repositories' git directories are not in her workspace at all: they sit beside it, and every
Git command the Station runs on them names them outright, with hooks, filesystem monitors
and filters (Git LFS among them) switched off, so nothing she writes can make the Station run
a program. Links are
off too: a link committed to one of her repositories is checked out as a plain file holding
its target, and the Station never reads, writes or removes anything in her workspace through
a symbolic link, a junction or another reparse point. Her
standing context goes in with `--append-system-prompt-file`, from a file beside, not inside,
her workspace. Every assistant Session runs with `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, so Claude
Code keeps no memory of its own beside hers. Codex gets the same standing context as
developer instructions, read from that file; it writes in her workspace only through her
[scripts](#her-scripts-on-codex), so without them she is told she has no file tools.

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
| `roster` | Operations, Agents, roles, covers and her own `[assistant]` settings |
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

A proposal shows in the chat with the exact request, a summary and the permission it needs
("Proposal · needs `mission.control` on `operation:G`"), what she had read since your last
message ("Based on: missions (G), mission (G/m-12)", the tools with their identifier
arguments, never free text), the page it changes as a chip ("Affects: Roster › agent 009"
that only opens the page, and "Open" once it is applied), how it stands (Waiting for you,
Applying, Applied, Refused with the reason, Cancelled, Expired) and **Confirm** and
**Cancel**. **Confirm** answers once; a proposal still applying after thirty seconds says
"Outcome unknown, check the page" instead of offering another. A proposal made in an exposed
conversation also carries the strip naming what exposed it (`tool:missions`, or the source a
Station reported). Every kind of entry has an icon and a label for screen readers: her answer,
a proposal, a view ("Made by Moneypenny"), a link to a page in Outer Office and a page she
published. If the person lacks that permission, there is no proposal at
all: she is told she cannot. A proposal whose text holds a character that renders as nothing
(a direction control, a zero-width or tag character) is refused, and the app shows any such
character in a request as `<U+XXXX>`, so what is confirmed is what is seen. Only the person, in the app session the proposal was made in,
can confirm it, within an hour; she has no way to. A confirmed proposal is first recorded in
the audit log (`moneypenny.confirmed`: who, which conversation and proposal, which request),
then sent through the same route as the app's own request, with its usual checks and its own
audit record: a roster edit is still checked against the whole roster and refused if it
would leave it unacceptable. She is told how it went. She may propose her own settings too
(`propose_roster_edit` with kind `assistant`, key `assistant`): the proposal is checked as
settings before it is shown, and its summary ends with `Weakens: …` naming what it loosens or
redirects against the settings in force: a weakening not already set, an interpreter, host or
looser network policy, a personal, shared or organisation repository that changes, a skill
added or with another path or pin, and a group or person table removed or changed. So that is
in view before anyone confirms it; when the settings in force cannot be read, nothing is
proposed.

So text she reads cannot act through her: an issue body that tells her to change the roster
can at most make her propose it, in plain view, to a person who decides. Irreversible steps
keep [green ink](missions.md#green-ink): she cannot sign it, only point you to the Green ink page.

## Her workspace and memory

When the roster gives people personal repositories, Moneypenny keeps her work and her
memory of each person in that person's own repository, so both outlive the conversation and
the Station. In the global `agents.toml` of a configuration directory:

```toml
[assistant]
personal_repo = "/srv/moneypenny/{person_digest}.git"   # or {person}
allow_elevated = false

[assistant.shared.ops]          # a directory group
repo = "ssh://git@git.example/ops/handbook.git"
write = false

[assistant.org]                 # everyone
repo = "https://git.example/org/handbook.git"
```

`{person_digest}` (16 hex digits of the SHA-256 of the person's identifier) works for every
identifier and is the one to use. `{person}` is the identifier itself, and only when it is 1
to 64 lowercase letters, digits, `.`, `_`, `@` and `-` not starting with `.`: any other
identifier is refused, so two people whose identifiers differ only in case never share a
repository on a case-insensitive filesystem. The template holds exactly one of the two,
inside the repository's path (not its host or user), and is refused when some identifier
would fill it in as a shared or the organisation's repository. A local path or `file://`
repository is created, empty and bare, the first time a conversation or a change uses it, readable by the Station's user only on
Unix (`0700` directories, `0600` files; on Windows it takes its parent directory's access);
`ssh://`, `https://` and `user@host:` repositories are reached with the Station user's Git
credentials and must exist. Universal Exports reads the
table from the roster it serves (its configuration provider) and sends each message with the person's
workspace; without `personal_repo` nothing changes from a plain conversation. A Station takes
no repository on Universal Exports' word: it fills the person's in from its own roster's
`[assistant]` and refuses a message or commit naming any repository that roster does not give
that person: a group's shared repository only for a member, by the groups each message and
each shared commit carries. So the gateway and every Station must read the same roster.

A SQL roster (`sqlite:` or `postgres://`, shared by the Stations of an HA deployment) keeps the
same keys as one record of kind `assistant`, keyed `settings`, in the table
`ai6_config_assistant`: the row `settings` holding `{"personal_repo":
"/srv/moneypenny/{person_digest}.git"}` is the first line above. An administrator with
`roster.manage` sets it with `PUT /v1/user/roster/assistant/settings` (the definition as a
JSON object, replacing the whole record) or removes it to go back to the defaults, through the
API or on the app's Roster page under **Moneypenny's settings**. It is
checked as the table is: an unknown key, a repository AI6 refuses or a record under another key
is refused with `422` and changes nothing. The answer lists what the record loosens or
redirects against the settings it replaced (`weakened`, as her proposals word it), and every
Station logs and reports each setting looser than its default as it takes the change up.

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
changed in `personal/` outside `memory/` is committed to `main`, authored by the person (a path
with a name starting with `.`, such as `.gitattributes`, never is, nor is anything in
`memory/`; when her memory cannot be put back as it was, nothing is saved), with
the message `moneypenny: update <paths>` and the trailer `Via: Moneypenny (conversation
<id>)`. If `main` moved on meanwhile her commit is replayed on it once; if that conflicts, it
is pushed to `moneypenny/<conversation digest>` instead and both of you are told.

Her memory is never written unseen. At the start of each answer the Station copies
`memory/`; at the end it puts that copy back and sends Universal Exports what she changed
there, with a diff. In a conversation that has read nothing other people wrote, it is
committed at once and the conversation shows what she kept. Once the conversation is
**exposed** to such text, the change becomes a **proposal** with the diff: **Confirm** commits
exactly what was shown (audited `moneypenny.confirmed`), **Cancel** discards it. Either way
she is told. A memory change records what each of its files held when she made it, and its
commit is refused when one of them has changed on `main` since (another conversation's
memory, or the person's own edit), so nothing written meanwhile is lost: she proposes it
again from what the file holds now. A memory change the Station cannot get to Universal
Exports (it is down, or no longer waits for that answer) is kept on that Station and sent again
with the conversation's next message there; no queue drops a memory change, a held Cable or
a Station's answer to make room (64 kept changes at most per Station, the oldest logged and
dropped past that).

A conversation is exposed, for good, when:

- any of her read tools answers other than `field_guide`, `profile` and `station_settings`
  (every other one can quote an Agent, a brief or another person), with a result or with a
  refusal that quotes what AI6 answered;
- she reads a [Cable](#cables) body;
- she reads one of the files she wrote in an exposed conversation. The Station keeps their
  paths in `.moneypenny/outside-text/` in the personal repository, a hidden directory she
  cannot write with one entry per file (so two conversations recording at once keep both),
  and counts as reading one a `Read` or `Edit` of it, a `Grep` or `Glob` whose search
  covers it, a file tool call it cannot read the input of, and a `memory/MEMORY.md` that
  imports it (`@path`); a script she runs counts too, since it may read any of them. Reading
  other files does not expose the conversation. Removing those files, or the record, in the
  repository clears it;
- she fetches a page or searches the web (`WebFetch`, `WebSearch`), or runs a script while her
  scripts may reach a host (a network policy that reaches one, or [unsandboxed
  scripts](#her-scripts-and-the-web)).

The repositories in `shared/` are trusted text: the group or the organisation chose what is
in them, so reading them exposes nothing. Universal Exports keeps the one record of exposure;
the Station reports what only it sees, and the conversation shows a notice the first time.

A change to a shared repository is a proposal too, through `propose_shared_change` (the
group, a file path, its new text and a commit subject), and only for a member of that group;
anyone else's request is refused. Confirmed, it is committed under the person's name with the
same trailer. With `write = true` for that group (or for `org`) it is committed at once and
audited `moneypenny.shared.write`; that setting is a weakening, as are `allow_elevated` and
any `write = true`, and each is logged when a Station starts or its roster changes.

The change is made from what she read: as the Station sees her call, it reports what the file
held in her `shared/<group>/` checkout (its SHA-256, or that there was no such file), and
Universal Exports attaches that to the change. When the repository's `main` has changed the
file since, the commit is refused and nothing is written, so a group member's edit made
meanwhile is never replaced; she is told to make the change again from what the file holds
now. The Station sees the call in Claude Code's stream (the `tool_use` block) and in Codex's
(the `mcpToolCall` item it announces before calling her server). A call the Station reports
nothing for within five seconds (her checkout of that repository could not be opened) is
refused.

## Your memory in the app

What she remembers about you is yours to read and correct. In the app, **Ask Moneypenny**'s
menu opens **Her memory of you**: the files of `memory/` in your personal repository, each to
open, edit and save, and to forget (after you confirm). A new file is made there too. The page
talks to Universal Exports, which has no checkout of any repository: it asks the Headquarters
Station (else the first healthy one) to do it in a shallow clone of your repository of its
own, and answers within 30 seconds or says that the Station did not. A Station gives the clone
up at that deadline, refuses a request that reaches it later, and refuses one more than it
makes at once (4 by default) rather than queue it. Universal Exports keeps at most 2 of your
requests and 16 of everyone's waiting, and answers `429` past that. Reading or listing makes
no repository: a local one is created only by your first change.

| Route | Does |
| --- | --- |
| `GET /v1/user/assistant/memory` | Lists the files, their sizes and the commit `main` is at |
| `GET /v1/user/assistant/memory/{path}` | Reads one file, with a digest of its text |
| `PUT /v1/user/assistant/memory/{path}` | Writes one file: `{"content", "message"?, "base"?}` |
| `DELETE /v1/user/assistant/memory/{path}` | Forgets one file |

`{path}` is relative to `memory/`: up to seven names of letters, digits, `.`, `-` and `_`,
none starting with `.`; a file holds at most 64 KiB of text. Each save and each "forget" is
a commit to `main` authored by you, its message yours or `Edit memory/<path>` /
`Forget memory/<path>`, followed by `Via: Universal Exports`; history keeps what was there
before. The app saves with the digest of the text it opened as `base`, so a file she changed
meanwhile is refused (`409`) rather than overwritten: open it again and make the change from
what it holds now. Likewise her own next change to a file you edited is refused, and she makes
it again from your text. A conversation already running keeps the memory it read until her
harness starts again.

Your memory is yours alone. The deployment's administrators can read it, never change it: a
person holding `assistant.memory.admin` on `config:assistant` ([access
policy](access.md#the-access-policy)) adds `?person=<id>` to the two `GET` routes, and each
such read is audited `assistant.memory.read` before it is made. Nobody, administrator or not,
writes or forgets another person's memory. Your own changes are audited too
(`assistant.memory.write`, `assistant.memory.forget`), and one that cannot be audited is not
made.

## Her scripts and the web

Moneypenny can run small scripts to compute or draft something, and fetch pages from the hosts
the deployment lists, through Claude Code's own
[sandbox](https://code.claude.com/docs/en/sandboxing). The roster's `[assistant]` decides
what she may run and reach:

```toml
[assistant]
interpreters = ["python3"]        # the default; [] runs no script
network = "limited"               # the default; or "denied", or "open"
allowed_hosts = ["docs.python.org", "*.pypi.org"]   # the default is none
denied_networks = []              # the default: nothing denied (see below)
unsandboxed_scripts = false       # the default

[assistant.group.contractors]     # a directory group
network = "denied"
denied_networks = ["private"]     # added to the deployment's, never taken from it

[assistant.person."eve@sis"]      # one person
interpreters = []
```

A `[assistant.group.<group>]` or `[assistant.person.<id>]` table takes the same keys and may
only narrow them: fewer interpreters, a stricter network (`denied` before `limited` before
`open`), fewer hosts, more `denied_networks`, `unsandboxed_scripts = false`. A table that
would widen one is refused when the roster loads, naming the key (`assistant.group.contractors.network: "open" is looser
than assistant.network`). Universal Exports works out what each message may use from the
person's groups and their own table, and every Station caps it by its own roster again, so
nothing widens on the way.

The sandbox, not the interpreter list, is the boundary: a script runs as the interpreter named
(`python3 personal/calc.py`), but an interpreter can run any program the sandbox allows (a
Python script may call `os.system`, and Claude Code's read-only built-ins run), so what keeps
a script in bounds is where the sandbox lets it read, write and connect, not which tool it is:

- They write in her working directory, `personal/` and `scratch/` among it, but not in
  `shared/`, `CLAUDE.md`, `AGENTS.md`, or a `.git` directory under the working directory.
  What they leave in `personal/` is saved after the answer like anything else she writes there.
- They read nothing outside her working directory but what an interpreter needs: the system's
  programs and libraries (`/usr`, `/bin`, `/lib`, `/lib64` on Linux; `/usr`, `/bin`,
  `/System` and the command line tools on macOS), the install directory of each interpreter as
  found on the harness's `PATH` (`/opt/python3.12` for `/opt/python3.12/bin/python3`), only the
  files under `/etc` that a program, its resolver and TLS read (`ld.so.*`, `ssl`, `pki`,
  `localtime`, `hosts`, `resolv.conf`, `nsswitch.conf`, `passwd`, `group` and the like) and the
  device files `null`, `zero`, `random`, `urandom` and `tty`. Not the rest of `/etc` (so not
  `/etc/ai6`), `/opt`, `/var` or `/dev`, not the Station user's home, not her own configuration
  directory or another conversation's, not `/proc`. Reads are denied everywhere and re-opened
  only for those, and the Station's state, identity and secrets directories, AI6's platform
  state directory, `/usr/local/etc`, `/usr/etc` and the Station's own program are denied
  again inside them. Keep the Station's configuration and
  secrets out of the re-opened paths: an interpreter's install directory is readable whole.
- They see none of the harness's environment variables but `PATH`, `HOME`, the temporary
  directory, locale and terminal ones: not the model's key or login, her tool tokens, a proxy
  URL's basic-auth. A variable whose name is not letters, digits and `_` cannot be hidden, so
  her harness refuses to start with one in its environment.
- They reach only `allowed_hosts` under `limited`, nothing under `denied`, and everything the
  Station reaches under `open`; a host outside what is allowed is refused, never asked about.
  `denied_networks` takes hosts out of either (below).

Each conversation keeps its own: its own temporary directory, and its own Claude Code
configuration directory, so one person's transcripts and any stored login never sit where
another's scripts could reach. Her login must be a token (`CLAUDE_CODE_OAUTH_TOKEN` or an API
key in the Agent's environment): a shared login directory is not carried into her Sessions.
Claude Code's own managed settings on the Station still apply to her harness and merge with
these limits; a Station that has them as files logs it each time her harness starts, since an
`allowManagedHooksOnly` there would drop the hook that keeps her web fetches to their hosts.

A shell in the interpreter list (`bash`, `sh`, `env`) runs any program the sandbox allows, not
only the shell, so the Station logs it as a weakening; prefer a single scripting interpreter.

Her web tools follow the same policy. Under `limited` she may fetch pages from the hosts
listed (`*.example.com` covers every name under `example.com`), and a hook refuses every other
host, including the documentation sites Claude Code would otherwise fetch without asking.
Under `denied`, and under `limited` with no host, she has no web tool at all. Under `open` she
may fetch any page and search the web. A page she fetches, and a script that could reach a
host, expose the conversation ([as her memory is](#her-workspace-and-memory)). Her MCP
servers are the harness's own connections and need no entry.

Claude Code's sandbox runs on macOS, and on Linux and WSL2 with `bubblewrap` and `socat`
installed. On native Windows, or a Linux Station without them, she starts without scripts:
she is told they are unavailable on this Station, and the Station's log says why. A sandbox
that fails anyway never runs her scripts without it: Claude Code exits as it starts, which
ends her answer with the reason, or refuses each script.

!!! danger "`unsandboxed_scripts = true` gives every person and every page a shell"

    With `unsandboxed_scripts = true`, a Station whose harness has no sandbox runs her scripts
    with no limit on what they read, write or reach. Because the model can be steered by any
    text she reads — a page she fetches, a brief, another Agent's output — this effectively
    hands every person she serves, and every page she fetches, a shell running as the
    Station's own user: able to read its credentials, every other conversation and the Station
    itself. Turn it on only where the Station's user is already confined some other way (a
    container, a VM). Each harness started under it logs `ai6: weakening: assistant scripts run
    unsandboxed: …`, and the Station reports it at check-in.

`network = "open"` and `unsandboxed_scripts = true` are weakenings: a Station logs them when it
starts and reports them at check-in, and `open` is logged again with each harness it starts.

### What `open` reaches, and the deny list

`open` means open: her web tools and scripts reach everything the Station can reach, the
Station's own machine (its loopback services), its network and the cloud's instance metadata
service included. AI6 does not know a deployment's network, so it refuses nothing by default;
its own services do not rely on that, since each one asks every request for its token, from
loopback too (the Station API, the field runner, the Cable and Theater MCP endpoints, the
gateway and her MCP endpoint), and the gateway accepts an identity a reverse proxy asserts only
with the proxy's secret.

What she must not reach goes in `denied_networks`, which applies under `limited` too and which
a group or person table can add to but never take from:

| Entry | Keeps her from |
| --- | --- |
| `loopback` | The Station's own machine: `127.0.0.0/8`, `::1`, `0.0.0.0`, `::`, `localhost` and every name under it |
| `private` | The private networks: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, CGNAT `100.64.0.0/10`, unique-local `fc00::/7` |
| `link-local` | `169.254.0.0/16` and `fe80::/10`, the metadata address among them |
| `metadata` | The cloud instance metadata service: `169.254.169.254`, `fd00:ec2::254`, `metadata.google.internal` |
| `10.20.0.0/16`, `2001:db8::/32` | A network |
| `192.0.2.7`, `[2001:db8::7]` | One address |
| `git.corp.example`, `*.corp.example` | A host, or every name under it |

To keep her off the Station's own services name `loopback`; off the office or data centre
network, `private` and `link-local`; off the instance's cloud credentials, `metadata`.
A name is checked as given and by every address it resolves to, an IPv4-mapped IPv6 address
counting as its IPv4 address, so `localtest.me`, `127.0.0.1.nip.io`, `0x7f.1` and
`[::ffff:127.0.0.1]` are caught by `loopback`. Her WebFetch hook resolves a name before the
fetch; a name it cannot resolve while the list holds addresses is refused.

Claude Code's sandbox proxy, which carries her scripts' connections, refuses hosts by name and
by single address, the addresses a name resolves to included, but not by network (measured with
Claude Code 2.1.310: `127.0.0.0/8` in its deny list left `127.0.0.2` reachable). So with scripts
and a network that reaches a host, a deny list holding a network or `loopback`, `private` or
`link-local` refuses to start her harness, naming the entries; list single addresses and
names instead (`127.0.0.1`, `0.0.0.0`, `[::1]`, `localhost`), or run no interpreter. The proxy
itself also refuses a name that resolves to a loopback address and the address `127.0.0.1`.
On Codex her scripts have no network and she has no web tool, so the list has nothing to keep.

## Her scripts on Codex

On Codex her scripts run in Codex's own sandbox, under a permission profile
([Codex permissions](https://learn.chatgpt.com/docs/permissions)) the handler passes as
`ai6_assistant`: the whole filesystem denied, the platform's own paths (`:minimal`) and her
working directory readable, and only `personal/` and `scratch/` writable (a `.git` under them
stays read-only). The install directory of the `codex` executable and of each interpreter, as
found on the harness's `PATH`, is readable too (`/opt/python3.12` for
`/opt/python3.12/bin/python3`, as for Claude Code), unless that directory is `/`, is or holds
the home directory, her working directory or a credential directory (`CODEX_HOME`, any
`*_HOME`), or lies inside a credential directory: an interpreter there is unreachable. Her
scripts keep only the environment variables Claude Code's keep
(`shell_environment_policy.include_only`). On Windows the handler sets `windows.sandbox =
"mxc"`, the one Windows implementation that denies reads; `unelevated` refuses such a
profile and `elevated` needs the whole filesystem readable (Codex 0.164).

Codex's `:minimal` is wider than Claude Code's system paths: with Codex 0.164 it re-opens all
of `/etc`, `/usr`, `/dev`, `/proc` and `/run` on Linux, and `C:\Windows`, `C:\Program Files`
and `C:\ProgramData` on Windows. So the handler denies the Station's own paths again inside
it: `/etc/ai6`, `/usr/local/etc` and `/usr/etc` (not on Windows), the Station's state,
identity and secrets directories, AI6's platform state directory, and the directory of the
Station's own program (or only the program, when an
interpreter's install directory holds it). Keep the Station's configuration and secrets in
one of those, or outside what `:minimal` opens: elsewhere under `/etc` or `C:\ProgramData`
her scripts can read what the Station's user can.

Codex merges its user's own `config.toml` into these settings: a `[permissions.ai6_assistant]`
table there would add to the profile (with Codex 0.164, `network.enabled = true` in it gave
her scripts the network), and `shell_environment_policy.set` would change the variables they
keep. So before her thread starts or resumes, the handler reads Codex's effective
configuration (`config/read`) and the thread's own (`thread/start`), and refuses to start her
unless the profile, its choice as the default and the environment policy are exactly what it
passed and the thread runs in that profile without network. The refusal names the setting;
remove it from the Codex configuration of the Station's user (or its managed configuration).

What differs from Claude Code:

- **Any command, not only the interpreters.** Codex 0.164 offers the model only its code-mode
  tools, `functions.exec` (JavaScript that calls her server's tools and the command tools) and
  `functions.wait`, and keeps no list of allowed programs: inside the sandbox she may run any
  command. Each harness started with scripts reports that as a weakening (`ai6: weakening:
  Codex gives Moneypenny's thread functions.exec and functions.wait …`, and a `weakened`
  event first). The tool check reads the tools `exec` declares and admits only the command
  tools, Codex's patch tool when she has files to write, her servers' tools and resource
  tools, and a clock; anything else stops her start (`AI6_CODEX_TOOL_CHECK=warn` starts it as a
  weakening). Codex 0.164 always declares the patch tool, so there she needs `files` to start
  with interpreters.
- **No network for scripts.** Codex's app server enforced no host list for a profile's
  network in Codex 0.164 (with network on, a script reached unlisted hosts and the Station's
  own loopback services), so her scripts get none under every policy; she is told so when the
  policy names hosts, and the Station logs it. She has no web search on Codex.
- **Files only through scripts.** Codex has no file tool outside `exec`: without interpreters
  she writes nothing, and is told so.
- **Every command counts as reading her files.** The Station sees each command Codex starts
  (`exec_command`) and treats it as Claude Code's `Bash`: it reads the outside-text record's
  files, and reaches the network while the policy names hosts. A command her code-mode script
  runs is announced like any other (Codex 0.164).
- **Links do not lead out.** A script may create a link in `personal/` pointing outside her
  working directory (a symbolic link on Linux and Windows; MXC refused a junction), but neither
  a script nor Codex's patch tool reached its target with Codex 0.164: the patch tool runs
  inside the same sandbox, so a write through the link failed and the target was unchanged.

Whether Codex's sandbox runs is asked once in 15 minutes per executable and configuration:
the handler runs `codex sandbox -P` with the same kind of profile, no model involved. Where it
fails (Windows without MXC, a Linux host that refuses bubblewrap), she starts without scripts,
as on a Station where Claude Code has no sandbox, unless `unsandboxed_scripts = true`: then
Codex runs her commands with no sandbox (`danger-full-access`), the weakening above.

## Her skills

Moneypenny can use [Claude Code skills](https://code.claude.com/docs/en/skills), but only
those of a catalog in the global `agents.toml`, each pinned to the exact files it was
reviewed with:

```toml
[assistant.skills.report-writer]
path = "skills/report-writer"     # absolute, or relative to this configuration directory
pin = "sha256:..."                # ai6 assistant pin-skill skills/report-writer

[assistant.group.contractors]     # a directory group
skills = []

[assistant.person."eve@sis"]      # one person
skills = ["report-writer"]
```

`ai6 assistant pin-skill DIR` prints every file of the skill directory with its SHA-256 and
the `pin`: the digest of one line per file, path and digest. Change any byte, add or remove a
file, and the pin does not match. Everyone is given every catalog skill; a group's table,
and a person's, may leave some out, never add one, and a person in several groups keeps only
what all of their tables keep. These are the same tables that narrow [her scripts and the
web](#her-scripts-and-the-web): one table may set `skills` beside `interpreters` or `network`.

When the roster loads, each skill's `SKILL.md` is read and the load fails, naming the skill,
when:

- it has no `pin` (unless `skills_unpinned = true`);
- its front matter is not YAML between two `---` lines, or does not have `name` equal to the
  catalog's name and a `description`;
- its front matter has any key but `name`, `description`, `when_to_use`, `argument-hint`,
  `arguments`, `disable-model-invocation`, `user-invocable`, `disallowed-tools`, `paths`,
  `metadata`, `license` and `compatibility`, or it injects a command (`` !`command` ``, or a
  block opened with `` ```! ``), unless `skills_allow_tools = true`. `allowed-tools` grants
  tools without asking for the turn that loads the skill, `hooks` registers commands for the
  rest of the Session, an injected command runs before she reads the skill, and `model`,
  `effort` or `context` change her Session, so none of these loads by default.

`skills_unpinned` and `skills_allow_tools` are weakenings, logged and reported at check-in as
`allow_elevated` is, and so is a SHA-1 pin. The skill directories must be readable wherever
the roster is loaded: by every Station and by Universal Exports. A SQL roster gives absolute
paths.

With each message Universal Exports sends the skills the person is given, with the person's
directory groups. The Station takes only names its own roster gives that person (its catalog,
less what its own group and person tables leave out), as it caps her scripts and network, and
restarts her harness when anything it started under changes: her skills, a skill's path or
pin in its catalog, `skills_allow_tools` or `skills_unpinned`, her scripts and network, her
repositories (a group she left takes its `shared/` checkout with it) or her tool token. Before her harness starts it copies each skill into
her working directory's `.claude/skills/<name>/`, never through a link, then checks the copy
against its pin and its `SKILL.md` again. A skill that does not match is left out: the
Station logs `ai6: assistant skill <name> does not match its pin` (or why else it was not
installed), files a `provider` finding in its logbook, and the conversation is told. Her
standing text lists the skills she has. She cannot write there: no path with a name
starting with `.` is hers to write, and her scripts' sandbox denies writes to `.claude`.

Claude Code started with `--restricted` loads no skill from the working directory, the
user's own `~/.claude/skills` or an installed plugin, so the Station hands the directory to
it as a plugin named `ai6` (`--plugin-dir`): she calls a skill as `ai6:<name>`, gets the
`Skill` tool only when she has skills, and has no other skill to call. Injected commands are
replaced with a notice (`disableSkillShellExecution`) unless `skills_allow_tools` is set; with
it, they still go through her permission rules. A skill's other files she reads with her
file tools, which she has with a [workspace](#her-workspace-and-memory). Skill text is the
deployment's own, like a shared repository: reading it exposes nothing. Codex keeps no list
of allowed skills (it loads those of the project, the user's home directory and the system,
and its own bundled ones: [Codex skills](https://learn.chatgpt.com/docs/build-skills)), so on
Codex the Station installs none, tells
her skills are unavailable and logs why; a person who needs skills runs on Claude Code.

## Cables

She can talk to Agents for you over [Cables](agents.md#cables), in the Operations where you
may: `cable.send` on `operation:<id>` lets her send and reply there, `cable.read` lets her
read the Cables sent to her there ([access policy](access.md#the-access-policy)). With
neither anywhere she has no Cable tools at all. Universal Exports works out those Operations
with each message you send, and the Station asks it again before every Cable call, so a
permission taken away holds from her next call; when Universal Exports does not answer, the
call is refused.

The Station running her gives her a second MCP server, `cables`, with `discover_agents`,
`send_cable`, `read_cables`, `acknowledge_cable` and `reply_to_cable`; every call names an
Operation, and one outside your permissions is refused exactly as an unknown target is. Its
token expires with the token of her own tools, and the Station starts her again with new ones
before that. Recipients see the Cable as from her **for you** (`moneypenny for <your person
ID>`), and only Agents of the Operation it is sent in can get it. Your permissions are not
enough on their own: the Station's `cables.toml` must also let her Agent send and receive,
and its quotas and fan-out limits apply to her as to any Agent, except that the sender quota
is counted for each person she acts for:

```toml
# cables.toml
[[grants]]
agent = "moneypenny"
action = "send"
scope = "station"
name = "hq"

[[grants]]
agent = "moneypenny"
action = "receive"
scope = "station"
name = "hq"
```

One Agent is Moneypenny for everyone, so her conversations are never an audience: a Cable to
the Agent `moneypenny`, to a Station, a Theater, a channel or everyone does not reach any of
them. Only a reply to one of her Cables reaches her, and only the conversation that sent it,
never your other conversations or another person's. She reads only the Cables of the
Operation she names; the others stay unread.

Once the conversation is **exposed** ([as her memory is](#her-workspace-and-memory): a Cable
body exposes it too), a Cable she sends or replies with is not sent. Universal Exports keeps
that one record, and the Station shows her a Cable body only once Universal Exports has
recorded it, so a conversation resumed or moved to another Station stays exposed. The Station
holds the Cable and the chat shows it as a proposal with the Operation, the recipient, how
long it lasts and the exact text, needing `cable.send`. Universal Exports checks that what it
shows is what the Station holds (a digest of all of it) and shows nothing otherwise. Confirm,
and your permission is checked again, the confirmation is audited (`moneypenny.confirmed`),
and the Station holding it sends it if it hears within the answer timeout (a minute by
default); otherwise the proposal fails and nothing is sent. Cancel, or leave it for an hour,
and nothing is sent. A held Cable lives only in that Station's memory: if her session there
closes first, confirming it fails and says why. A body with a character that renders as
nothing is refused before it can become a proposal. With 32 Cables already held, or 32
proposals waiting, she is told so and the new one is refused; nothing waiting for you is
dropped to make room.

Every Cable sent for you is recorded twice in the audit log: by the Station's relay before the
Cable is stored (action `cable.send`, you as actor, her Agent and the audience in the target;
refusals too), and a Cable whose record cannot be written is not sent; then by Universal
Exports (`moneypenny.cable.sent`, you as actor, the conversation and the Cable).

## Views she shows

When a table or a chart says it better than prose, she shows a **view** with her `show` tool.
A view is data in a strict, versioned schema that the app renders with its own widgets:

| Block | Shows |
| --- | --- |
| `text` | Markdown: headings, lists, bold, italic, code, and links to app pages only |
| `table` | Columns and rows of plain values |
| `key_values` | A list of names and values |
| `chart` | A bar (side by side or `stacked`), line or pie chart of up to eight series; a pie is one series of up to twelve slices, none below 0 |
| `schedule` | Bars over time, one row each from `start` to `end` (RFC 3339 times or numbers), coloured by state, each optionally opening an app page (a route link) |
| `graph` | Nodes (Operations, Stations, Theaters, Missions, Sessions, Agents) and the edges between them, in layers or as a tree; a node may open an app page |
| `svg` | A drawing in SVG, with a title |
| `card` | A Mission, Station or Theater, opening its page |
| `timeline` | Dated events |
| `diff` | Two texts, line by line |
| `form` | A draft of one of her `propose_*` tools for the person to fill in |

Nothing in a view runs: there is no HTML, no script and no outside link. Universal Exports
refuses a view that is not exactly the schema, within its bounds, and tells her why, so the
app never receives it; the app checks again and shows a refused view as "View refused" with
the reason, never as a guess. Submitting a form records a proposal of its tool, which the
person still confirms. Every chart, schedule and graph has **Show as table**, which shows the
same values in a table, and a summary for screen readers.

An SVG drawing holds drawing elements (`svg`, `g`, `defs`, `title`, `desc`, `path`, `rect`,
`circle`, `ellipse`, `line`, `polyline`, `polygon`, `text`, `tspan`, `linearGradient`,
`radialGradient`, `stop`, `clipPath`), their geometry and presentation attributes and `id`,
and nothing else; `url(#id)` may name only an `id` of the same drawing. It needs a `viewBox`
and a `<title>`. A script, style, event attribute (`onload` and the like), `href`, `image`,
`foreignObject`, `use`, link, animation, filter, DOCTYPE, entity or processing instruction is
refused with the reason, never removed, so she fixes the drawing herself. Universal Exports
keeps the drawing written out again from what it accepted, and the app draws that text in the
theme's colours (`currentColor` is the text colour), at most 480 pixels high; it loads
nothing.

A view she builds after reading text from outside AI6 (a conversation that is exposed when she
shows it) is marked so, and the app frames it: "Made after reading text from outside AI6.
Check it before acting on it."

Her `missions` tool filters exactly as the Missions page does (both are held to one shared
set of cases): by Operation; by state, keeping the Sessions in those states (a Session that
reported none is working) and each Mission with one kept; then by the Station or Agent of a
kept Session and by text in the Operation, Mission, Session IDs or brief, in any case.

The bounds are settings, `[assistant.views]` in the global `agents.toml` (the `views` table of
the `assistant` record), each at least 1; Universal Exports reads them when it starts:

| Key | Bounds | Default |
| --- | --- | --- |
| `blocks` | blocks in a view | 24 |
| `view_bytes` | bytes of a view as JSON | 65536 |
| `text_bytes` | bytes of a text block | 8192 |
| `short_bytes` | bytes of a title, label, cell or other short text | 512 |
| `columns`, `rows` | columns and rows of a table | 12, 200 |
| `items` | items of a key-value list or a timeline | 100 |
| `series`, `points` | series of a chart, points of a series | 8, 200 |
| `pie_slices` | slices of a pie | 12 |
| `diff_bytes` | bytes of each side of a diff | 16384 |
| `fields` | fields of a form or a card | 20 |
| `schedule_rows` | rows of a schedule | 50 |
| `graph_nodes`, `graph_edges` | nodes and edges of a graph | 60, 120 |
| `svg_bytes`, `svg_elements`, `svg_depth` | bytes, elements and nesting of a drawing | 32768, 2000, 16 |
| `navigate_label_bytes` | bytes of the label of a [page she points to](#pages-she-points-to) | 120 |
| `svg_id_bytes` | bytes of an `id` in a drawing | 64 |

```toml
[assistant.views]
graph_nodes = 100
svg_bytes = 65536
```

The app is sent the bounds with each conversation and checks views against the same values.

## Pages she points to

When a page of the app answers better than a table, such as "the failed Missions of G", she
offers it with her `navigate` tool: `{"to": ROUTE, "label": TEXT}`, the route in its JSON form
(`{"page": "missions", "operation_id": "G", "state": ["failed"]}`) and a few words on why
(at most `navigate_label_bytes`, 120 by default). Universal Exports checks the route as it checks every
route and refuses one that is not valid with the reason ("Navigation refused: …"), so the
app never receives it. A valid one is a card in the chat: the page's name and every field of
the route as chips, which the app builds from the route itself, her label beneath, and
**Open**. Her summaries of records offer the route of what she summarised, and say when a
tool refused or cut what she could read.

A route names a page and its filters and nothing else: no action, dialog or form value, and
pages open a selection read only. Opening one only reads, so it is not audited (her `show`
is not either); the conversation records it. A filtered page always shows its filters as
chips with **Clear filters**, so nothing a route sets is hidden.

**Open** takes the page in the window: a new browser history entry, so Back returns to the
page before; an expanded chat docks, and one over the page (a sheet, or full screen on a
phone) closes, so the page is in view; the page shows **Opened by Moneypenny** until the
route changes. Her links in text, schedules, graphs and cards open pages the same way.

By default nothing opens until the person chooses **Open**. **Open Moneypenny's links at
once**, in her menu, opens each card as it arrives instead; it is the person's own choice,
kept on each device, and off unless they turn it on. Even then a card waits for **Open**
when she offered it after reading text from outside AI6 (it is framed like such a view),
or while a dialog covers the page.

Her `audit` tool takes an Operation or a Station, not both, as the Audit page does.

## Pages she publishes

`publish_page` makes a page you open in a browser tab outside the app, never inside it. It is
a **dashboard** (reads live data through `feeds`), a **report** (a snapshot of data she put in
the files, made when you ask for one) or a **file** (one log, CSV, Markdown or image). She gives
a `path` (a file or directory under `personal/` or `scratch/`, which the Station uploads; a
bundle's entry is `index.html`) or inline `content` of a `format` (html, markdown, text, csv,
json, svg, up to 192 KiB). Markdown is rendered; text, CSV and JSON are shown as text; scripts,
styles and images of a bundle load as they are; anything else (a PDF included) downloads.
`replace` keeps a page's id. `unpublish_page` deletes one; `pages` lists yours (and, when one
was made after outside text, marks her conversation exposed).

A page is kept for a day unless you **Remember** it, counts toward quotas
(`[assistant.pages]`, [Moneypenny's limits](configuration.md#moneypennys-limits)) and is audited
(`moneypenny.page.published`, `.opened`, `.remembered`, `.forgotten`, `.deleted`, `.expired`).
Only you can open it: another person, an administrator included, gets 404. The link is a
capability, `{pages base}/v/{token}/`, valid for an hour, bound to your sign-in (signing out
ends it) and to a network profile; the app mints it when you click. Opening shows the gateway's
**classification bar** over the page: "Made by Moneypenny for you, not part of Outer Office",
the title, kind and expiry, and, after she read text from outside AI6, an amber warning naming
what she read (up to five sources, then a count). The close control hides the bar for that
view; a reload shows it again.

Pages are open by default: scripts run and they may fetch from anywhere, because a dashboard
may need a chart library or an API. What they cannot do is act as the app. Every response
carries `Content-Security-Policy: sandbox ...` without `allow-same-origin`, so the page is an
opaque origin with no cookie, storage or sign-in of its host, `/v1` answers it no CORS header,
and it cannot navigate the top window. Dashboards read **live** data: each feed is a read tool
with input fixed at publish, run when the page asks for `data/NAME` as whoever holds the link,
so a viewer sees only what their own permissions allow and a revoked grant applies at once.
Feeds read; they never propose or send. Options, set by an administrator for everyone:
`scripts = false`, `network = "own"` (its own files and feeds) or `"none"` (its own files), `feeds
= false`, and a separate origin for pages (`unex.cfg` `[pages] origin`). You may also open one
page with the network off.

The remaining risk, plainly: a dashboard made after she read outside text can be steered to
read, through its declared feeds, whatever its viewer may read, and send it anywhere for as
long as the link lives. Feeds are read-only and fixed at publish, so it can neither change
anything nor read past them. The bar naming what she read is the mitigation; `network`, `feeds`
and opening with the network off are the tighter options. On the API's own origin the address
bar shows the app's host, so a page can draw a faithful sign-in: a separate origin removes that
and a proxy that strips the sandbox header.

## In the app

**Ask Moneypenny** (top right on every page, or Ctrl+Alt+I; ⌃⌘I on macOS) opens the chat:
beside the page in a wide window, over it in a narrower one, full screen on a phone. It can
be expanded into the centre (Ctrl+Alt+Shift+I) and, on desktop, popped out into its own
window (Ctrl+Alt+O); every layout shows the same conversation, and the app remembers the
layout per person on each device (the app's README has the details). The app's demo build
(`OO_DEMO=true`) shows a scripted sample conversation instead, labelled as one; it reaches no
network and its proposals change nothing.

The app opens a WebSocket to `/v1/user/moneypenny` and sends its access token in the first
frame. With each message it sends the page the person is on and its filters, as the page's
route (for example `app:/missions?operation_id=G&state=failed`), so "why is this one stuck?"
means something. Her links and page cards open app pages the same way
([Pages she points to](#pages-she-points-to)). Her answers, the tools she used, her
views and her proposals (with **Confirm** and **Cancel**) appear in the chat.
Conversations are kept per person (20 each, for a week) in Universal Exports' memory;
reopening one shows what was said. One message is answered at a time.

While she answers, **Stop** takes the place of Send. It sends `{"type":"interrupt"}` on the
socket, which only ever reaches the person's own conversation. A message no Station has
taken up yet is simply withdrawn (a Station that was just starting on it shows nothing of
that answer and closes her harness, which from the Stop on has no tools and no Cables; a
Cable it sent before is still recorded in the audit log); otherwise Universal Exports asks the Station running her
harness to interrupt it, and that Station's handler stops the turn the way the harness
expects (a stream-json interrupt request for Claude Code, `turn/interrupt` for Codex). The
conversation then shows "You stopped Moneypenny's answer.", her harness stays up for the next
message, and she is told with that message that she was stopped. Stop with nothing being
answered does nothing.

## Settings

| Setting | Default | Where |
| --- | --- | --- |
| The Agent she is | `moneypenny` | the `unex` record's `assistant_agent` ([Configuration](configuration.md)) |
| Serve her at all | yes | the `unex` record's `assistant` |
| The bounds of her views | [Views she shows](#views-she-shows) | `[assistant.views]` |
| What a job, a report and her memory may hold | 32 KiB a message, 64 events a report, and so on | `[assistant.limits]` ([Moneypenny's limits](configuration.md#moneypennys-limits)) |
| Conversations kept | 20 a person, 2,000 in all, 7 days | `[assistant.gateway]` `conversations_per_person`, `conversations`, `conversation_age` |
| Tool token lifetime | 1 hour | `[assistant.gateway] token_ttl` |
| Time for a Station to take a message | 60 seconds | `[assistant.gateway] answer_timeout` |
| Tool calls per conversation | 120 a minute | `[assistant.gateway] tool_calls_per_minute` |
| Conversations of one person with a running harness or an answer pending | 2 (the least recently used idle harness is closed to make room) | `[assistant.gateway] live_per_person` |
| How long an answer may go without word from its Station | 10 minutes, then it is given up | `[assistant.gateway] answer_deadline` |
| Memory kept | 1 MiB per conversation, 4 MiB per person (oldest views go first); the app is sent the newest 100 entries | `[assistant.gateway]` `conversation_bytes`, `person_bytes`, `ready_entries` |
| Memory requests waiting for a Station | 2 a person, 16 in all; `429` past either | `[assistant.gateway]` `memory_requests_per_person`, `memory_requests` |
| Assistant Sessions on one Station | 4 running or starting; closed after 10 idle minutes | `[assistant.desk]` `live`, `idle` |
| Memory requests a Station makes at once | 4; the next is refused | `[assistant.desk] memory_requests` |
| A clone of one of her repositories | 120 seconds and 512 MiB, then given up and removed | `[assistant.desk]` `clone_timeout`, `clone_bytes` |
| What one save of her work commits | 16 MiB; past it nothing is saved and she is told | `[assistant.desk] save_bytes` |
| Personal repositories | none: no workspace | `[assistant] personal_repo` in the global `agents.toml`, or the `assistant` record of a SQL roster |
| Shared repositories | none | `[assistant.shared.<group>]`, `[assistant.org]` (`write = false`) |
| Run on an elevated Station | no | `[assistant] allow_elevated` |
| Interpreters her scripts may run | `python3`, in the harness's sandbox | `[assistant] interpreters` |
| Hosts her web tools and scripts reach | none (`limited` with no host) | `[assistant] network`, `allowed_hosts` |
| Scripts where the harness has no sandbox | none | `[assistant] unsandboxed_scripts` |
| Skills | none | `[assistant.skills.<name>]` (`path`, `pin`) |
| Narrower scripts, network and skills for a group or a person | none | `[assistant.group.<group>]`, `[assistant.person.<id>]` |
| Load a skill without a pin | no | `[assistant] skills_unpinned` |
| Load a skill that grants tools, registers hooks or runs commands | no | `[assistant] skills_allow_tools` |
