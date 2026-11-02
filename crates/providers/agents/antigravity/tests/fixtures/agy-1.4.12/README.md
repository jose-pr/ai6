# Recorded agy 1.4.12 stream lines

agy is closed source and updates itself, so these are what the handler's tests hold it to. A new
version that changes one of these shapes should fail those tests rather than a Mission. Each file
is a real `agy --input-format stream-json --output-format stream-json` run on x86-64 Linux, with
conversation IDs fixed and paths made neutral; the fake CLI in `tests/handler.rs` replays them.

| File | Run |
| --- | --- |
| `init.jsonl` | The line agy writes at start, before reading a message; a real login |
| `turn_answer.jsonl` | One turn, one answer; real model `gemini-3.8-flash-low` |
| `turn_soft_denied.jsonl`, `.stderr` | A real model's `run_command` under no rule: refused, the turn ends, exit code 0 |
| `turn_deny_rule.jsonl` | A deny rule refuses, and the turn goes on until a soft denial ends it |
| `turn_hook_denied.jsonl` | A `PreToolUse` hook answers `deny` |
| `turn_hook_failed.jsonl` | A `PreToolUse` hook exits 1: the call does not run |
| `turn_mcp_call.jsonl` | Two tools of a server, through `call_mcp_tool`, allowed by `mcp(server/*)` |
| `turn_cached_usage.jsonl` | A response with cached and thinking tokens |
| `turn_quota_429.jsonl`, `.stderr` | A model answering 429 `RESOURCE_EXHAUSTED` with `QUOTA_EXHAUSTED` details: exit code 3, no retry |
| `turn_plain_429.jsonl` | The same with no details, one retry |
| `turn_error.jsonl` | A turn that cannot start: an agent naming a tool agy does not have |
| `resume_unknown_init.jsonl`, `.stderr` | `--conversation` with an ID agy does not hold: a new conversation, and a warning on standard error |

Files not marked as real above ran against a scripted loopback Gemini API
(`modelProvider: "gemini"`, `GOOGLE_GEMINI_BASE_URL`, a fake key, a private `HOME`), so the error
bodies of the two 429 files are that stand-in's. What a signed-in account's quota says has not been
seen in a run: agy's binary holds `You have exhausted your quota on this model.` and
`individual quota reached`.
