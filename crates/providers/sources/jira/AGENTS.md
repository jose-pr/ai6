# `ai6-source-jira` — API header

Header-file-style reference for the `ai6-source-jira` crate: the `jira` Source and Tracker
provider for AI6, so it can be used without reading its source. It ships inside the package and is
self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features), `ureq`, `url`,
`data-encoding`, `serde`, `serde_json` and `toml`. The library is both an `rlib` and the
Guatiao `cdylib` a Station loads. Unsafe code is forbidden.

## API

```rust
pub const PROVIDER_ID: &str = "jira";
pub struct Jira { /* settings, site origin, HTTP agent */ }

impl Jira {
    pub fn from_settings(settings_toml: &str) -> Result<Self, String>;
}
impl TryFrom<ai6::SourceConfig> for Jira { /* same validation; BAD_VALUE on refusal */ }
impl ai6::Source for Jira {
    fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError>;
}
impl ai6::Tracker for Jira {
    fn capabilities(&self, item: Option<String>) -> Result<TrackerCapabilities, ProviderError>;
    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError>;
    fn replies(&self, request: ReplyPoll) -> Result<ReplyBatch, ProviderError>;
    fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError>;
    fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError>;
    fn create(&self, spec: IssueSpec) -> Result<TrackerIssue, ProviderError>;
    fn link(&self, link: TrackerLink) -> Result<(), ProviderError>;
}
```

Provider ID `jira`, kinds `ai6_source` and `ai6_tracker`; one table builds both. The host
passes the instance's `sources.<name>` table (its `provider` key names this one), serialized
as TOML, in `SourceConfig::settings_toml`.

## Settings

| Key | Default | Meaning |
| --- | --- | --- |
| `site` | required | site root; `https`, or `http` on loopback only |
| `ca_file` | none | absolute path of a PEM file of certificate authorities trusted besides the public roots; a file that cannot be read or holds no certificate is refused, and a set one is reported as a weakening |
| `project` (alias `target`) | none | project key; a bare string under `sources` is this |
| `jql` | none | extra JQL condition, ANDed in parentheses; one of `project`/`jql` is required |
| `labels` | `["ai6-ready"]` | an issue needs all of them; `[]` drops the label filter |
| `deployment` | `cloud` | `cloud` (REST v3) or `server` (REST v2, Server/Data Center) |
| `auth` | `basic` | `basic` (`email:token`) or `bearer` (personal access token) |
| `email_env` | none | environment variable holding the email; required for `basic` |
| `token_env` | `JIRA_API_TOKEN` | environment variable holding the token |
| `request_timeout_secs` | 30 | seconds one request may take; 0 is refused |
| `page_size` | 50 | issues asked for per page; 0 is refused |
| `max_pages` | 5 | pages one poll reads at most; 0 is refused |
| `settling_secs` | 60 | seconds a poll searches behind the cursor; 0 searches from the cursor's minute |

| `reply_authors` | `[]` | account IDs (Cloud) or user names (Server) besides the reporter and assignee whose comments may answer |
| `issue_type` | `Task` | the type of a created issue |
| `subtask_type` | the project's first sub-task type | the type of a created child |
| `capability_ttl_secs` | 600 | how long the project's issue types and priorities are remembered |
| `link_type` | `Relates` | the issue link type that ties a child to a parent it cannot sit under |

Unknown keys are refused. `provider`, `intake` and `interval_secs` are read by the Station and
never reach the provider, nor do the Station's own keys (`status_labels`, `status_transitions`,
`agent_labels`, `agent_transitions`, `priority_map`): an unknown key here is refused.

## Behaviour

- **Poll** searches `project = "KEY" AND labels = "..." AND (jql) AND statusCategory != Done
  AND updated >= "<cursor minute>" ORDER BY updated ASC`. Cloud uses
  `GET /rest/api/3/search/jql` with `nextPageToken`; Server uses `GET /rest/api/2/search`
  with `startAt`. At most `max_pages` (5) pages of `page_size` (50). Each issue becomes a `SourceItem` whose ID and `code` are the
  issue key, with no `code_tag`; Cloud descriptions in Atlassian Document Format are flattened to plain text:
  one line per block, links and mentions as their visible text, emoji and status lozenges as
  their text, dates as a UTC `YYYY-MM-DD`; an attachment has no text and is dropped.
  The `<cursor minute>` is the cursor, `settling_secs` earlier, on its own clock; the returned
  cursor is the newest `updated`, and never earlier than the one given, so what changed in that
  margin repeats and the Registry sees it as unchanged. When the pages one poll reads hold
  nothing newer than the cursor and more remain, it searches again from the cursor's own
  minute, so a margin fuller than a poll cannot stop it.
