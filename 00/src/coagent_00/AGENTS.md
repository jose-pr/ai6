# `coagent_00` — public API header

Adapters that drive agent CLIs for coagent's supervisor. Self-contained: it ships inside
the package. Source: <https://github.com/jose-pr/coagent>.

No runtime dependencies. M loads adapters through the `coagent.adapters` entry-point
group and never imports this package; an adapter receives M's request by duck typing
(`profile`, `launcher`, `prompt`, `cwd`, `station`, `cover`, `clearance`, `options`).

## Contract

An adapter has `name` and `launch(request) -> (argv, env, typed)`; optionally
`debrief(transcript: bytes) -> dict | None`. An argv element `{context}` (or
`{context_file}`) is filled by M once every hook has contributed, and dropped with the
flag before it when there is no context.

## `coagent_00.claude.ClaudeAdapter` (entry point `claude`)

Options, from the launcher's `options` with the profile's `vars` over them: `mode`
(`print` default, or `interactive`), `executable` (`claude`), `permission_mode`
(overrides the clearance mapping), `allowed_tools`, `disallowed_tools` (lists),
`mcp_config` (list), `settings` (a table merged into `--settings`), `fallback_model`.

- `print`: `-p --output-format stream-json --verbose ... --permission-prompts none`.
- Clearance: `CLEARANCE_MODES = {"desk": "plan", "field": "acceptEdits", "00":
  "bypassPermissions"}`; below `00`, `FIELD_DENIED` (`git push`) is always disallowed.
- Cover: `covert` and any cover without free attribution → `--settings
  {"attribution": {"commit": "", "pr": "", "sessionUrl": false}}`; a cover with
  `suggest` → that text (with `{profile}`, `{model}` filled); `overt` → nothing.
- `effort` must be one of `EFFORTS` (`low`, `medium`, `high`, `xhigh`, `max`); `ValueError`
  otherwise, and for an unknown `mode`.
- The prompt comes right after `-p` (or the executable), ahead of every option, so a
  list-valued option such as `--disallowedTools` never takes it.
- `debrief` / `parse_result(transcript)`: the last `{"type": "result"}` record as
  `agent`, `subtype`, `is_error`, `session_id`, `num_turns`, `duration_ms`,
  `total_cost_usd`, `permission_denials` (a count), `result` (first 2000 characters);
  `None` without one.
