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
  it to other providers without a terminal. `Pipe::default()` keeps only standard error's
  tail; `Pipe::new(PipeConfig)` takes a configuration. As a Guatiao provider it is built from a
  `PipeConfig` value, and from an empty map (every field defaulted) when the host passes none.
- `PipeConfig` — `ToValue`, `FromValue`, `Schema`, `Default`; every field is optional and
  each default is the value the provider used before it was a key. A Station sets them in
  `[providers.pipe]`.

  | Key | Default | What |
  | --- | --- | --- |
  | `stderr` | `tail` | `tail`, `merge` or `drop` (below) |
  | `read_chunk_bytes` | 65536 | Bytes of one read of the output; 0 is refused |
  | `drain_quiet_ms` | 200 | How long the output stays quiet, once the program has exited, before it ends while a process left behind holds the pipe |
  | `kill_wait_ms` | 2000 | How long `kill` waits for the program and its tree to have exited |
  | `stderr_tail_bytes` | 8192 | Bytes of the end of standard error `tail` keeps |
  | `swept_fds` | 65536 | Unix: the highest descriptor marked close-on-exec where `close_range` is not there; Windows ignores it |
  | `tree_scans` | 32 | Unix: times the process table is read again when a tree is killed; Windows ignores it |
- `StderrMode` — `Tail` (`"tail"`, the default): standard error goes through a pipe of its
  own, and only its last `stderr_tail_bytes` (8192), from the first whole line in them, are
  added to the output once the program's own output has ended, as lines of their own (preceded
  by a line feed when the output ended inside a line). A JSON stream on standard output is
  never broken up by it, and the last thing the program said there (why it failed) is still
  read. `Merge` (`"merge"`): into the output stream, through the same pipe as standard output,
  in the order the program wrote them. `Drop` (`"drop"`): discarded.

## Session behaviour

- **Start**: `argv[0]` with the rest as separate arguments, never through a shell. `env`, when
  given, is the whole environment; `cwd` the working directory. `rows` and `cols` are ignored.
  An empty `argv` or a malformed spec is `GUATIAO_ERR_BAD_VALUE`; a program that cannot be
  started is `GUATIAO_ERR_INTERNAL` naming it. On Windows a bare name is looked up on the
  child's `PATH` with each `PATHEXT` extension (`codex` finds `codex.cmd`; a file named
  exactly `codex` is not run), a name with an extension is taken as given, and a bare name
  found on no `PATH` directory is refused (never looked for in the current directory). A
  `.bat` or `.cmd` file runs under the system's `cmd.exe` with its arguments quoted as the
  standard library quotes them (`%` expands nothing, `&` ends nothing); an argument holding CR
  or LF is refused for one.
- **Alone**: the child inherits only its standard streams. On Unix it leads a new session (so
  also a new process group, with no controlling terminal), and every descriptor above
  standard error is marked close-on-exec before the program runs (`close_range` on Linux
  5.11 or later, else each descriptor below the soft `RLIMIT_NOFILE`, at most `swept_fds`, 65536). On
  Windows it is started with `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` naming only its three
  standard handles, inside its Job Object from its first instruction
  (`PROC_THREAD_ATTRIBUTE_JOB_LIST`), and with `CREATE_NO_WINDOW`, so a Station without a
  console opens no window.
- **No terminal**: standard input and output are pipes, so `isatty` is false for both. Nothing
  is echoed, no line discipline applies (an input line of any length arrives whole), and the
  output is the program's bytes exactly: no CR inserted, no control sequences.
- **Output** is read on a thread into a channel, so `read` honours its timeout; a chunk
  larger than the caller's buffer is kept for the next `read`.
- **End of output**: `read` returns `0` once the program has exited and its output pipe has
  closed, after standard error's tail under `Tail` (whose pipe is waited for up to `drain_quiet_ms`, 200 ms). A process the program left running that holds the pipe keeps it open; then the
  end is reported once the program has exited and nothing has arrived for `drain_quiet_ms` (200 ms). A pipe that
  closes while the program still runs ends nothing until it exits. After the first `0`, every
  `read` returns `0`.
- **Input** is written as given; a write to a program that has exited fails.
- **Resize** does nothing and succeeds.
- **Exit** is asked of the child. On Unix a signal is reported by number; on Windows the exit
  code is reported as the standard library's signed 32-bit value (`0xFFFFFFFF` is `-1`,
  `0xC000013A` is `-1073741510`), and `kill` makes it `1`.
- **Kill** takes the whole process tree. On Unix every process of the child's process group
  and, on Linux and Android, its session, and every process whose parent is in the tree
  (found from `/proc` on Linux and Android, from `ps` elsewhere, until a scan finds nothing
  new), is stopped with `SIGSTOP`, then all are killed with `SIGKILL`, so a process that left
  the group (`setsid`, `setpgid`) is killed too. A process reparented away from the tree (its
  parent exited first) and outside the group and session is not found. On Windows the Job
  Object (`KILL_ON_JOB_CLOSE`) is terminated. `kill` returns once the child has exited and, on
  Windows, the Job Object has no process left, or after `kill_wait_ms` (2 s). It is idempotent.
- **Drop** kills a running program and waits for it.

## Gotchas

- On Windows the child's ends of its pipes are inheritable for the instant the child is being
  started: a process the Station starts meanwhile with every inheritable handle (as the
  standard library's `Command` does) also gets them, and keeps the pipe open while it runs.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
