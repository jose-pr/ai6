# The Watchdog

The Rust AI6 daemon monitors provider events, terminal output and accepted operator
messages for each Agent Session. Provider events and terminal bytes count as activity;
an accepted follow-up message resets both inactivity and repeated-output timing.

After ten quiet seconds at a recognized terminal prompt, AI6 moves the Mission to
`blocked` so an operator can respond. If the Session then remains inactive for five
minutes, AI6 marks it `stalled`, interrupts and closes the provider Session, and ends
the active operation. New terminal output moves a blocked Mission back to `working`.

The Rust monitor also detects repeated terminal lines. It strips ANSI control sequences,
treats carriage returns and line feeds as line boundaries, and tracks a bounded least-
recently-used history of non-empty lines. Fifty repeated lines with no novel line for five
minutes stalls the Mission and closes its operation. The monitor bounds partial lines to
4 KiB and retains at most 4,096 line digests.

## Defaults

| Limit | Rust daemon default |
| --- | ---: |
| Quiet period before `stalled` | 300 s |
| Quiet period before `blocked` at a prompt | 10 s |
| Repeated lines before loop tracking applies | 50 |
| Novel-output period before loop `stalled` | 300 s |
| Maximum remembered line digests | 4,096 |
| Maximum Session runtime | Disabled |
| Digit normalization for loop matching | Disabled |

`WatchdogConfig` exposes these settings to Rust callers, and `ServeConfig::with_watchdog`
applies one configuration to Sessions started by that daemon. `ai6 serve` exposes the same
limits through `--stall-timeout`, `--loop-timeout`, `--loop-min-lines`, `--blocked-after`,
`--max-runtime`, `--history` and `--normalize-digits`; timeout values accept fractional,
non-negative seconds. Each Agent can override selected settings in its `[agent.<id>.limits]`
table; values layer over daemon settings and merge through Agent inheritance.

## Lifecycle

```text
queued ─► working ⇄ blocked ─► done | failed | abandoned
             │         │
             └────┬────┘
                  ▼
               stalled
```

`blocked` is a recoverable prompt wait. `stalled` closes the active operation while
preserving the Mission record and any changed safe-house work for recovery.

## Run the watchdog

The Rust watchdog runs as part of the daemon:

```console
cargo run -p ai6 --features cli -- serve --help
```

The watchdog runs inside `ai6 serve`; it is not a separate supervisor command.
