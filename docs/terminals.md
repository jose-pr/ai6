# Terminals and plugins

An agent runs on a terminal, so it behaves as it would for a person, and a person can
take over without restarting it. The terminal comes from a **provider**.

## Providers

| Provider | Where | Notes |
| --- | --- | --- |
| `coagent_terminal_pty_portable_pty` | Linux, macOS; Windows to come | the Rust plugin on `portable-pty`, priority 30 |
| `posix-pty` | Linux, macOS, BSD | the standard library's `pty`, priority 20 |
| `conpty` | Windows | scaffold: always reports itself unavailable for now |
| `pipes` | any OS | no terminal at all; for headless agents, priority 0 |

```bash
$ python -m coagent_m.watchdog providers
  30  coagent_terminal_pty_portable_pty ok
  20  conpty               unavailable: ConPTY exists only on Windows
  20  posix-pty            ok
   0  pipes                ok
```

Every provider kills the agent together with every process it started (a process group
on POSIX; a Job Object on Windows, to come). A mission picks a provider with the
profile's `terminal`, or gets the best one available.

## Plugins in any language

A terminal plugin is a [guatiao](https://github.com/Boriken-Dev/guatiao) library serving
the `coagent_terminal` kind: a provider with one slot, `spawn`, answering a session
object (`pid`, `read`, `write`, `resize`, `kill`, `exit_status`). Write one in Rust
against `coagent-terminal-kind`, or in any language guatiao binds. Python packages can
also add providers through the `coagent.terminals` entry-point group.

```bash
cargo build -p coagent-native -p coagent-terminal-pty
export GUATIAO_LIBRARY=$PWD/target/debug/libcoagent_native.so   # .dylib on macOS
export COAGENT_TERMINAL_PLUGINS=$PWD/target/debug
pip install -e "m[guatiao]"
```

## The cursor handshake

A shell under a pty, and ConPTY at start-up, asks where the cursor is (`ESC[6n`) and
waits for the answer. With no emulator attached that looks exactly like a hung agent, so
the watchdog answers every query itself — always row 24, column 1, because the real size
may not have reached the terminal yet — and removes it from the stream so an attached
viewer does not answer a second time.
