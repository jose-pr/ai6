# `coqui-acp` — API header

Header-file-style reference for the `coqui-acp` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-acp>.

One Cargo feature, off by default: `secret` (the `secret` module below), which adds `hpke`, `p256`, `x25519-dalek`, `hmac`, `sha2`, `zeroize`, `base64` and `data-encoding`. MSRV Rust 1.89. A default build depends on `agent-client-protocol`, `futures` and `serde_json`. Version `0.0.0-alpha.0`, not published; the API may change in any release before 1.0.

Imports: the crate root holds everything but `lines`. `coqui_acp::sdk` is the protocol's own crate, for its transports.

No `unsafe` code (`#![forbid(unsafe_code)]`). No process-wide state. No async runtime is required: each session runs a `futures` executor on a thread of its own.

## `Client`, `Builder` (crate root)

```rust
pub fn Client::builder(cwd: impl Into<String>) -> Builder
```
Starts a session description; `cwd` is the agent's absolute working directory.

```rust
pub fn client_info(self, name: impl Into<String>, version: impl Into<String>) -> Self
pub fn resume(self, session: impl Into<String>) -> Self
pub fn load(self, session: impl Into<String>) -> Self
pub fn mcp_server(self, server: Value) -> Self
pub fn meta(self, key: impl Into<String>, value: Value) -> Self
pub fn model(self, model: impl Into<String>) -> Self
pub fn effort(self, effort: impl Into<String>) -> Self
pub fn model_option(self, id: impl Into<String>) -> Self
pub fn effort_option(self, id: impl Into<String>) -> Self
pub fn prompt(self, text: impl Into<String>) -> Self
pub fn refuse_kinds(self, kinds: impl IntoIterator<Item = impl Into<String>>) -> Self
pub fn authenticate(self, authenticate: Authenticate) -> Self
pub fn turn_end_by(self, key: impl Into<String>) -> Self
pub fn ends_turn(self, ends: impl Fn(&str, &Value) -> bool + Send + Sync + 'static) -> Self
pub fn disconnect_detail(self, detail: impl FnOnce() -> Option<String> + Send + 'static) -> Self
pub const fn initialize_timeout(self, limit: Option<Duration>) -> Self
pub const fn request_timeout(self, limit: Option<Duration>) -> Self
pub const fn close_grace(self, limit: Option<Duration>) -> Self
pub const fn max_pending_events(self, events: usize) -> Self
pub const fn text_flush(self, bytes: usize) -> Self
pub const fn max_tool_kinds(self, calls: usize) -> Self
pub fn decider(self, decider: Arc<dyn Decider>) -> Self
pub fn elicitor(self, elicitor: Arc<dyn Elicitor>) -> Self
pub const fn max_elicitation_fields(self, fields: usize) -> Self
pub const fn max_elicitation_options(self, options: usize) -> Self
pub const fn close_on_drop(self, close: bool) -> Self
pub const fn fail_on_disconnect(self, fail: bool) -> Self
pub fn on_close(self, on_close: impl FnOnce() + Send + 'static) -> Self
pub fn connect(self, transport: impl ConnectTo<sdk::Client> + 'static) -> Result<Session, Error>
```

