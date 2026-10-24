from __future__ import annotations

import os
import re
import sys
import time
from pathlib import Path

import pytest

if sys.platform == "win32":
    pytest.skip("the ssh tests drive a POSIX shell", allow_module_level=True)

from coagent_m.hosts import Host, HostError, Hosts
from coagent_m.terminal import Registry, TerminalSpec, builtin_providers
from coagent_m.terminal._ssh import SshProvider, forwarded_env, remote_command
from coagent_m.watchdog import AgentSpec, KillReason, Limits, State, Watchdog

FAST = Limits(stall_timeout=1.5, loop_timeout=1.5, loop_min_lines=5, blocked_after=0.3)
FAKE = Host(
    "fake", "nobody@nowhere", client=(sys.executable, str(Path(__file__).with_name("fake_ssh.py")))
)


def real_host() -> "Host | None":
    dest = os.environ.get("COAGENT_TEST_SSH")
    if not dest:
        return None
    options = {"StrictHostKeyChecking": "no", "BatchMode": "yes"}
    for item in filter(None, os.environ.get("COAGENT_TEST_SSH_OPTIONS", "").split(",")):
        k, _, v = item.partition("=")
        options[k] = v
    return Host(
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


HOSTS = [pytest.param(FAKE, id="fake-ssh")]
if real_host() is not None:
    HOSTS.append(pytest.param(real_host(), id="real-ssh"))


def run(host: Host, behavior: str, **kw):
    registry = Registry([*builtin_providers(), SshProvider(host)])
    argv = [sys.executable, "-m", "coagent_m.watchdog", "mock-agent", behavior]
    spec = AgentSpec(behavior, argv, terminal=host.provider_name, **kw)
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
        path = tmp_path / "hosts.toml"
        path.write_text("[host.w]\nssh = 'u@w'\nos = 'windows'\nport = 2200\ntags = ['gpu']\n")
        (host,) = Hosts.load(path)
        assert (host.os, host.port, host.tags, host.provider_name) == (
            "windows",
            2200,
            ("gpu",),
            "ssh:w",
        )
        assert host.ssh_argv(tty=True)[:4] == ["ssh", "-tt", "-p", "2200"]
        path.write_text("[host.w]\nssh = 'u@w'\nos = 'plan9'\n")
        with pytest.raises(HostError, match="os must be"):
            Hosts.load(path)


@pytest.mark.parametrize("host", HOSTS)
class TestRemote:
    def test_output_cwd_and_exit(self, host: Host, tmp_path: Path) -> None:
        out, text = run(host, "work", cwd=str(tmp_path))
        assert (out.state, out.returncode) == (State.DONE, 0)
        assert "all tests pass" in text and "coagent-sid" not in text

    def test_the_cursor_handshake_crosses_ssh(self, host: Host) -> None:
        out, text = run(host, "cursor")
        assert out.state is State.DONE and "cursor at 24;1" in text

    def test_a_hung_remote_agent_is_killed(self, host: Host) -> None:
        out, _ = run(host, "hang")
        assert out.state is State.STALLED and out.reason is KillReason.INPUT_HANG

    def test_remote_descendants_die_with_it(self, host: Host) -> None:
        out, text = run(host, "orphan")
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
