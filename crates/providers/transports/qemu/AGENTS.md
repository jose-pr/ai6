# `ai6-transport-qemu` — API header

Header-file-style reference for the `ai6-transport-qemu` crate: the `qemu` transport provider
for AI6, so it can be used without reading its source. It ships inside the package and is
self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features), `data-encoding`,
`serde`, `serde_json` and `toml`. The crate root exports `Qemu`, which implements
`ai6::directorates::q::transport::Transport`; the library is both an `rlib` and the Guatiao
`cdylib` a Station loads. Provider ID: `qemu`. No unsafe code. The guest must run `qemu-ga`
and the VM must expose its channel as a socket chardev.

## Configuration

A Theater's `access = { transport = { provider = "qemu", config = { … } } }`; unknown keys are
refused, and exactly one of `address` and `socket` is required.

| Key | Default | Meaning |
| --- | --- | --- |
| `address` | | `host:port` of the agent's TCP chardev. |
| `socket` | | Path of the agent's Unix socket chardev; Unix hosts only. |
| `reply_timeout_secs` | 30 | How long to wait for one reply from the agent. |

## Capabilities

`run` and `files`. No `terminal` (a serial console is not implemented) and no `forward`.
`probe()` pings the agent and reads `os` and `arch` from `guest-get-osinfo`; a Windows guest
reports `files = false` with a note, since file operations need a POSIX shell.

## Behaviour

- **Connection**: made on first use and again after a failure. Each connection first sends a
  `0xFF` byte and a `guest-sync-delimited` with a fresh ID, and discards everything until its
  own answer. Calls are serialized. A call that fails on the connection drops it and fails;
  nothing is re-sent, because `guest-exec` is not idempotent.
- **`run`**: `guest-exec` with `capture-output`, then `guest-exec-status` polled with a growing
  pause until the program exits. `guest-exec` has no working directory, so a `cwd` runs the
  command line through `/bin/sh -c "cd … && exec …"`. Output past `max_output` is dropped and
  reported as truncated. The agent cannot end a process: a timeout answers `timed_out` and the
  program keeps running in the guest.
- **Files**: the kit's shell file operations run through `guest-exec /bin/sh -c`; standard
  input travels as `input-data`, so bytes survive. A name holding a tab or a newline is not
  listed correctly.
- An error from the agent is `GUATIAO_ERR_INTERNAL` carrying its description; an agent that
  cannot be reached is an error naming the address.

## Gotchas

- This transport was tested against an in-process agent; no real guest was available. The
  command and field names follow the QEMU guest agent reference.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
