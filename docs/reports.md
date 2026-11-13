# Reports

A **Report** is one record of something observed: an Agent's finding along the way, a person's
note, the Head of Station's round. It is kept in the Registry against its **scope**, an
Operation, a Station or M, and tied to what it is about. Filing a Report creates no Mission,
brief or placement: an Operation's charge or a person decides what, if anything, it becomes.

A Report is filed `open`. The Operation's charge, or a person where there is none, **assesses** it
(see [Assessing](#assessing)); it is then `assessed` until what it was assessed into ends, and
`closed` with its resolution kept.

## The record

| Field | What |
| --- | --- |
| `id` | `report-` and a random hex string |
| `scope` | `operation:ID`, `station:ID` or `m` |
| `kind`, `severity` | A short label (`finding` by default) and `info`, `warning` or `critical` |
| `title`, `text` | What the filer said. Untrusted unless a person filed it |
| `filer` | A Source and its item, an Agent with its Run and Mission, a person, a probe, a review or the Watchdog |
| `sub_source`, `from` | Who passed it on to the filer, and when the information is from; `created` is when it was reported |
| `links` | Missions, other Reports, decisions (green ink requests) and evidence |
| `state` | `open`, `assessed` or `closed`, with `assessment` (the assessor's verdict and comment, apart from the filed text) and `resolution` |
| `credibility` | `1` (confirmed by other sources) to `6` (cannot be judged); unset until assessed, and `1` is refused without a link to the corroborating Report or evidence |
| `classification`, `handling`, `compartments` | The markings, from the lists in [Settings](#settings) |
| `tearline` | A sanitised text with its own, lower, markings |
| `priority`, `deadline` | From the priorities list ([Settings](#settings)), proposed by the filer; a deadline in Unix seconds |

A Report's **source** is its filer (`agent:007`, `person:ana`, `source:NAME`, `probe:NAME`,
`review` or `watchdog`), and each source has a record of its **reliability**, `A` (completely
reliable) to `F` (cannot be judged). A source with no record is `F`, as every new one is.

Reports list most urgent first: by priority, then deadline (sooner first), credibility, the
source's reliability, then age (oldest first). An unset priority, deadline or credibility comes
after every set one.

The file Registry keeps `reports/<scope>/<id>.json` and `report-sources/<key>.json`; SQL keeps
the `ai6_reports`, `ai6_report_links` and `ai6_report_sources` tables.

## Who reads what

Reading a Report's content takes all three:

1. **The scope's read.** For an Operation, `mission.read` there: for a person from the
   [access policy](access.md) or a [compartment](compartments.md) granting `mission.read` on it,
   for an Agent's Run its own Operation, or another through such a compartment. A read
   through a compartment is a crossing, written to the audit log first. A Station's Reports need
   `station.read` on `station:ID` (and for an Agent, being that Station's Head of Station
   round), M's `station.read` on `config:reports`.
2. **Being read in to each of its compartments**, as a compartment's `read_in` lists people,
   groups, accounts, Operations, Missions and Agents. Reading in and out is the compartment's own,
   audited like any change to it, and holds from the next call. A compartment that does not exist
   reads nobody in.
3. **A clearance at or above its classification**, from the [clearances](#clearances) in the
   Registry. A Report with no
   classification is at the lowest.

The rule is the same for a person and for an Agent's Run, and nobody is exempt. A reader
who has the first but not the other two sees:

- the **tearline** in the Report's place, when it carries one and they are cleared for the
  tearline's own classification and read in to its compartments (which are some of the Report's);
- otherwise its **ID, scope and markings** alone. Reading it is refused (`403`), and a listing
  shows it that way.

**A link is not access.** Whatever links to a Report (a Mission's Reports, another Report)
shows it to someone not read in by its ID and markings, never its title or text.

## Markings

The filer sets the classification, the handling mark, the compartments and the tearline when
filing, at any level the lists allow. Changing them afterwards needs someone who reads the content
as it is and would read it as it will be: lowering them needs someone read in, and nobody marks a
Report past what they hold themselves. The tearline is never classified above its Report.

The handling mark says how far a Report may go (this Operation, its compartments, a peer through
Liaison). It is checked at every crossing an assessment makes ([Assessing](#assessing)): a Report
goes only as far as its mark, and a Report with none is marked the narrowest.

## Filing

**Agents.** Every Agent's Run has `file_report` on its AI6 MCP server, filed against its
Mission's Operation and linked to its Mission ([Agents](agents.md#reports)). It takes a summary
(its first line is the title), detail, typed findings, a proposed priority and deadline, and the
markings. `reports` lists and `read_report` reads what the Run may, inside `<ai6-data>`; a
Report from another Operation marks the Run exposed as that Operation's product.

A **Head of Station** round's `file_report` keeps its logbook, check-in and channel
([Stations](stations.md)), and is also stored as a Report of its Station, kind `station`, linked
to the round.

An Agent's filings are rate limited per Run (`per_run`) and per scope (`per_operation`)
over `window`; past either, `file_report` answers to wait.

**People.** Filing needs `mission.create` on the Operation, or `station.configure` on the Station
or on `config:reports`.

| Route | Answers |
| --- | --- |
| `POST /v1/reports` | `{"scope", "title", "text", "kind", "severity", "priority", "deadline", "information_from", "sub_source", "classification", "handling", "compartments", "tearline", "links"}`; `201` with the Report |
| `GET /v1/reports?scope=S` or `?mission=ID` | `{"reports": [...], "priorities": [...], "classifications": [...]}`: the Reports most urgent first, with `state`, `kind` and `limit` to narrow, and the deployment's lists an assessment and the markings choose from |
| `GET /v1/reports/{id}` | The Report, or its tearline; `403` with `report` (its ID and markings) when that is all the person may see; `404` for a scope they may not read |
| `PUT /v1/reports/{id}/markings` | `{"classification", "handling", "compartments", "tearline"}`, replacing all four |
| `PUT /v1/reports/{id}/assessment` | `{"assessment", "reason", ...}`: assesses an open Report ([Assessing](#assessing)); needs `mission.control` on its Operation (`station.configure` for a Station's or M's) and reading its content. `404` for a scope the person may not read, `403` without the action or the content, `409` for a Report that is not open, `400` for anything out of bounds |
| `POST /v1/reports/{id}/reopen` | `{"reason"}`: opens an assessed or closed Report again; the same authority. `409` for one already open |
| `GET /v1/reports/sources/{key}` | A source's `reliability`, its tally (`counted`, `confirmed`), the `rule` it is judged by and the closed `reports` behind it that the person may read |
| `GET /v1/missions/{id}/reports` | The Reports tied to a Mission the person reads, each as far as they may see it |

Each answer has `sight`: `content`, `tearline` or `markings`.

```text
ai6ctl reports file operation:coqui --title "The release notes are stale" --text "They name 0.3." \
    --priority low --link mission:coqui-41
ai6ctl reports file operation:coqui --title "A key leaked" --text "..." --compartments KESTREL \
    --classification restricted --tearline "Rotate the deploy key."
ai6ctl reports list --scope operation:coqui [--state open]
ai6ctl reports list --mission coqui-41
ai6ctl reports show report-0f3c...
ai6ctl reports assess report-0f3c... duplicate --of coqui-41 --reason "the same fault"
ai6ctl reports assess report-0f3c... awaiting --question "Which branch?" --wait-seconds 86400 \
    --reason "cannot tell" --likelihood likely --confidence moderate
ai6ctl reports reopen report-0f3c... --reason "answered: main"
ai6ctl reports source agent:007
```

## Assessing

An open Report is assessed once, by the Operation's charge with `assess_report` or by a person with
`ai6ctl reports assess` or the route above. Every assessment is audited before it is recorded, kept
on the Report apart from the filer's words (the reason is the assessor's comment), and put on the
Operation's timeline. It may also set the Report's `credibility` and `priority`, and state the
assessor's judgement in the words of the UK probability yardstick with a confidence: `likelihood`
(`remote chance` under 5%, `highly unlikely` 10-20%, `unlikely` 25-35%, `realistic possibility`
40-50%, `likely` 55-75%, `highly likely` 80-90%, `almost certain` over 95%) and `confidence` (`high`,
`moderate`, `low`); the band is shown beside the words, never in place of them. Credibility `1` needs
a corroborating Report or evidence link.

| Assessment | Takes | Then |
| --- | --- | --- |
| `mission` | `mission`: the Mission proposed or amended for it (a Source item needs none: the Station that polls its Source queues it) | `assessed`, linked to the Mission; closes when the Mission settles |
| `duplicate` | `of`: a Report, Mission or evidence | `closed`; a Report it repeats names it back, so each lists the other |
| `related` | `of`, the same | `closed`, tied the same way |
| `no_action` | | `closed` with the reason |
| `awaiting` | `question`, `wait_seconds` (its deadline), optionally `ask` (who) and `via` (a handling mark) | `assessed`; open again at the deadline or when reopened |
| `later` | `wait_seconds`, `condition`, or both | `assessed`; open again at its time or when reopened |
| `decision` | `question` | `assessed`, with a green ink request of kind `decision` for those who may sign; closes when it is signed |

A Report closes when what it was assessed into ends, and its resolution stays: a Mission settles
done (after verifying) or failed, or a `decision` request is approved or rejected. A request that
expires unsigned opens the Report again rather than close it on an answer nobody gave. Every
Station may run the pass that closes and reopens, since a Report is changed once wherever it is met;
the Operation's charge is told by Cable, by ID alone, of a Report that is open again. Reopening is
audited (`report.reopen`), and so are closing (`report.close`) and assessing (`report.assess`).

A tie to a Report of another scope is made only when the assessor reads that scope and both
Reports' handling marks reach `compartments`; an `awaiting` question goes only as far as `via`
allows the Report's mark to go. Each such crossing is audited (`report.cross`).

**A source's reliability** is earned from how its Reports came out. When a Report closes, its
outcome (`mission_done`, `mission_failed`, `decision_approved`, `decision_rejected`, `duplicate`,
`related`, `no_action`) counts toward its source if the deployment's rule says so, and the source is
rated once enough have counted ([Settings](#settings)). `GET /v1/reports/sources/{key}` and
`ai6ctl reports source` show the rating with the tally, the rule and the closed Reports behind it.

## The Reports page

Outer Office's **Reports** page, under Work, is where a person reads and assesses Reports. It is
there for anyone who reads an Operation (`mission.read`) or a Station (`station.read`), and lists one
row per Report across them, most urgent first. Its table has the app's usual sort, search and density,
and filters by Operation, state, who filed it (a Source, an Agent, a charge or a person),
classification, kind and priority; **Needs assessment**, **Assessed** and **Closed** are one click.
A view is a link (`/reports?state=open&operation=G`), and a scope the person cannot read is said in a
line above the list, so a short list is not taken for a whole one. M's own Reports are not listed.

Selecting a row shows the Report beside the list (under it on a narrow screen): its title and text
with what renders as nothing spelled out, a note that an Agent's or a Source's words are reported and
not verified, who filed it, its source with the reliability letter, when the information is from and
when it was filed, its markings, the assessment (the verdict, the assessor, the judgement in the
yardstick's words with its band, the assessor's comment, what it waits for and until when), the
resolution and how it came out, and its links: a Mission and a decision open on their own pages, and
a Report opens in the list. A Report the person may see only as its tearline, or by its markings
alone, says so and has no actions.

An open Report has **Assess this Report**, which asks for how it is assessed (`mission`, `duplicate`,
`related`, `no_action`, `awaiting`, `later` or `decision`), what that takes (an existing Mission, or
for a Source item none, since the Station that polls the Source queues its brief; the other Report
to pick; the question and the deadline; what it waits for), a note that is kept as the assessor's
comment, and optionally the credibility, the priority from the deployment's list, and the likelihood
with its confidence. An assessed or closed Report has **Reopen this Report**, which asks why. Both
need `mission.control` on the Operation (`station.configure` on a Station) and are shown disabled
with that said when the person lacks it; a refusal of the gateway (permission, a handling mark, the
desk's clearance) is shown in its own words and leaves the dialog open. **Source standing** opens a
source's reliability, how it was earned and the closed Reports behind it, each of which opens.

A Mission's page has a **Reports** section: those it was begun for or resolves, and those its Agents
filed while it ran, each opening on this page. It is shown to whoever reads the Operation.

The attention bell and the Overview do not count unassessed Reports: Universal Exports' attention
summary (`GET /v1/user/attention`) has no Reports section.

## What files them

| Producer | Kind | Filed against, linked to |
| --- | --- | --- |
| An Agent's `file_report` | its own | its Operation, linked to its Mission |
| A Source's item | `source-item` | its Operation; the filer is `source:INSTANCE` and the item. One Report per item: a changed item updates it |
| Deliverables read back (a probe) | `evidence` | the Mission's Operation, linked to the Mission; a warning when anything is not found. Filer `probe:deliverables` |
| Vetting holding a brief | `vetting-hold` | the brief's Operation, linked to the Mission it would be. Filer `probe:vetting` |
| The Chief of Staff's review holding a brief | `review-hold` | as above; the filer is the review of that Mission |
| M's scorecard proposing a hold or a preference | `scorecard` | the Operation, linked (`decision`) to its green ink request. Filer `probe:scorecard` |
| The Head of Station | the finding's | its Station |
| A person | their own | the scope they name |

A producer files beside the record it already keeps (the evidence on the run, the hold on the
brief, the green ink request), not in place of it, and files again for the same thing as an
update: a probe run twice, or a hold seen on every pass, is one Report. A Report that cannot be
stored is logged and never stops the check.

A Source item's Report keeps the item as last polled and the decision on it, and the Operation's
charge (or a person) decides it: [Items are Reports](missions.md#items-are-reports).

## Clearances

Who reads which classification is kept in the Registry: one record per subject, `person:ID`,
`group:NAME` or `agent:CODENAME`, naming the highest classification it reads. A reader counts the
highest of the records that name them; anyone no record names reads only the lowest
classification. Every read looks them up again, so a grant or a revoke holds from the next
request, with nothing restarted.

A clearance hands someone more than their access, so changing one takes `access.manage` on
`config:reports`, as reading a person into a compartment takes `access.manage`; listing them takes
`access.read` there. Each grant and revoke is written to the audit log
(`report.clearance.grant`, target `SUBJECT CLASSIFICATION`; `report.clearance.revoke`, target
`SUBJECT`; resource `config:reports`) before it is made, and a failed write refuses it.

| Route | Answers |
| --- | --- |
| `GET /v1/reports/clearances` | `{"clearances": [{"subject", "classification", "by", "at"}], "classifications"}` |
| `PUT /v1/reports/clearances/{subject}` | `{"classification"}`, one of the deployment's; replaces the subject's |
| `DELETE /v1/reports/clearances/{subject}` | `{"revoked": bool}` |

```text
ai6ctl reports clearance list
ai6ctl reports clearance grant group:security confidential
ai6ctl reports clearance revoke group:security
```

The file Registry keeps them in `report-clearances/`, SQL in `ai6_report_clearances`.

## Settings

The lists a Report is marked and prioritised from are deployment policy, kept by the
configuration provider in the `roster` record's `reports` table, and read again at every call.
Without the table, the defaults below apply.

```toml
# operations.toml, or the roster record of a SQL provider
[roster.reports]
classifications = ["unclassified", "restricted", "confidential"]   # lowest first
handlings = ["operation", "compartments", "peers"]                 # narrowest first
priorities = ["critical", "high", "normal", "low"]                 # most urgent first
rating_min_reports = 5                                             # counted Reports before a source is rated
rating_counts = ["mission_done", "mission_failed", "decision_approved", "decision_rejected", "no_action"]
rating_confirms = ["mission_done", "mission_failed", "decision_approved"]
rating_bands = [90, 75, 50, 25]                                    # percent confirmed for A, B, C, D; below, E
```

A Report filed or remarked with a value outside these lists is refused. One already stored with a
classification since removed from the list is shown to nobody but by its ID and markings.

Sizes, counts and rates are Registry limits, in `[registry.limits.reports]` of `ai6.cfg`, read by
the Stations and the gateway alike when they start ([Configuration](configuration.md)):
`record_bytes`, `reports`, `text_bytes`, `name_bytes`, `links`, `page`, `per_run`,
`per_operation`, `window` and `keep_closed`.
