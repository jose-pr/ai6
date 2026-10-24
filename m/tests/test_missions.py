from __future__ import annotations

import json
import sys
import textwrap
from pathlib import Path

import pytest

coagent_i = pytest.importorskip("coagent_i")
import coagent_i.__main__  # noqa: E402

from coagent_m.watchdog import main  # noqa: E402

AGENT = (
    "import os, sys; print('PROMPT', sys.argv[-1].splitlines()[0]);"
    " print('STATION', os.environ['COAGENT_STATION'], os.path.basename(os.getcwd()))"
)


@pytest.fixture
def hq(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    home, checkout = tmp_path / "hq", tmp_path / "guatiao"
    home.mkdir()
    checkout.mkdir()
    monkeypatch.setenv("COAGENT_HOME", str(home))
    monkeypatch.delenv("COAGENT_REGISTRY", raising=False)
    (home / "stations.toml").write_text(
        f"[station.G]\nname = 'guatiao'\npath = {json.dumps(str(checkout))}\n"
        "clearance = 'field'\n"
    )
    (home / "profiles.toml").write_text(textwrap.dedent(f"""
            [launcher.py]
            command = [{json.dumps(sys.executable)}, "-c", {json.dumps(AGENT)}]

            [profile.007]
            launcher = "py"
            clearance = "00"
            roles = ["implement"]
            priority = 10

            [profile.q-branch]
            launcher = "py"
            roles = ["implement"]
            """))
    reported = tmp_path / "reported.json"
    with open(home / "report.py", "w", newline="") as f:
        f.write(f"import sys\nopen({str(reported)!r}, 'w').write(sys.stdin.read())\n")
    with open(home / "poll.py", "w", newline="") as f:
        brief = {
            "id": "GUA-1",
            "title": "Fix the build",
            "station": "guatiao",
            "role": "implement",
        }
        f.write(f"import json\nprint(json.dumps([{brief!r}]))\n")
    (home / "sources.toml").write_text(
        f"[[source]]\nname = 'tracker'\n"
        f"command = [{json.dumps(sys.executable)}, {json.dumps(str(home / 'poll.py'))}]\n"
        f"report = [{json.dumps(sys.executable)}, {json.dumps(str(home / 'report.py'))}]\n"
    )
    return home


def test_a_supplied_brief_runs_at_its_station_and_is_reported(hq: Path, capfd) -> None:
    assert coagent_i.__main__.main(["poll"]) == 0
    assert main(["launch", "--next", "--no-hooks"]) == 0
    out = capfd.readouterr().out
    assert "PROMPT Fix the build" in out
    # 007 is cleared above the station, so the best profile it takes is q-branch.
    assert "STATION G guatiao" in out
    registry = coagent_i.Registry()
    assert registry.find("tracker--GUA-1")[0] == "done"
    outcome = registry.outcome("tracker--GUA-1")
    assert (outcome["state"], outcome["profile"], outcome["station"]) == ("done", "q-branch", "G")
    report = json.loads((hq.parent / "reported.json").read_text())
    assert report["brief"]["id"] == "GUA-1" and report["outcome"]["state"] == "done"


def test_a_brief_no_profile_suits_is_failed_and_reported(hq: Path, tmp_path: Path) -> None:
    coagent_i.Registry().submit(
        coagent_i.Brief("Deploy", id="X", source="tracker", station="G", role="deploy")
    )
    assert main(["launch", "--next", "--no-hooks"]) == 2
    registry = coagent_i.Registry()
    assert registry.find("tracker--X")[0] == "failed"
    assert registry.outcome("tracker--X")["state"] == "refused"
    assert json.loads((hq.parent / "reported.json").read_text())["outcome"]["state"] == "refused"


def test_an_empty_registry(hq: Path) -> None:
    assert main(["launch", "--next"]) == 0
