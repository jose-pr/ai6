# `ai6-source-jira` — API header

Header-file-style reference for the `ai6-source-jira` crate: the `jira` Source provider for
AI6, so it can be used without reading its source. It ships inside the package and is
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
    fn report(&self, report: CxReport) -> Result<(), ProviderError>;
}
```

Provider ID `jira`, kind `ai6_source`. The host passes the Operation's `sources.jira` table,
serialized as TOML, in `SourceConfig::settings_toml`.

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
| `done_transition` / `failed_transition` | none | transition name or ID applied when the Mission ends in that state |
| `request_timeout_secs` | 30 | seconds one request may take; 0 is refused |
| `page_size` | 50 | issues asked for per page; 0 is refused |
| `max_pages` | 5 | pages one poll reads at most; 0 is refused |

Unknown keys are refused. `interval_secs` is read by the Station and never reaches the
provider.

## Behaviour

- **Poll** searches `project = "KEY" AND labels = "..." AND (jql) AND statusCategory != Done
  AND updated >= "<cursor minute>" ORDER BY updated ASC`. Cloud uses
  `GET /rest/api/3/search/jql` with `nextPageToken`; Server uses `GET /rest/api/2/search`
  with `startAt`. At most `max_pages` (5) pages of `page_size` (50). Each issue becomes a `SourceItem` whose ID is the
  issue key; Cloud descriptions in Atlassian Document Format are flattened to plain text:
  one line per block, links and mentions as their visible text, emoji and status lozenges as
  their text, dates as a UTC `YYYY-MM-DD`; an attachment has no text and is dropped.
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
- **Report** posts `body` as a comment (ADF paragraphs on Cloud, plain text on Server),
  then applies `done_transition` (outcome `done`) or `failed_transition` (any other outcome)
  by listing the issue's transitions and matching a name case-insensitively or an ID. A
  transition that does not match is an error after the comment is posted. A blank `body` is
  an error before any request, since Jira refuses it.
- **Credentials** are read from the named variables on each call; a missing one is an
  error that names the variable. Cloud answers a refused credential on an issue with 404,
  so that error names `token_env` too. Errors never carry the credential or a response body.
- **Wrong `deployment`**: Cloud has removed REST v2 search and answers 410; the error says
  to check `deployment`.
- **Rate limits**: a 429 is an error naming `Retry-After`; the cursor does not move.
- Every request has a `request_timeout_secs` (30 s) timeout and follows no redirects.
- Moving an issue when its Mission starts is not implemented.
