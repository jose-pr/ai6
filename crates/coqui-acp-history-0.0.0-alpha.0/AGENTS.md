# `coqui-acp-history` — API header

Header-file-style reference for the `coqui-acp-history` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. The format itself is specified in `docs/session-history.md` of <https://github.com/Boriken-Dev/coqui-acp>, with shared vectors in `vectors/session_history/` that a Dart or Python reader runs too.

Cargo feature `cli` (off by default) builds the `coqui-acp-history` command and pulls in `clap`. MSRV Rust 1.89. Default dependencies: `agent-client-protocol` (for ACP's `StopReason` and `ToolKind`), `guatiao` and `guatiao-serde` (settings, and the JSON text of `non_defaults`), `regex`, `serde`, `serde_json` (with `float_roundtrip`, so a double reads back exactly), `sha2`, `data-encoding`. Version `0.0.0-alpha.0`, not published; the API may change in any release before 1.0.

Imports: everything is at the crate root except the tool table, `coqui_acp_history::tools`, and the modules `conversation`, `paging`, `patch`, `record` and `replay`. `StopReason` is ACP's (`agent_client_protocol::schema::v1`), re-exported.

No `unsafe` code (`#![forbid(unsafe_code)]`). No process-wide state. Text only: the crate reads and writes UTF-8 JSON Lines; it parses no harness's own files (the Claude and Codex codecs live in `coqui-acp-claude` and `coqui-acp-codex`, behind their `history` feature, and are built on `conversation` below).

Dialect: strict UTF-8 and strict JSON; one leading byte-order mark ignored on line 1 and none elsewhere; a repeated object key, a carriage return, an empty line, a missing final line feed, `NaN`/`Infinity`, a lone surrogate escape and a number no double holds (`1e400`) are refused; an integer outside `i64`/`u64` is read as the nearest `f64`; nesting stops at `PARSER_DEPTH` (127) levels whatever `max_depth` is. Written numbers follow the format document's canonical rule (`100.0`, `1e16`, `-0.0`), not `serde_json`'s.

Memory: a parsed line costs 3 to 4 times its bytes for text and up to about 60 times for a line of tiny objects. `Bounds::max_values` (default 500,000, keys counted) bounds one line while it is parsed. `Document` holds every line at once, so `Document::from_slice` and `str::parse` read under `ReaderSettings::for_document()` (16 MiB total); a larger history is read with a `Reader`, or with `Document::from_reader` under bounds you chose. An importer passes bounds of its own.

Untrusted text: a header's `model`, `mode`, `effort`, `cwd`, `origin.version` and `origin.sessionId` hold no control character, and `cwd` is an absolute path in the Unix or the Windows style. They describe the recording: **never build a path from `cwd`, `origin.sessionId`, a `_meta` native id or any text of a line**; use the host's session UUID (the header's `id`) and the working directory the host gives. Print anything from a history with its control and bidirectional characters escaped, as the command does.

## The format in brief

Line 1 is the header (`format` `coqui-acp-history`, `version` 1, `id` the host's session UUID, `origin.harness` and optionally `origin.version`, `origin.sessionId`, `createdAt`, `model`, `mode`, `effort`, `cwd`, `acpVersion`, `digest`, `redacted`, `_meta`). Each body line has a `method`: `session/prompt`, `session/update`, `session/cancel`, `session/request_permission`, `history/permission_answer`, `history/turn_end` (stop reason is ACP's, or `error`), `history/compaction` (optional `summary`, `trigger`, `preTokens`). A line may carry `at` and `_meta`. A prompt opens a turn and is refused while one is open; a turn end, cancel, permission request and answer need an open turn; updates and compactions may appear anywhere.

## Reading

```rust
pub struct Reader<R: Read>;                          // Debug; Iterator<Item = Result<Entry, HistoryError>>
pub fn Reader::builder(source: R) -> ReaderBuilder<R>;
// ReaderBuilder: .settings(ReaderSettings) .bounds(Bounds) .verify_digest(bool) .open() -> Result<Reader<R>, HistoryError>
pub fn Reader::header(&self) -> &Header;
pub fn Reader::header_raw(&self) -> &str;             // line 1 as the file has it
pub fn Reader::summary(&self) -> &Summary;            // what has been read so far
pub fn Reader::next_entry(&mut self) -> Result<Option<Entry>, HistoryError>;
pub fn Reader::check(self) -> Result<Summary, HistoryError>;                       // reads to the end, checking every line and the digest; keeps nothing
pub fn Reader::copy_to<W: Write>(self, sink: W) -> Result<Summary, HistoryError>;   // lines verbatim, each checked before it is written
#[non_exhaustive] pub struct Entry { pub number: u64, pub line: Line, pub raw: String }
```

`open` reads and checks the header (nothing before it performs I/O). It holds at most one line plus the one being returned. After any error `next_entry` returns `Ok(None)`.

**Entries are unverified until the end.** A line is checked for shape, order and bounds when `next_entry` returns it, but a digest in the header is compared only when the end is reached, and a mismatch is the error that replaces the `None`. A consumer that acts on entries as they come acts on content the digest has not vouched for; `copy_to` leaves the sink holding every line when it then fails (buffer the sink if it must never hold unverified content). To act only on verified content: read the whole history with `Document::from_reader` (returned only after the check), or call `Reader::check` on a source you can read again and then read it a second time. The digest shows accidental change only: it is optional and anyone can recompute it.

`Document::from_reader` reads under the reader's own settings; `Document::from_slice` and `str::parse::<Document>()` read under `ReaderSettings::for_document()`.

## Writing

```rust
pub struct Writer<W: Write>;
pub fn Writer::builder(sink: W) -> WriterBuilder<W>;  // .settings(WriterSettings) .bounds(Bounds) .start(&Header) -> Result<Writer<W>, HistoryError>
pub fn Writer::write(&mut self, line: &Line) -> Result<(), HistoryError>;
pub fn Writer::summary(&self) -> &Summary;
pub fn Writer::body_digest(&self) -> String;          // "sha256:<hex>" of the body so far
pub fn Writer::finish(self) -> Result<(W, Summary), HistoryError>;
```

`start` writes the header under the writer's bounds (nesting and values included: a header it writes is one a reader with the same bounds reads). A header carrying a digest is written as given, and `finish` returns `DigestMismatch` if the body written is not the one it names (`body_digest` or `Document::seal` makes the right one). `write` serialises the line, re-reads it with the reader's own checks (shape, order, bounds) and writes it only if they pass; a refused line writes nothing and the writer goes on, but a sink failure poisons it. The output is in canonical spelling: compact, header keys in table order, `method` `params` `at` `_meta` in lines, every other object's keys sorted. Reading and writing a document gives that spelling; use `Reader::copy_to` to keep a file byte for byte.

```rust
#[non_exhaustive] pub struct Document { pub header: Header, pub lines: Vec<Line> }
impl Document { fn new(Header); fn from_reader(Reader<R>); fn from_slice(&[u8]); /* FromStr */
                fn check(&self) -> Result<Summary, HistoryError>; fn to_writer(&self, W); fn to_vec(&self); fn to_string(&self);
                fn seal(&mut self) -> Result<(), HistoryError>; fn seed(&self) -> Seed<'_>; }
```

`Document::to_writer`, `to_vec` and `to_string` write a header that has a digest with the digest of the body as written, so a document read from another producer's spelling is written in canonical spelling with a digest that is true of it. `check` ignores the digest.

## Types

```rust
#[non_exhaustive] pub struct Header { pub id: String /*UUID*/, pub origin: Origin, pub created_at, model, mode, effort, cwd: Option<String>,
    pub acp_version: Option<u64>, pub digest: Option<String>, pub redacted: Option<Redacted>, pub meta: Option<serde_json::Value> }
fn Header::new(id: impl Into<String>, origin: Origin) -> Header;  fn Header::to_line(&self) -> Result<String, HistoryError>;   // under the default bounds
#[non_exhaustive] pub struct Origin { pub harness: String, pub version: Option<String>, pub session_id: Option<String> }  // Origin::new(harness)
#[non_exhaustive] pub struct Line { pub at: Option<String>, pub meta: Option<Value>, pub body: Body }
// Line::new(Body), prompt(Vec<Value>), text_prompt(&str), update(Value), turn_end(Stop), compaction(summary), .at(..), .meta(..), to_json()
#[non_exhaustive] pub enum Body { Prompt { prompt: Vec<Value> }, Update { update: Value }, Cancel,
    PermissionRequest { tool_call: Value, options: Vec<Value> }, PermissionAnswer { tool_call_id: String, outcome: Value },
    TurnEnd { stop: Stop, error: Option<String> }, Compaction { summary: Option<String>, trigger: Option<Trigger>, pre_tokens: Option<u64> } }
#[non_exhaustive] pub enum Stop { Acp(StopReason), Error }       // Stop::name(), Stop::from_name()
#[non_exhaustive] pub enum Trigger { Auto, Manual }
pub const FORMAT: &str; pub const VERSION: u64; pub const NATIVE_META_KEY: &str /* "dev.boriken.history/native" */; pub const ATTACHMENT_META_KEY: &str;
#[non_exhaustive] pub struct Summary { pub lines, bytes, turns, updates, compactions: u64, pub last_boundary, last_summary: Option<Boundary>, pub last_stop: Option<Stop>, pub open_turn: bool }
```

Payloads (`prompt` blocks, `update`, `tool_call`, `options`, `outcome`, `_meta`) are `serde_json::Value`: ACP's shapes are not re-declared here, so a v1 or v2 update passes as long as it is an object with a `sessionUpdate`. The checks a line gets are listed in the format document.

## Seeding

```rust
pub fn Document::seed(&self) -> Seed<'_>;
#[non_exhaustive] pub struct Seed<'a> { pub summary: Option<&'a str>, pub point: Option<SeedPoint>, pub lines: &'a [Line], pub first_line: u64, pub mid_turn: bool }
pub fn Seed::turns(&self) -> u64;  pub fn Seed::fit(&self, max_bytes: u64) -> Fit<'a>;
#[non_exhaustive] pub struct SeedPoint { pub line: u64, pub digest: String }   // guatiao ToValue/FromValue/Schema, Serialize
#[non_exhaustive] pub struct Fit<'a> { pub summary: Option<&'a str>, pub lines: &'a [Line], pub first_line: u64, pub dropped_turns: u64, pub bytes: u64, pub complete: bool }
```

The seed starts at the latest `history/compaction` that has a summary (else at the first body line); `point.digest` is the SHA-256 of that line in canonical spelling. `fit` keeps the summary and the latest whole turns within `max_bytes` (each line as `Line::to_json` writes it, plus a line feed); divide a token window by your own bytes-per-token figure.

## Conversation and recorder (`coqui_acp_history::conversation`)

What a codec on either side of a harness's files needs, with no knowledge of any harness.

```rust
pub struct Conversation { pub summary: Option<Boundary>, pub seeded_from: Option<SeedPoint>, pub summary_missing: bool,
    pub items: Vec<Item>, pub calls: BTreeMap<String, Call>, pub closed_calls: u64, pub closed_turn: bool }   // #[non_exhaustive]
pub fn Conversation::of(doc: &Document, use_summary: bool) -> Conversation;
pub fn Conversation::call(&self, id: &str) -> Option<&Call>;
pub enum Item { Prompt { blocks, stamp }, Note { block, stamp }, Message { text, stamp }, Thought { text, stamp }, Call { id },
    Result { id }, Plan { entries, stamp }, Cancel { stamp }, TurnEnd { stop, error, closed_here, stamp } }          // #[non_exhaustive]
pub struct Call { pub id, name, title, kind, status: Option<String>, raw_input, raw_output: Option<Value>, content, locations: Vec<Value>,
    call_meta, result_meta: Option<Value>, call_at, result_at: Option<String>, announced: bool }                    // Call::new, is_done, is_failed, output_text, announce, finish
pub struct Stamp { pub at: Option<String>, pub meta: Option<Value> }    // Stamp::at, new, native(harness, fields), native_of(harness)
pub struct Boundary { pub summary: String, pub trigger: Option<Trigger>, pub pre_tokens: Option<u64>, pub stamp: Stamp }
pub struct Recorder;   // new, is_open, prompt, update, message, thought, note, cancel, end_turn, compaction, finish() -> Vec<Line>
pub fn native_meta(harness: &str, fields: Map) -> Option<Value>;  pub fn native_of(meta: Option<&Value>, harness: &str) -> Option<&Map>;
pub fn derived_uuid(session, what, n) -> String;  is_uuid(&str) -> bool;  clean_id(id, taken) -> String;  shell_text(&[&str]) -> String;
pub fn call_note(&Call) -> String;  valid_time(&str) -> bool /* as the format reads a timestamp */;
```

`Conversation::of` seeds as `Document::seed` does (the latest compaction that has a summary, then what follows) when `use_summary`, and from the first line, with no boundary's text anywhere, when not. A tool call's `tool_call` and `tool_call_update`s are merged into one `Call`, which appears as an `Item::Call` where it was announced and an `Item::Result` where it ended (or where this ended it). It closes what the history left open, as the format's rule (a): a call with no result ends `cancelled` (`closed_calls`, an `Item::Result` before the turn's end) and a turn left open ends `Cancelled` (`closed_turn`, an `Item::TurnEnd` with `closed_here`). A permission request and its answer, and a compaction boundary other than the seed's, are not items (rule (b); a boundary with no summary is `summary_missing`). A result for a call the seed does not hold is a `Call` with `announced` false, for a writer to drop. Chunks of one message or thought are joined (a chunk with another `messageId` starts a new one); ACP updates that are no conversation (usage, modes, commands) are ignored. A history whose header says `acpVersion` 2 or more is read as version 2: `agent_message`, `agent_thought` and `user_message` replace the content of the message with their `messageId`, the first `tool_call_update` of a call announces it (there is no `tool_call`), and `plan_update` is a plan. `Recorder` also has `permission_request`, `permission_answer` and `len`.

`Recorder` builds lines that keep the order rules: `prompt` ends an open turn first as `end_turn` (a codec that knows better ends it itself), `cancel` and `end_turn` do nothing with no turn open, and `finish` leaves a turn open as a recording cut short. A codec puts what only its own harness reads into a line's `_meta` with `Stamp::native(harness, fields)` and reads it back with `native_of`, which answers only when the object names that harness.

## Recorder of any ACP agent (`coqui_acp_history::record`)

```rust
#[non_exhaustive] pub enum Direction { Sent, Received }          // as the client sees the message
pub struct SessionRecorder;                                      // Debug
pub fn SessionRecorder::new(id: impl Into<String> /*UUID*/, origin: Origin) -> SessionRecorder;
pub fn session(self, id: impl Into<String>) -> Self;                                        // follow this agent session only
pub fn max_line_bytes(self, n: u64) -> Self;  pub fn max_lines(self, n: u64) -> Self;      // defaults: the reader's
pub fn max_total_bytes(self, n: u64) -> Self;                                              // default DEFAULT_DOCUMENT_MAX_TOTAL_BYTES; sets `truncated`
pub fn observe(&mut self, direction: Direction, message: &Value);                         // one JSON-RPC message
pub fn observe_at(&mut self, direction: Direction, message: &Value, at: Option<&str>);    // with an RFC 3339 time (a bad one is dropped)
pub fn skipped(&self) -> u64;  pub fn truncated(&self) -> bool;
pub fn finish(self) -> Result<Document, HistoryError>;            // a turn still open stays open
```

Feed it every message of one connection in order; it keeps one session's (the one given to `session`, else the one the first `session/new`, `session/load` or `session/resume` it saw is answered with: a refused open names none, updates before the answer are not recorded, and a later open does not move it), ignores what it does not know and never fails the connection it watches. Updates are written as the agent sent them (v1 and v2); it adds only the turn structure (`session/prompt` and its answer, `session/cancel`), permission requests and the client's answers, a `current_mode_update` for a `session/set_mode` the agent did not announce, a `history/compaction` with its summary for a completed version 2 compaction, and header fields from `initialize` (`acpVersion`, `origin.version`), `session/new` (`cwd` when it is a valid path, mode, model) and mode updates. A message whose line the writer would refuse (parsed under the default `Bounds`: nesting 64, 500,000 values) or that is over `max_line_bytes` is `skipped`; a header value the format refuses (session id, mode, model, version, `protocolVersion`, `cwd`) is dropped; at `max_lines` (header included) or `max_total_bytes` it stops and `truncated` is true, so `finish` gives a history that always writes and reads back: every line is counted with its time, and the header has a fixed share of the limit (its first size and 12 KiB, at most a quarter of the limit) that a late mode, model or `cwd` cannot outgrow (one that would is dropped). The updates a `session/load` streams before its answer are not recorded. Never build a path from what it records.

## Replay as context (`coqui_acp_history::replay`)

```rust
#[non_exhaustive] pub struct ReplaySettings { pub max_bytes: u64 /* 32768 */, pub max_item_bytes: u64 /* 4096 */,
    pub redact_max_bytes: u64 /* 4 MiB scanned in all */, pub redact_max_millis: u64 /* 2000 */ }   // Default, guatiao ToValue/FromValue/Schema, Serialize
pub struct Replay;                                                 // Debug, Clone
pub fn Replay::of(doc: &Document, settings: &ReplaySettings) -> Replay;
pub fn Replay::of_redacted(doc: &Document, settings: &ReplaySettings, redactor: &mut Redactor) -> Result<Replay, HistoryError>;   // the redactor runs over each rendered item, after its pieces are joined
pub fn redaction(&self) -> Option<&RedactionReport>;
pub fn text(&self) -> Option<&str>;  pub fn block(&self) -> Option<Value>;                 // None: nothing to replay, or none fits
pub fn prompt(&self, blocks: Vec<Value>) -> Vec<Value>;            // the replay block first, the client's blocks untouched
pub fn report(&self) -> &LossReport;  pub fn seeded_from(&self) -> Option<&SeedPoint>;
pub fn kept_turns(&self) -> u64;  pub fn cut_items(&self) -> u64;
pub const DEFAULT_MAX_BYTES: u64;  pub const DEFAULT_MAX_ITEM_BYTES: u64;
```

For an agent with no way to import a history. Starts at the latest compaction summary (`Conversation::of`), keeps the newest whole turns whose block is within `max_bytes` (markers and sentences included), starting at a turn, and quotes every line of the record with `| ` between two markers carrying a digest of the record; every Unicode format character, variation selector, tag character, filler, control and separator is written `\u{..}`, and an item's limit is counted after that; a line after the first of an item is indented two spaces so a history cannot forge a labelled line. Every field a block shows (tool name, title and status, attachment names and addresses, plan entries) is cut like an item, and a prompt is cut as a whole, so one oversized field cannot empty a replay. **Everything is replayed verbatim, secrets a tool printed included, to whichever model reads it: use `of_redacted` when they cannot be ruled out.** It joins the pieces of an item (a message's chunks, a result's content items, a prompt's blocks, a tool's input with its keys) before matching, drops format and zero-width characters first, reads an item up to four times its limit and never more than 1 MiB (a text read to that bound is cut and counted `item_cut`), looks at `redact_max_millis` between matches and rules as well as between items, renders the newest items first so that a limit spent hides the oldest, matches a tool's `rawInput` and an object `rawOutput` by their unescaped keys and values, scans at most `redact_max_bytes` and runs at most `redact_max_millis`; text past a limit reads `[not shown: redaction limit reached]` and is counted `redaction_limit`. Patterns are Rust `regex` (linear time, no look-around or back-references). `Redactor::redact_text(&str) -> String` redacts text of your own making. The text says it is data and not instructions; the format is in `docs/session-history.md` ("Replaying a history as context"). Reasoning is not replayed; a tool call is one entry (name, status, input, output) and never runs. The report has `replayed_as_prompt` (1) when anything was carried, and `history_cut` (items), `item_cut`, `non_text_omitted`, `thinking`, `tool_as_note`, `summary_missing` as they apply; nothing carried (an empty history, or none fits) gives `text()` of `None` and no `replayed_as_prompt`. It is a defence in depth: the quotation is this crate's, whether an agent honours it is the agent's.

## Paging (`coqui_acp_history::paging`)

```rust
pub fn page(text: &str, after: Option<&str>, max_bytes: u64) -> Result<Page, PageError>;
pub struct Page { pub text: String, pub next: Option<String> }
pub enum PageError { Position, LineTooLarge(u64) }
```

Cuts a history's text into pages of whole lines for a provider's `export_session`. The position is `<byte offset>:<sha256 of the whole text>`, so a position of another text, one in the middle of a line or one that is not this function's is `Position`, and a next line that does not fit is `LineTooLarge(bytes with its line feed)`.

## Redaction

```rust
pub fn Redactor::try_new(settings: &RedactionSettings) -> Result<Redactor, HistoryError>;   // Pattern { index } for a bad, huge or empty-matching pattern
pub fn Redactor::header(&mut self, &mut Header) -> Result<(), HistoryError>;
pub fn Redactor::line(&mut self, number: u64, &mut Line) -> Result<(), HistoryError>;
pub fn Redactor::document(&mut self, &mut Document) -> Result<RedactionReport, HistoryError>;   // when it removed anything: marks header.redacted, drops the digest
pub fn Redactor::report(&self) -> &RedactionReport;   pub fn Redactor::mark(&self) -> Redacted;
#[non_exhaustive] pub struct RedactionReport { pub replaced: u64, pub lines: u64, pub rules: Vec<RuleReport> }
#[non_exhaustive] pub struct RuleReport { pub kind: RuleKind /*Value|Pattern*/, pub index: usize, pub replaced: u64, pub lines: u64 }
```

Off unless a caller builds a `Redactor`. Strings are changed everywhere in the header (not `format`, `id`, `createdAt`, `digest`, `origin.harness`) and the payloads, except the identifiers that pair lines, in the place that pairs them: an update's `sessionUpdate` and `toolCallId`, a request's `toolCall.toolCallId` and each option's `optionId`, an answer's `outcome.optionId` (the same key inside `rawInput` or `_meta` is changed). Every occurrence of every value and every match of every pattern is found in the original text and the stretches they cover are replaced once, so a value inside a longer one leaves none of the longer text; a rule's `replaced` counts what it found, the report's total counts replacements, and the two differ where matches overlap. A removal that would leave a line the format refuses (for example a pattern matching `selected`) is an error and leaves the line and the document as they were. The report and the redactor's `Debug` never hold a value or a pattern, only its position. `document` marks `header.redacted` and drops `header.digest` only when it removed something (call `Document::seal` after); with nothing removed the document is untouched.

## Loss report

```rust
#[non_exhaustive] pub struct Loss { pub kind: String, pub count: u64, pub detail: Option<String> }   // guatiao ToValue/FromValue/Schema
pub struct LossReport;   // add(kind, count, detail), losses(), count(kind), is_empty(), IntoIterator, Extend/FromIterator<Loss>
pub const THINKING, TOOL_AS_NOTE, ATTACHMENT_BY_DIGEST, REPLAYED_AS_PROMPT, SUMMARY_MISSING, HISTORY_CUT, ITEM_CUT, NON_TEXT_OMITTED: &str;
```

`Loss` is shaped for the `acp_harness` kind's `ImportReport.losses`; an importer fills it, nothing in this crate does. `detail` never holds lost content. The kinds are an open vocabulary.

## Tool table

```rust
#[non_exhaustive] pub enum Tool { Read, Write, Edit, Patch, Shell, Grep, Glob, Fetch }    // Tool::ALL, name(), kind() -> ACP ToolKind, from_name()
pub fn Tool::natives(self) -> &'static [Native];  pub fn Tool::native(self, harness: &str) -> Option<&'static Native>;
pub fn Tool::from_native(harness: &str, name: &str) -> Option<Tool>;
#[non_exhaustive] pub struct Native { pub harness: &'static str, pub name: &'static str, pub keys: &'static [(&'static str, &'static str)] }  // (normalised, native)
pub fn tools::normalise(harness: &str, name: &str, input: &Value) -> Option<(Tool, Value)>;
pub fn tools::denormalise(tool: Tool, harness: &str, input: &Value) -> Option<(&'static str, Value)>;
```

Harness names are `claude`, `codex`, `agy`. The functions rename input keys only; a change of shape is the codec's. When two keys of the input would become the same key (Claude `Read` with both `file_path` and `path`) the result is `None`: neither value may win. agy's input keys are unverified.

## Settings (Guatiao values)

All `#[non_exhaustive]`, `Default`, public fields, unknown keys refused, `ToValue + FromValue + Schema + Serialize`.

| Type | Key | Default |
| --- | --- | --- |
| `Bounds` | `max_line_bytes` | 16777216 |
| | `max_lines` | 5000000 |
| | `max_total_bytes` | 4294967296 |
| | `max_depth` (capped at `PARSER_DEPTH`, 127) | 64 |
| | `max_values` | 500000 |
| `ReaderSettings` | `bounds`, `verify_digest` | `Bounds` default, `true` |
| `WriterSettings` | `bounds` | `Bounds` default |
| `RedactionSettings` | `values` (sensitive), `patterns`, `replacement` | `[]`, `[]`, `[redacted]` |

Constants: `DEFAULT_MAX_LINE_BYTES`, `DEFAULT_MAX_LINES`, `DEFAULT_MAX_TOTAL_BYTES`, `DEFAULT_MAX_DEPTH`, `DEFAULT_MAX_VALUES`, `DEFAULT_DOCUMENT_MAX_TOTAL_BYTES` (16 MiB), `PARSER_DEPTH` (127), `DEFAULT_REPLACEMENT`. `Bounds::for_document()` and `ReaderSettings::for_document()` are the defaults with the smaller total that `Document::from_slice` reads under.

`non_defaults<T: ToValue + Schema + Default>(&T) -> Vec<NonDefault { setting, value, default }>` lists, by key path, every setting not at its default, from the settings' schema; values and defaults are JSON text. A field the schema marks sensitive (a redaction's `values`, at any depth of a caller's own settings) is named and shown as `"[not shown]"`. Precedence for the command: its flags over the defaults; the library reads no environment variable and no file.

## Errors

`HistoryError` (`#[non_exhaustive]`, `Display` one lowercase clause quoting nothing from the document): `Io`, `Empty`, `NotUtf8`, `BlankLine`, `LineEnding`, `Truncated`, `Syntax { line, column }`, `DuplicateKey`, `Shape { line, pointer, expected }`, `UnknownField { line, pointer }`, `UnknownFormat`, `UnknownVersion { version }`, `UnknownLine`, `Order { line, rule: OrderRule }`, `TooLong`, `TooManyLines`, `TooLarge`, `TooDeep { line, limit, by_parser }`, `TooManyValues { line, limit }`, `DigestMismatch`, `Pattern { index }`. `TooDeep`'s `limit` is the nesting allowed and `by_parser` says it is the parser's own `PARSER_DEPTH` because `max_depth` was set above it. `code() -> &'static str` is the stable name the vectors use, `line()` and `pointer()` the place.

## Command (`--features cli`)

`coqui-acp-history list [DIR]`, `show FILE`, `convert IN -o OUT [--redact-pattern RE]... [--redact-values-file FILE] [--replacement TEXT] [--seal]`, `export --harness H SESSION` and `import --harness H FILE` (exit status 2: they need a harness's codec, and the codecs depend on this crate, so the command cannot call them; `coqui-acp-claude` and `coqui-acp-codex` ship the `history_claude` and `history_codex` examples, which export to standard output and import from standard input, so a session moves between harnesses as `history_claude export ... | history_codex import ...`), and `--max-line-bytes`, `--max-lines`, `--max-total-bytes`, `--max-depth`, `--max-values` on the first three. `show` and a `convert` that redacts or seals hold the history in memory, so they start from the 16 MiB total of `ReaderSettings::for_document()`; `list` and a verbatim `convert` stream. `convert` copies the checked lines verbatim, after the whole history has been checked, unless a redaction removes something or `--seal` asks for a rewrite in canonical spelling with a digest; the output is written whole or not at all, to a new uniquely named file beside it (created exclusively, so no existing file or link is written through) and renamed. A secret to remove is read from a file, never an argument. A bound looser than its default is reported on standard error. Everything a history says is printed with control, line-separator and bidirectional characters escaped as `\u{..}`. Exit status 1 for a refused history, 2 for a usage error.