- `resume`: `session/resume`, else `session/load` with its replayed history dropped. `load`: `session/load` whatever else the agent offers, delivering what it replays.
- `meta` sets a key of the session request's `_meta`. `model` and `effort` go through the agent's config options: by the id given with `model_option`/`effort_option`, else the option of category `model`/`thought_level`, else the id `model`/`effort`. What cannot be applied is an `Event::Notice`.
- `refuse_kinds`: refused without asking the decider. A tool of such a kind that runs, whether it asked or not, ends the session with `Event::Failed`; a tool that reports `failed` ends nothing. A kind once announced as refused stays so, whatever a later message claims.
- `max_tool_kinds(calls)`: the tool calls whose kind is kept, which a later message about one without its kind (an update, a permission request) is judged by. At the limit the oldest of a kind not refused is forgotten, which changes no refusal: such a message is then of unknown kind, so `PermissionRequest::kind` is `None`, and the first one forgotten is an `Event::Notice`. One of a refused kind is never forgotten: when every one kept is refused, a further tool of a refused kind ends the session with `Event::Failed`. Any value is taken; 0 keeps none.
- `turn_end_by(key)`: when the agent advertises `key` in `agentCapabilities._meta`, its notifications carry turn endings: standard updates and config options are left alone, and a failed prompt is `Event::PromptFailed`.
- `ends_turn(ends)`: while `turn_end_by` is active, says which of the extension's notifications end a turn, given their method and parameters, so a connection that ends after one is no `Event::Failed`. It runs on the connection's thread, in order and ahead of the connection's end, so it cannot race a reader; it must not block. Turns are counted: no turn runs only once as many endings have arrived as turns were sent, so a turn sent before the last one's ending arrived still counts as running. `PromptFailed` is still delivered after such a turn: the caller judges it.
- `disconnect_detail(detail)`: appended to the `Event::Failed` of a lost connection as `<why>: <detail>`. Asked at most once, on the connection's thread, only when that event is due; it may wait a while for the agent's exit.
- Defaults: 20 s `initialize`, 30 s other requests, 2 s close grace; `None` waits without limit. 100000 pending events, then the session ends with `Event::Failed`. Agent text is delivered as an `Event::Text` once 64 KiB of UTF-8 has gathered (`text_flush`; any value is taken). The kinds of 4096 tool calls. `RefuseAll`. `close_on_drop` and `fail_on_disconnect` on.
- `connect` initializes, authenticates, opens the session and sends the first prompt. It errors on the agent's refusal or silence, a protocol version other than 1, or a capability the session needs, such as HTTP MCP servers.

## `Session` (crate root)

Every method takes `&self`, so one thread can read events while another sends.

```rust
pub fn id(&self) -> Option<String>
pub fn extension_active(&self) -> bool
pub fn connection_lost(&self) -> bool
pub fn send(&self, text: impl Into<String>) -> Result<(), Error>
pub fn notify(&self, method: impl Into<String>, params: Value) -> Result<(), Error>
pub fn next_event(&self, timeout: Duration) -> Option<Event>
pub fn cancel(&self) -> Result<(), Error>
pub fn close(&self)
```

`next_event` answers `None` when none came in time or the session has ended. `send` is never retried: after a lost connection its outcome is unknown. `close` waits no longer than the close grace, then runs `on_close`.

Only this session's traffic counts: a `session/update` whose `sessionId` is not this session's is dropped, and a `session/request_permission` naming another session is answered cancelled without asking the decider. Before the session is known nothing is this session's, except that `resume` and `load` know it from the start. That is what an agent kept by a bridge (`coqui-acp-bridge serve --linger`) sends of an earlier connection's session. Request ids are the SDK's UUIDs, so they do not repeat across connections.

## `Event` (crate root)

`#[non_exhaustive]`. `Text(String)`: agent text, gathered until the next tool or the end of the turn, and flushed at `Builder::text_flush` UTF-8 bytes (64 KiB). `Tool { id, title, name: Option<String>, kind }`. `Cost { amount: f64, currency }`. `Notice(String)`. `Permission { tool, allowed: bool, reason: Option<String> }`. `Completed { stop }`: `end_turn`, `max_tokens` or `max_turn_requests`. `Failed(String)`: the turn failed, the agent refused, or the connection was lost. `Cancelled`: the agent ended the turn as cancelled, whether asked to or by itself. `Blocked(String)`: a permission request or a question could not be answered, so the agent was told it was cancelled; the turn goes on until the agent ends it. `PromptFailed(String)`: see `turn_end_by`. `OutputLost { lines: u64, bytes: u64 }`: `_dev.boriken.coqui.acp/output_lost`, which a bridge sends first on a connection when it dropped the kept agent's output while nobody was connected; what the session shows has a gap there, so reconcile by `load` or tell the person. `Extension { method, params: Value }`: any other notification whose method starts with `_`, untouched.

