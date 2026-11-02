# `ai6-source-github` — API header

Header-file-style reference for the `ai6-source-github` crate: the `github` Source and Tracker
provider for AI6, so it can be used without reading its source. It ships inside the package and is
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
}
impl ai6::Tracker for Github {
    fn capabilities(&self, item: Option<String>) -> Result<TrackerCapabilities, ProviderError>;
    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError>;
    fn replies(&self, request: ReplyPoll) -> Result<ReplyBatch, ProviderError>;
    fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError>;
    fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError>;
    fn create(&self, spec: IssueSpec) -> Result<TrackerIssue, ProviderError>;
    fn link(&self, link: TrackerLink) -> Result<(), ProviderError>;
}
```

Provider ID `github`, kinds `ai6_source` and `ai6_tracker`; one table builds both. The host
passes the instance's `sources.<name>` table (its `provider` key names this one), serialized
as TOML, in `SourceConfig::settings_toml`.

## Settings

| Key | Default | Meaning |
| --- | --- | --- |
| `repo` (alias `target`) | required | `owner/name`; a bare string under `sources` is this |
| `labels` | `["ai6:ready"]` | an issue needs all of them; `[]` takes every open issue |
| `assignee` | none | only issues assigned to this login |
| `token_env` | `GITHUB_TOKEN` | environment variable holding the token |
| `api_url` | `https://api.github.com` | REST root; `https`, or `http` on loopback only |
| `ca_file` | none | absolute path of a PEM file of certificate authorities trusted besides the public roots; a file that cannot be read or holds no certificate is refused, and a set one is reported as a weakening |
| `request_timeout_secs` | 30 | seconds one request may take; 0 is refused |
| `page_size` | 100 | issues asked for per page; 0 is refused |
| `max_pages` | 5 | pages one poll reads at most; 0 is refused |
| `settling_secs` | 60 | seconds a poll searches behind the cursor; 0 searches from the cursor |

| `reply_authors` | `[]` | logins besides the issue's author and the associations below whose comments may answer a question |
| `reply_associations` | `["OWNER", "MEMBER", "COLLABORATOR"]` | `author_association` values that may answer |
| `capability_ttl_secs` | 600 | how long the repository's labels and settings are remembered |

Unknown keys are refused. `provider`, `intake` and `interval_secs` are read by the Station and
never reach the provider, nor do the Station's own keys (`status_labels`, `status_transitions`,
`agent_labels`, `agent_transitions`, `priority_map`): an unknown key here is refused.

## Behaviour

- **Poll** lists `GET /repos/{repo}/issues` (open, oldest change first, `page_size` (100) per page,
  at most `max_pages` (5) pages) and maps each issue or pull request to a `SourceItem` with ID
  `owner/name#number`, `code` the number and `code_tag` `gh`. The returned cursor is the newest `updated_at`, and never earlier than
  the one it was given; the next poll sends it, `settling_secs` earlier, as `since`, which GitHub
  treats as inclusive, so what changed in that margin repeats and the Registry sees it as
  unchanged. When the pages one poll reads hold nothing newer than the cursor and more remain,
  it searches again from the cursor itself, so a margin fuller than a poll cannot stop it. A
  `Link: rel="next"` that leaves the API origin is not followed, so the token is never sent
  elsewhere.
- **Listing lag**: a new or changed issue can take several seconds to show in the listing,
  so it can appear on a later poll. The margin is for one shown after a newer one, whose time
  the cursor has by then taken.
- **Items**: an item ID is `owner/name#number` naming this instance's repository; any other
  is refused (`GUATIAO_ERR_BAD_VALUE`, message `foreign item ...`) before a request.
- **Capabilities**: comment, edit, labels (the repository's own), assign, close, create,
  replies, markers, `children = sub_issues` (`none` when an item's sub-issues endpoint
  answers an error), no transitions, no issue types or priorities, no link kinds besides
  naming an issue in text, `visibility` from the repository's `private`. Repository answers
  are cached `capability_ttl_secs`.
- **Post** comments (`POST .../comments`) or edits (`PATCH .../issues/comments/{id}`; the ID
  is digits only). Every post ends with an HTML comment `<!-- ai6:NAME -->` (`NAME` the `marker`,
  `comment` without one), which GitHub keeps in `body` and which alone makes a reply `own`. A
  blank body is an error before any request, since GitHub refuses it.
- **Update**: labels added (a label the repository lacks is created) and removed (a label the
  issue lacks is not an error; a missing issue is, told apart by GitHub's message), assignees
  added and removed, `state` `open` or `closed` (closing sets `state_reason: completed`). A
  `transition` is `GUATIAO_ERR_UNSUPPORTED`.
- **Create** keeps only the labels the repository has (GitHub would create the rest);
  `issue_type` and `priority` are not GitHub concepts and are ignored; a `parent` is attached
  with `POST .../sub_issues` and, when that fails, the issue stays created and comes back with
  no parent. `link` is `GUATIAO_ERR_UNSUPPORTED`.
- **Replies** read `GET .../issues/comments?since=` (repository-wide, cursor = newest
  `updated_at`, inclusive) for the issues asked about. `permitted`: the comment's
  `author_association` is in `reply_associations`, the login is in `reply_authors`, or the
  login is the issue's author. `own`: the login is the token's (`GET /user`), or the comment
  carries the provider's marker, for a token with no user.
- **Token**: read from `token_env` on each call; a missing token is an error for every call
  but a read (`GET`), and a private repository answers a read with no token with 404, so that
  error names the unset variable. Errors name the variable, never the value or a response body.
- **API version**: every request asks for `2026-03-10`; under `2022-11-28` creating an issue
  answers `Deprecation`, with a sunset of 2028-03-10.
- **Rate limits**: a 429, or a 403 with `x-ratelimit-remaining: 0`, is an error naming
  the reset time; the cursor does not move and the Station polls again at its next interval.
- Every request has a `request_timeout_secs` (30 s) timeout and follows no redirects.
- Review comments on a pull request are not forwarded to the Mission's Session.
