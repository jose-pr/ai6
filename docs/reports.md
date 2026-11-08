# Reports

A **Report** is one record of something observed: an Agent's finding along the way, a person's
note, the Head of Station's round. It is kept in the Registry against its **scope**, an
Operation, a Station or M, and tied to what it is about. Filing a Report creates no Mission,
brief or placement: an Operation's watch or a person decides what, if anything, it becomes.

Assessment (turning a Report into a Mission, a link, no action, a question or a decision) and the
closing that follows it are not implemented yet: a Report is filed `open` and stays so, and its
assessment and resolution fields are empty.

## The record

| Field | What |
| --- | --- |
| `id` | `report-` and a random hex string |
| `scope` | `operation:ID`, `station:ID` or `m` |
| `kind`, `severity` | A short label (`finding` by default) and `info`, `warning` or `critical` |
| `title`, `text` | What the filer said. Untrusted unless a person filed it |
| `filer` | A Source and its item, an Agent with its Session and Mission, a person, a probe, a review or the Watchdog |
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
   for an Agent's Session its own Operation, or another through such a compartment. A read
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

The rule is the same for a person and for an Agent's Session, and nobody is exempt. A reader
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
Liaison). It is kept and shown; the checks at each crossing come with assessment.

## Filing

**Agents.** Every Agent's Session has `file_report` on its AI6 MCP server, filed against its
Mission's Operation and linked to its Mission ([Agents](agents.md#reports)). It takes a summary
(its first line is the title), detail, typed findings, a proposed priority and deadline, and the
markings. `reports` lists and `read_report` reads what the Session may, inside `<ai6-data>`; a
Report from another Operation marks the Session exposed as that Operation's product.

A **Head of Station** round's `file_report` keeps its logbook, check-in and channel
([Stations](stations.md)), and is also stored as a Report of its Station, kind `station`, linked
to the round.

An Agent's filings are rate limited per Session (`per_session`) and per scope (`per_operation`)
over `window`; past either, `file_report` answers to wait.

**People.** Filing needs `mission.create` on the Operation, or `station.configure` on the Station
or on `config:reports`.

| Route | Answers |
| --- | --- |
| `POST /v1/reports` | `{"scope", "title", "text", "kind", "severity", "priority", "deadline", "information_from", "sub_source", "classification", "handling", "compartments", "tearline", "links"}`; `201` with the Report |
| `GET /v1/reports?scope=S` or `?mission=ID` | `{"reports": [...]}` most urgent first, with `state`, `kind` and `limit` to narrow |
| `GET /v1/reports/{id}` | The Report, or its tearline; `403` with `report` (its ID and markings) when that is all the person may see; `404` for a scope they may not read |
| `PUT /v1/reports/{id}/markings` | `{"classification", "handling", "compartments", "tearline"}`, replacing all four |
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
```

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
```

A Report filed or remarked with a value outside these lists is refused. One already stored with a
classification since removed from the list is shown to nobody but by its ID and markings.

Sizes, counts and rates are Registry limits, in `[registry.limits.reports]` of `ai6.cfg`, read by
the Stations and the gateway alike when they start ([Configuration](configuration.md)):
`record_bytes`, `reports`, `text_bytes`, `name_bytes`, `links`, `page`, `per_session`,
`per_operation`, `window` and `keep_closed`.
