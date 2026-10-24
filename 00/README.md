# coagent-00

The **00 Section** of [C.O.A.G.E.N.T.](https://github.com/jose-pr/coagent): the agents
that execute the work, and **adapters that know how to drive each agent CLI** so M does
not have to.

M finds adapters through the `coagent.adapters` entry-point group; a launcher names one.
This package ships `claude`, for Claude Code.

```toml
# ~/.config/coagent/profiles.toml
[launcher.claude]
adapter = "claude"
options = { mode = "print" }          # or "interactive": the TUI, for a person to take over

[profile.007]
aliases = ["JamesBond"]
launcher = "claude"
model = "opus"
effort = "high"
roles = ["plan", "review"]
clearance = "00"
```

| The profile's | becomes |
| --- | --- |
| clearance `desk` / `field` / `00` | `--permission-mode plan` / `acceptEdits` (and `git push` denied) / `bypassPermissions` |
| station cover `covert` / `partial` / `overt` | Claude Code's `attribution` setting: hidden / the cover's suggested trailer / its default |
| `model`, `effort` | `--model`, `--effort` |
| the context M's hooks gathered | `--append-system-prompt` |

In `print` mode the run is `claude -p --output-format stream-json --verbose` with
`--permission-prompts none`, so nothing waits on a prompt nobody will answer, and the
final `result` record becomes the mission's debrief: session id, turns, cost and
permission denials.

`roster.example.toml` is a starting roster. The adapter's contract is in
[`src/coagent_00/AGENTS.md`](https://github.com/jose-pr/coagent/blob/main/00/src/coagent_00/AGENTS.md).

```bash
pip install -e 00/
```

## License

MPL-2.0.
