# `ai6-terminal-pty` — API header

Header-file-style reference for the `ai6-terminal-pty` crate: the `pty` terminal provider
for AI6, so it can be used without reading its source. It ships inside the package and is
self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features) and
`portable-pty`; `libc` and `nix` (`term`) on Unix and `windows-sys` on Windows. The crate root exports
`Pty` (and its `PtyConfig`), which implements `ai6::directorates::q::terminal::Terminal`; the library is both an
`rlib` and the Guatiao `cdylib` a Station loads. Provider ID: `pty`. Unsafe code is
limited to the platform calls that own the process tree (`../process_tree/`, shared with
`pipe`, and `windows.rs`), each with a safety comment.

`PtyConfig` (`ToValue`, `FromValue`, `Schema`, `Default`; `Pty::new(PtyConfig)`) holds the
provider's limits; every key is optional and each default is the value the provider used before
it was a key. A Station sets them in `[providers.pty]`.

| Key | Default | What |
| --- | --- | --- |
| `read_chunk_bytes` | 65536 | Bytes of one read of the output; 0 is refused |
| `drain_quiet_ms` | 200 | How long the output stays quiet, once the program has exited, before it ends |
| `kill_wait_ms` | 2000 | How long `kill` waits for the program and its tree to have exited |
| `tree_scans` | 32 (8 on Windows) | Times the process table is read again when a tree is killed (Unix) or adopted (Windows) |

`tty()` answers `true`. A Station prefers `pty` for a harness whose handler does not declare
JSON over standard input and output (`HarnessStdio::Json`), when several terminal providers
are available and none is pinned with `ai6 serve --terminal`; a JSON harness is never given
`pty`, pinned or not. Agents and handlers do not choose a provider.

## Session behaviour

- **Start**: `argv[0]` with the rest as separate arguments, on a new pseudo-terminal of
  `rows` × `cols`, each capped at 32767 (ConPTY takes them as `i16`). `TERM` is set to
  `xterm-256color` when the environment lacks it. An empty `argv` or a malformed spec is
  `GUATIAO_ERR_BAD_VALUE`. `resize` takes 1 to 32767.
- **Output** is read on a thread into a channel, so `read` honours its timeout; a chunk
  larger than the caller's buffer is kept for the next `read`.
- **Cursor position queries** (`ESC[6n`, also split across reads) are, on Windows only,
  removed from the output and answered with `ESC[1;1R` by a thread of the session's own, so
  the reader never waits on a write that holds the writer. ConPTY sends one before its
  program's first output and holds that output until it is answered, because `portable-pty`
  0.9 creates every pseudo-console with `PSEUDOCONSOLE_INHERIT_CURSOR` and offers no way not
  to. On Unix, and for other terminal queries, the bytes pass through as output.
- **Terminal mode on Unix**: before the program starts, the terminal is set to
  non-canonical mode with no echo (`ICANON`, `IEXTEN`, `ECHO`, `ECHONL`, `IXON` and `IMAXBEL`
  off, the last because FreeBSD answers input past a full queue with BEL in the output;
  `VMIN` 1, `VTIME` 0), so an input line of any length reaches the program whole and what
  is written never comes back as output. `ISIG` stays on (a written `^C` is SIGINT to the
  program), as do `ICRNL` (Enter typed in an attached viewer arrives as CR) and `OPOST`
  (the program's LF is output as CRLF). A program may set its own mode afterwards; nothing
  is restored. In canonical mode Linux cuts a line at 4095 bytes and macOS never delivers
  one over 1024 (`MAX_CANON`).
- **ConPTY output is a rendering**, not the program's bytes: mode and title sequences come
  first, blank lines may become cursor moves, line-mode input is echoed, and a line longer
  than `cols` is broken with CRLF and a cursor move. Read JSON-line harnesses with
  `ai6::directorates::q::terminal::JsonLines` and spawn them with `cols: STREAM_COLS`.
- **Exit** is asked of the child, never inferred from EOF: a pseudo-console can stay
  readable after its program exits. A signal is reported by number when `portable-pty`
  names it, else `-1`. A Windows exit code is reported as a signed 32-bit value
  (`0xC000013A` is `-1073741510`), as the `pipe` provider and the transports do.
- **End of output**: `read` returns `0` when the terminal closes, and also once the child
  has exited and no output has arrived for `drain_quiet_ms` (200 ms), on every platform. ConPTY keeps the
  output open until the pseudo-console is closed, and on Unix a process the child left
  running keeps the terminal open; either way the caller sees the end after the child's
  last output. A waiting `read` asks for the exit every 50 ms. After the first `0`, every
  `read` returns `0`.
- **Kill** takes the whole process tree. On Unix the child is a session leader; every process
  of its group and, on Linux and Android, its session, and every process whose parent is in the
  tree (found from `/proc` on Linux and Android, from `ps` elsewhere, until a scan finds
  nothing new), is stopped with `SIGSTOP`, then all are killed with `SIGKILL`, so a process
  that left the group (`setsid`, `setpgid`) is killed too. A process reparented away from the
  tree (its parent exited first) and outside the group and session is not found. On Windows
  the child is assigned to a Job Object with `KILL_ON_JOB_CLOSE` just after it starts
  (`portable-pty` starts it and takes no Job Object), and every process it started before then,
  found by parent and created after it, is assigned too; one whose parent already exited in
  that instant is not found. `kill` returns once the child has exited and, on Windows,
  the Job Object has no process left, or after `kill_wait_ms` (2 s). Windows may still refuse to remove the
  child's working directory for a few tens of milliseconds after that (os error 32): retry.
- **Input** is passed through on Unix. On Windows, ConPTY reads Enter as CR, so a bare LF
  or a CRLF pair is sent as one CR.
- **Drop** kills a running program and waits for it.

## Gotchas

- The Windows process ID must be available from `portable-pty` to build the Job Object;
  if it is not, the start fails and the child is killed.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
- On FreeBSD 14.5 the output of a program that exits within milliseconds of starting
  (`sh -c 'printf X'`) does not arrive, and `read` reports the end with nothing: three
  contract tests fail there. A program that lives longer (one reading its input first, as the
  harnesses do) is read in full.
