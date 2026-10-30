# Configuration

AI6 keeps two kinds of configuration apart:

- **A bootstrap file** on each host holds only what a process needs before it can reach
  anything else: where the configuration provider and the Registry are, the passwords to
  reach them, the host's own directories, its listener, and how it checks the provider
  libraries it loads. A Station and the `ai6` command read `ai6.cfg`, Universal Exports
  reads `unex.cfg`, and Outer Office reads `OO.cfg`.
- **A configuration provider** holds everything else as records: Operations, Agents and
  roles, covers, Theaters, loadouts and account pools, Cable settings, provider library
  pins, each Station's runtime settings and TLS policy, and Universal Exports' sign-in and
  role policy. Every Station of a deployment, and Universal Exports, read the same provider.

## Bootstrap files

### Where they are found

The first place that has a file wins; files are never merged, and with none every key is at
its default. The working directory is never searched: a daemon's working directory is
arbitrary, and a checkout could plant a file there.

| | `ai6.cfg` (Station, `ai6`, `ai6ctl`) | `unex.cfg` (`universal-exports`) |
| --- | --- | --- |
| 1 | `--config PATH`, which must exist | `--config PATH`, which must exist |
| 2 | `AI6_CONFIG`, which must exist | `UNEX_CONFIG`, which must exist |
| 3 | `ai6.cfg` in the user's configuration directory | `unex.cfg` there |
| 4 | `/etc/ai6/ai6.cfg`, `%ProgramData%\ai6\ai6.cfg` on Windows | `unex.cfg` there |

The user's configuration directory is `$XDG_CONFIG_HOME/ai6`, `%APPDATA%\ai6` on Windows,
else `~/.config/ai6`. `ai6 config path` prints the `ai6.cfg` in use, or `none (defaults)`.

A relative path in a bootstrap file is relative to the file's directory. A key a file does
not know is refused, naming it. A command-line flag beats the file, which beats the default;
the only environment variables are `AI6_CONFIG` and `UNEX_CONFIG`, and the ones a secret
reference names.

### Formats

A file's extension chooses its format: `.toml`, `.json`, `.yaml` or `.yml`. A `.cfg` file names
its format on its first line:

- `#!json`, `#!yaml` (or `#!yml`) and `#!toml` name the format by the line's last word, after
  any `/` (so `#!/usr/bin/env yaml` is YAML), ignoring case and leading dots. The line itself
  is not part of the document.
- A document starting `%YAML` is YAML.
- Anything else is TOML. Nothing is guessed from the content: a `.cfg` starting `{` is TOML,
  and fails as TOML.
- A name no format answers to is an error that lists `json`, `toml` and `yaml`.

`OO.cfg` follows these rules today. `ai6.cfg`, `unex.cfg` and the provider's family files
named `.cfg` are read as TOML until the shared reader of the first line is in place; give them
a `.json` or `.yaml` extension for those formats.

### Secrets

A password or token is never a value the file has to hold. It is a secret reference:

```toml
password = { file = "db-password" }   # a file; one trailing newline is dropped
password = { env = "AI6_DB_PASSWORD" } # an environment variable
password = "the value itself"          # inline
```

