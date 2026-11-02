# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `ca_file` on a GitHub or Jira Source or tracker instance: a PEM file of certificate authorities
  added to the public roots, by absolute path, for a site under a company's own authority. A bad file
  is refused naming its path, and a set one is reported as a weakening at start and at check-in.
- `ai6 config public [--output PATH]` writes the public `ai6.cfg` a web page reads beside
  `index.html`: `[unex] url` and what the page needs of `[oo]`, from an allow list, so a new key is
  private until someone says otherwise. Text under `[oo]` other than `[oo.appearance] theme` and
  `[oo.assets] cdn` is left out and named on standard error, and a `url` holding a credential, a query
  or a fragment is refused.

### Changed

- **A Cable no longer crosses an Operation on a Station grant alone.** A recipient in another Operation
  needs a compartment giving the sender `cable.send` there, as well as the grants; `cross_operation =
  "station_grants"` in the `cables` record keeps the grants alone, logged and reported at check-in as
  `cables_weakened`. `discover_agents` follows the same line, and `CableRelay` keeps no such log by
  default: `with_person_audit` is now `with_durable_audit`, and without one nothing crosses.
- `send_cable` takes scope `operation`, as its description said it did.
- An audit record's `resource` may also name `operation:`, `mission:` or `compartment:`.

- `tools/sdk/check_lock.py` fails when `pubspec.lock` names a package the `deps` commit's `pub/` lacks,
  which only an offline build found out before.
- Moneypenny's chat comes back by itself after a socket fails, and says `Reconnecting…` meanwhile:
  after `[oo.chat] retry_delay` (default 1 second), doubling up to `retry_max_delay` (default 16). It
  stays down after a refused sign-in, a person's `Stop trying`, a socket lost while stopping an answer,
  and once the gateway's profile says she is off; `Try again` still works in each.
- A socket of a web build no longer carries the page's query or `#/route`: Moneypenny's kept them,
  and a Session's left a bare `#` that a browser's `WebSocket` may refuse.
- A person's avatar shows the initials of their first and last names, not their first two.
- The green ink gate reads redirections as a shell does, so `git push origin main 2>&1` and `git push
  origin main > log` are the push of `main` rather than a refusal over a ref named `2>`, and `git push
  2>&1 origin main` goes to `origin`, not a remote named `2>`. It holds more of what it could not read
  and used to pass: a word only the shell can fill in, in any shell's spelling (`git push origin
  HEAD:$BRANCH` was read as a push to a branch literally called `$BRANCH`, so no protected ref was
  asked about), a command in a double-quoted `$(...)`, one after a comment or here-document holding an
  apostrophe, `git.cmd`, an unquoted `C:\Git\cmd\git.exe`, `iex`, shortened `pwsh -Command` flags,
  `pwsh -EncodedCommand`, `cmd /c"..."`, a `^` inside a word for cmd, and a shell nested deeper than
  `[approvals] classify_nesting`, which was skipped. A commit message in a here-document that has a
  line reading like a governed command is held, as one without an apostrophe was.
- Provider libraries can be signed with no tooling but `ai6`, so a rebuild needs no record change:
  `ai6 plugins keygen --out PATH` once on the build host, `ai6 plugins sign --key PATH FILE...` in
  every build (`FILE.sig` beside each library, valid for that file name alone), and `ai6 plugins pin
  --publisher KEY FILE...` once for each name. `ai6 plugins check [--config PATH] [PATH...]` says what
  a `strict` scan would, with nothing loaded and no copy written, and exits 1 on any refusal.
  **A publisher entry is weaker than a digest pin, and a stolen key matters more.** Anything that key
  signed for that name loads, an older vulnerable build included, since nothing binds a version; a
  stolen key signs anything until the record changes, with no revocation list; and the key sits on
  whatever signs, where a digest pin needs no secret. So `PinSet::weakenings` adds a line per key whose
  signature alone is enough, `publisher ed25519:... alone admits N libraries`, which reaches the log,
  `GET /internal/plugins` and `plugins_weakened`, and a Station with such an entry now reports it.
  `require = "both"` gives up none of this and gets no line. New: `PublisherSigner`,
  `publisher_entry`, `check_libraries`.
- A `strict` scan removes old verified copies of provider libraries, which were never removed: it
  keeps what it loaded and the newest `keep_verified` files of `[station.plugins]` or `[unex.plugins]` (default 8; `0` keeps none of
  them; `"none"` means no limit). A copy another process loaded or is about to load is left, and a copy
  that cannot be removed is listed and never fails the scan. This covers the Station, Universal
  Exports and the field runner (`ai6 runner serve --keep-verified`). New: `KeepVerified`,
  `PluginPolicy::with_keep_verified`, `ServeConfig::with_keep_verified`,
  `PluginScanSummary::verified_removed` and `verified_held`.

- **Outer Office reads `ai6.cfg`.** `OO.cfg` and `OO_CONFIG` are gone: on the desktop it reads the one
  file the other programs read (`AI6_CONFIG`, else the same directories, with an extension or first
  line choosing the format as they do) and takes `[unex] url`, `[unex] ca_file`, now trusted as well
  as the system's roots, and `[oo]`, which holds the tables `OO.cfg` had: `[oo.appearance]`,
  `[oo.chat]`, `[oo.refresh]` and the rest. On the web it reads the public `ai6.cfg` beside the page.
  Saving an address edits only the `url` line, keeping comments and every other section. A message
  names a key or a position and never quotes the file. `#!cfg` is no longer a format line.
- **One bootstrap file.** `ai6.cfg` serves every program on a host or for a user, and `unex.cfg`,
  `UNEX_CONFIG`, `UnexBootstrap` and `UNEX_FILE_NAME` are gone. Shared: `[provider]` and `[registry]`.
  `[station]` holds `id` (the old `station`), `state_dir`, `listen`, and `[station.plugins]`,
  `.identity` and `.secrets`. `[unex]` holds the clients' `url` and `ca_file`, the gateway's `listen`,
  `public_origin`, `proxy_secret`, `proxy_secret_min_bytes` and `state_dir`, and `[unex.plugins]`,
  `.access` and `.pages`. The Station's `[unex] token` and the gateway's `station_token` are one key,
  `[unex] station_token`. A key nobody knows is refused by every program, and each says when a key it
  needs is missing. A file of the old layout is refused, naming the key: a deployment moves its
  file with the binaries. Settings and `[registry.limits]` read through `FromValue` and declare a
  title and a default in their schema (`Bootstrap::schema()`). A `unex.cfg` beside a directory
  provider's family files is now the `unex` family's file.
- Configuration documents are read through `anycfg` over guatiao's value, in one reader for
  bootstrap files and family files. A `.cfg` file now names its format on its first line (`#!json`,
  `#!yaml`, `#!toml`, or a document starting `%YAML`), else it is TOML. A refusal names the key or
  the position and never quotes the document, and a repeated key is refused in JSON as in TOML and
  YAML. A YAML plain scalar has its type before a setting is asked, so `n`, `yes` or `123` in a
  text setting needs quotes. New: `ai6::bootstrap::format::{parse, read}`.
- A Mission ID names its Operation and a number or the tracker's code, not a random one: `ai6-42`,
  `ai6-gh-63`, `ai6-AI6-64`, `ai6-backend-63`, `ai6-nightly-43`. It is chosen once, where the brief
  enters, and stays through a retry, a return to the queue, a resume and a restart. Numbers
  count per Operation across every Station and gateway on the Registry and are never reused. IDs
  already stored stay as they are. New: `MissionEntry`, `BriefStore::allot_mission_id`,
  `entered_mission_id` and `enter_brief`, `Brief::mission_id`, `SourceItem::code` and `code_tag`,
  `[registry.limits.briefs] mission_id_bytes`. Removed: `ReviewTarget::review_key`. A PostgreSQL
  Registry already in use needs two new tables.
- A run's Session record is `<mission>.<attempt>` (`ai6-42.1`), not a random ID; the provider's
  Session ID stays apart. A Mission shows a name beside its ID, an issue's title as data or a
  person's title, with a link to the issue, in Outer Office and `ai6ctl`, whose `missions` table
  gains a `NAME` column. Search matches the ID, the run and the name. New: `Brief::name`,
  `Mission::name`, `LaunchManifest::name`, `[registry.limits.sources] name_bytes`.
- **Security.** Moneypenny refuses to run on a Station that also runs Missions on its own host. They
  run as the same OS user as her harness, whose environment holds her tokens, and a throwaway process
  under one user read a planted value from another's environment on both Linux and Windows. Any job
  there is refused with that reason, and the Station says so when it starts. Run her on a Station
  without the `execution` role or a local Theater, or set `[assistant]
  allow_beside_local_missions = true`, which accepts it and is reported as a weakening. **A Station that
  runs both today stops serving her until you do one of those.**
