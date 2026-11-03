# `ai6-source-gitlab` — API header

Header-file-style reference for the `ai6-source-gitlab` crate: the `gitlab` Source and Tracker
provider for AI6, so it can be used without reading its source. It ships inside the package and is
self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features), `ureq`,
`url`, `serde`, `serde_json` and `toml`. The library is both an `rlib` and the Guatiao
`cdylib` a Station loads. Unsafe code is forbidden.

## API

```rust
pub const PROVIDER_ID: &str = "gitlab";
pub struct Gitlab { /* settings, API origin, HTTP agent */ }

impl Gitlab {
    pub fn from_settings(settings_toml: &str) -> Result<Self, String>;
}
impl TryFrom<ai6::SourceConfig> for Gitlab { /* same validation; BAD_VALUE on refusal */ }
impl ai6::Source for Gitlab {
    fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError>;
}
impl ai6::Tracker for Gitlab {
    fn capabilities(&self, item: Option<String>) -> Result<TrackerCapabilities, ProviderError>;
    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError>;
    fn replies(&self, request: ReplyPoll) -> Result<ReplyBatch, ProviderError>;
    fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError>;
    fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError>;
    fn create(&self, spec: IssueSpec) -> Result<TrackerIssue, ProviderError>;
    fn link(&self, link: TrackerLink) -> Result<(), ProviderError>;
}
```

Provider ID `gitlab`, kinds `ai6_source` and `ai6_tracker`; one table builds both. The host
passes the instance's `sources.<name>` table (its `provider` key names this one), serialized
as TOML, in `SourceConfig::settings_toml`.

## Settings

| Key | Default | Meaning |
| --- | --- | --- |
| `project` (alias `target`) | required | `group/name` or `group/subgroup/name`; a bare string under `sources` is this |
| `labels` | `["ai6:ready"]` | an issue needs all of them; `[]` takes every open issue |
| `assignee` | none | only issues assigned to this username |
| `token_env` | `GITLAB_TOKEN` | environment variable holding the token |
| `api_url` | `https://gitlab.com/api/v4` | REST root; `https`, or `http` on loopback only |
| `ca_file` | none | absolute path of a PEM file of certificate authorities trusted besides the public roots; a file that cannot be read or holds no certificate is refused, and a set one is reported as a weakening |
| `request_timeout_secs` | 30 | seconds one request may take; 0 is refused |
| `page_size` | 100 | issues, comments or labels asked for per page; 0 is refused |
| `max_pages` | 5 | pages one listing reads at most; 0 is refused |
| `settling_secs` | 120 | seconds a poll searches behind the cursor; 0 searches from the cursor |
| `reply_authors` | `[]` | usernames besides the issue's author and the members below whose comments may answer a question |
| `reply_min_access` | 30 | the least project access level that may answer: 10 Guest, 15 Planner, 20 Reporter, 30 Developer, 40 Maintainer, 50 Owner |
| `capability_ttl_secs` | 600 | how long the project's labels and settings are remembered |

Unknown keys are refused. `provider`, `intake` and `interval_secs` are read by the Station and
never reach the provider, nor do the Station's own keys (`status_labels`, `status_transitions`,
`agent_labels`, `agent_transitions`, `priority_map`): an unknown key here is refused.

## Behaviour

- **Requests** go to `{api_url}/projects/{project, URL-encoded}/...` with the token in
  `PRIVATE-TOKEN`. GraphQL, used only for tasks under an issue, is the API root's sibling:
  `.../api/v4` answers it at `.../api/graphql`.
- **Poll** lists `GET .../issues` (`state=opened`, `order_by=updated_at`, `sort=asc`,
  `labels` joined by commas, `assignee_username`, `page_size` (100) per page, at most
  `max_pages` (5) pages) and maps each issue to a `SourceItem` with ID `group/name#iid`, `code`
  the `iid` and `code_tag` `gl`. The returned cursor is the newest `updated_at`, and never
  earlier than the one it was given; the next poll sends it, `settling_secs` earlier, as
  `updated_after`. When the pages one poll reads hold nothing newer than the cursor and more
  remain, it searches again from the cursor itself, so a margin fuller than a poll cannot stop
  it. A `Link: rel="next"` that leaves the API origin is not followed, so the token is never
  sent elsewhere.
- **Items**: an item ID is `group/name#iid` naming this instance's project (compared without
  case); any other is refused (`GUATIAO_ERR_BAD_VALUE`, message `foreign item ...`) before a
  request. An item's `url` is GitLab's own `web_url`.
