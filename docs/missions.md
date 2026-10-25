# Missions and intake

AI6 does not bundle tracker integrations. A user or external integration supplies a
Mission brief through the Universal Exports API or the file-backed I Branch Registry. The Rust
daemon validates the brief, resolves its Operation and Agent policy, and schedules the
Mission at an eligible Station.

```text
source ─► brief ─► I Branch registry ─► AI6 Station ─► AgentHandler ─► provider Session
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
from the shared file-backed I Branch registry, then resolves the Agent role, Operation
cover, Station capacity and provider handler before it starts a Session.

## File-backed intake

The I Branch registry defaults to the platform AI6 state directory and can be overridden
with `AI6_REGISTRY`. Briefs in `new/` are claimed atomically and filed under `claimed/`,
then `done/` or `failed/`. The stable brief identity combines its source and external
record ID. User-supplied integrations can write validated brief JSON into `new/` or call
the Universal Exports API.

SQLite/PostgreSQL Registry backends and distributed Registry placement are not currently
implemented. Remote Theaters are also not implemented; every Session runs on its
Station's host.

## Restart recovery and publication

AI6 persists the selected Agent, handler, terminal, working directory, model, effort,
provider Session ID and cover settings for a started Mission. Environment values are
not stored. On restart, the scheduler retries reattachment using the saved provider
Session ID and verified Station safe house. A handler must return the same Session ID or
AI6 refuses the resume.

A Station can publish lifecycle and terminal events to Universal Exports. When
enabled, it consumes the gateway's authenticated input/resize queue and routes frames to
the matching live provider Session. Configure `AI6_EXPORTS_URL` and
`AI6_EXPORTS_TOKEN` together; leave both unset to disable publication. Events use a
bounded background worker and finite request timeouts, so a gateway outage does not block
the provider Session. Terminal bytes remain binary.
