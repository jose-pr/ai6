# Configuration

AI6 keeps two kinds of configuration apart:

- **One bootstrap file, `ai6.cfg`,** on each host or for each user holds only what a process
  needs before it can reach anything else: where the configuration provider and the Registry are,
  the passwords to reach them, the host's own directories, its listeners, and how it checks the
  provider libraries it loads. A Station, the `ai6` command, Universal Exports, `ai6ctl` and Outer
  Office on the desktop all read it, each taking its own sections.
- **A configuration provider** holds everything else as records: Operations, Agents and
  roles, covers, Theaters, loadouts and account pools, Cable settings, provider library
  pins, each Station's runtime settings and TLS policy, and Universal Exports' sign-in and
  role policy. Every Station of a deployment, and Universal Exports, read the same provider.

## The bootstrap file

### Where it is found

The first place that has a file wins; files are never merged, and with none every key is at
its default. The working directory is never searched: a daemon's working directory is
arbitrary, and a checkout could plant a file there.

1. `--config PATH`, which must exist.
2. `AI6_CONFIG`, which must exist.
3. `ai6.cfg` in the user's configuration directory: `$XDG_CONFIG_HOME/ai6`, `%APPDATA%\ai6` on
   Windows, else `~/.config/ai6`.
4. `/etc/ai6/ai6.cfg`, `%ProgramData%\ai6\ai6.cfg` on Windows.

`ai6 config path` prints the file in use, or `none (defaults)`.

A relative path in the file is relative to the file's directory. A key nobody knows is refused by
every program, naming it, in any section but `[oo]`, which only Outer Office reads and judges. Each
program says when a key it needs is missing: `universal-exports` needs `[unex] public_origin` and
`station_token`, and nothing else does. A command-line flag beats the file, which beats the
default; the only environment variables are `AI6_CONFIG` and the ones a secret reference names.

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

The provider's family files follow the same rules. Every format becomes one value before a
type reads it, so a refusal is the same in each: it names the key or the position and never
quotes the document. A repeated key is refused in every format. A YAML plain scalar has its
type before the setting is asked, so `n`, `yes` or `123` in a text setting is refused; quote it.

### Secrets

