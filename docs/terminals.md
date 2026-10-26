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

A program under a pty can ask for the cursor position (`ESC[6n`) and wait for an emulator,
and ConPTY on Windows always asks once before its program's first output and holds that
output until it is answered (`portable-pty` creates every pseudo-console with
`PSEUDOCONSOLE_INHERIT_CURSOR`). On Windows the `pty` provider answers every such query
itself (row 1, column 1, from a thread of its own, so output keeps flowing while a write
waits) and removes it from the output, so a harness starts with nobody attached and an
attached viewer does not answer a second time. On Unix queries pass through to whoever
reads.

A transport that reaches another host and can open a terminal there (`ssh`) does so through
the same contract. A harness on another host runs under the runner there, in the runner's
terminal provider, or, on a POSIX host without one, in the detached wrapper the Station
starts over the transport (no pty; a resize does nothing). See [Theaters](theaters.md).

## Reading a harness through ConPTY

ConPTY renders the console it hosts rather than passing bytes through. Its output starts
with mode and title sequences (`ESC[?9001h ESC[?1004h ESC[m`, `ESC]0;title BEL`,
`ESC[?25h`); it may move the cursor where the program wrote blank lines; input a program
reads a line at a time comes back as an echo; and a line longer than the terminal is wide
is broken with CRLF and a cursor move. The Claude Code and Codex handlers therefore start
harnesses on the widest terminal ConPTY takes (32767 columns, `STREAM_COLS`) and read them
with `JsonLines`, which removes the controls and rejoins a broken JSON line, marking it
rebuilt: the Codex handler declines a green ink approval read from such a line. An OSC
sequence with no end stops at the line break, and a CSI at the first byte no CSI holds or
after 64 bytes.
([`ai6::directorates::q::terminal`](https://jose-pr.github.io/ai6/rust/ai6/directorates/q/terminal/)).
