# `ai6-source-github` — API header

Header-file-style reference for the `ai6-source-github` crate: the `github` Source provider
for AI6, so it can be used without reading its source. It ships inside the package and is
self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

No Cargo features. MSRV: Rust 1.89. Depends on `ai6` (no default features), `ureq`,
`url`, `serde`, `serde_json` and `toml`. The library is both an `rlib` and the Guatiao
`cdylib` a Station loads. Unsafe code is forbidden.

## API

```rust
pub const PROVIDER_ID: &str = "github";
pub struct Github { /* settings, API origin, HTTP agent */ }

impl Github {
    pub fn from_settings(settings_toml: &str) -> Result<Self, String>;
}
impl TryFrom<ai6::SourceConfig> for Github { /* same validation; BAD_VALUE on refusal */ }
impl ai6::Source for Github {
    fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError>;
    fn report(&self, report: CxReport) -> Result<(), ProviderError>;
}
```

Provider ID `github`, kind `ai6_source`. The host passes the Operation's
`sources.github` table, serialized as TOML, in `SourceConfig::settings_toml`.

## Settings

| Key | Default | Meaning |
| --- | --- | --- |
| `repo` (alias `target`) | required | `owner/name`; a bare string under `sources` is this |
| `labels` | `["ai6:ready"]` | an issue needs all of them; `[]` takes every open issue |
| `assignee` | none | only issues assigned to this login |
| `token_env` | `GITHUB_TOKEN` | environment variable holding the token |
| `api_url` | `https://api.github.com` | REST root; `https`, or `http` on loopback only |
| `done_label` / `failed_label` | none | label added when the Mission ends in that state |

Unknown keys are refused. `interval_secs` is read by the Station and never reaches the
provider.

## Behaviour

- **Poll** lists `GET /repos/{repo}/issues` (open, oldest change first, 100 per page,
  at most 5 pages) and maps each issue or pull request to a `SourceItem` with ID
  `owner/name#number`. The returned cursor is the newest `updated_at`; the next poll sends
  it as `since`, which GitHub treats as inclusive, so the newest item repeats and the
  Registry sees it as unchanged. A `Link: rel="next"` that leaves the API origin is not
  followed, so the token is never sent elsewhere.
- **Report** posts `body` as an issue comment, then adds `done_label` (outcome `done`) or
  `failed_label` (any other outcome) when set. The token is required.
- **Token**: read from `token_env` on each call; a missing token is an error only where a
  token is required (`report`). Errors name the variable, never the value or a response
  body.
- **Rate limits**: a 429, or a 403 with `x-ratelimit-remaining: 0`, is an error naming
  the reset time; the cursor does not move and the Station polls again at its next interval.
- Every request has a 30 s timeout and follows no redirects.
- Review comments on a pull request are not forwarded to the Mission's Session.
