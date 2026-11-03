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

## Settings

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
