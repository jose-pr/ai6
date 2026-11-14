# AOS conformance vectors

JSON data an implementation in any language tests against. The format of each file, the comparison
rules and the classes they serve are in
[Conformance](https://boriken-dev.github.io/agent-overlay-spec/spec/v0/conformance/#vectors).

| File | What it checks |
| --- | --- |
| `reading.json` | Format line, TOML default, JSON and YAML data model, static reading, limits, `dotenv` and its substitution, env-script output, and catalogs read by file name |
| `front_matter.json` | `OVERLAY.md` and front matter: the split, the closing fence, a byte-order mark and `\r`, the body's bytes, positions, bounds, the format line, restricted YAML including billion-laughs and deep-nesting shapes, a context file's text, and an overlay's two manifests and its context |
| `skills.json` | Skills from elsewhere: where a name resolves, missing and ambiguous names, pins, what a consumer that does not fetch says, collisions, and the checks on a fetched skill, with the files its digest covers |
| `validation.json` | Every field, valid and invalid, and files a field names; whether a manifest with problems under `about` still applies; which skill names a validator flags |
| `names.json` | Overlay identities, normalization, root variables |
| `source.json` | `[git+]<location>[@<ref>][#<path>]` |
| `digest.json` | The tree digest |
| `compose.json` | `requires`, cycles, composition order, contributions, environment across overlays (values chained from one overlay to the next, and `pre-env` hooks' output before every overlay's), and the defaults of `rules`, `context`, `skills` and `paths` |
| `effective.json` | One overlay's environment, setup, setup phases, runtime, needs and non-default settings on a given OS, with the defaults of `paths`, `env_files`, `env_scripts` and `setup`, and env scripts off until the digest is allowed |
| `templating.json` | `${NAME}` in context and in a bundled skill's body: the five `AOS_` names, an unknown name and other forms left as written, `$${`, options with a default, with a configured value and with the wrong type, a sensitive option kept out, and a value never read again |
| `catalog.json` | Catalogs in both forms, overlays and skills, sources resolved beside the catalog, refused entries and ignored fields, names several catalogs share, and icons |
| `hooks.json` | Hooks: which run at each event and with what, `hook.<ext>` chosen by runtime, off until allowed at one digest (a project's over every file beside its configuration), a guard that is off keeping its overlay inactive, events a consumer cannot fire or fires in part; each event's payload; which calls a `match` selects; a run's output, and every way it fails, an attempt to grant, an undeclared variable and a control character included; several hooks of one event combined within the event's bound, and what a consumer cannot apply on a harness |

Error classes:

- reading: `invalid-utf8`, `no-format-named`, `unsupported-format`, `parse-error`, `not-a-mapping`,
  `null-value`, `duplicate-key`, `non-string-key`, `datetime-value`, `non-finite-float`,
  `integer-range`, `invalid-unicode`, `yaml-tag`, `yaml-anchor`, `yaml-merge-key`,
  `yaml-directive`, `too-deep`, `too-large`, `dotenv-syntax`, `env-script-output`,
  `unterminated-front-matter`, `malformed-fence`, and for an overlay's files `two-manifests`;
- templating: `invalid-option`, and `invalid-utf8` and `too-large` as reading;
- substituting, in `dotenv` reading cases and when applying: `undefined-variable`,
  `secret-placeholder`, `invalid-pattern`;
- digest: `symlink`, `special-file`, `non-utf8-name`, `invalid-name`, `case-collision`;
- applying: `unresolved-reference`, `invalid-reference`, `missing-file`, `invalid-env-file`,
  `env-path-conflict`, `reserved-variable`;
- resolving: `cycle`, `invalid-source`;
- skills: `skill-name`, `skill-duplicate`, `skill-ambiguous`, `skill-case`, `local-source`,
  `source-credentials`, `ref-not-allowed` and `manifest-file` in a manifest, and when one is fetched `pin-mismatch`,
  `not-a-skill`, `skill-unreadable`, `skill-name-mismatch`;
- catalogs: `not-a-catalog` and the reading classes for a whole catalog; for an entry
  `invalid-name`, `duplicate-name`, `invalid-entry`, `invalid-source`, `invalid-pin`,
  `ref-not-allowed`, `leaves-repository`, `local-source`, `source-credentials`; for several catalogs `invalid-label`,
  and for a requested name `ambiguous`, `not-found`;
- icons: `icon-format`, `icon-too-large`, `icon-mismatch`, `icon-unreadable`, `icon-dimensions`;
- hooks: a run fails with `event-timeout`, `not-run`, `payload-too-large`, `timeout`, `too-large`,
  `exit`, `grant` or `output`; reports are `hook-not-allowed`, `env-script-not-allowed`, `hook-unsupported`, `hook-partial`,
  `hook-failed`, `hook-not-applied` and `hook-ask-denied`, and the non-default setting `hook`; a `pre-env` refusal
  is `hook-refused` when applying.

`python -m pytest tests/test_vectors.py tests/test_digest.py` runs them through this repository's
reference reader.
