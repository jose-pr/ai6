from __future__ import annotations

import time
from pathlib import Path

import pytest

from coagent_i import Brief
from coagent_i.registry import LeaseLost

from backends import BACKENDS, make_registry


@pytest.fixture(params=BACKENDS)
def registry(request, tmp_path: Path):
    box = make_registry(request.param, tmp_path)
    box.submit(Brief("work", id="1"))
    return box


def test_a_claim_holds_a_lease_its_node_renews(registry) -> None:
    brief = registry.claim(office="a", lease=30)
    lease = registry.lease(brief.key)
    assert (lease.office, lease.attempt) == ("a", 1) and lease.expires > time.time() + 20
    assert registry.renew(brief.key, "a", lease=30)
    assert not registry.renew(brief.key, "b")
    assert registry.expired() == []


def test_a_lost_node_s_brief_is_requeued_then_failed(registry) -> None:
    for attempt in (1, 2):
        brief = registry.claim(office=f"n{attempt}", lease=0.01)
        time.sleep(0.02)
        (lease,) = registry.expired()
        assert lease.attempt == attempt
        assert registry.recover(lease, max_attempts=3, by="survivor") == "new"
        assert registry.find(brief.key)[0] == "new"
    brief = registry.claim(office="n3", lease=0.01)
    time.sleep(0.02)
    (lease,) = registry.expired()
    assert registry.recover(lease, max_attempts=3, by="survivor") == "failed"
    outcome = registry.outcome(brief.key)
    assert outcome["state"] == "lost" and "n3 was lost; 3 attempts" in outcome["reason"]


def test_recovery_is_refused_for_a_live_or_renewed_lease(registry) -> None:
    brief = registry.claim(office="a", lease=0.01)
    time.sleep(0.02)
    (stale,) = registry.expired()
    registry.renew(brief.key, "a", lease=30)
    assert registry.recover(stale) is None
    assert registry.find(brief.key)[0] == "claimed"


def test_a_node_that_lost_its_lease_cannot_file(registry) -> None:
    brief = registry.claim(office="a", lease=0.01)
    time.sleep(0.02)
    registry.recover(registry.expired()[0])
    registry.claim(office="b")
    with pytest.raises(LeaseLost):
        registry.finish(brief, {"state": "done"}, ok=True, office="a")
    assert not registry.renew(brief.key, "a")
    registry.finish(brief, {"state": "done"}, ok=True, office="b")
    assert registry.find(brief.key)[0] == "done" and registry.lease(brief.key) is None


def test_lease_meta_and_listing(registry) -> None:
    brief = registry.claim(office="a", meta={"station": "G", "profile": "007"})
    registry.renew(brief.key, "a")
    (lease,) = registry.leases()
    assert lease.meta == {"station": "G", "profile": "007"}


def test_offices_announce_themselves(registry) -> None:
    registry.heartbeat("office-1", {"capacity": 4})
    registry.heartbeat("office/2", {"capacity": 2})
    assert sorted(o["office"] for o in registry.offices()) == ["office-1", "office/2"]
    assert registry.offices(within=-1) == []


def test_one_leader_at_a_time_and_takeover(registry) -> None:
    assert registry.elect("a", ttl=0.05)
    assert not registry.elect("b", ttl=0.05)
    assert registry.elect("a", ttl=0.05)  # renewal
    time.sleep(0.08)
    assert registry.elect("b", ttl=30)
    assert not registry.elect("a", ttl=30)
    assert registry.leader()["office"] == "b"
