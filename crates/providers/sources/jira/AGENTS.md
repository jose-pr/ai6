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

| `reply_authors` | `[]` | account IDs (Cloud) or user names (Server) besides the reporter and assignee whose comments may answer |
| `issue_type` | `Task` | the type of a created issue |
| `subtask_type` | the project's first sub-task type | the type of a created child |
| `capability_ttl_secs` | 600 | how long the project's issue types and priorities are remembered |

Unknown keys are refused. `provider`, `intake` and `interval_secs` are read by the Station and
never reach the provider. `status_labels`, `status_transitions`, `agent_labels`,
`priority_map` and `reply_associations` are applied by the Station (or are GitHub's) and are
accepted and ignored here, so one table serves every tracker.

## Behaviour

- **Poll** searches `project = "KEY" AND labels = "..." AND (jql) AND statusCategory != Done
  AND updated >= "<cursor minute>" ORDER BY updated ASC`. Cloud uses
  `GET /rest/api/3/search/jql` with `nextPageToken`; Server uses `GET /rest/api/2/search`
  with `startAt`. At most `max_pages` (5) pages of `page_size` (50). Each issue becomes a `SourceItem` whose ID is the
  issue key; Cloud descriptions in Atlassian Document Format are flattened to plain text.
- **Cursor** is the newest `updated` timestamp. JQL compares at minute precision in the
  account's time zone, so the next search starts at that minute (`updated >= "YYYY-MM-DD
  HH:MM"`) and the newest issue repeats; the Registry sees it as unchanged.
- **Items**: an item ID is an issue key in the instance's `project`; any other is refused
  (`GUATIAO_ERR_BAD_VALUE`, message `foreign item ...`) before a request.
- **Capabilities**: comment, edit, free-text labels (`free_labels`), assign, close, issue
  types and priorities from `GET /issue/createmeta/{project}/issuetypes` and `GET /priority`,
  `children = parent` on Cloud and `subtasks` on Server (`none` when the project has no
  sub-task type), `issue_links` and `remote_links`, `visibility = unknown`, no markers, and
  the transitions open on the item asked about. Project answers are cached
  `capability_ttl_secs`.
- **Post** comments (ADF on Cloud, plain text on Server) or edits
  (`PUT /issue/{key}/comment/{id}`). Cloud text goes through the Markdown subset headings,
  bullet and numbered lists, fenced code blocks and `http(s)` links; raw HTML stays literal.
- **Update**: labels by `update.labels` add/remove (whitespace becomes `-`); one assignee
  (`accountId` on Cloud, `name` on Server; clearing sends null); a `transition` by name
  (case-insensitive) or ID, or a `state`: `closed` takes a transition into a `done`-category
  status, `open` one into `new` or `indeterminate`. An unmatched transition is
  `GUATIAO_ERR_UNSUPPORTED` naming the ones that are open.
- **Create** posts `fields` with the project, the first line of the title, the body, the
  type (`issue_type`; a child takes `subtask_type` or the first sub-task type, and on Server
  needs one), labels, a priority only when the project has it, and `parent`. It returns the
  issue read back. `link` posts `POST /issueLink` (`Relates`) for an issue in the project, or
  `POST /issue/{key}/remotelink` for an `http(s)` address.
- **Replies** list each issue's comments (`GET /issue/{key}/comment`), keep those updated
  after the cursor (newest `updated` becomes it), and flatten ADF to text. `permitted`: the
  author is the issue's reporter or assignee, or is in `reply_authors`. `own`: the author is
  the credential's account (`GET /myself`).
- **Credentials** are read from the named variables on each call; a missing one is an
  error that names the variable. Errors never carry the credential or a response body.
- **Rate limits**: a 429 is an error naming `Retry-After`; the cursor does not move.
- Every request has a `request_timeout_secs` (30 s) timeout and follows no redirects.
