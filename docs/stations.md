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
view of Station health, roles, capacity, the Operations each Station serves and the
current Headquarters Station; host and process details are omitted.

## Station identity

With the `station-tls` feature, each Station has a persistent certificate:
`ai6 identity` prints its fingerprint, `ai6 peer waiting` lists peers a listener refused,
and `ai6 peer accept FINGERPRINT` pins one.
