from __future__ import annotations

import time
from pathlib import Path

import pytest

from coagent_i import Brief
from coagent_i.inbox import Inbox, LeaseLost


@pytest.fixture
def inbox(tmp_path: Path) -> Inbox:
    box = Inbox(tmp_path)
    box.submit(Brief("work", id="1"))
    return box


def test_a_claim_holds_a_lease_its_node_renews(inbox: Inbox) -> None:
    brief = inbox.claim(node="a", lease=30)
    lease = inbox.lease(brief.key)
    assert (lease.node, lease.attempt) == ("a", 1) and lease.expires > time.time() + 20
    assert inbox.renew(brief.key, "a", lease=30)
    assert not inbox.renew(brief.key, "b")
    assert inbox.expired() == []


def test_a_lost_node_s_brief_is_requeued_then_failed(inbox: Inbox) -> None:
    for attempt in (1, 2):
        brief = inbox.claim(node=f"n{attempt}", lease=0.01)
        time.sleep(0.02)
        (lease,) = inbox.expired()
        assert lease.attempt == attempt
        assert inbox.recover(lease, max_attempts=3, by="survivor") == "new"
        assert inbox.find(brief.key)[0] == "new"
    brief = inbox.claim(node="n3", lease=0.01)
    time.sleep(0.02)
    (lease,) = inbox.expired()
    assert inbox.recover(lease, max_attempts=3, by="survivor") == "failed"
    outcome = inbox.outcome(brief.key)
    assert outcome["state"] == "lost" and "n3 was lost; 3 attempts" in outcome["reason"]


def test_recovery_is_refused_for_a_live_or_renewed_lease(inbox: Inbox) -> None:
    brief = inbox.claim(node="a", lease=0.01)
    time.sleep(0.02)
    (stale,) = inbox.expired()
    inbox.renew(brief.key, "a", lease=30)
    assert inbox.recover(stale) is None
    assert inbox.find(brief.key)[0] == "claimed"


def test_a_node_that_lost_its_lease_cannot_file(inbox: Inbox) -> None:
    brief = inbox.claim(node="a", lease=0.01)
    time.sleep(0.02)
    inbox.recover(inbox.expired()[0])
    inbox.claim(node="b")
    with pytest.raises(LeaseLost):
        inbox.finish(brief, {"state": "done"}, ok=True, node="a")
    assert not inbox.renew(brief.key, "a")
    inbox.finish(brief, {"state": "done"}, ok=True, node="b")
    assert inbox.find(brief.key)[0] == "done" and inbox.lease(brief.key) is None


def test_lease_meta_and_listing(inbox: Inbox) -> None:
    brief = inbox.claim(node="a", meta={"station": "G", "profile": "007"})
    inbox.renew(brief.key, "a")
    (lease,) = inbox.leases()
    assert lease.meta == {"station": "G", "profile": "007"}


def test_offices_announce_themselves(inbox: Inbox) -> None:
    inbox.heartbeat("office-1", {"capacity": 4})
    inbox.heartbeat("office/2", {"capacity": 2})
    assert sorted(o["node"] for o in inbox.offices()) == ["office-1", "office/2"]
    assert inbox.offices(within=-1) == []


def test_one_leader_at_a_time_and_takeover(inbox: Inbox) -> None:
    assert inbox.elect("a", ttl=0.05)
    assert not inbox.elect("b", ttl=0.05)
    assert inbox.elect("a", ttl=0.05)  # renewal
    time.sleep(0.08)
    assert inbox.elect("b", ttl=30)
    assert not inbox.elect("a", ttl=30)
    assert inbox.leader()["node"] == "b"
