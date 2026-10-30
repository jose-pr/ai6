# `ai6-transport-local` — API header

Header-file-style reference for the `ai6-transport-local` crate: the `local` transport
provider for AI6, so it can be used without reading its source. It ships inside the package
and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features) only. The crate
root exports `Local` and `LocalConfig`; `Local` implements `ai6::directorates::q::transport::Transport`;
the library is both an `rlib` and the Guatiao `cdylib` a Station loads. Provider ID: `local`.
No unsafe code.

`LocalConfig` (`ToValue`, `FromValue`, `Schema`, `Default`; `Local::new(LocalConfig)`), set by a
Station in `[providers.local]`: `reader_grace_ms` (default 1000), how long a call waits for the
output pipes once the program has ended.

## Capabilities

`run` and `files`, with `os` and `arch` of the Station's host. Not `terminal` (the Station
opens terminals on its own host with its Terminal provider and adds that capability to the
`local` Theater itself) and not `forward`. An operation outside these answers
`GUATIAO_ERR_UNSUPPORTED`.

## Behaviour

- **`run(RunSpec)`**: `argv[0]` is spawned with the rest as arguments, never through a
  shell; `env` is added to the Station's environment; `cwd` defaults to the Station's.
  `stdin` is written and closed; with none the program reads nothing. Each output stream
  keeps at most `max_output` bytes (the rest is drained and `truncated` is set). A positive
  `timeout_ms` kills the program (only the program itself, not its descendants) and reports
  `timed_out`, with no `code`. `signal` is set on Unix when a signal ended the program.
  An empty `argv` is `GUATIAO_ERR_BAD_VALUE`; a program that cannot be started is
  `GUATIAO_ERR_NOT_FOUND` when it does not exist, else `GUATIAO_ERR_INTERNAL`.
- **`read_file(ReadSpec)`**: `limit` bytes from `offset`, and the whole file's `size`.
- **`write_file(WriteSpec)`**: replaces the file, or appends; `create_dirs` makes missing
  parents.
- **`list_dir(path)`**: entries sorted by name; `kind` is `file`, `dir`, `link` or `other`.
  A symbolic link is reported as a link, not followed.
- Relative paths resolve against the Station's working directory.
- A missing file or directory is `GUATIAO_ERR_NOT_FOUND`.

## Gotchas

- A descendant of a timed-out program can keep running and holds the output pipes; the
  call waits at most `reader_grace_ms` (one second) for the pipes before returning what it has.
- After renaming or removing a provider library, clean the plugin directory: a Station
  loads every provider library it finds.
