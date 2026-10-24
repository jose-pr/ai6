# `coagent-terminal-kind` — API header

The `coagent_terminal` and `coagent_terminal_session` guatiao kinds, as Rust traits, and
the value types that cross them. Self-contained: it ships with the crate. Source:
<https://github.com/jose-pr/coagent>.

`pub use guatiao;` — name every guatiao path through it
(`#[provider(Terminal, crate = ::coagent_terminal_kind::guatiao)]`,
`coagent_terminal_kind::guatiao::providers!(..)`), so a provider and the supervisor
agree on one guatiao. guatiao is a git dependency pinned to a commit.

## Kinds

Slot order is the ABI: append with a default body, never reorder or change a required
signature. `TerminalVtable::FLOOR_HASH = 3285297011` and
`TerminalSessionVtable::FLOOR_HASH = 3148381037`, pinned by this crate's test and copied
into the Python mirror (`coagent_m.terminal._guatiao`); a change to either must change
both.

### `trait Terminal: Send + Sync` — kind `coagent_terminal`

- `spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError>`:
  `spec` is a `Spec` as a value; the caller owns the answered session.

### `trait TerminalSession: Send + Sync` — object kind `coagent_terminal_session`

Every slot takes `&self`: a caller writes, resizes or kills from one thread while
another blocks in `read`.

- `pid(&self) -> i64`
- `read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError>`: bytes
  copied, `0` once no more output can ever arrive, `READ_TIMED_OUT` (`-1`) otherwise.
- `write(&self, data: &[u8]) -> Result<(), ProviderError>`: all of it, or an error.
- `resize(&self, rows: i64, cols: i64) -> Result<(), ProviderError>`
- `kill(&self) -> Result<(), ProviderError>`: the program and everything it started;
  idempotent; a no-op once it has been reaped.
- `exit_status(&self, timeout_ms: i64) -> Result<Value, ProviderError>`: an `ExitInfo`
  as a value, after waiting up to `timeout_ms` (`0` polls).
- Dropping the object (its `destroy`) kills a program still running.

## Types

- `Spec { argv: Vec<String>, cwd: Option<String>, env: Option<BTreeMap<String, String>>, rows: u16 = 24, cols: u16 = 80 }`:
  `env` absent inherits, present replaces. `argv` elements are never re-split by a shell.
- `ExitInfo { running: bool, code: Option<i64>, signal: Option<i64> }`: at most one of
  `code`, `signal`; neither while `running`.
- `READ_TIMED_OUT: i64 = -1`.
