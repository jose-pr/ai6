# The watchdog

M's Medical Section, in its first form: it watches an agent's output and ends the run
when the agent is no longer working.

| Kill reason | When |
| --- | --- |
| `input_hang` | no output and no operator input for `stall_timeout`, at something shaped like a prompt |
| `silent` | the same, with no prompt |
| `loop` | output keeps arriving, but no line has been new for `loop_timeout` |
| `max_runtime` | the optional hard cap |

Lines are compared after stripping ANSI escapes, and `\r` counts as a line break, so a
spinner redrawing in place counts as repetition. Typing into an agent counts as activity,
so a person driving it is never killed for being slow.

## Lifecycle

```text
pending ─► working ⇄ blocked ─► done | failed
              │         │
              └────┬────┘
                   ▼
                stalled   (input_hang · silent · loop · max_runtime)
```

*Blocked* means the output ends in a prompt and has been quiet for `blocked_after`: a
warning on the board, not a kill, so an operator can attach and answer in time.

## Limits

| Limit | Default |
| --- | --- |
| `stall_timeout` | 300 s |
| `loop_timeout` | 300 s |
| `loop_min_lines` | 50 repeated lines |
| `blocked_after` | 10 s |
| `max_runtime` | none |
| `normalize_digits` | off: `retry 1` and `retry 2` are different lines |

A profile's `limits` table overrides them per agent.

## Commands

```bash
python -m coagent_m.watchdog run -- some-agent --flag     # supervise any command
python -m coagent_m.watchdog demo --timeout 3             # three mock agents
```
