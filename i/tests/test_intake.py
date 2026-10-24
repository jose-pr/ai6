from __future__ import annotations

import json
import sys
import textwrap
from pathlib import Path

import pytest

from coagent_i import (
    Brief,
    BriefError,
    CommandSource,
    Inbox,
    SourceError,
    load_sources,
    poll,
    report,
)
from coagent_i.__main__ import main
from coagent_i.sources import parse_briefs

from backends import BACKENDS, make_inbox


def script(path: Path, body: str) -> list:
    with open(path, "w", newline="") as f:
        f.write(textwrap.dedent(body))
    return [sys.executable, str(path)]


class TestBrief:
    def test_round_trip(self) -> None:
        b = Brief(
            "Fix the flaky test",
            "It fails one run in ten.",
            id="GUA-12",
            source="jira",
            station="G",
            role="implement",
            tags=("ci",),
            url="https://t/GUA-12",
            data={"jira": {"key": "GUA-12"}},
        )
        assert Brief.from_json(b.to_json()) == b
        assert b.key == "jira--GUA-12"
        assert (
            b.prompt == "Fix the flaky test\n\nIt fails one run in ten.\n\nSource: https://t/GUA-12"
        )

    def test_strict(self) -> None:
        with pytest.raises(BriefError, match="needs a title"):
            Brief.from_dict({"body": "x"})
        with pytest.raises(BriefError, match="unknown brief keys"):
            Brief.from_dict({"title": "x", "titel": "y"})
        with pytest.raises(BriefError, match="newer"):
            Brief.from_dict({"title": "x", "version": 99})


@pytest.fixture(params=BACKENDS)
def box(request, tmp_path: Path):
    return make_inbox(request.param, tmp_path)


class TestInbox:
    def test_submit_claim_finish(self, box) -> None:
        inbox = box
        low = Brief("low", id="1", priority=0)
        high = Brief("high", id="2", priority=5)
        inbox.submit(low)
        inbox.submit(high)
        assert [b.title for b in inbox.list()] == ["high", "low"]
        claimed = inbox.claim()
        assert claimed == high and inbox.find(high.key)[0] == "claimed"
        inbox.finish(claimed, {"state": "done"}, ok=True, transcript=b"all good\n")
        assert inbox.transcript(high.key) == b"all good\n"
        assert inbox.find(high.key)[0] == "done"
        assert inbox.outcome(high.key) == {"state": "done"}
        assert [b.title for b in inbox.list("new")] == ["low"]

    def test_a_brief_is_filed_once(self, box) -> None:
        inbox = box
        inbox.submit(Brief("a", id="1"))
        with pytest.raises(BriefError, match="already new"):
            inbox.submit(Brief("a", id="1"))
        inbox.submit(Brief("a again", id="1"), replace=True)
        inbox.claim()
        with pytest.raises(BriefError, match="already claimed"):
            inbox.submit(Brief("a", id="1"), replace=True)

    def test_claim_by_key_and_empty(self, box) -> None:
        inbox = box
        assert inbox.claim() is None
        inbox.submit(Brief("a", id="1"))
        inbox.submit(Brief("b", id="2", priority=9))
        assert inbox.claim("manual--1").title == "a"

    def test_a_file_that_does_not_parse_is_skipped(self, tmp_path: Path) -> None:
        inbox = Inbox(tmp_path)
        (tmp_path / "new").mkdir()
        (tmp_path / "new" / "junk.json").write_text("{nope")
        inbox.submit(Brief("ok"))
        assert [b.title for b in inbox.list()] == ["ok"]


class TestSources:
    def test_parse_formats(self) -> None:
        one = {"title": "t", "id": "1"}
        assert len(parse_briefs(json.dumps([one]))) == 1
        assert len(parse_briefs(json.dumps({"briefs": [one, {**one, "id": "2"}]}))) == 2
        assert len(parse_briefs(json.dumps(one) + "\n" + json.dumps({**one, "id": "2"}))) == 2
        assert parse_briefs("  ") == []
        with pytest.raises(SourceError, match="'x'"):
            parse_briefs('[{"nope": 1}]', "x")

    def test_command_source_polls_owns_and_reports(self, tmp_path: Path) -> None:
        poller = script(
            tmp_path / "poll.py",
            """
            import json, os
            print(json.dumps([{"title": "from " + os.environ["COAGENT_SOURCE"], "id": "T-1"}]))
            """,
        )
        log = tmp_path / "reported.json"
        reporter = script(
            tmp_path / "report.py",
            f"""
            import sys
            open({str(log)!r}, "w").write(sys.stdin.read())
            """,
        )
        src = CommandSource("tracker", poller, report_command=reporter, station="G")
        inbox = Inbox(tmp_path / "inbox")
        filed, errors = poll([src], inbox)
        assert errors == [] and [b.key for b in filed] == ["tracker--T-1"]
        assert filed[0].station == "G" and filed[0].title == "from tracker"
        assert poll([src], inbox) == ([], [])  # already filed
        assert report(filed[0], {"state": "done"}, [src]) is True
        assert json.loads(log.read_text())["outcome"] == {"state": "done"}

    def test_a_failing_source_does_not_stop_the_others(self, tmp_path: Path) -> None:
        bad = CommandSource("bad", [sys.executable, "-c", "raise SystemExit(3)"])
        good = CommandSource("good", [sys.executable, "-c", 'print(\'[{"title": "t"}]\')'])
        filed, errors = poll([bad, good], Inbox(tmp_path))
        assert len(filed) == 1 and errors[0].startswith("bad: ")

    def test_sources_toml(self, tmp_path: Path) -> None:
        (tmp_path / "sources.toml").write_text(
            "[[source]]\nname = 'a'\ncommand = ['./x.sh']\nreport = ['r']\nstation = 'B'\n"
        )
        (src,) = load_sources(tmp_path / "sources.toml", entry_points=False)
        assert src.command == (str((tmp_path / "x.sh").resolve()),) and src.station == "B"
        (tmp_path / "sources.toml").write_text("[[source]]\nname = 'a'\ncomand = ['x']\n")
        with pytest.raises(SourceError, match="unknown keys"):
            load_sources(tmp_path / "sources.toml", entry_points=False)

    def test_report_to_an_unknown_source(self) -> None:
        assert report(Brief("t", source="gone"), {}, []) is False


def test_cli_submit_and_list(tmp_path: Path, capsys, monkeypatch) -> None:
    path = tmp_path / "b.json"
    path.write_text(json.dumps({"title": "Ship it", "id": "7", "station": "Q"}))
    assert main(["--inbox", str(tmp_path / "in"), "submit", str(path)]) == 0
    assert main(["--inbox", str(tmp_path / "in"), "list"]) == 0
    assert "manual--7 @Q  Ship it" in capsys.readouterr().out
    assert main(["--inbox", str(tmp_path / "in"), "submit", str(path)]) == 2
