# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Moneypenny keeps each person's work and memory in their own repository. The global
  `agents.toml` takes an `[assistant]` table (`ai6::AssistantSettings`, also
  `RosterBuilder::assistant`): `personal_repo` (`{person}` or `{person_digest}`; a local or
  `file://` repository is created bare on first use), `[assistant.shared.<group>]` and
  `[assistant.org]` repositories (`write = false`), and `allow_elevated` (false). Universal
  Exports sends each message with the person's workspace (`AssistantJob::workspace`); the
  Station clones `personal/` and `shared/<name>/` into the conversation's directory, writes
  `CLAUDE.md` (importing `memory/MEMORY.md`), `AGENTS.md` and a context file beside it, and
  after each answer commits `personal/` to `main` as the person with a `Via: Moneypenny
  (conversation <id>)` trailer, replaying once and falling back to
  `moneypenny/<conversation digest>`. Changes to `memory/` are set aside and reported
  (`memory_changes`): committed at once in a conversation that has read nothing others wrote,
  a `memory_commit` proposal once it has, and a confirmed one commits exactly the confirmed
  files. `propose_shared_change` proposes a change to a group's shared repository for its
  members, or commits it at once (audited `moneypenny.shared.write`) when the group's
  repository has `write = true`. `{person}` takes lowercase identifiers only;
  `{person_digest}` works for all. A Station takes only the repositories its own roster gives
  the job's `person_id`. Her repositories' git directories live beside her workspace, every
  Git command the Station runs on them has hooks and filesystem monitors off, and paths with
  a name starting with `.` are neither written by her nor committed. A starting desk removes
  what desks that are gone left in the temporary directory.
- One record of a Moneypenny conversation's exposure, `Conversation.exposed` in Universal
  Exports, carried to the Station as `AssistantJob::exposed`: every read tool sets it but
  `field_guide`, `profile` and `station_settings`; so does a workspace with a shared
  repository, from the first message; and the Station sets it (`POST
  /v1/station/assistant/exposure`, read with `GET`) when she reads files while her repository
  records files written while exposed (`.moneypenny/outside-text`).
- An assistant mode names the directories she may write (`AssistantMode::files`) and a
  context file (`AssistantMode::context_file`): Claude Code gets its file tools under
  `--permission-mode dontAsk` with `Edit(/<dir>/**)` rules and the file through
  `--append-system-prompt-file`; every assistant Session runs with
  `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`. The Codex handler refuses both.
- A Station running elevated (root, or an administrator's elevated token on Windows;
  `ai6::station::elevation::is_elevated`) refuses every assistant job unless
  `assistant.allow_elevated` is set, which is logged as a weakening at each harness start and
  published at check-in (`assistant_weakened`) with the other `[assistant]` weakenings.
- The `git` safe house clones a repository (`SafeHouseSpec::clone`, a URL or local
  repository, created bare on request) and saves to a branch it shares
  (`CheckpointSpec::branch`): it commits in place, pushes as a fast-forward, replays once on
  a refusal, then pushes to `CheckpointSpec::fallback`. The `copy` safe house refuses a clone.

- Moneypenny's answer can be stopped: the app's **Stop** button, in place of Send while she
  answers, sends `{"type":"interrupt"}` on the chat socket, which acts only on the socket's
  own conversation and is a no-op (`stopping: false`) when nothing is being answered. A
  message no Station has accepted is withdrawn; otherwise Universal Exports queues an
  interrupt job (`AssistantJob::interrupt`) for the Station running her harness, whose
  desk calls the handler's interrupt (stream-json for Claude Code, `turn/interrupt` for
  Codex) and reports the turn as `interrupted` (`EVENT_INTERRUPTED`). The conversation
  shows a `notice` entry, the harness stays up for the next message, and she is told the
  person stopped her.
- `POST /v1/user/operations/{operation}/missions/{mission}/actions` answers `404` for a
  Mission nothing knows (no Session record, brief lease, queued brief of the Operation or
  board entry) before anything is audited or queued; the Headquarters Station takes only
  known Missions no other Station holds.
- Windows exit codes are reported as signed 32-bit values by the `pipe` and `pty` terminals
  (`0xFFFFFFFF` is `-1`), and the Claude Code and Codex handlers show a negative one with its
  NTSTATUS hex (`exited with code -1073741510 (0xC000013A)`, through
  `ai6::directorates::q::terminal::exit_code_text`).
- The Station logs the handler and terminal provider of each harness it starts or resumes,
  for Missions and Moneypenny alike (`ai6: Mission "<id>": started the claude harness in the
  pipe terminal`).
- The Codex tool check sees what an app-server thread is sent: it keeps `incremental_tools`
  on (`ai6_agent_codex::KEPT_FEATURES`), without which `codex debug prompt-input` renders no
  tool while the thread is still sent `functions.exec` and `functions.wait`. Codex 0.164 sends
  those two to every model in its catalog and no setting removes them, so in `strict` Codex
  answer-only (Chief of Staff) and assistant threads are refused on that release. Restricted
  threads also get `tools.experimental_request_user_input={enabled=false}`, and every MCP
  server of the user's own Codex configuration that is not the thread's is switched off
  (`mcp_servers.<name>.enabled=false`, names from `codex mcp list --json`, read and cached
  with the feature list): a `-c mcp_servers` table adds to the user's servers rather than
  replacing them. A server list that cannot be read, or a name that cannot be addressed,
  fails the check. `tools.view_image`, which Codex reports as unrecognized, is not passed.
- The `pipe` terminal provider (`ai6-terminal-pipe`): the program gets plain pipes for its
  standard input and output and no terminal, so its output arrives byte for byte and a
  harness that checks for a terminal reads its input. Standard error is merged into the
  output (`stderr = "merge"`, the default) or dropped (`"drop"`). Kill takes the whole tree:
  a process group on Unix, a Job Object on Windows.
- A handler declares how its harness talks (`AgentHandler::stdio`: `terminal` by default,
  `json` for the Claude Code and Codex handlers) and a terminal provider whether it gives a
  terminal (`Terminal::tty`). A Station, its assistant desk and the field runner give a JSON
  harness a provider without a terminal, preferring `pipe`, and refuse the Mission at start
  with the reason when none is available or `--terminal` pins one that gives a terminal;
  other harnesses prefer `pty`. `ai6::choose_terminal` and
  `Plugins::choose_terminal` apply the rule, and `ai6::DEFAULT_TERMINAL` is in
  `ai6::directorates::q::terminal` beside `DEFAULT_PIPE`. Claude Code 2.1.310 with `-p` exits
  at once when its standard input is a terminal, so it runs under `pipe`.
- The `pty` terminal reports the end of a program's output (`read` returns `0`) once the
  program has exited and its output has been quiet for 200 ms, on every platform, so a
  harness that exits is noticed although ConPTY keeps the output open after its program has
  exited. `kill` returns once the program, and on Windows its whole Job Object, has exited
  (at most 2 s).
- The Claude Code handler interrupts a turn with stream-json's `interrupt` control request
  (`{"type":"control_request","request_id":…,"request":{"subtype":"interrupt"}}`), which
  stops Claude on `pipe`, where a written `^C` is only input. Claude's `control_response` is
  read and dropped, never an event, and an `error` response is logged; Claude Code 2.1.310
  answers `success` even when no turn is running. Each message is written in one write, so
  an interrupt cannot land inside it.