- Moneypenny's conversations carry no exposure: it is for an Agent doing work, and a Mission's
  Session keeps it unchanged. **This weakens what protected you.** Nothing she reads marks her
  conversation, so text she reads can steer her with nothing to show it happened. Her memory
  commits at once with its diff in the chat, whatever she has read, so such text can persist into
  later conversations; the commit is still refused when a file changed on `main` since she read
  it. Her views, proposals, link cards and pages no longer say "Made after reading text from
  outside AI6", a page's bar no longer warns or names what she read, `frame = false` now applies
  to every page, and **Open Moneypenny's links at once** applies to every link. Your confirmation
  is the one control on what she does. It is tighter in one way: every Cable she sends or replies
  with now waits for your confirmation, whatever she has read, because a Cable reaches other
  people and Agents. Her tool results stay framed in `<ai6-data>`, green ink stays a person's, and
  a Cable from her exposes no Session
  ([what this gives up](https://jose-pr.github.io/ai6/assistant/#what-she-reads-and-what-that-gives-up)).
- Removed with it: `[assistant.gateway] exposed_sources` and `source_bytes`, `[assistant.limits]
  memory_import_depth`, `AssistantJob::exposed`, `AssistantPageFile::outside_text`,
  `CheckpointSpec::record`, `outside_tool`, `AssistantReach::scripts_reach_hosts`, the EXPOSED
  column of `ai6ctl pages list`, and `POST /v1/station/assistant/exposure`; a Station asks `GET
  /v1/station/assistant/cables` for the person's Cable permissions. A roster or database record
  that sets one of those settings is refused until it goes. Files already under
  `.moneypenny/outside-text/` in a person's repository are left there, unread.
- A Mission's handler holds its Session to the Agent's clearance. At `desk`, or a clearance the
  handler does not know: the `claude` handler offers reading, searching, editing inside the working
  directory and AI6's MCP servers, and no `Bash` or `PowerShell`; on a remote Theater, no
  `theater_run` or `theater_write`. The `codex` handler starts the thread with no command tool and
  file changes inside the working directory, after its tool check, which refuses a release that
  offers a command; Codex reads through commands, so that Session can read no file. The `acp` handler
  refuses to start it, because AI6 does not choose an external agent's tools, unless
  `unconfined_desk = true`, reported as a weakening. `field` and `00` keep what they had. Answer-only
  and assistant Sessions do not change. On `claude`, `allowed_tools` can still add a command at
  `desk`, reported as a weakening. See "Agents and clearance" in the guide.
- A resume carries the clearance the Agent works at then (`AgentResume::clearance`), so a lowered
  clearance reaches a harness at its next start or resume, and a resumed Session is offered what
  the new clearance allows. A harness that is already running keeps its tools until then.
- `ai6ctl mission` prints each Session's lifecycle reason, such as `permission-denied`.
- A Mission on the `claude` handler can edit files and run commands. It runs under `--permission-mode
  dontAsk` with an explicit list: reading and searching, editing inside the working directory,
  `Bash` and `PowerShell`, a checklist and AI6's MCP servers; `WebFetch`, `WebSearch`, sub-agents and
  other MCP servers stay out. Before this it could only read and call AI6's tools, unless the
  operator allowed more in the harness's user settings, and it still reported `done`. Green ink
  still decides each command. An Agent's `handler_settings` (`allowed_tools`, `denied_tools`) change
  the list, and a rule that adds to it is reported as a weakening (`agents_weakened`). A harness
  with `Bash` runs as the Theater's OS user and can read its own environment: isolate with a
  container Theater where that matters.
- An Agent's `handler_settings` go beside the `station` record's `[providers.<id>]` table for its
  handler, which they used to replace.
- A turn whose calls the Claude Code harness refused is reported on the Mission, also when it ends
  `done`: a `denied` event, `denied_tools` on the Mission, the lifecycle reason `permission-denied`,
  and a line in the report on its source issue unless the cover hides it.
- After a refused resume the gateway shows the Mission's own state. It used to show its Session
  `working`, while the Mission was blocked.
- CI's app jobs and the documentation build use the SDKs `tools/sdk/sdk.lock` names instead of
  floating ones: Flutter at a commit rather than `channel: main`, Python and the docs tooling
  hash-locked, PostgreSQL's image by digest. One Rust leg stays on floating stable.
- In Outer Office a name opens what it names, on every page: Operations, Missions, Sessions,
  Stations, Theaters, Agents, roles and pools, and Overview's counts open their page filtered to
  what they counted. Each page has one heading and a line on what it is for; an empty page says
  what to do first, with a link, and Missions offers **New Mission in …**. Timeline and Audit say
  actions in words, with the action's name on hover; demo words are the same everywhere; Station
  cards take a phone's width.
- A Missions route's `state`, in Outer Office and Moneypenny's `missions` tool, goes by a Mission's
  state, that of the Session updated last, so the Missions under a count on Overview or an
  Operation are the ones it counted; it used to keep a Mission with any Session in that state. The
  page's state chips count Missions too. The Sessions page still filters each Session by its own
  state. Outer Office used to pick that Session by event number, which is counted per Session, so a
  long failed attempt hid the newer retry after it.
- Outer Office's wallpapers are one drawing in each theme's colours, exact at any size and under
  a fifth of the bytes, and the hero picks its file by the box's physical size, decoding only what
  the box needs (`[art] max_width` in `OO.cfg`). Light and Classic gain a sharper paper hero. The
  wallpaper's label no longer clips: it is drawn whole beside the sign-in panel, or not at all.
- Outer Office's rail is organised by what a person is asking: Work, People and policy, Machines
  and Record, each group shown only when the person may see something in it. Narrow windows get
  icons with labels on hover, a phone a bottom bar with **More**. A search result opens what it
  names, not just its page, and `[pages] search` says how many it lists.
- Outer Office asks less of a slow link. The board reads what the page shown displays every three
  seconds, what only the bell and search need every `[refresh] slow`, and the person's profile
  every `[refresh] profile` (60 s), instead of everything every three seconds; while the tab cannot
  be seen it reads once every `[refresh] hidden` (60 s; 0 reads nothing), and the pages' own timers
  wait too.
- Outer Office says what a person may do in plain words by group (Missions, Sessions, Green ink,
  Agents, Pools, Cables, Audit, Access), with the action names on hover, on Operations, an
  Operation's summary and the operator menu.

### Fixed

- **Security.** An ACP permission request AI6 cannot decide is refused, not answered with an error: one
  that does not parse, a decision that cannot start or an answer that cannot be written now answers
  outcome `cancelled` with a `blocked` event saying why. The Claude ACP adapter 0.99.1 has been seen to
  go ahead with the tool on an error answer, in Manual mode (not checked by AI6 against that adapter).
- A person's confirmation of a Cable works after Moneypenny's harness has gone. A held Cable waits an
  hour and an idle harness closes after ten minutes, and its closing dropped what it held, so the
  confirmation failed; every Cable she sends is held now. The desk keeps what is held past the harness,
  a harness that starts again carries it on, and a confirmation that sends nothing says why. A Station
  restart still drops what is held.
- A lowered clearance reaches a running harness. A person lowering it, a quarantine taking back a
  raise, desk duty beginning or a retirement left the harness its tools until it next ended. It is now
  stopped at the Station's next check-in, the Mission `blocked` with reason `clearance-lowered` and an
  `agent.clearance.stop` audit record, and a resume goes on at the clearance the Agent has then. A
  harness a field runner kept is stopped as soon as a Station re-attaches to it: its Session record
  (`SessionLaunch::clearance`) now says what it was launched at. A failed Registry read stops nothing.
- A Codex thread in remote-hands mode no longer keeps a command tool on the Station. Its read-only
  sandbox left `exec_command` reading any file the Station's user can read on Linux (measured with
  Codex 0.164.0; on Windows such commands were refused by policy). `features.shell_tool=false` now
  takes it out, and what the handler's documentation said, that it could not be switched off, was wrong.
- The Stations guide said Stations speak `ai6-station/1` to each other. Nothing dials or listens on
  it: they coordinate through the Registry. `ai6 identity` and `ai6 peer` stay as they are.
- A terminal that reconnects replays from what it received. It started from, and followed, the board's
  sequence, so it could skip output the board had seen and it had not. A viewer whose position was
  trimmed from the gateway's history, or whose Session's sequence skips a number, is now told: DipTel
  gains `TerminalGap`, on a new channel 5 with `gap_frames` and `IncomingFrame::Gap`, which Universal
  Exports sends on attach and live, and Outer Office shows `[AI6: output is missing here]` there.
- Terminal output longer than the gateway takes in one frame arrives whole. `[publisher]
  terminal_bytes` defaulted to 1 MiB against the gateway's `frame_bytes` of 256 KiB, and the gateway's
  refusal ended publishing. It now defaults to 192 KiB, which with room for a frame's header is that
  256 KiB; registering a Session (`POST /v1/station/sessions`) answers `terminal_bytes`, what its
  frames may carry; the Station splits output to fit, and halves a frame the gateway still refuses.
- A Station keeps publishing through a gateway outage. One failed publication or one full queue used
  to end its publish thread until the Station restarted; a single `desk` Codex start could do it, its
  tool check's output filling the queue. Now it tries again with growing pauses (`[publisher]
  publish_retry_delay`, `retry_max_delay`), registers each live Session again when the gateway
  answers, and every `refresh_every` seconds for one that restarted during a quiet spell, with its
  brief and latest state, and goes on with its sequence. Output past `[publisher] queue` is dropped,
  oldest first; the Session's sequence skips a number there, a Session's latest state is never what
  goes, and `/internal/publisher` gains `lost_terminal_bytes`. Consecutive reads of a Session travel
  as one frame. Output past `pre_bind_terminal_bytes`, a Session changing its ID and a lock failure
  no longer stop publishing either. Failed input and control polls now back off too.
- **Security.** Codex's tool check no longer reaches a Session's viewers. `codex mcp list --json`
  prints each MCP server's environment values and `codex debug prompt-input` the thread's
  instructions, and both went through the terminal a Station publishes, so anyone with
  `session.read` on a `desk` Mission or an answer-only review on Codex could read them. Those
  commands now run through `AgentTerminal::spawn_private`, a new slot every implementer of the kind
  adds: only the handler reads its output. Treat a value an MCP server's environment held as read
  by those people if such Missions ran with a publisher.
- Resuming a Mission whose harness is still running no longer ends it. The old harness answers
  being stopped with the end of its turn, and that could fail the Mission with the new harness
  running, or refuse the resume. The old harness is now stopped once the new one has taken its
  place, and nothing it says after that counts. A Session that truly ends while its resume is
  under way refuses the resume (`409`), and the harness the resume started is stopped.
- A person stays signed in past the access token's hour. `POST /v1/auth/renew` moves the token's
  expiry on, up to `[gateway] session_max` (twelve hours) from signing in, with the same value, so
  her chat, her tools and a waiting proposal carry on. Outer Office renews `[refresh] renew_before`
  (ten minutes) ahead on its own timer, says once when the sign-in is about to end, and goes back
  to its last conversation after signing in again. `session_max = 0` turns renewal off
  ([Access and sign-in](https://jose-pr.github.io/ai6/access/#staying-signed-in)).
  (the Access and sign-in guide, "Staying signed in").
- A long answer no longer loses Moneypenny's tools when her tool token's time passes: past it, the
  token of a running turn stays accepted while its Station is heard from within `answer_deadline`,
  and until the person stops it. Her Cable token follows the same rule.
- The access token's default and the ceiling of `ttl_seconds` are settings, `[gateway]
  access_token_ttl` and `access_token_max_ttl`, and a request may name a lifetime under a minute.
- A resume with no message of its own (a restarted Station's, or an operator's) tells the harness
  to carry on: Claude Code and Codex do nothing until they read a message, so the Mission sat
  `working` until it stalled. An interrupted turn that ends before the harness reported any
  Session is not left waiting for a resume that could never run.
- `ai6 runner connect` no longer waits for ever on a runner that accepts and stays silent: the
  `[runner]` setting `answer_wait` (10 s by default) bounds the read of its answer.
- A gated Codex thread's `apply_patch` no longer waits for a stall:
  `item/fileChange/requestApproval` is answered by the rule the Claude handler's gate keeps. An
  edit inside the working directory is accepted; a path outside it or a wider `grantRoot` is
  declined and the Agent told, since Codex applies an accepted patch wherever it points.
- A mirror is no duplicate of its own original, so a class authorisation `issue-create` can
  approve one; a second copy of the same issue still is.
- `own` means a comment AI6 wrote, decided by AI6's mark and never by its author, so one person's
  token works: that person's reply on a tracker is delivered like anyone's when `reply_authors` or
  the tracker's rules permit it. GitHub marks with its hidden comment; Jira with the comment
  entity property `dev.coqui.ai6`, set on every comment AI6 writes and read with
  `expand=properties`.
- A Source issue's labels no longer become a brief's required Agent tags, so anyone able to label
  an issue cannot fail its Mission, and AI6's own status labels cannot either. They stay readable
  to the Agent inside the fenced brief. A brief that never became a Mission reports that, not an
  empty Mission ID.
- A harness that reached no model no longer ends its Mission `done`: a Claude Code `result` with
  `is_error` or an `error_*` subtype is a failed turn, whatever its subtype says. Under strict
  exposure a Session keeps the credential its harness reaches its model with (its pool account, or
  a loadout secret in a provider credential variable), so a Source Mission can run; every other
  secret stays withheld.
- A stalled Mission can be resumed: the stall closes its harness and keeps its safe house, which
  is finished when the Mission reaches a final state, also when that is a cancel with no live
  Session. A refused resume says why: the Station logs the cause, and the operator's answer
  carries the provider's first line unless it holds something shaped like a secret.
- Interrupting a Mission leaves it `blocked` however the harness reports the end of the turn:
  Claude Code answers an interrupt with a result, which ended the Mission `done`. The Mission is
  blocked before the harness is told, with reason `interrupted`; the end of that turn closes the
  harness, and its Watchdog waits for the operator. Ctrl-C in a Session view does the same.
- A Claude Mission can be resumed: a resume no longer demands the provider Session ID before the
  harness's stream reports it, which Claude Code does only after reading a message. The ID is
  checked when it appears; a harness that resumed another Session is stopped and its Mission stays
  blocked, with reason `resume-refused`, nothing recorded.
- A Claude Mission can call its AI6 tools: the handler allows exactly the `ai6-cables` and
  `ai6-theater` servers with `--allowedTools`. Under `-p` Claude Code denied them, and the Mission
  went on to end `done` having done nothing.
- A Claude Mission's AI6 tools work from its first Session: the Cable endpoint is bound when the
  Session ID first appears in the harness's stream, and the ACP facade passes that ID on with the
  event that reported it. Until then every issue, Cable and green ink tool answered "Cable
  identity is unavailable".
- With `assistant = false` Outer Office no longer offers Moneypenny. `GET /v1/user/profile` says
  whether the gateway serves her (`moneypenny`); off, the app shows no card, button or chat, and
  Settings says so. On, she greets only once her socket is ready, and a socket that fails, closes
  or stays silent past `[chat] connect_timeout` shows **Try again** instead of an endless
  "Connecting…" and an uncaught exception.

### Added

- **Compartments: need-to-know access across Operations.** Clearance stays how far an Agent may act
  in its own work; a compartment says what one may know of, and say to, other Operations' work. It
  names what it grants (`mission.status`, `mission.read`, `product.read`, `cable.send`, `cable.read`,
  `mission.propose`; anything else is refused), what it covers (Operations, with what is under them
  unless `propagate = false`, or single Missions) and who is read in (`person:`, `group:`, `account:`,
  `operation:`, `mission:`, `agent:`). The record is the new `compartment` family, global, with a
  `min_clearance` and limits that each take `none`; looser than the default is a weakening. Every read
  and every Cable asks the provider what is granted at that moment, so a removed grant holds from the
  next call; a record that cannot be read grants nothing, and a provider that cannot be read refuses.
  Every crossing is written to the audit log before it returns, and a failed write refuses it.
- An Agent holding a compartment has `compartments`, `missions_in`, `mission_brief` and
  `mission_products` on its Session's MCP server. A brief or a product marks the Session
  `product:<operation>`. `propose_mission` takes `operation`, needing `mission.propose` there and
  clearance `00`; the proposal is untrusted intake in the target, exposed `brief:compartment`.
- Cable scope `compartment`, with its own fan-out and buckets across Stations, and replies that stay on
  the grant that opened the conversation. `discover_agents` shows Agents reached through a compartment
  by codename, Operation and Mission only.
- People: a compartment adds to the access policy for `mission.status`, `mission.read`, `product.read`,
  `cable.send` and `cable.read`, and nothing else. New actions `compartment.read`, `compartment.manage`,
  `compartment.cover` (an Operation's consent to be covered, not delegable by default), `mission.status`
  and `product.read`; routes `/v1/user/compartments`, `/v1/missions/{id}/products` and
  `/v1/missions?compartment=K`; `[gateway] compartment_audit`; `ai6ctl compartments` and `ai6ctl
  missions products`; a Compartments page in Outer Office, a chip on an Operation and a Missions filter;
  Moneypenny's `compartments` tool.
- A PostgreSQL configuration provider needs the new table `ai6_config_compartment` before the new
  binaries start: `CREATE TABLE ai6_config_compartment (scope TEXT NOT NULL DEFAULT '', key TEXT NOT
  NULL, definition TEXT NOT NULL, revision BIGINT NOT NULL, PRIMARY KEY (scope, key))`, as role `ai6`.

- `tools/sdk/`: `sdk.lock` names every SDK a build needs at exact versions, `bootstrap.sh` and
  `bootstrap.ps1` install them under one directory, verified, without touching the system or the
  shell profile, and `check_lock.py` holds the lock to the repository.
- Outer Office has a Linux runner. Linux, Windows and the web are its supported targets.
- Forms in Outer Office for adding, editing and removing Operations, Agents, Agent roles, cover
  policies and Moneypenny's settings, in place of the JSON box. Each form comes from the record's
  own type: `#[derive(ToValue, FromValue, Schema, Form)]` declares every field's title, words,
  default, choices and section once, and `GET /v1/user/roster/{kind}/schema` serves the schema and
  form to anyone with `roster.read`. Fields that name another record offer it, handlers and
  Stations come from what Stations report, what is rarely changed is folded, a refusal shows on the
  field it names, and a removal first says what names the record. **Add** and edit controls are on
  the Operations and Agents pages too. Rows show a summary in the fields' titles; a read-only "As
  configuration" view stays for copying ([Operations](https://jose-pr.github.io/ai6/operations/)).
- `If-Match` on `PUT` and `DELETE /v1/user/roster/{kind}/{key}`: the `revision` each record now
  carries, or `0` for one that must be new. A record changed since it was read answers `409` with
  `changed` and changes nothing; the app offers to reload. A write without it replaces what is
  there, as before.
- `handlers` on each Station of `GET /v1/stations`: the Agent handlers it reported at check-in.
- Records are read through Guatiao's value: a stored definition or a file becomes a value at the
  format boundary and the type comes from that (`FromValue`), so a refusal names the key in
  Guatiao's words, never the value. What is stored does not change. `RosterSettings` and
  `RosterBuilder::max_operation_depth` take a `Limit` and `WatchdogOverrides` holds `Seconds`.

- Outer Office signs in with a name and password on the web as on desktop, on a page of its own
  with the wordmark over the theme's wallpaper. A wrong password, the login limit's `429` (its
  `Retry-After` counts down and holds **Continue**), a refused origin and an unreachable gateway
  each say what happened. An organization proxy that vouches for the person keeps its button. The
  token still lives only in memory. The new `GET /v1/auth/methods`, which needs no token, says
  what `POST /v1/auth/login` accepts ([Access and
  sign-in](https://jose-pr.github.io/ai6/access/#signing-in-from-a-browser), with what a reverse
  proxy must pass).
- `[unex] ca_file` in `ai6.cfg` and `ai6ctl --ca-file` (`UNEX_CA_FILE`): a PEM file of certificate
  authorities trusted for Universal Exports besides the public roots, on every Station and
  `ai6ctl` request to it. A file that cannot be read or holds no certificate is refused at start,
  naming the path; a set `ca_file` is reported as a weakening
  ([Configuration](https://jose-pr.github.io/ai6/configuration/)).
- Moneypenny publishes pages: `publish_page` (a dashboard that reads live data through
  feeds, a report, or one file; from `personal/` or `scratch/` through the Station, or inline
  text), `unpublish_page` and `pages`. Universal Exports keeps them (`unex.cfg` `[pages]`,
  quotas and options in the roster's `[assistant.pages]`), expires them after a day unless
  the person remembers one, and serves each through a capability link under a CSP `sandbox`
  (an opaque origin: no cookie, storage or `/v1` of its host), optionally on an origin of its
  own, in a frame under a classification bar naming what she read. Only the person opens a
  page; feeds run with the viewer's own permissions. Her conversation now lists what exposed
  it, and the Station names the web tool and host instead of `tool:outside-text`. Outer
  Office shows each page as a card (kind, size, expiry or Remembered, what she read, its
  feeds) whose Open launches it outside the app, with a menu to open it with the network
  off, remember or forget it and delete it; `ai6ctl pages list | open | remember | forget |
  rm` does the same. Sharing a page waits for compartments
  ([Moneypenny](https://jose-pr.github.io/ai6/assistant/)).
- Moneypenny's proposals show what they rest on and what they touch. A proposal records the
  read tools she called since the person's last message (each with its identifier arguments
  only), the conversation's exposure and what exposed it (`exposed_by`, `tool:NAME` or the
  Station's source as given), and the link of the page it changes (`opens`). The card reads
  "Proposal · needs ACTION on SCOPE", lists "Based on" her reads, offers an "Affects" chip
  that only opens the page, says how it stands in words with icons, answers Confirm once,
  and reads "Outcome unknown, check the page" after thirty seconds of applying. Every kind of
  chat entry has an icon and a semantics label from one table
  ([Moneypenny](https://jose-pr.github.io/ai6/assistant/)).
- Outer Office says how current and how complete what it shows is. Each source of the board
  (Stations, Sessions, Missions, held briefs, green ink, Mission proposals, timeline) settles
  on its own: a failed read keeps the last data and says "Last updated", a 403 hides it, a
  401 signs out and clears everything, and a strip appears only while the gateway is
  unreachable. Overview and the bell say nothing needs the person only when every attention
  source was read, and now include green ink requests and Mission proposals waiting, each
  opening its own request. A control the person asked for is sending, accepted, applied,
  refused or an outcome unknown, disables its Mission's menu while open, and lists under
  **Your requests** with **Check again**, which never posts twice. Times are relative with
  the exact local time on hover; search says it covers what loaded and opens with Ctrl+K.
- Outer Office's Missions page is a queue and its Mission, Station and Operation routes open
  detail views over what the board already holds. Rows say state, place and age (relative
  times with the exact local time on hover), sorted newest update first by default with ties
  in Operation and Mission order; the `queued` filter counts held briefs and Mission proposals
  with links to Briefings; **Mission details** shows every attempt with Station and Theater
  links and, for a queued Mission, where its Operation's placement stands; a Station shows
  Online or Dark, its last check-in, resources as sampled (or **Not reported**) and the
  Sessions running there; an Operation shows its access, Missions by state and recent
  activity ([Outer Office](https://github.com/jose-pr/ai6/blob/main/clients/outer-office/README.md)).
- A field runner speaks the Agent Client Protocol instead of its HTTP API: a Station connects
  over the loopback port, or through `ai6 runner connect` on a terminal its transport opens,
  attaches to a Session with `session/load` (the runner replays what the Station missed), and a
  newer connection takes a Session over from an older one. `ai6 runner call` and the HTTP
  routes are gone; the runner's `url` in `runner.json` is now `address`. The Theater `runner`
  settings gain `connect_timeout_ms`, `initialize_timeout_ms` and `request_timeout_ms`; the
  `station` record's `[runner]` table gains `pump_poll`, `feed_poll` and `replay_batch` (which
  replaces `page`), renames `call_margin` to `start_margin`, and loses `max_wait` and
  `call_output_bytes`, which only the HTTP API had a use for.
- The `acp` handler (`ai6-agent-acp`): any Agent Client Protocol agent (the Claude and Codex
  adapters, `gemini --acp`) is an Agent's handler, started with the Session's full
  environment and judged by the green-ink gate. Configured by the Agent's `handler_settings`
  (`command`, `args`, `env`, `meta`, option ids and timeouts).
- An Agent's `handler_settings` table configures its handler provider (the provider validates
  it). Operator terminal input to a Claude or Codex Session, which speak JSON on plain pipes,
  is no longer written to the harness's input: typed lines become messages and Ctrl-C
  interrupts.
- The Agent Client Protocol (ACP) wire in `ai6` (feature `acp`, part of `daemon`):
  `AcpSession` drives any ACP agent as an `AgentSession`, and `AcpFacade` serves any
  `AgentHandler` as an ACP agent, with AI6's environment, model, effort, policy and events
  carried in `_meta["dev.coqui.ai6/launch"]` and `_ai6/event`.
- Built-in access roles. The gateway's people roles are now `chief`, `controller`, `operator` and
  `auditor`: `chief` holds every action, is computed and never stored, and is named only in the
  top-level `chief` list (and `unex.cfg`), which no stored rule shadows; the other three are
  ordinary roles a first start seeds. New actions `access.read`, `access.manage` and
  `account.manage`; a role bound at an Operation grants only the actions that mean something
  there (`OPERATION_ACTIONS`). A change is guarded: an editor adds only what they hold, only a
  chief changes the chief list, and the last chief cannot be removed. `ai6::access_from_toml`
  returns an `EffectiveAccess` (the tiers evaluated together, with `explain`), replacing the
  bare `RoleAuthorizer`.
- Operation roles. An Operation has its own access policy, the `operation_access` record: roles
  and rules that only add to the global policy, hold only Operation actions within the global
  `delegable` cap (every Operation action but `access.manage` and `approval.class` by
  default), and bind only at the Operation or one under it. Whoever
  holds `access.manage` there manages them and can hand out only what they hold, down the
  Operations a grant reaches. A rule the tree or the cap leaves without effect is listed as
  orphaned and the rest of the record stays in force.
- Access administration through Universal Exports. A first start with user access seeds the
  `controller`, `operator` and `auditor` roles (audited `access.seeded`); `unex.cfg [access] chief`
  names subjects that are always chief. `GET/PUT /v1/user/access` (roles, rules, the delegation
  cap, the chief list, `check`) and `/v1/user/operations/{id}/access` change the policy under an
  `If-Match` revision and the no-escalation guard, audited; `/v1/user/accounts` makes, disables and
  resets accounts (a password is shown once, never audited), with the `unex` record's `accounts`
  provider. Moneypenny reads with `access` and proposes with `propose_access_change` and
  `propose_account_change`; a reset password reaches only the app. `ConfigProvider::put_if` stores
  a record only when it is the one the writer read. `[text] access_name_bytes` in the `unex`
  record (128 by default) bounds a role name, an Operation ID or an account name in these routes.
- `ai6ctl access` (show, role set and remove, rules export and import, delegable, chief, check)
  and `ai6ctl accounts` (list, create, disable, enable, groups, reset-password), and an Access
  page in Outer Office: roles with a switch per action greyed where you could not give it, the
  ordered rules, the cap, accounts and a Check, for the global policy and for one Operation.
- The issue policy and a tracker instance's settings are declared once, on their structs, with a
  title and a description on each key; the tables in the Operations guide are generated from them
  and a test fails when they differ. The GitHub provider asks for REST API `2026-03-10`, whose
  issue creation carries no deprecation; `2022-11-28` sunsets on 2028-03-10.
- Trackers: a second Guatiao kind (`ai6_tracker`) the `github` and `jira` providers serve
  beside `ai6_source`, built from the same `sources.<name>` table and token. A Tracker reads
  an issue, lists the replies that may answer a question, posts and edits a comment, changes
  labels, assignees, state or a workflow transition, creates an issue (a child under a
  parent) and links issues; what an instance can do is discovered, and a call it cannot make
  fails naming what it can. An instance is named by its key under `sources`, `provider`
  picks the provider (default: the key), and `intake = false` keeps it out of polling. A
  Mission's outcome is a comment plus the instance's `status_labels` and `status_transitions`
  entry for it, replacing `done_label`, `failed_label`, `done_transition` and
  `failed_transition`; `Source::report` and `CxReport` are removed. Jira Cloud text takes
  headings, lists, code blocks and links.
- Linked issues and the outbox. A Mission that starts from an issue is linked to it (a
  Registry record on the file and SQL Registries; a link names a tracker instance and an item
  and how the issue relates to the Mission), and AI6 keeps the issue current: by default one
  status comment edited in place as the Mission works, waits or pauses, the instance's status
  labels and transitions, and the report as a new comment. Every write to a tracker is an
  outbox entry that only the Headquarters Station sends, fenced by the election term, audited,
  rate limited, retried with a doubling delay, and settled once across a Headquarters change.
  `[operation.ID.issues]` sets how much is written and its bounds; looser values are reported
  at start and at check-in. `CableStore::take_cable_quotas` is renamed `take_quotas`.
- Agents split work across trackers: `issue_create`, `issue_split` and `issue_mirror` ask for
  issues a person approves, or a class authorisation `issue-create` for an unexposed Session with
  no findings, or none under `create = "direct"`, a reported weakening. Children go under their
  parent where the tracker can; a mirror lists what it cannot carry and leads the original to the
  copy. A created issue never starts work: intake labels are stripped, and only an approval with
  `mark_ready`, which also needs `mission.create`, adds them. Caps per Mission and per day and the
  depth of a split are refused at the call
  ([Linked issues](https://jose-pr.github.io/ai6/missions/#linked-issues)).
- People work a Mission's issues: `issue.link`, `issue.comment`, `issue.update` and `issue.create`,
  seeded on controller and operator and delegable, with routes under
  `/v1/user/operations/{op}/missions/{mission}/issues`, `/v1/user/operations/{op}/issues` and
  `/v1/user/operations/{op}/issue-proposals`, `ai6ctl mission issues`, `ai6ctl issues` and
  `ai6ctl issue-proposals`, Moneypenny's `issue_proposals` and `propose_issue_*`, and Outer Office's
  Linked issues on Mission detail. A person's comment carries their name in a footer, and on an
  issue with an open question it answers it as an operator's text. A PostgreSQL Registry needs the
  new `ai6_issue_*` tables and `ai6_mission_issues` from `schema/postgres.sql`, created as role `ai6`
  with every Station and gateway stopped.
- Agents work their issues. A Mission linked to issues gives its Agent `issues`, `issue_read`,
  `issue_comment`, `issue_request_info`, `issue_update` and `issue_link` on its Session endpoint:
  comment on the source, a related issue a person accepted and the children it created, ask a
  question (the Mission blocks with reason `issue` until a permitted reply, an expiry that resumes,
  fails or keeps waiting, or a withdrawal), and change labels within the instance's `agent_labels`.
  Closing, reopening, assigning and an unlisted transition wait for green ink (`issue-close`,
  `issue-assign`); an exposed Session's post outside its own issue waits too (`issue-comment`,
  exposure restriction `issues`). Everything an Agent writes is checked first for secret values,
  credential shapes, invisible characters and links to other hosts. The Headquarters Station reads
  comments; replies from the issue's author or reporter, a maintainer or a listed login reach the
  Session as fenced untrusted text and mark it exposed (`issue:TRACKER`).
- Operations nest ([Subordinate Operations](https://jose-pr.github.io/ai6/operations/#subordinate-operations)).
  An Operation's `parent` names another Operation; a child layers its keys over its
  ancestors' (tables merge, scalars replace, the nearest wins; `inherit = false` opts out), a
  rule, pool glob or Cable grant on a parent reaches the Operations under it
  (`propagate = false` on a rule stops that), and a missing parent or a cycle refuses the
  roster. `max_operation_depth` bounds the depth (default 64). Universal Exports lists
  Missions by subtree and serves an Operation's effective policy and where each key comes
  from; `ai6ctl operations` and Outer Office show the tree.
- Every limit, pace and size the code fixed is a setting, its default the value it had, and
  a bound can be taken away where a bound makes sense (`null`, or `"none"` in TOML) with the
  loosened ones reported as weakenings. Bootstrap files hold what starting needs: the
  Registry's and the configuration provider's limits (`[registry.limits.*]`, `[provider]`) and
  `unex.cfg`'s `proxy_secret_min_bytes`. The `station` record gains `[scheduler]`, `[publisher]`,
  `[sources]`, `[transports]`, `[recovery]`, `[vetting]`, `[accounts]`, `[approvals]`,
  `[harness]`, `[provision]`, `[secrets]`, `[terminal_output]`, `[green_ink]`, `[detached]`,
  `[runner]`, `[audit]`, `[theaters]`, `[theater_tools]` and `[head_of_station]`, and
  `[providers.<id>]` tables that configure the handlers, terminals, safe houses and `local`
  transport it builds; the `assistant` record gains `[limits]`, `[desk]` and `[gateway]`; the
  `cables` record its body size and lifetimes; the `unex` record `[gateway]`, `[controls]`,
  `[timeline]`, `[login]`, `[text]` and `provider_config_depth`. Providers take their own
  bounds as config keys (`claude`, `codex`, `pipe`, `pty`, `git`, `copy`, the transports, the
  GitHub and Jira Sources and the ephemeral container provider), `OO.cfg` gains `[refresh]`,
  `[controls]`, `[pages]`, `[terminal]` and `[limits]`, and `ai6ctl` takes `--timeout` and
  `--poll-interval`. The ceilings and floors that refused a value as too loose or too tight are
  gone (a minimum clone timeout and secret lease, the secret broker's one-hour ceiling, the
  Source poll minimum, the gateway's 1 MiB frame and 300 second TTL ceilings, and the like);
  only nonsense, such as a zero where nothing could pass, is refused. The public constants that
  became settings are removed (`STDERR_TAIL_BYTES`, `MIN_PROXY_SECRET_LEN`,
  `DEFAULT_TIMELINE_PAGE`, `MAX_TIMELINE_PAGE` and others), `run_process` takes the reader grace
  and `with_proxy_secret` the least size. The wire contract's frame sizes (`ai6-diptel`) stay
  fixed, since the Dart client shares them. See
  [Configuration](https://jose-pr.github.io/ai6/configuration/).
- Configuration moved to bootstrap files and a configuration provider
  ([Configuration](https://jose-pr.github.io/ai6/configuration/)). A Station and the `ai6`
  command read `ai6.cfg`, Universal Exports `unex.cfg`, and Outer Office `OO.cfg`, each found
  by `--config`, a variable (`AI6_CONFIG`, `UNEX_CONFIG`, `OO_CONFIG`) or the user's then the
  system's configuration directory, holding only what starting needs: locations, listeners,
  directories, and secrets as references to a file, a variable or an inline value (the last
  two reported as weakenings). Everything else is a record in one provider every Station reads:
  a directory of family files in TOML, JSON or YAML (`FileProvider`) or SQLite and PostgreSQL
  tables (`SqlProvider`), each record global or one Station's own: Operations, Agents, roles,
  covers, Moneypenny's settings, Theaters, loadouts and pools, Cables, provider pins, the
  Station's runtime settings, its TLS policy (`tls`, reported at check-in as `tls_weakened`),
  and Universal Exports' sign-in and role policy (`unex`, `access`). `ai6 config
  path|show|import|export` read and write them. `ConfigSource`, `TomlConfigDir`,
  `SqlConfigSource` and `MemoryConfigSource` became `ConfigProvider`, `FileProvider`,
  `SqlProvider` and `MemoryProvider`; a singleton record is keyed `settings`.
  Removed: the flags `--config-dir`, `--config-poll`, `--headquarters-lease`,
  `--max-concurrent-missions`, the watchdog flags, `--role`, `--label`, `--terminal`,
  `--resources-every`, `--safe-house-cache`, `--safe-house-cache-age` and `--plugin-pins` of
  `ai6 serve` (the roster's `--config` is now `--provider`); `--port`, `--roster`,
  `--plugin-pins`, the sign-in, login-limit, access-cache and assistant flags of
  `universal-exports`; the runner's `--plugin-pins` (now `--pins`); the variables `AI6_HOME`,
  `AI6_STATE`, `AI6_REGISTRY`, `AI6_CONFIG_DIR`, `AI6_STATION_ID`, `AI6_CONFIG_PASSWORD`,
  `AI6_REGISTRY_PASSWORD`, `AI6_ACCESS_TOKEN` (now `UNEX_ACCESS_TOKEN`) and every
  `AI6_EXPORTS_*` (`ai6ctl` reads `UNEX_URL`, else `ai6.cfg`'s `[unex] url`); `password_file=`
  in SQL locations; a pin set's `profile`, which is each host's own `[plugins] profile`.
- The Station API asks every `/internal` request for `Authorization: Bearer TOKEN`, the token
  in `api-token` under the Station's state directory, made on first start; `/healthz` stays
  open. Universal Exports accepts an identity its reverse proxy asserts only with
  `X-AI6-Proxy-Secret` equal to `AI6_EXPORTS_PROXY_SECRET` (required with
  `AI6_EXPORTS_PROXY_USER_HEADER`, at least 32 characters).
- `assistant.denied_networks` (and in group and person tables, which only add to it): the
  categories `loopback`, `private`, `link-local` and `metadata`, networks, addresses and host
  names Moneypenny's web tools and scripts may not reach. Empty by default: `open` reaches
  everything the Station reaches, the Station itself included. Her WebFetch gate checks a name
  by every address it resolves to.
- Her scripts read only what an interpreter needs (not `/etc`, `/opt` or `/dev` whole), each
  interpreter's install directory, and never AI6's state directory or the Station's own program,
  under Claude Code and Codex alike.
- The `pipe` terminal keeps a program's standard error on its own pipe by default
  (`stderr = "tail"`) and adds only its last lines after the output ends; it starts the program
  alone (its own session, no inherited descriptor or handle) and kills processes that left its
  process group.

- Universal Exports keeps its Directory's lookups and a plugin Authorizer's decisions for a
  few seconds (coqui-access-kit's `Cached`; `AI6_EXPORTS_ACCESS_CACHE_TTL` 5 s,
  `AI6_EXPORTS_ACCESS_CACHE_NEGATIVE_TTL` 1 s, `AI6_EXPORTS_ACCESS_CACHE_ENTRIES` 10 000, `0`
  turning one off), and reports a longer lifetime at start. `Plugins::cached_directory` and
  `cached_authorizer` load a provider that way.
- Outer Office (short name OO) is the Flutter desktop and web operator app, in
  `clients/outer-office` (Dart package `outer_office`); Moneypenny is the AI assistant
  inside it. Its build defines are `OO_API` and `OO_DEMO`.
- Outer Office takes the design kit's look: six themes (Night Ops, the default, Black Ops,
  Dusk, Classic, Light, Red File) or the system setting, chosen by sight (a miniature of
  each theme with its name, character and a check mark) from the top bar's gear or the
  operator menu and stored as `oo.theme`, with every text colour at WCAG AA where it is
  drawn (panels over art included), a chart palette per theme that stays apart for
  colour-blind readers, the system's reduced motion honoured and its high contrast
  dropping every glow, gradient and picture; pale cartographic hero art for Light and
  Classic; accent edges and glow only on the selection and the primary action; the
  **Wallpaper** switch (`oo.backdrop`) for the art behind sign-in, the Overview hero and
  empty pages; the kit's icons; and a rail of twelve pages
  (new Overview, Operations and Sessions pages; Agents, Green Ink, Briefings and Roster take
  Administration's tabs; Theaters, settings and placement are tabs of Stations) that has
  labels from 1200 px, is compact from 900 px and sits behind a menu button below that.
  It opens on Overview, whose attention list includes the briefs vetting holds, read with
  the board from each Operation where the person has `mission.create`.
- Outer Office pages are routes: a closed set of pages, each with typed optional filters
  (`app:/missions?operation_id=G&state=failed`), the same in Moneypenny's links, the page
  context sent with each message and web URLs (`#/missions?operation_id=G&state=failed`).
  Every filter shows as a removable chip with **Clear filters**; a page change is a browser
  history entry and a filter change replaces it; **Copy link** copies the page's web link;
  a URL that is not a route opens Overview and says so. Universal Exports and the app parse
  routes alike, tested on one fixture. The old `app:/missions/OP/ID`, `app:/stations/ID` and
  `app:/theaters/ID` links are refused, and a Moneypenny message carries `route` (the
  route's JSON form) instead of `page`. Every column of the Sessions table sorts through
  the route. Board Sessions (`/v1/board`) carry `station_id`, from their Registry record, and
  `agent`, from their Mission's brief, which the Station and Agent filters read. A route's
  `text` filter is at most 128 bytes by default: the `unex` record's `route_text_bytes` and
  `OO.cfg`'s `[routes] text_bytes` set it.
- Outer Office makes no request outside its own origin by default: it bundles Roboto and
  its icons, and the web build loads CanvasKit from `canvaskit/` and fallback fonts from
  `assets/fonts/fallback/`. `[assets] cdn = "fonts"` or `"all"` in the `OO.cfg` beside
  `index.html` opts in to Google Fonts, or to it and Flutter's CDN for the engine.
- Moneypenny's chat in Outer Office moves like an editor's chat view: hidden behind the
  top-bar button (with a dot when she answered or proposed something meanwhile); docked
  beside the page from 1200 px, resizable from 320 to 720 px by its drag handle or the arrow
  keys, a modal sheet from 900 px and full screen below; expanded into the centre, the rail
  kept and the page left as it was; or, on desktop, in her own window. Ctrl+Alt+I (⌃⌘I on
  macOS) opens, focuses or hides it, Ctrl+Alt+Shift+I expands it, Ctrl+Alt+O pops it out or
  brings it back, Esc steps back; Enter sends and Shift+Enter starts a new line. Each
  person's layout and width are kept on each device as `oo.chat.<person>`. `OO.cfg`'s
  `[chat]` sets its sizes (the docked width and its limits, the arrow-key step, the page's
  minimum beside it, the expanded text column, her window's size) and transition time.
  Overview, Sessions and Missions lay out by the width the page has, so a docked chat never
  squeezes them. In the demo (`OO_DEMO=true`) she answers from a scripted sample
  conversation, labelled as such, with no network and no effect.
- The Outer Office web build serves the Noto fallback fonts itself (727 files, 34.4 MiB,
  OFL-1.1), so text in other scripts and emoji render with no outside request;
  `tool/cdn_files.dart fill` puts them in after `flutter build web`, from the `deps`
  branch's `cdn/` or downloaded, each checked against `tool/cdn_files.json`. The bootstrap
  passes its configuration to `initializeEngine`, without which the engine fetched
  fallback fonts from Google Fonts even under the default.
- Moneypenny's views gain stacked bar and pie charts, a `schedule` of bars over time, a
  `graph` of nodes and edges (layered or a tree) and `svg` drawings. Universal Exports admits
  only an allow-list of SVG drawing elements and presentation attributes, refuses anything
  else with the reason (script, style, event attributes, `href`, images, `use`, animation,
  filters, DOCTYPEs and entities) and keeps the drawing written out again; the app draws it
  with `flutter_svg` and loads nothing. Charts are drawn with `fl_chart`, graphs with the
  app's own layered layout. Every chart, schedule and graph has **Show as table**. A view
  built in a conversation that read outside text carries `exposed: true`, and the app frames
  it with a warning. Her `missions` tool takes the Missions page's filters (state, Station,
  Agent, text) and filters as the page does; the gateway's route code and the app's
  `routeMissions` are held to one shared fixture. The bounds of her views are settings,
  `[assistant.views]` (`AssistantViewLimits`, `AssistantSettings::views`, `ViewLimits`,
  `AssistantConfig::with_view_limits`), sent to the app with each conversation.
- Moneypenny points to pages: her `navigate` tool (`{"to": ROUTE, "label": TEXT}`) offers an
  app route, checked as every route is, with a label of at most `[assistant.views]
  navigate_label_bytes` (120 by default), as a `navigation` entry the app shows as a card with
  the page and its filters as chips and **Open**. Opening only reads and is not audited; it
  is a browser history entry, docks an expanded chat and marks the page **Opened by
  Moneypenny**, as her links do. A per-person choice in her menu, **Open Moneypenny's links
  at once** (`oo.chat.<person>.follow`, off by default), opens a card as it arrives, never
  one offered after she read outside text or while a dialog covers the page. Her
  instructions say to offer the route of the records she summarises and to say when a tool
  cut what she read; her `audit` tool takes an Operation or a Station as the Audit page does.
- Moneypenny runs on Codex with scripts and her workspace. With interpreters, the Codex
  handler admits Codex's code-mode tools (`functions.exec`, `functions.wait`) inside Codex's
  own sandbox, a permission profile (`ai6_assistant`) that denies the filesystem but the
  platform's paths, her working directory (read) and `personal/` and `scratch/` (write), with
  only the script environment kept (`ai6::SCRIPT_ENVIRONMENT`, shared with Claude Code) and,
  on Windows, `windows.sandbox = "mxc"`; each such start is reported as a weakening, since any
  command runs inside the sandbox. Scripts on Codex get no network under any policy. Her
  context file reaches Codex as developer instructions; her server's tools are approved
  without asking (`default_tools_approval_mode = "approve"`); commands are reported as the
  tool `exec_command`, which counts as `Bash` does for exposure; a `propose_shared_change`
  call is read from Codex's `mcpToolCall` item for its shared-change base.
  `AgentHandler::assistant_scripts` for Codex asks `codex sandbox -P` whether the sandbox
  runs. New `AgentHandler::assistant_skills`: a Station installs no skill for a handler that
  cannot keep to the catalog (Codex) and tells her so.
- A change to a shared repository records what the file held when Moneypenny made it. As a
  Station sees her `propose_shared_change` call, it reports the file's SHA-256 in her
  `shared/<group>/` checkout, or that there was none (`AssistantEvent::shared`,
  `AssistantSharedBase`); Universal Exports attaches it to the change as
  `AssistantChange::base`, waiting up to five seconds for the report and refusing the call
  without one. The commit is refused, nothing written, when the repository's `main` has
  changed that file since.
- Moneypenny runs scripts and reaches the web on Claude Code, inside its sandbox and within
  the roster's `[assistant]` keys `interpreters` (`["python3"]`), `network` (`limited`,
  `denied` or `open`), `allowed_hosts` (none) and `unsandboxed_scripts` (false), which
  `[assistant.group.<group>]` and `[assistant.person.<id>]` tables may narrow and never widen
  (a widening table fails the roster load naming the key; `AssistantSettings::reach_for`,
  `AssistantNarrowing`, `NetworkPolicy`). Universal Exports sends each message's reach
  (`AssistantJob::reach`, `AssistantReach`), with the person's directory groups
  (`AssistantJob::groups`) so the Station re-caps by the same tables and refuses rather than
  skip a per-group narrowing it cannot apply; a job with no person takes the strictest reach.
  The Station recomputes the cap on every message and restarts the harness when it changed.
  Claude Code gets `Bash` allowed only as `Bash(<interpreter> *)` under `dontAsk`, a sandbox
  that exits rather than run unsandboxed and denies reads everywhere (`denyRead: ["/"]`) but
  the working directory and the system directories an interpreter needs, so the Station user's
  home, AI6's files, `/proc` and other conversations stay unreadable; writes only in her
  working directory, never a `.git` under it; the model credentials and every non-essential
  environment variable hidden (`credentials.envVars`); and each conversation its own temporary
  and Claude Code configuration directory (`credentials.files`), so transcripts and any stored
  login are isolated. `WebFetch` is held to the allowed hosts by a `PreToolUse` hook (`ai6
  assistant web-gate`), which also refuses this deployment's own hosts, loopback and the
  metadata address even under `open`; `WebSearch` only under `open`. A Station whose harness
  has no sandbox (native Windows, or Linux without `bwrap` and `socat`:
  `AgentHandler::assistant_scripts`) starts her without scripts and tells her so, unless
  `unsandboxed_scripts` allows them, logged as a loud weakening; `network = "open"` and a shell
  interpreter are weakenings too. A `WebFetch` or `WebSearch` call, or a script that can reach
  a host, exposes the conversation at once, with or without a workspace, and the hold on her
  Cables sees it the same turn.
- Moneypenny's skills come from an allow-listed catalog in the global `agents.toml`:
  `[assistant.skills.<name>]` with `path` (absolute, or relative to the configuration
  directory) and `pin`, the digest `ai6 assistant pin-skill DIR` prints
  (`ProjectManifest::read_tree`, `SkillEntry`). The roster load reads each skill's `SKILL.md`
  and refuses a skill without a pin, one whose front matter does not name it or has no
  description, and one whose front matter has any key beyond the inert ones
  (`SKILL_INERT_KEYS`; `allowed-tools`, `hooks`, `shell`, `model` among those refused) or that
  injects a command (`` !`…` ``, `` ```! ``), naming it; `skills_unpinned` and
  `skills_allow_tools` let them load and are reported as weakenings. Every catalog skill is
  given to everyone unless an `[assistant.group.<group>]` or `[assistant.person.<id>]` table
  narrows `skills` (`AssistantNarrowing`, `AssistantSettings::skills_for`); Universal Exports
  sends the person's in `AssistantJob::skills`, and a Station refuses a name its own roster
  does not give the person (its catalog less its own tables for the job's directory groups)
  and restarts the harness when the skills changed. These are the tables that narrow her
  scripts and network. At each harness start the Station copies each into the working directory's
  `.claude/skills/<name>/`, never through a link, and checks the copy against its pin and its
  `SKILL.md` again; a skill that fails is left out with `ai6: assistant skill <name> does not
  match its pin` (or why it was not installed), a `provider` finding in the Station's logbook
  and a notice in the conversation. `.claude/` is a plugin named `ai6`
  (`AssistantMode::skills_dir`), and her standing text lists the skills. The Claude handler
  offers `Skill` with `Skill(ai6:<name>)` allowed, passes the directory with `--plugin-dir`
  (under `--restricted`, Claude Code 2.1.311 loads no other skill: not the working
  directory's, the user's or an installed plugin's), and sets `disableSkillShellExecution`
  unless `AssistantMode::skill_commands`. The Codex handler still refuses skills.
- Moneypenny keeps each person's work and memory in their own repository. The global
  `agents.toml` takes an `[assistant]` table (`ai6::AssistantSettings`, also
  `RosterBuilder::assistant`): `personal_repo` (`{person}` or `{person_digest}`; a local or
  `file://` repository is created bare on first use, private to the Station user on Unix), `[assistant.shared.<group>]` and
  `[assistant.org]` repositories (`write = false`), and `allow_elevated` (false). Universal
  Exports sends each message with the person's workspace (`AssistantJob::workspace`); the
  Station clones `personal/` and `shared/<name>/` into the conversation's directory, writes
  `CLAUDE.md` (importing `memory/MEMORY.md`), `AGENTS.md` and a context file beside it, and
  after each answer commits `personal/` to `main` as the person with a `Via: Moneypenny
  (conversation <id>)` trailer, replaying once and falling back to
  `moneypenny/<conversation digest>`. Changes to `memory/` are set aside and reported
  (`memory_changes`): committed at once in a conversation that has read nothing others wrote,
  a `memory_commit` proposal once it has, and a confirmed one commits exactly the confirmed
  files. A memory change records what each file held when it was made
  (`AssistantChange::base`, checked through `CheckpointFile::base`), and its commit is
  refused when one has changed on `main` since. `propose_shared_change` proposes a change to a group's shared repository for its
  members, or commits it at once (audited `moneypenny.shared.write`) when the group's
  repository has `write = true`. `{person}` takes lowercase identifiers only;
  `{person_digest}` works for all. A Station takes only the repositories its own roster gives
  the job's `person_id`. Her repositories' git directories live beside her workspace, every
  Git command the Station runs on them has hooks, filesystem monitors, filters (Git LFS
  among them) and links off (a committed link is checked out as a plain file), the Station never reads, writes or removes
  anything in her workspace through a link, junction or other reparse point, and paths with
  a name starting with `.` are neither written by her nor committed. A starting desk removes
  what desks that are gone left in the temporary directory.
- A SQL roster keeps the `[assistant]` settings as one record of the new kind
  `ConfigRecord::Assistant` (`assistant`, `ConfigRecord::ASSISTANT_KEY`), the row
  `assistant` of `ai6_config_assistant` in SQLite and PostgreSQL, checked with
  `AssistantSettings::validate` like any other change; without it the roster has the
  defaults. `TomlConfigDir::definitions` lists the global `[assistant]` table under the same
  kind. The roster routes take `assistant` (`/v1/user/roster/assistant/assistant`, under
  `roster.read` and `roster.manage`) and an accepted edit answers with the settings it loosens
  (`RosterChange::weakened`); Moneypenny's `roster` and `propose_roster_edit` tools take it
  too, a proposal naming each weakening in its summary.
- The person reads and changes Moneypenny's memory of them from the app: Universal Exports
  serves `GET /v1/user/assistant/memory` (the files of `memory/`, their sizes and the commit
  `main` is at) and `GET`, `PUT` (`{"content", "message"?, "base"?}`) and `DELETE`
  `/v1/user/assistant/memory/{path}`, each a memory job (`AssistantJob::memory`,
  `ai6::MemoryOp` with `MemoryAction::{List, Read, Write, Forget}`, answered with
  `AssistantEvent::memory`, `ai6::MemoryAnswer`) for the Headquarters Station, else the first
  healthy one, within 30 seconds (`504` otherwise). The Station works in a clone of the
  person's repository of its own and commits a write or a "forget" to `main` exactly, authored
  by the person with `Via: Universal Exports` in the message, never to a fallback branch; a
  write whose `base` the file no longer holds is refused (`409`). A person's memory is theirs
  alone: the new action `assistant.memory.admin` on `config:assistant` reads another
  person's (`?person=<id>`), audited `assistant.memory.read`, and nobody changes another's;
  the person's own changes are audited `assistant.memory.write` and
  `assistant.memory.forget`. The app's **Ask Moneypenny** menu opens **Her memory of you**:
  list, open, edit and save, add, and forget after a confirmation, with Universal Exports'
  own words when it refuses.
- Outer Office's Roster page lists Moneypenny's settings (the `assistant` record),
  edited under its fixed key `assistant` and added only while there is none, and after an
  accepted edit shows each setting the stored record leaves looser than its default.
- One record of a Moneypenny conversation's exposure, `Conversation.exposed` in Universal
  Exports, carried to the Station as `AssistantJob::exposed`: every read tool sets it but
  `field_guide`, `profile` and `station_settings`, and the Station sets it (`POST
  /v1/station/assistant/exposure`, read with `GET`) when she reads a file her repository
  records as written while exposed (`.moneypenny/outside-text/`, one entry per file, which
  conversations add to at once without replacing each other's): a `Read` or `Edit` of it, a
  `Grep` or `Glob` whose search covers it, a call whose input cannot be read, or a
  `MEMORY.md` that imports it. Reading other files does not. Group and organisation
  repositories are trusted text and expose nothing.
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
  a refusal, then pushes to `CheckpointSpec::fallback`; with `CheckpointSpec::record` the
  commit also records each file it adds or changes in a directory, from what it commits, and
  `CheckpointSpec::exclude` leaves paths out. The `copy` safe house refuses a clone.
- Moneypenny sends and reads Cables for the person she talks to. New access actions
  `cable.send` and `cable.read` on `operation:<id>` (in `ai6::ACTIONS` and the profile's
  Operation grants) bound what she may do; each `AssistantJob` carries the person
  (`person_id`) and the Operations where they hold either (`cables`, at most 64,
  `AssistantCableGrant`). With any, the Station serves her conversation a second MCP server,
  `cables` (`AssistantMode::with_cables`, environment `AI6_ASSISTANT_CABLE_MCP_URL`/`_TOKEN`),
  whose `discover_agents`, `send_cable`, `read_cables`, `acknowledge_cable` and
  `reply_to_cable` name an Operation and act as her Agent for the person, under the Station's
  `cables.toml`, quotas (the sender quota per person) and fan-out. A Cable sent for a person
  carries `on_behalf_of` (`CablePrincipal::with_on_behalf_of`, `Cable::on_behalf_of`,
  `CableDelivery::sent_as` `"<agent> for <person>"`), reaches only Agents of its Operation,
  and its reply only the conversation that sent it; her conversations
  (`CableEndpoint::with_conversation_only`) are in no other audience. The relay writes its
  audit record (`cable.send`, the person as actor) before storing it and refuses the send when
  it cannot (`CableRelay::with_person_audit`); Universal Exports audits it too
  (`moneypenny.cable.sent`). Before every Cable call the Station asks Universal Exports
  (`GET /v1/station/assistant/exposure` with `operation`, which then also answers `send` and
  `read`) whether the person may still send or read in that Operation, and refuses without an
  answer; the endpoint's token expires with her MCP token. `CableRelay::inbox_from` reads one
  Operation's Cables and leaves the rest undelivered.
- A Cable body exposes her conversation too: the Station shows one only after Universal
  Exports has recorded the exposure (`POST /v1/station/assistant/exposure`). In an exposed
  conversation a Cable she sends or replies with is held by the Station and shown as a
  proposal (`EVENT_CABLE_PROPOSAL`) with its recipient, lifetime and
  text, whose digest (`AssistantCable::request_digest`) Universal Exports checks against what
  it shows; the person confirms it (checked again, audited `moneypenny.confirmed`, then sent
  through a `cable_confirm` job within the answer timeout) or cancels it (`cable_discard`).
  Held Cables live in the Station's memory for at most an hour and never survive the harness;
  at 32 held Cables or 32 waiting proposals a new one is refused rather than one dropped. A
  control job's answer is taken only from the Station it was given to.
- Outer Office uses the Outer Office icon for its web favicon, web app icons (with maskable
  versions) and Windows application icon. Moneypenny's chat shows her own icon as an avatar
  beside her messages (labelled "Moneypenny" for screen readers).
- Moneypenny's answer can be stopped: the app's **Stop** button, in place of Send while she
  answers, sends `{"type":"interrupt"}` on the chat socket, which acts only on the socket's
  own conversation and is a no-op (`stopping: false`) when nothing is being answered. A
  message no Station has accepted is withdrawn; otherwise Universal Exports queues an
  interrupt job (`AssistantJob::interrupt`) for the Station running her harness, whose
  desk calls the handler's interrupt (stream-json for Claude Code, `turn/interrupt` for
  Codex) and reports the turn as `interrupted` (`EVENT_INTERRUPTED`). The conversation
  shows a `notice` entry, the harness stays up for the next message, and she is told the
  person stopped her. The Station reports a message `accepted` before anything her harness
  says about it, and nothing of a turn whose `accepted` Universal Exports refused.
- `POST /v1/user/operations/{operation}/missions/{mission}/actions` answers `404` for a
  Mission nothing knows (no Session record, brief lease, queued brief of the Operation or
  board entry) before anything is audited or queued; the Headquarters Station takes only
  known Missions no other Station holds.
- On Unix, killing a `pipe` or `pty` Session also kills the processes that left its process
  group (`setsid`, `setpgid`): every process whose parent is in the tree is found and stopped
  before all are killed.
- The `pipe` terminal starts its program alone. On Unix the program leads a session of its
  own and inherits no descriptor but its standard streams; on Windows it inherits only its
  three standard handles (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`) and is inside its Job Object
  from its first instruction (`PROC_THREAD_ATTRIBUTE_JOB_LIST`), and a bare program name
  found on no `PATH` directory is refused.
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
  harness that checks for a terminal reads its input. Standard error goes through a pipe of
  its own and only its last 8 KiB of whole lines follow the output once it has ended
  (`stderr = "tail"`, the default, `stderr_tail_bytes`), so a JSON stream
  is never broken up by it; it can be merged into the output (`"merge"`) or dropped
  (`"drop"`). Kill takes the whole tree:
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
  `station.read` on the Station, and Outer Office shows it on the Station card. `StationConfig::with_safe_house_hygiene(SafeHouseHygiene)` names the
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
  diffs, and forms that draft a proposal); an invalid view is refused with the reason.
  Outer Office has an **Ask Moneypenny** chat panel on every page that renders them
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
  the inbox, the API and Outer Office show it. A clean proposal from an unexposed Session that
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
  list|release|refuse`, and Outer Office's Administration has a Held briefs tab that shows the
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
  `roster.remove` or `roster.refused`), and Outer Office's Administration has a Roster tab.
- Outer Office's Station settings set the minimum free memory and maximum CPU percent past
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
  gateway's Mission detail and Outer Office show the origin. `submit_brief` stores it,
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
  Outer Office's Account pools view; `pool.manage` lifts a limit (`ai6ctl pools clear-limit`,
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
  (default) or `"pack"` orders them. The board, placement previews, Outer Office and
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
  checkpoint branch), and Outer Office shows them on the Mission card.

- Theater provisioning. A Theater declares what Sessions need there
  (`[theater.ID.requires]`: `os`, `harnesses` such as `"claude>=2.1"`, `tools`, `disk_mb`)
  and how to install what is missing (`[theater.ID.provision]`: an ai6 build for its
  runner, provider libraries, an install command per program). The Station probes each
  POSIX Theater over its transport every five minutes and publishes the report at check-in
  (`TheaterSummary::requires`, `report`; `BoardTheater` likewise): placement rules out a
  Theater whose requirements are unchecked or unmet, and in field mode an Agent whose
  harness is missing there, each with the reason; Outer Office marks such a Theater and says
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
  and requests appear on the timeline. Outer Office has Green ink and Class authorisations
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
  interrupt` wait for the outcome; Outer Office offers the controls in each Mission card's
  menu with the reason any is unavailable. DipTel has the `queued` and `abandoned`
  states, and board Sessions carry `updated_at`, so a client finds a Mission's current
  Session.
- An Operation activity timeline. `GET /v1/user/operations/{operation_id}/timeline`
  (`audit.read`) pages newest first through lifecycle transitions, control outcomes,
  Cables sent and Sleeper authorizations from a new Registry activity log
  (`ai6::TimelineEvent`, the newest 10,000 kept) merged with the Operation's audit records,
  correlated by Station, Mission and Session, filterable by Mission and continued by an
  opaque cursor. Items are metadata only. Outer Office shows it in a Timeline view.

- A Station's identity defaults to `<host>-<random>`, created on first start and kept in
  `station-id` in the AI6 state directory, so a restarted daemon stays the same Station;
  `--station` or `AI6_STATION_ID` override it.
- Theaters: a Station reads `theaters.toml` (`[theater.<id>]` with `access = "local"` or an
  `ssh` host, plus labels), always reaches its own host as the implicit `local` Theater and
  publishes its Theaters at check-in. A Session's launch record carries `station_id` and
  `theater_id` separately. `GET /v1/stations` lists each
  Station's Theaters, and Outer Office shows them and the Station and Theater of each Session.
- Transports. A Station reaches a Theater through a transport, a new provider kind
  (`ai6_transport`, `ai6::directorates::q::transport`) with optional, capability-gated
  operations: open a terminal, run a command, read, write and list files, and forward a port
  from the host back to the Station. Each transport declares its `Capabilities`
  (`terminal`, `run`, `files`, `forward`, `os`, `arch`); a contract suite beside the kind
  checks any transport against what it declares. Theaters declare `agents` (`auto`, `local` or
  `remote-hands`), `egress` (`direct`, a proxy, `station` or `none`), `harnesses`, `workdir`
  and `access = { transport = { provider, config } }` for any other transport provider.
  Stations publish each Theater's capabilities and declared facts, and `GET /v1/stations`
  and Outer Office's Theater tooltips show them; a Theater that cannot be reached is shown
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
  Outer Office carry the mode.
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
  and Outer Office's Stations view has a placement preview panel.
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
  `ai6-diptel`, the gateway and Outer Office.

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
  by later attempts, shown on the board (`exposure`) and as an `EXPOSED` chip in Outer Office.
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
  `ai6ctl` and Outer Office, and `ai6ctl` lists a Session's exposure.

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
  `coqui-peer-tls` and `coqui-peer` crates. Stations pass the modern policy (TLS 1.3, SHA-2
  pins); the waiting list is bounded per source address, in total and by re-record time, a peer
  is listed only after it proved its key, and `ai6 peer waiting` shows each source address.
- Access kinds: Authenticator, Directory, Authorizer and Accounts as Guatiao kinds from the shared
  `coqui-access` crate, the first-match `RoleAuthorizer` from `coqui-roles`, and the providers
  `coqui_access_password`, `_key`, `_ldap`, `_proxy`, `_mtls` and `coqui_roles`, loaded as plugin
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
- Outer Office: the Flutter desktop and web operator app, with Stations and the
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
  `coqui_roles` provider takes `scope_kinds`, `roles` and `rules`. `coqui_access_password`,
  `coqui_access_key` and `coqui_access_mtls` are configured with
  `{"source": {"file": {"path": P, "format": "toml"}}}` (or `json`) over an `accounts`, `keys` or
  `peers` table;
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

### Fixed

- The `jira` Source reports a refused credential or a project it cannot see. Jira Cloud answers
  such a search with status 200 and no issues, so a revoked token used to look like no work, for
  as long as it stayed revoked. A poll that finds nothing now makes one more request to tell them
  apart. Flattened descriptions keep emoji, status lozenges and dates, a rejected comment or
  transition no longer blames the JQL, and a blank report is refused before any request, on
  `github` too. Found by running both providers against real services; their test fixtures are now
  recorded from that traffic ([Sources](https://jose-pr.github.io/ai6/missions/#sources)).
- A restricted Session over ACP now ends when an agent runs a tool it was refused, not only one
  it never asked about, and an agent cannot make a restricted tool pass by claiming another kind
  when it asks.
- A PostgreSQL connection now sets a keepalive interval, a tenth of its idle, unless its location
  gives one. Without it Windows sent probes 0 ms apart and reset any connection idle for 10
  seconds on a link slow enough that the answer had not arrived.
- The Stations view shows a Station to anyone holding `station.read` on it, even with no
  Operation's checkout. A control Station holds none, so the board said no Station had checked in.
- A Station's input poll no longer aborts every two seconds: the gateway holds it for `wait_ms`
  (`[publisher] input_wait`, 20 seconds by default, bounded by `max_poll_wait`) and answers `204`
  when nothing came.
- Idle Stations no longer poll Universal Exports in a loop while Moneypenny is off. The gateway
  holds the job poll for its wait (`[assistant.gateway] max_wait` bounds it), and a Station
  leaves at least `[assistant.desk] job_retry` between polls when the gateway answers early.

[Unreleased]: https://github.com/jose-pr/ai6/commits/main
