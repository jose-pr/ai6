# Theaters

A **Theater** is where a Session actually works: the host or environment whose processes
and files the agent harness uses. A Station is the node that places and supervises the
Session; the Theater is the place it works. They are separate on purpose, the way a
Kubernetes node is separate from the container runtime it drives.

A Station reaches a Theater through a **transport**: the way in. `local` is the Station's own
machine, `ssh` is an SSH server, and other transports (a container, a VM, an Android device)
are providers that declare what they can do. A Theater is whatever its transport reaches.

## Declaring Theaters

A Station reaches its Theaters through its own `theaters.toml` in the configuration
directory:

```toml
[theater.build-box]
access = { ssh = { host = "build.example.net", user = "ci", host_key = "sha256:…" } }
labels = { os = "linux", gpu = "yes" }
agents = "auto"
egress = "direct"
harnesses = ["claude"]
workdir = "/srv/work"
```

| Key | Meaning |
| --- | --- |
| `access` | The transport: `"local"`, `{ ssh = { … } }`, or `{ transport = { provider = "…", config = { … } } }` for any other transport provider. |
| `labels` | Labels an Operation's `station_labels` may match, overlaid on the Station's own. |
| `agents` | Where the harness may run: `"auto"` (default), `"local"` or `"remote-hands"`. See [Two ways to work on a Theater](#two-ways-to-work-on-a-theater). |
| `egress` | How the Theater reaches the provider's API: `"direct"` (default), `{ proxy = "http://…" }`, `"station"` (a proxy the Station provides over the transport's `forward`) or `"none"` (no internet). |
| `harnesses` | Harnesses installed on the Theater (`"claude"`, `"codex"`). Another host without a runner needs one here before a harness runs on it, in the [detached wrapper](#the-detached-wrapper). |
| `workdir` | Where a Mission works on this Theater when its Operation names no path there. |
| `runner` | The [field runner](#the-field-runner) that supervises harnesses on the Theater: `{}` for every default, or `command`, `state_dir`, `plugin_paths` and `terminal`. |
| `requires` | What the Theater must have before Sessions run there: `os`, `harnesses`, `tools`, `disk_mb`. See [Fitting a Theater out](#fitting-a-theater-out). |
| `provision` | How `ai6 theater provision` installs what it lacks: `runner`, `plugins`, `install`, `install_timeout_secs`. |

A Station's own host is always reachable as the Theater `local` without any configuration;
declaring `[theater.local]` only adds labels, facts or a runner to it.

## Fitting a Theater out

Q Branch fits Theaters out. A Theater says what Sessions need there and how to install what
is missing, and the Station checks it before it sends work there:

```toml
[theater.build-box.requires]
os = "linux"
harnesses = ["claude>=2.1", "codex"]   # AgentHandler provider IDs, also the command names
tools = ["git", "cargo>=1.89"]
disk_mb = 2048                         # free MiB in `workdir` (the default directory without one)

[theater.build-box.provision]
runner = "dist/linux-aarch64/ai6"      # an ai6 build for the Theater, on the Station
plugins = ["dist/linux-aarch64/libai6_agent_claude.so", "dist/linux-aarch64/libai6_terminal_pty.so"]
install = { git = "sudo dnf install -y git", claude = "npm install -g @anthropic-ai/claude-code" }
install_timeout_secs = 600
```

A program is written `name` (any version) or `name>=version`, compared number by number, so
`2.10` is after `2.9`. Its version is the first number in what `name --version` prints.

**The probe.** A Station looks over each POSIX Theater it reaches through the transport's
`run` (a few `command -v`, `--version` and `df` calls, no writes) when it first contacts it
and every five minutes after. It publishes what it found at check-in: versions, free space,
the requirements that are unmet and why. Placement uses it:

- A Theater whose requirements are unchecked, or whose `os`, `tools` or `disk_mb` are unmet,
  is ruled out with the reason.
- In field mode the harness runs on the Theater, so a Theater that lists `harnesses` hosts
  only Agents whose handler is listed and installed there; the others are ruled out there,
  each with the reason (`harness "claude" is missing on this Theater: claude>=2.1: not
  installed`). Remote-hands mode runs the harness on the Station and is not affected.
- A Theater without `requires` is not checked.

The probe also reports **ambient credentials**: credentials already on the Theater that AI6
did not lease (a `~/.git-credentials` store, a git credential helper, an SSH agent or
private key, a GitHub CLI, Claude Code or Codex login, a provider key in the login
environment). It names where they are, never their values, and each is a weakening the
Station publishes and Moneypenny shows: a Session there can use them whatever its own leases
allow. Removing them is the operator's choice; AI6 does not refuse the Theater.

**Provisioning.** `ai6 theater provision THEATER` fits a Theater out over its own transport,
checking before each step and acting only where the Theater differs:

| Step | Checks | Acts |
| --- | --- | --- |
| `os`, `disk` | the probe | never: a mismatch fails |
| each program in `tools`, then `harnesses` | the probe | runs its `provision.install` command with `sh -c`, then probes again |
| `runner` | SHA-256 of `runner.command` on the Theater | copies `provision.runner` (the Station's own executable when the Theater has the Station's OS and architecture) |
| `plugin NAME` | SHA-256 in the first `runner.plugin_paths` directory | copies each of `provision.plugins` |

Each step prints `ok`, `changed` or `failed`, so a second run on a fit Theater prints `ok`
for every step. A file is written beside its destination and moved over it, so a running
runner is replaced safely. `--check` only looks: a step that would act prints
`would-change`, and nothing on the Theater is written. The command exits non-zero when a
step failed, or with `--check` when one would change. Pass `--plugin-path` for the
Theater's transport provider and `--config-dir` for another configuration directory.

`runner.command` must be a path for the runner to be installed (`~/` is the transport's
default directory, the login user's home over SSH). Provisioning carries no secret: harness
and forge credentials reach a Session as leases, never as files on the Theater.

### SSH access

```toml
access = { ssh = { host = "build.example.net", user = "ci", port = 22,
                   identity = "~/.ssh/ci_ed25519", host_key = "sha256:…" } }
```

`user` defaults to the Station's user, `port` to 22 and `identity` to the first of
`~/.ssh/id_ed25519`, `id_ecdsa` and `id_rsa` that exists. **A key is never in configuration**:
only the path of an unencrypted private key file is, and a passphrase-protected key is
refused. The SSH client is built into the `ssh` transport provider (`russh`), so nothing
depends on an installed `ssh` or on `ControlMaster`, and it behaves the same on every OS. The
remote side must have a POSIX shell.

**Pin the host key.** `host_key = "sha256:HEX"` is the SHA-256 of the host's public key,
the same text a peer ID uses. Get it on the host with:

```bash
awk '{print $2}' /etc/ssh/ssh_host_ed25519_key.pub | base64 -d | sha256sum
```

A host that presents any other key is refused. With no pin, the first contact is refused too,
and the message names the key the host presented so you can pin it.
`trust_on_first_use = true` accepts the first key instead. That is weaker than pinning, so
the Station logs it, `GET /v1/stations` lists it as `weakened`, and the key it trusted is
published in the Theater's capability notes so you can pin it afterwards.

## What a transport can do

Each transport declares its capabilities, and a Station publishes them at check-in with the
Theater's declared facts (`GET /v1/stations`, and the Theater's tooltip in Moneypenny):

| Capability | Meaning |
| --- | --- |
| `terminal` | Start a program on a terminal. On the Station's own host this comes from its Terminal provider. |
| `run` | Run a command to completion and get its output. |
| `files` | Read, write and list files. |
| `forward` | Make a port on the Theater reach an address on the Station. |
| `os`, `arch` | What the host is, once the transport has looked. |

| Transport | Capabilities |
| --- | --- |
| `local` | `run`, `files`; the Station adds `terminal` from its Terminal provider |
| `ssh` | `terminal` (a pty), `run`, `files` (SFTP), `forward` (reverse); `os` and `arch` from a probe |
| `adb` | `run`, `files`, `forward` (`adb reverse` to the Station's loopback); an Android device, so `agents = "remote-hands"` |
| `container` | `run`, `files`; a running Docker or Podman container, files need a POSIX `sh` in it |
| `qemu` | `run`, `files`; a QEMU guest through its guest agent, files need a POSIX `sh` in the guest |

The `adb`, `container` and `qemu` transports are configured with
`access = { transport = { provider = "adb", config = { serial = "emulator-5554" } } }` (and
`{ container = "build", engine = "podman" }`, `{ address = "127.0.0.1:4555" }`); each
provider's API header lists its settings.

A Theater that has not answered yet is probed in the background, so a dead host never delays
a check-in; until it answers the Theater is listed as unreachable with the reason, and
placement rules it out.

## Two ways to work on a Theater

- **Field mode**: the harness runs *on* the Theater, in a terminal there. It needs the
  harness installed and a route to the provider's API (`egress` not `"none"`, and `forward`
  when it is `"station"`). A Theater with a [runner](#the-field-runner) qualifies (another
  host also needs `run`, which is how the Station reaches the runner). Without one, the
  Station's own host needs `terminal` and `files`, and another host needs `run`, `files`, a
  POSIX `os` and a declared harness for the [detached wrapper](#the-detached-wrapper). The
  Mission works in the Operation's path on the Theater when the Operation is bound to it,
  else in the Theater's `workdir`, which must be absolute.
- **Remote-hands mode**: the harness runs on the *Station* and acts on the Theater through the
  transport. It needs only `run`. For hosts that cannot run an agent: an Android device,
  an appliance, an air-gapped VM, a host with no internet.

`agents = "auto"` picks field mode where it is possible and remote-hands mode otherwise;
`"local"` and `"remote-hands"` force one, and a Theater that cannot do what is forced is ruled
out with the missing capability named. Placement previews show the mode of each choice.

In remote-hands mode the Station starts the harness in a scratch directory under its state
directory (`remote-hands/<mission>`, not removed), switches off the harness's own shell and
file tools (Claude Code's `--disallowedTools`; Codex runs in a read-only sandbox that asks
nothing), and gives it four tools on a per-Session authenticated MCP endpoint, bound to the
Mission:

| Tool | Does |
| --- | --- |
| `theater_run` | Run a command (`command` for `sh -c`, or `argv`); returns exit code, stdout and stderr. |
| `theater_read` | Read part of a UTF-8 file. |
| `theater_write` | Write or append a UTF-8 file, optionally creating directories. |
| `theater_list` | List a directory. |

Relative paths are resolved against the Mission's working directory on the Theater: the
Operation's `path` when the Operation is bound to this Theater (its checkout is there), else
the Theater's `workdir`. An Operation with a `path` that is not bound to a Theater is placed
on a remote-hands Theater only if that Theater has a `workdir`. A file tool on a transport
without `files` answers that it is unavailable and points at `theater_run`.

## The field runner

A harness the Station supervises itself is the Station's child: if the Station stops or
dies, so does the work. A **runner** keeps it going. It is the same `ai6` binary in its
Theater mode, `ai6 runner serve`: it loads the same AgentHandler and Terminal providers,
starts harnesses for Stations, and writes every event a harness emits to disk, numbered,
before any Station reads it. It has no scheduler, no Registry and no roster.

```toml
[theater.local]
access = "local"
runner = {}

[theater.build-box]
access = { ssh = { host = "build.example.net", user = "ci", host_key = "sha256:…" } }
harnesses = ["claude"]
workdir = "/srv/work"
runner = { command = "/opt/ai6/bin/ai6", plugin_paths = ["/opt/ai6/lib"] }
```

| Key | Default |
| --- | --- |
| `command` | `ai6` on the Theater's `PATH`; on the Station's own host, the Station's own binary. |
| `state_dir` | `runner` in AI6's state directory on the Theater. |
| `plugin_paths` | None: give the directories or libraries that hold the harness's handler and a terminal provider on the Theater. |
| `terminal` | An available terminal provider on the Theater, preferring `pty`. |

A Station starts the runner when none answers, then starts each Session's harness on it. A
Station that is shut down, killed or fenced out leaves the harness running, and its Mission
stays Working. When the Station starts again it re-attaches to the same runner Session and
receives every event it missed, in order, from the last one it acknowledged (the last
event it took may come twice). A Station that loses the Session's lease lets go the same
way. When a Station goes dark for good, another Station that reaches the same runner (a
Theater with the same ID and a `runner`) adopts the Session and re-attaches in the same way
([Station loss](missions.md#station-loss-adoption)). A Mission parked at a provider limit
resumes in a new harness instead, as before.

The runner listens on a loopback port only, behind a bearer token kept in its state
directory (`token`, readable only by its user). On the Station's own host the Station talks
to it directly. On another host it runs `ai6 runner call` there through the Theater's
transport for each request, and `ai6 runner serve --detach` when no runner answers; a link
that drops costs only time, since the runner keeps every event until a read gets through.
The harness's Cable tools go through a relay the runner gives each Session, with its own URL
and token, which the Station points at its own Cable endpoint (on another host, through a
reverse forward over the transport) and repoints when it re-attaches or the link returns;
while no Station is attached the relay answers that none is.

Run one by hand with `ai6 runner serve [--state-dir PATH] [--plugin-path PATH]...` (or
`--detach` to start it in the background); `ai6 runner call METHOD PATH` sends it one
request. Stopping the runner stops the harnesses it supervises, and a runner that restarts
ends the Sessions it lost with a `failed` event saying why. Its events are in
`sessions/<key>/events.jsonl`; ended Sessions are removed a week after they started, the
next time a runner starts.

The Station's watchdog sees a runner's harness only through its events: the checks that
read terminal output (a blocked prompt, repeated output) do not apply to it.

## The detached wrapper

A host where nothing can be installed still runs a harness if it has one: a POSIX Theater
with `run`, `files` and a declared harness, and no runner (an `ssh` Theater, typically). The
Station starts the harness over the transport in a session of its own (`setsid`, or `nohup`
where there is none), so the link that started it can drop without ending it:

- Its files are in `~/.ai6/detached/<mission>-<id>/` on the Theater (the login user's home
  over SSH; readable only by that user): `env` (a FIFO the Session's environment passes
  through once, so no value is on a command line or on disk; removed after it is read), `in`
  (a FIFO it reads), `out` (everything it writes), `pid` and, once it ends, `exit`. The
  directory stays after the Mission for inspection.
- The Station reads `out` from the last byte it took and writes to `in`. While the link is
  down a read finds nothing new and the harness keeps running; once the transport
  reconnects, the Station reads on from where it stopped, so nothing is lost.
- The harness reaches the Station's Cable endpoint through a reverse forward on a loopback
  port of the Theater, which the transport re-opens on the same port after a reconnect.
- The harness gets the host's login environment plus the Session's, and no terminal: it is
  for harnesses that speak a protocol on standard input and output, as the `claude` and
  `codex` handlers do.

The Station still supervises the harness: shutting the Station down stops it, and one whose
Station died is ended by the next start of the same Mission before another starts. Only a
runner keeps the work going while no Station is attached.

## Reaching a Theater

A Station publishes the Theaters it reaches at each check-in: ID, access (`local`, `ssh` or
`transport`), the SSH host or transport provider, labels, capabilities, `agents`, the kind of
`egress` (never a proxy URL), `harnesses`, `requires` with the last probe's report, and why a
Theater is unreachable. The SSH user, port, key path and `provision` settings stay on the
Station. `GET /v1/stations` and Moneypenny's Stations view list them; a Theater whose
requirements are unmet or unchecked shows a tool icon, and its tooltip says what is missing.

A Session records the Station that supervises it, the Theater it works on and the mode
(`station_id`, `theater_id`, `mode`); Moneypenny shows them on the Mission detail. Records
written before this was tracked lack them and still load, as field mode.

An Operation's `theater` binds it to one Theater by ID. See
[Placement](stations.md#placement) for how the Station, Theater and Agent are chosen and how
to ask why.

## Not yet

- Remote safe houses: a remote-hands Mission has no safe house, so no per-Mission isolation,
  history or cover check; it works in the directory it is given.
- A Station-provided egress proxy (`egress = "station"`) is recorded and requires `forward`,
  but no proxy is started yet.
- A terminal over the `adb`, `container` and `qemu` transports, and a QEMU guest agent that
  can end a process it started (a timeout leaves it running in the guest).
