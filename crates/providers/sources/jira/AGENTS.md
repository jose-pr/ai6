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

Unknown keys are refused. `interval_secs` is read by the Station and never reaches the
provider.

## Behaviour

- **Poll** searches `project = "KEY" AND labels = "..." AND (jql) AND statusCategory != Done
  AND updated >= "<cursor minute>" ORDER BY updated ASC`. Cloud uses
  `GET /rest/api/3/search/jql` with `nextPageToken`; Server uses `GET /rest/api/2/search`
  with `startAt`. At most 5 pages of 50. Each issue becomes a `SourceItem` whose ID is the
  issue key; Cloud descriptions in Atlassian Document Format are flattened to plain text.
- **Cursor** is the newest `updated` timestamp. JQL compares at minute precision in the
  account's time zone, so the next search starts at that minute (`updated >= "YYYY-MM-DD
  HH:MM"`) and the newest issue repeats; the Registry sees it as unchanged.
- **Report** posts `body` as a comment (ADF paragraphs on Cloud, plain text on Server),
  then applies `done_transition` (outcome `done`) or `failed_transition` (any other outcome)
  by listing the issue's transitions and matching a name case-insensitively or an ID. A
  transition that does not match is an error after the comment is posted.
- **Credentials** are read from the named variables on each call; a missing one is an
  error that names the variable. Errors never carry the credential or a response body.
- **Rate limits**: a 429 is an error naming `Retry-After`; the cursor does not move.
- Every request has a 30 s timeout and follows no redirects.
- Moving an issue when its Mission starts is not implemented.
