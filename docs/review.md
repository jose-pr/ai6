# Review

A **review policy** says who decides that a Mission's plan or its output is good enough, so it is
not one model's word. It names the Agents or roles on the panel, what each vote weighs, how much of
the weight must agree in how many rounds, and how many times the author is sent back before a
person decides.

## The record

```toml
# reviews.toml, or the `review` family of a SQL provider
[review.two-models]
panel = [
  { role = "reviewer", weight = 2, veto = true },   # an Agent or a role
  { agent = "Tanner", weight = 1 },
]
pass_at = 0.66              # share of the weight that must pass; default: all of it
distinct_models = 2         # reviewers on different models, none the author's; default 1
rounds = 2                  # rounds to reach agreement before on_no_agreement; default 1
redo_limit = 2              # times the author is sent back; default 1
on_no_agreement = "person"  # or "refuse"
max_turns = 12              # reviewer turns one review may spend; default no limit
max_tokens = 200000         # tokens one review may spend; default no limit

[operation.G.review]        # in operations.toml
plan = "none"               # a policy's name, or "none", the default
output = "two-models"

[role.case-officer.review]  # in agents.toml: comes before the Operation's
output = "two-models"
```

A policy, a seat that names an Agent or role the global roster does not have, or a choice that
names a policy that does not exist, makes the configuration invalid. A review reads only the
global roster: a checkout's `.ai6/` files have no say in what reviews a Mission.

## How votes decide

A vote is `pass`, `redo` with reasons, or `abstain`.

- **The share.** The weight voting `pass` over the weight counted must reach `pass_at`. An
  abstention leaves its weight out of what is counted.
- **A veto.** A seat with `veto` voting `redo` ends the round as a redo, whatever the others said.
- **Absent.** A reviewer that fails, times out or gives an answer that cannot be read is shown as
  absent. Its weight stays in what is counted, so it can make the share unreachable: nobody passes
  a review by silencing a reviewer.
- **Models.** A pass needs `distinct_models` different models among the reviewers who judged. Agents
  that share a model name on any route count as one model, and an Agent that names no model counts
  as its handler's default.
- **Rounds.** In round one each reviews alone. If neither `pass` nor `redo` reached the share, the
  next round gives each the others' reasons, up to `rounds`; past that it is `on_no_agreement`.
- **Sent back.** A redo goes to the author's Session with the reasons, up to `redo_limit`; past
  that a person decides.

## Who chooses the policy

The Mission's, then the role's, then the Operation's. A brief names a policy per gate in its `data`:
`{ "review": { "output": "two-models" } }`.

| Who wrote the brief | Naming a stricter policy | Naming a weaker one, or `none` |
| --- | --- | --- |
| A person | applies | applies, audited |
| An Agent with `00` clearance, by a proposal | applies | applies to another Agent's Mission, audited; never its own |
| Anything from outside AI6: a tracker's item or label, another Agent's proposal, a dropped brief, a Sleeper | applies | ignored, and the timeline says so |

A policy is stricter when everything it would pass, the other would too: the same seats and
weights, each veto kept, a share and model count no lower, no more rounds or redos, a budget no
larger. A seat added is stricter only when all of the weight must pass, since otherwise a heavy new
seat could carry the share alone. Text from outside naming a policy that does not exist is ignored,
so a label cannot stop a Mission; a person's or a configuration's stops it, with the reason shown.

## The panel

Each seat gets an Agent: the one it names, or one qualified for the role it names. The author is
never on its own panel, nor is any Agent that shares a model with the author. Nobody sits twice.
Agents that are quarantined, retired, held or have no handler are passed over. A panel that cannot
be filled, or cannot reach `distinct_models`, makes the Mission wait with the reason shown: it is
never reviewed by fewer.

## The output gate

Where a Mission would be `done`, it waits as `blocked`, reason `review-output`. Its Session is
closed and its safe house kept. Nothing that says it is done moves: not a CX report, a status
comment, a linked issue's state or a Source's report. Only a pass does.

Each reviewer's turn is an answer-only Session of its own, on the Station's own host, with no tool
that changes anything, in an empty directory, started as a [Chief of Staff review](missions.md#vetting-and-held-briefs)
is. It gets the goals, the Agent's own report (which is where any test results are: AI6 runs no
tests of its own), the branch, its commits and the change, each cut to the Station's
[`[review]` limits](configuration.md), a cut one saying so.

Everything the author wrote, and any other reviewer's reasons in a later round, is quoted between
markers no line of it contains. Whatever it says, it reaches a decision only through a reviewer's
vote, and a vote is read only from that reviewer's own final message: one JSON object at its end.
An answer that is not plainly a pass never passes.

