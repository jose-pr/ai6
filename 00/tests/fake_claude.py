"""A stand-in ``claude -p --output-format stream-json``: records its arguments and prints
records shaped like Claude Code's, ending with the ``result`` record."""

import json
import os
import sys

args = sys.argv[1:]
with open(os.environ["FAKE_CLAUDE_ARGS"], "w") as f:
    json.dump(args, f)
print(json.dumps({"type": "system", "subtype": "init", "session_id": "s-1"}), flush=True)
print(
    json.dumps({"type": "assistant", "message": {"content": [{"type": "text", "text": "on it"}]}})
)
print(
    json.dumps(
        {
            "type": "result",
            "subtype": "success",
            "is_error": False,
            "session_id": "s-1",
            "num_turns": 3,
            "duration_ms": 1200,
            "total_cost_usd": 0.0123,
            "permission_denials": [{"tool_name": "Bash"}],
            "result": "fixed: " + args[1],
        }
    ),
    flush=True,
)
