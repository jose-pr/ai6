# coagent-terminal-pty

A [coagent](https://github.com/jose-pr/coagent) terminal provider on
[`portable-pty`](https://crates.io/crates/portable-pty): **one plugin for `openpty` on
Unix and ConPTY on Windows**, loaded by the supervisor at run time.

```bash
cargo build -p coagent-terminal-pty -p coagent-native
export GUATIAO_LIBRARY=$PWD/target/debug/libcoagent_native.so
export COAGENT_TERMINAL_PLUGINS=$PWD/target/debug
python -m coagent_m.watchdog providers     # coagent_terminal_pty_portable_pty first
```

The API header is [`AGENTS.md`](https://github.com/jose-pr/coagent/blob/main/m/crates/coagent-terminal-pty/AGENTS.md).

## License

MPL-2.0.
