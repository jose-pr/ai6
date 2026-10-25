# Missions and intake

AI6 does not bundle tracker integrations. A user or external integration supplies a
Mission brief through the Universal Exports API or the file-backed Registry. The Rust
daemon validates the brief, resolves its Operation and Agent policy, and schedules the
Mission at an eligible Station.

```text
source ─► brief ─► Registry ─► AI6 Station ─► AgentHandler ─► provider Session
                                                │
                                                └─► outcome and DipTels to Universal Exports
```

## Mission and Session identities

A **Mission ID** identifies one unit of work and may group multiple agent Sessions. A
**Session ID** comes from the provider and remains stable when AI6 resumes that same
agent operation. An external tracker task ID is input metadata; it is not an AI6 Mission
ID.

```json
{
  "title": "Fix the flaky watchdog test",
  "prompt": "It fails one run in ten on macOS.",
  "operation_id": "G"
}
```

## Queue a Mission through Universal Exports

An authorized user needs `mission.create` for the target Operation. The gateway returns
the queued Mission ID without echoing the prompt:

```bash
curl -f -X POST "$AI6_EXPORTS_URL/v1/user/operations/G/missions" \
  -H "authorization: Bearer $AI6_ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"title":"Fix the flaky watchdog test","prompt":"Investigate the intermittent failure."}'
```

The Moneypenny app's Operations view offers the same action. The AI6 daemon claims the brief
from the shared file-backed Registry, then resolves the Agent role, Operation
cover, Station capacity and provider handler before it starts a Session.

## File-backed intake

The Registry defaults to the platform AI6 state directory and can be overridden
with `AI6_REGISTRY`. Briefs in `new/` are claimed atomically and filed under `claimed/`,
then `done/` or `failed/`. The stable brief identity combines its source and external
record ID. User-supplied integrations can write validated brief JSON into `new/` or call
the Universal Exports API.

SQLite/PostgreSQL Registry backends and distributed Registry placement are not currently
implemented. Remote Theaters are also not implemented; every Session runs on its
Station's host.

## Sources

A **Source** is a Guatiao provider that polls a tracker or CI system for work and posts the
outcome back: `github` (labeled issues and pull requests) and `jira` (a JQL search). An
Operation names its sources under `sources.<provider>` in `operations.toml`
([settings](operations.md#sources)).

```text
tracker ─► Source.poll ─► brief (new/) ─► Mission ─► Source.report ─► comment on the item
```

- **Where it runs.** Only the Station holding the Headquarters lease polls, every
  `interval_secs` (default 60, at least 10) per source. Provider calls block on the
  network, so they run off the daemon's async runtime. Each Operation and provider keeps a
  cursor in the Registry, so a Station that takes over the lease resumes where the last one
  stopped.
- **Brief identity.** An item becomes a brief keyed `source_<provider>--<item>`. If the
  item changes while its brief is still in `new/`, the brief is updated in place. Once a
  Station has claimed it, polling leaves it alone; a finished item is never queued again,
  so a Mission is never duplicated.
- **Untrusted text.** The brief's title is generated. The tracker's own title, labels and
  description are placed between fenced markers and introduced as data that cannot change
  the Agent's task or permissions. A description cannot close the fence.
- **Credentials.** A source names environment variables (`token_env`, `email_env`); values
  are read when needed and never written to configuration, logs or error messages.
- **CX report.** When a Mission ends, the Headquarters Station posts a comment on the
  item (and may add a label or move a Jira issue): the outcome, plus what the
  Operation's cover policy allows. `covert` says only that the work finished or could not
  be completed, and the branch; `partial` adds that AI helped and the Mission ID; `overt`
  also names the Agent and the provider Session IDs. A report that fails is retried a few
  times, then dropped.
- **Limits.** Review comments on a pull request are not forwarded to the Mission's
  Session. Jira issues are not transitioned when a Mission starts, only when it ends.

## Restart recovery and publication

AI6 persists the selected Agent, handler, terminal, working directory, model, effort,
provider Session ID and cover settings for a started Mission. Environment values are
not stored. On restart, the scheduler retries reattachment using the saved provider
Session ID and verified Station safe house. A handler must return the same Session ID or
AI6 refuses the resume.

A Station can publish lifecycle and terminal events to Universal Exports. When
enabled, it consumes the gateway's authenticated input/resize queue and routes frames to
the matching live provider Session. Configure `AI6_EXPORTS_URL` and
`AI6_EXPORTS_STATION_TOKEN` together; leave both unset to disable publication. Events use a
bounded background worker and finite request timeouts, so a gateway outage does not block
the provider Session. Terminal bytes remain binary.