## `Decider` (crate root)

```rust
pub trait Decider: Send + Sync { fn decide(&self, request: &PermissionRequest) -> Decision; }
pub enum Decision { Allow, Deny(String) }
pub struct RefuseAll;
```

A decider may block: it runs on a thread of its own. `PermissionRequest` (`#[non_exhaustive]`): `session_id`, `tool_call_id`, `title: Option<String>`, `kind: Option<String>`, `raw_input: Option<Value>`, `options: Vec<PermissionOption>`. `PermissionOption`: `option_id`, `kind`. `raw_input` is what the agent wants to do: untrusted.

## `Elicitor` (crate root)

```rust
pub trait Elicitor: Send + Sync { fn elicit(&self, request: &Elicitation) -> ElicitationAnswer; }
pub enum ElicitationAnswer { Accept(Map<String, Value>), Decline, Cancel }
```

An agent's `elicitation/create`, in form mode, put to the person. Without `Builder::elicitor` nothing is advertised and the request is answered method-not-found; with one, `initialize` advertises `clientCapabilities.elicitation.form`, and an elicitor may block: it runs on a thread of its own. `Elicitation` (`#[non_exhaustive]`): `session_id`, `tool_call_id: Option<String>`, `message`, `title`, `description`, `fields: Vec<Field>`. `Field`: `name`, `title`, `description`, `required`, `kind: FieldKind` (`#[non_exhaustive]`): `Text { min_length, max_length, format, default }`, `Choice { options, default }`, `Choices { options, min_items, max_items, default }`, `Number { minimum, maximum, default }` (f64), `Integer { .. }` (i64), `Boolean { default }`, or `Unsupported { kind }`, never rendered as a known control. `ChoiceOption`: `value`, `title` (the value when the agent gave none), `description`. A bound or default of the wrong type is left out; a choice default not offered is left out.

Another session's question is answered `cancel` without asking. One that cannot be put to a person (not form mode, URL mode included, tied to no session, no `message`, `requestedSchema.properties` not an object, more than `max_elicitation_fields` questions (64) or a question with more than `max_elicitation_options` choices (256)) is answered `cancel` with an `Event::Blocked`. `Accept` is sent only when it fits the form: no field it did not ask, every required one, each value of its field's type and within its bounds (text length in characters, choices each offered and none twice, a whole number for an integer); otherwise `cancel` and `Event::Blocked`. Never an error answer. All of it is the agent's text, untrusted. The Dart client keeps the same, as `vectors/elicitation.json` says.

## `Exchange` (crate root)

```rust
pub fn Exchange::new() -> Exchange      // also Default
pub fn client_info(self, name: impl Into<String>, version: impl Into<String>) -> Self
pub fn authenticate(self, authenticate: Authenticate) -> Self
pub const fn initialize_timeout(self, limit: Option<Duration>) -> Self
pub const fn request_timeout(self, limit: Option<Duration>) -> Self
pub const fn describe_limits(self, limits: DescribeLimits) -> Self
pub fn run(
    self,
    transport: impl ConnectTo<sdk::Client> + 'static,
    calls: Vec<(String, Value)>,
) -> Result<Vec<Result<Value, RpcFailure>>, Error>
pub struct Authenticate { pub method: String, pub meta: Map<String, Value> }
pub struct RpcFailure { pub code: i32, pub message: String }
```

Initializes, authenticates when asked, and makes each call in turn: for questions that are no session. Each call has its own result; `code` is `-1` when no answer came in time. Defaults: calls itself `coqui-acp` at this crate's version, nothing to authenticate, 20 s `initialize`, 30 s each call, `DescribeLimits::new()`; `None` waits without limit. `describe_limits` matters only to `describe`.

## Describing an agent (crate root)

