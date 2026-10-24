from __future__ import annotations

import json
import os
import time
from pathlib import Path

import pytest

coagent_i = pytest.importorskip("coagent_i")

from coagent_m.missions import MissionRunner, Scheduler  # noqa: E402
from coagent_m.stations import Stations  # noqa: E402

from test_scheduler import brief, hq  # noqa: E402,F401

LEASE = 0.6
BACKENDS = ["file", "sqlite"] + (["postgres"] if os.environ.get("COAGENT_TEST_POSTGRES") else [])


@pytest.fixture(params=BACKENDS, autouse=True)
def backend(request, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> str:
    """Runs every test here against each store the offices could share."""
    if request.param == "sqlite":
        monkeypatch.setenv("COAGENT_INBOX", f"sqlite:///{tmp_path / 'inbox.db'}")
    elif request.param == "postgres":
        url = os.environ["COAGENT_TEST_POSTGRES"]
        from coagent_i import sql

        box = sql.SqlInbox(url)
        with box._engine.begin() as db:
            for table in reversed(sql._meta.sorted_tables):
                db.execute(table.delete())
        box.dispose()
        monkeypatch.setenv("COAGENT_INBOX", url)
    return request.param


def office(name: str, **kw) -> Scheduler:
    runner = MissionRunner(
        coagent_i.open_inbox(), stations=Stations.load(), use_hooks=False, sources=[]
    )
    return Scheduler(runner, node=name, lease=LEASE, **kw)


def step_until(offices, done, timeout: float = 15.0) -> None:
    deadline = time.monotonic() + timeout
    while not done():
        assert time.monotonic() < deadline, "timed out"
        for o in offices:
            o.step()
        time.sleep(0.05)


def states(inbox) -> dict:
    return {state: sorted(b.key for b in inbox.list(state)) for state in ("new", "claimed", "done")}


def test_briefs_spread_across_offices(hq, tmp_path: Path, monkeypatch) -> None:
    gate = tmp_path / "gate"
    monkeypatch.setenv("GATE", str(gate))
    a, b = office("a", max_total=2), office("b", max_total=2)
    for i in range(4):
        a.runner.inbox.submit(brief(f"p{i}", "P"))
    b.step()  # b announces itself, and is M: the first to hold the lease
    a.step()
    b.step()
    a.step()
    assert len(a.running) == 2 and len(b.running) == 2
    gate.write_text("")
    step_until([a, b], lambda: len(states(a.runner.inbox)["done"]) == 4)


def test_a_control_office_runs_nothing_itself(hq, tmp_path: Path) -> None:
    boss = office("boss", roles=["control"])
    worker = office("worker", roles=["execution"])
    boss.runner.inbox.submit(brief("1", "P"))
    worker.step()
    step_until([boss, worker], lambda: states(boss.runner.inbox)["done"] == ["t--1"])
    assert boss.leading and not worker.leading
    assert [o["agent"] for o in worker.finished] == ["1"] and boss.finished == []


def test_a_selector_keeps_a_station_on_its_offices(hq, tmp_path: Path) -> None:
    home, _, plain = hq
    stations = home / "stations.toml"
    stations.write_text(stations.read_text() + "selector = { gpu = 'y*' }\n")  # station P
    plain_office = office("plain")
    gpu_office = office("gpu", labels={"gpu": "yes"})
    plain_office.runner.inbox.submit(brief("1", "P"))
    step_until([plain_office, gpu_office], lambda: states(gpu_office.runner.inbox)["done"])
    assert [o["agent"] for o in gpu_office.finished] == ["1"] and plain_office.finished == []


def test_when_m_s_office_is_lost_another_takes_over(hq, tmp_path: Path, monkeypatch) -> None:
    gate = tmp_path / "gate"
    monkeypatch.setenv("GATE", str(gate))
    a, b = office("a", max_total=1), office("b", max_total=1)
    inbox = a.runner.inbox
    inbox.submit(brief("x", "P"))
    a.step()
    assert a.leading and a.running == ["t--x"]
    b.step()
    assert not b.leading

    # Office a dies: its agent goes with it and nothing more is heard from it.
    a.runner.abandon("t--x")
    time.sleep(LEASE + 0.1)
    gate.write_text("")
    step_until([b], lambda: states(inbox)["done"] == ["t--x"])
    assert b.leading and inbox.attempts("t--x") == 2
    assert [o["state"] for o in b.finished] == ["done"]
    a.wait()
    assert [o["state"] for o in a.finished] == ["abandoned"]


def test_roles_are_checked(hq) -> None:
    with pytest.raises(ValueError, match="roles"):
        office("x", roles=["janitor"])
