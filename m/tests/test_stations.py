from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

from coagent_m.profiles import Launcher, Profile, ProfileError, ProfileSet
from coagent_m.stations import Station, StationError, Stations, clearance_rank, local_config
from coagent_m.watchdog import AgentSpec, State, Watchdog


@pytest.fixture
def home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("COAGENT_HOME", str(home))
    monkeypatch.delenv("COAGENT_TRUST_PROJECT", raising=False)
    return home


def write_stations(home: Path, text: str) -> Stations:
    (home / "stations.toml").write_text(text)
    return Stations.load()


def roster() -> ProfileSet:
    s = ProfileSet()
    show = (
        "import os; print('AT', os.getcwd()); print('STATION', os.environ.get('COAGENT_STATION'))"
    )
    s.add_launcher(Launcher("py", (sys.executable, "-c", show), prompt="none"))
    s.add(Profile("007", aliases=("JamesBond",), launcher="py", clearance="00", roles=("plan",)))
    s.add(Profile("q-branch", launcher="py", roles=("plan", "implement"), priority=-1))
    return s


class TestRegistry:
    def test_load_by_id_name_and_alias(self, home: Path, tmp_path: Path) -> None:
        stations = write_stations(
            home,
            f"[station.GT]\nname = 'guatiao'\naliases = ['Kingston']\npath = {json.dumps(str(tmp_path))}\n",
        )
        assert stations.get("GT") is stations.get("guatiao") is stations.get("Kingston")
        assert stations.get("GT").path == tmp_path.resolve()

    def test_a_cover_name_cannot_be_taken_twice(self) -> None:
        with pytest.raises(StationError, match="clashes"):
            Stations([Station("A", aliases=("Nassau",)), Station("B", name="Nassau")])

    def test_unknown_keys_and_clearances_are_errors(self, home: Path) -> None:
        with pytest.raises(StationError, match="unknown keys"):
            write_stations(home, "[station.A]\nroaster = []\n")
        with pytest.raises(StationError, match="unknown clearance"):
            write_stations(home, "[station.A]\nclearance = 'top'\n")

    def test_for_path_picks_the_deepest(self, tmp_path: Path) -> None:
        outer, inner = tmp_path / "ws", tmp_path / "ws" / "lib"
        inner.mkdir(parents=True)
        stations = Stations(
            [Station("W", path=outer.resolve()), Station("L", path=inner.resolve())]
        )
        assert stations.for_path(inner / "src").id == "L"
        assert stations.for_path(outer).id == "W"
        assert stations.for_path(tmp_path) is None

    def test_clearance_order(self) -> None:
        assert clearance_rank("desk") < clearance_rank("field") < clearance_rank("00")


class TestLocalConfig:
    def test_only_a_station_or_explicit_trust_is_read(self, tmp_path: Path, monkeypatch) -> None:
        root = tmp_path / "p"
        (root / ".coagent").mkdir(parents=True)
        assert local_config(str(root), Stations()) == (None, None)
        assert local_config(str(root), Stations(), trust=True)[0] == (root / ".coagent").resolve()
        station = Station("P", path=root.resolve())
        local, found = local_config(str(root), Stations([station]))
        assert (local, found) == ((root / ".coagent").resolve(), station)


class TestPosting:
    def test_roster_and_clearance(self) -> None:
        profiles = roster()
        desk = Station("D", clearance="desk")
        assert "clearance '00'" in desk.why_not(profiles.get("007"))
        posted = Station("H", roster=("JamesBond",), clearance="00")
        assert posted.why_not(profiles.get("007")) is None
        assert "not posted" in posted.why_not(profiles.get("q-branch"))

    def test_choose_skips_what_the_station_refuses(self) -> None:
        profiles = roster()
        assert profiles.choose("plan").name == "007"
        assert profiles.choose("plan", station=Station("F")).name == "q-branch"
        with pytest.raises(ProfileError, match="at Station D"):
            profiles.choose("plan", station=Station("D", roster=("nobody",)))

    def test_a_mission_runs_in_the_station(self, tmp_path: Path) -> None:
        station = Station("C", name="caribbean", path=tmp_path.resolve(), clearance="00")
        spec, limits = AgentSpec.from_profile(roster(), "JamesBond", station=station)
        spec = AgentSpec(**{**spec.__dict__, "terminal": "pipes"})
        output = []
        out = Watchdog(spec, limits, on_output=output.append).run()
        text = b"".join(output).decode()
        assert out.state is State.DONE
        assert f"AT {tmp_path.resolve()}" in text and "STATION C" in text

    def test_from_profile_refuses_an_uncleared_profile(self) -> None:
        with pytest.raises(ProfileError, match="above Station F"):
            AgentSpec.from_profile(roster(), "007", station=Station("F"))
