# `coqui-aos` — API header

Header-file-style reference for the `coqui-aos` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-aos>.

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `fs` | `StdFs`, `std::fs` as a `FileSystem` | reading a directory from the operating system | on |
| `serde` | `Serialize` and `Deserialize` for `SetupRecord` and `Phase` (`{"digest": ..., "phase": "install"}`) | keeping a setup record | off |
| `test-support` | `coqui_aos::vectors`: the specification's conformance vectors as a `Runner` (`std`, and `base64` for the vectors' bytes) | a consumer's own tests | off |

MSRV Rust 1.89. Version `0.0.0-alpha.0`, not published; the API may change in any release before 1.0. A default build depends on `granit-parser` (the YAML parser `serde-saphyr` is built on), `guatiao` (the derives that describe `Settings`), `serde_json`, `sha2` and `toml`. `--no-default-features` compiles for `wasm32-unknown-unknown`. `serde_json` is re-exported as `coqui_aos::serde_json`, because the data model is its `Value` and `Map`, and its `preserve_order` feature is on for every crate in the build.

Imports: everything is at the root. No `unsafe` code (`#![forbid(unsafe_code)]`). No process-wide state; the library prints nothing, reads no environment variable and starts no thread. Edition: AOS v0, `0.1.0-draft`.

Reading a manifest is static: no function here runs a script, reads a file, opens a connection or consults the environment, except `tree_digest` and `read_tree`, which read through the `FileSystem` they are given.

## Errors

```rust
#[non_exhaustive] pub struct Error { pub class: Class, pub subject: String, pub line: Option<usize>, pub column: Option<usize>, pub setting: Option<&'static str> }
pub fn Error::new(class: Class, subject: impl Into<String>) -> Error
pub fn Error::at(self, line: usize, column: usize) -> Error
pub fn Error::on_line(self, line: usize) -> Error
pub fn Error::over(self, setting: &'static str) -> Error   // names the Settings field whose limit was passed
```

`Clone`, `Debug`, `Eq`, `Display` (`class: subject at line N column M`), `std::error::Error`. **`class` is the contract**: `Class`, a `#[non_exhaustive]` enum of the classes the specification's conformance vectors name (`ParseError`, `DuplicateKey`, `TooDeep`, `Symlink`, `UndefinedVariable`, ...) and this crate's `Io`, `DuplicateMember`, `InvalidRequirements` and `InvalidDigest`; `Class::as_str` is the word the vectors write and `Class::named` reads it back. `Issue::code` is `IssueCode`, the same for a manifest's problems. `subject` names what it is about (a variable, a path, a pointer) for a person, never text from a document or an operating system's message; do not match on it. `Error::new` writes each control, format, separator, surrogate, private-use and unassigned character of it as `\xNN`, `\uNNNN` or `\UNNNNNNNN` and cuts it to 256 characters, then `...`. `line` and `column` are one-based, `column` counts Unicode scalar values; a line counts from the first line of the file, a format line included. **`setting`** is set on a refusal of a limit (`too-large`, `too-deep`, `too-complex`, `icon-too-large`, `icon-dimensions`) to the key of the `Settings` field that was passed (`tree_files`, `tree_depth`, `tree_bytes`, `file_bytes`, `manifest_bytes`, `manifest_depth`, `catalog_bytes`, `context_bytes`, `icon_bytes`, `icon_width`, `icon_height`, `substitution_bytes`, `substitution_depth`, `substitution_steps`): stable, unlike `subject`'s wording, and `None` for any other refusal.

Functions that cannot refuse return no `Result`; the others return `Result<_, Error>`, except `compose` (`Cycle`) and `dependencies_of` (`PartialLock`), which return the structure the vectors expect, and both implement `Display` and `std::error::Error`. The closed sets of words are enums, each `#[non_exhaustive]` with `as_str` and `named`: `Runtime`, `LeftState`, `ManifestFile`, `Format`, `FailStep`, `NonDefaultId`, `Decision`, `HookEvent`, `HookFailure`, `OnFailure`, `HookScope`, `PlannedOrigin`, `CatalogForm`, `IconFormat` and `EntryRefusal`; `Step` is an enum with data and implements `Display`. Reports and non-default settings in the plan and event types are `(id, subject)` pairs of text.

