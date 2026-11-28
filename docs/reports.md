---
kind: tradecraft
topics: [reports, assessment, knowledge, memory, tradecraft]
summary: What a Report is, how it is filed, assessed and read, and the kinds of knowledge it holds.
---

# Reports

A **Report** is one record of something observed: an Agent's finding along the way, a person's
note, the Station charge's report. It is kept in the Registry against its **scope**, an
Operation, a Station, M, a person or everyone, and tied to what it is about. What AI6 knows or was
told is a Report too, of a known kind: see [Knowledge](#knowledge). Filing a Report creates no Mission,
brief or placement: an Operation's charge or a person decides what, if anything, it becomes.

A Report is filed `open`. The Operation's charge, or a person where there is none, **assesses** it
(see [Assessing](#assessing)); it is then `assessed` until what it was assessed into ends, and
`closed` with its resolution kept.

## The record

| Field | What |
| --- | --- |
| `id` | `report-` and a random hex string |
| `scope` | `operation:ID`, `station:ID`, `person:ID`, `global` or `m` |
| `kind`, `severity` | A short label (`finding` by default; `memory`, `tradecraft`, `rule` and `contact` are [knowledge](#knowledge)) and `info`, `warning` or `critical` |
| `title`, `text` | What the filer said. Untrusted unless a person filed it |
| `summary`, `topics` | One line a search lists beside the title, and free words a search and a facet use |
| `filer` | A Source and its item, an Agent with its Run and Mission, a person, Moneypenny for a person, a probe, a review or the Watchdog; for knowledge, who wrote what it holds now |
| `revision`, `updated_by` | How many times knowledge has been written (a write names the revision it read), and who made the current one (`person:ID`, `agent:CODENAME` or `moneypenny:ID`) |
| `exposures`, `proposes` | How the Run that wrote it was exposed; what an open proposal asks to change in another Report |
| `sub_source`, `from` | Who passed it on to the filer, and when the information is from; `created` is when it was reported |
| `links` | Missions, other Reports, decisions (green ink requests) and evidence |
| `state` | `open`, `assessed`, `closed` or `standing` (knowledge only), with `assessment` (the assessor's verdict and comment, apart from the filed text) and `resolution` |
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

The file Registry keeps `reports/<scope>/<id>.json`, `report-versions/<id>/<revision>.json` and
`report-sources/<key>.json`; SQL keeps the `ai6_reports`, `ai6_report_links`, `ai6_report_versions`
and `ai6_report_sources` tables, and the words a search reads (an FTS5 table on SQLite, a generated
`tsvector` on PostgreSQL).

## Who reads what

Reading a Report's content takes all three:

1. **The scope's read.** For an Operation, `mission.read` there: for a person from the
   [access policy](access.md) or a [compartment](compartments.md) granting `mission.read` on it,
   for an Agent's Run its own Operation, or another through such a compartment. A read
   through a compartment is a crossing, written to the audit log first. A Station's Reports need
   `station.read` on `station:ID` (and for an Agent, being that Station's charge), M's `station.read` on `config:reports`.
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

**A Run files only what its Operation's charge can read.** When the Operation has a charge, a
Report an Agent files above that charge Agent's clearance, or in a compartment the charge is not
read in to, is refused at filing, and the refusal names the highest classification the charge may
read; the Run files it lower. Without a charge nothing is added: the filer's own clearance is the
bound, and a person assesses.

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

The **Station charge's** `file_report` keeps its logbook, check-in and channel
([Stations](stations.md#chief-of-station)), and is also stored as a Report of its Station, kind
`station`, filed by the charge and linked to no Mission.

An Agent's filings are rate limited per Run (`per_run`) and per scope (`per_operation`)
over `window`; past either, `file_report` answers to wait.

**People.** Filing needs `mission.create` on the Operation, or `station.configure` on the Station
or on `config:reports`.

| Route | Answers |
| --- | --- |
| `POST /v1/reports` | `{"scope", "title", "text", "kind", "severity", "priority", "deadline", "information_from", "sub_source", "classification", "handling", "compartments", "tearline", "links"}`; `201` with the Report |
| `GET /v1/reports?scope=S` or `?mission=ID` | `{"reports": [...], "priorities": [...], "classifications": [...], "handlings": [...]}`: the Reports most urgent first, with `state`, `kind` and `limit` to narrow, and the deployment's lists an assessment and the markings choose from (the handling marks narrowest first) |
| `GET /v1/user/reports/unassessed[?operation_id=G]` | The open Reports of every scope the person reads (their Operations, their Stations and M's), each with its Reports page `link`, and which scopes were read, which the person may not read and which could not be read (`coverage`, `complete`); Moneypenny's `reports` tool reads it |
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
ai6ctl reports assess report-0f3c... mission --role case-officer --reason "a real fault"
ai6ctl reports assess report-0f3c... awaiting --question "Which branch?" --wait-seconds 86400 \
    --reason "cannot tell" --likelihood likely --confidence moderate
ai6ctl reports reopen report-0f3c... --reason "answered: main"
ai6ctl reports source agent:007
```

## Assessing

A `mission` assessment of a Source item may name the Agent `role` the item's first run takes (`--role`
on `ai6ctl reports assess`, `role` in the body of the route, `role` on the charge's `assess_report`).
It must be a role of the roster, and is refused with the roles there are when it is not; it is refused
with any other assessment. When the assessment names none, the role a queued brief already has stands,
then the Operation's `intake_role` ([Operations](operations.md#sources)).

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

## Knowledge

A memory, tradecraft (how a thing is done in AI6), a rule an Agent follows and a contact are Reports of
a known kind, in the same tables, read and written by the same tools and routes. They add the scopes
`person:ID` (what is known of one person) and `global` (what everyone may read), and the state
`standing`: known and current, which no verdict closes and a search lists. Observations never stand,
and are never a person's or everyone's.

| Scope | Read | Write | Decided by |
| --- | --- | --- | --- |
| `person:ID` | that person, and Moneypenny for them; another person only with `assistant.memory.admin` on `config:assistant`, audited before it is read | that person, and Moneypenny at once for them | no one: an Agent cannot propose into it |
| `operation:ID` | `mission.read` there, or a compartment read-in (a crossing, audited) | a person with `memory.write` on the Operation; an Agent at the Operation's `memory_write` clearance or above | the Operation's charge, or a person with `memory.write` there |
| `global` | every person and Agent | a person with `memory.write` on `config:memory`; an Agent never directly | M's charge (`assess_report`, audited with it as the actor), or a person with that write |

Written by someone who may write the scope, a known kind is `standing` at once. An Agent's write it may
not make is filed `open` as a proposal, readable by those who decide it and by whoever filed it, and the
verdict `accepted` makes it `standing`, or, for a change to a standing Report, applies it as that
Report's next revision and closes the proposal, linked to it (`no_action` closes it as for any Report;
every verdict is audited as `report.assess`). Every compartment of a known Report must be read in
for it to be read, as for every Report, and nobody hides a Report from themselves.

A write names the revision it read and is refused (`409`, with the revision it is at) once the Report
has moved on. Each revision it replaces is kept, up to `versions` of
[`[registry.limits.reports]`](configuration.md) (8 by default; `0` keeps none). Forgetting a `memory`
removes it with its revisions, and the audit keeps that it was forgotten and by whom, never its text.
Tradecraft, a rule or a contact is retired instead: `closed` with the resolution `retired`, kept.

A Report written by an exposed Run stays marked, and whoever reads it is told: a Run that reads one is
exposed as `report:ID`, as by a Cable. A person's own edit clears the mark, audited.

**Search** takes any of words, `kind`, `topics`, `title` and `scope`, over every scope the asker may
read, and answers each Report's ID, kind, title and summary, best first: a Report ranks by how many of
the words it holds, then how many of those are in its title, summary or topics, then its newest
change. Words match whole words, whatever their case. A Report the asker may not read is neither listed,
counted nor ranked: the filtering comes first. Without `state`, knowledge is current only when it is
`standing`, and an observation is any state.

**Built-in guides.** The build ships guides as `global` Reports of kind `tradecraft` or `rule`: how to write
a brief, how access works, how to search, what a repository is to AI6, and the guides of this site written for
Agents. Each is listed and read like any `global` Report, by whoever reads `global`, with an ID that begins
`built-in.` and its revision `1`. They are not stored, so they cannot be edited, retired or assessed, and they
change with the build. A Report a deployment stores with the same title is listed before the built-in one:
write it to replace a guide for this deployment.

```bash
ai6ctl reports file operation:ID --kind tradecraft --title "Writing a brief" \
    --summary "How a brief is written" --topics briefs,writing --text "Name the goal first."
ai6ctl reports list --words "how to write a brief" --kind tradecraft
ai6ctl reports revise REPORT-ID --revision 1 --title "Writing a brief" --text "Name the goal, then the checks."
ai6ctl reports versions REPORT-ID
ai6ctl reports forget REPORT-ID
ai6ctl reports assess PROPOSAL-ID accepted --reason "checked"
```

The routes are `GET /v1/reports?words=&topics=&title=&kind=&scope=` (search), `POST /v1/reports` with a
known `kind` (filed `201` standing, or `202` waiting for a verdict), `PUT /v1/reports/{id}` with the
`revision` read, `DELETE /v1/reports/{id}[?revision=N]` and `GET /v1/reports/{id}/versions`.

Who may do what is asked at every call, of what is granted at that moment: a removed rule or
read-in holds from the next call. The cases work the same on the file Registry, SQLite and
PostgreSQL. A write names the revision it read; titles and summaries may repeat.

### Protected by compartments

A known Report can carry [compartments](compartments.md). They narrow who can see it at all, never
widen: a reader needs the scope's read and to be read into each of them, so a `global` Report can be
everyone's in reach and need-to-know in fact. What the compartment grants or covers does not matter
here, only who is read into it (a person, group or account; an Agent, an Operation or a Mission, at the
compartment's clearance floor).

- **Hidden means hidden.** To anyone else the Report is not in a search, a listing, a count or a
  starting context, and reading, changing or forgetting it answers exactly as an ID that does not
  exist. Its summary is protected with its text, and since titles may repeat, trying to take its title
  tells nothing.
- **It fails closed.** A compartment that does not exist, or whose record cannot be read, reads nobody
  in: the Report is hidden from everyone.
- **Who sets one.** Only someone read into a compartment adds it to a Report or removes it, and nobody
  may leave a Report hidden from themselves. A change is audited `memory.compartments`. Writing a
  protected Report still needs the scope's write.

### What an Agent does with them

A Mission's Agent has `reports`, `read_report`, `file_report` and `forget_report` on its AI6 MCP
server. It reads its own Operation's knowledge and the global one, and another Operation's through a
compartment granting `memory.read`, each such read written to the audit log before it is answered. It
reads no person's memory. What it reads comes back inside `<ai6-data>`, as data. `reports` takes
`words`, `kind`, `topics`, `title` and `scope`; `file_report` takes `kind` (a known one), `line` (the
summary), `topics`, `scope`, `compartments`, and `revises` with the `revision` it read to change one;
`forget_report` takes an ID and optionally a `revision`.

Its write is a proposal that waits for a verdict, unless the Operation lets its clearance write:

```toml
[operation.G]
memory_write = "00"   # field or 00; none by default: every Agent proposes
```

It proposes to the global scope at any clearance, and to no other Operation's. Proposals wait (64 a scope
by default) for those who decide the scope, who approve (`accepted`) or reject (`no_action`) them in the
app or with `ai6ctl reports assess`. The tools work from the Run as its Station started it, so they
answer before the provider's Run ID arrives on its stream.

### Knowledge can carry an attack

An Agent that read an issue, a web page or a Cable can write a note a later Agent trusts. So a known
Report keeps what marked its writer:

- What an [exposed](medical.md) Run writes stays **marked** with how it was exposed, and so does what it
  proposes. Accepting a proposal is not an edit: the Report stays marked.
- Reading a marked Report, or seeing it in a search, marks the reader's Run exposed (`report:<id>`), as
  a Cable from an exposed sender does, and what that Run writes afterwards is marked in turn.
  Moneypenny's conversation is never exposed, but one that read a marked Report marks what she writes
  from it.
- An Agent's edit keeps the mark. A person's own edit clears it, audited `memory.mark_cleared`.

Every change is audited before it is made (`memory.write`, `memory.forget`, `memory.proposed`, and
`report.assess` for a verdict), naming the Report and the Run it came from: an Operation's under that
Operation, where its timeline shows them, the global scope's under `config:memory`, and a person's under
`config:assistant`. One that cannot be audited is not made.

### In the app

Outer Office's **Reports** page lists what is known beside the Reports: memories, tradecraft, rules and
contacts, with the `kind` and `topic` filters (and a **Knowledge** preset that keeps the four kinds), so
one place holds everything AI6 knows. The page reads each scope the person may read, the guides shipped
with the build, and what a compartment gives; what is known of a person is theirs alone, and the gateway
decides that, never the app. Opening one shows its summary, topics, revision and who wrote it, with a
warning on one an exposed Run wrote. **Edit** opens a form (title, summary, compartments, text) saved
from the revision that was opened, **Forget** asks first, and **+** adds a memory, tradecraft, a rule or a
contact to the person's own, everyone's or an Operation's. What Agents proposed waits as an open Report
with **Accept** and **Decline**. A shipped guide opens read only with **Override for this deployment**,
which files a Report of the same title in `global` to rank above it. Every Report has **Download as
Markdown**, the document of [As Markdown documents](#as-markdown-documents). Moneypenny's menu entry
**Her memory of you** opens the page on the `memory` kind.

### Limits

`[registry.limits.reports]` ([Configuration](configuration.md#limits)) holds what one scope may hold of
knowledge: `knowledge_text_bytes` (256 KiB, a known Report's text; observations keep `text_bytes`, 16 KiB), `scope_entries` (1,024) and `scope_bytes` (8 MiB) standing Reports, `proposals` waiting
(64), `versions` kept of each (8), `summary_bytes`, `topics`, and for a search `search_results` (50),
`query_bytes` (4 KiB) and `index_bytes`. Hidden Reports count toward a scope's limits, so a full scope
refuses the next one even when its writer sees room; the refusal names no count and nothing of what is
hidden.

## As Markdown documents

Every Report, of every kind, is a Markdown document: each field but the text is front matter, the text is
the body as it was written. `GET /v1/reports/{id}/document` with `Accept: text/markdown` returns it
(`ai6ctl reports show ID -o md`), and `ai6ctl reports export DIR --scope ...` writes every Report a scope
or search lists, one file each under `DIR/KIND/`.

```markdown
---
id: "report-..."
scope: "operation:G"
kind: "tradecraft"
title: "Writing a brief"
summary: "what a brief must say"
topics: ["brief"]
source: "person:operator"
filer: {"kind":"person","person":"operator"}
state: "standing"
revision: 2
created: 1763000000.0
updated: 1763000100.0
exposed: false
untrusted: false
sight: "content"
---

The text, as written.
```

The front matter holds `id`, `scope`, `kind`, `title`, `summary`, `topics`, `source` and `filer`,
`sub_source`, `from`, `links`, `severity`, `credibility` and the source's `reliability`, the markings
(`classification`, `handling`, `compartments`, `tearline`), `priority`, `deadline`, `state`, `assessment`,
`resolution`, `revision`, `updated_by`, `created`, `updated`, `exposed` (an exposed Run wrote it),
`untrusted` and `sight`; a key with nothing to say is left out. A reader who sees only the tearline gets
the tearline as the body and the markings, no title and no text; one who may not read it gets nothing
(`403` with its markings alone where the Report is an observation they may know exists, `404` for a
memory, tradecraft, rule or contact, which hides as missing). A text that opens with front matter of its
own keeps it, as text, under `author`; a document whose body was cut by `[text] document_bytes` says
`truncated: true`.

**Importing.** `POST /v1/reports/import` (a `text/markdown` body; `ai6ctl reports import FILE...`, or
`ai6ctl apply -f FILE-or-DIR` for `.md` files, `--dry-run` to check) reads that form back, as the person
importing it and under the rules of filing, revising and marking by hand:

- **No `id`**: a new Report. `scope` is required and `kind` is `finding` when left out; `title`,
  `summary`, `topics`, `severity`, `priority`, `deadline`, `from`, `sub_source`, the markings and `links`
  are as `reports file` takes them (a memory, tradecraft, rule or contact takes title, summary, topics and
  compartments only). Keys the gateway owns (`id`, `revision`, `created`, `updated`, `updated_by`, `filer`,
  `source`, `reliability`, `state`, `assessment`, `resolution`, `exposed`, `untrusted`, `sight`) are
  refused: it files the Report as the person importing it. A known Report with the title of a standing one
  in that scope and kind is refused `409` with that one's `id` and `revision`, so applying a directory of
  tradecraft twice changes nothing.
- **An `id`**: the stored Report, which must be one the person reads in full. Everything the gateway owns
  must repeat what is stored, and `revision` must be the one stored (`409` with the current one
  otherwise). A document that changes nothing answers `unchanged`. A known Report takes a new title,
  summary, topics, compartments and text as its next revision; any other field, or any change to an
  observation, is refused (its markings change at `PUT .../markings`, its verdict at `PUT
  .../assessment`). A built-in guide is read only: to override one, import a Report with its title and
  kind in `global`.
- A key that is no field of a Report, a body cut short and a front matter that does not read are `400`.

The answer is the Report as the person sees it, with `change` (`created`, `changed`, `unchanged` or
`proposed`) and `201`, `200` or `202`; `?dry_run=true` checks the document against the stored Report and
stores nothing.

## The Reports page

Outer Office's **Reports** page, under Work, is where a person reads and assesses Reports, and
reads what AI6 knows ([In the app](#in-the-app)). It is there for everyone, and lists one
row per Report across the Operations they read (`mission.read`) and the Stations (`station.read`)
and what is known, most urgent first. Its table has the app's usual sort, search and density,
and filters by Operation, state, who filed it (a Source, an Agent, a charge or a person),
classification, kind and priority; **Needs assessment**, **Assessed** and **Closed** are one click.
A view is a link (`/reports?state=open&operation=G`), and a scope the person cannot read is said in a
line above the list, so a short list is not taken for a whole one. M's own Reports are listed for a person
who holds `station.read` on `config:reports` (the profile says so under `reports`), and assessed with
`station.configure` there.

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
with its confidence. An **Awaiting an answer** assessment may also name how far the question goes, as
**May go as far as**: the deployment's handling marks up to the Report's own, since a question may not go
further than the Report may.
An assessed or closed Report has **Reopen this Report**, which asks why. Both
need `mission.control` on the Operation (`station.configure` on a Station) and are shown disabled
with that said when the person lacks it; a refusal of the gateway (permission, a handling mark, the
desk's clearance) is shown in its own words and leaves the dialog open. **Source standing** opens a
source's reliability, how it was earned and the closed Reports behind it, each of which opens.

A Mission's page has a **Reports** section: those it was begun for or resolves, and those its Agents
filed while it ran, each opening on this page. It is shown to whoever reads the Operation.

The attention bell and the Overview count unassessed Reports: Universal Exports' attention summary
(`GET /v1/user/attention`) has a Reports section that reads each scope the person may read through the
same listing the page uses, so a compartment's crossing is audited as there, and says which scopes it
read, which are not the person's to see (M's are left unmentioned unless the person holds them) and which
could not be read. Only Reports the person may assess are listed (they read the scope, hold
`mission.control` on the Operation or `station.configure` on a Station or M, and read the content), so an
item is always one they can act on; a Report held back from them is neither counted nor named. Each item
opens the Reports page on that Report. Moneypenny lists the same Reports with her `reports` tool and
offers the page to assess them; she assesses nothing herself.

An approval that lapses with no step taken under it files a `green_ink` warning Report naming the signer,
the request, the Mission and when it was approved and was valid until. It reaches the people who may assess
it (the Operation's charge, and a person with `mission.control`) as an unassessed Report, as above.

## What files them

| Producer | Kind | Filed against, linked to |
| --- | --- | --- |
| An Agent's `file_report` | its own | its Operation, linked to its Mission |
| A Source's item | `source-item` | its Operation; the filer is `source:INSTANCE` and the item. One Report per item: a changed item updates it |
| Deliverables read back (a probe) | `evidence` | the Mission's Operation, linked to the Mission; a warning when anything is not found. Filer `probe:deliverables` |
| Vetting holding a brief | `vetting-hold` | the brief's Operation, linked to the Mission it would be. Filer `probe:vetting` |
| The Chief of Staff's review holding a brief | `review-hold` | as above; the filer is the review of that Mission |
| M's scorecard proposing a hold or a preference | `scorecard` | the Operation, linked (`decision`) to its green ink request. Filer `probe:scorecard` |
| The Station's charge (Chief of Station) | the finding's | its Station |
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

Sizes, counts and rates are Registry limits, `[registry.limits.reports]` of the `registry` record of the configuration provider, applied by
the Stations and the gateway alike at the next poll, with no restart ([Configuration](configuration.md#limits-the-registry-record)):
`record_bytes`, `reports`, `text_bytes`, `name_bytes`, `links`, `page`, `per_run`,
`per_operation`, `window` and `keep_closed`.