- The Claude Code and Codex handlers notice a harness that exits while its terminal keeps the
  output open (a timed-out read asks the terminal's exit status) and end the Session with one
  `failed` event naming the exit code or signal and quoting the last eight lines the harness
  wrote outside its JSON stream, so a dead harness no longer holds a Mission until the stall
  watchdog or an assistant slot for good. `close` waits up to 2 s for the harness to exit.
- The Claude Code handler's resume sends its message right after starting Claude Code and
  returns without waiting for output, since with stream-json input Claude writes nothing
  until it has read a message; the resumed Session's ID is checked as events are read.
- A restricted (answer-only or assistant) Claude Code Session runs with
  `CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1`, so Claude Code attaches no server-side advisor tool,
  and its tool tripwire also ends the Session on any tool the model's API runs itself (a
  `server_tool_use` or `mcp_tool_use` block, reported as `TYPE:NAME` in
  `AgentEvent::tools`). The handler's deny lists name only tools Claude Code 2.1.310 knows,
  without `MultiEdit` and `LS`.
- A Station's assistant desk closes a harness that fails or exits at once, reporting the
  handler's reason to the conversation and freeing its slot; one that writes nothing for ten
  minutes while answering is closed the same way. A resume that fails, when asked or before
  the resumed harness confirms its Session, is logged on the Station and the conversation
  starts afresh with its recap. A closed harness's directory is removed with retries for up to
  2 s while Windows still holds it.
- The `pty` terminal on Linux and macOS starts its program in non-canonical mode with no
  echo, so a handler's input line of any length (a large stream-json message or JSON-RPC
  request) reaches the harness whole and what is written does not come back as output.
  Signals (a written `^C`), CR-to-LF on input and output processing stay on.
- Theater setup on a round: a Head of Station round may call `provision_theater {theater,
  apply}` on a Theater its Station reaches. `apply = false` checks (as `ai6 theater provision
  --check`); `apply = true` runs exactly the steps the Theater's `requires`, `provision` and
  `runner` declare, refreshes the probe report placement reads, and audits
  `head_of_station.provisioned` when a step changed; it is refused while a Mission of the
  Station works on the Theater, and a failed step's output is withheld from the round. An
  ephemeral template and a Theater made for one Mission are refused.
- Safe-house hygiene within fixed limits: a Head of Station round may call
  `tidy_safe_house {mission, action, reason?}` on the kept safe house of a Mission of its
  Station that is over, holding the Mission's start reservation throughout so a retry or
  resume cannot reopen it meanwhile. `caches` removes the hygiene's cache directories once the
  safe house has been left alone for the cache age (in a Git safe house only directories Git
  ignores whole); `remove` discards it at once when its provider shows everything exists
  elsewhere (ignored files only inside cache directories), and otherwise files a green ink
  request of the new built-in kind `safe-house-delete` (`SAFE_HOUSE_DELETE`, compared as exact
  text; target `safe house MISSION on Station ID`) under the Operation that owns the safe
  house, on its Mission, that a person must decide even when that Operation's policy would let
  the Agent delete unattended (`green_ink::Desk::request_always`); no class authorisation
  approves it. Any later call spends the live approval on that one removal. A Mission that is
  not over or is being started, one the Station does not keep, and any other action are
  refused. Removals are audited
  (`head_of_station.caches_removed`, `head_of_station.removed`, resource `station:ID`) and
  record `kept = false`. `ai6 serve --safe-house-cache NAME` (repeatable) and
  `--safe-house-cache-age SECS` set the hygiene.
- Head of Station tools: a round (a Mission of role `head-of-station` whose leased brief is its
  Station's round of an authorized `each_station` Sleeper, by the provenance the Registry kept)
  gets `station_report` and `file_report` on its Cable endpoint, started or resumed; no other
  Session lists them, and each call checks again that the round is live and its Sleeper still
  authorized. Universal Exports refuses a brief for the `head-of-station` role and an Agent
  may not propose one. A round that reads `station_report` or a non-empty logbook is marked
  exposed (`STATION_REPORT`, `STATION_LOGBOOK`), since both quote other Missions' text; logbook
  entries reach later rounds quoted. `station_report`
  reads the Station's capacity, Missions not over, free space under the state directory,
  resources, Theaters with why placement would not use them, every kept safe house (Mission
  state, age, size, cache directories, whether its work exists elsewhere), provider libraries
  loaded without a pin and the enforced limits; nothing secret. `file_report` files a
  `StationReport` with typed findings in the logbook, records a timeline event and sends it to
  the `heads-of-station` Cable channel when configured. Each check-in carries the latest
  report (`head_of_station`); Universal Exports adds it to `BoardStation` for people with
  `station.read` on the Station, and Moneypenny shows it on the Station card. `StationConfig::with_safe_house_hygiene(SafeHouseHygiene)` names the
  cache directories (default `target`, `node_modules`, `.dart_tool`) and their age (7 days).
- Safe-house providers may say what of a safe house's work exists only there and remove one
  whatever it holds: two optional `SafeHouseSession` slots, `exports` (`SafeHouseExports`:
  `unexported` change summaries, `dirty` and `ignored` paths; unsupported reads as all of it)
  and `discard`, wrapped by `SafeHouse::exports` and `SafeHouse::discard`. The `git` provider
  fetches each remote with `--prune` and lists commits since the base none of them holds,
  counts untracked files whatever the status configuration says, lists ignored paths, and
  reports submodules and Git LFS as unchecked work; it removes the worktree and its branch;
  the `copy` provider removes its copy and cannot tell what is exported. Neither ever removes
  work in place.
- Head of Station rounds: a Sleeper may set `stations` (narrowing its briefs, like a brief's
  `stations`) and `each_station = true`, which queues one brief per window for each Station
  whose last check-in placement accepts for the Operation and the sleeper's Agent or role,
  narrowed to that Station and keyed `sleeper--<operation>.<name>.<window>.<station>`, so a
  Headquarters failover never doubles a round; load, resources and quarantines do not exclude
  a Station. Both fields are part of the fingerprint when set (an unchanged definition keeps
  its approval). `SleeperStore::fire_sleeper` takes `(window, station)`. Each Station keeps a
  logbook of the newest 20 Head of Station reports (`LogbookStore`: `file_station_report`,
  `logbook`, on the file and SQL Registries; `StationReport`, `StationFinding`, `StationFindingKind`,
  `Severity`), and a round (`HEAD_OF_STATION_ROLE`) starts with that logbook as fenced,
  quoted context. The example
  roster defines the role and a commented `housekeeping` Operation.
- Moneypenny, the operator's assistant: the app chats with her over `GET /v1/user/moneypenny`
  and her model runs on a Station through the Agent's own handler (Claude Code or Codex,
  subscription or API key), as the roster Agent `moneypenny` (`--assistant-agent`,
  `--no-assistant`). Universal Exports relays each message to a Station
  (`/v1/station/assistant/next`, `/v1/station/assistant/reports`; `ai6::AssistantJob`,
  `AssistantReport`), which keeps one harness per conversation, warm for ten idle minutes
  and resumed afterwards. Handlers start her in `AgentStart::assistant` mode
  (`AssistantMode`: no built-in tool, only the named MCP servers; interpreters, skills and
  network beyond them are refused for now). Her tools are served on `/v1/moneypenny/mcp` to a
  token bound to the person and the conversation, and each runs through the gateway's own
  routes as that person, so a refusal there is a refusal for her; results are wrapped as
  data. `Gateway::with_assistant` and `AssistantConfig` are the library form. Her
  `propose_*` tools (Missions, Mission controls, proposals, held briefs, roster edits, Station
  settings, class authorisations) only record a proposal the person confirms or cancels in
  the app, and only when the person holds the permission; a confirm from the session it was
  proposed in is audited (`moneypenny.confirmed`) and then sent through the usual route.
  Her `show` tool displays views in a strict version 1 schema (text with in-app links only,
  tables, key-value lists, bar and line charts, Mission, Station and Theater cards, timelines,
  diffs, and forms that draft a proposal); an invalid view is refused with the reason. The
  Moneypenny app has an **Ask Moneypenny** chat panel on every page that renders them
  natively.
