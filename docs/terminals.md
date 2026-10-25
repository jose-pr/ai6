# Terminals and providers

An AgentHandler runs inside an AI6 terminal Session so the watchdog can observe its output
and an authorized operator can attach without restarting the provider Session.

## The Station chooses the terminal

Which terminal a Session gets depends on where it runs, not on the Agent or its handler:
every terminal provider fulfils the same contract. Without a pin, a Station uses an
available terminal provider, preferring `pty` (`ai6::DEFAULT_TERMINAL`) when several are
available. `ai6 serve --terminal ID`, or `StationConfig::with_terminal` in Rust, pins one.
A resumed Session reuses the terminal it started with.

## Shipped provider

| Provider | Platforms | Behavior |
| --- | --- | --- |
| `pty` | Linux, macOS, Windows | `openpty` on Unix; ConPTY and a Job Object on Windows |

`ai6-terminal-pty` is one crate: the session logic is shared, and the platform parts
(owning the child's process tree, translating input) live in `src/unix.rs` and
`src/windows.rs`. Build it with the daemon:

```console
cargo build -p ai6 --features cli -p ai6-terminal-pty
```

Pass the build output directory with `--plugin-path` when starting `ai6 serve`. AI6
discovers Guatiao providers and reports unavailable ones in its diagnostics.

## Adding a terminal provider

A terminal provider implements Guatiao's `ai6_terminal` kind, the
`ai6::directorates::q::terminal` contract: `spawn`, read and write, resize, kill and exit
status. Put it under `crates/providers/terminals/<name>/`; the library holds its own
Guatiao entry point, so no forwarding-only plugin crate is needed. Every provider must
terminate the whole agent process tree: a process group on POSIX, a Job Object on
Windows.

## Cursor handshake

A shell under a pty can ask for the cursor position (`ESC[6n`) and wait for an emulator.
Without an attached viewer, AI6's terminal bridge answers the query itself and removes it
from the output, so an attached terminal does not answer a second time.

A transport that reaches another host (`ssh`) opens a terminal there through the same
contract, but a harness does not run on it yet: a Session on a remote Theater runs on the
Station, acting on the Theater through the transport. See [Theaters](theaters.md).
