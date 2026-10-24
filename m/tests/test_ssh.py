from __future__ import annotations

import os
import re
import sys
import time
from pathlib import Path

import pytest

if sys.platform == "win32":
    pytest.skip("the ssh tests drive a POSIX shell", allow_module_level=True)

from coagent_m.theaters import Theater, TheaterError, Theaters
from coagent_m.terminal import Registry, TerminalSpec, builtin_providers
from coagent_m.terminal._ssh import SshProvider, forwarded_env, remote_command
from coagent_m.watchdog import AgentSpec, KillReason, Limits, State, Watchdog

FAST = Limits(stall_timeout=1.5, loop_timeout=1.5, loop_min_lines=5, blocked_after=0.3)
FAKE = Theater(
    "fake", "nobody@nowhere", client=(sys.executable, str(Path(__file__).with_name("fake_ssh.py")))
)


def real_theater() -> "Theater | None":
    dest = os.environ.get("COAGENT_TEST_SSH")
    if not dest:
        return None
    options = {"StrictHostKeyChecking": "no", "BatchMode": "yes"}
    for item in filter(None, os.environ.get("COAGENT_TEST_SSH_OPTIONS", "").split(",")):
        k, _, v = item.partition("=")
        options[k] = v
    return Theater(
        "real",
        dest,
        port=(
            int(os.environ["COAGENT_TEST_SSH_PORT"])
            if os.environ.get("COAGENT_TEST_SSH_PORT")
            else None
        ),
        identity=os.environ.get("COAGENT_TEST_SSH_IDENTITY"),
        options=options,
    )


THEATERS = [pytest.param(FAKE, id="fake-ssh")]
if real_theater() is not None:
    THEATERS.append(pytest.param(real_theater(), id="real-ssh"))


def run(theater: Theater, behavior: str, **kw):
    registry = Registry([*builtin_providers(), SshProvider(theater)])
    argv = [sys.executable, "-m", "coagent_m.watchdog", "mock-agent", behavior]
    spec = AgentSpec(behavior, argv, terminal=theater.provider_name, **kw)
    output = []
    out = Watchdog(spec, FAST, on_output=output.append, registry=registry).run()
    return out, b"".join(output).decode(errors="replace")


class TestCommand:
    def test_posix_quoting_and_env(self) -> None:
        cmd = remote_command(
            TerminalSpec(["agent", "fix it; rm -rf /"], cwd="/srv/my repo"), {"X": "a'b"}, True
        )
        assert cmd.startswith("cd '/srv/my repo' && printf ")
        assert cmd.endswith("exec env X='a'\"'\"'b' agent 'fix it; rm -rf /'")

    def test_only_what_m_added_is_forwarded(self, monkeypatch) -> None:
        monkeypatch.setenv("HOME_ONLY", "local")
        env = {**os.environ, "COAGENT_STATION": "G"}
        fwd = forwarded_env(TerminalSpec(["x"], env=env))
        assert fwd["COAGENT_STATION"] == "G" and "HOME_ONLY" not in fwd and "PATH" not in fwd

    def test_hosts_toml(self, tmp_path: Path) -> None:
        path = tmp_path / "theaters.toml"
        path.write_text("[theater.w]\nssh = 'u@w'\nos = 'windows'\nport = 2200\ntags = ['gpu']\n")
        (theater,) = Theaters.load(path)
        assert (theater.os, theater.port, theater.tags, theater.provider_name) == (
            "windows",
            2200,
            ("gpu",),
            "ssh:w",
        )
        assert theater.ssh_argv(tty=True)[:4] == ["ssh", "-tt", "-p", "2200"]
        path.write_text("[theater.w]\nssh = 'u@w'\nos = 'plan9'\n")
        with pytest.raises(TheaterError, match="os must be"):
            Theaters.load(path)


@pytest.mark.parametrize("theater", THEATERS)
class TestRemote:
    def test_output_cwd_and_exit(self, theater: Theater, tmp_path: Path) -> None:
        out, text = run(theater, "work", cwd=str(tmp_path))
        assert (out.state, out.returncode) == (State.DONE, 0)
        assert "all tests pass" in text and "coagent-sid" not in text

    def test_the_cursor_handshake_crosses_ssh(self, theater: Theater) -> None:
        out, text = run(theater, "cursor")
        assert out.state is State.DONE and "cursor at 24;1" in text

    def test_a_hung_remote_agent_is_killed(self, theater: Theater) -> None:
        out, _ = run(theater, "hang")
        assert out.state is State.STALLED and out.reason is KillReason.INPUT_HANG

    def test_remote_descendants_die_with_it(self, theater: Theater) -> None:
        out, text = run(theater, "orphan")
        assert out.reason is KillReason.SILENT
        pid = int(re.search(r"child (\d+)", text).group(1))
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            if _zombie(pid):
                break
            time.sleep(0.05)
        else:
            pytest.fail(f"remote descendant {pid} survived")


def _zombie(pid: int) -> bool:
    try:
        with open(f"/proc/{pid}/stat") as f:
            return f.read().split(") ", 1)[1].startswith("Z")
    except (OSError, IndexError):
        return False