- Vetting: before M places a brief, the Headquarters holder runs deterministic checks under
  the Operation's `[operation.ID.vetting]` settings (`vetting = "off"` turns them off, a
  reported weakening). A malformed brief (no visible title, text past `max_brief_bytes`,
  default 64 KiB) is refused with outcome state `refused`; a duplicate of queued, held or
  running work (same URL or title), untrusted briefs past `max_per_hour` (20) or
  `max_per_source_per_hour` (10), the day's spend at `operation_cost_usd_per_day`, or
  untrusted text carrying Unicode tag characters outside an emoji flag, bidirectional
  overrides or unbalanced embeddings, runs of zero-width characters, variation selectors or
  fillers, invisible operators, HTML comments (`html_comments`), forged fence lines or a
  manipulation phrase (`phrases` extends the list; text is normalized first) is held.
  Untrusted means a Source item, a Mission proposal, or a brief without valid provenance (a
  registry drop). Held briefs leave the queue (`VettingStore`: `hold_brief`, `held_briefs`,
  `release_held`, `refuse_held`, `requeue_held`, `clearance`, `clear_brief`,
  `promote_cleared`, on the file and SQL Registries; at most 1,000 per Operation). A release
  clears the kinds it was held for, for the brief's content as released
  (`ai6::content_digest`): an edited brief is vetted again. Releasing or refusing again is a
  no-op, and a claimed or finished brief can be neither. A brief held only for a rate limit
  returns to the queue after `rate_hold_hours` (24). The timeline records `intake` events.
  `ai6::vet` and `VettingPolicy` are the library form.
- The Chief of Staff: when an Operation configures the `chief-of-staff` role, each untrusted
  brief that passes the vetting rules is reviewed by a short review Mission first
  (`vetting.review = "untrusted" | "all" | "off"`); with no qualified Agent available it is
  held (`review-unavailable`), never placed unreviewed. The review runs answer-only
  (`AgentStart::answer_only` and `AgentResume::answer_only`: Claude Code with `--restricted`,
  no tools and no MCP servers; Codex in its read-only sandbox, never asking, web search off)
  at `desk` clearance, on its Station's own host, in a new private directory removed when
  it ends, with no Cable tools, loadout secrets, pool account or project settings, and only
  the model credential of its Agent's environment. It is exposed from the start, carries
  `review` provenance (`Origin::Review`), quotes the brief and the Operation's other work as
  data, and answers with a JSON verdict (`ai6::read_review`): a single clear verdict lets M
  place the brief as reviewed (`ReviewTarget` binds the review to the brief's content), any
  hold holds it, and an unreadable or ambiguous answer, a failed review or one that ends
  without an answer holds it with `review-error`. The reviewer can only add holds. The
  example roster gives `Tanner` the role.
- Vetted Mission proposals: Headquarters vets each pending proposal (the rules but the hourly
  rate, then the Chief of Staff's review, at most `max_proposal_reviews_per_hour` (20) per
  Operation and `max_proposal_reviews_per_session_per_hour` (5) per proposing Session) and
  stores the result on it (`MissionProposal::vetting`, `BriefStore::set_proposal_vetting`);
  the inbox, the API and Moneypenny show it. A clean proposal from an unexposed Session that
  a live class authorisation of kind `proposal` covers (proposer Agent, or the parent
  Mission's recorded role, `MissionProposal::parent_role`; a requested role must be that
  role or one in `ClassAuthorization::proposed_roles`; title pattern; `max_uses`) is
  promoted by M once it is 30 seconds old and audited as `mission.proposal.approved` by
  `class:ID`; every other proposal waits for an operator. A promotion clears the promoted
  brief in the same Registry step; an operator's approval clears every finding but
  `malformed`.
- Held briefs for operators: Universal Exports lists, releases and refuses them
  (`/v1/user/operations/{id}/held-briefs`, `mission.create`; audited as
  `mission.brief.released` and `mission.brief.refused`), `ai6ctl held-briefs
  list|release|refuse`, and Moneypenny's Administration has a Held briefs tab that shows the
  findings and the brief's text only on request; both show invisible characters by code
  (`<U+200B>`, `<tags: ...>`). Administration appears for an account whose only
  administrative permission is `mission.create`.
- Live configuration: a Station checks its roster source and `loadouts.toml` every 5 seconds
  (`ai6 serve --config-poll`, `StationConfig::with_config_poll`, 0 off) and applies a change
  without a restart; running Sessions keep what they started with. A revision that fails
  the configuration checks is refused once, logged, audited as `config.refused` and
  reported at check-in while the last good one keeps running; a changed `loadouts.toml`
  declares the pool accounts again and withdraws the ones gone
  (`AccountStore::withdraw_account`). Changes to `theaters.toml` and `cables.toml` are
  reported as needing a restart. Each Station publishes a `ConfigStatus` (`config`) the
  board shows. `ConfigSource` has `revision`, `definitions`, `put` and `remove`;
  `RosterError::kind` tells an invalid configuration from an unavailable or read-only
  source; a configuration directory resolves every Agent when it loads
  (`Roster::check_agents`). Universal Exports serves roster administration from
  `--roster` / `AI6_EXPORTS_ROSTER` (`/v1/user/roster/...`, `roster.read` and
  `roster.manage` on the new scope `config:roster`; edits audited as `roster.update`,
  `roster.remove` or `roster.refused`), and Moneypenny's Administration has a Roster tab.
- Moneypenny's Station settings set the minimum free memory and maximum CPU percent past
  which a Station takes no new Missions.
- Brief provenance (bona fides): every brief carries where it came from (`Provenance`:
  `origin` `operator`, `source`, `sleeper`, `proposal` or `registry_drop`, the
  authenticated `principal` and `reference`, `received_by`, `received_at` and a
  `sha256:` digest of the whole record, `record_digest`), set where it enters and kept by
  the Registry apart from the record, so a brief file or row cannot claim one, a brief
  edited afterwards loses it, and a finished brief's provenance is spent so the same record
  queued again has none. A brief found in the queue with none is a `registry_drop`:
  accepted, its Sessions exposed (`brief:registry_drop`), and never reported to a Source; a
  Source is reported to only for its own item in its own Operation. The Mission record, the
  gateway's Mission detail and Moneypenny show the origin. `submit_brief` stores it,
  `ingest_source_item`, `fire_sleeper` and `promote_mission_proposal` take the receiving
  Station or gateway.
- Provider library pins and publishers (bona fides): a `plugins.toml` pin set
  (`PinSet`, `ai6 serve --plugin-pins`) accepts each provider library by its `sha256:` digest
  (`ai6 plugins pin PATH` prints the entry), by an Ed25519 publisher key whose signature of
  `ai6-provider-library-v1`, the file name and the library's SHA-256 ships beside it as
  `FILE.sig` (`publisher_message`), or by both. Under the `strict`
  profile, the default with a pin set, only accepted libraries load, each from a verified
  copy under the state directory, and a refusal names the expected and found digests or
  the publisher; `report`, the default without one, loads everything and lists each
  unaccepted library as a weakening; `off` checks nothing (`--plugin-profile`). Weakenings
  are printed at start, served by `/internal/plugins` and published in the check-in. A
  pin set named with `--plugin-pins` must exist. The field runner, `ai6 theater provision`
  and Universal Exports (`AI6_EXPORTS_PLUGIN_PINS`, `AI6_EXPORTS_PLUGIN_PROFILE`) check
  their provider libraries the same way.
- Harness project settings: a harness loads the configuration its working directory carries
  (settings, hooks, skills, subagents, commands, project `CLAUDE.md`, MCP servers, Codex's
  `.codex/` layer) only when the Operation allows it
  (`[operation.ID.harness] project_settings = "ignore" | "pinned" | "load"`, default
  `ignore`). Claude Code runs with `--setting-sources user --strict-mcp-config` and Codex
  with the working directory marked an untrusted project; AI6's own green ink hook and MCP
  servers apply either way. `pinned` loads it only while its manifest (every file under
  `.claude/` and `.codex/`, `.mcp.json`, and the scripts their hooks and MCP servers name,
  read without following links) has the Operation's `pin` (`ai6 harness pin DIR`), and
  otherwise starts as `ignore`; the manifest digest is recorded on the timeline either
  way. `load` and a SHA-1 pin are reported weakenings. `AgentStart` and `AgentResume`
  carry the decision (`project_settings`, `project_mcp`).

- Account pools: provider accounts shared by every Station and balanced by the quota they
  have left. `loadouts.toml` declares `[pool.<id>]` (handler, `operations` globs, strategy
  `headroom`, `round_robin` or `priority`) and its accounts (kind `claude-oauth-token`,
  `anthropic-api-key`, `anthropic-auth-token`, `openai-api-key`, `codex-access-token` or
  `config-dir`; the loadout secret or directory; `priority`, `max_concurrent`); an
  Operation's `pools` and an Agent's `pool` narrow which may be used. Each Session gets one
  account's credential and no other, claimed in one Registry write (`AccountStore`, on the
  file and SQL Registries); Codex accounts are logged in to a per-Mission `CODEX_HOME`.
  Claude Code `rate_limit_event`s and Codex `account/rateLimits/updated` notifications are
  kept per account (`AgentEvent::quota`), as are the tokens used each day. `on_limit`
  takes `rotate`, the default: a limited account is skipped until its reset, and the
  Session moves to another account (the same Claude Session, or a new Codex attempt with
  a hand-off). Intake keeps a brief queued while no account of its pools has headroom.
  Universal Exports serves the pool board (`GET /v1/user/operations/{id}/pools`,
  `pool.read`; other Operations' Sessions only as a count), shown by `ai6ctl pools` and
  Moneypenny's Account pools view; `pool.manage` lifts a limit (`ai6ctl pools clear-limit`,
  audited). Limits last at most eight days (`StationConfig::with_account_limit_horizon`). A
  green ink secret cannot back a pool account, and a pooled Agent's loadout cannot set a
  provider credential variable.

