import 'oo_link.dart';
import 'oo_markdown.dart';

const _tracker = 'tracker.example.com';
const _forge = 'git.example.com';

/// The issue key, issue and Mission the preview's documents mention, as the gateway resolves
/// them for a reader.
List<DocReference> demoReferences() => [
  DocReference(
    text: 'OPS-12',
    url: Uri.parse('https://$_tracker/browse/OPS-12'),
  ),
  DocReference(
    text: 'acme/demo#12',
    url: Uri.parse('https://$_forge/acme/demo/issues/12'),
  ),
  DocReference(
    text: 'mission-007',
    route: OoLinkTo.mission('G', 'mission-007').toUri(),
    issues: [
      DocLink(
        label: 'acme/demo#12 Crash on save (open)',
        url: Uri.parse('https://$_forge/acme/demo/issues/12'),
      ),
    ],
    boards: [
      DocLink(
        label: 'acme/demo issues',
        url: Uri.parse('https://$_forge/acme/demo/issues'),
      ),
    ],
  ),
];

const _mention = 'Tracked as OPS-12 and acme/demo#12, continuing mission-007.';

OoDocument _document(String markdown) => OoDocument(
  markdown: markdown,
  references: demoReferences(),
  trackers: const {_tracker, _forge},
);

/// The preview's document at [path] (a gateway document route), or `null` for one it has none
/// for.
OoDocument? demoDocument(String path) {
  final route = Uri.decodeFull(path);
  if (route.endsWith('/missions/mission-007/brief')) {
    return _document('''
---
kind: brief
operation: G
mission: mission-007
handle: mission-007
agent: technical-officer
role: case-officer
station: station-control-01
theater: local
mode: field
handler: claude
terminal: pty
working_directory: C:/operations/field
model: claude-sonnet
effort: high
environment_names:
  - AI6_MISSION_ID
origin: Started by demo-operator through Universal Exports
---

# Review the requested change

$_mention

| Check | Expectation |
| --- | --- |
| Tests | pass |
| Docs | updated |

Report findings in the Mission's final message.
''');
  }
  if (route.endsWith('/green-ink/green-ink-preview/document')) {
    return _document('''
---
kind: green_ink
operation: G
id: green-ink-preview
state: pending
mission: mission-007
agent: "007"
step: push
target: "origin:refs/heads/main"
---

## Why this push

The release branch passed review. $_mention

| Gate | Result |
| --- | --- |
| Plan | passed |
| Output | passed |
''');
  }
  if (route.endsWith('/held-briefs/source_github--org_repo_7/document')) {
    return _document('''
---
kind: held_brief
operation: G
key: source_github--org_repo_7
source: "source:github"
verdict: hold
findings:
  - kind: duplicate
    detail: the same URL as other work
    related: mission-007
---

# Crash on save

The app crashes when saving. $_mention

| Step | Result |
| --- | --- |
| Open a file | works |
| Save it | crashes |
''');
  }
  if (route.endsWith('/missions/mission-007/reviews/document')) {
    return _document('''
---
kind: review
operation: G
mission: mission-007
---

## Output review

$_mention

| Reviewer | Vote |
| --- | --- |
| r1 | redo |

> r1: no test for the new path
''');
  }
  if (route.endsWith('/charges/station:station-control-01/document')) {
    return _document('''
---
kind: charge
scope: "station:station-control-01"
agent: "007"
station: station-control-01
---

# Head of Station report

$_mention

| Theater | State |
| --- | --- |
| local | ready |
| build-box | ready |
''');
  }
  return null;
}
