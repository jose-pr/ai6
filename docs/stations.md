# Stations and failover

A **Station** is a node running the `ai6` daemon. It checks in to the shared Registry
with its roles, labels, capacity, active work and the Operation checkouts it has. One
control Station holds the Headquarters lease at a time and is the **Headquarters Station
(HQS)**; M's authority follows that lease.

Start a Station with both roles:

```console
ai6 serve --station hq-1 --role control --role execution
```

A standby control Station takes over the Headquarters lease if it expires
(*relocation* to a *fallback Station*):

```console
ai6 serve --station hq-2 --role control
```

An execution-only Station (an *outstation*) runs work but cannot hold the lease:

```console
ai6 serve --station lab-gpu --role execution --label gpu=yes
```

The default roles are `control` and `execution`. Every Station is also labeled with its
Station ID, host and operating system; `--label KEY=VALUE` adds more.

## Serving Operations

Operations are global: they live in `operations.toml`, and no Station owns one. A Station
runs an Operation's Missions when its labels satisfy the Operation's `station_labels` and, for
a local Operation, when it has the checkout. One Station can serve many Operations, and
one Operation can be served by many Stations.

## Placement

The Headquarters Station places each queued brief with M's placement. For one Mission it
considers every Station that has checked in, each Theater that Station reaches (see
[Theaters](theaters.md)) and each Agent posted to the Operation, and keeps the
combinations that pass these checks:

| Check | A Station, Theater or Agent is ruled out when |
| --- | --- |
| Operation `stations` | the Operation lists Stations and this one is not among them |
| Mission `stations` | the brief narrows to Stations and this one is not among them. A Mission can only narrow the Operation's list, never widen it |
| Role | the Station has no `execution` role |
| Health | its last check-in is older than 60 seconds (gone dark) |
| Capacity | its capacity is 0, or it already holds that many leased Missions |
| Theater binding | the Operation's `theater` names a different Theater |
| Theater access | the Theater is reached over SSH, which placement does not choose yet |
| Labels | `station_labels` is not satisfied. Patterns are globs matched against the Station's labels overlaid by the Theater's |
| Checkout | the Operation has a local `path` and the Station does not hold it |
| Handler | the Agent's handler is not available on the Station, so the next qualified Agent is the backup. A Station that did not report its handlers is not excluded |

Among what remains, the least-loaded Station comes first (the deciding Station wins a tie,
then Station ID), the `local` Theater before other Theaters, then the Agent with the
highest priority. There is no scoring beyond that.

Ask before launching: `GET /v1/user/operations/{id}/placement-preview` on Universal
Exports (needs `mission.read` on the Operation) returns the eligible choices, best first,
and a reason for every rejection. Moneypenny's Stations view has a **Placement preview**
panel for it. The query parameters `agent`, `role`, `stations` and `tags` narrow it the
way a Mission would. The preview uses the Operation's policy as the Headquarters Station
published it at its last check-in, and it lists every Station in the Registry, not only
those hosting the Operation.

## Terminal

The Station, not the Agent, decides which terminal provider harnesses run in. It uses
an available provider, preferring `pty`; `ai6 serve --terminal ID` pins one. See
[Terminals](terminals.md).

## Shared Registry

Stations sharing a file Registry (`AI6_REGISTRY`) coordinate check-ins, leases and
Headquarters election. Lease expiry uses wall-clock time, so Stations need synchronized
clocks. Check-ins expire after 45 seconds; a Station whose check-ins lapse has *gone dark*
and its leased briefs are recovered by another Station. The Registry is file-backed; a SQL
backend is not supported.

## Capacity and visibility

Operators granted `station.configure` can change a Station's maximum concurrent Missions
(1–1,024) in Moneypenny's Administration view; the change applies to new Missions and is
audited. `GET /v1/stations` on Universal Exports returns the signed-in user's authorized
view of Station health, roles, capacity, the Operations each Station serves, the Theaters
each Station reaches and the current Headquarters Station; host and process details are
omitted.

## Station identity

With the `station-tls` feature, each Station has a persistent certificate:
`ai6 identity` prints its fingerprint, `ai6 peer waiting` lists peers a listener refused,
and `ai6 peer accept FINGERPRINT` pins one.
