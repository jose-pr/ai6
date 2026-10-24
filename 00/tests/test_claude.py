from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from coagent_00.claude import ClaudeAdapter, parse_result

coagent_m = pytest.importorskip("coagent_m")
from coagent_m.cover import BUILTIN_COVERS  # noqa: E402
from coagent_m.profiles import Launcher, LaunchRequest, Profile  # noqa: E402
from coagent_m.watchdog import fill_context  # noqa: E402


def launch(clearance="field", cover=None, prompt="fix it", model="opus", effort="high", **options):
    profile = Profile("007", launcher="c", model=model, effort=effort, clearance=clearance)
    request = LaunchRequest(
        profile, Launcher("c", adapter="claude", options=options), prompt, None, cover=cover
    )
    return ClaudeAdapter().launch(request)[0]


def flag(argv, name):
    return argv[argv.index(name) + 1]


class TestLaunch:
    def test_print_mode(self) -> None:
        argv = launch()
        assert argv[:5] == ["claude", "-p", "--output-format", "stream-json", "--verbose"]
        assert (flag(argv, "--model"), flag(argv, "--effort")) == ("opus", "high")
        assert flag(argv, "--permission-prompts") == "none"
        assert argv[-3:] == ["--append-system-prompt", "{context}", "fix it"]

    def test_clearance_sets_the_permission_mode(self) -> None:
        assert flag(launch("desk"), "--permission-mode") == "plan"
        field = launch("field")
        assert flag(field, "--permission-mode") == "acceptEdits"
        assert "Bash(git push:*)" in field[field.index("--disallowedTools") + 1 :]
        double_o = launch("00")
        assert flag(double_o, "--permission-mode") == "bypassPermissions"
        assert "--disallowedTools" not in double_o

    def test_cover_sets_attribution(self) -> None:
        def settings(cover):
            argv = launch(cover=BUILTIN_COVERS[cover])
            return json.loads(flag(argv, "--settings")) if "--settings" in argv else None

        hidden = {"commit": "", "pr": "", "sessionUrl": False}
        assert settings("covert") == {"attribution": hidden}
        assert settings("partial")["attribution"]["commit"] == "Assisted-by: AI"
        assert settings("overt") is None

    def test_interactive_and_options(self) -> None:
        argv = launch(
            mode="interactive",
            allowed_tools=["Bash(pytest *)"],
            mcp_config=["q.json"],
            executable="/opt/claude",
            settings={"model": "x"},
        )
        assert argv[0] == "/opt/claude" and "-p" not in argv and "--permission-prompts" not in argv
        assert (
            flag(argv, "--allowedTools") == "Bash(pytest *)"
            and flag(argv, "--mcp-config") == "q.json"
        )
        assert json.loads(flag(argv, "--settings")) == {"model": "x"}

    def test_bad_options(self) -> None:
        with pytest.raises(ValueError, match="effort"):
            launch(effort="extreme")
        with pytest.raises(ValueError, match="mode"):
            launch(mode="batch")

    def test_context_is_filled_or_dropped(self) -> None:
        argv = launch(prompt="go")
        assert fill_context(argv, None, None)[-1:] == ["go"]
        assert "--append-system-prompt" not in fill_context(argv, None, None)
        assert flag(fill_context(argv, "be brief", "/c.md"), "--append-system-prompt") == "be brief"


@pytest.mark.skipif(shutil.which("claude") is None, reason="Claude Code is not installed")
def test_every_flag_exists_in_the_installed_claude() -> None:
    help_text = subprocess.run(["claude", "--help"], capture_output=True, text=True).stdout
    argv = launch(
        mode="print",
        allowed_tools=["x"],
        mcp_config=["y"],
        fallback_model="z",
        cover=BUILTIN_COVERS["covert"],
    )
    for f in (a for a in argv if a.startswith("--")):
        assert re.search(
            rf"(^|[\s,]){re.escape(f)}\b", help_text, re.M
        ), f"{f} is not a claude flag"


class TestDebrief:
    def test_reads_the_last_result_record(self) -> None:
        lines = [
            b'{"type":"system","subtype":"init"}',
            b"\x1b[0mnot json",
            b'{"type":"result","subtype":"success","is_error":false,"session_id":"s","num_turns":2,'
            b'"duration_ms":5,"total_cost_usd":0.5,"permission_denials":[{},{}],"result":"done"}',
        ]
        d = parse_result(b"\r\n".join(lines))
        assert (d["subtype"], d["num_turns"], d["permission_denials"], d["result"]) == (
            "success",
            2,
            2,
            "done",
        )

    def test_none_without_a_result(self) -> None:
        assert parse_result(b"interactive output only") is None


coagent_i = pytest.importorskip("coagent_i")


def test_a_mission_through_m_has_a_debrief(tmp_path: Path, monkeypatch) -> None:
    from coagent_m.missions import MissionRunner
    from coagent_m.stations import Stations

    home = tmp_path / "hq"
    home.mkdir()
    monkeypatch.setenv("COAGENT_HOME", str(home))
    monkeypatch.setenv("FAKE_CLAUDE_ARGS", str(tmp_path / "args.json"))
    fake = Path(__file__).with_name("fake_claude.py")
    (home / "stations.toml").write_text(f"[station.G]\npath = {json.dumps(str(tmp_path))}\n")
    (home / "profiles.toml").write_text(
        "[launcher.claude]\nadapter = 'claude'\n"
        "[profile.007]\nlauncher = 'claude'\nmodel = 'opus'\nroles = ['implement']\nterminal = 'pipes'\n"
    )
    # The fake script stands in front of the adapter's own flags.
    monkeypatch.setattr(
        ClaudeAdapter,
        "launch",
        lambda self, r, _orig=ClaudeAdapter.launch: (
            [sys.executable, str(fake), *_orig(self, r)[0][1:]],
            {},
            None,
        ),
    )
    runner = MissionRunner(coagent_i.Inbox(), stations=Stations.load(), use_hooks=False, sources=[])
    out = runner.run(coagent_i.Brief("Fix the build", id="1", station="G", role="implement"))
    args = json.loads((tmp_path / "args.json").read_text())
    assert out["state"] == "done"
    assert out["debrief"]["session_id"] == "s-1" and out["debrief"]["permission_denials"] == 1
    assert args[-1] == "Fix the build" and "--append-system-prompt" in args
    # The station's covert cover reaches Claude Code as hidden attribution.
    assert json.loads(args[args.index("--settings") + 1])["attribution"]["commit"] == ""
