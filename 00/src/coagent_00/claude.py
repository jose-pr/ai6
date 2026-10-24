"""The ``claude`` adapter: Claude Code, driven headless or interactively.

The flags are Claude Code's own (``claude --help``): ``-p`` with
``--output-format stream-json --verbose`` for an unattended run whose final ``result``
record becomes the debrief, ``--permission-mode`` from the profile's clearance,
``--settings`` carrying the station's cover as Claude Code's ``attribution`` setting,
and ``--append-system-prompt`` for the context M's hooks gathered.

Launcher options (``[launcher.<name>.options]``, or a profile's ``vars``):

``mode``             ``print`` (default) or ``interactive``
``executable``       the ``claude`` command (default ``claude``)
``permission_mode``  overrides the clearance mapping
``allowed_tools``    list, passed as ``--allowedTools``
``disallowed_tools`` list, added to the clearance's own denials
``mcp_config``       list of files or JSON strings, passed as ``--mcp-config``
``settings``         a table merged into the ``--settings`` JSON
``fallback_model``   passed as ``--fallback-model``
"""

from __future__ import annotations

import json
import re
from typing import Any, Dict, List, Mapping, Optional, Tuple

__all__ = ["CLEARANCE_MODES", "EFFORTS", "ClaudeAdapter", "parse_result"]

EFFORTS = ("low", "medium", "high", "xhigh", "max")
# What each clearance may do without asking: desk plans only, field edits its worktree,
# 00 acts unattended.
CLEARANCE_MODES = {"desk": "plan", "field": "acceptEdits", "00": "bypassPermissions"}
# Irreversible steps a field agent is never allowed; M's authorization comes first.
FIELD_DENIED = ("Bash(git push:*)", "Bash(git push)")
_ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")


class ClaudeAdapter:
    """Claude Code. ``launch`` builds the command line; ``debrief`` reads the transcript."""

    name = "claude"

    def launch(self, request: Any) -> Tuple[List[str], Dict[str, str], Optional[str]]:
        opts: Mapping[str, Any] = request.options
        mode = str(opts.get("mode", "print"))
        if mode not in ("print", "interactive"):
            raise ValueError(f"claude adapter: mode is print or interactive, not {mode!r}")
        profile = request.profile
        argv: List[str] = [str(opts.get("executable", "claude"))]
        if mode == "print":
            argv.append("-p")
        # First, ahead of every option: the list-valued ones (--allowedTools and the
        # like) take every argument up to the next option, the prompt included.
        if request.prompt:
            argv.append(request.prompt)
        if mode == "print":
            argv += ["--output-format", "stream-json", "--verbose"]
        if profile.model:
            argv += ["--model", profile.model]
        if profile.effort:
            if profile.effort not in EFFORTS:
                raise ValueError(
                    f"claude adapter: effort is one of {EFFORTS}, not {profile.effort!r}"
                )
            argv += ["--effort", profile.effort]
        if opts.get("fallback_model"):
            argv += ["--fallback-model", str(opts["fallback_model"])]
        clearance = request.clearance
        argv += [
            "--permission-mode",
            str(opts.get("permission_mode") or CLEARANCE_MODES[clearance]),
        ]
        if mode == "print":
            # Unattended: a prompt nobody will answer is a refusal, not a wait.
            argv += ["--permission-prompts", "none"]
        allowed = [str(t) for t in opts.get("allowed_tools", ())]
        if allowed:
            argv += ["--allowedTools", *allowed]
        denied = [
            *(FIELD_DENIED if clearance != "00" else ()),
            *map(str, opts.get("disallowed_tools", ())),
        ]
        if denied:
            argv += ["--disallowedTools", *denied]
        mcp = [str(m) for m in opts.get("mcp_config", ())]
        if mcp:
            argv += ["--mcp-config", *mcp]
        settings = {**_cover_settings(request.cover, profile), **dict(opts.get("settings", {}))}
        if settings:
            argv += ["--settings", json.dumps(settings, sort_keys=True)]
        # M fills {context} once every hook has added to it, and drops the flag if none did.
        argv += ["--append-system-prompt", "{context}"]
        return argv, {}, None

    def debrief(self, transcript: bytes) -> Optional[Dict[str, Any]]:
        """The run's final ``result`` record, summarised; ``None`` without one (an
        interactive session, or one killed before it finished)."""
        return parse_result(transcript)


def parse_result(transcript: bytes) -> Optional[Dict[str, Any]]:
    record = None
    for line in _ANSI.sub("", transcript.decode("utf-8", errors="replace")).splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            data = json.loads(line)
        except ValueError:
            continue
        if isinstance(data, dict) and data.get("type") == "result":
            record = data
    if record is None:
        return None
    result = record.get("result")
    return {
        "agent": "claude",
        "subtype": record.get("subtype"),
        "is_error": bool(record.get("is_error")),
        "session_id": record.get("session_id"),
        "num_turns": record.get("num_turns"),
        "duration_ms": record.get("duration_ms"),
        "total_cost_usd": record.get("total_cost_usd"),
        "permission_denials": len(record.get("permission_denials") or []),
        "result": result[:2000] if isinstance(result, str) else None,
    }


def _cover_settings(cover: Any, profile: Any) -> Dict[str, Any]:
    """Claude Code's ``attribution`` setting for the station's cover: hidden unless the
    cover allows attribution, then the cover's suggested text; overt keeps Claude Code's
    own. Session links are never added under a cover."""
    if cover is None:
        return {}
    trailers = tuple(getattr(cover, "trailers", ()))
    if "*" in trailers and getattr(cover, "announce", False):
        return {}
    suggest = str(getattr(cover, "suggest", "") or "")
    text = suggest.replace("{profile}", profile.name).replace("{model}", profile.model or "")
    return {"attribution": {"commit": text, "pr": text, "sessionUrl": False}}
