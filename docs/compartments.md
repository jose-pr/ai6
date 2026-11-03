# Compartments

A **Compartment** is need-to-know access to other Operations' work. It says what may be known of,
and said to, a set of Operations or Missions, and who may. Real classification has the same two
axes, a level and compartments: being cleared does not by itself give access to anything in a
compartment; a person is **read in** to one, and **read out** when they no longer need to know.

| | Clearance | Compartment |
| --- | --- | --- |
| Answers | How far an Agent may act in its own work | What one may know of, and say to, other Operations' work |
| Values | `desk`, `field`, `00` | Any you define, a codeword by convention (`KESTREL`) |
| Held by | An Agent | People, groups, accounts, Operations, Missions and Agents |
| Changed by | Green ink, promotion, retirement | Reading in and out, covering and uncovering |

A compartment carries no policy authority. It never bypasses cover, Q leases, secrets, safe
houses, green ink, quarantine or Station policy, and its vocabulary is closed: an action outside
the list below makes the record invalid.

## The record

```toml
# compartments.toml, or the `compartment` family of a SQL provider
[compartment.KESTREL]
description = "The release of coqui"
actions = ["mission.status", "product.read", "cable.send"]
covers = [{ operation = "coqui" }, { mission = "ai6-41" }]
read_in = [
  { subject = "operation:ai6" },
  { subject = "group:release" },
  { subject = "agent:007" },
]
min_clearance = "field"                # optional; no floor by default

[compartment.KESTREL.limits]
max_fanout = 100
rate = 10.0
burst = 50
```

| Action | Grants | To |
| --- | --- | --- |
| `mission.status` | IDs, states, roles, Agent codenames, attempts and times, with no text | both |
| `mission.read` | The brief as well, and for a person what `mission.read` means anywhere; implies `mission.status` | both |
| `product.read` | The filed outcome, the CX report and the refs | both |
| `cable.send` | Sending Cables there | both |
| `cable.read` | Moneypenny reading the Cables sent to her there | people |
| `mission.propose` | Proposing Missions there, as untrusted intake | Agents |
| `memory.read` | Reading the Operation's [memory](memories.md), audited as a crossing | both |

**Covers** are what it grants access to: an Operation, with its Missions and everything under it
unless the entry says `propagate = false`, or one Mission. An Operation a compartment names must
exist, and removing one is refused like any edit that leaves the roster invalid; a Mission is a
runtime record, so an ID no Mission has simply matches nothing. A record may cover nothing, which
grants nothing: covers are added one at a time, and an Operation may always withdraw the last one.

**Read-ins** are who holds it: `person:`, `group:`, `account:`, `operation:`, `mission:` or `agent:`.
An Operation read in includes the Operations under it unless the entry says `propagate = false`;
nothing reaches upward. A Mission read in reaches only that Mission's Sessions. An Agent read in
applies in every Operation it works in: read in a Mission or an Operation to narrow.

Compartments add up: access is the union over all of them, and for a person the union with the
[access policy](access.md). A person's compartment never narrows a grant, and it does not count
towards what an editor may hand out.

`min_clearance` skips the compartment for a Session below it, read at each call, so a lowered
clearance holds from the next one. People have no clearance and are not asked.

## What is checked, and when

- **At every call.** Every read and every Cable asks the configuration provider what is granted at
  that moment, so a removed grant holds from the next call. Nothing is remembered between calls,
  and a running Session is held to it like any other.
- **A record that cannot be read grants nothing**, and the others go on. A provider that cannot be
  read refuses the call rather than answering from memory.
- **Every crossing is audited first.** A read, a send or a proposal across Operations is written to
  the Registry audit log before it returns, naming the reader's Operation, the target and the
  compartment. A failed write refuses the call. Both sides read it: `audit.read` on the reader's
  Operation or on the one read.

