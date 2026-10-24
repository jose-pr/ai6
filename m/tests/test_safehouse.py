from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from coagent_m.safehouse import SafeHouse, SafeHouseError, branch_name, is_git_checkout


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True, text=True
    ).stdout


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    git(repo, "init", "-q", "-b", "main")
    git(repo, "config", "user.name", "t")
    git(repo, "config", "user.email", "t@example.invalid")
    (repo / "a.txt").write_text("a\n")
    git(repo, "add", ".")
    git(repo, "commit", "-q", "-m", "first")
    return repo


def test_branch_names() -> None:
    assert branch_name("{id}-{slug}", id="GUA-12", title="Fix the Flaky test!") == (
        "GUA-12-fix-the-flaky-test"
    )
    assert branch_name("{station}/{id}", id="x..y", title="", station="G") == "G/x.y"
    assert branch_name("{slug}", id="1", title="***") == "mission"


def test_an_untouched_worktree_is_removed_with_its_branch(repo: Path, tmp_path: Path) -> None:
    wt = SafeHouse.create(repo, "m1", "m1-branch", root=tmp_path / "wts")
    assert (wt.path / "a.txt").read_text() == "a\n"
    assert wt.commits() == [] and not wt.dirty()
    assert wt.cleanup() is False
    assert not wt.path.exists()
    assert "m1-branch" not in git(repo, "branch", "--list")


def test_work_is_kept(repo: Path, tmp_path: Path) -> None:
    wt = SafeHouse.create(repo, "m2", "m2-branch", root=tmp_path / "wts")
    (wt.path / "b.txt").write_text("b\n")
    assert wt.dirty()
    git(wt.path, "add", ".")
    git(wt.path, "commit", "-q", "-m", "add b")
    assert [c.split(" ", 1)[1] for c in wt.commits()] == ["add b"]
    assert wt.cleanup() is True
    info = wt.describe()
    assert info["kept"] and info["branch"] == "m2-branch" and len(info["commits"]) == 1


def test_refusals(repo: Path, tmp_path: Path) -> None:
    SafeHouse.create(repo, "m3", "m3-branch", root=tmp_path / "wts")
    with pytest.raises(SafeHouseError, match="already exists"):
        SafeHouse.create(repo, "m3", "other", root=tmp_path / "wts")
    with pytest.raises(SafeHouseError, match="m3-branch"):
        SafeHouse.create(repo, "m4", "m3-branch", root=tmp_path / "wts")
    plain = tmp_path / "plain"
    plain.mkdir()
    assert not is_git_checkout(plain)
    with pytest.raises(SafeHouseError, match="not a git checkout"):
        SafeHouse.create(plain, "m5", "b", root=tmp_path / "wts")
