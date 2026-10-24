# coagent-terminal-kind

The terminal contract of [coagent](https://github.com/jose-pr/coagent)'s supervisor as
[guatiao](https://github.com/Boriken-Dev/guatiao) kinds: **a provider starts programs on
terminals, in any language, and the supervisor finds it at run time.**

A provider implements `Terminal` (one slot, `spawn`) and hands back a
`TerminalSession` object (`pid`, `read`, `write`, `resize`, `kill`, `exit_status`).
`coagent-terminal-pty` is one, on `portable-pty`.

```rust
use coagent_terminal_kind::guatiao::library::{Object, ProviderError};
use coagent_terminal_kind::guatiao::{FromValue, Status, Value};
use coagent_terminal_kind::{Spec, Terminal, TerminalSession};

#[derive(Default, coagent_terminal_kind::guatiao::Provider)]
#[provider(Terminal, crate = ::coagent_terminal_kind::guatiao)]
pub struct Mine;

impl Terminal for Mine {
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
        let spec = Spec::from_value(spec)
            .map_err(|e| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &e.to_string()))?;
        todo!("start {:?}", spec.argv)
    }
}

coagent_terminal_kind::guatiao::providers!(Mine);
```

The API header is [`AGENTS.md`](https://github.com/jose-pr/coagent/blob/main/m/crates/coagent-terminal-kind/AGENTS.md).

## License

MPL-2.0.