| Outcome | What happens |
| --- | --- |
| Pass | The cover is audited on what is there now, the safe house finished, the Mission `done`, its reports sent |
| Redo | The author's Session is resumed with the reasons, also quoted, up to `redo_limit`; past it `blocked`, reason `review-redo`, for a person |
| No agreement, or the budget runs out | `on_no_agreement`: `blocked` for a person, or the Mission fails |
| No panel can be filled, or no policy read | `blocked`, the reason on the review, until it can |

Votes and where the review stands are in the Registry, one record per Mission and gate. A Station
that restarts mid-review carries on from it: a seat has one vote a round, so a reviewer started
again is not counted twice. A reviewer's lost harness is resumed when it named a Session and started
again when it did not. Every vote, weight, reason, model and round is on the Operation's timeline
(kind `review`) and in Audit, and so is a weakening or a naming that was ignored.

`transition` to `done` is refused while a Mission waits for its review. An operator's `resume` starts
the author again, and what it then says is done is reviewed again.

## The plan gate

Under a `plan` policy, a Mission's Agent first works at clearance `read`, which only AI6 sets: it can
read and search its working directory, and its only AI6 tool is `submit_plan`. It cannot edit, run a
command, send a Cable, write to an issue or propose a Mission; the checkout's own settings are not
loaded. It submits its plan (or gives it as its final message) and ends its turn; the Mission waits as
`blocked`, reason `review-plan`, and the panel judges the plan against the goals, as above. A redo
resumes it, still at `read`, with the reasons. A pass resumes the same Session at the Mission's own
clearance, with the Mission's tools.

That needs a harness that takes up new tools on a resume of the same Session. A handler says whether
it can, and a Mission under a plan gate does not start on one that cannot; it waits, with the reason
on the Mission:

| Handler | Under a plan gate |
| --- | --- |
| `claude` | reads and searches (`Read`, `Glob`, `Grep`) and calls `submit_plan`; everything else is denied by name. A resume passes its own tool flags |
| `codex` | holds the thread to `desk` with no file change at all: a patch ends it. It has no command tool there, so it reads only what its prompt and AI6's tools give it |
| `antigravity`, `acp` | refuses: a resumed conversation keeps the tools of its first launch, or AI6 does not choose the agent's tools |

A Mission in remote-hands mode does not start under a plan gate: the Theater's tools can change it
whatever the handler offers.

## People

The Mission page in Outer Office shows each gate's review: the policy, where it stands and why, the
panel with each seat's weight and veto, and every vote with its round and reasons. A person with
`mission.review` decides over the panel with a reason: pass it, send it back (the reason goes to the
author and counts as a send-back) or refuse it. The decision is audited and on the timeline, and the
Station holding the Mission acts on it at its next round. The same through `ai6ctl`:

```bash
ai6ctl reviews show --operation G G-12
ai6ctl reviews overrule --operation G G-12 --gate output --decision pass --reason "checked by hand"
```

A review policy is a record like any other: Outer Office's roster page edits review policies with a
form drawn from the record's declaration, and an Operation's or a role's form picks one per gate. A
Mission created through Universal Exports can name one (`review: {output: "..."}`), which needs
`mission.review`, since a person's naming may weaken it.


<!-- schema:record.review:begin -->
| Key | Default | Meaning |
| --- | --- | --- |
| `panel` | none | **Panel.** Who reviews: Agents or roles, each with a weight and an optional veto. |
| `pass_at` | 1 | **Share that must pass.** The share of the weight, above 0 and at most 1, that must vote to pass. Abstentions are left out of the share. Leave it at 1 for all of it. |
| `distinct_models` | 1 | **Different models.** How many different models must be behind the votes, none of them the author's. |
| `rounds` | 1 | **Rounds to agree.** How many rounds the panel has to agree before `on_no_agreement`. |
| `redo_limit` | 1 | **Times sent back.** How many times the author is sent back before a person decides. |
| `on_no_agreement` | `person` | **When the panel does not agree.** What happens when the panel does not agree, or the budget runs out. |
| `max_turns` | none | **Turns one review may spend.** Reviewer turns one review may spend. Leave it empty for no limit of its own. |
| `max_tokens` | none | **Tokens one review may spend.** Tokens one review may spend. Leave it empty for no limit of its own. |
| `reviewer_timeout_secs` | 3,600 | **A reviewer answers within.** How long one reviewer has to answer before it counts as absent; 0 is no limit. |
<!-- schema:record.review:end -->