- A SQL Registry and roster, so the Stations of an HA deployment need no shared
  filesystem. `SqlRegistry` (features `sqlite` and `postgres`) keeps every Registry record
  family in SQLite or PostgreSQL, one table each, with the file Registry's guarantees:
  every write takes the Registry's clock row first, so claims, leases, the Headquarters
  term and Session fencing behave the same. `SqlConfigSource` keeps the roster as one row
  per Operation, Agent, role and cover (the keys of its TOML table as JSON), built with
  `Roster::from_parts`; `put` and `remove` refuse a change that leaves the roster
  invalid, and its revision says what changed. Tables are created on first use and
  versioned. `ai6 serve --registry LOCATION` and `--config LOCATION` take a directory,
  `sqlite:PATH` or a `postgres://` URL, as does `AI6_REGISTRY` for the `ai6` command and
  Universal Exports (`registry::backend::{open, open_with, from_environment}`,
  `config::{open, open_with}`). PostgreSQL connections use TLS and verify the server by
  default (`sslrootcert=` for a private CA; `sslmode=require` or `disable` are accepted
  and printed as weakenings), take the password from `password_file=`,
  `AI6_REGISTRY_PASSWORD` or `AI6_CONFIG_PASSWORD`, and get statement, lock, idle and
  connection timeouts derived from the Headquarters lease (`SqlOptions`), so a Station
  that stalls inside a write cannot hold off a takeover. Leases use the database's clock;
  writes run `READ COMMITTED` and reads `REPEATABLE READ READ ONLY`; a Station that a newer
  `ai6` migrated past stops writing.
- `ai6 serve --headquarters-lease SECS` (`StationConfig::with_headquarters_lease`) sets how
  long the Headquarters lease lasts between renewals, and so how long a standby control
  Station waits before taking over (default 60 seconds).

- Load balancing. Stations publish their host's CPUs, CPU percent, load and memory at every
  check-in (`resources`, sysinfo), and each Theater reports its own from the probe: `nproc`,
  the load average and `/proc/meminfo`, narrowed by a cgroup v2 container's `cpu.max` and
  `memory.max`, or `sysctl` on macOS, sampled every 60 seconds between full probes
  (`ai6 serve --resources-every`). Placement rules out a Station or Theater with less free
  memory than `min_free_memory_mb` (default 512) or a CPU busier than `max_cpu_percent`
  (default 95), both Station settings in the Registry
  (`PUT /v1/user/stations/{id}/settings`), and scores the rest on Missions held against
  capacity, CPU, memory and Sessions on the Theater; an Operation's `placement = "spread"`
  (default) or `"pack"` orders them. The board, placement previews, Moneypenny and
  `ai6ctl stations` show the figures, their sample time and each choice's score
  (`ai6::Resources`, `ResourceLimits`, `PlacementStrategy`, `probe_resources`).
- Recovery when a runner dies while its Station lives. A Station that cannot reach a
  Session's runner, or the harness the detached wrapper runs, for the Operation's
  `lost_after_secs` (default 45, `directorates::m::recovery::DEFAULT_LOST_AFTER_SECS`)
  probes it once more through the Theater's transport; if the runner is gone, no longer has
  the Session, or the Theater cannot be reached, the Session is marked `lost` with the
  reason and the brief is handed back, to be tried again or filed under `on_lost` like a
  Session lost with its Station. A runner that restarts ends the Sessions it lost with a
  `lost` event, which the Station handles the same way. A Station re-attaching after a
  restart or an adoption backs off between tries (`reattach_delay`: 5 s doubling to a
  minute), and gives the Session up once its
  tries have failed for `lost_after_secs` and a probe fails. A retry on the same Theater
  ends the lost runner Session first, and a retry working where the lost attempt did is
  told so in its hand-off.
- Recovery when a Station goes dark. Once its brief lease and its Session's lease lapse, the
  Headquarters Station hands the brief to a live Station that reaches the same Theater's
  field runner, which takes the Session lease (fenced) and re-attaches, replaying the events
  the dark Station never took: nothing is launched twice. A Session that ran on the dark
  Station's own host, or without a runner, is marked `lost` (`ai6::SessionStatus::Lost`,
  `FileRegistry::lose_session`) and its brief is queued again: the next attempt, on another
  eligible Station, links to it as its predecessor and is briefed on why it was lost and
  what it was last seen doing. `on_lost = "fail"` files the brief instead. Session records
  carry the harness's launch (`SessionLaunch`), its recent events and its last checkpoint;
  `FileRegistry::hand_over` gives a lapsed brief lease to another Station, and
  `directorates::m::recovery::adopter` chooses it.
- Safe-house checkpoints. With `workspace.checkpoint_remote` set, a Station saves each
  running Mission's safe house every `checkpoint_secs` (default 300) as a commit holding
  its uncommitted work, under the Operation's cover, pushed to the Mission's branch without
  touching the Agent's worktree; a later attempt's safe house starts from it. The safe-house
  kind has the optional `checkpoint` slot and `SafeHouseSpec.checkpoint`
  (`CheckpointSpec`, `SafeHouseCheckpoint`), which the `git` provider implements.
