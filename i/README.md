# coagent-i

**I Branch** of [C.O.A.G.E.N.T.](https://github.com/jose-pr/coagent): intelligence and
communications. It turns whatever you track work in into **mission briefs M can act on,
without coagent knowing your tracker.**

coagent ships no Jira, GitHub or webhook integration. You supply those, as a program in
any language, a Python package, a skill or an overlay, and I gives them three ways in:

| Way in | How |
| --- | --- |
| **Drop a brief** | write a JSON file into the inbox, or `python -m coagent_i submit brief.json` |
| **Be polled** | a `[[source]]` in `sources.toml` whose command prints briefs as JSON |
| **Be a package** | a Python object in the `coagent.intake` entry-point group |

A source can also take **reports**: how each brief ended, to comment on its ticket or
move it.

```json
{
  "id": "GUA-12",
  "source": "jira",
  "title": "Fix the flaky watchdog test",
  "body": "It fails one run in ten on macOS.",
  "station": "G",
  "role": "implement",
  "tags": ["ci"],
  "priority": 5,
  "url": "https://example.atlassian.net/browse/GUA-12",
  "data": {"anything": "the source wants back"}
}
```

```toml
# ~/.config/coagent/sources.toml
[[source]]
name = "jira"
command = ["jira-briefs", "--jql", "project = GUA AND labels = agent"]
report = ["jira-report"]
station = "G"
```

```bash
python -m coagent_i poll                  # run every source once
python -m coagent_i list                  # what waits, best first
python -m coagent_m.watchdog launch --next   # M takes the best one
```

The contract is in
[`src/coagent_i/AGENTS.md`](https://github.com/jose-pr/coagent/blob/main/i/src/coagent_i/AGENTS.md).
Planned here as well: the wire format M, Moneypenny and agents talk in, briefings (the
context an agent starts with) and debriefs.

## License

MPL-2.0.