| Action | Written when | Resource | Target |
| --- | --- | --- | --- |
| `compartment.read` | An Agent's read; a person's read allowed only by a compartment | `operation:<id>`, `mission:<id>` or `compartment:<id>` | `<compartment> <tool> <mission or ->`, ending `refused` when refused |
| `mission.propose` | A proposal into another Operation | `operation:<id>` | `<compartment> propose_mission -` |
| `cable.send` | A Cable that crosses an Operation | `compartment:<id>` or `operation:<recipient>` | `<scope>:<name> <cable id>` |
| `cable.read` | Such a Cable delivered | `operation:<source>` | `<scope>:<name> <cable id>` |
| `compartment.put`, `.delete`, `.cover`, `.uncover`, `.read_in`, `.read_out` | A change to a record | `compartment:<id>` | the entry |

A person's reads repeat as the app polls, so `[gateway] compartment_audit` writes one record per
person, compartment, resource and action every 60 seconds by default; `0` writes one for each.

## Agents

A Session holding a compartment has these tools on its AI6 MCP server. They are listed only while
it holds one, and work from the identity the Station started the Session with. A harness lists its
tools when it starts, so a Session read in later sees them when it resumes.

| Tool | Needs | Answers |
| --- | --- | --- |
| `compartments` | Holding one | Each one's ID, description, what it grants, and the Operations and Missions it reaches |
| `missions_in(operation` or `compartment, limit)` | `mission.status` | Each Mission's ID, Operation, state, role, Agent, attempts, creation time and why it is blocked. Never a brief, a Session ID, a Station, a Theater, a safe house, a launch manifest, usage or cost |
| `mission_brief(mission)` | `mission.read` | The status and the brief's text |
| `mission_products(mission)` | `product.read` | The status, the outcome's `state`, `reason`, `branch` and `summary`, the CX report and the refs, each text cut at `max_product_bytes` |

