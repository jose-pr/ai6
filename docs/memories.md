# Memories

AI6 remembers in rows of the Registry: what [Moneypenny](assistant.md#her-memory) keeps of each
person, and what an Operation's Missions and everyone should know. Every entry has a scope:

| Scope | What it is | Read by | Changed by |
| --- | --- | --- | --- |
| `person:<id>` | What Moneypenny remembers of that person | That person, through her or the app; an administrator with `assistant.memory.admin`, audited | That person, and she at once for them |
| `operation:<id>` | The Operation's knowledge base | Anyone with `mission.read` on the Operation, and any Agent working in it; through a compartment granting `memory.read`, audited as a crossing | A person with `memory.write` on the Operation; an Agent only as a proposal, unless the Operation lets its clearance write |
| `global` | What everyone shares | Every person and every Agent | A person with `memory.write` on `config:memory`; an Agent only as a proposal |

Who may do what is asked at every call, of what is granted at that moment: a removed rule or
read-in holds from the next call. The tables, limits and searches work the same on the file
Registry, SQLite and PostgreSQL.

An entry is an ID (a generated UUID, set once), an optional name, a one-line summary, its content,
when it was made and last changed and by whom, its **source** (who wrote it, from which Session,
and whether that Session was exposed), its compartments, and its revision. Names and summaries may
repeat. A change names the revision it was read at and is refused once the entry has moved on;
prior versions are kept (8 by default) and go with a forget.

## Search

A search looks for the words of a query in an entry's name, summary and content, as words, not
parts of words. An entry holding one of them matches; those holding more of them rank first, then
those holding them in the name or summary, then the newest. It filters what the asker may not see
before it ranks or counts, so a total, an order or a score tells nothing of a hidden entry. The
database's own text index (FTS5 on SQLite, `tsvector` on PostgreSQL; a scan on the file Registry)
only finds the candidates.

## Protected by compartments

An entry can carry [compartments](compartments.md). They narrow who can see it at all, never
widen: a reader needs the scope's read and to be read into one of them, so a `global` entry can be
everyone's in reach and need-to-know in fact. What the compartment grants or covers does not
matter here, only who is read into it (a person, group or account; an Agent, an Operation or a
Mission, at the compartment's clearance floor).

- **Hidden means hidden.** To anyone else the entry is not in a search, a listing, a count or a
  starting context, and reading, changing or forgetting it answers exactly as an ID that does not
  exist. Its summary is protected with its content, and since names may repeat, trying to take its
  name tells nothing.
- **It fails closed.** A compartment that does not exist, or whose record cannot be read, reads
  nobody in: the entry is hidden from everyone but a chief.
- **Who sets one.** Only someone read into a compartment adds it to an entry or removes it, and
  nobody may leave an entry hidden from themselves. A change is audited `memory.compartments`.
  Writing a protected entry still needs the scope's write.

## Agents

A Mission's Agent has `memory_search`, `memory_read`, `memory_write` and `memory_forget` on its AI6
MCP server. It reads its own Operation's memory and the global one, and another Operation's
through a compartment granting `memory.read`, each such read written to the audit log before it is
answered. It reads no person's memory. What it reads comes back inside `<ai6-data>`, as data.

Its write is a **proposal** a person decides, unless the Operation lets its clearance write:

```toml
[operation.G]
memory_write = "00"   # field or 00; none by default: every Agent proposes
```

It proposes to the global memory at any clearance, and to no other Operation's. A proposal waits
(64 a scope by default) for a person who may change the scope and may see the entry; they approve
or reject it in the app or with `ai6ctl memories decide`. The tools work from the Session as its
Station started it, so they answer before the provider's Session ID arrives on its stream.

## A memory can carry an attack

An Agent that read an issue, a web page or a Cable can write a note a later Agent trusts. So an
entry keeps its source:

- What an [exposed](medical.md) Session writes stays **marked** with how it was exposed, and so
  does what it proposes. Approving a proposal is not an edit: the entry stays marked.
- Reading a marked entry, or seeing it in a search, marks the reader's Session exposed
  (`memory:<id>`), as a Cable from an exposed sender does, and what that Session writes afterwards
  is marked in turn. Moneypenny's conversation is never exposed, but one that read a marked entry
  marks what she writes from it.
- An Agent's edit keeps the mark. A person's own edit clears it, audited `memory.mark_cleared`.

## In the app

Outer Office's **Memory** page (`app:/memory`, `?scope=global`, `?operation_id=G`) shows one memory
at a time, chosen at its top: what Moneypenny keeps of you, everyone's, or an Operation's you may
read; an Operation's own page links to its memory. It lists the entries, newest change first, with
who wrote each, any compartment protecting it, and a warning on one an exposed Session wrote;
searches them; and opens one in a form (summary, name, compartments, content) saved from the
revision that was opened, or forgotten once you confirm. Where you may change that memory, what
Agents proposed for it waits at the top, with who asked and their mark, to approve or reject.

## Routes

`/v1/user/memories` serves every scope; `/v1/user/assistant/memory` is a person's own
([Moneypenny](assistant.md#your-memory-in-the-app)).

| Route | Does |
| --- | --- |
| `GET /v1/user/memories[?scope=SCOPE][&q=TEXT]` | One scope, newest change first; with `q`, or across every scope you may read, a search, best first: `{"entries", "total"}`, with no content |
| `POST /v1/user/memories` | Adds an entry: `{"scope", "name"?, "summary", "content", "compartments"?}` |
| `GET /v1/user/memories/{id}` | One entry with its content |
| `PUT /v1/user/memories/{id}` | Changes one: `{"name"?, "summary", "content", "revision", "compartments"?}`; compartments left out stay |
| `DELETE /v1/user/memories/{id}[?revision=N]` | Forgets one, with its prior versions |
| `GET /v1/user/memories/{id}/versions` | What it held before, newest first |
| `GET /v1/user/memories/proposals[?scope=SCOPE]` | What Agents proposed that you may decide, oldest first |
| `POST /v1/user/memories/proposals/{id}` | `{"approve": true}` carries it out, `false` drops it |

An entry you may not see answers `404`, the same as an ID that does not exist. Every change is
audited before it is made (`memory.write`, `memory.forget`, `memory.proposed`,
`memory.proposal.approved`, `memory.proposal.rejected`), naming the entry and the Session it came
from: an Operation's under that Operation, where its timeline shows them, the global memory's under
`config:memory`, and a person's under `config:assistant`. One that cannot be audited is not made.
`ai6ctl memories` does the same from a shell.

## Limits

`[registry.limits.memories]` ([Configuration](configuration.md#limits)): 256 KiB an entry, 1,024
entries and 8 MiB a scope, 8 prior versions, 50 entries a search, 4 KiB a query, 64 proposals
waiting a scope. Hidden entries count toward a scope's limits, so a full scope refuses the next
entry even when its writer sees room; the refusal names no count and nothing of what is hidden.
