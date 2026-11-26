# Documents

A brief, a green ink request, a held brief, a Mission's reviews, a charge's report (a Station's is
its charge's) and a [Report](reports.md) are texts people read, so the gateway serves each as a
**document**: Markdown with frontmatter the gateway writes, and the references in it resolved for
the person asking. Outer Office renders them; `Accept: text/markdown` gives the Markdown alone, for
a download or an agent.

## The shape

```json
{
  "markdown": "---\nkind: \"brief\"\n...\n---\n\nFix OPS-12 ...",
  "references": [
    {"start": 412, "end": 418, "text": "OPS-12", "kind": "issue",
     "url": "https://tracker.example/browse/OPS-12"}
  ],
  "trackers": ["tracker.example"],
  "omitted_references": 0
}
```

- `markdown` opens with a frontmatter block between `---` lines that the gateway writes from the
  record: identifiers, who and where, times, state. It never holds a secret or an environment
  variable's value (a brief lists the variables' names). The body follows: what the record's author
  wrote.
- A body that opens with frontmatter of its own keeps it, as text, in the gateway's block under the
  key `author` (`author: |2` and the lines indented). It is the author's words and nothing reads it
  as keys, so the gateway's keys are the only ones to rely on.
- `references` are spans of the **body**, as character (not byte) offsets into `markdown`, in
  order. `kind` is `issue`, `mission`, `operation` or `run`. Text inside code (fenced or inline) and
  inside an address (`scheme://...`, `app:/...`) is never a reference.
- `trackers` are the hosts of the record's Operation's Sources: the hosts the gateway vouches for.
  A web address in the body that is not on one of them is the author's.
- `omitted_references` counts the references past `document_references`, which stay text; it is
  absent when none were. A body past `document_bytes` is cut and the frontmatter says
  `truncated: true`.

## Routes

Each answers the JSON above, or the Markdown alone to `Accept: text/markdown` (named before JSON or
a wildcard), with `Vary: Accept`.

| Route | Needs | Frontmatter `kind` and keys |
| --- | --- | --- |
| `GET /v1/user/operations/{op}/missions/{mission}/brief` | `mission.read` | `brief`: `operation`, `mission`, `handle`, `title`, `state`, `source`, `created`, `url`, `stations`, `name`, `agent`, `role`, `station`, `theater`, `mode`, `handler`, `terminal`, `working_directory`, `model`, `effort`, `environment_names`, `origin` (who asked, from the brief's provenance). The body is the exact instructions the Agent got |
| `GET /v1/user/operations/{op}/green-ink/{id}/document` | `approval.decide` | `green_ink`: `operation`, `id`, `state` (pending, approved, rejected, expired), `mission`, `run`, `agent`, `step`, `target`, `requested_at`, `expires_at`, `proposed_role`, `rearmed`, `links` (label and address), and once decided `signer`, `decided_at`, `decision_expires_at`, `notified_at`, `spent_at`, `revoked`. The body is the Agent's reason, then the signer's note under a heading |
| `GET /v1/user/operations/{op}/held-briefs/{key}/document` | `mission.create` | `held_brief`: `operation`, `key`, `mission`, `handle`, `title`, `name`, `source`, `agent`, `role`, `url`, `stations`, `priority`, `created`, `held_at`, `verdict`, `findings` (kind, detail, related), `origin`. The body is the brief's text |
| `GET /v1/user/operations/{op}/missions/{mission}/reviews/document` | `mission.read` | `review`: `operation`, `mission`, `reviews` (each gate's policy, status, iteration, round, panel, turns, tokens and overrule). The body is a section per gate: the votes, and each reviewer's reasons as block quotes |
| `GET /v1/user/charges/{scope}/document` | `charge.converse` | `charge`: `scope`, `agent`, `route`, `configured`, `station`, `held`, `lease_expires`, `last_wake`, `last_ended`, `turns_last_hour`. The body is what its last turn said, then its logbook |
| `GET /v1/reports/{id}/document` | reading the Report's scope, as the Report | The Report's fields as `GET /v1/reports/{id}` gives them, in the order [Reports](reports.md#as-markdown-documents) lists, `kind` being the Report's own (`finding`, `tradecraft`, ...) and `sight` saying how much the reader sees. The body is its `text` |

A record the person may not read answers as its own route does (`403`, or `404` for another
Operation's or an unknown one).

## References

What resolves, for the person reading:

- **Issue keys** of the record's Operation: `KEY-123` for a Jira Source whose `project` is `KEY`,
  `owner/repo#123` for a GitHub Source of that repository and `group/project#123` for a GitLab one,
  become the tracker's address (`url`). A key no Source of the Operation owns stays text.
- **Operation IDs, Mission UIDs and handles, and Run UIDs** become an `app:` route (`route`, in
  the form Outer Office opens) only where the reader holds `mission.read` on that Operation. For
  an Operation other than the record's, that is asked of the access policy and then of the
  [compartments](compartments.md); a compartment's grant is written to the audit log before it
  is allowed. Otherwise the words stay text and nothing about them is said.
- A **Mission or Operation reference** also carries what it leads to besides its page:
  `issues`, the Mission's linked issues (`issue` as `TRACKER:ITEM`, `title`, `state`, `url`), and
  `boards`, the board each of the Operation's Sources names (`tracker`, `provider`, `url`).

### Boards

A Source provider answers `board_url`: the page where people read its instance's issues as a board,
worked out from its own settings with no request to the tracker, or none. The gateway holds no
provider, so a document with a Mission or Operation reference asks the Headquarters Station once per
Operation (`SourceRead` with `boards`), waiting at most `document_board_wait` (5 seconds). With no
Headquarters Station, or none answering in time, the references carry no `boards`.

| Provider | The board | Setting that names another |
| --- | --- | --- |
| `github` | the repository's issues page, on GitHub's own site by default or on the host of a GitHub Enterprise `api_url` ending `/api/v3` | `board_url`, `https` |
| `gitlab` | the project's issue boards, `.../-/boards` | `board_url`, `https` |
| `jira` | Cloud: the project's boards page; Server: the project page; none without a `project` | `board_url`, `https` |

## Settings

`document_bytes` (1 MiB), `document_references` (256) and `document_board_wait` (5 seconds) are in the `[text]` table of the `unex`
record ([Configuration](configuration.md)); `"none"` takes either away.