These answer for other Operations only: an Operation's own work is read as it always was. Its
[memory tools](memories.md#agents) read another Operation's memory through `memory.read`. A
Mission the Session may not see answers as a Mission that does not exist.

A brief or a product is outside text, so reading one marks the Session
[exposed](medical.md#exposure) as `product:<operation>` before the text is returned, and the
Operation's exposure policy then applies. Status is not text and does not expose.

**Proposing across.** `propose_mission` takes `operation`. Another Operation needs `mission.propose`
there, and clearance `00` as any proposal does. The proposal is filed in the target Operation with
`from_operation` and the compartment, exposed `brief:compartment`, so the target's
[vetting](operations.md) treats it as untrusted, its Mission is exposed from the start, and
nothing promotes it but a person.

## Cables

`send_cable` takes scope `compartment` with the compartment as target: the live Sessions of every
Operation and Mission it covers, minus the sender's own, snapshotted when it is accepted. Optional
`operation` narrows that to one covered Operation. The sender needs `cable.send` in it, and being
covered is the receiving side's consent, so no receive grant is asked. The receiver's `[inbound]`
mode still decides deliver, hold or refuse, and a quarantined sender's Cables are held.

**Any other Cable that would cross an Operation** reaches a recipient only when a compartment gives
the sender `cable.send` on that recipient's Mission or Operation, as well as the Cable grants.

```toml
# the `cables` record
cross_operation = "compartments"       # default; "station_grants" lets a Station grant cross
```

`"station_grants"` is a weakening, logged at start and reported at check-in as `cables_weakened`.
Discovery follows the same line: `discover_agents` shows another Operation's Agents only through a
compartment, by codename, Operation and Mission, never by Station, Theater or Session.

Delivery asks the grant again: a Cable waiting when the sender is read out is refused, one already
delivered is not shown again, and one that cannot be checked stays where it is. A reply stays on
the grant that opened the conversation, back to that Session alone, so a covered Agent can answer
without being read in; once the opener is read out, the reply is refused. Reading a Cable from
another Operation marks the Session `cable:SENDER`, as ever.

A `compartment` grant in the `cables` record names only `action = "release"`, for who may release
what a `hold` mode held; any other action there is invalid.

<!-- schema:record.compartment.limits:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `max_fanout` | 100 | **Recipients of one Cable.** The most recipients one Cable to the compartment may reach. |
| `rate` | 10 | **Cables a second.** Cables a second to the compartment, from everyone, across Stations. |
| `burst` | 50 | **Burst of Cables.** Cables to the compartment in a burst. |
| `sender_rate` | none | **Cables a second, per sender.** Cables a second from one sender; `none` keeps no per-sender count. |
| `sender_burst` | none | **Burst per sender.** Cables from one sender in a burst; applies with `sender_rate`. |
| `max_product_bytes` | 64 KiB | **Size of a product text.** Bytes of one text an Agent reads from a product; the rest is cut with a marker. |
<!-- schema:record.compartment.limits:end -->

`rate` and `burst` count in the bucket `compartment:<id>`, shared by every Station on the Registry;
`sender_rate` and `sender_burst` in `compartment:<id>:sender:<agent>`. Each takes `none` for no
bound. A limit looser than its default is a weakening, shown in the record
and logged when the configuration is applied. The roster's `max_compartment_rules` (10,000) bounds
read-ins times covers per compartment; `none` lifts it. The sending Station's per-Agent bucket, the
one-hop rule and the body limit still apply. A relay sees the Sessions of its own Station, so a
compartment audience reaches those.

Moneypenny has no `compartment:` audience: her Cables stay per Operation, in the Operations the
person may use, which their compartments widen.

## People

A person holds an action where the access policy allows it or a compartment read into them grants
it. Only `mission.status`, `mission.read`, `product.read`, `cable.send` and `cable.read` can come from
a compartment. The profile lists, per Operation, the compartments that give the person something
there.

| Action | Where | Means |
| --- | --- | --- |
| `compartment.read` | `compartment:<id>` | Read the record |
| `compartment.manage` | `compartment:<id>`, or `*` to make new ones | Change its settings, delete it, read Agents, Operations and Missions in and out |
| `access.manage` | `compartment:<id>` | With `compartment.manage`, read a person, group or account in |
| `compartment.cover` | `operation:<id>` | That Operation's consent to be covered, its Missions included; a rule on a parent consents for its subtree |

Adding a cover needs `compartment.manage` and `compartment.cover`. Removing one needs either, so an
Operation may always withdraw. Who manages a compartment is a matter for the access policy alone:
the record has no owner field. `compartment.cover` is left out of what an Operation's own roles may
hold by default (`delegable`).

A record shows a person only what they could see: a cover or a read-in of an Operation or Mission
they cannot read with `mission.status` or `mission.read` is neither named nor counted, and an edit
leaves it as it was. Settings and each entry are written separately for that reason, each checked
against what was read.

| Route | Answers |
| --- | --- |
| `GET /v1/user/compartments`, `/{id}` | The record, its weakenings, and the entries the person may see |
| `GET /v1/user/compartment-form` | The form its settings are edited with |
| `PUT`, `DELETE /v1/user/compartments/{id}` | Its description, actions, floor and limits; never covers or read-ins |
| `PUT`, `DELETE /v1/user/compartments/{id}/covers/{resource}` | One cover, `operation:<id>` or `mission:<id>` |
| `PUT`, `DELETE /v1/user/compartments/{id}/read-in/{subject}` | One read-in |
| `GET /v1/missions/{id}/products` | What a Mission produced; anything the person may not read is `404` |
| `GET /v1/missions?compartment=K` | Missions the person may read that K covers |

A provider that takes no changes answers `409` with its reason; a directory of family files is edited
by hand or through version control.

```text
ai6ctl compartments
ai6ctl compartments show KESTREL
ai6ctl compartments set KESTREL --description "The release" --actions mission.status,product.read \
    --min-clearance field --max-fanout none
ai6ctl compartments cover KESTREL operation:coqui [--no-propagate]
ai6ctl compartments uncover KESTREL operation:coqui
ai6ctl compartments read-in KESTREL group:release [--no-propagate]
ai6ctl compartments read-out KESTREL group:release
ai6ctl compartments remove KESTREL
ai6ctl missions --compartment KESTREL
ai6ctl missions products ai6-41
```

Outer Office has a **Compartments** page: settings through the form, a Weakened chip, covers and
read-ins as lists, with **Cover…** offering only the Operations where the person holds
`compartment.cover`. An Operation shows the compartments that give the person something there, and
the Missions page filters by compartment. Moneypenny's `compartments` tool reads the same list.

## Protecting a memory entry

A [memory entry](memories.md#protected-by-compartments) can name compartments: only those read
into one of them see it at all, whatever the compartment grants or covers. A compartment that
does not exist or cannot be read reads nobody in, so its entries are hidden from everyone but a
chief.
