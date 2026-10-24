from __future__ import annotations

import json
import subprocess
import sys
import textwrap
import threading
import time
from pathlib import Path

import pytest

coagent_i = pytest.importorskip("coagent_i")

from coagent_m.missions import MissionRunner, Scheduler  # noqa: E402
from coagent_m.stations import Stations  # noqa: E402
from coagent_m.watchdog import main  # noqa: E402

# A stand-in agent: says where it runs, optionally commits, optionally waits on a file.
AGENT = textwrap.dedent("""
    import os, subprocess, sys, time
    prompt = sys.argv[-1]
    print("AT", os.getcwd(), flush=True)
    if "commit" in prompt:
        open("work.txt", "w").write("done\\n")
        subprocess.run(["git", "add", "."], check=True)
        subprocess.run(["git", "-c", "user.name=a", "-c", "user.email=a@example.invalid",
                        "commit", "-qm", "agent work"], check=True)
    gate = os.environ.get("GATE")
    while gate and not os.path.exists(gate):
        time.sleep(0.02)
    """)


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True, text=True
    ).stdout


@pytest.fixture
def hq(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    home = tmp_path / "hq"
    home.mkdir()
    repo = tmp_path / "guatiao"
    repo.mkdir()
    git(repo, "init", "-q", "-b", "main")
    (repo / "README").write_text("x\n")
    git(repo, "add", ".")
    git(repo, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "init")
    plain = tmp_path / "plain"
    plain.mkdir()
    monkeypatch.setenv("COAGENT_HOME", str(home))
    monkeypatch.delenv("COAGENT_INBOX", raising=False)
    monkeypatch.delenv("COAGENT_STATE", raising=False)
    (home / "agent.py").write_text(AGENT)
    (home / "stations.toml").write_text(
        f"[station.G]\npath = {json.dumps(str(repo))}\nmax_concurrent = 1\n"
        f"[station.P]\npath = {json.dumps(str(plain))}\n"
    )
    (home / "profiles.toml").write_text(
        f"[launcher.py]\ncommand = [{json.dumps(sys.executable)}, {json.dumps(str(home / 'agent.py'))}]\n"
        "[profile.007]\nlauncher = 'py'\nroles = ['implement']\nterminal = 'pipes'\n"
        "[profile.solo]\nlauncher = 'py'\nroles = ['solo']\nterminal = 'pipes'\nmax_concurrent = 1\n"
    )
    return home, repo, plain


def runner(**kw) -> MissionRunner:
    return MissionRunner(
        coagent_i.Inbox(), stations=Stations.load(), use_hooks=False, sources=[], **kw
    )


def brief(id: str, station: str, title: str = "work", role: str = "implement", **kw):
    return coagent_i.Brief(title, id=id, source="t", station=station, role=role, **kw)


class TestRunner:
    def test_an_untouched_worktree_is_removed(self, hq) -> None:
        home, repo, _ = hq
        r = runner()
        r.inbox.submit(brief("1", "G"))
        out = r.run(r.inbox.claim(), claimed=True)
        wt = out["worktree"]
        assert out["state"] == "done" and not wt["kept"] and wt["branch"] == "1-work"
        assert "1-work" not in git(repo, "branch", "--list")
        transcript = r.inbox.transcript("t--1").decode()
        assert f"AT {wt['path']}" in transcript and str(repo) not in transcript

    def test_work_is_kept_on_its_branch(self, hq) -> None:
        home, repo, _ = hq
        r = runner()
        out = r.run(brief("2", "G", title="commit a fix"))
        wt = out["worktree"]
        assert wt["kept"] and wt["branch"] == "2-commit-a-fix"
        assert [c.split(" ", 1)[1] for c in wt["commits"]] == ["agent work"]
        assert "agent work" in git(repo, "log", "--format=%s", wt["branch"])
        assert git(repo, "log", "--format=%s", "main").strip() == "init"

    def test_a_station_that_is_not_git_runs_in_place(self, hq) -> None:
        _, _, plain = hq
        out = runner().run(brief("3", "P"))
        assert out["state"] == "done" and "worktree" not in out

    def test_unassignable_is_refused_and_filed(self, hq) -> None:
        r = runner()
        r.inbox.submit(brief("4", "G", role="deploy"))
        out = r.run(r.inbox.claim(), claimed=True)
        assert out["state"] == "refused" and "no profile suits" in out["reason"]
        assert r.inbox.find("t--4")[0] == "failed"


class TestScheduler:
    def test_station_and_profile_budgets(self, hq, tmp_path: Path, monkeypatch) -> None:
        gate = tmp_path / "gate"
        monkeypatch.setenv("GATE", str(gate))
        r = runner()
        for i in range(2):
            r.inbox.submit(brief(f"g{i}", "G"))  # station G takes one at a time
        for i in range(2):
            r.inbox.submit(brief(f"s{i}", "P", role="solo"))  # profile solo: one at a time
        r.inbox.submit(brief("p", "P"))
        s = Scheduler(r, max_total=10)
        assert s.step() == 3
        assert s.running == ["t--g0", "t--p", "t--s0"]
        assert s.step() == 0
        gate.write_text("")
        s.run(once=True, idle=0.05)
        assert sorted(o["state"] for o in s.finished) == ["done"] * 5
        assert r.inbox.list("new") == []

    def test_max_total(self, hq, tmp_path: Path, monkeypatch) -> None:
        gate = tmp_path / "gate"
        monkeypatch.setenv("GATE", str(gate))
        r = runner()
        for i in range(3):
            r.inbox.submit(brief(f"p{i}", "P"))
        s = Scheduler(r, max_total=2)
        assert s.step() == 2
        gate.write_text("")
        s.run(once=True, idle=0.05)
        assert len(s.finished) == 3

    def test_unassignable_briefs_do_not_block(self, hq) -> None:
        r = runner()
        r.inbox.submit(brief("bad", "G", role="deploy", priority=9))
        r.inbox.submit(brief("ok", "P"))
        s = Scheduler(r)
        s.run(once=True, idle=0.05)
        assert {o["state"] for o in s.finished} == {"refused", "done"}


def test_serve_once_cli(hq) -> None:
    coagent_i.Inbox().submit(brief("c1", "P"))
    assert main(["serve", "--once", "--idle", "0.05", "--no-hooks"]) == 0
    assert coagent_i.Inbox().find("t--c1")[0] == "done"
