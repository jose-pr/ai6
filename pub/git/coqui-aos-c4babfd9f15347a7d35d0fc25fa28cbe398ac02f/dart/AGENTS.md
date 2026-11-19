# `coqui_aos` (Dart) — API header

Header-file-style reference for the Dart package: every public item with its contract and gotchas, so it can be used without reading the source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-aos>.

Version `0.0.0-alpha.0`, `publish_to: none`, not published; the API may change in any release before 1.0. Dart 3.9 or later; pure Dart, no Flutter, no native code, no `dart:io` in the core, so it runs on the VM and on the web. Two entry points: `package:coqui_aos/coqui_aos.dart` (everything) and `package:coqui_aos/io.dart` (`IoFileSystem`, `currentOs()`; imports `dart:io`). Runtime dependencies: `yaml` (its parser's events show tags, anchors and aliases before anything is composed), `crypto` (SHA-256) and `mustache_template` (BSD-3-Clause, pure Dart, VM and web, pinned at 2.0.4: knowledge files that name `mustache` are rendered by it, through a few of its node classes). TOML and JSON are read by the package's own readers. Edition: AOS v0, `0.1.0-draft`. No process-wide state; nothing is printed; no environment variable is read.

Reading a manifest is static: nothing here runs a script, opens a connection or consults the environment. Files are read only by `readTree` and `treeDigest`, through the `FileSystem` they are given.

The names are the TypeScript package's, in Dart style: functions and fields are lowerCamelCase, constants too (`defaultKeys`), a TS `with` is `copyWith`. Closed sets of words are plain `String`s; each has a `Words` constant (`runtime`, `format`, `manifestFile`, `leftState`, `failStep`, `nonDefaultId`, `hookEvent`, `decision`, `onFailure`, `hookScope`, `hookFailure`, `plannedOrigin`, `catalogForm`, `iconFormat`, `entryRefusal`, `changes`, `phaseWords`, `issueCodes`) with `all`, `contains(word)` and `named(word)`. Sealed classes and records stand in for TS unions and tuples.

## Data model (`Value`, `Document`)

```dart
typedef Value = Object;                    // bool | BigInt | double | String | List<Object> | Document
typedef Document = Map<String, Object>;    // the document's order; any key, `__proto__` included
```

An integer is a `BigInt` in the signed 64-bit range and a float a finite `double`, so `30` is `BigInt.from(30)` and `30.0` is `30.0`: on the web a Dart `int` and a `double` are one JavaScript number, so integers are never `int`. `valuesEqual(a, b)` compares structurally (mapping order ignored, integer never equal to float). There is no null: a null is refused as `null-value`. Helpers: `isDocument`, `isList`, `isInteger`, `toPlain(value)` (integers that fit become `int`), `fromPlain(json)` (an `int` becomes an integer).

## Errors (`AosError`)

```dart
class AosError implements Exception { final String errorClass; final String subject; int? get line; int? get column; String? get setting; String get message;  AosError over(String setting) }
const List<String> errorClasses;  String? classNamed(String word)

For a refusal of a limit (`too-large`, `too-deep`, `too-complex`, `icon-too-large`, `icon-dimensions`) `setting` is the key of the setting that was passed, as a configuration writes it (`tree_files`, `manifest_depth`, `icon_width`, ...): stable, unlike `subject`'s wording; null for any other refusal.
```

**`errorClass` is the contract**: a word from `errorClasses` (`'parse-error'`, `'duplicate-key'`, `'too-deep'`, `'symlink'`, `'undefined-variable'`, ...), including `'io'`, `'duplicate-member'`, `'invalid-requirements'` and `'invalid-digest'`. `subject` names what it is about for a person, never text from a document; do not match on it. `line` and `column` are one-based, `column` counts Unicode scalar values, a line counts from the file's first line, a format line included. `message` is `class: subject at line N column M`. A `subject` is escaped as the specification's conformance chapter writes it and cut to 256 characters. Subclasses: `Cycle` (`chain`; from `resolve` and `compose`), `PartialLock` (from `dependenciesOf` and `environments`), `HookFailed` (`failure`; from `hookResult`). A caller's `FileSystem` or `SkillSource` may throw anything; the core reports a file system's as `io`.

## Settings (`Settings`, `Limit`)

```dart
typedef Limit = int?;                      // null: no bound
class Settings { Settings([Map<String, Limit> changes]); factory Settings.fromData(Map<String, Object?> data);
  Settings copyWith(Map<String, Limit> changes); Map<String, Limit> values(); List<NonDefault> nonDefaults();
  Limit get manifestBytes, manifestDepth, treeFiles, treeDepth, treeBytes, fileBytes, contextBytes, catalogBytes, iconBytes, iconWidth, iconHeight, hookTimeout, hookEventTimeout, hookOutputBytes, hookPayloadBytes, hookReasonChars, substitutionBytes, substitutionDepth, substitutionSteps }
final Settings defaultSettings;  bool allows(Limit limit, Object n);  String limitText(Limit limit)
```

Immutable. `changes` are keyed by field name (`{'manifestDepth': null}` lifts a bound); an unknown name or a negative number is an `ArgumentError`. `fromData` takes snake_case keys with a number, `BigInt`, null or `"none"`. Defaults: 1 MiB, 64, 4096, 64, 64 MiB, 16 MiB, 1 MiB, 4 MiB, 512 KiB, 1024, 1024, 60 s, 120 s, 64 KiB, 1 MiB, 1024 characters, 1 MiB, 32, 10 000 000. Every limit has no floor; `nonDefaults()` reports what differs (`NonDefault(setting, value, byDefault)`), so report it. `manifest*` bound a manifest, an env file and an env script's output; `treeFiles` counts files and directories together; `treeDepth` counts directory levels. Lifting `manifestDepth` does not risk the stack: the JSON and TOML readers nest without recursion.

## Reading

```dart
Document readDocument(Uint8List data, Settings settings)
List<DotenvLine> parseDotenv(Uint8List data)                          // DotenvLine(name, text, literal)
List<(String, String)> readDotenv(Uint8List data, Environment env)    // substituted against env, in order
List<(String, String?)> readEnvScriptOutput(Uint8List data, Settings settings)   // null unsets
Split splitFrontMatter(Uint8List data, Settings settings);  FrontMatter readFrontMatter(Uint8List data, Settings settings);  Uint8List contextFileText(Uint8List data, Settings settings)
OverlayRead readOverlay(OverlayTree tree, Settings settings)
```

`readDocument`: one optional byte-order mark, an optional format line (`#!json`, `#!/usr/bin/env yaml`), else TOML. **Dialect**: a duplicate key in one mapping, a null, a TOML date or time, a non-finite float, an integer outside the signed 64-bit range, a mapping key that is not text, a lone surrogate escape and a byte-order mark where the text starts are refused. In YAML a plain scalar is typed by the 1.2 core schema (`yes` is text), and a tag, anchor, alias, merge key (`<<` in any style), `%YAML`/`%TAG` directive and a second document are refused from the parser's events, before anything is composed. TOML is 1.0 (a newline in an inline table is a `parse-error`). Nesting past `manifestDepth` is `too-deep` in every format.

`Split` (`splitCase`, `metadata`, `metadataOffset`, `metadataLine`, `bodyOffset`, `bodyLine`, `body(data)`) divides bytes unchanged; offsets are bytes, lines from 1. `FrontMatter` (`frontMatterCase`: `none | malformed | unterminated | too-large | empty | unknown-format | does-not-parse | not-a-mapping | mapping`, `data`, `error`, `format`, `bodyOffset`, `bodyLine`): `error` is what a gate refuses with, the case what a viewer shows. `OverlayRead` holds `manifestFile`, `manifest`, `context`, `contextIgnored`, `issues` and `harnessContext`; it refuses two manifests (`two-manifests`), a manifest that does not read, and a context past `contextBytes` or not UTF-8.

## Validation

```dart
List<Issue> validate(Document manifest, [OverlayTree? tree])    // Issue(code, at): at is an RFC 6901 pointer; sorted by pointer
bool applies(List<Issue> issues)                                 // only problems under /about leave a manifest applying
List<(String, String)> namedFiles(Document doc);  List<String> skillFlags(Document doc, OverlayTree tree);  bool validLicense(String expression)
```

With a tree, the files a field names must be in it. The `hooks` field's files are checked, not run.

## Templating

```dart
TemplateFacts({String? root, state, name, scope, project, Map<String, Object> options = const {}})   // option values: String, BigInt, bool
Map<String, String> templateValues(Document manifest, TemplateFacts facts)   // invalid-option
String template(String text, Map<String, String> values)
String contextText(Uint8List data, Map<String, String> values, Settings settings)   // front matter dropped; too-large, invalid-utf8
String skillText(Uint8List data, Map<String, String> values, Settings settings)     // front matter as written
```

`${NAME}` is replaced in the context a consumer assembles and in the body of a bundled skill it serves, never in an installed file, and only for `AOS_OVERLAY_ROOT`, `AOS_OVERLAY_STATE`, `AOS_OVERLAY_NAME`, `AOS_OVERLAY_SCOPE`, `AOS_PROJECT_ROOT` and the overlay's `options` that have a value; a name with no value, an unknown name, one with an operator and a sensitive option are left as written, `$${` is `${`, and a value is never read again. A skill's front matter is served as written. A configured value of the wrong type is `invalid-option`, never converted. The bound on the context counts the text after replacing: the caller checks it.

## Names, sources, digests

```dart
bool validOverlayName(String), String normalizeName(String), rootVariable, stateVariable, bool validRelativePath, isVariableName, isReservedVariable, isSensitiveVariable
class Source { final String kind; location; reference; path;  factory Source.parse(String text);  shown(); toString(); isLocal(); holdsCredentials(); isWebUrl() }
class Digest { factory Digest.of(Uint8List); factory Digest.parse(String); equals(Digest); toString() }     // `sha256:<64 hex>`
TreeDigest digestEntries(Iterable<(List<Object>, TreeEntry)> entries)   // order does not matter; a path twice is `duplicate-member`
Future<TreeDigest> treeDigest(FileSystem files, String root, Settings settings)
Future<({OverlayTree tree, TreeDigest digest})> readTree(FileSystem files, String root, Settings settings)
sealed class TreeEntry { FileEntry(bytes), DirectoryEntry(), SymlinkEntry(), SpecialEntry() }
class TreeDigest { final Digest digest; final Map<String, Digest> files; List<Uint8List> lines() }
```

`Source.kind` is `git`, `url` or `directory`. `Source` hides user information and query in `shown()`, `toString()` and `toJson()`; `location` and `keyWith()` hold them, so never print those. Non-canonical IPv4 hosts (`127.1`, `2130706433`, `0x7f.0.0.1`) are classified as the address a URL parser reads, and `100.64.0.0/10` is private. Case folding of identities, variables and sensitivity is ASCII only; substitution's `^^` and `,,` fold by Unicode rules, with the SpecialCasing mappings that Dart's `toUpperCase` lacks.

## The filesystem (`FileSystem`, `IoFileSystem`)

```dart
abstract interface class FileSystem {
  FutureOr<Metadata> metadata(String path);             // WITHOUT following a link: a link is FileKind.symlink
  FutureOr<Uint8List> read(String path, int max);       // the first `max` bytes
  FutureOr<List<Object>> list(String path);             // single components, any order; non-Unicode names as Uint8List
}
enum FileKind { file, directory, symlink, other };  class Metadata { final FileKind kind; final int len }
class IoFileSystem implements FileSystem               // package:coqui_aos/io.dart
String currentOs()                                     // package:coqui_aos/io.dart: windows, macos, linux, ...
```

The core joins a root and a name with `/` (`joinPath`). `treeDigest` checks every entry with `metadata` before it reads or descends, and refuses a listed name that is empty, `.`, `..` or holds `/`, `\` or NUL, and a name that is not Unicode (`non-utf8-name`). `IoFileSystem.read` and `list` open through links: a link swapped in between `metadata` and `read` is followed, so read only trees no one else can write.

## What an overlay's files contribute

```dart
class OverlayTree { OverlayTree.fromPaths(paths), .fromTexts(Iterable<(String, String)>), .fromBytes(...), .empty();  withText(path, text), withBytes(path, bytes);  paths(), members(), contains, hasMember(path), text(path), bytes(path) }
rulesOf, contextOf, pathsOf, envFilesOf, skillsOf(manifest, tree), defaultPaths, defaultOn(manifest, key), contextIgnored, harnessContextOf(manifest, harness, tree)
List<SkillEntry> skillEntries(Document manifest, OverlayTree tree)         // SkillEntry(name, origin, source, key, pin, from)
List<Dependency> dependenciesOf(Document manifest, OverlayTree tree, String os)   // throws PartialLock
void checkRequirements(String text, bool locked)                           // `invalid-requirements`; never quotes the line
```

`OverlayTree` is immutable. `withText` ignores a path the tree does not hold; `withBytes` adds it. The body of `OVERLAY.md` is the context default when its text was read into the tree.

## Knowledge served through MCP

```dart
Map<String, Object?> kbRead(List<KnowledgeOverlay> overlays, {required String path, Object? from, Object? to, String? overlay, Map<String, Object?>? context, CallerFacts caller = const CallerFacts(), Iterable<String> engines = knowledgeEngines, KnowledgeSettings? settings})
Map<String, Object?> kbList(List<KnowledgeOverlay> overlays, {String? overlay, Iterable<String> modes = const [], CallerFacts caller, Iterable<String> engines = knowledgeEngines, KnowledgeSettings? settings})
Map<String, Object?> knowledgeCall(String tool, List<KnowledgeOverlay> overlays, Object? arguments, {Iterable<String> modes = const [], KnowledgeSettings? settings, CallerFacts caller, Iterable<String> engines = knowledgeEngines})   // {'result': ...} or {'error': {'class', 'message'}}
Map<String, Object?> knowledgeResult(Map<String, Object?> outcome)     // MCP: {content: [{type: text, text}], structuredContent, isError}
List<Map<String, Object?>> knowledgeTools(Iterable<String> modes, [KnowledgeSettings? settings])   // kb_list, kb_read, and kb_search when a mode is declared
Map<String, Object?> knowledgeArguments(String tool, Object? arguments, Iterable<String> modes, [KnowledgeSettings? settings])   // throws KnowledgeError invalid-arguments
KnowledgeSet knowledgeSet(KnowledgeOverlay overlay, [KnowledgeSettings? settings])   // files (KnowledgeFile: path, text, engine, asWritten, options, bytes, data, rest), skipped [(path, reason)]
String knowledgeServe(KnowledgeOverlay overlay, KnowledgeSet held, String path, Map<String, Object?> context, CallerFacts call, KnowledgeSettings limit, List<String> engines)   // the served text; KnowledgeError include-error, invalid-option, template-error, too-large
bool knowledgeRenders(KnowledgeFile item, List<String> engines)          // whether this server renders the file's engine with its options
List<String> knowledgeEngineList(Iterable<String> engines)   // the edition's engines among them, in order, then each `-engine:option=value` entry
({String name, Map<String, String> options})? knowledgeFormat(Uint8List data)   // the format and options a first line names, else null
KnowledgePlan knowledgePlan(Document manifest, OverlayTree tree, [bool? user])      // served, leftOut
Future<Map<String, KnowledgeEntry>> readKnowledgeTree(FileSystem files, String root, {Settings? limits, KnowledgeSettings? settings})
String renderMustache(String source, Map<String, Object?> data, {required String path, required MustachePartials partials, String escape = 'html', KnowledgeSettings? settings})   // the one Mustache function; `typedef MustachePartials = MustachePartial? Function(String name, String fromPath)`, `MustachePartial = ({String path, String text})`
String renderEnvsubst(String text, Map<String, String> values, {String pattern = 'all', Set<String> features = const {}, String undefined = knowledgeUndefinedDefault})   // the one envsubst function
({String pattern, Set<String> features, String undefined}) envsubstOptions(Map<String, String> options)   // KnowledgeError template-error
const String knowledgeUndefinedDefault = 'debug';  const List<String> knowledgeEngines = ['envsubst', 'mustache']
String inert(String text);  List<String> knowledgeLines(String text)
List<Map<String, Object?>> searchOrder(List<Map<String, Object?>> passages);  List<String> searchCheck(overlays, arguments, response, Iterable<String> modes, {settings, caller, engines})
class KnowledgeOverlay { KnowledgeOverlay({required identity, Document manifest, required Map<String, KnowledgeEntry> tree, bool served = false, TemplateFacts facts});  KnowledgeEntry.file(bytes, {size}), .text(text), .directory(), .symlink(), .special() }
class CallerFacts { const CallerFacts({String? role, profile, Map<String, String> env, List<String> envAllow}) }
class KnowledgeSettings { KnowledgeSettings([Map<String, Limit> changes]);  KnowledgeSettings.fromData(Map<String, Object?>);  copyWith, values(), nonDefaults();  maxFileBytes, maxReadBytes, maxRenderedBytes, maxQueryCharacters, maxTop, maxContextBytes, maxContextDepth, maxIncludeDepth }
class KnowledgeError implements Exception { final String errorClass; final String message }
```

`kbRead` is the one call a consumer's `kb_read` makes: it validates the arguments, finds the file, serves its text, cuts the lines, writes the text out and returns the MCP result; a consumer never expands or renders a file itself. A markdown file (`.md`, `.markdown`, any ASCII case) names its format on its first line: `#!` and words, a word of the form `name=value` an option, the name the last other word (after any `/` or `\`, leading dots dropped, lower-cased); a file of any other name is served as stored. No name, `aos` and `agentic`: expanded, `${NAME}` replaced (never in code; an unknown name stays as written), `$$` a dollar outside code, and `@path` included from the including file's folder (a served file of the same overlay, up to `maxIncludeDepth`, a cycle is `include-error`). `markdown`, `md` and `raw`: the text after the line, byte for byte. `mustache` is standard Mustache (the `mustache_template` package, pinned) with `escape=html|none`; a partial is a served file found as an `@path` is (a missing one is empty), nested up to `maxIncludeDepth`. `envsubst` is GNU envsubst with the options `dialect`, `pattern`, `features` and `undefined` (`debug` by default); this package implements the `escape` and `default` features, and a file that asks for another is served like an engine the server lacks. Both engines read the overlay's facts, `caller` (role, profile, the environment names the user allowed with their values, never a sensitive one) and `context`. `engines` may carry, beside the engines, option values this server lacks, written `-engine:option=value` (`-mustache:escape=none`): a file whose first line gives such a value is served as written with its context, `rendered: false`, as an engine the server lacks is; `kb_list` does not list those entries. This package has no standing lack. Any other name, or an engine left out of `engines`, is served as is with its first line kept and one front matter block, `aos_template_context`, that holds those facts. `kb_list` says `engines`, and `template` and `rendered` for such a file; its `lines` and `bytes` are the served text's, and a file whose text cannot be made without a `context` (a cycle, an option of the wrong type, a format option with a value it does not have) is skipped with the reason `render`. `renderMustache` and `renderEnvsubst` are the only places either engine is called, so one can be swapped. In `knowledgeCall` and `knowledgeArguments` an integer is a `BigInt` (a plain `int` is not one, since on the web a whole `double` is an `int`); `kbRead` takes `from`, `to` and `context` as plain JSON and reads each `int` as an integer. A result's numbers are `int`. A read stops before the line that would pass `maxReadBytes` and says `truncated`. Served text is written out by `inert` (control and hiding characters become `<U+XXXX>`) and a consumer never splices it into text of its own. `readKnowledgeTree` records a link and never enters it, keeps only the start of a file past `maxFileBytes`, and leaves out names an overlay may not hold. `kb_search` is the consumer's: this package declares no mode and searches nothing, and `searchCheck` and `searchOrder` check an answer's shape and order against the served text, rendered and pass-through templates included. Every knowledge limit is a setting with no floor (a null lifts it); `nonDefaults()` reports the ones that differ. A manifest's `[knowledge]` table holds only the boolean `serve`; `knowledgePlan` says which `rules` and `context` files a served overlay leaves out of the context.

## Composition and the environment

```dart
Resolution resolve(Map<String, Document> overlays, requested);  List<String> compositionOrder(overlays, active)     // throw Cycle
Composition compose(Map<String, Document> overlays, requested, Map<String, OverlayTree> trees)
Environments environments(overlays, active, trees, String os, List<String> removed)     // throws `partial-lock`; a name with no manifest is skipped
Effective effective(Document manifest, Facts facts)
class Facts { Facts(String os);  copyWith(...);  mutable fields os, root, identity, state, pinned, baseEnv, hostEnv, grants, secrets, secretFiles, providers, providerValues, enforcesNetwork, envScriptsEnabled, gateEnvScripts, digest, allowed, interpreters, packages, tree, settings }
class Environment { Environment([Iterable<(String, String)> base, bool windows]);  settings(s), get, set(name, value, secret), unset, values(), isSecret, clone }
ComposedEnvironment composedEnvironment(List<(Document, Facts)> active, Environment base, Map<String, HookRun> preEnv, List<NonDefaultSetting> nonDefaults)
Preparation preparation(change, record, digest, setup, dependencies, fails);  String? setupPhase(change, recorded, digest)
Map<String, String> SetupRecord.toJson();  SetupRecord.fromJson(Object? json)   // {"digest", "phase"}; a FormatException for anything else;  phaseWords.named(word)
String substitute(String text, Lookup lookup);  String substituteWith(String text, Lookup lookup, Settings settings)
```

`Facts.copyWith` treats a null argument as "keep"; to clear a nullable field, copy and assign. `Effective` holds `ungranted`, `supported`, `env`, `envScripts`, `setup`, `python`, `node`, `dependencies`, `needs`, `nonDefaults` and `reports` (`(id, subject)`: `env-script-not-allowed`) (`NonDefaultSetting(id, subject)`: the spec's reports such as `unpinned` and `env-sensitive`, never a value). An `Environment` never lets a placeholder read a variable that holds a resolved secret. Substitution refuses `too-large`, `too-deep` and `too-complex` past `substitution*`; a placeholder's variable or pattern is never quoted. With `Facts.gateEnvScripts` an overlay whose `digest` is not its `allowed` one (the allowance of its hooks; a missing digest is no match) runs no env script, declared or default, and is reported `env-script-not-allowed` when it has one for the OS; the rest of it applies. A lookup may throw an `AosError`.

## Catalogs and icons

```dart
Catalog readCatalog(Uint8List data, String name, String? base, Settings settings);  Document readCatalogDocument(Uint8List data, String name, Settings settings)
String? catalogFormat(String name);  Listing listCatalogs(List<(String, Catalog)> catalogs);  ListedEntry chooseListed(Listing listing, requested)
Icon readIcon(Uint8List data, String name, Settings settings)      // Icon(format, width, height), judged from the header alone
```

`name` is the catalog's file name, whose extension picks the format; `base` is the source string it came from. A refused entry is listed in `refused` by name. A `description` is a stranger's text: do not render it as markup. Nothing is fetched.

## Skills from elsewhere, and hooks

```dart
SkillPlan resolveSkills(overlays, requested, trees, catalogs, List<String> order, bool fetches, Settings settings)     // SkillPlan.toFetch: what to fetch
FetchedSkill readFetchedSkill(String name, List<(String, Uint8List)> files, String? pin, Settings settings)     // FetchedSkill(digest, reported, files)
abstract interface class SkillSource { FutureOr<List<(String, Uint8List)>> fetch(Source source) }
Future<List<FetchedPlanned>> fetchPlanned(SkillPlan plan, SkillSource fetcher, Settings settings)     // one at a time; FetchedPlanned.ok, .skill, .error
hooksOf, bool hooksDeclared(Document manifest, OverlayTree tree) /* any hook on any OS with any interpreter */, HookPlan hookPlan(Document manifest, files, HookFacts facts, Settings settings);  hookMatches, hookPayload, readHookOutput, HookOutput hookResult(event, HookRun run, Settings settings);  FiredEvent fireEvent(event, runs, applies, settings, windows, [int contextUsed = 0, Object? toolInput, String? toolKind]);  List<(String, String)> hookOrder(overlays, requested, has, scopes);  Environment hookEnvironment(event, active, base, preEnv, String? identity, [List<EnvSet> earlier = const []]);  Environment processEnvironment(String process /* setup, server, command, env-script */, active, base, preEnv, String identity)
HookRun hookRun(String source, [Uint8List? stdout])        // fields: exit, seconds, timedOut, notRun, payloadSize, sets, onFailure
```

The core decides what to fetch and checks what came back, pin first, so nothing unverified is parsed; it holds no network. Running a hook is the caller's: it builds a `HookRun` from what the process did and the core checks it in a fixed order (started, payload, time, size, exit, output), combines the runs of one event to the tightest decision (a block or deny beats an ask, an ask beats an allow; `'allow'` is a `pre-tool` hook approving the call, which never lifts the user's own refusals) and reports what could not be applied. `FetchedSkill.files` are the paths the digest covers: write those and nothing else, since the digest leaves out entries under an excluded name (`node_modules`, `__pycache__`, `*.pyc`, `.git`). A `pre-env` hook's output is read exactly as an env script's. A hook is off until the user allows it at the overlay's digest (`HookFacts.allowed`); `HookPlan.activates` is false for a guard that is off. Context a hook adds counts in UTF-8 bytes, with `contextUsed`, against `contextBytes`: a part that would pass it is not added, whole, and is reported `hook-not-applied`, and a later part that fits still is. `hookOrder` is the overlays that have a hook in composition order, then the user's, then the project's; `hookEnvironment` is the session's environment as one hook sees it: every overlay's declarations stacked in composition order with the top layer winning, every overlay's `pre-env` output, and what the earlier hooks of the event set (`earlier`, one `EnvSet`, a list of `(name, value?)`, each, in run order); a `pre-env` hook sees the root and state variables and the `pre-env` output before it, and no declaration. `processEnvironment` is the same for an overlay's `setup`, `server` or `command` (the finished stack) and `env-script` (the stack as it stands at that overlay's step). A `pre-tool` hook may print `decision: allow` with an `input` object that replaces the call's (`HookOutput.input`); pass the call's input as `toolInput` and `FiredEvent.seen` says what each hook was given and `FiredEvent.input` what the call runs with, set only for a combined allow, since an ask or a deny beats a rewrite (an ask reports it `hook-not-applied` `input`). An `allow` or an `input` outside `applies` is not applied and is reported; so is the allow of a hook whose `HookRun.select` (its entry's `match`) is given and does not name `other`, on a call whose `toolKind` is `other` (its deny and ask still count). Any decision the event does not take is an `output` failure; there is no `grant` cause.

## Gotchas

- JSON decoded as `Map<String, Object?>` is not a `Document` at run time (`is Map<String, Object>` fails): build documents with the readers, or copy with `fromPlain`.
- Equal mapping keys are refused, and a validator names the **later** of two keys equal under case (`env-name-case-duplicate`), so mappings keep insertion order.
- `readTree` and `treeDigest` are the only places the core asks for a file, and they are `Future`s because a caller's `FileSystem` may be. Everything else is synchronous, except `fetchPlanned`.
- The YAML parser's cost grows with the square of flow nesting, so a document with flow collections nested past `manifestDepth` is refused before it is scanned.
- The tests read the specification's vectors from `../vectors`, which are not in the package.