- Attempt history: `GET /v1/missions` and the Mission detail list each attempt
  (`universal_exports::MissionAttempt`: Station, Theater, status, why it was lost and its
  checkpoint branch), and Moneypenny shows them on the Mission card.

- Theater provisioning. A Theater declares what Sessions need there
  (`[theater.ID.requires]`: `os`, `harnesses` such as `"claude>=2.1"`, `tools`, `disk_mb`)
  and how to install what is missing (`[theater.ID.provision]`: an ai6 build for its
  runner, provider libraries, an install command per program). The Station probes each
  POSIX Theater over its transport every five minutes and publishes the report at check-in
  (`TheaterSummary::requires`, `report`; `BoardTheater` likewise): placement rules out a
  Theater whose requirements are unchecked or unmet, and in field mode an Agent whose
  harness is missing there, each with the reason; Moneypenny marks such a Theater and says
  what is missing. Credentials already on a Theater outside AI6's leases (a git credential
  store or helper, an SSH agent or key, a CLI login, a provider key in the login
  environment) are reported as weakenings, by location only. `ai6 theater provision
  THEATER [--check]` fits a Theater out over its transport, checking before each step:
  `ok`, `changed` or `failed` per step, so a second run changes nothing, and `--check`
  writes nothing (`ai6::directorates::q::provision`).
- Credentials as Q leases. A Station reads `loadouts.toml` and, at every Session start and
  resume, leases the Agent's loadout secrets from its secret backend and hands each value to
  the harness in its environment only (`[secret.NAME]`: `env`, `from = { file | env }` on
  the Station, `green_ink`, `kinds`). The Station's own backend (`StationSecrets`) reads a
  file (`secrets/NAME` in the configuration directory by default) or, logged as a weakening,
  the Station's environment; `StationConfig::with_secret_backend` replaces it. A
  `green_ink` secret is leased only while the Mission holds a live green ink approval of a
  kind it names, once per approval and no longer than it lasts. Leases are revoked when the
  Mission ends or its Session stops; each issue is a `credential` timeline event naming the
  secrets, never a value. `SecretBroker::lease_names` leases part of a loadout.
- Ephemeral Theaters. `access = { ephemeral = { provider, config, keep_failed,
  keep_failed_hours } }` makes a Theater a template: each Mission bound to it gets a fresh
  Theater from a Theater provider (the new kind `ai6_theater_provider`,
  `ai6::directorates::q::ephemeral`), fitted out as the template's `requires` and
  `provision` say, and destroyed once the Mission is done or abandoned; a failed Mission's
  Theater is kept for inspection within the template's bounds, and a restarted Station
  adopts what is left. The new `ai6-theater-container` crate is the `ephemeral-container`
  provider: one Docker or Podman container per Mission, reached by the `container`
  transport. Check-ins publish a Theater's `made_for` Mission, and placement never chooses
  it for another.
- Green ink: a person's recorded approval of one irreversible step. An Agent calls the
  `request_approval` tool on its Session's AI6 endpoint with the step's kind (`push`,
  `merge`, `release`, `delete`, `clearance` or one its Operation adds) and target. The
  Operation's `[operation.ID.green_ink]` policy (preset `strict` by default, or `licensed`,
  where `00` pushes and merges unattended; every key overridable) answers `not_required`,
  a live class authorisation answers `approved`, and otherwise the request is `pending`:
  the Mission is `blocked` with reason `green-ink`, its Watchdog paused, until an operator
  with the new `approval.decide` permission approves it for a chosen time or rejects it
  (`GET /v1/user/operations/{id}/green-ink`, `POST .../{request}/approve|reject`). The
  Station tells the Agent the decision and the Mission works again; a request nobody
  decides expires as refused after 24 hours. Class authorisations (`approval.class`,
  `/v1/user/operations/{id}/class-authorizations`) approve a kind of step on a target glob
  for named Agents or roles, up to a use limit, and are revoked there. Records live in the
  Registry (`ai6::ApprovalRequest`, `Approval`, `ClassAuthorization`); decisions are audited
  and requests appear on the timeline. Moneypenny has Green ink and Class authorisations
  tabs under Administration.
- The green ink harness gate. The Claude handler passes Claude Code a `PreToolUse` hook on
  `Bash|PowerShell` through `--settings` (`onFailure = "block"`, Claude Code 2.1.295 or
  later) that runs `ai6 green-ink gate`; the Codex handler answers
  `item/commandExecution/requestApproval` with the same gate (`accept`, or `decline` plus a
  `turn/steer` naming what to ask for). The gate names the governed steps of a shell command
  (`ai6::classify`: `git push` refs, `git push --delete`, `gh pr merge`, `gh release
  create`, also inside `sh -c` and friends) and asks the Station through the Session's
  Cable endpoint (new MCP tool `check_approval`) whether a live approval covers each one;
  otherwise the command is refused with the `kind` and `target` to pass to
  `request_approval`. `ai6::directorates::section_00::gate` (feature `gate`) holds the
  client. `ActionScope::covers` matches an approval to a step however its ref was written,
  and `ApprovalStore::live_approval` uses it. A push of every branch, tag or ref
  (`refs/heads/*`) counts as protected when it can reach a protected ref.
- `ai6ctl green-ink list|approve|reject|revoke` and `ai6ctl class-authorizations
  list|revoke`, and `POST /v1/user/operations/{id}/green-ink/{request}/revoke`, which
  withdraws an approval (`approval.decide`). Green ink is audited end to end:
  `green_ink.requested` and `green_ink.expired` by the Station, `green_ink.revoked` by the
  gateway, beside the existing decisions and class authorisation records; every target
  names the request and its step (`REQUEST_ID KIND TARGET`), never the Agent's reason.
- The green ink gate reads more of the shell and holds what it cannot read: shell keywords
  (`if`, `do`, `{`, `!`), wrappers with their options (`timeout`, `nice`, `ionice`, `stdbuf`,
  `env -C`/`-S`, `sudo -u`/`-D`, `setsid`, `xargs`), `cd` and `git -C`/`--git-dir`/`GIT_DIR`
  (refs are resolved in the repository the command acts on), git aliases, `heads/`/`tags/`
  destinations, the repository's push configuration (`remote.NAME.push`, `push.default`,
  `push.followTags`, the branch's push remote, `-c` overrides), `--tags` beside refspecs,
  `--no-dry-run`, and `gh api` merges (`ai6::classify` takes an
  `ai6::directorates::m::approvals::Repository`). Pushes always name their remote, merges
  name `-R OWNER/REPO`, and any destination with a pattern character is protected. A `git`
  or `gh` it cannot place, an unknown ref, `xargs` arguments and `gh api` calls that move refs
  or releases are held. The Station gates every remote-hands `theater_run` command itself; a
  gated Codex thread runs with `approvalPolicy = "untrusted"` and input to a running command
  is declined; the Claude hook carries the Cable URL in its own arguments; `ai6 green-ink
  gate` denies input it cannot read; a start or resume request cannot set
  `AI6_GREEN_INK_GATE`, and a roster value is logged as a weakening.
