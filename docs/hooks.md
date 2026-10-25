# Hooks and initialization context

The current Rust daemon does not load hook plugins. AgentHandlers receive the initial
context, task prompt, Operation working directory and authorized environment directly
through the `AgentStart` contract. Q Branch separately controls declared tools and
Mission-scoped secret leases.

External integrations can prepare a Mission brief before submission. They should keep
credentials out of the brief, worktree, Cable body and audit metadata. A
future hook system must keep M's authorization in the daemon and pass only scoped
capabilities to a handler.
