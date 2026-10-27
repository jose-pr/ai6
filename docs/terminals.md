# Terminals and providers

An AgentHandler runs inside an AI6 terminal Session so the watchdog can observe its output
and an authorized operator can attach without restarting the provider Session.

## The Station chooses the terminal

Which terminal provider a Session gets is decided by the Station, from what the Agent's
handler declares about its harness (`AgentHandler::stdio`), never from a provider the Agent or
handler names:

- A handler whose harness speaks JSON over standard input and output (`json`: the Claude
  Code and Codex handlers) gets a provider without a terminal (`Terminal::tty` is `false`),
  preferring `pipe` (`ai6::DEFAULT_PIPE`). With none available, the Mission is refused at
  start with the reason.
- Any other handler (`terminal`, the default) gets `pty` (`ai6::DEFAULT_TERMINAL`) when it is
  available, else another provider with a terminal, else any available provider.

`ai6 serve --terminal ID`, or `StationConfig::with_terminal` in Rust, pins one. A pin is used
when it suits the harness; a pin that gives a JSON harness a terminal is refused at Mission
start with the reason, never silently replaced. A field runner chooses the same way among the
providers it loads, under its own `terminal` setting or the Station's. A resumed Session
reuses the terminal it started with.

## Shipped providers

| Provider | Platforms | Terminal | Behavior |
| --- | --- | --- | --- |
| `pty` | Linux, macOS, Windows | yes | `openpty` on Unix; ConPTY and a Job Object on Windows |
| `pipe` | Linux, macOS, Windows | no | Plain pipes; a process group on Unix, a Job Object on Windows |

`ai6-terminal-pty` and `ai6-terminal-pipe` are one crate each: the session logic is shared,
and the platform parts (owning the child's process tree, and for `pty` translating input) live
in `src/unix.rs` and `src/windows.rs`. Build them with the daemon:

```console
cargo build -p ai6 --features cli -p ai6-terminal-pty -p ai6-terminal-pipe
```

Pass the build output directory with `--plugin-path` when starting `ai6 serve`. AI6
discovers Guatiao providers and reports unavailable ones in its diagnostics.

## The `pipe` provider

`pipe` starts the program with a pipe for its standard input and one for its standard output,
so it sees no terminal: a harness that checks (`claude -p` refuses to read its input from a
terminal) reads what is written to it, nothing is echoed, an input line of any length arrives
whole, and the output is the program's bytes exactly, with no CR inserted and no control
sequence. Standard error goes into the same pipe as standard output, keeping the order the
program wrote them in; the provider setting `stderr = "drop"` discards it instead. The output
ends once the program has exited and its pipe has closed, or, when a process it left running
holds the pipe, once it has exited and nothing has arrived for 200 ms. `resize` does nothing.

Kill takes the whole tree: on Unix the program leads a process group of its own, which gets
`SIGKILL`; on Windows it runs in a Job Object that is terminated, and it starts with no console
window. On Windows a bare program name is found on the child's `PATH` with each `PATHEXT`
extension, so `codex` finds npm's `codex.cmd`.

## Terminal mode on Unix

A Unix terminal starts in canonical mode with echo: it holds input until a line ends, cuts a
line at its limit (4095 bytes on Linux; macOS never delivers a line over 1024 bytes), and
copies what is written back into the output. A handler writes each message to its harness as
one line, of any length, and never needs a line editor. So before the program starts, the
`pty` provider turns off canonical mode, echo, `IEXTEN` and XON/XOFF flow control. It keeps
signals (a written `^C` interrupts the program), CR-to-LF on input (Enter typed in an attached
viewer) and output processing (the program's LF arrives as CRLF, which `JsonLines` and an
attached viewer both read). A program that wants another mode sets it itself.

## Adding a terminal provider

A terminal provider implements Guatiao's `ai6_terminal` kind, the
`ai6::directorates::q::terminal` contract: `spawn`, read and write, resize, kill and exit
status, and `tty`, which answers `false` when the program gets no terminal (it defaults to
`true`). Put it under `crates/providers/terminals/<name>/`; the library holds its own
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
the same contract. A harness on another host runs under the runner there, in the terminal
provider the runner chooses, or, on a POSIX host without one, in the detached wrapper the
Station starts over the transport (no terminal, as with `pipe`; a resize does nothing). See
[Theaters](theaters.md).

## Reading a harness through ConPTY

ConPTY renders the console it hosts rather than passing bytes through. Its output starts
with mode and title sequences (`ESC[?9001h ESC[?1004h ESC[m`, `ESC]0;title BEL`,
`ESC[?25h`); it may move the cursor where the program wrote blank lines; input a program
reads a line at a time comes back as an echo; and a line longer than the terminal is wide
is broken with CRLF and a cursor move. The Claude Code and Codex handlers run in `pipe`,
whose stream is the harness's own. They read it with `JsonLines`, which changes nothing on a
clean stream; on a rendered one it removes the controls and rejoins a broken JSON line,
marking it rebuilt, and the Codex handler declines a green ink approval read from such a
line. They also ask for the widest terminal ConPTY takes (32767 columns, `STREAM_COLS`),
which `pipe` ignores. An OSC sequence with no end stops at the line break, and a CSI at the
first byte no CSI holds or after 64 bytes.
([`ai6::directorates::q::terminal`](https://jose-pr.github.io/ai6/rust/ai6/directorates/q/terminal/)).