- Green ink secrets take `targets` (globs on the approval's target), and an approval unlocks
  one lease on any Station, ever (`Approval::spent_at`, `ApprovalStore::spend_approval`). A
  green ink secret without `targets`, or served from the Station's own files or environment,
  is logged as a weakening. Revoking an approval is audited before it is applied.
- A `green_ink` credential's lease is revoked at the secret backend as soon as its approval
  expires or is revoked, and any lease past its lifetime is revoked at the next scheduler
  pass (`credential` / `revoked` timeline event).
- The DipTel lifecycle frame carries an optional `reason` (`green-ink`, `budget`), shown on
  the board beside the Session's state.

- Operator controls. With the new `mission.control` permission an operator cancels,
  retries, resumes or interrupts a Mission through
  `POST /v1/user/operations/{operation_id}/missions/{mission_id}/actions` (idempotent by
  key, audited as `mission.cancel`, `mission.retry`, `mission.resume`,
  `session.interrupt`). Universal Exports queues a typed DipTel control
  (`ai6_diptel::MissionControl`) for the Station holding the Mission, which takes it from
  `/v1/station/controls/next`, checks the Mission, its Operation and its fenced Session
  lease, applies it and reports the outcome, read back from the control's status route.
  A retry queues the Mission as a new attempt, optionally for another qualified Agent
  (`MissionStore::retry`). `ai6ctl mission cancel|retry|resume` and `ai6ctl session
  interrupt` wait for the outcome; Moneypenny offers the controls in each Mission card's
  menu with the reason any is unavailable. DipTel has the `queued` and `abandoned`
  states, and board Sessions carry `updated_at`, so a client finds a Mission's current
  Session.
- An Operation activity timeline. `GET /v1/user/operations/{operation_id}/timeline`
  (`audit.read`) pages newest first through lifecycle transitions, control outcomes,
  Cables sent and Sleeper authorizations from a new Registry activity log
  (`ai6::TimelineEvent`, the newest 10,000 kept) merged with the Operation's audit records,
  correlated by Station, Mission and Session, filterable by Mission and continued by an
  opaque cursor. Items are metadata only. Moneypenny shows it in a Timeline view.

- A Station's identity defaults to `<host>-<random>`, created on first start and kept in
  `station-id` in the AI6 state directory, so a restarted daemon stays the same Station;
  `--station` or `AI6_STATION_ID` override it.
- Theaters: a Station reads `theaters.toml` (`[theater.<id>]` with `access = "local"` or an
  `ssh` host, plus labels), always reaches its own host as the implicit `local` Theater and
  publishes its Theaters at check-in. A Session's launch record carries `station_id` and
  `theater_id` separately. `GET /v1/stations` lists each
  Station's Theaters, and Moneypenny shows them and the Station and Theater of each Session.
- Transports. A Station reaches a Theater through a transport, a new provider kind
  (`ai6_transport`, `ai6::directorates::q::transport`) with optional, capability-gated
  operations: open a terminal, run a command, read, write and list files, and forward a port
  from the host back to the Station. Each transport declares its `Capabilities`
  (`terminal`, `run`, `files`, `forward`, `os`, `arch`); a contract suite beside the kind
  checks any transport against what it declares. Theaters declare `agents` (`auto`, `local` or
  `remote-hands`), `egress` (`direct`, a proxy, `station` or `none`), `harnesses`, `workdir`
  and `access = { transport = { provider, config } }` for any other transport provider.
  Stations publish each Theater's capabilities and declared facts, and `GET /v1/stations`
  and Moneypenny's Theater tooltips show them; a Theater that cannot be reached is shown
  with the reason and ruled out of placement.
- Five transport providers. `ai6-transport-local` runs commands and reads and writes
  files on the Station's own host. `ai6-transport-adb` reaches an Android device through
  `adb` (commands, files built from shell commands, `adb reverse` forwards),
  `ai6-transport-container` a running Docker or Podman container through `exec`, and
  `ai6-transport-qemu` a QEMU guest through its guest agent; the shell-based file operations
  they share are in `ai6::directorates::q::transport::kit`. `ai6-transport-ssh` reaches a host over SSH with a
  built-in pure-Rust client (`russh`, `ring`), so it needs no installed `ssh` and behaves the
  same on every OS: commands, pty terminals, SFTP files and reverse forwards, reconnecting
  on its own. The host key is pinned (`host_key = "sha256:HEX"`); first contact without a pin
  is refused, naming the key to pin, unless `trust_on_first_use = true`, which the Station
  logs and `GET /v1/stations` reports as a weakening. Keys stay in files: only a key path is
  configured, and a passphrase-protected key is refused.
- Remote-hands mode. Placement chooses field mode (the harness runs on the Theater) or
  remote-hands mode (the harness runs on the Station and acts on the Theater through its
  transport) from the Theater's capabilities and its `agents` and `egress` settings, and
  every rejection names the missing capability. In remote-hands mode the harness's own shell
  and file tools are switched off (Claude Code `--disallowedTools`; Codex runs in a read-only
  sandbox that asks nothing) and four Theater tools (`theater_run`, `theater_read`,
  `theater_write`, `theater_list`) on a per-Session authenticated MCP endpoint act on the
  Theater instead. Placement choices, launch records, the published Mission brief and
  Moneypenny carry the mode.
- The field runner, `ai6 runner serve`: the AI6 binary's Theater mode, which supervises
  harnesses on the Theater so a Session outlives its Station. A Theater declares it with
  `runner = {}` (or `command`, `state_dir`, `plugin_paths`, `terminal`), and the Station
  starts it when none answers. Every event a harness emits is spooled to disk with a sequence
  number before a Station reads it; a Station that is shut down, killed or fenced out leaves
  the harness running and its Mission Working, and the Station that comes back re-attaches
  and replays every event it missed from the last one it acknowledged. Each Session gets a
  Cable relay with its own URL and token that the attached Station repoints. The runner
  listens on loopback behind a token in its state directory; a Station on another host
  reaches it with `ai6 runner call` over the Theater's transport. Placement offers field
  mode on a Theater with a runner, on the Station's own host or another one.
- The detached wrapper: on a POSIX Theater with a declared harness and no runner (an SSH
  host where nothing is installed), the Station starts the harness over the transport under
  `setsid`, with its output spooled to a file, so a dropped link costs only time and the
  Station reads on from the last byte it took once the transport reconnects. Its Cable
  endpoint reaches the harness through a reverse forward.
- Placement is M's decision (`ai6::directorates::m::placement`). It filters by Station role, health, capacity, labels, checkout, Theater
  and available handler, orders by load then Agent priority, and gives a reason for every
  rejection. An Operation may list the Stations allowed to run it (`stations = [..]`); a
  Mission's brief may narrow the list but never widen it. Stations also publish their
  handlers, Theaters and each Operation's placement policy at check-in.
- Universal Exports `GET /v1/user/operations/{id}/placement-preview` (needs `mission.read`)
  returns the eligible Station, Theater and Agent choices and the reason for each rejection,
  and Moneypenny's Stations view has a placement preview panel.
- Recovery. Every Session has a record in the shared Registry (`sessions/`): AI6 Session
  ID, provider Session ID, Station, Theater, attempt and predecessor, status. A Station acts
  on a Session only while it holds the record's lease and presents its fencing token, which
  is drawn from the Headquarters election term (`HeadquartersLease::term`) and always
  grows; a stale token is refused and the Station closes its harness and abandons the
  Mission. Moving a Session to another Station or Theater is not implemented.
- Provider usage limits. The Claude and Codex handlers report a usage or rate limit, with
  its reset time when given, as an `AgentEvent` of kind `limit` (`AgentEvent::limit`,
  `UsageLimit`, `LIMIT_EVENT`). The Mission becomes `parked` with `parked_until` and the same
  provider Session resumes when the quota resets (also across a daemon restart). An
  Operation sets `on_limit = "rotate"` (default), `"wait"` or `"backup"` (post the next qualified Agent
  on another handler as a new attempt in the same safe house) and `limit_fallback_secs`
  (default 1800, used when no reset time is reported). `parked` is a Session state in
  `ai6-diptel`, the gateway and Moneypenny.

- Medical: usage and budgets. The Claude and Codex handlers report tokens and cost from the
  provider's own events (`AgentEvent::usage`, `TokenUsage`); the Station keeps a per-Mission
  ledger in the shared Registry (`usage/`), returned as `usage` by the Mission detail of
  Universal Exports. `[operation.ID.budget]` and `[agent.ID.budget]` set `session_tokens`,
  `mission_tokens`, `mission_cost_usd`, `operation_cost_usd_per_day` and `warn_at`; nothing is
  limited unless set or `preset = "strict"` is chosen. Crossing `warn_at` emits a `budget`
  event and an audit record; reaching a limit blocks the Mission with `blocked_reason`
  `budget` (resumable once the limit is raised). A Mission cost limit also becomes Claude
  Code's `--max-budget-usd`.
- Medical: exposure. A Session is marked when its Mission came from a Source item, it calls a
  tool the Operation lists (`WebFetch` and `WebSearch` by default), or it reads a Cable from
  another Operation or an exposed Session. The mark is kept in the Session record, inherited
  by later attempts, shown on the board (`exposure`) and as an `EXPOSED` chip in Moneypenny.
  `[operation.ID.exposure]` picks `strict` (default: an exposed Session sends Cables only as
  replies, and its Mission proposals carry its exposure) or `observe`, and can choose
  restrictions and tools individually.
- Medical: quarantine. An Agent that stalls or loops three times, hits a budget stop twice, or
  blows its cover (`[operation.ID.quarantine]`, 24 hours, each trigger can be set to 0) is
  quarantined in the Registry: its Sessions stop on every Station, its Missions become
  `quarantined` (safe houses kept), its Cables are held and the roster skips it. An operator
  quarantines or releases with `ai6 quarantine list|add|release` or the Universal Exports
  `quarantine` routes (`agent.configure`); a released Mission returns to the queue with the
  next qualified Agent. `quarantined` is a Session state in `ai6-diptel`, the gateway,
  `ai6ctl` and Moneypenny, and `ai6ctl` lists a Session's exposure.

- `ai6`: the Station daemon and AI6's public contracts in one Rust crate. Directorates
  under `ai6::directorates` (M, the Registry, Q Branch, 00 Section, Medical) and the Station
  runtime under `ai6::station` (server, provider host, safe houses, publisher, identity).
  The `ai6` command provides `serve`, `identity` and `peer waiting|accept`.
- Station daemon: a private loopback API for providers and the Mission lifecycle;
  Registry-leased intake with lease renewal and recovery; Mission records persisted in
  `missions.json` and reattached to provider Sessions after a restart; Station check-ins
  with roles (`control`, `execution`), labels and capacity; Headquarters Station election.
- Agents and roles: an Agent (`[agent.<codename>]`) owns its handler, model and effort and
  lists the roles it is qualified for; a role (`[role.<id>]`) is a purpose and a policy.
  M posts the highest-priority qualified Agent whose handler is available, so the next
  qualified Agent is the backup. Operators can override roles per Operation.
- The Station picks the terminal provider automatically, preferring `pty` when several
  are available; `ai6 serve --terminal ID` pins one. Agents do not choose a terminal.
- Safe houses: an isolated workspace per Mission, removed when untouched and kept with
  a report when it holds work. A safe house is made by a provider of the new
  `ai6_safehouse` Guatiao kind (`ai6::directorates::q::safehouse`); `ai6-safehouse-git`
  is the `git` provider (a worktree on its own branch). An Operation chooses one with
  `workspace = { provider, source, base, branch, isolate }`; the provider defaults to `git`
  for a Git checkout. The daemon runs no Git commands itself. `ai6-safehouse-copy` is the
  `copy` provider: a plain directory copy with no history.
- Cover checks read the safe house's changes through the provider. A provider that keeps
  no history makes the cover report say `checkable = false` instead of passing.
- Cover policies (`covert`, `partial`, `overt`, or `covers.toml`) with an Operation
  author identity; Mission commits are audited and a violation marks the Mission `blown`
  when the Operation sets `on_blown = "fail"`.
- Session watchdog: stall, repeated-output loop and blocked-prompt detection, with daemon
  options and per-Agent `[agent.<id>.limits]` overrides.
- Cables: a permission-scoped agent-to-agent relay (Agent unicast; Station, Theater,
  Global and named-channel broadcasts) with deliver/hold/refuse receiver policies, quotas,
  metadata-only audit, and per-Session MCP tools (`cables-mcp` feature).
- 00 coordination: a `00`-clearance Session can propose child Missions, which wait until
  an operator approves them.
- Q Branch: bounded loadout catalogs, Mission-scoped secret leases, Claude MCP
  configuration and bubblewrap argument translation.
- Station identities (`station-tls` feature): persistent certificates, SHA-256
  fingerprints, pinned-peer or CA mutual TLS and a waiting/accepted peer store, on the shared
  `coqui-peer-tls` and `coqui-peer` crates. The strict policy is the default (TLS 1.3, SHA-2
  pins); the waiting list is bounded per source address, in total and by re-record time, a peer
  is listed only after it proved its key, and `ai6 peer waiting` shows each source address.
- Access kinds: Authenticator, Directory, Authorizer and Accounts as Guatiao kinds from the shared
  `coqui-access` crate, the first-match `RoleAuthorizer` from `coqui-roles`, and the providers
  `coqui_access_local`, `_key`, `_ldap`, `_proxy`, `_peer` and `coqui_roles`, loaded as plugin
  libraries from `--plugin-path`.
- Universal Exports limits login tries: 5 a minute per client address and 10 in ten minutes per
  account name by default, refused with `429` and `Retry-After` before any Authenticator is
  asked. `--login-tries-per-address`, `--login-tries-per-name` and `--client-address-header`
  (or `AI6_EXPORTS_LOGIN_TRIES_PER_ADDRESS`, `_PER_NAME`, `AI6_EXPORTS_CLIENT_ADDRESS_HEADER`)
  change them; `LoginLimits` and `GatewayConfig::with_login_limits` do the same in code.
- Agent handlers `ai6-agent-claude` (Claude Code stream-json) and `ai6-agent-codex` (Codex
  app-server), and the `ai6-terminal-pty` terminal provider (openpty on Unix, ConPTY with
  Job Objects on Windows).
- `universal-exports`: the authenticated public API gateway: login through an
  Authenticator or a trusted reverse-proxy identity, revocable user tokens shared through
  the Registry, per-Operation authorization, Station, Mission and Session views, Mission
  creation and proposal review, role administration, Station capacity settings,
  metadata-only audit, attach grants with read-first WebSocket terminals and expiring
  write leases. Configured with `AI6_EXPORTS_*` variables.
- Sources: the `Source` Guatiao kind (`ai6_source`) polls a tracker or CI system for work
  and posts each Mission's outcome (the CX report) back. The Station holding the
  Headquarters lease polls each `sources.<provider>` of every Operation on an interval,
  keeps a cursor per Operation and provider in the Registry, and queues items as briefs
  keyed `source_<provider>--<item>`: a changed item updates its queued brief, and a claimed
  or finished one is never queued again. Tracker text is fenced in the brief as untrusted
  data. Reports are worded by the Operation's cover policy.
- Sleepers: standing orders under `operation.<id>.sleepers.<name>` (a five-field UTC cron
  `schedule`, a `title` and a `prompt`) that queue a brief per scheduled window. A sleeper
  fires only after `ai6 sleeper authorize <operation> <name>` approves its exact definition
  (editing it withdraws the approval), and the brief's key is derived from the window, so
  a Headquarters failover cannot queue it twice. `ai6 sleeper list|authorize|revoke`.
- Source providers `ai6-source-github` (labeled issues and pull requests, comment and
  label back) and `ai6-source-jira` (JQL on Cloud or Server/Data Center, comment and
  workflow transition back).
- `ai6-diptel`: the native and WebAssembly wire contract for Session lifecycle, terminal
  bytes and resize frames, with test vectors.
- `ai6ctl`: a read-only operator command for health, Stations, Agent roles, Missions and
  Sessions.
- Moneypenny: the Flutter desktop and web operator app, with Stations and the
  Headquarters Station, the Operation → Mission → Session board, Mission briefs, live
  terminals, administration, audit and an operator menu.
- `examples/config`, the documentation site with the Rust API reference, public API
  snapshots, and test, docs and release workflows.

- The Station's scheduler makes its Registry calls (check-in, lease renewals, election,
  intake) on Tokio's blocking pool, so a slow Registry does not stall the async workers.
