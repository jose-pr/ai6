# Hooks and initialization context

The current Rust daemon does not load hook plugins. AgentHandlers receive the initial
context, task prompt, Operation working directory and authorized environment directly
through the `AgentStart` contract. Q Branch separately controls declared tools and
Mission-scoped secret leases.

## Hooks a harness brings with it

A harness also reads settings from the directory it works in: Claude Code its project hooks,
permissions and MCP servers (`.claude/settings.json`, `.claude/settings.local.json`,
`.mcp.json`), Codex its project `.codex/` layer. Those files come with the checkout, so a
Mission working on a branch someone else wrote would run their hooks with the Agent's
environment. By default AI6 starts every harness with them ignored, and passes only its own
settings: the green ink gate's `PreToolUse` hook and the Station's MCP servers. An Operation
can load project settings when they match its pins, or always, a reported weakening
(`harness.project_settings`, see [Operations](operations.md#harness-project-settings)).
`AgentStart::project_settings` and `AgentResume::project_settings` carry the decision to the
handler.

External integrations can prepare a Mission brief before submission. They should keep
credentials out of the brief, worktree, Cable body and audit metadata. A
future hook system must keep M's authorization in the daemon and pass only scoped
capabilities to a handler.
