# Changelog

All notable changes to this package are documented in this file; the repository's
[CHANGELOG](../CHANGELOG.md) covers every implementation.

## Unreleased

- Hooks run like a harness's own, from specification revision ea41520: a `pre-tool` hook may print
  `decision: allow`, with an `input` object that replaces the call's; `fireEvent` takes the call's
  `toolInput` and reports what each hook `seen` and the `input` the call runs with. The failure cause
  `grant` is gone: any other decision is an `output` failure. `hookEnvironment` is the session's
  environment, overlays stacked in composition order, with `earlier` for what earlier hooks of the event set,
  and `processEnvironment` is the same for an overlay's setup, server, command and env script. `Environment`
  no longer keeps a write log, `PathJoin` and `EnvWrite` are gone.
- From specification revision c2f6d88, on a call of kind `other` only a hook that names `other` or has no
  `match` may allow it: `HookRun` gains `select` and `fireEvent` a `toolKind`; another hook's allow and
  `input` are not counted and are reported `hook-not-applied` `allow`, and its deny and ask still count.

- From specification revision ab00884, an overlay's knowledge served through MCP: `knowledgeTools`,
  `knowledgeArguments`, `kbList`, `kbRead` (the served text of a file: a markdown file names its format and its
  `name=value` options on its first line; expanded by default, `${NAME}`, `$$` and `@path` includes outside code;
  `markdown`, `md` and `raw` as written; standard Mustache with `escape` and partials from the overlay, through the
  `mustache_template` package; envsubst with `dialect`, `pattern`, `features` and `undefined`, its own small
  implementation of GNU's behaviour and the `escape` and `default` features; any other engine kept with an
  `aos_template_context` block), `knowledgeResult`, `knowledgeSet`, `knowledgeServe`, `knowledgeFormat`,
  `readKnowledgeTree`, `knowledgePlan` and the `[knowledge]` table's validation; `searchCheck` and `searchOrder` check
  a `kb_search` answer. Every limit is a `KnowledgeSettings` field with no floor, `maxIncludeDepth` included. New
  dependency: `mustache_template` 2.0.4 (BSD-3-Clause, pure Dart, VM and web). `engines` may also carry
  option values the server lacks, as `-engine:option=value` (the file is then served as written, `rendered: false`).
  One vector case, `mustache-spaces-in-tag`, is named as not run: that package reads `{{ & x }}` as the
  specification does.

## 0.0.0-alpha.0

- First release: the whole of AOS v0 `0.1.0-draft` in pure Dart, passing every conformance vector
  on the VM and in Chrome.