- The Registry's operations are traits, one per record family (`BriefStore`,
  `StationStore`, `AccessTokenStore`, `CableStore`, `AuditStore`, `AgentRoleStore`,
  `ApprovalStore`, `QuarantineStore`, `SessionStore`, `SleeperStore`, `SourceStore`,
  `TimelineStore`, `UsageStore`), together `RegistryBackend`; `FileRegistry` implements
  them, and calling them on it needs `use ai6::directorates::registry::backend::*`. The
  Station, its Cable relay and `universal_exports::Gateway::{with_registry, with_access}`
  take `Arc<dyn RegistryBackend>`; `ServeConfig::with_registry` sets the Station's.
  `submit_brief`, `submit_mission_proposal` and `record_audit` return `()`, and `finish`
  returns whether it filed the outcome.
- A Station's roster comes from a `ConfigSource` (`load`, `changes_since` with a
  `ConfigRevision`): `TomlConfigDir` (the configuration directory, the default) or
  `MemoryConfigSource`, chosen with `ServeConfig::with_config_source`.
  `Roster::from_parts` and `Roster::builder` build a roster from typed records, with
  `Operation::new`, `Operation::validate`, `Agent::new`, `AgentRole::new`, `Cover::new`
  and its `with_*` settings, and `Covers::insert`. Projects are configured as Operations in
  `operations.toml`, Agents and roles in `agents.toml`.