## Settings (`Settings`, `Limit`)

```rust
#[non_exhaustive] pub struct Settings { pub manifest_bytes, manifest_depth, tree_files, tree_depth, tree_bytes, file_bytes, context_bytes, catalog_bytes, icon_bytes, icon_width, icon_height, hook_timeout, hook_event_timeout, hook_output_bytes, hook_payload_bytes, hook_reason_chars, substitution_bytes, substitution_depth, substitution_steps: Limit }   // Default, Copy, Eq
// defaults: 1 MiB, 64, 4096, 64, 64 MiB, 16 MiB, 1 MiB, 4 MiB, 512 KiB, 1024, 1024, 60 s, 120 s, 64 KiB, 1 MiB, 1024 characters, 1 MiB, 32, 10_000_000
pub fn Settings::manifest_bytes(self, limit: Limit) -> Settings   // and one setter per field, named for it
pub fn Settings::non_defaults(&self) -> Vec<NonDefault>           // every setting that differs, read from the described type
pub struct NonDefault { pub setting: String, pub value: String, pub default: String }
pub struct Limit(..)                                               // Copy, Eq, Display ("none" or the number)
pub const Limit::UNBOUNDED: Limit
pub const fn Limit::at(n: u64) -> Limit
pub const fn Limit::get(self) -> Option<u64>
pub fn Limit::allows<N: TryInto<u64>>(self, n: N) -> bool
```

Every limit is a setting with a default and no floor; `Limit::UNBOUNDED` lifts one, and `non_defaults` reports it. `Settings` derives guatiao's `ToValue`, `FromValue` and `Schema` (`deny_unknown_fields`; a limit is a whole number, or `null` or `"none"` for no bound). `manifest_*` bound a manifest, an env file and an env script's output; `tree_files` counts files and directories together, excluded names not counted; `tree_depth` counts directory levels, the root's own being the first. One bound is not a setting: the nesting of parentheses in a license expression, 128.

Lifting `manifest_depth` leaves the nesting of what is read to the caller's stack when it later drops, clones or prints the `Value`; the reader itself builds without recursion.

## Reading (`read_document`, `read_front_matter`, ...)

```rust
pub type Document = serde_json::Map<String, serde_json::Value>;
pub fn read_document(data: &[u8], settings: &Settings) -> Result<Document, Error>
pub fn parse_dotenv(data: &[u8]) -> Result<Vec<DotenvLine>, Error>        // DotenvLine { name, text, literal }
pub fn read_env_script_output(data: &[u8], settings: &Settings) -> Result<ScriptOutput, Error>   // Vec<(String, Option<String>)>, None unsets
pub fn read_dotenv(data: &[u8], env: &mut Environment) -> Result<Vec<(String, String)>, Error>
pub fn split_front_matter<'a>(data: &'a [u8], settings: &Settings) -> Split<'a>
pub fn read_front_matter(data: &[u8], settings: &Settings) -> FrontMatter
pub fn context_file_text<'a>(data: &'a [u8], settings: &Settings) -> Result<&'a [u8], Error>
pub fn read_overlay(tree: &OverlayTree, settings: &Settings) -> Result<OverlayRead, Error>
```

`read_document`: one optional byte-order mark, an optional format line (`#!json`, `#!/usr/bin/env yaml`), else TOML; then the data model. **Dialect**: a duplicate key in one mapping, a null, a TOML date or time, a non-finite float, an integer outside the signed 64-bit range (refused before it is converted), a mapping key that is not a string, a lone surrogate escape in JSON and a byte-order mark where the text starts are refused. A JSON `30.0` is a float and `30` an integer, whatever their size. In YAML a plain scalar is typed by the 1.2 core schema (`yes` is a string), and a tag, anchor, alias, merge key, `%YAML` or `%TAG` directive and a second document are refused at the parser's events, before anything is expanded. Nesting past `manifest_depth` is `too-deep` while parsing, in every format; a parser's own failure on text deeper than the bound is reported as `too-deep` too. Mapping order is the document's.