A file is the default and the strongest form. A variable is inherited by every process the
reader starts, and an inline value is copied and committed with the file, so both are
**weakenings**: printed at start, and sent at check-in as `bootstrap_weakened`, as is a
limit looser than its default ([Limits](#limits)). They are allowed. A secret's value never
appears in a message, a log or `Debug` output.

### `ai6.cfg`

```toml
station = "hq-1"                      # default: made up once and kept in the state directory
state_dir = "/var/lib/ai6"            # default: the platform's state directory
listen = "127.0.0.1:7081"

[provider]
location = "."                        # a directory, sqlite:PATH or postgres://...
password = { file = "db-password" }   # when a SQL location gives none
poll = 5                              # seconds between checks for changes; 0 turns reloading off
loadout_bytes = 1048576               # and the other limits under Limits below

[registry]
location = "postgres://db/ai6"        # default: the provider's database when it is SQL,
                                      # else registry/ in the state directory
password = { file = "db-password" }   # default: the provider's, when the Registry is its database
headquarters_lease = 60               # seconds; also sets the SQL connections' timeouts
sqlite_busy_timeout = 30              # seconds a SQLite connection waits for a lock

[registry.limits.sessions]            # what the Registry keeps and accepts: Limits below
records = 100000

[identity]
dir = "/var/lib/ai6"                  # identity.pem and peers/; default: the state directory

[plugins]
paths = ["/usr/lib/ai6"]              # directories or library files to scan for providers
profile = "strict"                    # strict, report or off; default strict with pins, report without

[secrets]
dir = "secrets"                       # where loadout secret files are read; default secrets/ beside this file

[unex]
url = "https://ai6.example"           # Universal Exports; ai6ctl reads this and nothing else here
token = { file = "unex-token" }       # the Station's credential; without it the Station publishes nothing
```

The provider's location defaults to the directory holding `ai6.cfg`, else the user's
configuration directory. The platform's state directory is `%LOCALAPPDATA%\AI6` on Windows,
else `$XDG_STATE_HOME/AI6` or `~/.local/state/AI6`.

`ai6 serve` takes these flags over the file: `--config`, `--provider`, `--registry`,
`--state-dir`, `--station`, `--listen`, `--plugin-path` (adds to `[plugins] paths`) and
`--plugin-profile`. `ai6 identity`, `peer`, `sleeper`, `quarantine`, `theater provision` and
`config` take `--config`. The bootstrap file is read at start only.

### `unex.cfg`

```toml
listen = "127.0.0.1:7082"             # loopback, behind the HTTPS reverse proxy
public_origin = "https://ai6.example" # required
station_token = { file = "station-token" }   # required: the Stations' credential
proxy_secret = { file = "proxy-secret" }     # what a trusted reverse proxy sends in X-AI6-Proxy-Secret
state_dir = "/var/lib/unex"           # verified provider libraries, and a default Registry

[provider]                            # as in ai6.cfg: the same provider the Stations read
[registry]                            # as in ai6.cfg
[plugins]                             # as in ai6.cfg
```

`universal-exports` takes `--config`, `--provider`, `--registry`, `--listen`,
`--public-origin`, `--plugin-path` and `--plugin-profile` over the file.

### Limits

How much the Registry keeps and accepts, and how much of a document the configuration
provider reads, are bootstrap settings: both are needed before any provider record is read,
by Stations and Universal Exports alike. `unex.cfg` takes the same `[provider]` and
`[registry]` keys as `ai6.cfg`. Every key has a default, and a table may name only the keys it
changes.

A count or size is a whole number; `null`, or the string `"none"` in TOML, which has no
null, takes the bound away. A number of seconds may be fractional. Only nonsense is refused:
a negative number, or a word other than `none`. A value looser than its default (larger, or
no bound) is a weakening, reported like an inline secret:
`registry.limits.sessions.records none is looser than the default 100000`.

```toml
[provider]
loadout_bytes = 2097152               # a larger loadout document; reported as a weakening

[registry]
sqlite_busy_timeout = 10              # tighter than the default 30

[registry.limits.sessions]
records = 50000

[registry.limits.audit]
records = "none"                      # keep every audit record; reported as a weakening
```

`[provider]`:

| Key | Default | What |
| --- | --- | --- |
| `loadout_bytes` | 1 MiB | Bytes of the loadout document (`loadout`, `sandbox`, `mcp`, `secret` and `pool` records together) |
| `loadout_value_chars` | 4,096 | Characters of one text value in the loadout document |
| `access_policy_bytes` | 1 MiB | Bytes of the access policy (the `access` record) |

`[registry]`:

| Key | Default | What |
| --- | --- | --- |
| `headquarters_lease` | 60 | Seconds the Headquarters lease lasts between renewals; a PostgreSQL statement, lock wait and idle transaction end after a quarter of it, a connection attempt after a sixth |
| `sqlite_busy_timeout` | 30 | Seconds a SQLite connection waits for a lock another holds |

Each `[registry.limits.*]` table bounds one record family. A Station's
`max_concurrent_missions` setting is refused past `[registry.limits.stations]
mission_capacity`. A logbook's whole size follows from its four keys.

`[registry.limits.accounts]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 256 KiB | Bytes of one account's record |
| `claims` | 1,024 | Live claims on one account; past it the oldest is dropped |
| `windows` | 8 | Quota windows kept per account |
| `kept_days` | 8 | Days of token counts kept per account |

`[registry.limits.agents]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 1 MiB | Bytes of one Agent record |
| `operations` | 128 | Operations one Agent record tallies |
| `clearances` | 8 | Clearances one Operation of an Agent record tallies |
| `counted` | 256 | Mission outcomes remembered per Agent, so a repeated report is counted once |
| `model_history` | 16 | Model changes kept per Agent |
| `key_bytes` | 512 | Bytes of an outcome report's key |
| `agent_bytes` | 256 | Bytes of an outcome report's Agent, Operation and model |

`[registry.limits.agent_roles]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 32 KiB | Bytes of one Agent role record |
| `records` | 256 | Agent roles per Operation |

`[registry.limits.approvals]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 16 KiB | Bytes of one approval record |
| `pending` | 1,000 | Pending approval requests |
| `class_authorizations` | 1,000 | Class authorizations |
| `mission_requests` | 64 | Requests one Mission may have |
| `reason_bytes` | 2,048 | Bytes of a request's reason |
| `note_bytes` | 1,024 | Bytes of a decision's note |
| `signer_bytes` | 256 | Bytes of a signer's name |
| `idempotency_key_bytes` | 256 | Bytes of a request's idempotency key |
| `class_entries` | 64 | Step kinds, Agents, roles and proposed roles each class authorization may name |
| `pattern_bytes` | 256 | Bytes of a class authorization's target pattern |
| `name_bytes` | 128 | Bytes of each Agent or role a class authorization names |

`[registry.limits.audit]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 16 KiB | Bytes of one audit record |
| `records` | 10,000 | Audit records kept |
| `results` | 500 | Records one listing may ask for |

`[registry.limits.briefs]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 1 MiB | Bytes of one brief |
| `records` | 1,000 | Queued briefs |
| `provenance_bytes` | 16 KiB | Bytes of a brief's provenance |

`[registry.limits.cables]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 2 MiB | Bytes of one Cable record |
| `records` | 10,000 | Cable records the Registry holds |
| `body_bytes` | 16 KiB | UTF-8 bytes of a Cable's body the Registry stores |
| `max_ttl` | 86,400 (a day) | Seconds a Cable the Registry stores may live |

What a Station's relay accepts from a sender is the `cables` record's
([Families](#families)); the Registry's bounds above apply to what is stored, whoever wrote
it.

`[registry.limits.clearance]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 16 KiB | Bytes of one clearance grant |
| `grants` | 10,000 | Clearance grants |
| `text_bytes` | 512 | Bytes of a grant's signer, approval, model and reason |

`[registry.limits.files]`: how long the file Registry waits. The SQL Registry ignores it.

| Key | Default | What |
| --- | --- | --- |
| `rename_retry` | 2 | Seconds a record write retries replacing a file another process holds open |
| `lock_wait` | 30 | Seconds a write waits for the Registry's directory lock |

`[registry.limits.held]`: A held record may be 64 KiB, room for its verdict, larger than a brief's `[registry.limits.briefs] record_bytes`.

| Key | Default | What |
| --- | --- | --- |
| `per_operation` | 1,000 | Briefs one Operation may hold |
| `records` | 10,000 | Briefs held across the Registry |
| `results` | 500 | Held briefs one listing may ask for |
| `clearance_bytes` | 4,096 | Bytes of a stored clearance, the finding kinds a brief was released from |
| `reason_bytes` | 1,024 | Bytes of the reason a held brief is refused for |
| `actor_bytes` | 256 | Bytes of who released or refused a held brief |

`[registry.limits.logbook]`: A whole logbook's size is bounded by these four.

| Key | Default | What |
| --- | --- | --- |
| `entries` | 20 | Reports kept per logbook, and handed to the next round |
| `summary_bytes` | 4 KiB | Bytes of a report's summary |
| `findings` | 32 | Findings per report |
| `finding_bytes` | 1,024 | Bytes of one finding |

`[registry.limits.proposals]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 1 MiB | Bytes of one Mission proposal |
| `records` | 1,000 | Pending Mission proposals per Operation |
| `results` | 500 | Proposals one listing may ask for |
| `idempotency_key_bytes` | 256 | Bytes of a proposal's idempotency key |

`[registry.limits.quarantine]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 16 KiB | Bytes of one quarantine record |
| `records` | 10,000 | Quarantine records |
| `incidents` | 256 | Incidents kept per Agent |
| `keep_incidents` | 604,800 (a week) | Seconds an incident is kept |

`[registry.limits.sessions]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 64 KiB | Bytes of one Session record |
| `records` | 100,000 | Session records |
| `id_bytes` | 128 | Bytes of a Session ID |
| `exposures` | 32 | Exposures recorded per Session |
| `exposure_source_bytes` | 128 | Bytes of an exposure's source |
| `recent_events` | 20 | Recent event lines kept per Session |
| `recent_event_chars` | 300 | Characters kept of each recent event line and of a lost Session's reason |

`[registry.limits.sleepers]`

| Key | Default | What |
| --- | --- | --- |
| `window_age` | 86,400 (a day) | Seconds a scheduled window may be late and still fire; an older one is skipped |
| `prompt_bytes` | 16 KiB | Bytes of a Sleeper's prompt |
| `lookback_days` | 366 | Days a schedule is searched back for its latest window |

`[registry.limits.sources]`

| Key | Default | What |
| --- | --- | --- |
| `item_text_bytes` | 64 KiB | Bytes of a Source item's text |
| `report_attempts` | 5 | Times reporting a finished brief to its Source is tried before it is dropped |

`[registry.limits.stations]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 1 MiB | Bytes of one Station record |
| `records` | 1,000 | Station records |
| `settings_bytes` | 4,096 | Bytes of a Station's settings record |
| `mission_capacity` | 1,024 | Missions a Station's settings may let it run at once |
| `gone_dark_after` | 45 | Seconds after its last check-in that a Station counts as dark |

`[registry.limits.timeline]`

| Key | Default | What |
| --- | --- | --- |
| `records` | 10,000 | Events kept; the oldest are dropped past it |
| `record_bytes` | 8,192 | Bytes of one event |
| `id_bytes` | 128 | Bytes of an event's ID |
| `name_bytes` | 64 | Bytes of an event's kind, action and detail keys |
| `text_bytes` | 256 | Bytes of an event's Operation, Station, Mission, Session, actor and detail values |
| `detail_entries` | 8 | Detail entries per event |

`[registry.limits.tokens]`

| Key | Default | What |
| --- | --- | --- |
| `record_bytes` | 16 KiB | Bytes of one access token record |

`[registry.limits.usage]`

| Key | Default | What |
| --- | --- | --- |
| `ledger_bytes` | 1 MiB | Bytes of one Mission's usage ledger |
| `kept_days` | 8 | Days of usage kept |

### `ai6ctl`

`ai6ctl` has no file of its own. `--timeout SECONDS` (default 5, fractions allowed, `0` for no
limit) bounds each HTTP request and the control commands take `--poll-interval SECONDS` (default
0.5). Its Universal Exports origin is `--server`, else `UNEX_URL`,
else `[unex] url` of the `ai6.cfg` found as above (`--config` included), else
`http://127.0.0.1:7082`. Its user token comes only from `UNEX_ACCESS_TOKEN`; it never reads the
Station's `[unex] token`.

### `OO.cfg`

Outer Office on desktop reads `OO_CONFIG`, else `OO/OO.cfg` in the platform's configuration
directory: `%APPDATA%\OO`, `~/Library/Application Support/OO` on macOS, `$XDG_CONFIG_HOME/OO`
or `~/.config/OO`. On the web it reads `OO.cfg` beside `index.html`, as read-only deployment
defaults; a missing or unreadable one is logged and the defaults apply.

```toml
[unex]
url = "https://ai6.example"           # desktop; on the web the page's own origin is the API

[appearance]
theme = "night-ops"                   # night-ops, black-ops, dusk, classic, light, red-file or system

[chat]                                # Moneypenny's chat; logical pixels, every key optional
min_width = 320                       # the docked chat's narrowest drag
max_width = 720                       # and widest
width = 400                           # its width until the person drags it
width_step = 16                       # an arrow key on the drag handle moves it this far
min_page_width = 360                  # the page keeps at least this much beside it
text_width = 960                      # expanded: her text keeps to a column this wide
window_width = 520                    # desktop: her own window's size when popped out
window_height = 760
transition_ms = 200                   # how long the chat takes to move; 0 for none

[routes]
text_bytes = 128                      # the longest text filter a page link may carry, in bytes

[refresh]                             # seconds between a page's asks; 0 turns that refresh off
sessions = 3                          # the board of Sessions, Stations and Missions
overview = 10                         # the overview's recent activity
timeline = 10                         # an Operation's timeline
pools = 15                            # the account pools

[controls]                            # following a Mission control until the Station reports
wait = 10                             # seconds; 0 returns once the control is queued
poll = 0.5                            # seconds between asks for its status

[pages]
timeline = 50                         # items in one page of the timeline
audit = 100                           # records in the audit log

[terminal]                            # the live Session terminal
scrollback_lines = 10000
connect_timeout = 10                  # seconds the socket may take to open
retry_delay = 1                       # seconds before the first reconnect; each next one doubles
retry_max_delay = 16                  # seconds, the longest wait between reconnects

[limits]                              # characters a text field takes; 0 takes the bound away
mission_title_chars = 512
mission_prompt_chars = 65536
reason_chars = 512                    # the reason for lowering an Agent's clearance

[assets]
cdn = "none"                          # web only: none, fonts or all (read by the page's bootstrap)
```

Every value of `[refresh]` is a number of seconds, whole or fractional, `0` or more; `[controls]`
`wait` likewise, and the other seconds are more than 0. The counts of `[pages]` and
`scrollback_lines` are whole numbers of 1 or more, and `[limits]` takes whole numbers of 0 or
more. An unknown table or key, or a value that is not what its key takes, refuses the file. These
are what Outer Office asks of Universal Exports and how it paces itself; what the gateway allows
is the `unex` record's ([Universal Exports limits](#universal-exports-limits)), and Moneypenny's
view bounds come from the gateway's `[assistant.views]`.

The build define `OO_API` beats `[unex] url`, which beats the loopback default. A theme the
person chooses (`oo.theme`, kept on the device or in the browser) beats `[appearance] theme`,
which beats Night Ops; an unknown theme id is logged and read as the default. In `[chat]`
each value is a positive number (`transition_ms` a whole one, 0 allowed) and `width` lies
between `min_width` and `max_width`, or the file is refused; a width the person drags to
(`oo.chat.<person ID>`) beats `width` and is kept within the limits. The system's reduced
motion setting turns transitions off whatever `transition_ms` says.
`[routes] text_bytes` (1 or more, default 128) bounds the `text` filter of a page link: a web
URL or one of Moneypenny's links with a longer one is refused like any other bad link. Keep it
equal to the `unex` record's `route_text_bytes`, or her links that one accepts and this one
does not show as a refused view. The operator
menu on desktop writes `[unex] url` back in the format the file declares, keeping its `#!`
line; a YAML file keeps its comments, a TOML or JSON one does not. The address applies at the
next start.

## Configuration providers

A provider holds records of each **family**, each record either **global** or one
**Station's** own. A Station reads its own records over the global ones, key by key; a
family with one record (a singleton, keyed `settings`) takes the Station's whole when it has
one. The provider hands out only configuration that passed every check: an invalid one is
refused, and a running Station keeps the last good one.

### Families

| Family | File | Station's own | Singleton | Applies | Holds |
| --- | --- | --- | --- | --- | --- |
| `operation` | `operations` | no | no | live | `[operation.<id>]`, sleepers included |
| `agent`, `role` | `agents` | no | no | live | `[agent.<codename>]`, `[role.<id>]` |
| `assistant` | `agents` | no | yes | live | `[assistant]`, Moneypenny's settings |
| `cover` | `covers` | no | no | live | `[cover.<name>]` |
| `theater` | `theaters` | yes | no | restart | `[theater.<id>]` |
| `loadout`, `sandbox`, `mcp`, `secret`, `pool` | `loadouts` | yes | no | live | `[loadout.<agent>]`, `[sandbox.<name>]`, `[mcp.<name>]`, `[secret.<name>]` (references, never values), `[pool.<id>]` |
| `cables` | `cables` | yes | yes | restart | the whole file: grants, inbound, channels, limits |
| `plugin` | `plugins` | yes | no | restart | `[[library]]` entries, keyed by their `file` |
| `station` | `station` | yes | yes | restart | the whole file: the Station's runtime settings |
| `tls` | `tls` | yes | yes | restart | the whole file: the Station TLS policy |
| `access` | `access` | no | yes | live, in Universal Exports | the whole file: `[role.<name>]` and `[[rule]]` |
| `unex` | `unex` | no | yes | restart | the whole file: Universal Exports' sign-in, limits and assistant |

A Station polls its provider every `[provider] poll` seconds. A family that applies live
reaches requests, the scheduler and new Sessions at once; running Sessions keep what they
started with. A changed family that applies at restart is logged and listed at check-in in
`config.needs_restart` by name. A revision that fails its checks is refused once: logged,
audited as `config.refused`, and reported at check-in.

The `station` record takes `max_concurrent_missions`, `stall_timeout`, `loop_timeout`,
`loop_min_lines`, `blocked_after`, `max_runtime`, `history`, `normalize_digits`, `roles`
(`control`, `execution`), `labels`, `terminal`, `resources_every`, `safe_house_caches`,
`safe_house_cache_age` and `partial_line_bytes` (bytes of a terminal line the watchdog holds
before it counts as a line, default 4,096); seconds may be fractional. Its tables bound
and pace the Station's work ([Station runtime limits](#station-runtime-limits)), and its
`[providers.<id>]` tables configure providers. The `tls` record is a coqui-peer-tls policy
document (`profile = "modern" | "intermediate" | "legacy"`, `cipher_profiles`, `tls_versions`
and the rest), read over the library's default; whatever it gives up against that default is
logged and sent at check-in as `tls_weakened`. The `unex` record takes `authenticator`,
`directory`, `authorizer` and their `*_config` objects (handed to the provider as they are),
`proxy_user_header`, `proxy_lookup_by`, `login_tries_per_address`, `login_tries_per_name`,
`client_address_header`, `access_cache_ttl`, `access_cache_negative_ttl`,
`access_cache_entries`, `assistant` (default `true`), `assistant_agent` (default
`moneypenny`) and `route_text_bytes` (default 128, the longest `text` filter of an Outer
Office page link Moneypenny's views and page context may carry). A credential written inline in one of those configurations is reported as a
weakening: name a file there instead.

### Universal Exports limits

`unex.cfg` takes one more key, `proxy_secret_min_bytes` (default 32): the fewest bytes the
reverse-proxy secret may hold. A smaller value is a weakening.

The `unex` record's tables bound and pace the gateway, and apply at restart. They follow the
[Limits](#limits) rules: a count or size is a whole number and `null` or `"none"` takes the
bound away, seconds may be fractional, a value looser than its default is reported at start,
and only a zero where nothing could pass is refused. Flat keys: `provider_config_depth`
(default 64) is how deeply nested a provider configuration may be.

```toml
[gateway]
sessions = 32                         # live Sessions, instead of 16
frame_bytes = "none"                  # no bound on a frame (a weakening)

[controls]
pending_per_station = 8
```

`[gateway]`

| Key | Default | What |
| --- | --- | --- |
| `sessions` | 16 | Live provider Sessions |
| `clients_per_session` | 12 | Simultaneous browser connections to one Session |
| `scrollback_events` | 64 | Value frames kept per Session for a client that joins late |
| `frame_bytes` | 256 KiB | Bytes of one WebSocket or ingress frame |
| `input_frames_per_second` | 40 | Browser input frames accepted per connection per second |
| `input_queue` | 64 | Capacity of the queue between the gateway and its Stations, at least 1 |
| `fanout` | 8 | Capacity of each per-Session viewer queue, at least 1 |
| `buffer_budget_bytes` | 512 MiB | Bytes the gateway may hold in buffers; the configuration is refused when its limits could exceed it |
| `client_slots` | 100,000 | Connections the gateway keeps slots for; refused when Sessions times connections could exceed it |
| `attach_token_ttl` | 300 | Seconds an attach token lives, more than 0 |
| `write_lease_ttl` | 30 | Seconds a terminal write lease lives, more than 0 |
| `station_token_min_bytes` | 32 | Characters the Station token holds at least; smaller is looser |
| `origin_bytes` | 2,048 | Bytes of the public origin |

`[controls]`

| Key | Default | What |
| --- | --- | --- |
| `controls` | 1,024 | Controls held for status reads and replays; the oldest finished goes first |
| `pending_per_station` | 64 | Controls one Station may have open |
| `ttl` | 120 | Seconds an open control waits for its Station before it expires |
| `poll_wait` | 1 | Seconds a Station's poll waits when it names no wait |
| `max_poll_wait` | 25 | The longest wait a Station's poll may ask for, in seconds |
| `idempotency_key_bytes` | 128 | Bytes of an idempotency key |
| `reason_chars` | 512 | Characters of an operator's reason; longer text is cut |

`[timeline]`

| Key | Default | What |
| --- | --- | --- |
| `page` | 50 | Items in a page when the request names no limit, at least 1 |
| `max_page` | 200 | The most items a page may ask for |
| `text_chars` | 256 | Characters of one item's text; longer text is cut |

`[login]` (the tries allowed are the flat `login_tries_per_address` and `login_tries_per_name`)

| Key | Default | What |
| --- | --- | --- |
| `name_bytes` | 64 | Bytes of an account name the tries are counted under |
| `forget_every` | 256 | Tries between sweeps that forget old counters, at least 1 |
| `address_window` | 60 | Seconds one client address's tries are counted over |
| `name_window` | 600 | Seconds one account name's tries are counted over |

`[text]`

| Key | Default | What |
| --- | --- | --- |
| `agent_reason_chars` | 512 | Characters of the reason that lowers an Agent's clearance |
| `green_ink_max_grant` | one year | The longest a green ink grant or a clearance may last, in seconds |
| `green_ink_note_chars` | 1,024 | Characters of a green ink note |
| `held_reason_chars` | 1,024 | Characters of the reason for a held brief's decision |
| `roster_target_bytes` | 512 | Bytes of a roster record's target line; longer text is cut |
| `roster_key_bytes` | 128 | Bytes of a roster record's key |

### Station runtime limits

The `station` record's tables bound and pace one Station's work, and apply at restart like
the rest of the record. They follow the [Limits](#limits) rules: a count or size is a whole
number or `"none"` (no bound); seconds may be fractional; a key left out keeps its default.
Only what cannot work is refused: a queue, page or attempt count of 0. Looser values are not
reported as weakenings.

```toml
# stations/hq-1/station.toml
[scheduler]
check_in = 5                          # check in every 5 seconds instead of 15

[publisher]
queue = 256                           # more lifecycle events held while Universal Exports is slow

[secrets]
max_lease_ttl = 900                   # no lease, green ink or not, outlives 15 minutes

[providers.pty]                       # the pty terminal's configuration when nothing else gives one
scrollback = 2000
```

`[providers.<id>]` is the Guatiao configuration a Station hands provider `<id>` when it builds
it with none of its own: handlers, terminals and safe houses. Transports, Sources and Theater
providers take theirs from the `theater` and `operation` records. A Station hands its field
runners the `[runner]` table and the `[providers.<id>]` tables
([The field runner](theaters.md#the-field-runner)).

`[scheduler]`

| Key | Default | What |
| --- | --- | --- |
| `event_queue` | 256 | Events of one Session queued for its event pump; past it the oldest is dropped |
| `interval` | 0.5 | Seconds between scheduler rounds |
| `retry_delay` | 5 | Seconds before a Mission whose start failed is tried again |
| `check_in` | 15 | Seconds between the Station's check-ins with the Registry |
| `intake_attempts` | 3 | Times a brief is placed before it is filed as failed |
| `role_catalog_bytes` | 512 KiB | Bytes of the Agent role catalog and of the placement policies a check-in carries; past it, each is left out |
| `shutdown_wait` | 2 | Seconds a stopping Station waits for each of its loops |

`[publisher]`: publication to Universal Exports.

| Key | Default | What |
| --- | --- | --- |
| `request_timeout` | 2 | Seconds one request may take |
| `queue` | 64 | Lifecycle and terminal events held for publication; when full, publication stops and is reported unhealthy |
| `terminal_bytes` | 1 MiB | Bytes of terminal output in one published frame |
| `pre_bind_terminal_bytes` | 1 MiB | Bytes of terminal output held for a Session not yet bound to its Mission |
| `input_retry_delay` | 0.1 | Seconds before a failed input or control poll is tried again |
| `control_wait` | 1 | Seconds Universal Exports holds a control poll open |
| `control_queue` | 16 | Mission controls queued for the scheduler |
| `control_report_attempts` | 3 | Times the outcome of a control is reported |

`[sources]`

| Key | Default | What |
| --- | --- | --- |
| `poll` | 60 | Seconds between polls of a Source whose `interval_secs` is not set |
| `min_poll` | 10 | The shortest interval a Source is polled at, in seconds; a smaller `interval_secs` is raised to it, and 0 takes the floor away |
| `report_every` | 15 | Seconds between rounds reporting finished briefs to their Sources |
| `sleeper_every` | 20 | Seconds between rounds firing Sleepers |

`[transports]`

| Key | Default | What |
| --- | --- | --- |
| `reprobe_after` | 60 | Seconds before a Theater the Station could not reach is probed again |
| `report_every` | 300 | Seconds between looks over a Theater for its requirements and ambient credentials |

`[recovery]`

| Key | Default | What |
| --- | --- | --- |
| `first_reattach_delay` | 5 | Seconds before the first try to re-attach to a Session; each failed try doubles it |
| `max_reattach_delay` | 60 | The longest wait between tries, in seconds |
| `give_up_tries` | 10 | Tries at a Registry step a concurrent renewal can refuse, when the Station gives a lease up |
| `notes_every` | 15 | Seconds between writes of a Session's recent events to its record |

`[vetting]`

| Key | Default | What |
| --- | --- | --- |
| `findings` | 16 | Findings a verdict keeps |
| `review_work` | 50 | Other work a review brief lists for the reviewer |
| `detail_bytes` | 256 | Bytes of a finding's detail, of the lines a review brief quotes, and of a vetting timeline detail |
| `phrases` | 256 | Manipulation phrases an Operation may screen for; past it, or past `phrase_bytes`, screened briefs are held |
| `phrase_bytes` | 256 | Bytes of one such phrase |
| `rate_window` | 3,600 | Seconds of the rolling window hourly rates are counted over |
| `proposals` | 500 | Pending Mission proposals looked at per Operation each round, within the Registry's `[registry.limits.proposals] results` |
| `proposal_request_ttl` | 3,600 | Seconds the green ink request a class authorisation approves for a proposal lasts |
| `proposal_settle` | 30 | Seconds a proposal waits before M promotes it on its own |
| `expiry_sweep` | 60 | Seconds between looks at rate holds for expiry |

`[accounts]`: pool accounts.

| Key | Default | What |
| --- | --- | --- |
| `reset_grace` | 15 | Seconds past a reported reset before a Session parked on a limit resumes |
| `unknown_use_permille` | 500 | Thousandths used an account whose provider reported nothing counts as, 0 to 1,000 |
| `claim_ttl` | 60 | Seconds a claim on an account lasts unless the Station renews it at check-in |
| `limit_horizon` | 691,200 (8 days) | The longest a limited account is kept out, in seconds, whatever reset time its provider reports |
| `login_timeout` | 60 | Seconds a pool account's login command may take |

`[approvals]`: green ink steps.

| Key | Default | What |
| --- | --- | --- |
| `kind_bytes` | 32 | Bytes of the kind of a step an Agent asks about |
| `target_bytes` | 256 | Bytes of its target |
| `classify_nesting` | 4 | How deep nested shells (`sh -c "sh -c ..."`) and git aliases are followed when a command is read; the harness gate gets it in `AI6_GREEN_INK_NESTING` |

`[harness]`: a working directory's harness configuration under `harness.project_settings =
"pinned"`, and a skill Moneypenny's desk installs.

| Key | Default | What |
| --- | --- | --- |
| `manifest_file_bytes` | 1 MiB | Bytes of one file |
| `manifest_bytes` | 16 MiB | Bytes of all of them |
| `manifest_files` | 4,096 | Files |

`[provision]`: probing and provisioning Theaters.

| Key | Default | What |
| --- | --- | --- |
| `probe_timeout` | 60 | Seconds the requirements probe, and a resources sample, may take |
| `command_timeout` | 120 | Seconds one provisioning command may take |
| `name_bytes` | 64 | Bytes of a required program's name; a longer one fails the probe |
| `version_chars` | 200 | Characters of a program's version kept from the probe |

`[secrets]`

| Key | Default | What |
| --- | --- | --- |
| `lease_ttl` | 3,600 | Seconds the leases of a Session's loadout and pool account secrets last |
| `max_lease_ttl` | 3,600 | The longest lease, in seconds; a green ink secret's lease lasts as long as its approval, up to it |

`[terminal_output]`

| Key | Default | What |
| --- | --- | --- |
| `json_line_bytes` | 64 MiB | Bytes of one line of a JSON harness's output held while the rest arrives |
| `csi_bytes` | 64 | Bytes of one terminal control sequence before it is read as text |

`[green_ink]`

| Key | Default | What |
| --- | --- | --- |
| `gate_timeout` | 30 | Seconds the harness gate's exchange with the Station may take; the gate gets it in `AI6_GREEN_INK_TIMEOUT` |

`[detached]`: harnesses run detached on another host, over its transport.

| Key | Default | What |
| --- | --- | --- |
| `poll` | 0.2 | Seconds between reads of the harness's output |
| `script_timeout` | 30 | Seconds one of the wrapper's scripts may take |
| `name_chars` | 64 | Characters of the Mission's name in the wrapper's file names |

`[runner]`: field runners, and the Station's side of them.

| Key | Default | What |
| --- | --- | --- |
| `page` | 256 | Events one read of a runner returns |
| `max_wait` | 30 | The longest a read of a runner waits for a new event, in seconds |
| `keep_ended` | 604,800 (a week) | Seconds an ended Session is kept by its runner |
| `refused_body_drain_bytes` | 64 KiB | Bytes of a refused request's body a runner, or the Station's own API, reads before it answers |
| `cable_request_bytes` | 4 MiB | Bytes of one request through a runner's Cable relay |
| `cable_connect_timeout` | 10 | Seconds a runner's Cable relay may take to reach the Station |
| `read_wait` | 1 | Seconds the Station's read of a runner waits for an event |
| `call_output_bytes` | 32 MiB | Bytes of output an `ai6 runner call` over a transport may return |
| `connect_timeout` | 5 | Seconds connecting to a runner may take |
| `call_margin` | 30 | Seconds a request to a runner may take past `max_wait`, and starting one over a transport past `start_wait` |
| `start_wait` | 30 | Seconds a started runner has to answer |

`[audit]`

| Key | Default | What |
| --- | --- | --- |
| `names_detail_chars` | 240 | Characters of the secret names a credential timeline event lists |
| `target_bytes` | 512 | Bytes of a refused configuration's audit target |

`[theaters]`

| Key | Default | What |
| --- | --- | --- |
| `ephemeral_name_bytes` | 100 | Bytes of an ephemeral Theater's name; a longer one is cut and given a digest |

`[theater_tools]`: the `theater_*` tools of a remote-hands Session.

| Key | Default | What |
| --- | --- | --- |
| `output_bytes` | 256 KiB | Bytes of each stream of a command's output `theater_run` returns |
| `read_bytes` | 256 KiB | Bytes `theater_read` returns |
| `timeout` | 120 | Seconds a `theater_run` command may take unless it asks for another |
| `max_timeout` | 3,600 | The longest a command may ask for, in seconds |
| `git_timeout` | 30 | Seconds each git command the tools run for green ink may take |

`[head_of_station]`

| Key | Default | What |
| --- | --- | --- |
| `listed_changes` | 20 | Changes of a kept safe house listed in one report |
| `survey_entries` | 500,000 | Directory entries one survey of a safe house walks before it reports a partial size |

The `cables` record takes `grants`, `inbound` and `channels`, and the relay's limits:
`max_fanout` (Agent Sessions one broadcast may reach, default 100), `sender_rate` and
`sender_burst` (sends a second and burst per sending Agent, 2 and 20), `scope_rate` and
`scope_burst` (per target scope, 10 and 50), `body_bytes` (UTF-8 bytes of a Cable's body,
16 KiB; `"none"` takes the bound away), `max_ttl` (the longest lifetime a sender may ask for,
in seconds, 86,400; `"none"` for no bound) and `default_ttl` (the lifetime of a Cable that
asks for none, 900, within `max_ttl`). Each must be positive. Universal Exports reads
`body_bytes` and `default_ttl` when it starts, to refuse a Cable Moneypenny proposes with a
longer body and to tell the person how long one that names no lifetime lasts.

### Moneypenny's limits

Three tables of the `assistant` record bound and pace Moneypenny. They follow the
[Limits](#limits) rules: a count or size is a whole number or `"none"` (no bound); seconds may
be fractional; a key left out keeps its default. Each bound looser than its default (larger,
or no bound; for `memory_import_depth`, smaller) is a weakening, logged by every Station and
listed at its check-in with the record's other weakenings:
`assistant.desk.live none is looser than the default 4`. The keys that only pace work
(`token_margin`, the waits, retries and margins, `report_attempts`, `unlocked_stale`,
`token_reuse_margin`, `sweep_every`, `shared_base_wait`) are never weakenings. Only what
cannot work is refused: a message, context, event count, job size, conversation, live
conversation or socket bound of 0, no report attempt, an event buffer of 0, a sweep every 0
seconds.

```toml
# agents.toml
[assistant.limits]
message_bytes = 65536                 # longer messages; reported as a weakening

[assistant.desk]
live = 8                              # more harnesses on each Station; reported as a weakening
idle = 300                            # close an idle harness after five minutes

[assistant.gateway]
conversations_per_person = 50
token_ttl = 1800
```

`[assistant.limits]`: what passes between Universal Exports and a Station, which both check,
and what a Station reads of her memory. A Station reads it for each job and harness start;
Universal Exports when it starts.

| Key | Default | What |
| --- | --- | --- |
| `message_bytes` | 32 KiB | Bytes of one job's message; Universal Exports refuses a person's longer message |
| `context_bytes` | 24 KiB | Bytes of a job's standing context and recap together; a recap stays under half |
| `cable_grants` | 64 | Operations one job grants Cable actions in |
| `skills` | 64 | Skills one job gives her |
| `event_bytes` | 64 KiB | Bytes of one reported event's text; longer text is cut, and a memory change whose diff is longer is not kept |
| `events` | 64 | Events in one report |
| `change_bytes` | 64 KiB | Bytes of file content in one memory change, commit, memory write or memory read |
| `change_files` | 32 | Files in one memory change or commit |
| `commit_message_bytes` | 4,096 | Bytes of one commit's message |
| `shared_bases` | 32 | Shared-change bases one event carries, and Universal Exports keeps per conversation |
| `memory_list_files` | 1,000 | Files one memory listing names |
| `memory_list_bytes` | 128 KiB | Bytes of paths one memory listing names |
| `inlined_memory_bytes` | 32 KiB | Bytes of `MEMORY.md` inlined into her standing context |
| `memory_file_bytes` | 256 KiB | Bytes of one memory file the desk reads; a longer one is read to this length |
| `memory_files` | 1,024 | Memory files the desk reads at a turn's edge |
| `memory_bytes` | 8 MiB | Bytes over all of them |
| `memory_walk` | 10,000 | Files of `personal/` looked at to name what a turn changed |
| `memory_import_depth` | 5 | How deep `@path` imports from her memory are followed to find outside text; Claude Code follows five |

`[assistant.desk]`: how each Station's desk runs her harnesses. Read for each job, harness
start and memory request; `known`, `parked` and `report_queue` at each use; `job_wait`,
`job_bytes`, `job_retry`, `call_margin`, `report_attempts` and `report_retry` at each poll or
report; `unlocked_stale` when the desk starts.

| Key | Default | What |
| --- | --- | --- |
| `idle` | 600 | Seconds a harness waits for the next message, or writes nothing while answering, before it is closed; `"none"` keeps it |
| `live` | 4 | Harnesses a Station runs or starts at once; past it a message is refused and another Station is asked |
| `token_margin` | 120 | Seconds before its tool token expires that a running harness is restarted with the job's |
| `job_wait` | 1 | Seconds Universal Exports holds a job poll open |
| `job_bytes` | 256 KiB | Bytes of one job as Universal Exports sends it |
| `call_margin` | 2 | Seconds a request to Universal Exports may take past what it waits for |
| `job_retry` | 0.5 | Seconds before a failed job poll is tried again |
| `report_queue` | 256 | Ordinary reports (her text, her tool calls) waiting to be sent; past it the newest are dropped |
| `report_attempts` | 3 | Times a report is sent before it counts as lost |
| `report_retry` | 0.2 | Seconds between tries of a report that did not arrive |
| `parked` | 64 | Memory changes kept on the Station because they could not be delivered; past it the oldest is dropped |
| `known` | 1,024 | Conversations whose provider Session the desk remembers for a resume |
| `close_wait` | 10 | Seconds a message waits for its conversation's closing harness |
| `remove_retry` | 2 | Seconds removing a closed harness's directory is tried again |
| `unlocked_stale` | 3,600 | Seconds before a working directory left by a desk that is gone, without its lock, is swept |
| `clone_timeout` | 120 | Seconds a clone of one of her repositories may take; a memory request's clone also stops at its deadline |
| `clone_bytes` | 512 MiB | Bytes a clone may hold on disk, then it is given up and removed |
| `save_bytes` | 16 MiB | Bytes one save of her work in `personal/` may commit; past it nothing is saved and she is told |
| `memory_requests` | 4 | The app's memory requests a Station makes at once; the next is refused |

`[assistant.gateway]`: how Universal Exports keeps her conversations. Read when it starts.

| Key | Default | What |
| --- | --- | --- |
| `conversations_per_person` | 20 | Conversations kept for one person; the oldest goes first |
| `conversations` | 2,000 | Conversations kept in all |
| `conversation_age` | 604,800 (7 days) | Seconds a conversation is kept untouched |
| `conversation_entries` | 500 | Entries kept per conversation |
| `conversation_bytes` | 1 MiB | Bytes of entries kept per conversation; the oldest go first |
| `person_bytes` | 4 MiB | Bytes of entries kept per person; a person's views go first |
| `live_per_person` | 2 | One person's conversations with a harness running or an answer pending; the least recently used idle harness is closed to make room |
| `tool_calls_per_minute` | 120 | Tool calls one conversation makes in a minute |
| `memory_requests_per_person` | 2 | The app's memory requests waiting for a Station, for one person; `429` past it |
| `memory_requests` | 16 | The same, for everyone |
| `token_ttl` | 3,600 | Seconds a tool token given to a harness is accepted |
| `token_reuse_margin` | 600 | Seconds a token must still have to be given to the next message; kept above `[assistant.desk] token_margin` |
| `answer_timeout` | 60 | Seconds a Station has to take up a message before another is asked |
| `answer_deadline` | 600 | Seconds an accepted message may go without word from its Station before it is given up |
| `memory_timeout` | 30 | Seconds a Station has to answer a memory request |
| `proposal_ttl` | 3,600 | Seconds a proposal waits for the person's confirmation |
| `proposals` | 32 | Proposals waiting in one conversation |
| `message_bytes` | 16 KiB | Bytes of one message the person writes |
| `memory_message_bytes` | 1,024 | Bytes of a commit message the person writes for a memory change |
| `change_message_chars` | 200 | Characters of the one-line message of a shared change she proposes |
| `recap_bytes` | 8 KiB | Bytes of the recap a conversation's new harness starts with |
| `ready_entries` | 100 | Entries the app is sent when it opens a conversation |
| `ready_bytes` | 512 KiB | Bytes of those entries |
| `socket_message_bytes` | 64 KiB | Bytes of one message on the app's socket |
| `hello_timeout` | 10 | Seconds the app's socket has to say hello |
| `max_wait` | 25 | Seconds a Station's job poll may be held open |
| `sweep_every` | 1 | Seconds between an open conversation's looks for jobs no Station took up |
| `shared_base_wait` | 5 | Seconds a shared change waits for its Station's report of what it was made from |
| `event_buffer` | 256 | Events queued for a conversation's open sockets; one further behind is closed |
| `route_bytes` | 4 MiB | Bytes of an AI6 route's answer to one of her tools |
| `result_chars` | 200,000 | Characters of one tool's answer she is given; longer is cut |
| `author_chars` | 200 | Characters of a person's name and email on her commits |
| `audit_target_bytes` | 512 | Bytes of the target of an audit record of her actions |

`[assistant.views]` bounds the views she shows ([Moneypenny](assistant.md#views-she-shows)).

### The directory provider

A directory holds one file per family, named for it: `operations.toml`, `agents.yaml`,
`station.cfg`, and so on, with any of the five extensions. Two files for one family
(`agents.toml` beside `agents.json`) are refused, naming both. `ai6.cfg` and `unex.cfg` in the
same directory are bootstrap files, not family files, so the `unex` family is `unex.toml`,
`.json`, `.yaml` or `.yml`. A Station's own records go in `stations/<id>/`:

```text
config/
  ai6.cfg
  agents.toml
  operations.toml
  theaters.yaml
  station.cfg
  stations/
    hq-1/
      station.toml        # hq-1's own runtime settings over station.cfg
```

The directory is read-only: edit its files by hand or through version control. Its revision
is a digest of the files a Station reads, so a Station notices a change at its next poll.

### The SQL provider

`sqlite:PATH` or a `postgres://` URL keeps each family in a table, `ai6_config_<family>`, with
the columns `scope` (`''` for global, `station:<id>`), `key`, `definition` (a JSON object
with the keys the record has in its file) and `revision`. `[operation.G]` with
`roster = ["007"]` is the row `('', 'G', '{"roster": ["007"]}')`. Every change advances the
`config` row of `ai6_clock` and is checked, in the same transaction, against every view it
touches: the global one and each Station's with records of its own. A tool that writes the
tables directly advances that row in the same transaction, or Stations do not see the change.

### The Registry

The Registry holds what Stations write as they run: check-ins, leases, the Headquarters
term, Sessions, Cables, approvals and audit. It stays a store of its own: by default the
provider's database when that is SQL, so one PostgreSQL location configures a whole HA
deployment, else `registry/` in the state directory. Configuration can live in a git
checkout while the Registry lives in PostgreSQL.

## `ai6 config`

```console
ai6 config path                            # the ai6.cfg in use
ai6 config show agent 007                  # one record, as TOML
ai6 config show theater --scope station:hq-1
ai6 config import ./config                 # a directory's records into a SQL provider
ai6 config import ./hq-1 --scope station:hq-1
ai6 config export ./backup                 # every family and Station scope as TOML files
```

`import` checks the whole directory first and stores its records in one transaction: all of
them or none. `export` writes files readable by their owner only on Unix, since a family may
hold a credential.

## Who can write the provider

Whoever writes the provider decides which provider libraries every Station loads (`plugin`;
each host still applies its own `[plugins] profile`), its TLS policy (`tls`), the programs a
Station starts on its own host (a `theater` runner), the files a Station reads secrets from
(`secret`), and who signs in to Universal Exports and what they may do (`unex`, `access`).
Treat the provider's write credentials as sensitive as the strongest role they grant. Every
weakening those records cause is logged at start and sent at check-in.
