# `coagent-terminal-pty` — API header

A guatiao plugin (`cdylib` only) serving one `coagent_terminal` provider, key
`coagent_terminal_pty_portable_pty`. Self-contained: it ships with the crate. Source:
<https://github.com/jose-pr/coagent>.

- **Start**: `argv[0]` with the rest as separate arguments, on a new pseudo-terminal
  of `rows` × `cols`. `TERM` is set to `xterm-256color` when the environment lacks it.
  An empty `argv` or a malformed spec is `GUATIAO_ERR_BAD_VALUE`.
- **Output** is read on a thread into a channel, so `read` honours its timeout; a
  chunk larger than the caller's buffer is kept for the next `read`.
- **Exit** is asked of the child, never inferred from EOF: a pseudo-console can stay
  readable after its program exits. A signal is reported by number when
  `portable-pty` names it, else `-1`.
- **Kill**: on Unix the child is a session leader, so its process group is killed
  (`killpg`, SIGKILL). On Windows only the direct child is killed: descendants survive
  until the tree is held in a Job Object.
- **Drop** kills a running program and waits for it.
- Never link this crate as an rlib: `providers!` exports the library entry symbol.
