# `ai6-terminal-pipe` — API header

Header-file-style reference for the `ai6-terminal-pipe` crate: the `pipe` terminal provider
for AI6, so it can be used without reading its source. It ships inside the package and is
self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features); `libc` on Unix
and `windows-sys` on Windows. The library is both an `rlib` and the Guatiao `cdylib` a
Station loads. Provider ID: `pipe`. Unsafe code is limited to the platform calls that own the
process tree (`../process_tree/`, shared with `pty`) and start the child, each with a safety
comment; the rest of the crate denies it.

## Exports

- `Pipe` — implements `ai6::directorates::q::terminal::Terminal`; `tty()` answers `false`,
  so a Station gives it to a harness whose handler declares `HarnessStdio::Json`, preferring
  it to other providers without a terminal. `Pipe::default()` merges
  standard error; `Pipe::new(PipeConfig)` takes a configuration. As a Guatiao provider it is
  built from a `PipeConfig` value, and from an empty map (every field defaulted) when the host
  passes none.
- `PipeConfig { stderr: StderrMode }` — `ToValue`, `FromValue`, `Schema`, `Default`.
  `stderr` defaults to `merge`.
- `StderrMode` — `Merge` (`"merge"`, the default): standard error goes into the output
  stream, through the same pipe as standard output, so the order the program wrote them in is
  kept. `Drop` (`"drop"`): discarded.

## Session behaviour

- **Start**: `argv[0]` with the rest as separate arguments, never through a shell. `env`, when
  given, is the whole environment; `cwd` the working directory. `rows` and `cols` are ignored.
  An empty `argv` or a malformed spec is `GUATIAO_ERR_BAD_VALUE`; a program that cannot be
  started is `GUATIAO_ERR_INTERNAL` naming it. On Windows a bare name is looked up on the
  child's `PATH` with each `PATHEXT` extension (`codex` finds `codex.cmd`; a file named
  exactly `codex` is not run), and a name with an extension is taken as given.
- **No terminal**: standard input and output are pipes, so `isatty` is false for both. Nothing
  is echoed, no line discipline applies (an input line of any length arrives whole), and the
  output is the program's bytes exactly: no CR inserted, no control sequences.
- **Output** is read on a thread into a channel, so `read` honours its timeout; a chunk
  larger than the caller's buffer is kept for the next `read`.
- **End of output**: `read` returns `0` once the program has exited and its output pipe has
  closed. A process the program left running that holds the pipe keeps it open; then the
  end is reported once the program has exited and nothing has arrived for 200 ms. A pipe that
  closes while the program still runs ends nothing until it exits. After the first `0`, every
  `read` returns `0`.
- **Input** is written as given; a write to a program that has exited fails.
- **Resize** does nothing and succeeds.
- **Exit** is asked of the child. On Unix a signal is reported by number; on Windows the exit
  code is reported as the standard library's signed 32-bit value (`0xFFFFFFFF` is `-1`,
  `0xC000013A` is `-1073741510`), and `kill` makes it `1`.
- **Kill** takes the whole process tree. On Unix the child leads a new process group; every
  process of the group, and every process whose parent is in the tree (found from `/proc` on
  Linux and Android, from `ps` elsewhere, until a scan finds nothing new), is stopped with
  `SIGSTOP`, then all are killed with `SIGKILL`, so a process that left the group (`setsid`,
  `setpgid`) is killed too. A process reparented away from the tree (its parent exited first)
  is not found, unless it is still in the child's group. On Windows the child is assigned to a
  Job Object with `KILL_ON_JOB_CLOSE` right after it starts, and is started
  with `CREATE_NO_WINDOW`, so a Station without a console opens no window. `kill` returns once
  the child has exited and, on Windows, the Job Object has no process left, or after 2 s. It is
  idempotent.
- **Drop** kills a running program and waits for it.

## Gotchas

- A process the program starts in the instant before it joins the Job Object is not in it.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