```rust
pub fn Exchange::describe(self, transport: impl ConnectTo<sdk::Client> + 'static) -> Result<Description, Error>

#[non_exhaustive]
pub struct Description {
    pub options: Answer<Vec<sdk::schema::v1::SessionConfigOption>>,
    pub modes: Answer<sdk::schema::v1::SessionModeState>,
    pub sessions: Answer<SessionList>,
}
#[non_exhaustive]                                               // Clone, Debug, PartialEq
pub struct SessionList { pub sessions: Vec<sdk::schema::v1::SessionInfo>, pub incomplete: Option<String> }
pub enum Answer<T> { Known(T), Unknown(String), NotSupported }   // Clone, Debug, PartialEq, Eq

#[non_exhaustive]                                               // Clone, Copy, Debug, Default, PartialEq, Eq
pub struct DescribeLimits { pub options: usize, pub values: usize, pub modes: usize, pub sessions: usize, pub text: usize, pub pages: usize }
pub const fn DescribeLimits::new() -> DescribeLimits            // 64, 1024, 64, 500, 4096, 10
pub const fn options(self, options: usize) -> Self              // and values, modes, sessions, text, pages: one setter per field

pub const SESSION_OPTIONS_CAPABILITY: &str = "dev.boriken.coqui.acp/session_options";
pub const SESSION_OPTIONS_METHOD: &str = "_coqui_acp/session_options";
```

What an agent offers before any session, on a connection of its own that opens none: initialize, authenticate as `Exchange` does, then ask only what `initialize` advertised.

- `options` and `modes`: `SESSION_OPTIONS_METHOD` with `{}`, sent only when `agentCapabilities._meta[SESSION_OPTIONS_CAPABILITY]` is `true` or an object; otherwise both are `NotSupported`. The answer is `{"configOptions": [SessionConfigOption], "modes": SessionModeState, "unknown": {"configOptions" | "modes": reason}}`: a part present is `Known`, at the values a session starts with; one left out is `Unknown` with the agent's reason, or `the agent did not say what <part> it has`. A model is the option of category `model`, an effort the one of category `thought_level`.
- `sessions`: `session/list`, sent only when `agentCapabilities.sessionCapabilities.list` is an object; otherwise `NotSupported`. The first request is `{}`, and each next one `{"cursor": <the last page's nextCursor>}`, until a page gives no cursor or an empty one, `sessions` are kept, or `pages` requests were made. A first page that fails or cannot be read decides the answer; a later one ends the list with the pages before it. A list that stops short says why in `incomplete`, which is `None` only when it holds all the agent listed: `a later page failed: <why>`, `the sessions limit was reached` when that limit left out a session or a page the agent offered, or `the page limit was reached` when a cursor was left. `pages` 0 asks nothing: `Unknown("the sessions were not asked: the page limit is 0")`.
- A call that fails or gets no answer in time is `Unknown("the agent could not say: <why>")`. `Err` only when the connection, `initialize` or `authenticate` failed.

The answer is the agent's, untrusted, and only read: each item is read as the ACP type or skipped, and `_meta` and `additionalDirectories` are dropped. `DescribeLimits` bounds what is kept, and any value is taken, 0 included: at most `options` options (default 64), `values` values per option across its groups (1024), `modes` modes (64) and `sessions` sessions across pages (500). An item whose identifier (option, value, group, mode or session id, current value, `cwd`) is empty or longer than `text` UTF-8 bytes (4096) is dropped, a category that long is left out, and other text (names, descriptions, titles, reasons) is cut there on a character boundary. `pages` defaults to 10: the 500 sessions at 50 a page, and at most ten request timeouts spent on an agent that always gives a cursor. The Dart client keeps the same, with the same defaults; `vectors/describe.json` holds both to it.

## Lines (`coqui_acp::lines`)

```rust
pub const DEFAULT_MAX_LINE: u64 = 16 * 1024 * 1024;
pub type IoLines;       // the transport
pub type Diagnostics = Arc<Mutex<VecDeque<String>>>;
pub fn over_io(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static, diagnostic_lines: usize, max_line: u64) -> io::Result<(IoLines, Diagnostics)>
```