- **Empty search (Cloud)**: Jira Cloud answers a search by a refused credential, or one
  naming a project the credential cannot see, with status 200 and no issues. So when a poll
  finds nothing, it makes one more request, `GET /project/KEY` (or `GET /myself` when only
  `jql` is set), and a 404 or 401 there is an error naming `token_env`. An unknown field or
  project inside `jql` still looks like an empty search: Jira answers that with 200 too.
- **Search lag**: a new or changed issue can take a few seconds to reach the search, so it
  can appear on a later poll.
- **Cursor** is the newest `updated` timestamp. JQL compares at minute precision in the
  account's time zone, so the next search starts at that minute (`updated >= "YYYY-MM-DD
  HH:MM"`) and the newest issue repeats; the Registry sees it as unchanged.
- **Items**: an item ID is an issue key in the instance's `project`; any other is refused
  (`GUATIAO_ERR_BAD_VALUE`, message `foreign item ...`) before a request.
- **Capabilities**: comment, edit, free-text labels (`free_labels`), assign, close, issue
  types and priorities from `GET /issue/createmeta/{project}/issuetypes` and `GET /priority`,
  `children = parent` on Cloud and `subtasks` on Server (`none` when the project has no
  sub-task type), `issue_links` and `remote_links`, `visibility = unknown`, markers, and
  the transitions open on the item asked about. Project answers are cached
  `capability_ttl_secs`.
- **Post** comments (ADF on Cloud, plain text on Server) or edits
  (`PUT /issue/{key}/comment/{id}`). Cloud text goes through the Markdown subset headings,
  bullet and numbered lists, fenced code blocks and `http(s)` links; raw HTML stays literal.
  A blank body is an error before any request, since Jira refuses it. Every new comment carries
  the comment property `dev.coqui.ai6` (`{"marker": NAME}`), which an edit leaves in place.
- **Update**: labels by `update.labels` add/remove (whitespace becomes `-`); one assignee
  (`accountId` on Cloud, `name` on Server; clearing sends null); a `transition` by name
  (case-insensitive) or ID, or a `state`: `closed` takes a transition into a `done`-category
  status, `open` one into `new` or `indeterminate`. An unmatched transition is
  `GUATIAO_ERR_UNSUPPORTED` naming the ones that are open.
- **Create** posts `fields` with the project, the first line of the title cut to 255 bytes, the
  body when there is one, the type, labels, a priority only when the project has it. A child on
  Cloud takes the instance's `issue_type` under an epic and `subtask_type` (or the first sub-task
  type) under a standard issue; under a sub-task, or with no sub-task type, it is created beside
  its parent and tied by `link_type`, and comes back with no parent. On Server a child needs a
  sub-task type. It returns the issue read back. `link` posts `POST /issueLink` (`link_type`) for
  an issue in the project, or `POST /issue/{key}/remotelink` for an `http(s)` address.
- **Replies** list each issue's comments (`GET /issue/{key}/comment`), keep those updated
  after the cursor (newest `updated` becomes it), and flatten ADF to text. `permitted`: the
  author is the issue's reporter or assignee, or is in `reply_authors`. `own`: the author is
  the credential's account (`GET /myself`); a call that cannot name it fails, since its comments
  carry no marker. Timestamps are ordered by the instant they name, not their text: each carries
  the account's offset.
- **Capabilities on a refused credential**: an error naming `token_env`, not a project that
  cannot create: when create metadata cannot be read, the project lookup decides.
- **Credentials** are read from the named variables on each call; a missing one is an
  error that names the variable. Cloud answers a refused credential on an issue with 404,
  so that error names `token_env` too. Errors never carry the credential or a response body.
- **Wrong `deployment`**: Cloud has removed REST v2 search and answers 410; the error says
  to check `deployment`.
- **Rate limits**: a 429 is an error naming `Retry-After`; the cursor does not move.
- Every request has a `request_timeout_secs` (30 s) timeout and follows no redirects.
