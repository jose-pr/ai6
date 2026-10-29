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
**weakenings**: printed at start, and sent at check-in as `bootstrap_weakened`. They are
allowed. A secret's value never appears in a message, a log or `Debug` output.

### `ai6.cfg`

```toml
station = "hq-1"                      # default: made up once and kept in the state directory
state_dir = "/var/lib/ai6"            # default: the platform's state directory
listen = "127.0.0.1:7081"

[provider]
location = "."                        # a directory, sqlite:PATH or postgres://...
password = { file = "db-password" }   # when a SQL location gives none
poll = 5                              # seconds between checks for changes; 0 turns reloading off

[registry]
location = "postgres://db/ai6"        # default: the provider's database when it is SQL,
                                      # else registry/ in the state directory
password = { file = "db-password" }   # default: the provider's, when the Registry is its database
headquarters_lease = 60               # seconds; also sets the SQL connections' timeouts

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

### `ai6ctl`

`ai6ctl` has no file of its own. Its Universal Exports origin is `--server`, else `UNEX_URL`,
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

[assets]
cdn = "none"                          # web only: none, fonts or all (read by the page's bootstrap)
```

The build define `OO_API` beats `[unex] url`, which beats the loopback default. A theme the
person chooses (`oo.theme`, kept on the device or in the browser) beats `[appearance] theme`,
which beats Night Ops; an unknown theme id is logged and read as the default. In `[chat]`
each value is a positive number (`transition_ms` a whole one, 0 allowed) and `width` lies
between `min_width` and `max_width`, or the file is refused; a width the person drags to
(`oo.chat.<person ID>`) beats `width` and is kept within the limits. The system's reduced
motion setting turns transitions off whatever `transition_ms` says. The operator
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
(`control`, `execution`), `labels`, `terminal`, `resources_every`, `safe_house_caches` and
`safe_house_cache_age`; seconds may be fractional. The `tls` record is a coqui-peer-tls policy
document (`profile = "modern" | "intermediate" | "legacy"`, `cipher_profiles`, `tls_versions`
and the rest), read over the library's default; whatever it gives up against that default is
logged and sent at check-in as `tls_weakened`. The `unex` record takes `authenticator`,
`directory`, `authorizer` and their `*_config` objects (handed to the provider as they are),
`proxy_user_header`, `proxy_lookup_by`, `login_tries_per_address`, `login_tries_per_name`,
`client_address_header`, `access_cache_ttl`, `access_cache_negative_ttl`,
`access_cache_entries`, `assistant` (default `true`) and `assistant_agent` (default
`moneypenny`). A credential written inline in one of those configurations is reported as a
weakening: name a file there instead.

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