- **Capabilities**: comment, edit, labels (the project's own, group labels included), assign,
  close, create (unless the project has issues off), replies, markers, `links = ["issue_links"]`,
  `children = subtasks`, no transitions, no issue types or priorities, `visibility` `private` or
  `public` from the project (`internal` is `unknown`). With an item, `children` is `none` unless
  the item is an issue: a task takes no children. Project answers are cached
  `capability_ttl_secs`.
- **Post** comments (`POST .../issues/{iid}/notes`) or edits (`PUT .../notes/{id}`; the ID is
  digits only). Every post ends with an HTML comment `<!-- ai6:NAME -->` (`NAME` the `marker`,
  `comment` without one), which GitLab keeps in `body` and which alone makes a reply `own`. A
  blank body is an error before any request, since GitLab refuses it. The comment's address is
  built from the API root less `/api/v4`: `{web}/{project}/-/issues/{iid}#note_{id}`, `None`
  when the root does not end so.
- **Update** is one `PUT .../issues/{iid}`: `add_labels` and `remove_labels` as comma-joined
  lists (a label the project lacks is created; one the issue lacks is ignored), `state` `open`
  or `closed` as `state_event` `reopen` or `close`, and assignees as the whole new
  `assignee_ids` list, read from the issue and changed by username; an added username is looked up
  among the project's members (`members/all?query=`, matched exactly, without case) and one not
  found is `GUATIAO_ERR_BAD_VALUE` before any write. A `transition` is
  `GUATIAO_ERR_UNSUPPORTED`; another `state` is `GUATIAO_ERR_BAD_VALUE`; both before a request.
- **Create** keeps only the labels the project has (GitLab would create the rest); `issue_type`
  and `priority` are ignored. With a `parent` that is an issue, the new one is a task
  (`issue_type=task`) attached under it with GraphQL `workItemUpdate` (`hierarchyWidget`); when
  that fails the task stays created and comes back with no parent. A parent that is a task, or
  cannot be read, gets a plain issue with no parent.
- **Issue** reads `GET .../issues/{iid}`; `state` `opened` reads `open`; `assignees` and
  `author_id` are usernames. A task's `parent` comes from GraphQL (`workItems(iid:)`, the
  hierarchy widget) as `{namespace fullPath}#{iid}`; an issue has none.
- **Link** makes a `relates_to` link (`POST .../issues/{iid}/links`) for every relation, to an
  issue of this project; already linked (409) is not an error. A link to an address alone is
  `GUATIAO_ERR_UNSUPPORTED`; one with neither is `GUATIAO_ERR_BAD_VALUE`.
- **Replies** read each issue's notes newest first (`order_by=updated_at`, `sort=desc`), stopping
  at the page that reaches `since` (inclusive), up to `max_pages` pages per issue, and return them
  oldest first. GitLab's own notes (`system: true`) are skipped but move the cursor, the newest
  `updated_at` read. `permitted`: the username is in `reply_authors`, is the issue's author, or
  is a project member (`members/all/{user id}`, inherited membership included) at
  `reply_min_access` or above; each lookup is made once per call. `own`: the comment carries the
  provider's marker, whoever wrote it.
- **Token**: read from `token_env` on each call; a missing token is an error for every call
  but a read (`GET`), and a private project answers a read with no token with 404, so that
  error names the unset variable. Errors name the variable, never the value or a response body.
- **Errors**: 401 names the token's variable; 403 says the token may not do this (a paid feature
  answers it too); 404 that the project or issue is not shown; 400 and 422 that GitLab rejected
  the request.
- **Rate limits**: a 429 is an error naming `retry-after`; the cursor does not move and the
  Station polls again at its next interval. A page answered with `ratelimit-remaining: 0` is the
  last one read.
- Every request has a `request_timeout_secs` (30 s) timeout and follows no redirects.
- Merge requests are not a Source.

## Gotchas (measured on gitlab.com, November 2026)

- **`updated_at` stands still for a change within about a minute of the last one.** A label,
  assignee or comment change 5 to 47 s after the issue's last change left `updated_at` where it
  was; 65 s or more after, it moved; the first change after creation moved it. An issue labelled
  for intake soon after another change keeps the older time, so `settling_secs` defaults to
  120, past that minute.
- `updated_after` drops the fraction of a second (`...13.002Z` is sent on as `...13+00:00`) and
  is inclusive, so the newest item repeats on the next poll; the Registry sees it unchanged.
- `web_url` is `.../-/work_items/{iid}`, not `/-/issues/`, for issues too.
- A label change makes no note (GitLab keeps label events apart); an assignee change makes a
  `system` note.
- The notes listing has no time filter: hence newest first, to the cursor.
- `blocks` and `is_blocked_by` links answer 403 ("not available for current license") on the
  free tier; `relates_to` works. An issue cannot be a child of an issue; a task can.
- GraphQL answers a refused mutation with 200 and the reason in `data.workItemUpdate.errors`.
- `Link` headers repeat the query in GitLab's own form (`labels[]=`, `updated_after=...+00:00`);
  `x-next-page` and `x-total` come too.
- The free plan's burst limit is 100 requests a minute per user (`ratelimit-name:
  burst_traffic_per_user_free_plan`); every answer carries `ratelimit-limit`,
  `ratelimit-remaining` and `ratelimit-reset` (epoch seconds). A 429 was not recorded.