- The detached wrapper passes a harness's environment through a FIFO, so nothing it
  carries is written on the Theater.
- The `daemon` feature brings `ureq`, `subtle` and `futures-util` (the runner's client,
  token checks and Cable relay); `publisher` adds only `url` and `ai6-diptel` on top of it.
- `AgentStart`, `AgentResume`, `AgentMessage`, `AgentEvent`, `UsageLimit` and `TokenUsage`
  are serde `Serialize` and `Deserialize`.
- A provider whose settings all have defaults is built with them when no configuration is
  given, so the `claude` and `codex` handlers load without one.
- The `ssh` transport has the host listen again for its open reverse forwards, on the same
  ports, when it reconnects.
- Access details: `Attempt::headers` is an ordered `Vec<(String, String)>`;
  `ai6::role_authorizer_from_toml` and `load_role_authorizer` read `access.toml`; the
  `coqui_roles` provider takes `scope_kinds`, `roles` and `rules`. `coqui_access_local`,
  `coqui_access_key` and `coqui_access_peer` are configured with
  `{"source": {"file": {"path": P}}}` over an `[[accounts]]`, `[[keys]]` or `[[peers]]` file;
  a key's `hash` is labelled (`sha256:<hex>`), the key `prefix` is configurable, and a peer's
  `certificate` may be in any fingerprint spelling.
- Station TLS details: `ai6::station::identity` re-exports `coqui-peer-tls`'s `Identity`,
  `PeerId`, `TrustStore` and `Trust` (none at the crate root); `client_config` and
  `server_config` take a `Policy` and return `Built<..>`. Handshakes are TLS 1.3 only unless
  the policy allows 1.2. Peer ids print as `sha256:<hex>` and are read in any spelling.
- Harnesses start on a Windows Station: the `pty` provider answers ConPTY's cursor position
  query (`ESC[6n`) and removes it from the output, on Windows only, from a thread of its own.
  The Claude Code and Codex handlers open their terminal 32767 columns wide and read it with
  `ai6::directorates::q::terminal::JsonLines`, which removes ConPTY's mode and title
  sequences, bounds OSC and CSI parsing, and rejoins a line ConPTY broke at the terminal's
  width.
- Codex answer-only and assistant threads start only after a tool check. Every enabled Codex
  feature but `ai6_agent_codex::KEPT_FEATURES` is switched off, as are agents
  (`agents.enabled`) and the skills catalog. `codex debug prompt-input` is rendered on every
  start and must show exactly the thread's MCP tools, else the thread is refused, as is output
  it cannot read exactly. The feature list is reused, keyed by the executable, its version,
  the settings, the mode, `CODEX_HOME`, the home directories and `PATH`.
  `AI6_CODEX_TOOL_CHECK=warn` in the Agent's environment starts the thread with a `weakened`
  event. Codex 0.164 offers MCP tools only inside `functions.exec`, so Moneypenny is refused
  on it.
- A restricted Session (answer-only, assistant) that uses a tool outside its MCP servers is
  ended with a `failed` event naming the tool, in both handlers.
- The Codex handler declines a green ink approval read from a line the terminal rebuilt
  (`StreamLine::Json::rebuilt`) or with no earlier `item/started` for the same item and
  command, and records why as a `declined` event.
- A review refuses to start when the Operation's own `.ai6/agents.toml` sets `PATH`,
  `CODEX_HOME`, a `*_BASE_URL` or `AI6_CODEX_TOOL_CHECK` differently from the global roster.
- `AgentResume` carries `initial_context` (the Agent's context, its role's purpose and policy,
  Moneypenny's instructions), passed to Claude Code again as `--append-system-prompt` for a
  Session whose recorded prompt was compacted away. `AssistantJob` carries the conversation
  recap apart (`recap`), and a resume does not get it. Moneypenny's harness works in the same
  directory each time her conversation resumes on a Station, since Claude Code finds a Session
  only from the directory that recorded it.
- Codex answer-only (Chief of Staff) and assistant threads run with command execution,
  local image attachment, web search, sub-agents and app connectors switched off
  (`features.shell_tool`, `features.unified_exec`, `tools.view_image`, `web_search`,
  `features.multi_agent`, `features.apps`) as well as in the read-only sandbox, which alone
  would let Codex's shell read anything the Station's user can.
- Moneypenny's tool token is bound to the Station that runs her harness and reused for later
  messages to it, and a harness left on a Station the conversation moved away from loses its
  tools at once (its Station closes it when its accept is refused). A person holds at most
  two conversations with a harness or an answer pending
  (`with_live_conversations_per_person`); views are at most 64 KiB, conversations and people
  have byte budgets (`with_memory_limits`) and `ready` sends the newest 100 entries; an
  accepted answer silent for 10 minutes is given up (`with_answer_deadline`). Proposals
  refuse hidden characters, and the app shows any in a request as `<U+XXXX>`.
- The Headquarters Station places a brief on the facts of the Station that would run it: a
  brief naming no Agent gets one chosen per Station from that Station's handlers, and a
  Station's published capacity and running Sessions count, not the Headquarters Station's
  own.

[Unreleased]: https://github.com/jose-pr/ai6/commits/main