A password or token is never a value the file has to hold. It is a secret reference, the
same shape everywhere AI6 takes one: a [secret provider](agents.md#secret-providers), the secret's
`key` there and whatever further fields that provider declares, flat beside them.

```toml
password = { provider = "file", key = "db-password" }     # a file; one trailing newline is dropped
password = { provider = "env", key = "AI6_DB_PASSWORD" }  # an environment variable
password = { provider = "inline", value = "the value" }   # inline
```

`file`, `env` and `inline` are built in, and they are the only providers this file can name: it is
read before any provider library loads, and before the configuration provider that holds
`[secret.<name>]` and `[secret_provider.<name>]` records. A relative `file` key is relative to this
file.

A file is the strongest form. A variable is inherited by every process the reader starts, and an
inline value is copied and committed with the file, so both are **weakenings**: printed at start,
and sent at check-in as `bootstrap_weakened`, as is a limit looser than its default
([Limits](#limits)). They are allowed. A secret's value never appears in a message, a log or
`Debug` output.

### What it holds

```toml
[provider]                            # shared: where the configuration provider is
location = "."                        # a directory, sqlite:PATH or postgres://...
password = { provider = "file", key = "db-password" }   # when a SQL location gives none
poll = 5                              # seconds between checks for changes; 0 turns reloading off
loadout_bytes = 1048576               # and the other limits under Limits below

[registry]                            # shared: where the Registry is, and how much it keeps
location = "postgres://db/ai6"        # default: the provider's database when it is SQL,
                                      # else registry/ in the reading program's state directory
password = { provider = "file", key = "db-password" }   # default: the provider's, when the Registry is its database
headquarters_lease = 60               # seconds; also sets the SQL connections' timeouts
sqlite_busy_timeout = 30              # seconds a SQLite connection waits for a lock

[registry.limits.sessions]            # what the Registry keeps and accepts: Limits below
records = 100000

[unex]                                # where Universal Exports is, and how it runs
# Its clients: a Station, ai6ctl and Outer Office.
url = "https://ai6.example"           # its origin; ai6ctl reads this and ca_file, nothing else here
ca_file = "enterprise-ca.pem"         # certificate authorities to trust for it; default: the public roots alone
# Both sides of the Station channel.
station_token = { provider = "file", key = "station-token" }   # a Station publishes with it, and the gateway requires it
# The gateway.
listen = "127.0.0.1:7082"             # loopback, behind the HTTPS reverse proxy
public_origin = "https://ai6.example" # required
proxy_secret = { provider = "file", key = "proxy-secret" }     # what a trusted reverse proxy sends in X-AI6-Proxy-Secret
state_dir = "/var/lib/unex"           # verified provider libraries, pages, and a default Registry

[unex.plugins]                        # the gateway's provider libraries, as [station.plugins]

[unex.access]                         # who is always chief, whatever the access policy says
chief = ["account:root"]              # account:NAME, person:ID or group:NAME

[unex.pages]                          # the pages Moneypenny publishes
dir = "/var/lib/unex/pages"           # where they are kept (default: pages in state_dir)
origin = "https://pages.example.net"  # their own origin; omit to serve them under /pages/ here
listen = "127.0.0.1:7083"             # the loopback listener of that origin

[unex.tls]                            # the gateway serves TLS itself, with no reverse proxy
certificate = "unex.pem"              # its chain, PEM; with key, listen may be any address
key = "unex-key.pem"

[unex.liaison]                        # A2A for agents outside AI6, as Cables (liaison.md)
listen = "0.0.0.0:7084"               # the link listener: wss:// with [unex.tls]; omit for /v1/a2a only
identity = "/var/lib/unex/liaison/identity"   # the gateway's link key, made the first time
poll = 1                              # seconds between looks for replies and outgoing Cables

[unex.liaison.peer.acme]              # one peer: a link key, a principal, what it may write to
id = "sha256:…"                       # its peer id; omit for a peer that signs in over HTTP only
principal = "person:acme-bot"         # what it acts as; its cable.send and cable.read decide
addressees = ["agent:G/007", "role:G/coordinator"]
url = "wss://agents.acme.example/link"   # where Liaison dials it for Cables sent to a2a:acme

[station]                             # a Station and the ai6 command
id = "hq-1"                           # default: made up once and kept in the state directory
state_dir = "/var/lib/ai6"            # default: the platform's state directory
listen = "127.0.0.1:7081"

[station.plugins]
paths = ["/usr/lib/ai6"]              # directories or library files to scan for providers
profile = "strict"                    # strict, report or off; default strict with pins, report without
keep_verified = 8                     # verified copies kept besides those loaded; 0 keeps none of them, "none" means no limit

[station.identity]
dir = "/var/lib/ai6"                  # identity.pem, peers/ and sealing.key; default: the state directory

[station.secrets]
dir = "secrets"                       # where loadout secret files are read; default secrets/ beside this file

[oo.appearance]                       # Outer Office: below
theme = "night-ops"
```

A Station publishes when the file has both `[unex] url` and `station_token`; a `url` alone is for
`ai6ctl` and Outer Office. The Station's `[unex] station_token` and the gateway's are one value, so a
host running both names it once.

Universal Exports' certificate must chain to a public root unless `[unex] ca_file` names a PEM
file of more authorities to trust. They are added to the public roots, for every request of the
Station and of `ai6ctl` to Universal Exports. A relative path is beside this file. A file that
cannot be read, holds no certificate or holds a broken one stops `ai6 serve` at start, naming the
path, with or without a `station_token`. A set `ca_file` is reported as a weakening: more authorities can
vouch for the gateway. A harness Moneypenny runs reaches the gateway's MCP endpoint on its own TLS
stack, which does not read this file.

The provider's location defaults to the directory holding `ai6.cfg`, else the user's
configuration directory. The platform's state directory is `%LOCALAPPDATA%\AI6` on Windows,
else `$XDG_STATE_HOME/AI6` or `~/.local/state/AI6`.

`ai6 serve` takes these flags over the file: `--config`, `--provider`, `--registry`,
`--state-dir`, `--station`, `--listen`, `--plugin-path` (adds to `[station.plugins] paths`) and
`--plugin-profile`. `ai6 identity`, `peer`, `sleeper`, `quarantine`, `theater provision` and
`config` take `--config`. The bootstrap file is read at start only.

`[unex.pages] origin` names a second listener that holds only page routes (no `/v1`); the API
listener then serves none. Pages on the API's own origin are by design and log nothing. A
page is sandboxed either way (a CSP `sandbox` without `allow-same-origin`, so it is an opaque
origin that cannot read the app's token or storage), but on the API's origin the address bar
shows the app's host, so a page can draw a faithful Outer Office sign-in; a separate origin
shows another host. Quotas and options are the roster's `[assistant.pages]`
([Moneypenny's limits](#moneypennys-limits)).

`universal-exports` takes `--config`, `--provider`, `--registry`, `--listen`,
`--public-origin`, `--plugin-path` and `--plugin-profile` over the file.

### Limits

How much the Registry keeps and accepts, and how much of a document the configuration
provider reads, are bootstrap settings: both are needed before any provider record is read,
by Stations and Universal Exports alike, from the one `[provider]` and `[registry]`. Every key
has a default, and a table may name only the keys it
changes.

The tables on this page are generated from the settings' own declarations, so they cannot
disagree with them; `ai6 config schema SECTION` prints the same as JSON Schema. A count or size is
a whole number; `null`, or the string `"none"` in TOML, which has no
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

<!-- schema:provider:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `location` | see meaning | **Where the configuration provider is.** A configuration directory, `sqlite:PATH` or a `postgres://` URL. Default: the directory holding the bootstrap file, else the user's configuration directory. |
| `password` | none | **Database password.** The database password, when the location needs one and gives none. |
| `poll` | 5 | **Between checks for changes.** Seconds between checks for changes; 0 turns reloading off. |
| `loadout_bytes` | 1 MiB | **Loadout document.** Bytes of the loadout document (`loadout`, `sandbox`, `mcp`, `overlay`, `secret`, `secret_provider` and `pool` records together). |
| `loadout_value_chars` | 4,096 | **One loadout value, in characters.** Characters of one text value in the loadout document. |
| `access_policy_bytes` | 1 MiB | **Access policy.** Bytes of the access policy (the `access` record). |
| `replace_retry` | 2 | **Wait for a reader of a family file.** Seconds a directory provider's replacement of a family file waits for a reader that has the file open. |
<!-- schema:provider:end -->

`[registry]`:

<!-- schema:registry:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `location` | see meaning | **Where the Registry is.** A directory, `sqlite:PATH` or a `postgres://` URL. Default: the provider's database when the provider is SQL, else `registry` in the reading program's state directory. |
| `password` | see meaning | **Database password.** The database password, when the location needs one and gives none. Default: the provider's, when the Registry is the provider's database. |
| `headquarters_lease` | 60 | **Headquarters lease.** Seconds the Headquarters lease lasts between renewals; a PostgreSQL statement, lock wait and idle transaction end after a quarter of it, a connection attempt after a sixth. |
| `sqlite_busy_timeout` | 30 | **SQLite lock wait.** Seconds a SQLite connection waits for another's lock. |
<!-- schema:registry:end -->

Each `[registry.limits.*]` table bounds one record family. A Station's
`max_concurrent_missions` setting is refused past `[registry.limits.stations]
mission_capacity`. A logbook's whole size follows from its four keys.

`[registry.limits.accounts]`

<!-- schema:registry.limits.accounts:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 256 KiB | **Largest account record.** Bytes of one account's record. |
| `claims` | 1,024 | **Live claims per account.** Live claims on one account; past it the oldest is dropped. |
| `windows` | 8 | **Quota windows per account.** Quota windows kept per account. |
| `kept_days` | 8 | **Days of token counts per account.** Days of token counts kept per account. |
| `sealed_bytes` | 4 MiB | **Largest sealed value.** Bytes of one sealed value's record, every Station's copy together. |
<!-- schema:registry.limits.accounts:end -->

`[registry.limits.agents]`

<!-- schema:registry.limits.agents:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 1 MiB | **Largest Agent record.** Bytes of one Agent record. |
| `operations` | 128 | **Operations per Agent record.** Operations one Agent record tallies. |
| `clearances` | 8 | **Clearances per Operation of an Agent record.** Clearances one Operation of an Agent record tallies. |
| `counted` | 256 | **Mission outcomes remembered per Agent.** Mission outcomes remembered per Agent, so a repeated report is counted once. |
| `model_history` | 16 | Model changes kept per Agent. |
| `key_bytes` | 512 | **Longest outcome report key.** Bytes of an outcome report's key. |
| `agent_bytes` | 256 | **Longest Agent, Operation or model in a report.** Bytes of an outcome report's Agent, Operation and model. |
<!-- schema:registry.limits.agents:end -->

`[registry.limits.agent_roles]`

<!-- schema:registry.limits.agent_roles:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 32 KiB | **Largest Agent role record.** Bytes of one Agent role record. |
| `records` | 256 | Agent roles per Operation. |
<!-- schema:registry.limits.agent_roles:end -->

`[registry.limits.approvals]`

<!-- schema:registry.limits.approvals:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 16 KiB | **Largest approval record.** Bytes of one approval record. |
| `pending` | 1,000 | Pending approval requests. |
| `class_authorizations` | 1,000 | Class authorizations. |
| `mission_requests` | 64 | **Requests per Mission.** Requests one Mission may have. |
| `reason_bytes` | 2 KiB | **Longest request reason.** Bytes of a request's reason. |
| `note_bytes` | 1 KiB | **Longest decision note.** Bytes of a decision's note. |
| `signer_bytes` | 256 | **Longest signer name.** Bytes of a signer's name. |
| `idempotency_key_bytes` | 256 | **Longest idempotency key.** Bytes of a request's idempotency key. |
| `class_entries` | 64 | **Entries per class authorization.** Step kinds, Agents, roles and proposed roles each class authorization may name. |
| `pattern_bytes` | 256 | **Longest target pattern.** Bytes of a class authorization's target pattern. |
| `name_bytes` | 128 | **Longest name in a class authorization.** Bytes of each Agent or role a class authorization names. |
| `expired_listed` | 200 | **Expired requests listed.** Expired requests one list shows, the newest first. |
| `links` | 8 | **Links per request.** Links one request may carry. |
| `link_label_bytes` | 128 | **Longest link label.** Bytes of a link's label. |
| `link_url_bytes` | 1 KiB | **Longest link address.** Bytes of a link's address. |
<!-- schema:registry.limits.approvals:end -->

`[registry.limits.assistant_log]`: Moneypenny's conversation log. How long it is kept, and how
much, is `[assistant.gateway] log_age` and `log_bytes` ([Moneypenny](assistant.md#her-conversation-log)).

<!-- schema:registry.limits.assistant_log:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `entry_bytes` | 1 MiB | **Largest log entry.** Bytes of one entry of the log; a longer one is kept without its data and with its text cut, and says how large it was. |
<!-- schema:registry.limits.assistant_log:end -->

`[registry.limits.audit]`

<!-- schema:registry.limits.audit:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 16 KiB | **Largest audit record.** Bytes of one audit record. |
| `records` | 10,000 | Audit records kept. |
| `results` | 500 | **Most records in one listing.** Records one listing may ask for. |
<!-- schema:registry.limits.audit:end -->

`[registry.limits.briefs]`

<!-- schema:registry.limits.briefs:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 1 MiB | **Largest brief.** Bytes of one brief. |
| `records` | 1,000 | Queued briefs. |
| `provenance_bytes` | 16 KiB | **Longest brief provenance.** Bytes of a brief's provenance. |
| `handle_bytes` | 120 | **Longest Mission handle.** Bytes of a Mission's handle: a longer one AI6 gives keeps its start and adds a short hash, and a rename to a longer one is refused. It names a Git branch, so it stays under 240 bytes whatever this says. |
| `cancel_settle_after` | 300 | **Settle an unconfirmed cancel after.** Seconds after a cancel was asked for before the Headquarters Station settles a claimed brief its Station has not finished: filed as failed, its lease released. |
<!-- schema:registry.limits.briefs:end -->

`[registry.limits.cables]`

<!-- schema:registry.limits.cables:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 2 MiB | **Largest Cable record.** Bytes of one Cable record. |
| `records` | 10,000 | **Cable records kept.** Cable records the Registry holds. |
| `body_bytes` | 16 KiB | **Longest Cable body.** UTF-8 bytes of a Cable's body the Registry stores. |
| `max_ttl` | 86,400 (a day) | **Longest a Cable lives.** Seconds a Cable the Registry stores may live. |
| `page` | 1,000 | **Cables in one read.** Cables one read of an inbox or of the held Cables takes. |
<!-- schema:registry.limits.cables:end -->

What a Station's relay accepts from a sender is the `cables` record's
([Families](#families)); the Registry's bounds above apply to what is stored, whoever wrote
it.

`[registry.limits.clearance]`

<!-- schema:registry.limits.clearance:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 16 KiB | **Largest clearance grant.** Bytes of one clearance grant. |
| `grants` | 10,000 | Clearance grants. |
| `text_bytes` | 512 | **Longest signer, approval, model or reason.** Bytes of a grant's signer, approval, model and reason. |
<!-- schema:registry.limits.clearance:end -->

`[registry.limits.contexts]`: each Session's harness context.

<!-- schema:registry.limits.contexts:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `context_bytes` | 8 MiB | **Largest Session context.** Bytes of one Session's context kept; a longer one keeps its newest part and says how large it was. |
| `records` | 2,000 | **Session contexts kept.** Session contexts kept; the oldest filed go first. |
<!-- schema:registry.limits.contexts:end -->

`[registry.limits.files]`: how long the file Registry waits. The SQL Registry ignores it.

<!-- schema:registry.limits.files:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `rename_retry` | 2 | **Retry replacing a held file for.** Seconds a record write retries replacing a file another process holds open. |
| `lock_wait` | 30 | **Wait for the directory lock.** Seconds a write waits for the Registry's directory lock. |
<!-- schema:registry.limits.files:end -->

`[registry.limits.held]`: A held record may be 64 KiB, room for its verdict, larger than a brief's `[registry.limits.briefs] record_bytes`.

<!-- schema:registry.limits.held:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `per_operation` | 1,000 | **Held briefs per Operation.** Briefs one Operation may hold. |
| `records` | 10,000 | **Held briefs in all.** Briefs held across the Registry. |
| `results` | 500 | **Most held briefs in one listing.** Held briefs one listing may ask for. |
| `clearance_bytes` | 4 KiB | **Largest stored clearance.** Bytes of a stored clearance, the finding kinds a brief was released from. |
| `reason_bytes` | 1 KiB | **Longest refusal reason.** Bytes of the reason a held brief is refused for. |
| `actor_bytes` | 256 | **Longest name of who decided.** Bytes of who released or refused a held brief. |
<!-- schema:registry.limits.held:end -->

`[registry.limits.logbook]`: A whole logbook's size is bounded by these four.

<!-- schema:registry.limits.logbook:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `entries` | 20 | **Reports per logbook.** Reports kept per logbook, and handed to the next round. |
| `summary_bytes` | 4 KiB | **Longest report summary.** Bytes of a report's summary. |
| `findings` | 32 | Findings per report. |
| `finding_bytes` | 1 KiB | **Longest finding.** Bytes of one finding. |
<!-- schema:registry.limits.logbook:end -->

`[registry.limits.memories]`: what one scope of memory may hold: a person's, an Operation's or the
global one.

<!-- schema:registry.limits.memories:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `content_bytes` | 256 KiB | **Longest entry.** Bytes of one entry's content. |
| `entries` | 1,024 | **Entries in a scope.** Entries in one scope. |
| `scope_bytes` | 8 MiB | **All the entries of a scope.** Bytes of every entry's name, summary and content in one scope. |
| `versions` | 8 | **Prior versions of an entry.** Prior versions kept of one entry; forgetting the entry removes them. `0` keeps none. |
| `summary_bytes` | 1 KiB | **Longest summary.** Bytes of an entry's one-line summary. |
| `name_bytes` | 256 | **Longest name or author.** Bytes of an entry's name, or of who wrote it. |
| `compartments` | 16 | **Compartments on an entry.** Compartments on one entry, each at most `name_bytes`. |
| `index_bytes` | 512 KiB | **Words of an entry in the text index.** Bytes of an entry's distinct words a database's text index holds. A word past it is found only by a search that matched another word of the entry, and PostgreSQL refuses an index of 1 MiB or more for one entry. |
| `search_results` | 50 | **Entries a search answers.** Entries one search answers. |
| `query_bytes` | 4 KiB | **Longest search.** Bytes of what one search asks for. |
| `proposals` | 64 | **Proposals waiting in a scope.** Agents' proposals waiting for a person in one scope. |
<!-- schema:registry.limits.memories:end -->

`[registry.limits.proposals]`

<!-- schema:registry.limits.proposals:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 1 MiB | **Largest Mission proposal.** Bytes of one Mission proposal. |
| `records` | 1,000 | **Pending proposals per Operation.** Pending Mission proposals per Operation. |
| `results` | 500 | **Most proposals in one listing.** Proposals one listing may ask for. |
| `idempotency_key_bytes` | 256 | **Longest idempotency key.** Bytes of a proposal's idempotency key. |
<!-- schema:registry.limits.proposals:end -->

`[registry.limits.quarantine]`

<!-- schema:registry.limits.quarantine:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 16 KiB | **Largest quarantine record.** Bytes of one quarantine record. |
| `records` | 10,000 | Quarantine records. |
| `incidents` | 256 | **Incidents per Agent.** Incidents kept per Agent. |
| `keep_incidents` | 604,800 (7 days) | **Keep an incident for.** Seconds an incident is kept. |
<!-- schema:registry.limits.quarantine:end -->

`[registry.limits.reports]` ([Reports](reports.md))

<!-- schema:registry.limits.reports:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 64 KiB | **Largest Report.** Bytes of one Report record. |
| `reports` | 100,000 | Reports kept. |
| `text_bytes` | 16 KiB | **Longest Report text.** Bytes of a Report's text. |
| `name_bytes` | 256 | **Longest title or name.** Bytes of a Report's kind and title, and of each name on it: filer, links, compartments. |
| `links` | 64 | **Links on a Report.** Links on one Report. |
| `page` | 1,000 | **Reports in one list.** Reports one list returns. |
| `per_session` | 20 | **Reports per Session in the window.** Reports one Agent Session may file in the window. |
| `per_operation` | 200 | **Agent Reports per scope in the window.** Reports Agents together may file against one scope in the window. |
| `window` | 3,600 | **Rate window.** Seconds the two counts above look back over. |
| `keep_closed` | none | **Keep a closed Report for.** Seconds a closed Report is kept; `null` keeps it for good. |
<!-- schema:registry.limits.reports:end -->

`[registry.limits.sessions]`

<!-- schema:registry.limits.sessions:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 64 KiB | **Largest Session record.** Bytes of one Session record. |
| `records` | 100,000 | Session records. |
| `id_bytes` | 128 | **Longest Session ID.** Bytes of a Session ID. |
| `exposures` | 32 | **Exposures per Session.** Exposures recorded per Session. |
| `exposure_source_bytes` | 128 | **Longest exposure source.** Bytes of an exposure's source. |
| `recent_events` | 20 | **Recent event lines per Session.** Recent event lines kept per Session. |
| `recent_event_chars` | 300 | **Longest recent event line.** Characters kept of each recent event line and of a lost Session's reason. |
| `exposure_sources` | 32 | **Sources of exposure per brief.** Sources of exposure one Session's brief records; the rest are not kept. |
| `final_report_bytes` | 64 KiB | **Longest final report.** Bytes kept of an Agent's final message for its Mission's outcome; a longer one is cut and says so. |
| `verdict_note_bytes` | 4 KiB | **Longest verdict note.** Bytes of the note a person gives with a verdict on a Mission's outcome. |
| `amendment_bytes` | 8 KiB | **Longest redispatch amendment.** Bytes of what a redispatch adds to the next attempt's task. |
| `deliverables` | 32 | **Deliverables per attempt.** Deliverables kept on one attempt. |
| `evidence_entries` | 64 | **Probe findings per attempt.** Probe findings kept on one attempt; the oldest go first. |
| `evidence_detail_chars` | 300 | **Longest probe finding.** Characters kept of a probe finding's detail. |
<!-- schema:registry.limits.sessions:end -->

`[registry.limits.sleepers]`

<!-- schema:registry.limits.sleepers:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `catch_up` | see meaning | **A missed window still fires within.** Seconds a missed window may be late and still fire, once; an older one is skipped and recorded. `none` keeps no limit. Default: the window's own period, the time since the window before it. |
| `prompt_bytes` | 16 KiB | **Longest Sleeper prompt.** Bytes of a Sleeper's prompt. |
| `lookback_days` | 366 | **Days searched back for a window.** Days a schedule is searched back for its latest window. |
| `skipped_events` | 64 | **Skipped windows named one by one.** Skipped windows the timeline names one by one when a later window comes; any older ones are one event. |
<!-- schema:registry.limits.sleepers:end -->

`[registry.limits.issues]`

<!-- schema:registry.limits.issues:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `links_per_mission` | 32 | **Linked issues per Mission.** Linked issues one Mission holds at once; unlinked ones do not count. |
| `outbox_records` | 1,000 | **Outbox entries kept.** Outbox entries kept; settled ones are dropped oldest first. |
| `entry_bytes` | 128 KiB | **Largest outbox entry.** Bytes of one outbox entry's text and payload. |
| `replies_per_mission` | 200 | **Stored replies per Mission.** Replies from a linked issue one Mission stores; delivered ones make room first. |
| `reply_bytes` | 64 KiB | **Longest stored reply.** Bytes of one stored reply, cut past it. |
<!-- schema:registry.limits.issues:end -->

`[registry.limits.sources]`

<!-- schema:registry.limits.sources:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `item_text_bytes` | 64 KiB | **Longest Source item text.** Bytes of a Source item's text. |
| `report_attempts` | 5 | **Tries to report a finished brief.** Times reporting a finished brief to its Source is tried before it is dropped. |
| `name_bytes` | 256 | **Longest name beside a Mission ID.** Bytes of the name a brief shows beside its Mission ID. |
<!-- schema:registry.limits.sources:end -->

`[registry.limits.stations]`

<!-- schema:registry.limits.stations:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 1 MiB | **Largest Station record.** Bytes of one Station record. |
| `records` | 1,000 | Station records. |
| `settings_bytes` | 4 KiB | **Largest Station settings record.** Bytes of a Station's settings record. |
| `mission_capacity` | 1,024 | **Most Missions a Station may run at once.** Missions a Station's settings may let it run at once. |
| `gone_dark_after` | 45 | **A Station goes dark after.** Seconds after its last check-in that a Station counts as dark. |
<!-- schema:registry.limits.stations:end -->

`[registry.limits.timeline]`

<!-- schema:registry.limits.timeline:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `records` | 10,000 | **Events kept.** Events kept; the oldest are dropped past it. |
| `record_bytes` | 8 KiB | **Largest event.** Bytes of one event. |
| `id_bytes` | 128 | **Longest event ID.** Bytes of an event's ID. |
| `name_bytes` | 64 | **Longest kind, action or detail key.** Bytes of an event's kind, action and detail keys. |
| `text_bytes` | 256 | **Longest event text.** Bytes of an event's Operation, Station, Mission, Session, actor and detail values. |
| `detail_entries` | 8 | **Details per event.** Detail entries per event. |
<!-- schema:registry.limits.timeline:end -->

`[registry.limits.tokens]`

<!-- schema:registry.limits.tokens:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `record_bytes` | 16 KiB | **Largest access token record.** Bytes of one access token record. |
<!-- schema:registry.limits.tokens:end -->

`[registry.limits.usage]`

<!-- schema:registry.limits.usage:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `ledger_bytes` | 1 MiB | **Largest usage ledger.** Bytes of one Mission's usage ledger. |
| `kept_days` | 8 | Days of usage kept. |
<!-- schema:registry.limits.usage:end -->

`[registry.limits.watches]`: each watch's record ([Watches](watches.md)).

<!-- schema:registry.limits.watches:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `records` | 1,024 | **Watch records.** Watch records the Registry keeps. |
| `record_bytes` | 1 MiB | **Largest watch record.** Bytes of one watch's record. |
| `logbook` | 100 | **Logbook entries per watch.** Logbook entries kept per watch, and handed to a fresh Session; past it the oldest go. |
| `entry_bytes` | 2 KiB | **Longest logbook entry.** Bytes of one logbook entry's text, or of a control's note; a longer entry is cut. |
| `seen` | 1,024 | **Events remembered per watch.** Events a watch remembers being told of, so it is not told twice; past it the oldest go. |
| `controls` | 32 | **Controls waiting per watch.** Controls waiting for a watch's Station; past it a control is refused. |
<!-- schema:registry.limits.watches:end -->

### `ai6ctl`

`ai6ctl` has no file of its own. `--timeout SECONDS` (default 5, fractions allowed, `0` for no
limit) bounds each HTTP request and the control commands take `--poll-interval SECONDS` (default
0.5). Its Universal Exports origin is `--server`, else `UNEX_URL`,
else `[unex] url` of the `ai6.cfg` found as above (`--config` included), else
`http://127.0.0.1:7082`. Besides the public roots it trusts the authorities of `--ca-file`, else
`UNEX_CA_FILE`, else `[unex] ca_file` of that `ai6.cfg`; one that cannot be read or holds no
certificate is refused before any request, naming the path. Its user token comes only from
`UNEX_ACCESS_TOKEN`; it never reads `[unex] station_token`.

### Outer Office: `[unex]` and `[oo]`

Outer Office on the desktop reads the same `ai6.cfg`, found the same way, and takes `[unex] url`,
`[unex] ca_file` and `[oo]`; another section is another program's, so it is left alone. A missing
or unreadable file is logged, by path and never quoting it, and the defaults apply. Saving an
address in the app edits only the `url` line of `[unex]` and keeps everything else, comments
included; a file spelled so that this cannot be done safely is refused and edited by hand.

On the web a page can fetch only what is served, so what sits beside `index.html` is a public
`ai6.cfg`, never a host's own, which names files and addresses that are not the public's business.
`ai6 config public` writes it from a host's file:

```console
ai6 config public --config /etc/ai6/ai6.cfg --output /opt/ai6/web/ai6.cfg
```

It keeps an allow list, so a new key is private until someone says otherwise: the gateway's address,
and of `[oo]` its tables' numbers and booleans and the two words `[oo.appearance] theme` and
`[oo.assets] cdn`. The address is written as the page's `[unex] url`: the host's `[unex]
public_origin`, where browsers reach the gateway, else its `[unex] url`, since a host's own
`url` is often a loopback address no browser can use. Any other text under `[oo]` is left out and
named on standard error. An address holding a credential, a query or a fragment is refused, naming
its key. Without `--output` it prints to standard output; the
host's file itself is refused as an output. A web build never bundles a host's file: copy the
public one beside `index.html` after the build, and a missing one means the defaults.

```toml
[unex]
url = "https://ai6.example"           # desktop; on the web the page's own origin is the API
ca_file = "enterprise-ca.pem"         # desktop: trusted as well as the system's roots, beside this file when relative

[oo.appearance]
theme = "night-ops"                   # night-ops, black-ops, dusk, classic, light, red-file or system

[oo.chat]                             # Moneypenny's chat; logical pixels, every key optional
min_width = 320                       # the docked chat's narrowest drag
max_width = 720                       # and widest
width = 400                           # its width until the person drags it
width_step = 16                       # an arrow key on the drag handle moves it this far
min_page_width = 360                  # the page keeps at least this much beside it
text_width = 960                      # expanded: her text keeps to a column this wide
window_width = 520                    # desktop: her own window's size when popped out
window_height = 760
transition_ms = 200                   # how long the chat takes to move; 0 for none
connect_timeout = 10                  # seconds her socket may take to say it is ready before it counts as failed
retry_delay = 1                       # seconds before her socket's first reconnect; each next one doubles
retry_max_delay = 16                  # seconds, the longest wait between reconnects

[oo.routes]
text_bytes = 128                      # the longest text filter a page link may carry, in bytes

[oo.art]
max_width = 3840                      # the widest hero picture fetched, in pixels; a site on slow links sets 1920

[oo.refresh]                          # seconds between a page's asks; 0 turns that refresh off
sessions = 3                          # what the page shown displays from the board of Sessions, Stations and Missions
overview = 10                         # the overview's recent activity
timeline = 10                         # an Operation's timeline
pools = 15                            # the account pools
slow = 15                             # what the bell and search need and the page does not show: Sessions, Stations, held briefs, green ink requests, Mission proposals; 0 reads them with every refresh
profile = 60                          # who the person is and what they may do; 0 reads it only at sign-in and when the operator menu opens
hidden = 60                           # everything, while the tab or window cannot be seen; 0 reads nothing until it can
silent_intervals = 3                  # a Station silent for this many `sessions` intervals shows as silent
renew_before = 600                    # how long before the access token ends the app renews it, and before the sign-in ends says so; 0 does neither
renew_retry = 30                      # how soon a failed renewal is tried again when `sessions` is 0; else at `sessions`

[oo.controls]                         # following a Mission control until the Station reports
wait = 10                             # seconds; 0 returns once the control is queued
poll = 0.5                            # seconds between asks for its status
confirming = 30                       # seconds a proposal of Moneypenny's may stay applying before its outcome reads unknown

[oo.pages]
timeline = 50                         # items in one page of the timeline
audit = 100                           # records in the audit log
summary = 5                           # recent activity, and Operations and Stations, a summary lists
search = 24                           # results the top bar's search lists at once

[oo.terminal]                         # the live Session terminal
scrollback_lines = 10000
connect_timeout = 10                  # seconds the socket may take to open
retry_delay = 1                       # seconds before the first reconnect; each next one doubles
retry_max_delay = 16                  # seconds, the longest wait between reconnects

[oo.limits]                           # characters a text field takes; 0 takes the bound away
mission_title_chars = 512
mission_prompt_chars = 65536
reason_chars = 512                    # the reason for lowering an Agent's clearance

[oo.assets]
cdn = "none"                          # web only: none, fonts or all (read by the page's bootstrap)
```

The keys are declared on `OoSettings` in the `ai6` crate, and the app takes that schema with its
build; the values above are their defaults, which a test holds them to.

`[oo.appearance]`

<!-- schema:oo.appearance:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | `night-ops` | **Theme.** The theme a person who has chosen none starts with, or `system`; what a person chooses wins. |
<!-- schema:oo.appearance:end -->

`[oo.chat]`

<!-- schema:oo.chat:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `min_width` | 320 | **Narrowest docked chat.** How narrow the docked chat may be dragged. |
| `max_width` | 720 | **Widest docked chat.** How wide the docked chat may be dragged. |
| `width` | 400 | **Width of the docked chat.** The docked chat's width until the person drags it; what they drag to wins. |
| `width_step` | 16 | **An arrow key moves it.** How far an arrow key moves the drag handle. |
| `min_page_width` | 360 | **Page kept beside the chat.** The page keeps at least this much beside the docked chat. |
| `text_width` | 960 | **Widest column of her text.** The widest column her text keeps to when expanded. |
| `window_width` | 520 | **Width of her own window.** The width her own window opens at (desktop). |
| `window_height` | 760 | **Height of her own window.** The height her own window opens at (desktop). |
| `transition_ms` | 200 | **The chat moves in.** Milliseconds the chat takes to move; none when the system asks for no animations. |
| `connect_timeout` | 10 | **Her socket says it is ready within.** Seconds her socket may take to say it is ready before it counts as failed. |
| `retry_delay` | 1 | **First reconnect after.** Seconds before her socket's first reconnect; each next one doubles, up to `retry_max_delay`. |
| `retry_max_delay` | 16 | **Longest wait between reconnects.** The longest wait between reconnects. |
<!-- schema:oo.chat:end -->

`[oo.routes]`

<!-- schema:oo.routes:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `text_bytes` | 128 | **Longest text filter in a link.** Bytes of the `text` filter of a page link; a web URL or one of Moneypenny's links with a longer one is refused like any other bad link. Keep it equal to the `unex` record's `route_text_bytes`, or her links that one accepts and this one does not show as a refused view. |
<!-- schema:oo.routes:end -->

`[oo.art]`

<!-- schema:oo.art:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `max_width` | 3,840 | **Widest hero picture.** Pixels of the widest hero picture ever fetched; a site on slow links sets 1920. |
<!-- schema:oo.art:end -->

`[oo.refresh]`: how often each page asks again; `0` turns that refresh off.

<!-- schema:oo.refresh:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `sessions` | 3 | **The page shown.** What the page shown displays from the board of Sessions, Stations and Missions. |
| `overview` | 10 | **The overview.** The overview's recent activity. |
| `timeline` | 10 | **A timeline.** An Operation's timeline. |
| `pools` | 15 | The account pools. |
| `slow` | 15 | **The bell and the search.** What the bell and the search need and the page shown does not display: Sessions, Stations, held briefs, green ink requests and Mission proposals; `0` reads them with every refresh. |
| `profile` | 60 | **Who the person is.** Who the person is and what they may do; `0` reads it only at sign-in and when the operator menu opens. |
| `hidden` | 60 | **While nothing can be seen.** Everything, while the tab or window cannot be seen; `0` reads nothing until it can. |
| `silent_intervals` | 3 | **Intervals before a Station is silent.** A Station that has not checked in for this many `sessions` intervals shows as silent. |
| `renew_before` | 600 | **Renew a sign-in before it ends.** How long before the access token ends the app renews it, and before the sign-in itself ends the person is told; `0` does neither, and the sign-in ends with the token. A shorter lifetime than this is renewed halfway through. |
| `renew_retry` | 30 | **Retry a failed renewal after.** How soon a failed renewal is tried again when `sessions` is off; else it is tried again at `sessions`. |
<!-- schema:oo.refresh:end -->

`[oo.controls]`: following a Mission control until the Station reports.

<!-- schema:oo.controls:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `wait` | 10 | **Follow a control for.** How long to follow a control for the Station's outcome; `0` returns once it is queued. |
| `poll` | 0.5 | **Ask for its status every.** How often to ask for its status meanwhile. |
| `confirming` | 30 | **A proposal may stay applying.** How long a proposal of Moneypenny's may stay applying before its card says the outcome is unknown. |
<!-- schema:oo.controls:end -->

`[oo.pages]`: how much a page asks for.

<!-- schema:oo.pages:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `timeline` | 50 | **Timeline items in a page.** Items in one page of an Operation's timeline. |
| `audit` | 100 | **Audit records shown.** Records in the audit log. |
| `summary` | 5 | **Items in a summary.** Items of recent activity, Operations and Stations a summary lists. |
| `search` | 24 | **Search results listed.** Results the top bar's search lists at once. |
<!-- schema:oo.pages:end -->

`[oo.terminal]`: the live Session terminal.

<!-- schema:oo.terminal:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `scrollback_lines` | 10,000 | **Lines kept.** Lines the terminal keeps. |
| `connect_timeout` | 10 | **The socket opens within.** How long the socket may take to open. |
| `retry_delay` | 1 | **First reconnect after.** The wait before the first reconnect; each next one doubles, up to `retry_max_delay`. |
| `retry_max_delay` | 16 | **Longest wait between reconnects.** The longest wait between reconnects. |
<!-- schema:oo.terminal:end -->

`[oo.limits]`: how much a text field takes, in characters; `0` takes the bound away.

<!-- schema:oo.limits:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `mission_title_chars` | 512 | A new Mission's title. |
| `mission_prompt_chars` | 65,536 | A new Mission's instructions. |
| `reason_chars` | 512 | **The reason for lowering a clearance.** The reason for lowering an Agent's clearance. |
<!-- schema:oo.limits:end -->

`[oo.assets]`: read by the web page's bootstrap, before the app runs.

<!-- schema:oo.assets:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `cdn` | none | **Fetched from outside.** What the web page may fetch from outside its own origin; the desktop app reads nothing here. |
<!-- schema:oo.assets:end -->

Every value of `[oo.refresh]` is a number of seconds, whole or fractional, `0` or more; `[oo.controls]`
`wait` likewise, and the other seconds are more than 0. The counts of `[oo.pages]`,
`silent_intervals` and `scrollback_lines` are whole numbers of 1 or more, and `[oo.limits]` takes whole numbers of 0 or
more. An unknown table or key of `[oo]`, or a value that is not what its key takes, refuses the file. These
are what Outer Office asks of Universal Exports and how it paces itself; what the gateway allows
is the `unex` record's ([Universal Exports limits](#universal-exports-limits)), and Moneypenny's
view bounds come from the gateway's `[assistant.views]`.

The build define `OO_API` beats `[unex] url`, which beats the loopback default. A theme the
person chooses (`oo.theme`, kept on the device or in the browser) beats `[oo.appearance] theme`,
which beats Night Ops; an unknown theme id is logged and read as the default. In `[oo.chat]`
each value is a positive number (`transition_ms` a whole one, 0 allowed; `connect_timeout` in seconds) and `width` lies
between `min_width` and `max_width`, or the file is refused; a width the person drags to
(`oo.chat.<person ID>`) beats `width` and is kept within the limits. The system's reduced
motion setting turns transitions off whatever `transition_ms` says.
`[oo.routes] text_bytes` (1 or more, default 128) bounds the `text` filter of a page link: a web
URL or one of Moneypenny's links with a longer one is refused like any other bad link. Keep it
equal to the `unex` record's `route_text_bytes`, or her links that one accepts and this one
does not show as a refused view. The operator
menu on desktop writes `[unex] url` back in the format the file declares, keeping its `#!`
line, its comments and every other program's section. The address applies at the next start.
Below it, the same schema draws `[oo]` as a form: on desktop saving writes back what differs from
the defaults, replacing the `[oo]` tables with any comment inside them, and applies at the next
start; on the web it shows the deployment's file and changes nothing.

## Agent handler settings

An Agent's `handler_settings` table in the roster (`agents` family) configures its handler
provider, beside what the `station` record's `[providers.<id>]` table says; a handler refuses a
key it does not know. Codex takes none. The Claude handler takes `allowed_tools` and
`denied_tools`, lists of Claude Code permission rules that change what a Mission may call: both
empty by default, and a rule that adds to the list is reported as a weakening ([What a Claude Code
Mission may call](agents.md#what-a-claude-code-mission-may-call)). The `acp` handler's keys (`command`,
`args`, `env`, `meta`, `model_option`, `effort_option`, `protocol`, `unconfined_desk` and the
timeouts `initialize_timeout_ms` 20000, `request_timeout_ms` 30000, `start_timeout_ms` 60000,
`close_grace_ms` 2000) are in [the Agent Client Protocol](acp.md#settings). A Theater's `runner`
table bounds a Station's connection to its field runner with `connect_timeout_ms` 30000,
`initialize_timeout_ms` 20000 and `request_timeout_ms` 30000
([Theaters](theaters.md)).

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
| `loadout`, `sandbox`, `mcp`, `overlay`, `secret`, `pool` | `loadouts` | yes | no | live | `[loadout.<agent>]` (its `env` holds non-secret values, shown to anyone who reads it), `[sandbox.<name>]`, `[mcp.<name>]`, `[overlay.<name>]` ([overlays](agents.md#overlays)), `[secret.<name>]` (references, never values), `[pool.<id>]` |
| `cables` | `cables` | yes | yes | restart | the whole file: grants, inbound, channels, limits |
| `plugin` | `plugins` | yes | no | restart | `[[library]]` entries, keyed by their `file` |
| `station` | `station` | yes | yes | restart | the whole file: the Station's runtime settings |
| `tls` | `tls` | yes | yes | restart | the whole file: the Station TLS policy |
| `access` | `access` | no | yes | live, in Universal Exports | the whole file: `[role.<name>]`, `[[rule]]`, `chief` and `delegable` |
| `operation_access` | `operation_access` | no | no | live, in Universal Exports | `[operation_access.<id>]`: an Operation's own `[role.<name>]` and `[[rule]]` |
| `unex` | `unex` | no | yes | restart | the whole file: Universal Exports' sign-in, limits and assistant |
| `compartment` | `compartments` | no | no | at each call | `[compartment.<id>]`: [need-to-know access across Operations](compartments.md) |
| `review` | `reviews` | no | no | live | `[review.<name>]`: [review policies](review.md) |
| `watch` | `watches` | no | no | live | `[watch.<scope>]`: [watches](watches.md), keyed `m`, `operation:<codename>` or `station:<id>` |

A Station polls its provider every `[provider] poll` seconds. A family that applies live
reaches requests, the scheduler and new Sessions at once; running Sessions keep what they
started with. A changed family that applies at restart is logged and listed at check-in in
`config.needs_restart` by name. A revision that fails its checks is refused once: logged,
audited as `config.refused`, and reported at check-in.

The `station` record's own keys; seconds may be fractional. Its tables bound and pace the
Station's work ([Station runtime limits](#station-runtime-limits)), and its `[providers.<id>]`
tables configure providers.

<!-- schema:record.station:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `max_concurrent_missions` | 4 | **Sessions at once.** Agent Sessions it runs at once. |
| `stall_timeout` | 300 | **Stall a silent Session after.** Seconds without provider or terminal activity before a Session stalls. |
| `loop_timeout` | 300 | **Stall a repeating Session after.** Seconds without novel terminal output before repeated output stalls a Session. |
| `loop_min_lines` | 50 | **Repeated lines before a loop counts.** Repeated terminal lines required before loop detection applies. |
| `blocked_after` | 10 | **Mark a Session blocked after.** Quiet seconds before a prompt-shaped terminal tail marks a Session blocked. |
| `max_runtime` | none | **Longest a Session runs.** Longest a Session runs, in seconds. |
| `history` | 4,096 | **Lines remembered for loop detection.** Distinct terminal lines kept for repeated-output detection. |
| `normalize_digits` | `false` | **Ignore digits when comparing lines.** Whether digit runs are normalized when comparing terminal lines for loops. |
| `roles` | both | **What this Station does.** `control` (elects Headquarters) and `execution` (runs assigned Missions). |
| `labels` | none | Placement labels. |
| `terminal` | see meaning | **Terminal provider.** The terminal provider harnesses run in. Default: one that suits the handler. |
| `resources_every` | 60 | **Sample a Theater's resources every.** Seconds between samples of each remote Theater's resources; 0 leaves only the requirements probe. |
| `safe_house_caches` | see meaning | **Rebuildable cache directories.** Directory names the Head of Station may remove from a finished Mission's safe house. Default: `target`, `node_modules` and `.dart_tool`. |
| `safe_house_cache_age` | 604,800 (7 days) | **Leave a finished safe house alone for.** Seconds a finished Mission's safe house is left alone before its caches may go. |
| `partial_line_bytes` | 4 KiB | **Longest partial terminal line.** Bytes of a terminal line the watchdog holds before it counts as a line. |
| `providers` | none | **Provider configurations.** `[providers.<id>]`: provider `<id>`'s Guatiao configuration when the Station builds it with none of its own (handlers, terminals and safe houses). |
<!-- schema:record.station:end -->

The `tls` record is a coqui-peer-tls policy document (`profile = "modern" | "intermediate" |
"legacy"`, `cipher_profiles`, `tls_versions` and the rest), read over the library's default;
whatever it gives up against that default is logged and sent at check-in as `tls_weakened`.

The `unex` record's own keys, and its tables `[gateway]`, `[controls]`, `[timeline]`, `[login]`
and `[text]` ([Universal Exports limits](#universal-exports-limits)). A credential written inline
in a provider's configuration is reported as a weakening: name a file there instead.

<!-- schema:record.unex:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `authenticator` | none | **Sign-in provider.** The Authenticator provider ID for user sign-in. Nobody signs in. |
| `authenticator_config` | none | **Sign-in provider's configuration.** The Authenticator's configuration. |
| `directory` | none | **Directory provider.** The Directory provider ID; required when user access is configured. |
| `directory_config` | none | **Directory provider's configuration.** The Directory's configuration. |
| `authorizer` | none | **Custom authorizer.** A custom Authorizer provider ID, in place of the `access` record's role policy; the gateway's chief tier still applies beside it. |
| `authorizer_config` | none | **Custom authorizer's configuration.** The custom Authorizer's configuration. |
| `accounts` | see meaning | **Accounts provider.** The Accounts provider ID that keeps the accounts people sign in with, for the account routes. Default: the Authenticator when it offers accounts too. |
| `accounts_config` | none | **Accounts provider's configuration.** The Accounts provider's configuration. |
| `proxy_user_header` | none | **Reverse proxy's user header.** The sanitized header a trusted reverse proxy sets to the signed-in user. |
| `proxy_lookup_by` | `name` | **Directory attribute of the proxy's user.** The Directory attribute that resolves the reverse proxy's user. |
| `page_authenticator` | none | **Page viewers' authenticator.** The Authenticator provider ID that names the person asking for one of Moneypenny's pages from the request itself: `coqui_access_mtls` on the gateway's own TLS, `coqui_access_proxy` behind a front door. It answers only under the pages' base, never for `/v1`. Pages know a person by the page cookie alone. |
| `page_authenticator_config` | none | **Page viewers' authenticator's configuration.** The page authenticator's configuration. |
| `login_tries_per_address` | 5 | **Tries a minute per client address.** Password or key tries a minute from one client address. |
| `login_tries_per_name` | 10 | **Tries in ten minutes per account name.** Tries in ten minutes for one account name, from anywhere. |
| `client_address_header` | none | **Reverse proxy's client address header.** The header a trusted proxy sets to the client address. |
| `access_cache_ttl` | 5 | **Keep a found person or a grant for.** Seconds a found person or a granted action is kept; 0 keeps none. |
| `access_cache_negative_ttl` | 1 | **Keep a miss or a denial for.** Seconds "no such person" or a denial is kept; 0 keeps none. |
| `access_cache_entries` | 10,000 | **Lookups and decisions kept.** The most lookups, and decisions, kept; 0 keeps none. |
| `assistant` | `true` | **Serve Moneypenny.** Whether Moneypenny is served. |
| `assistant_agent` | `moneypenny` | **The Agent that is Moneypenny.** The roster Agent that is Moneypenny. |
| `route_text_bytes` | 128 | **Longest route text filter.** The longest `text` filter an Outer Office route may carry, in bytes: in Moneypenny's links and cards, and in the page a message was sent from. 0 reads as 1. |
| `provider_config_depth` | 64 | **Deepest provider configuration.** How deeply nested a provider configuration may be. |
<!-- schema:record.unex:end -->

### Universal Exports limits

The `unex` record's tables bound and pace the gateway, and apply at restart. They follow the
[Limits](#limits) rules: a count or size is a whole number and `null` or `"none"` takes the
bound away, seconds may be fractional, a value looser than its default is reported at start,
and only a zero where nothing could pass is refused.

```toml
[gateway]
sessions = 32                         # live Sessions, instead of 16
frame_bytes = "none"                  # no bound on a frame (a weakening)

[controls]
pending_per_station = 8
```

`[gateway]`

<!-- schema:record.unex.gateway:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `sessions` | 16 | **Live Sessions.** Live provider Sessions. |
| `clients_per_session` | 12 | **Browser connections per Session.** Simultaneous browser connections to one provider Session. |
| `scrollback_events` | 64 | **Frames kept for a late client.** Value frames kept per provider Session for a client that joins late. |
| `frame_bytes` | 256 KiB | **Largest frame.** Bytes of one WebSocket or ingress frame. A Station is told at registration how much terminal output that leaves room for, and splits longer output to fit. |
| `input_frames_per_second` | 40 | **Browser input frames a second.** Browser input frames accepted per connection per second. |
| `input_queue` | 64 | **Queue to the Stations.** Capacity of the queue between the gateway and its Stations, at least 1. |
| `fanout` | 8 | **Queue per Session viewer.** Capacity of each per-Session viewer queue, at least 1. |
| `buffer_budget_bytes` | 512 MiB | **All the gateway's buffers.** Bytes the gateway may hold in buffers across every Session and connection; the configuration is refused when its limits could exceed it. |
| `client_slots` | 100,000 | **Browser connection slots.** Browser connections the gateway keeps slots for; the configuration is refused when its Sessions times connections per Session could exceed it. |
| `attach_token_ttl` | 300 | **An attach token lasts.** Seconds an attach token lives, more than 0. |
| `write_lease_ttl` | 30 | **A terminal write lease lasts.** Seconds a terminal write lease lives, more than 0. |
| `station_token_min_bytes` | 32 | **Shortest Station token.** Characters the Station token holds at least; a smaller value is looser. |
| `origin_bytes` | 2 KiB | **Longest public origin.** Bytes of the public origin. |
| `access_token_ttl` | 3,600 | **A sign-in lasts.** Seconds a sign-in that names no lifetime gets, and each renewal gives from then, more than 0 and within `access_token_max_ttl`; longer is looser. |
| `access_token_max_ttl` | 86,400 (a day) | **Longest token a request may ask for.** The longest lifetime a request for a token may name, in seconds. |
| `session_max` | 43,200 | **Longest renewed sign-in.** How long renewing keeps a sign-in going, in seconds from when it was made; the password is not asked again until then. `0` turns renewal off, `"none"` lets it run on. |
| `session_cookie` | `true` | **Keep a browser's sign-in in a cookie.** Whether a browser's sign-in also gets an HttpOnly cookie, so a reload signs it back in up to `session_max` after signing in; with `false`, or `session_max` at `0`, a sign-in lives in the app's memory alone. |
| `compartment_audit` | 60 | **Between audit records of a compartment read.** Seconds between audit records of one person's reads of one resource allowed only by one compartment; the app polls, so `0` writes one for every read. |
| `socket_buffer_bytes` | 256 KiB | **Smallest socket write buffer.** Bytes the write buffer of a browser's socket holds at least, whatever `frame_bytes` (it holds twice that when that is more). |
<!-- schema:record.unex.gateway:end -->

`[controls]`

<!-- schema:record.unex.controls:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `controls` | 1,024 | **Controls held.** Controls held for status reads and replays; past it the oldest closed one goes, or the control is refused. |
| `pending_per_station` | 64 | **Open controls per Station.** Controls one Station may have open. |
| `ttl` | 120 | **An open control waits.** Seconds an open control waits for its Station before it expires. |
| `poll_wait` | 1 | **A Station's poll waits.** Seconds a Station's poll waits for a control when it names no wait. |
| `max_poll_wait` | 25 | **Longest poll a Station may ask for.** The longest wait a Station's poll may ask for, in seconds. |
| `idempotency_key_bytes` | 128 | **Longest idempotency key.** Bytes of an idempotency key. |
| `reason_chars` | 512 | **Longest operator reason.** Characters of an operator's reason; longer text is cut. |
<!-- schema:record.unex.controls:end -->

`[timeline]`

<!-- schema:record.unex.timeline:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `page` | 50 | **Items in a page.** Items in a page when the request names no limit, at least 1. |
| `max_page` | 200 | **Most items a page may ask for.** The most items a page may ask for. |
| `text_chars` | 256 | **Longest item text.** Characters of one item's text; longer text is cut. |
| `audit_page` | 100 | **Audit records in a page.** Records in a page of the audit log when the request names no limit, at least 1. |
| `max_audit_page` | 500 | **Most audit records a page may ask for.** The most records a page of the audit log may ask for. |
<!-- schema:record.unex.timeline:end -->

`[login]` (the tries allowed are the flat `login_tries_per_address` and `login_tries_per_name`)

<!-- schema:record.unex.login:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `name_bytes` | 64 | **Longest account name counted.** Bytes of an account name the tries are counted under; longer names are cut. |
| `forget_every` | 256 | **Tries between sweeps of old counters.** Tries between sweeps that forget old counters, at least 1. |
| `address_window` | 60 | **One address's tries are counted over.** Seconds one client address's tries are counted over. |
| `name_window` | 600 | **One account name's tries are counted over.** Seconds one account name's tries are counted over. |
<!-- schema:record.unex.login:end -->

`[text]`

<!-- schema:record.unex.text:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `agent_reason_chars` | 512 | **Longest reason for lowering a clearance.** Characters of the reason that lowers an Agent's clearance. |
| `green_ink_max_grant` | 31,536,000 (365 days) | **Longest green ink grant or clearance.** The longest a green ink grant or a clearance may last, in seconds. |
| `green_ink_note_chars` | 1,024 | **Longest green ink note.** Characters of a green ink note. |
| `green_ink_expired_window` | 2,592,000 (30 days) | **Expired green ink listed by default.** How far back the list of expired green ink requests reaches when it is not told. |
| `held_reason_chars` | 1,024 | **Longest reason for a held brief's decision.** Characters of the reason for a held brief's decision. |
| `roster_target_bytes` | 512 | **Longest roster target line.** Bytes of a roster record's target line; longer text is cut. |
| `roster_key_bytes` | 128 | **Longest roster key.** Bytes of a roster record's key. |
| `access_name_bytes` | 128 | **Longest role, Operation or account name.** Bytes of a role name, an Operation ID or an account name in the access and account routes. |
| `mission_title_chars` | 512 | **Longest Mission title.** Characters of the title of a Mission a person starts. |
| `mission_prompt_bytes` | 64 KiB | **Longest Mission instructions.** Bytes of the instructions of a Mission a person starts. |
<!-- schema:record.unex.text:end -->

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
read_chunk_bytes = 16384
```

`[providers.<id>]` is the Guatiao configuration a Station hands provider `<id>` when it builds
it with none of its own: handlers, terminals, safe houses and the `local` transport. The other
transports take theirs from the Theater's `access` ([Theaters](theaters.md)), Sources from the
Operation's source settings and the ephemeral Theater provider from the template's `config`. A
Station hands its field runners the `[runner]` table and the `[providers.<id>]` tables
([The field runner](theaters.md#the-field-runner)).

Each provider's bounds and waits are keys of its own, every one defaulting to the value the
provider applied before it was a key; the provider's API header lists them with their units
(`_ms` milliseconds, `_secs` seconds, `_bytes` bytes):

| Provider | Keys |
| --- | --- |
| `claude` | `json_line_bytes`, `csi_bytes`, `tail_lines`, `tail_line_bytes`, `close_wait_ms`, `exit_wait_ms` |
| `codex` | those of `claude` and `context_file_bytes`, `start_timeout_ms`, `request_timeout_ms`, `sandbox_probe_timeout_ms`, `sandbox_probe_lifetime_secs`, `sandbox_error_chars`, `tool_check_lifetime_secs`, `tool_command_timeout_ms` |
| `pipe` | `stderr`, `read_chunk_bytes`, `drain_quiet_ms`, `kill_wait_ms`, `stderr_tail_bytes`, `swept_fds`, `tree_scans` |
| `pty` | `read_chunk_bytes`, `drain_quiet_ms`, `kill_wait_ms`, `tree_scans` |
| `git` | `git_timeout_ms`, `measure_every_ms` |
| `copy` | `name_chars` |
| `local` (transport) | `reader_grace_ms` |
| `adb`, `container` (transport settings) | `reader_grace_ms` |
| `ssh` (transport settings) | `connect_timeout_secs`, `exit_grace_ms`, `keepalive_secs`, `keepalive_max` |
| `github`, `gitlab`, `jira` (Source settings) | `request_timeout_secs`, `page_size`, `max_pages`, `settling_secs` |
| `ephemeral-container` (Theater provider `config`) | `engine_timeout_ms`, `engine_output_bytes`, `reader_grace_ms` |

`[scheduler]`

<!-- schema:record.station.scheduler:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `event_queue` | 256 | **Events queued per Session.** Events of one Session queued for its event pump; past it the oldest is dropped. |
| `interval` | 0.5 | **Between scheduler rounds.** Seconds between scheduler rounds. |
| `retry_delay` | 5 | **Retry a failed Mission start after.** Seconds before a Mission whose start failed is tried again. |
| `check_in` | 15 | **Between check-ins with the Registry.** Seconds between the Station's check-ins with the Registry. |
| `intake_attempts` | 3 | **Tries to place a brief.** Times a brief is placed before it is filed as failed. |
| `role_catalog_bytes` | 512 KiB | **Largest role catalog and placement policies.** Bytes of the Agent role catalog and of the placement policies a check-in carries; past it, each is left out. |
| `shutdown_wait` | 2 | **Wait for each loop when stopping.** Seconds the Station waits for each of its loops when it stops. |
<!-- schema:record.station.scheduler:end -->

`[publisher]`: publication to Universal Exports.

<!-- schema:record.station.publisher:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `request_timeout` | 2 | **Longest request to Universal Exports.** Seconds one request to Universal Exports may take. |
| `queue` | 64 | **Events waiting to publish.** Events waiting for Universal Exports; past it output gives way, oldest first, and a Session's latest state is kept. |
| `terminal_bytes` | 192 KiB | **Terminal output in one frame.** Bytes of terminal output in one published frame. Longer output goes as several, and what Universal Exports states at registration lowers it. With room for its header, the default is the gateway's default `frame_bytes`. |
| `pre_bind_terminal_bytes` | 1 MiB | **Terminal output kept before a Mission binds.** Bytes of terminal output kept for a Session not yet bound to its Mission; past it the oldest goes. |
| `publish_retry_delay` | 0.5 | **Retry a failed publication after.** Seconds before a failed publication is tried again; each try doubles it, up to `retry_max_delay`. |
| `retry_max_delay` | 30 | **Longest pause between tries.** The longest pause between tries, of a publication or of an input or control poll. |
| `refresh_every` | 15 | **Register live Sessions again every.** Seconds between registering every live Session again, which is how a gateway restarted during a quiet spell learns of them. |
| `input_retry_delay` | 0.1 | **Retry a failed input or control poll after.** Seconds before a failed input or control poll is tried again; each try doubles it, up to `retry_max_delay`. |
| `control_wait` | 1 | **Control poll held open.** Seconds Universal Exports holds a control poll open; kept under `request_timeout`. |
| `input_wait` | 20 | **Input poll held open.** Seconds Universal Exports holds an input poll open; a keystroke ends it at once, so a long wait costs no delay. The gateway's `max_poll_wait` bounds it. |
| `control_queue` | 16 | **Mission controls queued.** Mission controls queued for the scheduler. |
| `control_report_attempts` | 3 | **Tries to report a control's outcome.** Times the outcome of a control is reported. |
<!-- schema:record.station.publisher:end -->

`[sources]`

<!-- schema:record.station.sources:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `poll` | 60 | **Between polls of a Source.** Seconds between polls of a Source whose `interval_secs` is not set. |
| `min_poll` | 10 | **Shortest poll of a Source.** The shortest interval a Source is polled at, in seconds; a smaller `interval_secs` is raised to it, and 0 takes the floor away. |
| `report_every` | 15 | **Report finished briefs every.** Seconds between rounds reporting finished briefs to their Sources. |
| `sleeper_every` | 20 | **Fire Sleepers every.** Seconds between rounds firing Sleepers. |
| `outbox_every` | 5 | **Send the tracker outbox every.** Seconds between rounds sending the outbox of tracker writes. |
| `reply_every` | 60 | **Read new issue comments every.** Seconds between rounds reading new comments on the issues Missions are linked to. |
| `outbox_batch` | 20 | **Outbox entries sent per round.** Outbox entries sent in one round. |
| `send_attempts` | 5 | **Tries to send an outbox entry.** Sends tried before an outbox entry fails. |
| `retry_backoff` | 30 | **Retry a failed send after.** Seconds before a failed send is tried again; each attempt doubles it. |
| `replies_read` | 10 | **Replies shown of a linked issue.** Stored replies `issue_read` shows of a linked issue, newest first. |
| `reopen_watch_every` | 300 | **Read settled items every.** Seconds between rounds reading the newest settled items again, so a label taken off one is seen and its return reopens it. |
| `reopen_watch` | 20 | **Settled items read per round.** Settled items read in each of those rounds, per tracker instance, newest first; each is one request to the tracker. |
| `read_wait` | 20 | **Wait for a live tracker read.** Seconds one ask for a live tracker read waits at Universal Exports before asking again. |
| `read_bytes` | 16 KiB | **Longest live tracker read asked for.** The most bytes of one live tracker read Universal Exports asks for. |
<!-- schema:record.station.sources:end -->

`[transports]`

<!-- schema:record.station.transports:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `reprobe_after` | 60 | **Probe a failed Theater again after.** Seconds before a Theater the Station could not reach is probed again. |
| `report_every` | 300 | **Look a Theater over every.** Seconds between looks over a Theater for its requirements and ambient credentials. |
<!-- schema:record.station.transports:end -->

`[recovery]`

<!-- schema:record.station.recovery:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `first_reattach_delay` | 5 | **First re-attach try after.** Seconds before the first try to re-attach to a Session; each failed try doubles it. |
| `max_reattach_delay` | 60 | **Longest wait between re-attach tries.** The longest wait between tries, in seconds. |
| `give_up_tries` | 10 | **Tries to give a lease up.** Tries at a Registry step a concurrent renewal can refuse, when the Station gives a lease up. |
| `notes_every` | 15 | **Write a Session's recent events every.** Seconds between writes of a Session's recent events to its record. |
| `probe_timeout` | 30 | **Longest look at a Theater.** Seconds the look at whether a Session's Theater still answers may take. |
<!-- schema:record.station.recovery:end -->

`[vetting]`

<!-- schema:record.station.vetting:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `findings` | 16 | Findings kept per verdict. |
| `review_work` | 50 | **Other work a review brief lists.** Other work a review brief lists for the reviewer. |
| `detail_bytes` | 256 | **Longest finding detail.** Bytes of a finding's detail, of the lines a review brief quotes, and of a vetting timeline detail. |
| `phrases` | 256 | **Manipulation phrases per Operation.** Manipulation phrases an Operation may screen for; past it, or past `phrase_bytes`, screened briefs are held. |
| `phrase_bytes` | 256 | **Longest manipulation phrase.** Bytes of one such phrase. |
| `rate_window` | 3,600 | **Rates are counted over.** Seconds of the rolling window hourly rates are counted over. |
| `proposals` | 500 | **Proposals looked at per round.** Pending Mission proposals looked at per Operation each round, within `[registry.limits.proposals] results`. |
| `proposal_request_ttl` | 3,600 | **A proposal's green ink request lasts.** Seconds the green ink request a class authorisation approves for a proposal lasts. |
| `proposal_settle` | 30 | **A proposal waits before it is promoted.** Seconds a proposal waits before M promotes it on its own. |
| `expiry_sweep` | 60 | **Look at rate holds every.** Seconds between looks at rate holds for expiry. |
| `answer_tail_bytes` | 16 KiB | **Review answer searched.** Bytes at the end of a review's answer searched for its verdict; the answer ends the message. |
<!-- schema:record.station.vetting:end -->

`[review]`: reviews of Missions' plans and output ([Review](review.md)).

<!-- schema:record.station.review:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `goals_bytes` | 32 KiB | **Goals a reviewer reads.** Bytes of a Mission's brief a reviewer is given. |
| `submitted_bytes` | 32 KiB | **Report a reviewer reads.** Bytes of an Agent's plan or final message a reviewer is given. |
| `diff_bytes` | 64 KiB | **Change a reviewer reads.** Bytes of a Mission's change a reviewer is given; a change cut here says so. |
| `commits` | 100 | **Commits a reviewer is shown.** Commits listed for a reviewer. |
| `reasons` | 16 | **Reasons kept per vote.** Reasons kept from one vote, and told to an author sent back. |
| `reason_bytes` | 1 KiB | **Longest reason.** Bytes of one reason, and of a line a review records. |
| `answer_tail_bytes` | 16 KiB | **Reviewer's answer searched.** Bytes at the end of a reviewer's answer searched for its vote; the vote ends the message. |
| `write_tries` | 16 | **Tries of a contested write.** Times a Station tries a write to a review another Station keeps writing first, before it logs that and leaves it for the next round. |
| `station_window` | 300 | **Handlers reported within.** How recent a Station's check-in must be for the handlers it reports to count when a panel is filled. |
<!-- schema:record.station.review:end -->

`[accounts]`: pool accounts.

<!-- schema:record.station.accounts:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `reset_grace` | 15 | **Wait past a reported reset.** Seconds past a reported reset before a Session parked on a limit resumes. |
| `unknown_use_permille` | 500 | **Use assumed when none is reported.** Thousandths used an account whose provider reported nothing counts as, 0 to 1,000. |
| `claim_ttl` | 60 | **A claim on an account lasts.** Seconds a claim on an account lasts unless the Station renews it at check-in. |
| `limit_horizon` | 691,200 (8 days) | **Longest a limited account stays out.** The longest a limited account is kept out, in seconds, whatever reset time its provider reports. |
| `login_timeout` | 60 | **Longest account login.** Seconds a pool account's login command may take. |
| `models_every` | 3,600 | **Look at the models every.** Seconds between this Station's looks at the models each account and route may use; 0 never looks on its own. |
| `model_probes` | 4 | **Model listings at once.** Accounts and routes a look at the models asks at once; `null` asks every one at once, and 0 asks none, so a look changes nothing. |
| `model_probe_timeout` | 20 | **Longest model listing.** Seconds a look waits for one account's or route's model listing; one that takes longer keeps the list it had before, or is unknown. |
| `renew_every` | 600 | **Between login renewal rounds.** Seconds between the Headquarters Station's rounds over the logins kept in the Registry; 0 runs none. |
| `renew_within` | 3,600 | **Renew a login this long before it ends.** Seconds before a login's known end at which a round renews it: well before its harness's own threshold, so a Session's harness has no reason to. |
| `renew_after` | 518,400 (6 days) | **Renew a login of unknown end after.** Seconds since a login was last renewed at which a round renews it, where its end cannot be read. |
| `renew_timeout` | 120 | **Longest login renewal.** Seconds a login's renewal command may take; never past its lease. |
| `renew_lease` | 300 | **A login's renewal lease lasts.** Seconds a Station holds a login's renewal lease; another may take it after that. |
| `renew_backoff` | 300 | **First wait after a failed renewal.** Seconds before the first retry of a failed renewal; each failure doubles it. |
| `renew_backoff_max` | 21,600 | **Longest wait after a failed renewal.** The longest wait between retries of a failed renewal, in seconds. |
| `renew_attempts` | 5 | **Failed renewals before a person is needed.** Failed renewals in a row before an account is marked as needing a person; `null` never marks one. |
<!-- schema:record.station.accounts:end -->

`[approvals]`: green ink steps.

<!-- schema:record.station.approvals:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `kind_bytes` | 32 | **Longest green ink step kind.** Bytes of the kind of a step an Agent asks about. |
| `target_bytes` | 256 | **Longest green ink step target.** Bytes of the target of a step an Agent asks about. |
| `classify_nesting` | 4 | **Nesting followed when classifying a command.** How deep nested shells (`sh -c "sh -c ..."`) and git aliases are followed when a command is read; the harness gate gets it in `AI6_GREEN_INK_NESTING`. |
| `checked_steps` | 64 | **Steps checked at once.** Steps one `check_approval` call may ask about. |
<!-- schema:record.station.approvals:end -->

`[harness]`: a working directory's harness configuration under `harness.project_settings =
"pinned"`, and a skill Moneypenny's desk installs.

<!-- schema:record.station.harness:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `manifest_file_bytes` | 1 MiB | **Largest harness configuration file.** Bytes of one file of a project's harness configuration. |
| `manifest_bytes` | 16 MiB | **Largest harness configuration.** Bytes of the whole configuration. |
| `manifest_files` | 4,096 | **Files in a harness configuration.** Files in the configuration. |
<!-- schema:record.station.harness:end -->

`[overlays]`: reading the [overlays](agents.md#overlays) loadouts and Operations issue, as `ai6
overlay pin` reads them, whether one without a pin is issued, and delivering and preparing them for
a Session. `unpinned = true` is a weakening, named each time such an overlay is issued.

<!-- schema:record.station.overlays:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `manifest_bytes` | 1 MiB | **Largest overlay manifest.** Bytes of an overlay's `overlay.cfg`, or of one of its `aos` env files. |
| `manifest_depth` | 64 | **Deepest overlay manifest.** Nesting of a manifest's arrays and tables, its top table being the first level. |
| `files` | 4,096 | **Files in an overlay.** Files in one overlay, the excluded names (`.git`, `node_modules`, ...) not counted. |
| `bytes` | 64 MiB | **Largest overlay.** Bytes of one overlay's files together. |
| `file_bytes` | 16 MiB | **Largest overlay file.** Bytes of one file of an overlay. |
| `unpinned` | `false` | **Issue overlays without a pin.** Issue an overlay whose `[overlay.<name>]` has no pin, reporting it as `unpinned` each time; without this such an overlay is refused. A weakening. |
| `context_bytes` | 24 KiB | **Largest overlay context.** Bytes of the rules, routing and context a Session's overlays add to its initial context together. Past it the Session is refused, naming this setting, rather than its text cut. |
| `fetch_timeout` | 300 | **Longest overlay fetch.** Seconds a fetch of one overlay's Git source may take. |
| `deps_timeout` | 600 | **Longest dependency install.** Seconds installing one overlay's dependency file may take. |
| `output_bytes` | 1 MiB | **Largest output of an overlay's step.** Bytes of each output stream kept from an overlay's setup, env script or package manager. |
| `dotagents_pyz` | none | **dotagents for overlay commands.** The `dotagents.pyz` on the Station that runs the `cmds/` of an overlay holding them. An overlay with `cmds/` is refused without it. |
| `dotagents_pin` | none | **Pin of dotagents.** The SHA-256 `dotagents_pyz` must have, `sha256:<hex>`; a mismatch refuses. Without one it is refused unless `unpinned` allows it. |
<!-- schema:record.station.overlays:end -->

`[provision]`: probing and provisioning Theaters.

<!-- schema:record.station.provision:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `probe_timeout` | 60 | **Longest requirements probe.** Seconds the requirements probe, and a resources sample, may take. |
| `command_timeout` | 120 | **Longest provisioning command.** Seconds one provisioning command may take. |
| `name_bytes` | 64 | **Longest required program name.** Bytes of a required program's name; a longer one fails the probe. |
| `version_chars` | 200 | **Longest program version kept.** Characters of a program's version kept from the probe. |
<!-- schema:record.station.provision:end -->

`[secrets]`

<!-- schema:record.station.secrets:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `lease_ttl` | 3,600 | **A secret's lease lasts.** Seconds the leases of a Session's loadout and pool account secrets last. |
| `max_lease_ttl` | 3,600 | **Longest secret lease.** The longest lease, in seconds; a green ink secret's lease lasts as long as its approval, up to it. |
| `min_matched_bytes` | 6 | **Shortest secret looked for.** Secrets shorter than this are not looked for in what an Agent writes outward: a short one would refuse ordinary words. Smaller is looser. |
<!-- schema:record.station.secrets:end -->

`[terminal_output]`

<!-- schema:record.station.terminal_output:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `json_line_bytes` | 64 MiB | **Longest line of JSON output.** Bytes of one line of a JSON harness's output held while the rest arrives. |
| `csi_bytes` | 64 | **Longest terminal control sequence.** Bytes of one terminal control sequence before it is read as text. |
<!-- schema:record.station.terminal_output:end -->

`[green_ink]`

<!-- schema:record.station.green_ink:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `gate_timeout` | 30 | **Longest harness gate exchange.** Seconds the harness gate's exchange with the Station may take; the gate gets it in `AI6_GREEN_INK_TIMEOUT`. |
<!-- schema:record.station.green_ink:end -->

`[detached]`: harnesses run detached on another host, over its transport.

<!-- schema:record.station.detached:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `poll` | 0.2 | **Read a detached harness's output every.** Seconds between reads of a detached harness's output. |
| `script_timeout` | 30 | **Longest wrapper script.** Seconds one of the wrapper's scripts may take over the transport. |
| `name_chars` | 64 | **Session name in the wrapper's files.** Characters of the Session's name in the wrapper's file names. |
<!-- schema:record.station.detached:end -->

`[runner]`: field runners, and the Station's side of them.

<!-- schema:record.station.runner:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `pump_poll` | 0.25 | **Check whether to stop after waiting.** Seconds a runner waits for a harness's next event before it checks whether to stop. |
| `feed_poll` | 0.05 | **Look for spooled events every.** Seconds between a runner connection's looks for spooled events to send. |
| `replay_batch` | 256 | **Spooled events taken at once.** Spooled events a runner connection takes from the spool at once. |
| `keep_ended` | 604,800 (7 days) | **Keep an ended Session for.** Seconds an ended Session is kept by its runner. |
| `cable_request_bytes` | 4 MiB | **Largest request through the Cable relay.** Bytes of one request through a runner's Cable relay. |
| `cable_connect_timeout` | 10 | **The Station reaches its Cable endpoint within.** Seconds the Station may take to reach its own Cable endpoint for a request a runner relays. |
| `cable_streams` | 64 | **Cable relay requests open at once.** Requests through a runner's Cable relay the Station answers at once for one Session; past it a request is refused until one ends. |
| `read_wait` | 1 | **One read of a Session waits.** Seconds one read by the Station of a runner's Session waits for an event. |
| `connect_timeout` | 5 | **`ai6 runner connect` reaches the runner within.** Seconds `ai6 runner connect` may take to reach the runner's loopback port. |
| `answer_wait` | 10 | **A runner answers its authentication within.** Seconds a runner that accepted `ai6 runner connect` has to answer its authentication. |
| `start_wait` | 30 | **A started runner answers within.** Seconds a started runner has to answer. |
| `start_margin` | 30 | **Extra time to start over a transport.** Seconds starting a runner over a transport may take past `start_wait`. |
| `start_poll` | 0.05 | **Look at whether a started runner serves every.** Seconds between looks at whether a runner started on this host serves yet. |
| `terminal_poll` | 0.2 | **One read of the connect terminal waits.** Seconds one read of `ai6 runner connect`'s terminal waits before it checks whether to stop or give up. |
| `superseded_close_wait` | 0.2 | **Keep a superseded connection open for.** Seconds a connection another one took a Session from stays open, so the notice saying so is written before the socket closes under it. |
| `retry_pause` | 0.1 | **Shortest pause between tries at a runner.** Seconds at least between the Station's tries at a runner it cannot reach. |
<!-- schema:record.station.runner:end -->

`[api]`: the Station's own HTTP API.

<!-- schema:record.station.api:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `refused_body_drain_bytes` | 64 KiB | **Refused request body read.** Bytes of a refused request's body the Station's own API reads before it answers. |
<!-- schema:record.station.api:end -->

`[audit]`

<!-- schema:record.station.audit:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `names_detail_chars` | 240 | **Secret names a credential event lists.** Characters of the secret names a credential timeline event lists. |
| `target_bytes` | 512 | **Longest refused configuration target.** Bytes of a refused configuration's audit target. |
<!-- schema:record.station.audit:end -->

`[theaters]`

<!-- schema:record.station.theaters:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `ephemeral_name_bytes` | 100 | **Longest ephemeral Theater name.** Bytes of an ephemeral Theater's name; a longer one is cut and given a digest. |
<!-- schema:record.station.theaters:end -->

`[theater_tools]`: the `theater_*` tools of a remote-hands Session.

<!-- schema:record.station.theater_tools:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `output_bytes` | 256 KiB | **Output `theater_run` returns.** Bytes of each stream of a command's output `theater_run` returns. |
| `read_bytes` | 256 KiB | **What `theater_read` returns.** Bytes `theater_read` returns. |
| `timeout` | 120 | **A command may take.** Seconds a `theater_run` command may take unless it asks for another. |
| `max_timeout` | 3,600 | **Longest a command may ask for.** The longest a command may ask for, in seconds. |
| `git_timeout` | 30 | **A git command may take.** Seconds each git command the tools run for green ink may take. |
<!-- schema:record.station.theater_tools:end -->

`[head_of_station]`

<!-- schema:record.station.head_of_station:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `listed_changes` | 20 | **Safe house changes in one report.** Changes of a kept safe house listed in one report. |
| `survey_entries` | 500,000 | **Directory entries one survey walks.** Directory entries one survey of a safe house walks before it reports a partial size. |
| `reason_bytes` | 2,000 | **Longest reason for a safe house deletion.** Bytes of the reason a round gives when it asks to delete a safe house; longer is cut. |
| `audit_target_bytes` | 500 | **Longest provisioning audit target.** Bytes of what a provisioning round's audit record names; longer is cut. |
<!-- schema:record.station.head_of_station:end -->

The `cables` record takes `grants`, `inbound` and `channels`, and the relay's limits:
`max_fanout` (Agent Sessions one broadcast may reach, default 100), `sender_rate` and
`sender_burst` (sends a second and burst per sending Agent, 2 and 20), `scope_rate` and
`scope_burst` (per target scope, 10 and 50), `body_bytes` (UTF-8 bytes of a Cable's body,
16 KiB; `"none"` takes the bound away), `max_ttl` (the longest lifetime a sender may ask for,
in seconds, 86,400; `"none"` for no bound) and `default_ttl` (the lifetime of a Cable that
asks for none, 900, within `max_ttl`). Each must be positive. Universal Exports reads
`body_bytes` and `default_ttl` when it starts, to refuse a Cable Moneypenny proposes with a
longer body and to tell the person how long one that names no lifetime lasts.
`cross_operation` says what it takes for a Cable to reach another Operation: `"compartments"`, the
default, asks a [compartment](compartments.md#cables) as well as the grants; `"station_grants"`
asks the grants alone, a weakening logged at start and reported at check-in.

### Moneypenny's limits

Three tables of the `assistant` record bound and pace Moneypenny. They follow the
[Limits](#limits) rules: a count or size is a whole number or `"none"` (no bound); seconds may
be fractional; a key left out keeps its default. Each bound looser than its default (larger,
or no bound) is a weakening, logged by every Station and
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

`[assistant.limits]`: what passes between Universal Exports and a Station, which both check.
A Station reads it for each job and harness start;
Universal Exports when it starts.

<!-- schema:record.assistant.limits:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `cable_grants` | 64 | **Operations with Cables in one job.** Operations one job grants Cable actions in. |
| `message_bytes` | 32 KiB | **Longest message.** Bytes of one job's message; Universal Exports refuses a person's longer message. |
| `context_bytes` | 24 KiB | **Standing context and recap.** Bytes of a job's standing context and recap together; a recap stays under half. |
| `event_bytes` | 64 KiB | **Longest reported event.** Bytes of one reported event's text; longer text is cut. |
| `events` | 64 | Events in one report. |
| `change_bytes` | 64 KiB | **File content in one commit.** Bytes of file content in one commit. |
| `change_files` | 32 | Files in one commit. |
| `commit_message_bytes` | 4 KiB | **Longest commit message.** Bytes of one commit's message. |
| `shared_bases` | 32 | **Shared change bases.** Shared-change bases one event carries, and Universal Exports keeps for one conversation until their calls arrive. |
| `skills` | 64 | **Skills in one job.** Skills one job gives her. |
| `personal_walk` | 10,000 | **Files looked at per turn.** Files of her personal folder looked at to name what a turn changed. |
| `page_file_bytes` | 8 MiB | **Largest file read for a page.** Bytes of one file the Station reads to publish a page. |
| `page_files` | 200 | **Files read for a page.** Files the Station reads to publish one page. |
| `page_bytes` | 16 MiB | **All the files read for a page.** Bytes over all the files of one page the Station reads. |
| `images` | 4 | **Images in one message.** Images one message carries. |
| `image_bytes` | 5 MiB | **Largest image in a message.** Bytes of one image a message carries. |
<!-- schema:record.assistant.limits:end -->

`[assistant.desk]`: how each Station's desk runs her harnesses. Read for each job and harness
start; `known` and `report_queue` at each use; `job_wait`,
`job_bytes`, `job_retry`, `call_margin`, `report_attempts` and `report_retry` at each poll or
report; `unlocked_stale` when the desk starts.

<!-- schema:record.assistant.desk:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `idle` | 600 | **Close an idle harness after.** Seconds a harness waits for the next message, or writes nothing while answering, before it is closed; `"none"` keeps it. |
| `live` | 4 | **Harnesses on one Station.** Harnesses a Station runs or starts at once; past it a message is refused and another Station is asked. |
| `token_margin` | 120 | **Restart before a token expires.** Seconds before its tool token expires that a running harness is restarted with the job's; a job with another token restarts it whatever is left. |
| `job_wait` | 1 | **Job poll held open.** Seconds Universal Exports holds a job poll open. |
| `job_bytes` | 256 KiB | **Largest job.** Bytes of one job as Universal Exports sends it. |
| `call_margin` | 2 | **Extra time for a request.** Seconds a request to Universal Exports may take past what it waits for. |
| `job_retry` | 0.5 | **Retry a failed job poll after.** Seconds before a failed job poll is tried again, and the least between polls when Universal Exports answers early. |
| `report_queue` | 256 | **Reports waiting to be sent.** Ordinary reports (her text, her tool calls) waiting to be sent; past it the newest are dropped. |
| `report_attempts` | 3 | **Tries to send a report.** Times a report is sent before it counts as lost. |
| `report_retry` | 0.2 | **Retry a report after.** Seconds between tries of a report that did not reach Universal Exports. |
| `known` | 1,024 | **Conversations remembered for a resume.** Conversations whose provider Session the desk remembers for a resume. |
| `close_wait` | 10 | **Wait for a closing harness.** Seconds a message waits for the conversation's closing harness. |
| `limit_fallback` | 1,800 | **Out for this long at a limit with no reset time.** Seconds a route or account stays out when a harness reports a limit with no reset time. |
| `remove_retry` | 2 | **Retry removing a directory for.** Seconds removing a closed harness's directory is tried again. |
| `unlocked_stale` | 3,600 | **Sweep abandoned directories after.** Seconds before a directory of working directories left without its lock, or a lock without its directory, is swept. |
| `clone_timeout` | 120 | **Longest clone.** Seconds a clone of one of her repositories may take. |
| `clone_bytes` | 512 MiB | **Largest clone.** Bytes a clone of one of her repositories may hold on disk, then it is given up and removed. |
| `save_bytes` | 16 MiB | **Largest save of her work.** Bytes one save of her work in her personal folder may commit; past it nothing is saved and she is told. |
| `held_cables` | 32 | **Cables held per conversation.** Cables one conversation holds for the person's confirmation; past it a send is refused, and nothing the person has not seen to is dropped to make room. |
| `held_cable_ttl` | 3,600 | **A held Cable waits.** Seconds a Cable held for the person's confirmation waits before it is dropped. |
| `page_upload_timeout` | 60 | **Longest page file upload.** Seconds sending one file of a page to Universal Exports may take. |
<!-- schema:record.assistant.desk:end -->

`[assistant.gateway]`: how Universal Exports keeps her conversations. Read when it starts.

<!-- schema:record.assistant.gateway:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `conversations_per_person` | 20 | **Conversations per person.** Conversations kept for one person; the oldest goes first. |
| `conversations` | 2,000 | **Conversations in all.** Conversations kept in all. |
| `conversation_age` | 604,800 (7 days) | **Keep an untouched conversation for.** Seconds a conversation is kept untouched. |
| `conversation_entries` | 500 | **Entries per conversation.** Entries kept per conversation. |
| `conversation_bytes` | 1 MiB | **Entries kept per conversation, in size.** Bytes of entries kept per conversation; the oldest go first. |
| `person_bytes` | 4 MiB | **Entries kept per person, in size.** Bytes of entries kept per person; a person's views go first. |
| `live_per_person` | 2 | **Live conversations per person.** One person's conversations with a harness running or an answer pending; the least recently used idle harness is closed to make room. |
| `tool_calls_per_minute` | 120 | **Tool calls a minute.** Tool calls one conversation makes in a minute. |
| `token_ttl` | 3,600 | **A tool token lasts.** Seconds a tool token given to a harness is accepted; past it, while that harness's answer runs and its Station was heard from within `answer_deadline`, until the person stops it. |
| `token_reuse_margin` | 600 | **Keep a token if it still has.** Seconds a token must still have to be given to the next message; kept above `[assistant.desk] token_margin`. |
| `answer_timeout` | 60 | **A Station takes up a message within.** Seconds a Station has to take up a message before another is asked. |
| `answer_deadline` | 600 | **Give up on a silent answer after.** Seconds an accepted message may go without word from its Station before it is given up. |
| `proposal_ttl` | 3,600 | **A proposal waits.** Seconds a proposal waits for the person's confirmation. |
| `proposals` | 32 | **Proposals waiting per conversation.** Proposals waiting for the person in one conversation. |
| `message_bytes` | 16 KiB | **Longest message a person writes.** Bytes of one message the person writes. |
| `memory_context_bytes` | 8 KiB | **Memory summaries in her context.** Bytes of her memory's summaries in a new harness's context; the job's context bound applies as well, and she can search for what does not fit. |
| `change_message_chars` | 200 | **Longest shared change message.** Characters of the one-line message of a shared change she proposes. |
| `recap_bytes` | 8 KiB | **Longest recap.** Bytes of the recap a conversation's new harness starts with. |
| `ready_entries` | 100 | **Entries sent when a conversation opens.** Entries the app is sent when it opens a conversation. |
| `ready_bytes` | 512 KiB | **Size of the entries sent then.** Bytes of entries the app is sent when it opens a conversation. |
| `socket_message_bytes` | 64 KiB | **Longest message on the app's socket.** Bytes of one message on the app's socket. |
| `hello_timeout` | 10 | **The app's socket says hello within.** Seconds the app's socket has to say hello. |
| `max_wait` | 25 | **Longest job poll.** Seconds a Station's job poll may be held open, also while `assistant = false`. |
| `sweep_every` | 1 | **Look for jobs nobody took every.** Seconds between an open conversation's looks for jobs no Station took up. |
| `shared_base_wait` | 5 | **A shared change waits for its base.** Seconds a shared change waits for its Station's report of what it was made from. |
| `event_buffer` | 256 | **Events queued for open sockets.** Events queued for one conversation's open sockets; a socket that falls further behind is closed. |
| `route_bytes` | 4 MiB | **Largest route answer to her tools.** Bytes of an AI6 route's answer to one of her tools. |
| `result_chars` | 200,000 | **Longest tool answer she is given.** Characters of one tool's answer she is given; longer is cut. |
| `author_chars` | 200 | **Longest name and email on her commits.** Characters of a person's name and email on her commits. |
| `audit_target_bytes` | 512 | **Longest audit record target.** Bytes of an audit record's target. |
| `proposal_reads` | 16 | **Reads a proposal lists.** Read tool calls of one turn a proposal lists as what she had read. |
| `log_age` | 7,776,000 (90 days) | **Keep her conversation log for.** Seconds an entry of her conversation log is kept; `none` keeps every entry. |
| `log_bytes` | 1 GiB | **Her conversation log, in size.** Bytes of her conversation log kept, every conversation's together; the oldest entries go first, and `none` keeps every entry. |
| `log_prune_every` | 3,600 | **Trim her conversation log every.** Seconds between two passes that drop what her conversation log keeps no longer. |
| `log_flush_timeout` | 10 | **Wait for her conversation log at a stop for.** Seconds a stopping gateway waits for her conversation log to keep what is still queued; `none` waits for all of it. |
| `source_comments` | 10 | **Comments read of a tracker item.** Comments of a tracker item `source_item` reads, the latest. |
| `source_items` | 50 | **Tracker items listed.** Items `source_items` lists. |
| `source_wait` | 30 | **Wait for a live tracker read.** Seconds a live tracker read waits for the Headquarters Station to answer. |
| `source_reads` | 32 | **Live tracker reads waiting.** Live tracker reads waiting for the Headquarters Station at once. |
| `source_answer_bytes` | 1 MiB | **Longest live tracker read answer.** Bytes of the Headquarters Station's answer to one live tracker read. |
<!-- schema:record.assistant.gateway:end -->

`[assistant.pages]`: the pages she publishes and how they are served, read at each use, so a
change applies to the next publish. A quota changed in either direction is a plain change,
never a weakening, and nothing is evicted when one is lowered. Pages are open by default
(scripts, network, feeds and the frame); each restriction is an option.

<!-- schema:record.assistant.pages:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `max_file_bytes` | 8 MiB | **Largest file of a page.** Bytes of one file of a page. |
| `max_page_bytes` | 16 MiB | **Largest page.** Bytes of one page. |
| `max_page_files` | 200 | **Files in a page.** Files of one page. |
| `max_pages` | 50 | **Pages per person.** Pages one person keeps. |
| `max_person_bytes` | 64 MiB | **All of one person's pages.** Bytes of all one person's pages. |
| `max_total_bytes` | 2 GiB | **All pages on this gateway.** Bytes of all pages on one gateway. |
| `inline_bytes` | 192 KiB | **Largest inline page.** Bytes of an inline page's content. |
| `title_chars` | 120 | **Longest page title.** Characters of a page's title. |
| `max_feeds` | 8 | **Feeds in a dashboard.** Feeds one dashboard declares. |
| `feed_reads_per_minute` | 60 | **Feed reads a minute, per link.** Feed reads one link makes in a minute. |
| `staged_uploads` | 64 | **Uploads held from Stations.** Pages Universal Exports keeps staged from Stations at once. |
| `default_ttl` | 86,400 (a day) | **Keep a page for.** Seconds a page lives unless remembered. |
| `open_ttl` | 3,600 | **A view of a page lasts.** Seconds a frame's view token, which the page's files and feeds are named under, is valid. A reload names a new one. |
| `handoff_ttl` | 30 | **A hand-off to a browser lasts.** Seconds a hand-off, the single-use token an app's request gives a browser that holds no page cookie, may wait to be used. A restart ends one not yet used. |
| `handoffs` | 16 | **Unused hand-offs per person.** Hand-offs one person holds unused at once; past it, their oldest ends. |
| `upload_wait` | 30 | **Wait for a Station's files.** Seconds a publish waits for the Station's files. |
| `stage_ttl` | 300 | **Keep an unclaimed upload for.** Seconds a staged upload no call claims is kept. |
| `sweep_every` | 60 | **Sweep expired pages every.** Seconds between sweeps of expired pages, links and staged uploads. |
| `scripts` | `true` | **Run pages' scripts.** Whether a page's scripts run. |
| `network` | `open` | **What a page may reach.** What a page may reach from the browser. |
| `feeds` | `true` | **Dashboards read live feeds.** Whether dashboards may read live feeds. |
| `frame` | `true` | **Show the classification bar.** Whether a page's frame shows the classification bar. The page stays in the gateway's frame either way, which names the view token of its files and feeds. |
| `sharing` | `true` | **People may share pages.** Whether a person may share a page. |
| `max_shares` | 32 | **Subjects a page is shared with.** Subjects one page is shared with. |
| `public` | `true` | **People may make public links.** Whether a person may give a page a public link, which opens it for anyone holding the link, with no sign-in and no feeds. Off, every public link stops answering. |
<!-- schema:record.assistant.pages:end -->

A Station reads `[assistant.limits]` `page_file_bytes` (8 MiB), `page_files` (200) and
`page_bytes` (16 MiB) when it reads a page from her workspace; the gateway's quotas above
decide what is kept.

`[assistant.views]` bounds the views she shows ([Moneypenny](assistant.md#views-she-shows)).

### The directory provider

A directory holds one file per family, named for it: `operations.toml`, `agents.yaml`,
`station.cfg`, and so on, with any of the five extensions. Two files for one family
(`agents.toml` beside `agents.json`) are refused, naming both. `ai6.cfg` in the same directory
is the bootstrap file, which no family is named for. A Station's own records go in
`stations/<id>/`:

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
ai6 config schema                          # every section of ai6.cfg and of the records, with its title
ai6 config schema record.station.scheduler # one section's JSON Schema: titles, defaults, units
ai6 config path                            # the ai6.cfg in use
ai6 config public --output web/ai6.cfg     # the public file a web page reads
ai6 config show agent 007                  # one record, as TOML
ai6 config show theater --scope station:hq-1
ai6 config import ./config                 # a directory's records into a SQL provider
ai6 config import ./hq-1 --scope station:hq-1
ai6 config rm agent 007                    # one record; refused while another names it
ai6 config export ./backup                 # every family and Station scope as TOML files
```

`import` checks the whole directory first and stores its records in one transaction: all of
them or none. `export` writes files readable by their owner only on Unix, since a family may
hold a credential.

`rm` deletes from the configuration every Station reads, so it is careful:

- It reads every family at every scope first. If anything cannot be read, nothing is removed.
- While another record names this one it refuses and lists them, with where in each the name is. Any
  whole string or table key equal to the name counts (`handler`, `provider`, `model` and `effort`
  name something else), so a field that only happens to hold the same text is listed too, and a
  person judges. A secret, often named like a setting (`path`, `token`), is named only where a
  record takes a secret's name: a `secrets` list (a loadout's, a route's), a `secret` value (a pool
  account's), and a pool account of its name that reads a secret and names none. `--force` removes
  it anyway and says what still names it. The provider's own checks
  still apply: the removal is refused if the configuration it leaves is not acceptable, such as an
  Agent another `extends`.
- Each removal and refusal is an audit record in the Registry, `roster.remove` or `roster.refused`
  under `config:roster`, actor `cli:<user>`, written before the removal: a removal the Registry
  cannot record does not happen. A forced one names what still named it.
- On a directory of files, the record leaves its family file and the rest stays as it was, comments
  included, for a TOML file of `[family.key]` tables; a table written another way, a JSON or YAML file
  and a `[[library]]` entry mean the file is written again, without its comments. Universal Exports
  still takes no edit of a directory of files.

## Who can write the provider

Whoever writes the provider decides which provider libraries every Station loads (`plugin`;
each host still applies its own `[station.plugins] profile`), its TLS policy (`tls`), the programs a
Station starts on its own host (a `theater` runner), the files a Station reads secrets from
(`secret`), and who signs in to Universal Exports and what they may do (`unex`, `access`).
Treat the provider's write credentials as sensitive as the strongest role they grant. Every
weakening those records cause is logged at start and sent at check-in.
