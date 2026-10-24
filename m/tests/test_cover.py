from __future__ import annotations

import json
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from coagent_m.cover import BUILTIN_COVERS, CoverError, Covers, Identity, check_commits

ME = Identity("Field Office", "office@example.invalid")


def commit(message: str, author: str = "Field Office <office@example.invalid>") -> dict:
    return {"sha": "a" * 40, "author": author, "committer": author, "message": message}


def rules(cover: str, *commits: dict, model: str = "") -> list:
    return [v.rule for v in check_commits(commits, BUILTIN_COVERS[cover], ME, model=model)]


class TestRules:
    def test_covert_allows_a_plain_commit(self) -> None:
        msg = "fix: the build\n\nFixes: #12\nSigned-off-by: Field Office <office@example.invalid>"
        assert rules("covert", commit(msg)) == []

    def test_covert_refuses_every_attribution(self) -> None:
        msg = (
            "fix: it\n\nWritten with model-x, generated with a tool\n\n"
            "Co-Authored-By: Bot <bot@example.invalid>\nAgent-Session: https://claude.ai/code/session_1"
        )
        assert sorted(rules("covert", commit(msg), model="model-x")) == [
            "announce",
            "forbid",
            "model",
            "trailer",
            "trailer",
        ]

    def test_partial_allows_only_a_generic_trailer(self) -> None:
        assert rules("partial", commit("fix: it\n\nAssisted-by: AI")) == []
        assert rules("partial", commit("fix: it\n\nCo-Authored-By: Bot <b@x.invalid>")) == [
            "trailer"
        ]
        assert rules("partial", commit("fix: it\n\nAssisted-by: model-x"), model="model-x") == [
            "model"
        ]

    def test_overt_allows_full_disclosure(self) -> None:
        msg = "fix: it\n\n🤖 Generated with a tool\n\nCo-Authored-By: model-x <bot@example.invalid>"
        assert (
            rules("overt", commit(msg, author="Bot <bot@example.invalid>"), model="model-x") == []
        )

    def test_the_author_must_be_the_cover_identity(self) -> None:
        assert rules("covert", commit("fix: it", author="Bot <bot@example.invalid>")) == [
            "author",
            "committer",
        ]

    def test_a_trailer_needs_its_own_paragraph(self) -> None:
        assert rules("covert", commit("Co-Authored-By: is mentioned on the subject line")) == []


class TestCustom:
    def test_covers_toml(self, tmp_path: Path) -> None:
        path = tmp_path / "covers.toml"
        path.write_text(
            "[cover.house]\ntrailers = ['Assisted-by']\nfooter = 'Universal Exports'\n"
            "forbid = ['internal\\.example']\n"
        )
        covers = Covers.load(path)
        house = covers.get("house")
        assert "covert" in covers.names() and house.footer == "Universal Exports"
        bad = commit("fix: it\n\nSee internal.example/x")
        good = commit("fix: it\n\nAssisted-by: AI\n\nUniversal Exports")
        assert [v.rule for v in check_commits([bad], house, ME)] == ["forbid", "footer"]
        assert check_commits([good], house, ME) == []

    def test_bad_declarations(self, tmp_path: Path) -> None:
        path = tmp_path / "covers.toml"
        path.write_text("[cover.x]\nmodle = true\n")
        with pytest.raises(CoverError, match="unknown keys"):
            Covers.load(path)
        path.write_text("[cover.x]\nforbid = ['(']\n")
        with pytest.raises(CoverError, match="bad pattern"):
            Covers.load(path)
        with pytest.raises(CoverError, match="no cover"):
            Covers().get("nope")

    def test_briefing(self) -> None:
        text = BUILTIN_COVERS["partial"].briefing(ME, "007", "model-x")
        assert "Commit as Field Office <office@example.invalid>" in text
        assert "End each commit message with: Assisted-by: AI" in text
        assert "Do not name the model" in text


coagent_i = pytest.importorskip("coagent_i")
from coagent_m.missions import MissionRunner  # noqa: E402
from coagent_m.stations import Stations  # noqa: E402

AGENT = textwrap.dedent("""
    import os, subprocess, sys
    print("CTX", open(os.environ["COAGENT_CONTEXT_FILE"]).read().splitlines()[0], flush=True)
    open("w.txt", "w").write("x\\n")
    subprocess.run(["git", "add", "."], check=True)
    subprocess.run(["git", "commit", "-qm", sys.argv[-1]], check=True)
    """)


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True, text=True
    ).stdout


@pytest.fixture
def station(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    home, repo = tmp_path / "hq", tmp_path / "repo"
    home.mkdir()
    repo.mkdir()
    git(repo, "init", "-q", "-b", "main")
    (repo / "README").write_text("x\n")
    git(repo, "add", ".")
    git(repo, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "init")
    monkeypatch.setenv("COAGENT_HOME", str(home))
    monkeypatch.delenv("COAGENT_INBOX", raising=False)
    (home / "agent.py").write_text(AGENT)
    (home / "profiles.toml").write_text(
        f"[launcher.py]\ncommand = [{json.dumps(sys.executable)}, {json.dumps(str(home / 'agent.py'))}]\n"
        "[profile.007]\nlauncher = 'py'\nroles = ['implement']\nterminal = 'pipes'\nmodel = 'model-x'\n"
    )

    def configure(on_blown: str) -> MissionRunner:
        (home / "stations.toml").write_text(
            f"[station.G]\npath = {json.dumps(str(repo))}\ncover = 'covert'\non_blown = '{on_blown}'\n"
            "identity = { name = 'Field Office', email = 'office@example.invalid' }\n"
        )
        return MissionRunner(
            coagent_i.Inbox(), stations=Stations.load(), use_hooks=False, sources=[]
        )

    return configure, repo


def brief(id: str, title: str):
    return coagent_i.Brief(title, id=id, source="t", station="G", role="implement")


def test_a_clean_mission_keeps_cover(station) -> None:
    configure, repo = station
    out = configure("flag").run(brief("1", "fix: the build"))
    assert out["state"] == "done"
    assert out["cover"] == {"policy": "covert", "commits": 1, "blown": False, "violations": []}
    branch = out["worktree"]["branch"]
    assert git(repo, "log", "-1", "--format=%an <%ae>", branch).strip() == (
        "Field Office <office@example.invalid>"
    )


def test_a_blown_cover_is_reported_or_fails_the_mission(station) -> None:
    configure, _ = station
    title = "fix: it\n\nCo-Authored-By: model-x <bot@example.invalid>"
    flagged = configure("flag").run(brief("2", title))
    assert flagged["state"] == "done" and flagged["cover"]["blown"]
    assert {v["rule"] for v in flagged["cover"]["violations"]} == {"trailer", "model"}
    runner = configure("fail")
    runner.inbox.submit(brief("3", title))
    failed = runner.run(runner.inbox.claim(), claimed=True)
    assert (failed["state"], failed["reason"]) == ("blown", "cover blown")
    assert runner.inbox.find("t--3")[0] == "failed"
