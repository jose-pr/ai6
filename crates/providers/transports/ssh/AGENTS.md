# `ai6-transport-ssh` — API header

Header-file-style reference for the `ai6-transport-ssh` crate: the `ssh` transport provider
for AI6, so it can be used without reading its source. It ships inside the package and is
self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features), `russh` (the
`ring` crypto provider, so no C toolchain), `russh-sftp`, `tokio`, `serde` and `toml`. The
crate root exports `Ssh`, which implements `ai6::directorates::q::transport::Transport`; the
library is both an `rlib` and the Guatiao `cdylib` a Station loads. Provider ID: `ssh`. No
unsafe code.

## Configuration

A Theater's `access = { ssh = { … } }` becomes this provider's settings (TOML text in a
`TransportConfig`); unknown keys are refused. `Ssh::try_from(TransportConfig)` builds it
without contacting the host.

| Key | Default | Meaning |
| --- | --- | --- |
| `host` | required | Name or address. |
| `port` | 22 | |
| `user` | the Station's user (`USER`, `USERNAME`) | |
| `identity` | first of `~/.ssh/id_ed25519`, `id_ecdsa`, `id_rsa` | Path of the private key file; a leading `~/` is expanded. |
| `host_key` | none | `sha256:HEX`, the SHA-256 of the host's public key blob (the text `awk '{print $2}' key.pub \| base64 -d \| sha256sum` gives). |
| `trust_on_first_use` | `false` | Accept the first key seen when no `host_key` is set. |
| `connect_timeout_secs` | 15 | Connection and login. |

**Keys are never in configuration**: only the path of an unencrypted private key file. A
passphrase-protected key is refused with a message saying so; there is no agent support.

## Host-key policy

- With `host_key`: any other key is refused, and the refusal names both keys.
- Without it and without `trust_on_first_use`: the connection is refused, and the message
  names the key presented so it can be pinned.
- With `trust_on_first_use = true` and no pin: the first key is accepted and remembered for
  the life of the transport; a reconnect that presents another key is refused. The key is
  published in `Capabilities.notes` so it can be pinned afterwards, and the Theater is
  reported as weakened.

## Capabilities

`terminal`, `run`, `files` and `forward` are all declared up front. `probe()` connects,
runs `uname -sm` for `os` and `arch`, and sets `files` to `false` (with a note) when the host
has no SFTP subsystem. `capabilities()` never connects.

## Behaviour

- **Connection**: made on first use, shared by every operation, remade once when a new
  channel cannot be opened because it dropped. Keepalives every 30 seconds, three missed
  ones close it. A command already sent is never re-sent.
- **`run`**: one exec channel. The remote command is
  `cd '<cwd>' && exec env 'K=V' 'argv0' 'arg'…`, every word single-quoted for a POSIX shell,
  so the remote side must have one; a Windows server is not supported. Standard input is
  written and closed. `timeout_ms` sends a `KILL` signal request and closes the channel
  (`timed_out`, no `code`). After the exit status arrives, output and the channel's close are
  awaited at most one more second.
- **`open_terminal`**: a pty of `rows` × `cols` (`TERM` from the spec's `env`, else
  `xterm-256color`) and the same command line. `pid()` is `-1`; `kill()` sends `KILL` and
  closes the channel, and the session reads as exited from then on.
- **Files**: SFTP, opened on first use and reopened if it dies. Paths are the host's; a
  relative path is resolved against the login directory. `create_dirs` makes missing parents.
  A missing file or directory is `GUATIAO_ERR_NOT_FOUND`.
- **`forward`**: a reverse forward (`tcpip-forward`): the host listens on
  `remote_host:remote_port` (port `0` lets it choose; the answer is in `info()`), and each
  connection is carried to the Station's `target`. The server must allow TCP forwarding.
- Calls block: the transport owns a two-thread Tokio runtime.

## Gotchas

- The host-key check runs on every connection, so a host that changes its key is refused
  even in the middle of a long-lived transport.
- An SSH server accepts only a few failed logins; a wrong `identity` can lock a host's
  throttle for a while.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
