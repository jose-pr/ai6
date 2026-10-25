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
| `harnesses` | Harnesses installed on the Theater (`"claude"`, `"codex"`), for the day a runner can supervise one there. |
| `workdir` | Where a remote-hands Mission works when its Operation names no path on this Theater. |

A Station's own host is always reachable as the Theater `local` without any configuration;
declaring `[theater.local]` only adds labels or facts to it.

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

- **Field mode**: the harness runs *on* the Theater, in a terminal there. It needs `terminal`
  and `files`, the harness installed, and a route to the provider's API (`egress` not
  `"none"`, and `forward` when it is `"station"`). Until a runner can supervise a harness on a
  remote host, only the Station's own host qualifies.
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

## Reaching a Theater

A Station publishes the Theaters it reaches at each check-in: ID, access (`local`, `ssh` or
`transport`), the SSH host or transport provider, labels, capabilities, `agents`, the kind of
`egress` (never a proxy URL), `harnesses` and why a Theater is unreachable. The SSH user, port
and key path stay on the Station. `GET /v1/stations` and Moneypenny's Stations view list them.

A Session records the Station that supervises it, the Theater it works on and the mode
(`station_id`, `theater_id`, `mode`); Moneypenny shows them on the Mission detail. Records
written before this was tracked lack them and still load, as field mode.

An Operation's `theater` binds it to one Theater by ID. See
[Placement](stations.md#placement) for how the Station, Theater and Agent are chosen and how
to ask why.

## Not yet

- Field mode on a remote Theater: a runner on the Theater that supervises the harness and
  outlives the Station's connection does not exist yet.
- Remote safe houses: a remote-hands Mission has no safe house, so no per-Mission isolation,
  history or cover check; it works in the directory it is given.
- A Station-provided egress proxy (`egress = "station"`) is recorded and requires `forward`,
  but no proxy is started yet.
- A terminal over the `adb`, `container` and `qemu` transports, and a QEMU guest agent that
  can end a process it started (a timeout leaves it running in the guest).
