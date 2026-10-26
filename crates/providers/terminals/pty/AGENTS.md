# `ai6-terminal-pty` — API header

Header-file-style reference for the `ai6-terminal-pty` crate: the `pty` terminal provider
for AI6, so it can be used without reading its source. It ships inside the package and is
self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features) and
`portable-pty`; `libc` on Unix and `windows-sys` on Windows. The crate root exports
`Pty`, which implements `ai6::directorates::q::terminal::Terminal`; the library is both an
`rlib` and the Guatiao `cdylib` a Station loads. Provider ID: `pty`. Unsafe code is
limited to the platform calls in `unix.rs` (`killpg`) and `windows.rs` (Job Objects),
each with a safety comment.

A Station prefers `pty` when several terminal providers are available and none is
pinned with `ai6 serve --terminal`; Agents and handlers do not choose one.

## Session behaviour

- **Start**: `argv[0]` with the rest as separate arguments, on a new pseudo-terminal of
  `rows` × `cols`, each capped at 32767 (ConPTY takes them as `i16`). `TERM` is set to
  `xterm-256color` when the environment lacks it. An empty `argv` or a malformed spec is
  `GUATIAO_ERR_BAD_VALUE`. `resize` takes 1 to 32767.
- **Output** is read on a thread into a channel, so `read` honours its timeout; a chunk
  larger than the caller's buffer is kept for the next `read`.
- **Cursor position queries** (`ESC[6n`, also split across reads) are answered by the
  provider with `ESC[1;1R` and removed from the output. ConPTY sends one before its program's
  first output and holds that output until it is answered, because `portable-pty` 0.9 creates
  every pseudo-console with `PSEUDOCONSOLE_INHERIT_CURSOR` and offers no way not to. Other
  terminal queries pass through as output bytes.
- **ConPTY output is a rendering**, not the program's bytes: mode and title sequences come
  first, blank lines may become cursor moves, line-mode input is echoed, and a line longer
  than `cols` is broken with CRLF and a cursor move. Read JSON-line harnesses with
  `ai6::directorates::q::terminal::JsonLines` and spawn them with `cols: STREAM_COLS`.
- **Exit** is asked of the child, never inferred from EOF: a pseudo-console can stay
  readable after its program exits. A signal is reported by number when `portable-pty`
  names it, else `-1`.
- **Kill** takes the whole process tree. On Unix the child is a session leader, so its
  process group is killed (`killpg`, SIGKILL). On Windows the child is assigned to a Job
  Object with `KILL_ON_JOB_CLOSE`.
- **Input** is passed through on Unix. On Windows, ConPTY reads Enter as CR, so a bare LF
  or a CRLF pair is sent as one CR.
- **Drop** kills a running program and waits for it.

## Gotchas

- The Windows process ID must be available from `portable-pty` to build the Job Object;
  if it is not, the start fails and the child is killed.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