A transport over blocking I/O, such as a process's pipes, one JSON-RPC message per line. A line that does not start a JSON value is kept in `Diagnostics`, the last `diagnostic_lines` of them, for the message shown when the peer dies. A line over `max_line` ends the transport.

## Secret prompts (`coqui_acp::secret`, feature `secret`)

An agent asks the person for a secret, or shows them one, without the model, a transcript or a log holding the value: the host sends the frontend of the session's owner `_coqui_acp/secret_request`, or `_coqui_acp/secret_show_open` and then `_coqui_acp/secret_show_deliver`, and each secret value travels sealed to a key only its receiver holds, from a sender the receiver expects. The wire, the capability and the binding rules are `docs/secrets.md`; `vectors/secret.json` holds this module and the Dart client (`package:coqui_acp/secret.dart`) to the same bytes. This module does the messages and the cryptography; the handle store, the tools and the person's approvals are the host's.

```rust
pub const CAPABILITY: &str = "dev.boriken.coqui.acp/secrets";   // clientCapabilities._meta key
pub const REQUEST_METHOD: &str = "_coqui_acp/secret_request";
pub const SHOW_OPEN_METHOD: &str = "_coqui_acp/secret_show_open"; pub const SHOW_DELIVER_METHOD: &str = "_coqui_acp/secret_show_deliver";
pub const VERSION: u64 = 1; pub const DEFAULT_LIFETIME: Duration;   // 240 s
pub const BINDING_ECDSA: &str = "ecdsa-p256-sha256"; pub const BINDING_DH: &str = "x25519-hmac-sha256";

pub struct Capability { pub request: bool, pub show: bool, pub bindings: Vec<String> }       // #[non_exhaustive]
impl Capability { pub fn all() -> Self; pub fn to_value(&self) -> Value; pub fn from_meta(meta: &Map<String, Value>) -> Option<Self> }

pub struct Context { pub session_id: String, pub request_id: String, pub message: String, pub schema: String, pub form: Option<String>, pub values: Option<String> }
impl Context { pub fn digest(&self, method: Method) -> [u8; 32] }   // schema, form and values (a show's plain values) are the JSON text as sent; these bytes are hashed

pub struct Limits { pub fields: usize, pub options: usize, pub schema_bytes: usize, pub name_bytes: usize, pub value_bytes: usize }   // 64, 256, 64 KiB, 128, 64 KiB; setters
pub struct Form { pub title: Option<String>, pub description: Option<String>, pub fields: Vec<FormField> }
pub struct FormField { pub field: Field, pub secret: bool, pub encoding: Option<String>, pub media_type: Option<String> }
impl Form {
    pub fn parse_text(schema: &str, limits: &Limits) -> Result<Form, Error>;  pub fn parse(schema: &Value, limits: &Limits) -> Result<Form, Error>;
    pub fn field(&self, name: &str) -> Option<&FormField>;
    pub fn check_plain(&self, values: &Map<String, Value>) -> Result<(), Error>;
    pub fn check_sealed<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Result<(), Error>;
    pub fn check_secret(&self, name: &str, value: &[u8]) -> Result<(), Error>;
    pub fn check_layout(&self, text: &str) -> Result<(), Error>;   // the layout as the JSON text sent
}

pub enum Method { Request, Show }                                   // bound into every sealed field and binding
pub struct IdentityId;                                              // sha256:<64 hex> of an SPKI; Display, FromStr, Ord
pub struct Prover;                                                  // an identity that signs or agrees on keys through functions the caller gives
impl Prover {
    pub fn signing(spki: impl Into<Vec<u8>>, sign: impl Fn(&[u8]) -> Result<[u8; 64], Error> + Send + Sync + 'static) -> Result<Prover, Error>;   // P1363 ECDSA P-256/SHA-256
    pub fn agreeing(spki: impl Into<Vec<u8>>, agree: impl Fn(&[u8; 32]) -> Result<[u8; 32], Error> + Send + Sync + 'static, collector_spki: &[u8]) -> Result<Prover, Error>;
    pub fn spki(&self) -> &[u8];  pub fn id(&self) -> IdentityId;
}
pub mod unprotected {   // raw keys in this process's memory: tests and tools only
    pub fn ecdsa_p256(scalar: &[u8]) -> Result<Prover, Error>;  pub fn ecdsa_p256_pkcs8(der: &[u8]) -> Result<Prover, Error>;
    pub fn x25519(secret: &[u8], collector_spki: &[u8]) -> Result<Prover, Error>;  pub fn x25519_pkcs8(der: &[u8], collector_spki: &[u8]) -> Result<Prover, Error>;
    pub fn x25519_agreement(secret: &[u8]) -> Result<(Vec<u8> /* spki */, impl Fn(&[u8; 32]) -> Result<[u8; 32], Error>), Error>;
}
pub struct Verifier;                                                // a checker: its own id, the identities it knows
impl Verifier {
    pub fn builder(own: IdentityId) -> VerifierBuilder;   // .trust(id) .trust_spki(&[u8]) .agreement(spki, fn)? .max_lifetime(Duration) (5 min) .skew(Duration) (30 s) .limits(Limits) .build()
    pub fn collector(&self, expected: &IdentityId, method: Method, context: &Context, recipient: &Recipient, now: u64) -> Result<Collector, Error>;
    pub const fn id(&self) -> IdentityId;
}
pub struct Receiver;
impl Receiver {
    pub fn new(method: Method, context: &Context, limits: &Limits, prover: &Prover, audience: &IdentityId, now: u64, lifetime: Duration) -> Result<Receiver, Error>;
    pub const fn recipient(&self) -> &Recipient;  pub fn request_id(&self) -> &str;
    pub fn open_values(self, verifier: &Verifier, sender: &IdentityId, plain: &str, sealed: &BTreeMap<String, Sealed>, auth: &Binding, now: u64) -> Result<Opened, Error>;   // once; plain is the JSON object text as sent
}
pub struct Collector;
impl Collector {
    pub const fn holder(&self) -> IdentityId;  pub const fn form(&self) -> &Form;  pub fn is_secret(&self, name: &str) -> bool;
    pub fn seal(&self, name: &str, value: &str) -> Result<Sealed, Error>;
    pub fn authenticate(&self, prover: &Prover, plain: &str, sealed: &BTreeMap<String, Sealed>, now: u64, lifetime: Duration) -> Result<Binding, Error>;   // covers the whole answer
}
pub struct Recipient { pub key: [u8; 32], pub binding: Binding }    // to_value / from_value
pub struct Binding { pub method: String, pub key: Vec<u8>, pub expires: u64, pub audience: IdentityId, pub proof: Vec<u8> }   // to_value / from_value
pub struct Sealed { pub enc: [u8; 32], pub ct: Vec<u8> }           // to_value / from_value
pub struct Opened { pub plain: Map<String, Value>, pub secrets: BTreeMap<String, SecretValue> }
pub struct SecretValue;  // expose() -> &[u8], expose_str() -> &str; zeroed on drop; Debug is "SecretValue(..)"
// Messages, each with to_value / from_value: RequestParams { context, recipient }, RequestReply (Accept { plain: String, sealed, auth } | Decline | Cancel),
// ShowOpen { context } (its plain values are context.values), ShowOpened (Ready(Recipient) | Decline | Cancel), ShowDeliver { session_id, request_id, sealed, auth }, ShowDone (Shown | Cancel)
pub enum Error { Schema, Malformed, Unsupported, Key, UnknownIdentity, BadProof, Expired, WrongAudience, Field, Seal, Open, TooLarge }   // #[non_exhaustive], static reasons, never a value
```

