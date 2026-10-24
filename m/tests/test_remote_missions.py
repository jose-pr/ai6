from __future__ import annotations

import json
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

if sys.platform == "win32":
    pytest.skip("remote missions drive a POSIX shell", allow_module_level=True)
coagent_i = pytest.importorskip("coagent_i")

from coagent_m.hosts import Hosts  # noqa: E402
from coagent_m.missions import MissionRunner  # noqa: E402
from coagent_m.stations import Stations  # noqa: E402

from test_ssh import HOSTS  # noqa: E402

AGENT = textwrap.dedent("""
    import os, subprocess
    print("AT", os.getcwd(), "HOST", os.environ.get("COAGENT_HOST"), flush=True)
    open("work.txt", "w").write("x\\n")
    subprocess.run(["git", "add", "."], check=True)
    subprocess.run(["git", "-c", "user.name=a", "-c", "user.email=a@example.invalid",
                    "commit", "-qm", "remote work"], check=True)
    """)


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True, text=True
    ).stdout


def toml_host(host) -> str:
    lines = [f"[host.{host.id}]", f"ssh = {json.dumps(host.ssh)}"]
    if host.port:
        lines.append(f"port = {host.port}")
    if host.identity:
        lines.append(f"identity = {json.dumps(host.identity)}")
    if host.options:
        lines.append(
            "options = { "
            + ", ".join(f"{k} = {json.dumps(v)}" for k, v in host.options.items())
            + " }"
        )
    lines.append("client = [" + ", ".join(json.dumps(c) for c in host.client) + "]")
    return "\n".join(lines)


@pytest.mark.parametrize("host", HOSTS)
def test_a_mission_at_a_remote_station(host, tmp_path: Path, monkeypatch) -> None:
    home, checkout, work, cfg = (tmp_path / n for n in ("hq", "checkout", "work", "cfg"))
    for d in (home, checkout, cfg):
        d.mkdir()
    git(checkout, "init", "-q", "-b", "main")
    (checkout / "README").write_text("x\n")
    git(checkout, "add", ".")
    git(
        checkout, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "init"
    )
    monkeypatch.setenv("COAGENT_HOME", str(home))
    monkeypatch.delenv("COAGENT_INBOX", raising=False)
    (home / "agent.py").write_text(AGENT)
    (home / "hosts.toml").write_text(toml_host(host) + f"\nwork_root = {json.dumps(str(work))}\n")
    (home / "stations.toml").write_text(
        f"[station.R]\nhost = {json.dumps(host.id)}\npath = {json.dumps(str(checkout))}\n"
        f"config = {json.dumps(str(cfg))}\n"
    )
    # The station's profiles come from its local config directory, not the remote checkout.
    (cfg / "profiles.toml").write_text(
        f"[launcher.py]\ncommand = [{json.dumps(sys.executable)}, {json.dumps(str(home / 'agent.py'))}]\n"
        "[profile.field]\nlauncher = 'py'\nroles = ['implement']\n"
    )
    runner = MissionRunner(
        coagent_i.Inbox(), stations=Stations.load(), hosts=Hosts.load(), use_hooks=False, sources=[]
    )
    out = []
    runner.on_output = lambda key, data: out.append(data)
    outcome = runner.run(coagent_i.Brief("Do it remotely", id="r1", station="R", role="implement"))
    text = b"".join(out).decode(errors="replace")
    wt = outcome["worktree"]
    assert outcome["state"] == "done", outcome
    assert wt["path"] == str(work / "manual--r1") and wt["kept"]
    assert f"AT {wt['path']} HOST {host.id}" in text
    assert "remote work" in git(checkout, "log", "--format=%s", wt["branch"])