`split_front_matter` splits on bytes, unchanged: `Split { case: SplitCase, metadata, metadata_offset, metadata_line, body_offset, body_line }` with `Split::body(data)`; offsets count bytes from the first byte of the file, lines from 1. `read_front_matter` adds the reading: `FrontMatter { case: Case, data: Option<Document>, error: Option<Error>, format, body_offset, body_line }`; `error` is what a gate refuses with (its `line` counted from the file's first line), `case` what a viewer shows. `Case`: `None`, `Malformed`, `Unterminated`, `TooLarge`, `Empty`, `UnknownFormat`, `DoesNotParse`, `NotAMapping`, `Mapping`. The head, through the closing fence's line end, is bounded by `manifest_bytes`, and nothing past it is read.

`read_overlay` takes an `OverlayTree` holding the bytes of `OVERLAY.md` or `overlay.cfg` and returns `OverlayRead { manifest_file, manifest, context, context_ignored, issues, harness_context }`; `issues` are JSON pointers. It refuses two manifests (`two-manifests`), a manifest that does not read, and a context body past `context_bytes` or not UTF-8.

## Validation (`validate`)

```rust
pub fn validate(manifest: &Document, tree: Option<&OverlayTree>) -> Vec<Issue>   // Issue { code: &'static str, at: String }, sorted by pointer
pub fn applies(issues: &[Issue]) -> bool          // only problems under /about leave a manifest applying
pub fn named_files(doc: &Document) -> Vec<(String, String)>   // (pointer, relative path) of each file a field requires
pub fn skill_flags(doc: &Document, tree: &OverlayTree) -> Vec<String>   // pointers of skill names neither bundled nor sourced
pub fn valid_license(expression: &str) -> bool
```

`at` is an RFC 6901 pointer with `~` and `/` escaped. With a tree, files a field names must be in it. The `hooks` field is not validated yet.

## Templating (`template_values`, `template`, `context_text`, `skill_text`)

```rust
#[non_exhaustive] pub struct TemplateFacts { pub root, state, name, scope, project: Option<String>, pub options: Document }   // Default; TemplateFacts::new().root(..).state(..).name(..).scope(..).project(..).options(..)
pub fn template_values(manifest: &Document, facts: &TemplateFacts) -> Result<Template, Error>   // invalid-option
pub fn template(text: &str, values: &Template) -> String;  Template::get(&self, name: &str) -> Option<&str>
pub fn context_text(data: &[u8], values: &Template, settings: &Settings) -> Result<String, Error>   // front matter dropped; too-large, invalid-utf8
pub fn skill_text(data: &[u8], values: &Template, settings: &Settings) -> Result<String, Error>     // front matter as written
```

`${NAME}` is replaced in the context a consumer assembles and in the body of a bundled skill it serves, never in an installed file, and only for `AOS_OVERLAY_ROOT`, `AOS_OVERLAY_STATE`, `AOS_OVERLAY_NAME`, `AOS_OVERLAY_SCOPE`, `AOS_PROJECT_ROOT` and the overlay's `options` that have a value; a name with no value, an unknown name, one with an operator and a sensitive option are left as written, `$${` is `${`, and a value is never read again. A skill's front matter is served as written. A configured value of the wrong type is `invalid-option`, never converted. The bound on the context counts the text after replacing: the caller checks it.

`options` is validated by `validate` (`option-default-type`, `option-sensitive-default`, a reserved or malformed name is `schema`). `substitute` and its operators are for env values and are unchanged.

## Names, sources, digests

```rust
pub fn valid_overlay_name(name: &str) -> bool
pub fn normalize_name(name: &str) -> String        // ASCII lowercase, `_` and `.` made `-`
pub fn root_variable(identity: &str) -> String     // NAME_OVERLAY_ROOT; state_variable: NAME_OVERLAY_STATE
pub fn valid_relative_path(path: &str) -> bool
pub fn is_variable_name / is_reserved_variable / is_sensitive_variable(name: &str) -> bool
pub fn Source::parse(text: &str) -> Result<Source, Error>   // [git+]<location>[@<ref>][#<path>]
#[non_exhaustive] pub struct Source { pub kind: SourceKind, pub location: String, pub reference: Option<String>, pub path: Option<String> }
pub fn Source::shown(&self) -> String              // no user information or query; Debug shows the same
pub fn Source::is_local / holds_credentials / is_web_url(&self) -> bool
pub struct Digest(..)                              // Copy, Ord, Display `sha256:<hex>`, FromStr, Digest::of(bytes), Digest::parse(text)
pub fn digest_entries<'a, I>(entries: I) -> Result<TreeDigest, Error>   // I: (Vec<&[u8]> components, &TreeEntry); order does not matter; a path twice is `duplicate-member`
pub fn tree_digest(files: &dyn FileSystem, root: &Path, settings: &Settings) -> Result<TreeDigest, Error>
pub fn read_tree(files: &dyn FileSystem, root: &Path, settings: &Settings) -> Result<(OverlayTree, TreeDigest), Error>   // every member's bytes and the digest of one walk
pub enum TreeEntry { File(Vec<u8>), Directory, Symlink, Special }
pub struct TreeDigest { pub digest: Digest, pub files: BTreeMap<String, Digest> }   // lines() gives the bytes that were hashed
```

Case folding of identities, variables and sensitivity is ASCII only; substitution's `^^` and `,,` operators and skill-name equality fold by Unicode rules, which the runtime's tables decide.

## The filesystem (`FileSystem`, `StdFs`)

```rust
pub trait FileSystem {
    fn metadata(&self, path: &Path) -> io::Result<Metadata>;          // WITHOUT following a link: a link is FileKind::Symlink
    fn read(&self, path: &Path, max: u64) -> io::Result<Vec<u8>>;     // the first `max` bytes
    fn list(&self, path: &Path) -> io::Result<Vec<OsString>>;         // single path components, any order
}
pub enum FileKind { File, Directory, Symlink, Other }     // non_exhaustive
pub struct Metadata { pub kind: FileKind, pub len: u64 }   // Metadata::new(kind, len)
pub struct StdFs;                                          // feature `fs`; a reparse point is a Symlink on Windows
```

`tree_digest` checks every entry with `metadata` before it reads or descends, and refuses a listed name that is empty, `.`, `..` or holds `/`, `\` or NUL. `StdFs::read` and `list` open through links, as `std::fs` does: a link swapped in between `metadata` and `read` is followed, so read only trees no one else can write. A name that is not Unicode is refused as `NonUtf8Name`.

## What an overlay's files contribute

```rust
pub struct OverlayTree     // paths and the bytes of those read; Default, from_paths, from_texts, with_text, with_bytes, paths, contains, text, bytes
pub fn rules_of / context_of / paths_of / env_files_of / skills_of(manifest: &Document, tree: &OverlayTree) -> ...
pub fn default_paths(manifest, tree) -> Vec<Value>;  pub fn default_on(manifest: &Document, key: &str) -> bool
pub fn context_ignored(manifest, tree) -> Vec<String>;  pub fn harness_context_of(manifest, harness: &str, tree) -> Vec<String>
pub fn skill_entries(manifest, tree) -> Vec<SkillEntry>   // SkillEntry { name, origin: SkillOrigin, source, key, pin }
pub fn dependencies_of(manifest, tree, os: &str) -> Result<Vec<Dependency>, PartialLock>
pub fn check_requirements(text: &str, locked: bool) -> Result<(), Error>   // `invalid-requirements`; never quotes the line
```

`with_text` ignores a path the tree does not hold; `with_bytes` adds it. The body of `OVERLAY.md` is the context default when its text was read into the tree.

## Composition and the environment

```rust
pub fn resolve(overlays: &BTreeMap<String, Document>, requested: &[String]) -> Result<Resolution, Cycle>
pub fn composition_order(overlays, active: &[String]) -> Vec<String>
pub fn compose(overlays, requested, trees: &BTreeMap<String, OverlayTree>) -> Result<Composition, Cycle>
pub fn environments(overlays, active, trees, os: &str, removed: &[String]) -> Result<Environments, Error>   // `partial-lock`; a name with no manifest is skipped
pub fn effective(manifest: &Document, facts: &Facts) -> Result<Effective, Error>
pub fn Facts::new(os: impl Into<String>) -> Facts   // pub fields: os, root, identity, state, pinned, base_env, host_env, grants, secrets, secret_files, providers, provider_values, enforces_network, env_scripts_enabled, gate_env_scripts, digest, allowed, interpreters, packages, tree, settings
pub struct Environment                               // new(base, windows), settings(Settings), get, set(name, value, secret), unset, values, is_secret
pub fn composed_environment(overlays: &[(&Document, Facts)], base: Environment, pre_env: &BTreeMap<String, HookRun>, non_defaults: &mut Vec<NonDefaultSetting>) -> ComposedEnvironment
pub fn preparation(change: Change, record: Option<&SetupRecord>, digest: Option<&str>, setup: Option<&SetupRun>, dependencies: &[Dependency], fails: Option<FailStep>) -> Preparation
pub fn setup_phase(change: Change, recorded: Option<&str>, digest: Option<&str>) -> Option<Phase>
pub fn Phase::named(name: &str) -> Option<Phase>      // exactly install, sync or remove; Phase::name is the way back
pub fn NonDefaultSetting::new(id: NonDefaultId, subject: impl Into<String>) -> NonDefaultSetting   // a host reports its own beside effective's
pub fn NonDefaultSetting::host(id: &'static str, subject: impl Into<String>) -> NonDefaultSetting  // a host's own id (`read-only-not-enforced`); the AOS id when AOS defines the word
NonDefaultId::Host(&'static str)   // as_str, Display and Ord give the word as given; NonDefaultId::host(word) builds it
pub fn substitute(text: &str, lookup: Lookup) -> Result<String, Error>;  pub fn substitute_with(text, lookup, settings: &Settings) -> Result<String, Error>
```

`Effective` holds `ungranted`, `supported`, `env`, `env_scripts`, `setup`, `python`, `node`, `dependencies`, `needs`, `non_defaults` and `reports` (`(id, subject)`: `env-script-not-allowed`) (`NonDefaultSetting { id, subject }`: the spec's reports such as `unpinned` and `env-sensitive`, never a value). An `Environment` never lets a placeholder read a variable that holds a resolved secret. Substitution refuses `too-large`, `too-deep` and `too-complex` past `substitution_*`; a placeholder's variable or pattern is never quoted. With `Facts::gate_env_scripts` an overlay whose `digest` is not its `allowed` one (the allowance of its hooks; a missing digest is no match) runs no env script, declared or default, and is reported `env-script-not-allowed` when it has one for the OS; the rest of it applies. `effective` and `composed_environment` refuse (`InvalidReference`, naming it) a variable name that is no name and a path that leaves the overlay before anything is applied, whatever `validate` was asked.

## Catalogs and icons

```rust
pub fn read_catalog(data: &[u8], name: &str, base: Option<&str>, settings: &Settings) -> Result<Catalog, Error>
pub fn read_catalog_document(data: &[u8], name: &str, settings: &Settings) -> Result<Document, Error>
pub fn catalog_format(name: &str) -> Option<&'static str>      // by extension: toml, json, yaml
pub struct Catalog { pub form: CatalogForm, pub name, description, homepage: Option<String>, pub overlays: CatalogSection, pub skills: Option<CatalogSection>, pub ignored: Vec<String> }
pub struct CatalogSection { pub entries: BTreeMap<String, CatalogEntry>, pub refused: Vec<(String, EntryRefusal)> }
pub struct CatalogEntry { pub source: Source, pub pin, description: Option<String>, pub about: Option<Document> }
pub fn list_catalogs(catalogs: &[(&str, &Catalog)]) -> Result<Listing, Error>   // `invalid-label`
pub fn Listing::choose(&self, requested: &str) -> Result<&ListedEntry, Error>   // `not-found`, `ambiguous`
pub fn read_icon(data: &[u8], name: &str, settings: &Settings) -> Result<Icon, Error>   // Icon { format: IconFormat, width, height }
```

`name` is the catalog's file name, whose extension picks the format (any other is an AOS document); `base` is the source string the file came from, against which relative sources resolve (a directory, a Git repository, or a URL). An entry that is refused is listed in `refused` by name; a catalog with neither an `overlays` nor a `skills` table is `not-a-catalog`. Nothing is fetched. An icon is judged from its header alone: extension, size, signature, dimensions.

## Skills from elsewhere

```rust
pub fn resolve_skills(overlays, requested, trees: &BTreeMap<String, OverlayTree>, catalogs: &[(&str, &Catalog)], order: &[&str], fetches: bool, settings: &Settings) -> Result<SkillPlan, Error>
pub struct SkillPlan { pub skills: Vec<PlannedSkill>, pub missing, ambiguous, unpinned, non_defaults, not_fetched, name_mismatch, shadowing, delivered, collisions, pub to_fetch: Vec<Fetch> }
pub fn read_fetched_skill(name: &str, files: &[(String, Vec<u8>)], pin: Option<&str>, settings: &Settings) -> Result<FetchedSkill, Error>
pub trait SkillSource { fn fetch(&self, source: &Source) -> Result<Vec<(String, Vec<u8>)>, Error>; }
pub fn fetch_planned(plan: &SkillPlan, fetcher: &dyn SkillSource, settings: &Settings) -> Vec<FetchedPlanned>
```

The core decides what to fetch (`SkillPlan::to_fetch`: the delivered skills from elsewhere, none for a consumer that does not fetch) and checks what came back, pin first, so nothing unverified is parsed; it holds no network. `catalog` entries' `description` is a stranger's text: do not render it as markup. A `SkillSource` is the caller's; `fetch_planned` calls it once per skill and reports its `Error` for that skill alone. `read_fetched_skill` needs the front matter of `SKILL.md` in YAML and its `name` to be the skill's, and reports every key beyond `name`, `description`, `license`, `compatibility` and `metadata`. `FetchedSkill::files` are the paths the digest covers, in byte order: a caller writes those and nothing else, since a fetched tree may also carry entries under an excluded name (`node_modules`, `__pycache__`, `*.pyc`, `.git`), which the digest leaves out and are not refused.

## Hooks

```rust
pub fn hooks_declared(manifest: &Document, tree: &OverlayTree) -> bool   // any hook on any OS with any interpreter: for a consumer that runs none and refuses an overlay that asks
pub fn hooks_of(manifest, tree: &OverlayTree, os: &str, interpreters: &[String]) -> (Vec<Hook>, Vec<String>)   // hooks, and the hook.<ext> files reported `script-not-run`
pub fn hook_plan(manifest, files: &OverlayTree, facts: &HookFacts, settings: &Settings) -> Result<HookPlan, Error>
pub fn hook_matches(event: HookEvent, select: Option<&[String]>, value: &str) -> bool
pub fn hook_payload(event: HookEvent, known: &Map<String, Value>) -> Map<String, Value>
pub fn read_hook_output(event: HookEvent, data: &[u8], sets: &[String], settings: &Settings) -> Result<HookOutput, HookFailure>
pub fn hook_result(event: HookEvent, run: &HookRun, settings: &Settings) -> Result<HookOutput, HookFailure>
pub fn fire_event(event: HookEvent, runs: &[HookRun], applies: Option<&[&str]>, settings: &Settings, windows: bool) -> FiredEvent
pub fn fire_event_after(event, runs, applies, settings, windows, context_used: usize) -> FiredEvent   // the session already holds `context_used` bytes
pub fn fire_event_for_call(event, runs, applies, settings, windows, context_used: usize, tool_input: Option<&Value>, tool_kind: Option<&str>) -> FiredEvent   // a `pre-tool` call whose input an allow may rewrite: `FiredEvent::seen`, `FiredEvent::input`; on kind `other` a run's allow counts only if its `HookRun::select` is `None` or names `other`
pub fn hook_order(overlays: &BTreeMap<String, Document>, requested: &[String], has: &[String], scopes: &[HookScope]) -> Result<Vec<(HookScope, String)>, Cycle>
pub fn hook_environment(event: HookEvent, active: &[(&Document, Facts)], base: &Environment, pre_env: &BTreeMap<String, HookRun>, identity: Option<&str>, earlier: &[EnvSet]) -> Result<Environment, Error>   // `None`: a user's or a project's own hook; `earlier`: what each earlier hook of the event set, in run order
pub fn process_environment(process: OverlayProcess, active: &[(&Document, Facts)], base: &Environment, pre_env: &BTreeMap<String, HookRun>, identity: &str) -> Result<Environment, Error>   // an overlay's setup, server, command or env script
pub type EnvSet = Vec<(String, Option<String>)>;   // `None` unsets
pub enum OverlayProcess { Setup, Server, Command, EnvScript }   // #[non_exhaustive]
```

Running a hook is the caller's: it builds a `HookRun` (`HookRun::new(source, stdout)` and its public fields: exit status, seconds, `timed_out`, payload size, `sets`, `select` (its entry's `match`), `on_failure`) from what the process did, and the core checks it in a fixed order (started, payload, time, size, exit, output), combines the runs of one event to the tightest decision (a block or deny beats an ask, an ask beats an allow; `Decision::Allow` is a `pre-tool` hook approving the call, which never lifts the user's own refusals), and reports what could not be applied. A hook is off until the user allows it at the overlay's digest (`HookFacts::allowed`; `HookFacts::tree` is a project's files by path and bytes, and its digest replaces `digest`); call `validate` first: a `run` that is no file of the overlay is a validation problem, not something `hooks_of` filters. `Debug` of a run, an output or an event shows no output text or value; `HookPlan::activates` is false for a guard that is off, so it never fails open. `composed_environment` takes the runs of each overlay's `pre-env` hook and returns a `ComposedEnvironment { env: Result<Environment, Error>, hook_reports, hook_non_defaults }`; a guard that failed is `HookRefused`. The hooks' bounds are `Settings::hook_*`. Context a hook adds counts in UTF-8 bytes against `Settings::context_bytes`: a part that would pass it is not added, whole, and is reported `hook-not-applied`, and a later part that fits still is. `hook_order` is the overlays that have a hook in composition order, then the user's, then the project's; `hook_environment` is the session's environment as one hook sees it: every overlay's declarations stacked in composition order with the top layer winning, every overlay's `pre-env` output, and what the earlier hooks of the event set; a `pre-env` hook sees the root and state variables and the `pre-env` output before it, and no declaration. `process_environment` is the same for an overlay's `Setup`, `Server` or `Command` (the finished stack) and `EnvScript` (the stack as it stands at that overlay's step). A `pre-tool` hook may print `decision = "allow"` with an `input` object that replaces the call's (`HookOutput::input`); `fire_event_for_call` tracks it: a later hook is given the rewritten input (`FiredEvent::seen`), and `FiredEvent::input` is what the call runs with, set only for a combined allow, since an ask or a deny beats a rewrite (an ask reports it `hook-not-applied` `input`). An `allow` or an `input` outside `applies` is not applied and is reported; so is the allow of a hook whose `select` is given and does not name `other`, on a call of kind `other` (its deny and ask still count). Any decision the event does not take is an `Output` failure; there is no `Grant` cause.

## The vectors as a runner (`test-support`)

```rust
pub struct vectors::Runner;   pub struct vectors::Report { pub file: String, pub passed: usize, pub skipped: Vec<String>, pub failed: Vec<String> }
pub const Runner::FILES: [&str; 12]
pub fn Runner::new(dir: impl Into<PathBuf>) -> Runner       // the specification's tests/vectors/ or a copy
pub fn Runner::skip(self, f: impl Fn(&str /*file*/, &Value /*case*/) -> Option<&'static str /*reason*/>) -> Runner
pub fn Runner::effective(self, f: impl Fn(&Document, &Facts) -> Result<Effective, Error>) -> Runner   // a host's wrapper of `effective`
pub fn Runner::host_non_defaults(self, settings: Vec<NonDefaultSetting>) -> Runner   // what that wrapper adds; each is taken out once before comparing
pub fn Runner::run_file(&self, file: &str) -> Option<Report>;  Runner::run_all(&self) -> Vec<Report>;  Runner::unrun(&self) -> Vec<String>
pub fn Report::is_clean(&self) -> bool;  Report::assert_clean(&self)   // panics naming each failing case
```

`Runner` runs every case of the vector files in a directory the caller names against this crate and reports; nothing is read but those files. `unrun` lists the `.json` files there that have no runner here (a specification change nothing checks yet). A setting the wrapper adds that `host_non_defaults` does not name fails the `effective.json` cases, so a host cannot weaken silently.

## Gotchas

- Equal mapping keys are refused, and a validator names the **later** of two keys equal under case (`env-name-case-duplicate`), which needs the document's order: `serde_json`'s `preserve_order` is on.
- A `Document` read from YAML, TOML and JSON is the same value; only the format line and `OVERLAY.md`'s front matter choose the parser.
- `tree_digest` and `read_tree` are the only places the core asks for a file. `StdFs` is the one `FileSystem` shipped; a binding supplies its own.
- The tests read the specification's vectors from `../vectors`, which are not in the packaged crate; the runner that reads them is `vectors::Runner`, behind `test-support`.