- **Roles.** The receiver of the secret values makes a key pair for the request (`Receiver::new`) and sends `recipient()`: the host for a `request`, the frontend (in answer to `secret_show_open`) for a `show`. The collector (`Verifier::collector`) is told which identity the request must come from and refuses the key unless its binding verifies for that identity, for this verifier's own id, and for the digest of the `Context` it received; it then seals each secret field and authenticates the whole answer (`Collector::authenticate`). The receiver opens the answer once (`open_values` consumes it) and only after it verified the sender over the whole answer, plain values included. `now` is an argument (Unix seconds): the module reads no clock. A receiver also refuses an answer after its own binding ended plus the verifier's `skew`. The owner of receivers remembers request ids for the verifier's `max_lifetime` plus `skew`. A context's `values` are for a show only (a request with values, or a show without, is refused); `authenticate` refuses an answer the form does not allow before it signs. The ciphertext of a secret is its length plus 48 bytes: the length is visible, nothing is padded.
- **Sealing.** HPKE (RFC 9180) base mode, DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-128-GCM, one message per field, no aad; the `info` is the extension's label, the method, the request id, the context digest and the field name, each length-prefixed, so a ciphertext opens only for the field and request it was sealed for. A secret value is its text (UTF-8, or JSON number grammar for a number) and is never parsed as JSON.
- **Forms.** The root has only `type`, `properties`, `required`, `title` and `description`. A secret field is `writeOnly` or `x-sensitive` (a non-boolean is refused), must be text or a number, and has only `type`, `title`, `description`, `minLength`, `maxLength`, `minimum`, `maximum`, `writeOnly`, `x-sensitive`, `contentEncoding`, `contentMediaType` and `format` (`password` only); `format: "password"` needs the mark; whole-number bounds stay within 2^53 - 1. Schema, layout, values and plain text are read strictly: a repeated key is refused. `Form::check_layout` (called for a context's layout) refuses a layout that is not guatiao-intake's flat form document, a `password` widget on a question that is not secret, any widget but `password` on one that is, and a `visibleWhen` on a secret question's value. A frontend masks from `FormField::secret` alone. `check_plain` refuses a name the form lacks, a secret field holding a plain value, a missing required one, and a value that does not fit. `open_values` checks both halves before it opens anything.
- **Bindings.** `ecdsa-p256-sha256` for an identity that signs (the `webcrypto` peer method; `s` low, a high `s` is refused and a signer's is normalized); `x25519-hmac-sha256` for one that cannot (the `noise` method), which needs `VerifierBuilder::agreement` and a `Prover::agreeing` made for that one collector. Every binding names its audience; a verifier refuses one made for another id. Proof bytes are not identifiers. The Diffie-Hellman proof is not transferable and whoever holds the checker's static key can forge it to the checker. A binding ends at the maker's `lifetime` (`DEFAULT_LIFETIME` 240 s); a verifier takes it that long plus `skew` (30 s) after it ended and refuses one beyond `max_lifetime` (5 min) plus `skew` from now.
- **Secrets in memory.** Plaintexts, the per-request private key and the MAC key and HMAC state are zeroed on drop (`zeroize`); no `Debug` prints a value, key or proof. The identity key is the caller's: this module only calls the signer or key agreement it was given. Copies a move leaves on the stack and `serde_json` strings of plain values are not zeroed.
- **Not here** (`coqui-acp-secrets` does these). The handle store, the use and show tools, approvals, output scrubbing, routing to the owner's frontend, the frontend's pending-open table, request-id memory, certificate chains for `tls` identities, (the plain values of an answer are covered by `auth`).

## Errors

`Error` (`#[non_exhaustive]`): `Refused(String)` in the agent's own words, `Unsupported(String)`, `Timeout { what, limit }` naming the operation and how long it had, `Closed`, `Thread`.

## Gotchas

- The SDK turns on `serde_json`'s `preserve_order` for the whole build.
- ACP version 1 only here; the SDK refuses a peer that negotiates 2. The Dart client speaks both.
- ACP carries no token counts and no usage limits. Cost appears only when the agent reports it.
