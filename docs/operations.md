# Operations

An Operation is a registered project or workspace and its policy. Operations are global:
no Station owns one, and any Station whose labels match the Operation's `station_labels` (and
that has its checkout) can run its Missions. The configuration owns the checkout, Agent
roster, clearance ceiling, concurrency budget, workspace and placement. AI6 reads Operations from `operations.toml` in its configuration directory
unless the Station is given another source ([Where configuration comes from](#where-configuration-comes-from)).
Only registered Operations may contribute a local `.ai6/agents.toml` override.

```toml
[operation.G]
name = "guatiao"
path = "~/projects/guatiao"
roster = ["007", "006"]
clearance = "field"
max_concurrent = 3
workspace = { provider = "git", base = "origin/main", branch = "agent/{id}-{slug}", isolate = true }
station_labels = { gpu = "yes", os = "linux" }
stations = ["lab-gpu", "hq-1"]
cover = "covert"
on_blown = "flag"
sources = { jira = "GUA" }
```

The Operation ID (`G`) and project name (`guatiao`) both resolve to the same registered
Operation. `path` is the local checkout. `theater` binds the Operation to one
[Theater](theaters.md) by ID; when it is bound to a remote Theater, `path` is read on that
Theater. `roster` limits which Agents may be assigned; when omitted or empty, every
configured Agent is eligible. `clearance` sets the highest authority available to its
Missions. `station_labels` is matched against the labels of an execution Station and its
Theater. `stations` optionally lists the Stations allowed to run the Operation's Missions;
empty or absent allows every Station. A Mission can narrow that list but never widen it.
See [Placement](stations.md#placement) for the full set of checks.

## Workspace

`workspace` says where a Mission's Agent works. A **safe-house provider** makes the
workspace: `git` cuts a worktree on a new branch under the AI6 state directory. An
untouched safe house is removed when the Mission ends; one with changes is retained and
recorded in the Mission report.

| Key | Default | Meaning |
| --- | --- | --- |
| `provider` | `git` when the path is a Git checkout, else none | The safe-house provider ID. With none, Missions work in `path` directly and cover is not checked |
| `source` | the Operation's `path` | Where the work comes from (a local path) |
| `base` | `HEAD` | Revision a safe house starts from |
| `branch` | `{id}-{slug}` | Branch name template; `{id}`, `{slug}` and `{operation}` are filled in |
| `isolate` | `true` | `false` works in `source` itself instead of a separate copy |
| `checkpoint_remote` | none | A Git remote (name or URL) each running Mission's safe house is checkpointed to, on its own branch, so a later attempt on another Station starts from its work. Unset pushes nothing |
| `checkpoint_secs` | `300` | Seconds between checkpoints while a Session runs. Must be above zero |

Two providers ship: `git` (a worktree on its own branch; commits are audited) and `copy`
(a plain copy of the directory, always kept, no history).

Providers are loaded by the Station that runs the Session (`ai6 serve --plugin-path`). A
provider that keeps no history makes the Mission's cover report say it was **not
checkable** instead of passing.

## Cover

Operations can set a cover policy and author identity for commits:

```toml
[operation.G]
cover = "partial" # covert, partial, overt or a configured policy
cover_identity = { name = "Field Office", email = "field@example.invalid" }
on_blown = "fail" # or "flag"
```

The identity is applied as the Git author and committer. AI6 checks Mission commits
against the configured cover policy and records any violations in the Mission report.

## Provider limits

When a provider refuses work until a quota resets (a usage or rate limit), the Operation
decides what happens to the Mission:

```toml
[operation.G]
on_limit = "rotate"        # or "wait" or "backup"
limit_fallback_secs = 1800 # how long to park when the provider gives no reset time
pools = ["claude-team"]    # the account pools its Missions may draw from; default: every pool
```

| Key | Default | Meaning |
| --- | --- | --- |
| `on_limit` | `rotate` | `rotate` moves the Session to another account of its [pools](agents.md#account-pools) that has headroom: a Claude Session on an environment credential resumes the same provider Session there; a Codex or `config-dir` Session, whose history stays with its account, is tried again as a new attempt with a hand-off. Without a pooled account to move to it waits. `wait` parks the Mission and resumes the same provider Session when the quota resets. `backup` posts the next qualified Agent whose handler is different, as a new attempt in the same safe house, and parks only when there is none |
| `pools` | every pool | The account pools whose own `operations` admit this Operation that its Missions may use; an empty list uses none |
| `limit_fallback_secs` | `1800` | Seconds to park when the provider reports no reset time (or one already past). Must be above zero |

Both are overridable per Operation and an unknown `on_limit` value fails the load. See
[Recovery](missions.md#recovery).

## Lost Sessions

A Session is lost when its Station goes dark and no other Station can re-attach to it (its
harness ran on that Station's own host), or when its Station lives but the Theater's runner,
or a wrapped harness, stays out of reach:

```toml
[operation.G]
on_lost = "retry" # or "fail"
lost_after_secs = 45
```

| Key | Default | Meaning |
| --- | --- | --- |
| `on_lost` | `retry` | `retry` queues the brief again as a new attempt on an eligible Station, starting from the last checkpoint (see `workspace.checkpoint_remote`) with a hand-off about the lost attempt, up to three attempts in all. `fail` files the brief as lost |
| `lost_after_secs` | `45` | Seconds a Session's runner or wrapped harness may stay out of reach (three Station check-ins) before its Station probes it once more through the Theater's transport and, if it is still gone, marks the Session lost. Must be above zero |

An unknown value fails the load. See [Station loss](missions.md#station-loss-adoption) and
[Runner loss](missions.md#runner-loss-the-station-lives).

## Spreading or packing work

Among the Stations and Theaters that pass placement's checks, an Operation's Missions go to
the least-loaded place by default, or fill the busiest place that still has room:

```toml
[operation.G]
placement = "pack" # or "spread"
```

| Key | Default | Meaning |
| --- | --- | --- |
| `placement` | `spread` | `spread` takes the lowest load score first, sharing work out; `pack` takes the highest, keeping other Stations and Theaters free. See [Placement](stations.md#placement) for the score |

An unknown value fails the load.

## Budgets, exposure and quarantine

Three optional tables set how [Medical](medical.md) treats the Operation's Agents. None sets
a spending limit unless you write one.

```toml
[operation.G.budget]
mission_cost_usd = 5.0
operation_cost_usd_per_day = 40.0
# preset = "strict" sets conservative limits for every key you leave out

[operation.G.exposure]
preset = "strict"   # or "observe": record exposure, restrict nothing
tools = ["WebFetch", "WebSearch"]

[operation.G.quarantine]
stalls = 3          # 0 turns a trigger off
budget_stops = 2
blown = true
window_hours = 24
```

The same `[agent.<id>.budget]` table sets limits for one Agent. Every key, its default and
what it does is on the [Medical](medical.md) page; an invalid value fails the load with an
error naming the key.

## Sources

`sources` lists where an Operation's work comes from. Each key names a Source provider and
holds that provider's settings; the Station holding the Headquarters lease polls them and
queues what it finds. See [Sources](missions.md#sources).

```toml
[operation.G.sources.github]
repo = "jose-pr/ai6"
labels = ["ai6:ready"]
interval_secs = 120

[operation.G.sources.jira]
site = "https://example.atlassian.net"
project = "GUA"
email_env = "JIRA_EMAIL"
```

Credentials are never configured: a source names the environment variable that holds its
token (`token_env`), and the Station reads it when it polls. A bare string is shorthand for
the provider's main target, as in `sources = { jira = "GUA" }`.

## Green ink

`green_ink` says which irreversible steps an Agent must ask a person to sign
([Green ink](missions.md#green-ink)). The default, `strict`, asks for every governed step
at every clearance and lets no `desk` Agent ask; `licensed` also lets `00` Agents push and
merge without asking. Every key overrides the preset:

```toml
[operation.G.green_ink]
preset = "licensed"
kinds = ["deploy"]                       # added to push, merge, release, delete, clearance, safe-house-delete
requesters = ["field", "00"]             # clearances that may ask
unattended = { "00" = ["push", "merge"], field = [] }   # per clearance named
protected = ["refs/heads/main", "refs/heads/release/*", "refs/tags/*"]
request_ttl_secs = 86400                 # an undecided request then expires as refused
```

| Key | Default | Meaning |
| --- | --- | --- |
| `preset` | `strict` | Starting point for the keys left out |
| `kinds` | none | Step kinds the Operation governs besides the built-ins; an Agent may not ask for an ungoverned kind |
| `requesters` | `["field", "00"]` | Clearances whose Agents may ask |
| `unattended` | none (`licensed`: `00 = ["push", "merge"]`) | Kinds a clearance takes without asking; each clearance named replaces its list |
| `protected` | `refs/heads/main`, `refs/heads/master`, `refs/heads/release/*`, `refs/tags/*` | Ref globs a `push` needs green ink for; a push elsewhere does not |
| `request_ttl_secs` | `86400` | How long a request waits for a decision |

An invalid value fails the load with an error naming the key. A Station prints each setting
looser than `strict` when it starts.

## Harness project settings

`harness` says whether an Agent's harness may load the configuration its working directory
carries. Claude Code reads settings, hooks, permissions, skills, subagents, commands and
project `CLAUDE.md` through its `project` and `local` setting sources (`.claude/`, `CLAUDE.md`)
and MCP servers from `.mcp.json`; Codex reads its `.codex/` layer (config, hooks, rules). Hooks
and MCP servers run commands, often scripts kept elsewhere in the checkout, with the
Agent's environment, and in an issue-driven Mission whoever wrote the branch could have
written all of it ([Hooks](hooks.md)).

```toml
[operation.G.harness]
project_settings = "pinned"       # ignore (default) | pinned | load
pin = "sha256:..."                # what `ai6 harness pin DIR` prints
```

| `project_settings` | The harness loads |
| --- | --- |
| `ignore` (default) | only the user's configuration and AI6's own: Claude Code runs with `--setting-sources user --strict-mcp-config`, so no project settings, hooks, skills, subagents, commands, project `CLAUDE.md` or `.mcp.json` servers; Codex runs with the working directory marked an untrusted project |
| `pinned` | the project's configuration while its manifest (below) has the digest `pin`; otherwise it starts as `ignore` |
| `load` | whatever is there; a Station prints it as a weakening when it starts |

The **manifest** of a working directory is every regular file under `.claude/` and `.codex/`
(at any depth), `.mcp.json`, and every file inside the working directory that a `command` or
`args` entry of a hook or MCP server names: in JSON and TOML settings, and on a `command:`
line of a skill's or subagent's front matter; `$CLAUDE_PROJECT_DIR` stands for the working
directory. `pin` is the digest of the manifest's lines, `PATH NUL sha256:HEX LF` in path
order. A symbolic link, a device, a pipe, a file over 1 MiB, more than 16 MiB or more than
4096 files in all make the manifest unreadable, and the harness starts as `ignore`. Under
`pinned`, Claude Code keeps `--strict-mcp-config` unless the manifest holds `.mcp.json`.

Each `pinned` start and resume reads the manifest on the Station and records its digest on
the Operation's timeline (`harness`): `project-settings-pinned` when it matches,
`project-settings-ignored` with the reason when it does not or cannot be read. A `pinned`
Operation whose harness runs on another host than its Station starts as `ignore`, since the
Station cannot read the files there.

What `pinned` does not cover: a command that names no path inside the working directory
(a program found on `PATH`, a script fetched at run time), anything a pinned script itself
reads, and changes made after the check, including by the Agent during the Session (Claude
Code reloads a changed settings file). A SHA-1 `pin` is accepted and reported as a
weakening. AI6's own settings, the green ink hook (`--settings`) and the Station's MCP
servers (`--mcp-config`), apply in every mode.

## Vetting

`vetting` sets the checks M runs on each brief before it is placed
([Vetting](missions.md#vetting-and-held-briefs)). Every check is on by default; each key
overrides one, and `vetting = "off"` turns them all off.

```toml
[operation.G.vetting]
max_brief_bytes = 65536
max_per_hour = 20             # 0 is no limit
max_per_source_per_hour = 10  # 0 is no limit
duplicates = true
budget = true
screening = "untrusted"       # or "all", "off"
phrases = ["deploy to production now"]
html_comments = true
rate_hold_hours = 24          # 0 keeps rate holds until an operator decides
review = "untrusted"          # or "all", "off"
max_proposal_reviews_per_hour = 20             # 0 is no limit
max_proposal_reviews_per_session_per_hour = 5  # 0 is no limit
```

| Key | Default | Meaning |
| --- | --- | --- |
| `max_brief_bytes` | `65536` | Largest brief text: a Source brief's quoted item, otherwise title and body. A larger brief is refused |
| `max_per_hour` | `20` | Untrusted briefs passed per rolling hour; the next is held |
| `max_per_source_per_hour` | `10` | The same, per Source |
| `duplicates` | `true` | Hold a brief with the same URL or title as queued, held or running work |
| `budget` | `true` | Hold briefs while the Operation's day spend is at `operation_cost_usd_per_day` |
| `screening` | `untrusted` | Which briefs are screened for invisible Unicode, forged fence lines and manipulation phrases |
| `phrases` | none | Phrases screened for beside the built-in patterns, matched ignoring case, punctuation and spacing |
| `html_comments` | `true` | Screening also holds a brief whose text outside its quoted item carries HTML comments, which render as nothing |
| `rate_hold_hours` | `24` | How long a brief held only for a rate limit stays held before it returns to the queue to be vetted again |
| `review` | `untrusted` | Which briefs a qualified `chief-of-staff` Agent reviews before they are placed |
| `max_proposal_reviews_per_hour` | `20` | Mission proposal reviews queued per rolling hour; the next proposal is held |
| `max_proposal_reviews_per_session_per_hour` | `5` | The same, for the proposals of one Agent Session |

Untrusted briefs are a Source's items, Agents' Mission proposals, and any brief without a
valid provenance (one dropped into the Registry queue by hand, or changed after it was
accepted); an operator's or a Sleeper's brief is trusted. An invalid value fails the load with an error naming the key. A
Station prints each setting looser than the default when it starts and when the
configuration changes.

## Sleepers

`sleepers` holds standing orders that queue a brief on a cron schedule, once an operator
authorizes them with `ai6 sleeper authorize`. See [Sleepers](missions.md#sleepers).

```toml
[operation.G.sleepers.nightly-deps]
schedule = "0 3 * * *"
title = "Update dependencies"
prompt = "Update the lockfile and run the tests."
```

## Where configuration comes from

A Station's configuration (Operations, Agents, roles, covers, Moneypenny's settings and its
Theaters) comes from a configuration provider. A program that embeds the Station can keep it
anywhere else (a database, an HTTP API, its own code): it builds the roster from typed records
with `Roster::builder()` (or `Roster::from_parts`), where `Operation::new`, `Agent::new`,
`AgentRole::new` and `Cover::new` start from the same defaults a file gives, and passes a
`ConfigProvider` (such as `MemoryProvider`) to `ServeConfig::with_config_provider`. Records
built this way get the same checks as the files; an invalid configuration stops the Station
rather than half-loading.

`ai6.cfg`'s `[provider] location` (or `ai6 serve --provider LOCATION`) names the provider:

- **A directory** (by default the one holding `ai6.cfg`), read-only: one file per family,
  `operations`, `agents` (Agents, roles and `[assistant]`), `covers` and `theaters`, each
  with the extension `.toml`, `.json`, `.yaml`, `.yml` or `.cfg` (TOML). Two files for one
  family (`agents.toml` beside `agents.yaml`) are refused. A Station's own records go in
  `stations/<id>/` beside them, for the families that take them (Theaters); a Station reads
  its own over the global ones, key by key.
- **`sqlite:PATH` or a `postgres://` URL**, for Stations that share one configuration
  without a shared filesystem. Each family is a table, `ai6_config_<family>`
  (`ai6_config_operation`, `ai6_config_agent`, `ai6_config_role`, `ai6_config_assistant`,
  `ai6_config_cover`, `ai6_config_theater`), each row a record's scope (`''` for every
  Station, `station:<id>` for one), key and a JSON object with the keys its table has in the
  files, so `[operation.G]` with `roster = ["007"]` is the row `('', 'G', '{"roster":
  ["007"]}')`. The rows get the files' checks, `extends` included; an Operation's own
  `.ai6/agents.toml` still applies. Moneypenny's `[assistant]` settings are the one row of
  `ai6_config_assistant`, keyed `settings` ([her
  workspace](assistant.md#her-workspace-and-memory)); without it the roster has the defaults.
  `put` and `remove` change one record and refuse a change that would leave any Station's
  configuration invalid. Each change moves the configuration to its next revision (the
  `config` row of `ai6_clock`); a tool that edits the tables directly advances that row in
  the same transaction.

Each Station reads its loadouts and Cable settings from the configuration directory (the
bootstrap file's own directory when the provider is a database).

### Changing the configuration of running Stations

A Station asks its configuration provider for its revision every 5 seconds (`ai6.cfg`'s
`[provider] poll`, `0` to stop) and applies a new one without a restart: new requests, the scheduler and new
Sessions use it, while a Session already running keeps the roster it started with. Every
Station sharing a SQL roster therefore takes up an edit within one poll. A new revision is
checked as the files are checked, every Agent resolved; one that fails is refused once:
the Station logs why, records a `config.refused` audit event under its own `station:<id>`,
reports the reason at check-in (Outer Office shows it beside the roster), and keeps running
the last good roster. A source it cannot reach is logged, not audited, and changes
nothing.

`loadouts.toml` is read again when its contents change, with the same refusal rule (unless
the loadouts were set in code); the Station then declares its pool accounts again and
withdraws the ones it no longer has. A secret's value is read from its source each time it
is leased, so it never needs a reload.

Everything else applies only at start, and a change waits for a restart: Theaters (a
Station that sees a `theater` record change logs it and lists `theater` as needing a restart
at check-in), `cables.toml` (listed the same way), the bootstrap file `ai6.cfg` (listen address, Registry and provider
locations, Station ID, plugin paths, the Headquarters lease), and the command-line settings:
roles, labels, terminal, capacity and watchdog defaults.

Universal Exports edits the roster for operators when it is started with the same
location (`unex.cfg`'s `[provider] location`, or `--provider`): `roster.read` and `roster.manage` on the
scope `config:roster` (for example `scope = "config:roster"` in `access.toml`) let a person
list and change Operations, Agents, roles and covers from Outer Office's Roster page.
The source checks the whole roster before it keeps an edit; an accepted edit is audited as
`roster.update` or `roster.remove`, a refused one answers with the reason and is audited as
`roster.refused`. A configuration directory is listed read-only: its files are edited by
hand, and every Station using it takes up the change at its next poll.

Missions and provider Sessions are visible in Outer Office and through `ai6ctl`. The Mission ID identifies
the work; the provider-issued Session ID identifies one resumable agent run. See
[Missions and intake](missions.md) and [Agents and clearance](agents.md).
