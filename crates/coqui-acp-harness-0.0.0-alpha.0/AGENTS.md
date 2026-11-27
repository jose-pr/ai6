# `coqui-acp-harness` — API header

Header-file-style reference for the `coqui-acp-harness` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-acp>.

MSRV Rust 1.89. A default build depends on `coqui-acp-api` (for the usage model), `base64`, `serde_json` and `uuid`, and `win32job` on Windows or `rustix` on Unix. Feature `kind` (off by default) adds `spawner`, the `acp_harness` kind's spawner on this machine and `Spawn` through any host's, and `provider`, which serves a harness over the kind's channel (with `guatiao-serde`, for catalogs). Feature `antigravity` (off by default) adds `antigravity`, which reads Antigravity's account and quota from its stored login and Google's service, or from its loopback port, and with it `serde`, `time` (parsing), `ureq` (rustls with ring and webpki roots, no default features), `memchr`, and `keyring-core` with the platform's store (`windows-native-keyring-store`, `apple-native-keyring-store` with `keychain`, or `zbus-secret-service-keyring-store` with `crypto-rust` on other Unix); its tests also use `rcgen` and `rustls`. Version `0.0.0-alpha.0`, not published; the API may change in any release before 1.0.

Imports: the root re-exports `acp`'s `AgentInfo`, `ConfigOption`, `Conversation`, `EventSink`, `Harness`, `HarnessError`, `McpServer`, `McpTransport`, `Open`, `OpenRequest`, `OptionValue`, `Settings`, `Takes`, `UsageTracker` and `serve`; `event`'s `Event`, `Outcome`, `PermissionRequest` and `Responder`; `process`'s `ErrorStream`, `Exit`, `Launch`, `LineWriter`, `PlainSpawn`, `Process`, `Spawn`, `Spawned`, `Starter`, `ending_once`, `exit_text`, `line_writer`, `set_starter` and `watch_exit`; and `stream`'s `Answer`, `LineHook`, `Step`, `Stream` and `Waiting`. The root also re-exports `mode`'s `Mode` and `settings`'s `ConnectionSettings` and `SessionLimits`, and with `kind` `spawner`'s `LocalSpawner` and `ThroughSpawner`. The modules `acp`, `event`, `lines`, `mode`, `process`, `settings`, `stream` and `version` are public, `spawner` and `provider` with `kind` and `antigravity` with `antigravity`.

No `unsafe` code but one Windows module (`#![deny(unsafe_code)]`): `antigravity/ports/win32.rs`, which asks `GetExtendedTcpTable` (with `windows-sys`, feature `antigravity`) for the listening sockets. No process-wide state.

## Processes (`coqui_acp_harness::process`)

```rust
#[non_exhaustive] pub struct Launch { pub program: OsString, pub args: Vec<OsString>, pub cwd: Option<PathBuf>,
    pub env: BTreeMap<OsString, OsString>, pub remove_env: Vec<OsString>, pub clear_env: bool }   // Clone, Default, PartialEq
pub fn Launch::new(program: impl Into<OsString>) -> Launch
pub fn arg(self, arg) -> Self;  pub fn args(self, args) -> Self;  pub fn cwd(self, cwd) -> Self
pub fn env(self, name, value) -> Self;  pub const fn clear_env(self, clear: bool) -> Self
pub fn display(&self) -> String          // the command line, for messages

pub trait Spawn: Send + Sync { fn spawn(&self, launch: &Launch) -> io::Result<Spawned>; }
pub trait Process: Send + Sync { fn kill(&self) -> io::Result<()>; fn try_wait(&self) -> io::Result<Option<Exit>>; }
#[non_exhaustive] pub struct Spawned { pub stdin: Box<dyn Write + Send>, pub stdout: Box<dyn Read + Send>,
    pub stderr: Option<Box<dyn Read + Send>>, pub process: Box<dyn Process> }
pub fn Spawned::new(stdin, stdout, stderr, process) -> Spawned
#[non_exhaustive] pub struct Exit { pub code: Option<i32>, pub signal: Option<i32> }   // Display: "exited with code 3"
pub fn exit_text(process: &dyn Process, wait: Duration) -> String   // how it ended, or "closed its output"
pub struct PlainSpawn;
pub type LineWriter = Arc<dyn Fn(&Value) -> io::Result<()> + Send + Sync>;
pub fn line_writer(stdin: Box<dyn Write + Send>, max_queued: usize, name: &str) -> io::Result<LineWriter>   // a thread of its own; a line past the bound fails
pub fn watch_exit(process: Arc<dyn Process>, poll: Duration, drain: Duration, done: Arc<AtomicBool>, end: impl FnOnce() + Send + 'static) -> io::Result<()>
pub fn ending_once() -> Arc<AtomicBool>
```

Each argument reaches the program as one argument, with no shell. `Launch`'s `Debug` names the environment's variables and counts the arguments, never showing their values.

`PlainSpawn` pipes all three streams. Its `kill` ends the tree. On Windows the process is put in a job object of its own just after it starts, and closing the job ends everything in it, at `kill` and when the host ends without one; a process started in the instant before it joins its job escapes. A process that had already ended when it would join (the system refuses it "access denied") has nothing to contain and starts as usual; any other refusal fails the start with the system's own error text. On Unix it leads a process group, which is sent `SIGKILL`, and it is not collected before that, so the group id stays its own; on Linux and Android `kill` first stops, then kills, every process descended from it, found through `/proc` by parent, those in a session of their own included. A watcher in the group (`/bin/sh` reading a pipe only the host holds) ends the group when the host ends without a kill; there a process that left the group escapes, and on other Unix systems it escapes `kill` too. `kill` then collects the process; killing one that has ended is no error, and once it is collected nothing is sent to its id or group again; `try_wait` then answers how it ended. On Unix `try_wait` leaves a running process uncollected. Dropping the process kills it as `kill` does.

Implement `Spawn` to run a harness elsewhere: in a container, over SSH, under a terminal.

## Spawner (`coqui_acp_harness::spawner`, feature `kind`)

```rust
pub struct LocalSpawner;                         // Clone, Debug, Default; #[must_use]
pub const fn LocalSpawner::new() -> LocalSpawner // 1 MiB of each output stream held unread
pub fn notices(self, hook: impl Fn(HarnessNotice) + Send + Sync + 'static) -> Self  // each notice its connection gives; none without one
pub const fn buffer(self, bytes: usize) -> Self  // past it the program waits to write
pub fn files(self, dir: impl Into<PathBuf>) -> Self  // serves the file methods in dir, which must exist
impl coqui_acp_api::ProcessSpawner for LocalSpawner
pub struct ThroughSpawner;                       // Debug
pub fn ThroughSpawner::new(spawner: Object<dyn ProcessSpawner>) -> ThroughSpawner
impl Spawn for ThroughSpawner
```

A bare program name is found on this process's own `PATH`, and in the folders `set_program_dirs(fn() -> Vec<PathBuf>)` names (asked at each start; a host's own runtimes), when the launch's `env` sets `PATH`: the system would look it up on that child `PATH` instead, and a folder a profile puts first would then decide which program an agent is. On Windows a name takes each `PATHEXT` extension unless it has one; a name with a directory, and one this process cannot find, are left to the system. A start the system refuses as `ETXTBSY` (Unix: a program just written is refused while another thread's `fork` holds its write descriptor) is tried again for about a second.

`set_starter(Arc<dyn Starter>)` has every process this crate starts itself start through `Starter::start(&Launch, ErrorStream) -> io::Result<Spawned>` instead of as this process's own child: `PlainSpawn`, `LocalSpawner` (a sign-in, an installer, a harness's models) and the Claude, Codex and Pi agents that start theirs through `PlainSpawn`. For a host inside an application whose process handling collects every child of its process that ends, where `try_wait` reports no child and a group kill assumes an uncollected one. A `Starter` starts the process in a group of its own with standard input and output piped and standard error as `ErrorStream` says (`Piped`, `IntoOutput`, `Discarded`), and returns a `Process` whose `kill` ends the group and collects it and whose `try_wait` says how it ended without collecting it (`Exit::new(code, signal)`). The watcher shell and the search for descendants that left the group are `PlainSpawn`'s own and are not part of a `Starter`. The last one set is used; none by default.

`LocalSpawner` is the spawner a host hands an `acp_harness` start by default: it starts a `SpawnSpec` as `PlainSpawn` starts a `Launch`, standard error apart, into its standard output or nowhere, as the spec says, and gives no terminal (`GUATIAO_ERR_UNSUPPORTED`). A missing program is `GUATIAO_ERR_NOT_FOUND`. A thread per output stream reads it into the buffer; `read` and `read_error` wait up to their timeout. `describe` says `local`, `separate_error`, `ends_with_host`, and `kills_escaped` on Windows, Linux and Android. Its link is always up. Destroying a process kills its tree. With `files(dir)` it serves the kind's file methods (`write_file`, `read_file`, `file_info`, `remove_file`) in `dir` and `describe` says `files`; a path is relative, and an absolute one, a `..` part, an empty one and a link leading out of `dir` are `GUATIAO_ERR_NOT_FOUND`. Without it `files` is false and the four methods are `GUATIAO_ERR_UNSUPPORTED`.

`ThroughSpawner` is `Spawn` over any host's spawner, so a translator served with `serve` starts its program wherever the host's spawner runs it. Every text of the `Launch` must be UTF-8. Its reads wait through a link that is down; dropping its standard input closes the program's.

## Provider (`coqui_acp_harness::provider`, feature `kind`)

```rust
pub fn Serving::new(settings: Settings, max_message_bytes: usize) -> Serving       // Debug; #[must_use]
pub fn Serving::open(self, spawner: Object<dyn ProcessSpawner>, sink: Object<dyn ChannelSink>,
    harness: impl FnOnce(Arc<dyn Spawn>) -> Result<Arc<dyn Harness>, ProviderError>) -> Result<Object<dyn Channel>, ProviderError>
pub fn Serving::open_shared(self, spawner: Arc<Object<dyn ProcessSpawner>>, sink, harness) -> ...   // as open; the spawner is shared (see Accounts). Unless `settings.on_notice` is set, each notice goes to `spawner.notice(HarnessNotice)`, the spawner held weakly
pub fn Serving::ending(self, f: impl FnOnce() + Send + 'static) -> Serving       // runs once after serve returned, before the spawner is released
pub fn shared(harness: impl Harness) -> Arc<dyn Harness>
pub fn track(thread: JoinHandle<()>);  pub fn to_json(&guatiao::Value) -> Value        // track: threads_done waits for it
pub fn threads_done() -> Status                                                    // a library's `unload` hook
pub fn takes_of(takes: &Takes) -> coqui_acp_api::Takes                              // resume true
pub fn schema(json: &Value) -> guatiao::Value                                      // any JSON as a Guatiao value
pub fn schema_of<T: guatiao::Schema>() -> guatiao::Value                           // what a catalog's `settings` and `inputs` hold: a type that derives `Schema`, a credential field `#[schema(sensitive)]`
pub fn settings_as<T: guatiao::FromValue>(settings: &guatiao::Value) -> Result<T, ProviderError>   // an account's settings read as T (`deny_unknown_fields` refuses a key the kind lacks); BAD_VALUE naming no value
pub struct Address;   // FromStr + Display, for a field `#[map(as_text)] #[schema(format = "uri")] base_url: Option<Address>`: an http(s) address of any length (`coqui_acp_api::is_http_url`), anything else refused with no value shown; .as_str()
pub fn own_kind(grant: &AccountGrant, offer: &AccountOffer) -> Result<(), ProviderError>   // the id is one and the offer lists the grant's kind; what an account provider asks before it keeps an account
pub fn account_refusal(catalog: &AccountCatalog, account: &AccountRef) -> Result<Option<String>, ProviderError>
```

What a provider of `acp_harness` needs to front a harness that `serve` serves. `Serving::open` builds the harness from a `Spawn` that starts its programs through the host's `spawner`, runs `serve` on a thread of its own and hands the host a `Channel`: each message sent is a line `serve` reads (a line feed inside a message that is not JSON is replaced by a NUL, so it stays one invalid line), each line it writes a message for the sink, which only one thread of the library calls. `send` waits while four messages wait on either side, so a sink that takes nothing holds the sender back (and `Settings::max_requests` bounds what `serve` has in hand); a message over `max_message_bytes` ends the connection `GUATIAO_ERR_BAD_VALUE`; `close` wakes a waiting `send` (`GUATIAO_ERR_GONE`), ends `serve`'s input, and `closed` follows once `serve` has returned and its lines are all handed over, with the host's status and reason, `GUATIAO_OK` when `serve` ended by itself and `GUATIAO_ERR_INTERNAL` when it failed. The spawner is destroyed before `closed`, whatever else holds the harness. A sink that refuses a message ends the connection with its status and message. A program that ends ends its session, not the connection: the host closes that.

`account_refusal` is for a provider that applies nothing to the harness's environment yet: `GUATIAO_ERR_BAD_VALUE` for an id that is not an account id or a kind the catalog does not list, a reason for any listed kind but `harness-login`, `None` for that one.

## Accounts (`coqui_acp_harness::{account, cli, login}`, feature `kind`; `cli` needs no feature)

What a provider that applies an account to a harness started through a host's spawner needs.

```rust
pub fn kind_info(id, title, description, settings: guatiao::Value) -> AccountKindInfo;  pub fn method_info(id, title, description, produces) -> LoginMethodInfo;  pub fn typed_method(..) -> LoginMethodInfo /* one `ask` step, remote */
pub fn provide_standards(catalog: &mut AccountCatalog, standard_of: impl Fn(&str) -> &str)   // what each kind provides, by its id
pub fn harness_catalog(full: AccountCatalog) -> AccountCatalog   // what the harness takes: no login method, no kind renews
pub fn provider_offer(full: AccountCatalog, usage: &[&str]) -> AccountOffer   // every kind but `harness-login`, the sign-in methods, `local`, and the kinds whose usage it reads
pub fn takes_no_settings(settings: &guatiao::Value) -> Result<(), ProviderError>;  pub struct NoSettings {}   // the harness's own login takes none
pub fn sole_credential<'a>(credentials: &'a Credentials, key: &str) -> Option<&'a str>   // the text of `key` when it is the only credential and not empty
pub fn bad(why: &str) -> ProviderError   // BAD_VALUE
pub struct SecretText;   // a credential as text: wiped when dropped, Debug shows its length; Deref<Target = str>
pub enum FileState { Missing, Text(SecretText), Planted /* longer than MAX_FILE_BYTES, a link, not a file */ }
pub struct Files;   // Clone; the held spawner's file methods
Files::new(Arc<Object<dyn ProcessSpawner>>);  .available() -> bool;  .spawner() -> &Arc<Object<dyn ProcessSpawner>> /* a call that also starts a program through it */;  .write(path, text);  .state(path) -> Result<FileState>;  .read(path) -> Result<Option<SecretText>>;
    .info(path) -> Result<Option<FileInfo>>;  .remove(path)
#[non_exhaustive] pub struct Watched { pub path, pub key, pub usable: fn(&str) -> bool, pub expires: fn(&str) -> Option<String>, pub identity: fn(&str) -> Option<String> }
Watched::new(path, key) /* usable: not blank */ .usable(f) .expires(f) .identity(f)
pub struct Accounts;   // one per provider; Accounts::new()
pub fn Accounts::hold(self: &Arc<Self>, files: Files, keeper: Option<Object<dyn AccountKeeper>>, id: &str, dir: &str, revision: u64,
    credentials: &Credentials, watched: Vec<Watched>) -> Result<Hold, ProviderError>
pub fn Accounts::observe(self: &Arc<Self>, files, keeper, id, dir, revision, credentials, watched) -> Result<Hold, ProviderError>   // as `hold`, but writes nothing: a login missing, blank or another account's is `lost` at once, one that differs from the grant's `renewed`; for an account provider's watch and renewal
pub struct Hold;   // Clone; #[must_use]
pub struct Watching;   // an `AccountWatch` over a `Hold`: Watching::begin(hold, poll: Duration) -> Object<dyn AccountWatch>; stop (or drop) looks a last time and lets go of the keeper and the spawner
Hold::look() -> Result<Looked { renewed: Vec<(Secret<Credentials>, Option<String>)>, lost: Vec<String> }>;  .lost() -> Option<String>;  .credentials() -> Result<Secret<Credentials>>;
    .expires_at() -> Option<String>;  .finish();  .watch(poll: Duration) -> Watcher   // Watcher::stop(); dropping it stops it too
pub fn rfc3339_ms(ms: i64) -> String;  pub fn jwt_expiry(token: &str) -> Option<String>;  pub fn fingerprint(text: &str) -> u64;  pub fn same_name(a: &str, b: &str) -> bool   // case-blind on Windows
pub const MAX_FILE_BYTES: u64;  pub const DEFAULT_POLL_MS: u64;
```

`hold` registers the keeper (a call with none hears nothing and tells the others) and places the files of `credentials` in `dir` (relative paths, through the spawner): absent, with no record of it or no hold on it, the grant's copy is written; planted (too long, a link, not a file), removed and replaced by the grant's copy, with a line on standard error; there with no record, the same text is kept, a login changed since the provider began (by the spawner's own clock, read from a file the hold writes and removes) is news (told, kept), a blank or older one replaced; there with a record and no other hold, the recorded file is kept unless the grant is a newer revision with other credentials, a login changed since the last look is news, a blank or older one is replaced by the grant's copy (a loss, once reported, is not reported again by the next hold); another hold on the file keeps it unless the grant is newer. **A change is read twice, 100 ms apart, and told only when the reads agree**: a file still being written is neither a renewal nor a loss. A change found by any look is **recorded, and given the account's next sequence, under one lock per account** (so an older reading never takes a later sequence) and then told to every keeper live for the account with no lock of the registry held. A text that is blank or that `usable` refuses is `lost`, once; a login that appears again is `renewed`; a login whose `identity` differs from the grant's is refused as lost and logged. **A keeper** may call back into the provider, end its own hold or close the connection from the call that tells it: ending a hold waits for the calls to its keeper that other threads have in flight, never for one on its own thread, and a `Watcher` stopped from its own thread is not joined; a keeper that blocks holds the thread that called it. `finish` (and drop) looks once more, lets go of the keeper (destroying it) and of the spawner. `Serving::open_shared(spawner, sink, build)` takes the spawner as an `Arc` so a hold can share it, and `Serving::ending(f)` runs `f` once on the serving thread after `serve` returned and before the spawner is released: stop the `Watcher` and call `finish` there.

```rust
pub fn cli::Cli::start(spawn: &dyn Spawn, launch: &Launch) -> io::Result<Arc<Cli>>;  .next(wait) -> Next { Piece(Piece { error, bytes }), Idle, Ended };  .write(bytes);  .close_input();
    .exit() -> Option<Exit>;  .wait(wait) -> Option<Exit>;  .kill();  .collect(input, limit, max_bytes) -> Done   // the output waits in a bounded queue (256 pieces): a reader that is busy holds the program back
pub fn cli::run(spawn, launch, input: Option<&[u8]>, limit: Duration, max_bytes: usize) -> io::Result<Done { exit, stdout, stderr, timed_out }>   // Done::succeeded(); inside a sign-in use Job::run
pub fn cli::plain(text) -> String;  pub fn cli::host_of(url) -> Option<String>;  pub fn cli::is_loopback(host) -> bool   // localhost and loopback or unspecified IPs, parsed
pub fn cli::address_in(text, hosts: &[&str]) -> Option<String>   // OSC 8 link, else the first http(s) address once something follows it; never one on this machine; with `hosts`, only on one of them or under one
pub fn login::supervise(spawner: Arc<Object<dyn ProcessSpawner>>, prompter: Object<dyn LoginPrompter>, timeout_ms: u64, work: Work) -> Result<LoginOutcome, ProviderError>   // Work: FnOnce(&Arc<Object<dyn ProcessSpawner>>, &Arc<Object<dyn LoginPrompter>>, &Job): the prompter is shared so the work can ask on a thread of its own
pub struct login::Job;  Job::start(&dyn Spawn, &Launch) -> Result<Arc<Cli>>;  Job::run(&dyn Spawn, &Launch, input, limit, max_bytes) -> Result<Done>;  Job::check() -> Result<()>      // GONE once the supervisor has answered
pub fn login::asked(result) -> result  // a prompter's GONE is `login cancelled`;  login::gone(why);  login::reason(output, typed: &[&str]) -> String   // a command's last line: no address (with or without a scheme), and nothing that echoes a value typed, whole or its first or last four characters
```

`supervise` runs the work on a thread of its own (tracked by `threads_done`) and answers when it does, or `GONE` `login cancelled` / `login timed out` when the person cancels or `timeout_ms` passes, after killing every command the work started through `Job::start` or `Job::run` (a command started as the supervisor answers is killed too); the work asks and starts nothing more once it has answered. When the supervisor answers first it gives the work one second to let go of the spawner and prompter, so a caller told of the end finds them gone unless the work is blocked in the host's prompter. `ThroughSpawner::link_wait(d)` bounds how long a read waits for a link that is down (five minutes by default).

The one process-wide state of the crate is the list of threads `provider` started; `threads_done` is `GUATIAO_ERR_BUSY` while one runs.

## Stdio provider (`coqui_acp_harness::stdio`, feature `kind`)

```rust
pub struct StdioProvider;                     // Debug; one harness as an `acp_harness`: no registered provider itself (a vendor's provider holds one)
pub fn StdioProvider::new(def: &'static StdioDef, settings: StdioSettings) -> StdioProvider;  .def() -> &'static StdioDef;  .settings() -> &StdioSettings;  .program_name() -> String
pub struct StdioSettings { pub executable: Option<String>, pub max_acp_version: u64 /* 1 */,
    pub max_requests: u64 /* 32 */, pub max_message_bytes: u64 /* 67108864 */,
    pub replay_max_bytes: u64 /* 0: no replay */, pub replay_max_item_bytes: u64 /* 4096 */, pub replay_redact_patterns: Vec<String> /* [] */,
    pub replay_waiting_secs: u64 /* 3600 */, pub replay_remembered_ids: u64 /* 4096 */, pub replay_waiting_max: u64 /* 64 */ }   // ToValue, FromValue, Schema, Default; StdioSettings::new()
pub struct StdioAccounts;                     // Debug; one harness's `account_provider`: no registered provider itself
pub fn StdioAccounts::new(def: &'static StdioDef, settings: StdioAccountSettings) -> StdioAccounts
pub struct StdioAccountSettings { pub executable: Option<String>, pub renew_poll_ms: u64 /* 30000, 0 looks only when a watch begins and ends */,
    pub command_timeout_ms: u64 /* 60000 */, pub signed_in_timeout_ms: u64 /* 5000: the longest a keyring may take to say whether the login is there */ }   // ToValue, FromValue, Schema, Default; StdioAccountSettings::new()
pub struct StdioDef { pub accounts: &'static HarnessDef, pub accounts_description: &'static str, pub signed_in: LoginCheck, pub logins: fn() -> Vec<LoginMethodInfo>, pub sign_in: Option<SignIn>,
    pub session_ids: Option<SessionIds>, pub import: Option<ImportSession>, pub export: Option<ExportSession>, pub import_unsupported: &'static str, pub export_unsupported: &'static str }   // what makes a harness one the stdio provider runs, beyond how it takes an account; `sign_in: None` answers `login` `UNSUPPORTED` with the table's `login_unsupported`; `import`/`export` are a harness's own store (an `UNSUPPORTED` or `INTERNAL` import falls back to a replay when allowed), `None` says the `*_unsupported` reason
pub type SignIn = fn(String, LoginRequest, Object<dyn ProcessSpawner>, Object<dyn LoginPrompter>) -> Result<LoginOutcome, ProviderError>;
pub type LoginCheck = fn(&Environment, Duration) -> SignedInCheck;   // `StdioDef::signed_in`: whether the harness's own login exists, from its variables and the places it keeps one; the time bounds a keyring   // the program, the request, the host's spawner and prompter
pub type ImportSession = fn(&StdioProvider, &SessionImport, Object<dyn ProcessSpawner>) -> Result<ImportReport, ProviderError>;  pub type ExportSession = fn(&StdioProvider, &SessionExport, Object<dyn ProcessSpawner>) -> Result<ExportPage, ProviderError>
pub struct SessionIds { pub native: fn(&str) -> Option<String>, pub host: fn(&str) -> Option<String> }   // how a harness that keeps an imported session under an id made from the host's UUID spells that id, and back; the relay turns one into the other
pub fn no_logins() -> Vec<LoginMethodInfo>
stdio_vendor! { harness: P, id: "p", accounts: A, id: "a", def: STDIO, settings: S, account_settings: AS }   // `#[macro_export]`: defines the two registered providers of a harness over a `StdioDef` static, each with `new(settings)`; the harness's description links the accounts provider (id `a`) sharing `executable`. `S: Default + Into<StdioSettings>`, `AS: Default + Into<StdioAccountSettings>`; the calling crate depends on `coqui-acp-api` (and guatiao) as every provider's does
pub mod login { pub struct Ctx; pub struct Child; pub fn run(..); pub fn settle(..) }   // what a harness's `sign_in` is written with: the CLI through the spawner, what it prints relayed to the person
pub mod vendors { pub mod grok; pub mod antigravity; pub mod opencode }   // each: `DEF` (the `HarnessDef`), `STDIO` (the `StdioDef`), `TARGET` (its access words), and its providers
pub mod signed_in { pub const FILE_LIMIT: u64; pub fn value(&Environment, name) -> Option<String>; pub fn first_set<'a>(&Environment, &[&'a str]) -> Option<&'a str>; pub fn by_variable(&Environment, &[&str]) -> Option<SignedInCheck>; pub fn home(&Environment) -> Option<PathBuf>;
    pub enum FileState { Missing, Empty, Present, Unreadable }; pub fn file_state(&Path) -> FileState; pub fn answer_for_file(&Environment, names: &[&str], path: Option<&Path>, place: &str) -> SignedInCheck; pub fn variables_note(&[&str]) -> String }
    // what each provider's `AccountProvider::signed_in` is made of: a variable counts when set and not empty (the call's `environment` over this process's; an empty value there is no login), a file when it holds something (not blank, `{}`, `[]` or `null`), and a reason says what was looked at and never a path or a value
pub mod keyring { pub enum Presence { Present, Absent, Unknown(String) }; pub fn present(service, user, target: Option<&str>, timeout: Duration) -> Presence; pub fn digest_prefix(text: &str, digits: usize) -> String }   // feature `keyring` (on with `antigravity`): whether a keyring or Keychain entry is there, with no secret in the answer (the Keychain is searched with no data loaded; Windows and the Secret Service give attributes) and an answer of unknown when it takes longer than `timeout`
pub fn antigravity::has_stored_login(&LoginSettings) -> Result<bool, LoginError>;   // feature `antigravity`: a whole sign-in is in the keyring or a file under `.gemini`, with no account read and no request
pub fn vendors::antigravity::signed_in_with(&Environment, Duration, Keyring) -> SignedInCheck;   // feature `antigravity`: Antigravity's own login; the keyring a test names stands in for the system's
pub mod vendors::grok { pub static DEF; pub static STDIO; pub const TARGET: Target; pub struct GrokProvider /* id `grok` */; pub struct GrokAccounts /* id `grok_accounts` */; pub const LOGIN, API_KEY, OIDC: &str }   // `GrokProvider::new(StdioSettings)`, `GrokAccounts::new(StdioAccountSettings)`
pub mod vendors::antigravity { pub static DEF; pub static STDIO; pub const TARGET: Target; pub struct AntigravityProvider /* id `antigravity` */; pub struct AntigravityAccounts /* id `antigravity_accounts` */ }
pub mod vendors::opencode { pub static DEF; pub static STDIO; pub const TARGET: Target; pub struct OpenCodeProvider /* id `opencode` */; pub struct OpenCodeSettings { pub executable: Option<String>, pub max_requests, max_message_bytes, replay_max_bytes, replay_max_item_bytes: u64, pub replay_redact_patterns: Vec<String>, pub replay_waiting_secs, replay_remembered_ids, replay_waiting_max: u64 } /* StdioSettings less the ACP version (1); OpenCodeSettings::new() */; pub struct OpenCodeAccounts /* id `opencode_accounts` */; pub struct OpenCodeAccountSettings { executable, renew_poll_ms /* 30000 */, command_timeout_ms /* 60000 */ } }
pub mod vendors::opencode::history { pub fn export(text: &str, id: &str) -> Result<Document, String>; pub fn is_native_id(id: &str) -> bool; pub fn native_id_of(uuid: &str) -> Option<String>; pub fn host_id_of(native: &str) -> Option<String>; pub const HARNESS: &str }   // `opencode export` output as a history; a UUID and `ses_` + its hex digits
pub mod vendors::opencode::import { pub fn import(doc: &Document, session: &str, cwd: &str) -> Result<Imported, String>; pub struct Imported { pub text: String, pub native_id: String, pub report: LossReport, pub seeded_from: Option<SeedPoint> } }   // a history as the file `opencode import` reads
pub mod accounts { pub struct HarnessDef; pub struct KindDef /* .provides; .provides_some(): the kind lists a standard or a setting's value picks one (an account provider offers and keeps only these) */; pub struct Setting /* .provides: (value, standard) pairs, listed as the schema's `x-provides-by-value` */; pub const PROVIDES_BY_VALUE: &str; pub fn provided(info: &AccountKindInfo, settings: &Value) -> Vec<String> /* what an account provides: its setting's listed standards, else the kind's */; pub struct AcceptDef; pub struct SlotDef; pub fn resolve_grant(def, grant); pub fn resolve(catalog, grant) /* provider */; pub fn catalog(def) /* what the harness takes: no login method, no kind renews */; pub fn apply(..); pub fn check_variable_name(name) /* the rule for a variable a key goes in */; pub fn known(..); pub struct Keeping /* an account provider's side of a def: offer, watch, renew, usage */;
    pub fn login_usable(text); pub fn identity_of(text); pub fn same_name(a, b) }                      // the machinery; each harness's table is a `HarnessDef` static in its own module (`vendors::*`, `coqui-acp-pi`)
```

A provider for agents that speak ACP on their own standard input and output, one provider per vendor over a `StdioDef`: `GrokProvider` (`grok agent stdio`, id `grok`), `AntigravityProvider` (`agy acp`, id `antigravity`) and `OpenCodeProvider` (`opencode acp`, id `opencode`); a harness this crate does not carry is a `StdioDef` and one `stdio_vendor!` in its own crate (Pi has its own translator, `coqui-acp-pi`) (`executable` replaces the program; the arguments are the harness's). The host's messages go to the program a line each and its lines come back as messages. The provider answers for itself only a message that is no request (`-32700`, `-32600`), the shape of `session/new`, `session/load`, `session/resume`, `session/fork` and `session/prompt` (`-32602`), and the working roots (`-32602` for a `cwd` or an additional directory outside them). A session id the program reports must be a UUID, as the kind says: nothing is translated. Measured: `agy acp` (1.5.3) answers `session/new` with a `sessionId` of the shape 8-4-4-4-12 lower-case hex, so Antigravity's ids are UUIDs. Measured on 1.5.9, in a home made for the test and signed in to nothing: `initialize` says `loadSession` and `sessionCapabilities` `resume`, `close`, `load`, `list` and `additionalDirectories`; each conversation is a SQLite database, `<home>/.gemini/antigravity-cli/conversations/<id>.db` (tables `steps` with a `step_type`, `status` and protobuf `step_payload` per step, `gen_metadata`, `executor_metadata`, `trajectory_meta` and `trajectory_metadata_blob`), beside `conversation_summaries.db` (a title, a preview and the folders of each conversation), and its steps are the protobuf messages the program embeds without publishing a schema (hundreds of step kinds, changing between releases); `session/load` of an id it does not hold answers as if it opened one and writes nothing, and `session/resume` of one is an error (`-32603`, "Failed to read session"). So a conversation it did not make cannot be written into its store, nor one of its own read out: `export_session` is refused with that reason, and an import is a replay. Grok's are unmeasured (it is not installed here) and may be refused by a host. `send` waits while four answers wait for the sink, and a request waits while `max_requests` have no answer taken by the sink yet; `close` cancels the prompts still out, waits up to three seconds for them to end (answering `cancelled` for one that does not), ends the program, lets go of the spawner, the keeper (destroyed unused) and the account's hold, and only then calls `closed`. A program that exits ends the connection (`GUATIAO_ERR_INTERNAL` with its exit code). It claims nothing of what the program takes beyond ACP version 1 (or 1 and 2 with `max_acp_version`), holds no requirement (every one is refused, one it cannot read `GUATIAO_ERR_BAD_VALUE`), reports no usage, refuses `export_session` `GUATIAO_ERR_UNSUPPORTED` (for Antigravity with the measured reason above, which a host shows its person; a host that records the sessions it serves exports them from its own record), imports by replay as below, and answers `models` as unknown. `describe` declares its account provider (`HarnessInfo::accounts`): `stdio_accounts`, sharing `harness` and `executable`; `OpenCodeProvider`'s is `opencode_accounts`, sharing `executable`.

**OpenCode** (measured on 1.19.6 and 2.3.2 with a stand-in model; accounts, `opencode export`, `session/list` and `session/load` also run against the real 1.19.6 in a throwaway home): it keeps its sign-in in `$XDG_DATA_HOME/opencode/auth.json` and every session in `opencode.db` beside it, and `XDG_DATA_HOME` moves both on every system measured (Windows included). An account therefore starts it with `XDG_DATA_HOME` set to the account's `state_dir`, so the person's own `auth.json` never meets the account's key and the account's sessions are its own; the person's `opencode.json` still applies. Its session ids are `ses_` and letters and digits, not UUIDs, and are passed on as they are. Kinds: `opencode-login` (the `auth.json` its own sign-in wrote, written to `opencode/auth.json` in the folder and watched; provides `opencode`; rotates), `opencode-api-key` (the kind an account of `anthropic-api`, `openai-api`, `gemini-api` or `xai-api` becomes, read from that API's variable; no endpoint moves) and `opencode-provider-key` (provides `opencode-key`; a key and the `variable` setting naming any environment variable (ASCII letters, digits and underscores, no digit first; `AOS_*`, `*_OVERLAY_ROOT` and `*_OVERLAY_STATE` are the host's and refused), the fourteen variables OpenCode was seen to list as a provider's sign-in being the schema's `examples` and no more; the `api-key` sign-in method asks for the variable and the key and sends nothing to the provider, so `checked` is false). The `provider-key` and `provider-code` methods (they produce `opencode-login`; inputs `provider` and, for a provider with several, `method`, the label `-m` takes; they differ in what they say they ask, a key or a code) run `opencode auth login -p PROVIDER [-m METHOD]` on the sign-in's data folder over **plain pipes**, which the program takes without a terminal (measured on 1.19.6), reads its prompt lines (colour and cursor codes removed), and answers them: the key prompt (`Enter your API key`) is asked of the person through the prompter and typed on one line (never in an argument or the environment), a selection nothing but Enter answers (a GitHub deployment type, a login method with no `-m`) takes its first choice, and `Go to:` and `Enter code:` become one `open_url` with the code. A page whose `redirect_uri` is a loopback address (the browser flows, which the program completes by listening on a port) is refused `UNSUPPORTED` with its reason, an argument that is empty, starts with `-` or holds a control character `BAD_VALUE`, and what the program says after `Error:` is the reason of a refusal. What it wrote, `opencode/auth.json` read once settled, is the account's credential (`checked` false: whether the provider takes it is not known without a model turn). The prompts are recorded in `tests/fixtures/opencode_auth_login.json`, which the stand-in prints and the tests read. A start on an account removes every key variable, `OPENCODE_AUTH_CONTENT` and `OPENCODE_CONFIG_CONTENT` the host passed, and keeps `OPENCODE_PERMISSION`, which is the host's access. `export_session` runs `opencode export --pure <id>` through the spawner on the account's data folder (the account's key is not passed), reads its JSON into a history (`opencode_history::export`: prompts, reasoning, tool calls with their results, answers, compaction summaries and aborted turns; the failed-call, running-call, abort, summary and attachment shapes follow OpenCode's published types and were not seen), and pages it; an id that is `ses_` and letters and digits, or a UUID (the host's name for a session written into the folder, below), is taken, anything else is `BAD_VALUE` and starts nothing, and a session the folder does not hold is `NOT_FOUND`. `import_session` writes the history into the account's data folder as a session of OpenCode's own (`opencode_import::import`): the file `opencode import` reads is made through the spawner's private files (`imports/<id>.json` in the folder, removed after) and the program runs on it in the session's working directory, which is the directory it gives the session (it ignores the file's `directory`). The session is `ses_` and the UUID's 32 hex digits, so the same UUID is the same session after a restart; the relay turns that id into the UUID for the host and the UUID into that id for the program (`Mapping::native`, stateless: the ids of sessions OpenCode made are as they are). Measured on 1.19.6: `import` takes ids that start `ses`, `msg` and `prt` and checks nothing after them, needs `info.{id, slug, title, version, time.created, time.updated}` and on a message and a part the keys `opencode_import`'s header lists, keeps a session that exists and adds only the messages whose ids it has not got (a message id is unique across sessions), and lists messages by their time; a `ses_` and 26-letter id is not required. Each step of a turn is an assistant message (a tool call with its result in the tool part, the answer after it the next message), a compaction summary is OpenCode's own (a user message with a `compaction` part and the summary as an assistant message with `summary`: the real program sent the model `What did we do so far?`, the summary and what followed, and none of what it replaced), calls of Claude, Codex and Pi become OpenCode's `read`, `write`, `edit`, `bash`, `grep`, `glob` and `webfetch` with its keys (an OpenCode history keeps its calls as they are, and a call with no equivalent is text, counted as `tool_as_note`), reasoning is dropped (counted as `thinking`: OpenCode takes none it did not write, with its signature) and a cancelled or failed turn ends its last message with OpenCode's own error. A real OpenCode then lists, loads and continues the session, and the follow-up reaches the model with the history in it. With no account (or an account that keeps no folder, the machine's own sign-in) the program is run on the store its environment names (`XDG_DATA_HOME`, else `~/.local/share`), and the file it reads is made in a new folder of this machine's temporary files (named `coqui-acp-opencode-` and a UUID, closed to others on Unix, removed with the file), so only a spawner that runs on this machine takes it: another is `UNSUPPORTED` with the reason. An account's folder with no private files in the spawner is `UNSUPPORTED` too, and either replays the history as context when `replay_max_bytes` is above 0 (as Grok does), the loss `replayed_as_prompt`. Measured on Windows (arm64), Linux (arm64, glibc) and macOS (x86_64), on 1.19.6, each with a throwaway home: `XDG_DATA_HOME` moves `opencode.db`, `auth.json` and the log on all three (with no XDG variable the folders are `~/.local/share/opencode`, `~/.config/opencode`, `~/.cache/opencode` and `~/.local/state/opencode` on all three, macOS's `Library` included); `auth list` lists a key variable under `Environment` and never its value; `export --pure` prints the session JSON; `import` takes the session's directory from its working directory; `auth login` takes a typed key and prints a device code on plain pipes; `opencode acp` initializes with `loadSession`. No difference was found between the systems. A first report that `opencode run` with the stand-in model never answered on macOS was the probe's: `opencode run` waits for the end of a standard input that is not a terminal, and the probe had handed it an open pipe; with the input at `/dev/null` it answers there, an ACP turn answers there, and the gated real-program test passed on all three systems (nothing here runs `opencode run`). Not measured: a completed sign-in, OAuth entries of `auth.json` and whether OpenCode rewrites the file, a provider variable beyond the fourteen and the few more cleared.

**`import_session` is a replay, and only when `replay_max_bytes` is set** (0, the default, refuses it `GUATIAO_ERR_UNSUPPORTED` with the reason: these agents keep no transcript the provider can write, and a replay sends the history to the agent's model, so it is the operator's choice). With a budget, the provider reads the history (`GUATIAO_ERR_BAD_VALUE` for an id that is not a UUID, one already waiting, or a history it cannot read; `UNSUPPORTED` for a version it does not read; `BUSY` past `replay_waiting_max` imports waiting (64 by default; 0 refuses every import)), builds `coqui_acp_history::replay::Replay` within the budget and keeps its block at the UUID. The answer is an `ImportReport` whose losses lead with `replayed_as_prompt` (`ImportReport::replayed()` is true; a native import such as Claude's or Codex's never reports it), then `history_cut`, `item_cut`, `non_text_omitted`, `thinking`, `tool_as_note` as they apply, `seeded_from` where the replay starts and `compacted` true when turns were cut. The first `session/resume` or `session/load` of that UUID on any connection starts a fresh session in the agent (`session/new`, the request's `cwd`, `mcpServers` and `additionalDirectories`), answers the host as the open it asked for (the answer holds no `sessionId`), and from then on the host speaks of the session by the UUID: the provider rewrites `params.sessionId` on the way in and out. The session's first `session/prompt` gets the replay block as its first block, once. Nothing is replayed through `session/load`'s history updates (the host has the history). An import opens once: a second connection finds nothing waiting and its resume reaches the agent as it is; an open the agent refuses puts the import back. Imports live in the provider's memory, not on disk, and one nobody opens expires after `replay_waiting_secs` (an hour; 0 forgets it at once). An import is bound to its UUID and nothing else, so **a host imports only at a UUID it minted and checks that it owns the session before it resumes or loads it**; the provider refuses (`BAD_VALUE`) an id the agent has already reported on any connection of the provider (the last `replay_remembered_ids` ids, 4096 by default; 0 remembers none), but cannot know ids of sessions that started before it. Ids are compared as UUIDs in any spelling (lower-case and hyphenated), and an id is live once the agent reports it in an answer or update, once a load or resume of it is answered, once it appears in a `session/list` answer, and once an import of it has opened; an import waiting at an id the agent then reports is dropped. An import taken for an open is given back when the connection ends before its first prompt (the open unanswered, the program gone, the write failed), and a second open with a request id already standing in for one is not converted. A replay carries tool output and messages verbatim, secrets included, to the agent's model: `replay_redact_patterns` (regular expressions) removes matches first. An open the working-root or shape checks refuse puts the import back; an agent answer without a string `sessionId` is a failure to the host, not a success. An empty history imports with no context and no loss. The text is quoted data; see `docs/session-history.md` in the repository for the format and what it defends.

Accounts (each harness's `DEF` is data; the catalog, the schemas and the checks on a grant come from it). A start on any kind but `harness-login` removes every name in the harness's `credential_variables` from what it starts (the host's environment included; on Windows whatever the case), then sets the kind's. Those names are the credentials, the cloud identities, the endpoints and the provider switches the harness reads: for Grok `XAI_API_KEY`, `GROK_CODE_XAI_API_KEY`, `GROK_AUTH`, the OIDC and external-program variables, `GROK_CLI_CHAT_PROXY_BASE_URL`, `GROK_XAI_API_BASE_URL`, `GROK_MODELS_BASE_URL`, `GROK_MODELS_LIST_URL`, `GROK_DEPLOYMENT_KEY`, `GROK_WEB_FETCH_PROXY` and the telemetry URLs and keys. Not measured: Grok's own `config.toml` `endpoints` keys beyond the two named in the notes (the account's directory has none), the variables Grok's binary reads that its README does not name, Antigravity's (it applies no kind but `harness-login`, which clears nothing, and the provider never passes `--cloud_code_endpoint`), and the general `HTTP(S)_PROXY` variables, which are the host's network and are left alone. A login file counts as settled when it is not blank and, when it looks like JSON, whole: half an object is never a renewal. A Grok renewal that leaves the login of another account in `auth.json` (the file says `account_id`, `user_id`, `sub` or `email`, which is unmeasured) is refused `GUATIAO_ERR_BAD_VALUE` and the grant's copy put back. A kind with a `state_dir` has its login file (`auth.json`) placed through `crate::account`; **nothing watches it and the keeper a call is handed is destroyed unused**: the account provider's `watch` does. A kind that needs files on a spawner that has none is `GUATIAO_ERR_UNSUPPORTED`; a bad setting, an unknown or missing credential, no directory for a kind that needs one, `GUATIAO_ERR_BAD_VALUE`.

| harness | kind | credential | variables and files | state_dir | rotates | renews |
| --- | --- | --- | --- | --- | --- | --- |
| grok | `grok-login` | `auth_json` | `GROK_HOME` = state_dir, `auth.json`; settings `proxy_base_url`, `xai_api_base_url` | yes | yes | `grok models` |
| grok | `xai-api-key` | `api_key` | `XAI_API_KEY`; `GROK_HOME` = state_dir, which holds no `auth.json` or `config.toml` to override the key or its endpoints | yes | no | no |
| grok | `grok-oidc` | `auth_json` (optional) | `GROK_OIDC_ISSUER`, `GROK_OIDC_CLIENT_ID`, `GROK_HOME`, `auth.json`; settings `issuer`, `client_id`, `proxy_base_url` | yes | yes | `grok models` |
| antigravity | none beyond `harness-login` (signed in by the method `start`) | | | | | |

The harness lists no login method and no kind renews (`accounts::catalog`); it has no `login`, `renew`, `models` or `usage` and takes no keeper. The account provider (below) lists the methods.

Login methods (Grok only): `device` (`grok login`, steps `open_url` with the code, remote), `browser` (`grok login --oauth`, loopback so not remote), `api-key` (asks for the key, checks it with `grok models`, `checked`). A sign-in runs the CLI in the login's directory on a thread of its own, so a cancel or the timeout answers `GUATIAO_ERR_GONE` (`login cancelled`, `login timed out`) within two seconds with the program killed; a key the CLI did not accept is `GUATIAO_ERR_BAD_VALUE` `login refused: ...` with no word of the CLI's output; the key is never in an argument. Antigravity's one method, `start` (`vendors::antigravity::START`, "Start Antigravity to sign in", makes `harness-login`), starts `agy acp` in the sign-in's directory, sends `initialize` and `session/new` and ends it: an answer to both is a signed-in login (`checked`), and an error or no answer is asked of the person as a form that says to start Antigravity, sign in there and continue, then tried again until it opens or the person cancels or the timeout passes; no credential is read or kept, and `agy acp --login`'s prompts, token location and Credential Manager use were not measured. Every other `login` is `GUATIAO_ERR_UNSUPPORTED`: Grok's identity-provider sign-in, which needs a provider to answer. `grok-external` (`GROK_AUTH_PROVIDER_COMMAND`, a program named in settings) is not a kind until the owner decides.

### Account provider (`StdioAccounts`; Grok's is `GrokAccounts`, id `grok_accounts`)

Offered by the same library file as the harnesses (`examples/stdio_provider.rs`); `AccountContract` holds it to the kind for each Grok kind (`tests/stdio.rs`, with `.program(true)` so the suite starts `fake_stdio_cli`). It runs no agent. `accounts::Keeping` is what it does with a `HarnessDef`, which `coqui-acp-pi`'s account provider uses too.

- `offers`: the kinds of the harness's table that provide a standard (Grok's three; Antigravity none, and its `login` takes only the method `start`), the login methods above, the automatic `local` account (the harness's own login: `offers().local`), and no usage kind: its accounts report no plan usage, so `usage` answers `AccountUsage::unsupported`, and the agent lists its models in its session, so `models` answers unknown with that reason (`Keeping::models`). Every call refuses `BAD_VALUE` an id that is not one and a kind it does not list, **including a kind of another provider that provides the same standard**.
- `renew`: runs the harness's renewal program (`grok models`) on the account's directory, then looks at the file: a changed file is the new credentials, an unchanged one `not_renewed_because`, a missing, blank or another account's one `GUATIAO_ERR_GONE`. The directory is **looked at, never filled from the grant**, so a host that wants the grant's copy there lets a harness start on the account first. A renewal that leaves the login of another account in `auth.json` is refused `BAD_VALUE` and the grant's copy put back.
- `signed_in`: whether the harness's own login exists, from its `StdioDef::signed_in`, with nothing started, no network and no secret in the answer; `StdioAccountSettings::signed_in_timeout_ms` bounds a keyring. Grok: `XAI_API_KEY` or `GROK_CODE_XAI_API_KEY` set and not empty, or `auth.json` in `GROK_HOME` holding something (`GROK_HOME` unset: `.grok` below the home is assumed, not measured, so a missing file there is `unknown`). OpenCode: a provider-key variable (`opencode::accounts::KEY_VARIABLES`) or `OPENCODE_AUTH_CONTENT` set and not empty, or `opencode/auth.json` in `XDG_DATA_HOME` (else `.local/share` below the home) holding something. Antigravity (feature `antigravity`): a whole sign-in in the keyring entry `gemini`/`antigravity` or in a file under `.gemini` below the home, as `AgyLogin` reads them, with no refresh and no request; a keyring that does not answer in time, and no file with the login, is `unknown`; without the feature it answers `unknown`.
- `watch`: for a kind that rotates and keeps a file, looks at `auth.json` every `renew_poll_ms` through the spawner and tells the keeper `renewed` or `lost`, once each; a half-written file is neither. An account that holds no login yet (`grok-oidc` without `auth_json`) is `UNSUPPORTED`.

## Stream (`coqui_acp_harness::stream`)

```rust
pub type LineHook = Arc<dyn Fn(&Value) -> bool + Send + Sync>;
pub type EventHook = Arc<dyn Fn(Event) -> bool + Send + Sync>;
pub type Answer = (String, Result<Value, String>);
pub type Waiting = Arc<Mutex<BTreeMap<String, mpsc::Sender<Result<Value, String>>>>>;
#[non_exhaustive] #[derive(Default)] pub struct Step { pub events: Vec<Event>, pub answers: Vec<Answer>, pub stop: Option<String> }
pub const fn Step::new(events, answers) -> Step;  pub const fn Step::stopping(events, answers, why: String) -> Step
#[non_exhaustive] pub struct Stream { pub name, pub process, pub max_line, pub exit_poll, pub exit_wait, pub tail, pub closed, pub sink, pub waiting, pub ended, pub on_line }
pub fn Stream::new(name: &'static str, process, tail, closed, sink) -> Stream
pub fn read(self, stdout: Box<dyn Read + Send>, thread_name: &str, line: impl FnMut(&Value) -> Step + Send + 'static, last: impl Fn() -> Vec<Event> + Send + Sync + 'static) -> io::Result<()>
```

`read` gives each JSON line to `on_line`, then `line`, on a thread of its own, until the output ends or the process has exited with its output still open. Then every waiting request fails, `last` says what is left, and `Event::Exited` follows once, unless the session was closed. A refusal nobody waits for is told as a notice. `Usage::elsewhere` marks another thread's spending: no update, no part of the turn's usage. `TurnEnd::Stopped { stop, result }` carries the harness's result text when it gives one; `PermissionRequest::details` the harness's own fields, for a host; no client sees either.

## Lines (`coqui_acp_harness::lines`)

```rust
pub enum Line { Json(Value), Text(String), TooLong(u64) }
pub fn read_lines(reader: impl Read, max_line: u64, each: impl FnMut(Line) -> bool) -> io::Result<()>
pub fn parse(bytes: &[u8]) -> Option<Line>        // None for a blank line
pub struct Tail;  pub const fn Tail::new(max_lines: usize, max_line_bytes: usize) -> Tail
pub fn push(&mut self, line: &str);  pub fn text(&self) -> String;  pub fn is_empty(&self) -> bool
pub fn cut(text: &str, max: usize) -> &str       // on a character boundary
pub fn one_line(text: &str) -> String            // each control character a space, for a log or a label
pub fn read_diagnostics(reader: impl Read, max_line: u64, tail: &Mutex<Tail>)   // standard error into `tail`, read to its end
```

`read_lines` reads until the end or until `each` answers `false`. A line over `max_line` bytes is handed over as `TooLong` with how much was read, and reading stops. A line ending is dropped, `\r\n` included. `Tail` keeps the last lines that were not protocol, for the message shown when a harness ends.

## Events (`coqui_acp_harness::event`)

`Event` (`#[non_exhaustive]`, `Debug`) is what a harness reports, in no protocol's terms:

| Variant | Carries |
| --- | --- |
| `SessionId(String)` | the harness's session id, once it says it |
| `UserChunk { message, content: Content }`, `UserEnd { message }` | the person's messages, in replayed history |
| `MessageChunk { message, text }`, `MessageEnd { message }` | agent text, and the end of a message |
| `ThoughtChunk { message, text }`, `ThoughtEnd { message }` | reasoning |
| `Tool(ToolCall)` | a tool call announced or changed: a `None` field is unchanged; `content` and `locations` replace the list |
| `ToolChunk { id, content: ToolContent }` | content added to a tool call |
| `Terminal(Terminal)` | a terminal of the agent's own: `id`, `command`, `output` to add, `exit: Option<Option<i32>>` once it ended |
| `Plan(Vec<PlanEntry>)` | the plan, whole |
| `Permission(PermissionRequest)` | a call that needs leave |
| `Usage(Usage)` | tokens (`input`, `output`, `cache_read`, `cache_write`, `cumulative`), `cost_usd`, `context_used`, `context_size`, `elsewhere`, `spend: Option<Spend>` (the session's running totals; the connection fills the session id), `turn: Option<TurnTokens>` (the running turn's own counts in all its requests: `input`, `output`, `cache_read`, `cache_write`, `reasoning`; the turn's answer reports these as its `usage`, with `thoughtTokens` when `reasoning` is set, in place of the last report's) |
| `TurnMeta(BTreeMap<String, Value>)` | facts about the running turn for the `_meta` of its answer, each under the key a host reads it by (`model`, or a namespaced key such as `codex:error_info`); a later value of a key replaces an earlier one and the turn's end clears them |
| `Buckets(Vec<Bucket>)` | the account's usage allowances (`coqui_acp_api::usage::Bucket`): one replaces the earlier of its id and keeps what it leaves unknown; `observed_at` 0 is stamped when the connection hears it |
| `Commands(Vec<Command>)` | commands by name: `name`, `description`, `hint` |
| `Options(Vec<ConfigOption>)` | the session's choices as they stand now, after a change not a client's own |
| `Title(String)` | the session's title |
| `Notice { severity: Severity, title }` | something for the person outside the conversation |
| `Compaction { id, summary: Option<String> }` | the conversation was compacted |
| `Denied(Vec<String>)` | tools the harness refused by its own rules this turn |
| `Child(Child)` | a child agent (subagent) announced or changed: `Child { id, parent_tool_call, title, task, state: Option<ChildState>, result, model, usage: Option<ChildUsage> }` (`#[non_exhaustive]`, `Child::new(id)`; a field left `None` is unchanged); `ChildState` is `Starting`, `Working`, `Completed`, `Failed`, `Cancelled`; `ChildUsage { total_tokens, tool_uses, duration_ms }`. Becomes a `tool_call_update` of `parent_tool_call` carrying only `_meta[CHILD_META_KEY]` |
| `InChild { child, parent_tool_call, event }` | an event of a child's own, told as `event` is with `_meta[CHILD_META_KEY] = {id, parentToolCallId}` added to each update |
| `TurnEnd(TurnEnd)` | `Stopped { stop, result }`, `Failed(String)` or `Limited(LimitHit)` (`coqui_acp_api::usage::LimitHit`: `bucket`, `resets_at`, `retry_after`, `source`, `text`, `observed_at`; 0 is stamped) |
| `Output(String)` | a line that was not protocol |
| `Exited(String)` | the harness ended, how, and what it wrote last |

`context_block(uri, text) -> String` writes a text resource as prompt text that it cannot close.

`Content`: `Text`, `Image { mime_type, data }` (base64), `ResourceLink { uri, name }`, `Resource { uri, mime_type, text, blob }`. `ToolCall` (`Default`): `id`, `name`, `title`, `kind: Option<ToolKind>`, `status: Option<ToolStatus>`, `input`, `output`, `locations: Option<Vec<Location>>`, `content: Option<Vec<ToolContent>>`; `ToolCall::new(id)`. `ToolContent`: `Text`, `Diff(Diff)`, `Terminal(id)`. `Diff`: `path`, `change: FileChange` (`Add`, `Modify`, `Delete`), `old_text`, `new_text`, `patch` (a git patch); `Diff::new(path, change)`. `ToolKind`, `ToolStatus`, `PlanStatus`, `Severity`, `Stop` and `OptionKind` have `as_str`, their names on the wire. Each `#[non_exhaustive]` struct has a `new` or `Default`.

```rust
pub struct PermissionRequest { pub tool: ToolCall, pub title: String, pub description: Option<String>,
    pub options: Vec<PermissionOption>, pub responder: Responder, pub details: Map<String, Value> }
pub fn PermissionRequest::new(tool, title, description, options, responder) -> PermissionRequest
pub fn details(self, details: Map<String, Value>) -> Self
pub struct PermissionOption { pub id: String, pub name: String, pub kind: OptionKind }
pub enum Outcome { Selected(String), Cancelled, Refused }
pub fn Responder::new(answer: impl FnOnce(Outcome) -> io::Result<()> + Send + 'static) -> Responder
pub fn answer(self, outcome: Outcome) -> io::Result<()>
```

A responder answers once; one dropped unanswered answers `Outcome::Cancelled`, so the harness is never left waiting. A tool call's input and output are the model's, untrusted.

## The ACP agent (`coqui_acp_harness::acp`)

```rust
pub trait Harness: Send + Sync + 'static {
    fn info(&self) -> AgentInfo;
    fn takes(&self) -> Takes;
    fn usage(&self) -> UsageSupport { UsageSupport::none() }     // the default is a true answer
    fn read_usage(&self) -> Option<Vec<Bucket>> { None }          // the account's usage with no session and no turn
    fn open(&self, request: OpenRequest, events: EventSink) -> Result<Box<dyn Conversation>, HarnessError>;
}
pub trait Conversation: Send + Sync {
    fn session_id(&self) -> String;
    fn options(&self) -> Vec<ConfigOption>;
    fn prompt(&self, content: Vec<Content>) -> Result<(), HarnessError>;
    fn interrupt(&self) -> Result<(), HarnessError>;
    fn set_option(&self, id: &str, value: &str) -> Result<Vec<ConfigOption>, HarnessError>;
    fn close(&self);
}
pub fn serve(harness: Arc<dyn Harness>, settings: Settings, input: impl Read, output: impl Write + Send + 'static) -> io::Result<()>
pub fn absolute_path(path: &str, what: &str) -> Result<PathBuf, String>
pub fn within_roots(path: &Path, roots: &[PathBuf]) -> bool                                 // links followed
pub fn path_admitted(path: &Path, roots: &[PathBuf], network_paths: bool) -> Result<(), String>   // for what a harness reads from a request besides its directories: a network or device path unless `network_paths`
pub fn directory_admitted(path: &Path, roots: &[PathBuf], network_paths: bool) -> Result<(), String>   // a session's `cwd` or additional directory; on Windows a share (\\server\share\work) is an ordinary absolute path, a device path is refused
pub struct EventSink;  pub fn send(&self, event: Event) -> bool     // Clone; waits while the queue is full; false once the session is gone
pub type NoticeHook = Arc<dyn Fn(NoticeKind, &str) + Send + Sync>;   // NoticeKind: Exited (how it ended, never what it last wrote) | Notice | Fault; one line, no client text
pub use coqui_acp_api::LIMIT_META_KEY;     // "dev.boriken.coqui.acp/limit", a `LimitHit` as JSON
pub const CHILD_META_KEY: &str;            // "dev.boriken.coqui.acp/child": {id, parentToolCallId} on a child's own update; a report adds title, task, state (starting|working|completed|failed|cancelled), result, model, usage {totalTokens, toolUses, durationMs}, each only when known
pub fn updates_of(events: &[Event], version: u8) -> Vec<serde_json::Value>;   // the session/update payloads a connection would send for these events, in order; for tests
pub const PLAN_ID: &str = "plan";
```

`open` returns once the session's id is known; for `Open::Load` it sends the history to `events` first. A turn's events and its `TurnEnd` go to the sink. Any `Conversation` method may be called while another runs.

`OpenRequest` (`new(cwd)`): `cwd`, `open: Open` (`New`, `Resume(id)`, `Load(id)`), `mcp_servers: Vec<McpServer>` (`name`, `transport: McpTransport` of `Stdio { command, args, env }`, `Http { url, headers }`, `Sse { url, headers }`; `Debug` shows names, never values), `system_prompt`, `additional_directories`, `meta` (the request's `_meta`, the client's, untrusted). `Takes` (`new()` and one setter per field): `images`, `embedded_context`, `mcp_http`, `mcp_sse`, `load`, `additional_directories`, `system_prompt`. `ConfigOption::new(id, name, current, values)` with `category` and `description`; `OptionValue::new(id, name, description)` (`selectable`, true unless `in_use_only()`: a value listed only because it is in use, told as `_meta[acp::SELECTABLE_META_KEY] = false`, which `mode::option` does for the current mode when it is not offered). `AgentInfo::new(name, title, version)`. `HarnessError`: `Refused(String)` is answered invalid params, `Failed(String)` internal error.

`serve` runs on the calling thread until the input ends, then closes every session it opened. Requests that take time run on threads of their own.

- A line that is not an object (a batch, a number) is answered `-32600` with id null, and one whose `method` is not text `-32600` with its id; an object with no method, result or error likewise.
- `initialize` is answered on a thread of its own, as asking a harness for `info` may take long. It answers version 2 to a client that asks for 2 or more, else version 1, up to `Settings::max_version`. Version 1: `agentCapabilities` with `loadSession`, `promptCapabilities`, `mcpCapabilities`, `sessionCapabilities` (`resume`, `close`, and `additionalDirectories` and `systemPrompt` when taken), `agentInfo`, `authMethods: []`. Version 2: `info` and `capabilities.session` (`prompt.image`, `prompt.embeddedContext`, `mcp.stdio`, `mcp.http`, `close`, `additionalDirectories`, `systemPrompt`). Any other request first is refused.
- `authenticate` answers `{}`. `session/new`, `session/load`, `session/resume` (version 2's `replayFrom: {"type": "start"}` replays, as `load`) check every part against `Takes` and `Settings` before the harness is asked: a `cwd`, MCP command or directory that is not absolute by this platform's rules (a drive or UNC path on Windows, `/…` elsewhere), a device path (`\\?\…`, `\\.\…`), an MCP command on a network path unless `network_paths`, a directory outside `working_roots` or holding `.` or `..` when roots are set (a folder on a share, `\\server\share\work`, is a directory like any other on Windows: compared by its parts without regard to case or separator, never resolved when there are no roots; elsewhere a path of two leading separators is refused as before), MCP servers when `client_mcp_servers` is off, two servers of one name, a transport not taken (version 2 has no SSE), a system prompt in a mode other than `append`, or a part not taken is invalid params. `new` answers `{sessionId, configOptions}` before the session's first events; `load` answers `{configOptions}` after its history; `resume` answers at once.
- A session being opened counts against `max_sessions` as an open one. `session/prompt` takes its turn on the thread that reads the client, so a `session/cancel` on the next line finds it; it takes text, images, resource links and resources as taken; any other block is invalid params, as is one larger than `max_prompt_bytes`. One turn at a time: a prompt while one runs is invalid request. Version 1 answers `{stopReason, usage}` at the turn's end; version 2 answers `{messageId}` at once, after a `user_message` and a `running` state update, and ends the turn with an `idle` state update carrying `stopReason` and `usage`. A failed turn is error `-32603` `Internal error` with the harness's words in `data.message` (version 1), or `idle` with `stopReason: "error"` and `error` (version 2). A limit is stop reason `refusal` with `_meta[LIMIT_META_KEY]` holding the `LimitHit` (its `kind` and `continuable` as the translator states them; `text_ids` in version 2 or `text_indexes` in version 1 name the turn's messages after its last tool call that say the limit and the ones after them, unless the translator named its own), and in version 2 a notice first.
- **Usage.** A harness whose `usage()` lists sources gets the usage extension (`coqui_acp_api` "Usage"): `initialize` advertises `dev.boriken.coqui.acp/usage` in `agentCapabilities._meta` (version 2: `capabilities._meta`) and reads the client's opt-in (`notify`, `account`) from `clientCapabilities._meta`; `_coqui_acp/usage` answers the connection's report with no session and no turn (a session's `spend` when `sessionId` names an open one, `-32602` for another); a harness that reports none answers it `-32601`. The report is built from `Event::Buckets`, `Event::Usage`'s `spend` and `TurnEnd::Limited`: a bucket replaces the earlier of its id and keeps what it leaves unknown, a change bumps `revision` and a repeat does not (only `observed_at` moves), a limit exhausts the bucket it names and is kept until its `resets_at` passes or a later bucket of that id shows room; `retry_after` is the harness's own word and is never worked out from `resets_at`. A client that set `notify` is sent `_coqui_acp/usage_update` `{sessionId, report}`, the whole report, on every change; `Settings::account` is the host's id for the account, over the client's. `Harness::read_usage` and `Conversation::read_usage` (default `None`) re-read the account with no turn, for a harness whose `UsageSupport::needs_turn` is false: each `_coqui_acp/usage` request asks one open session (the harness when none is open), a changed reading is a change (revision, notification) and an unchanged one is not, and the harness is read on a thread of its own once a session has opened and once each turn has ended. A harness with `needs_turn` is never asked, and one with `needs_session` (advertised as `"needs_session": true`) is asked only through an open session: with none open, `Harness::read_usage` is not called and the answer is what the connection has seen. `UsageTracker` (`new(support, account)`, `reports`, `notifies`, `client(notify, account)`, `snapshot(session, now)`, `buckets(incoming, now) -> bool`, `spend(session, spend) -> bool`, `limit(hit, now) -> bool`, `replace(prefix, incoming, now) -> bool`) is the connection's report with these rules, public for a host that reads buckets itself and merges them the same way; a `bool` answers whether the report changed other than in when it was read. `replace` takes a complete reading of the buckets whose ids start with `prefix`: each replaces the held bucket of its id outright (nothing of an earlier reading, such as a reset that has passed, is merged back), a held bucket of that prefix the reading leaves out is dropped, other ids are untouched.
- `session/cancel` calls `interrupt` once per turn, after the turn's prompt has reached the harness; the turn's end is answered `cancelled`. If it has not ended within `cancel_grace` it is answered `cancelled` anyway, its later end is dropped, and a prompt is refused until it comes.
- `session/set_config_option` (`configId`, `value` an id) calls `set_option` and answers `{configOptions}`: `id` in version 1, `configId` in version 2.
- `session/close` answers a running turn `cancelled`, closes the harness's session and lets go of it, and answers `{}`. Anything else with an id is method not found.
- A permission request is `session/request_permission`: version 1 `{sessionId, toolCall, options}`; version 2 `{sessionId, title, description, subject: {type: "tool_call", toolCall}, options}`, with `requires_action` before it and `running` after. An answer naming an option not offered, an outcome nobody knows, or an error is `Outcome::Refused` (this request alone is refused); the cancelled outcome, no answer within `permission_timeout`, or a client that has gone is `Outcome::Cancelled`; so is a question still open when its turn ends, is answered cancelled after `cancel_grace`, or its session closes, and the client's later answer is dropped.

What each event becomes:

| Event | Version 1 | Version 2 |
| --- | --- | --- |
| user, message, thought chunks | `user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk` | the same, with `messageId` |
| their ends | nothing | `user_message`, `agent_message`, `agent_thought` with no `content`, since `[]` clears the message |
| `Tool` | `tool_call` first, then `tool_call_update`; a diff as `{path, oldText, newText}`; `cancelled` as `failed` | `tool_call_update` only, with `name`; a diff as `{changes, patch}` |
| `ToolChunk` | `tool_call_update` with the whole content so far | `tool_call_content_chunk` |
| `Terminal` | its output added to its tool call's content | `terminal_update`, output base64-encoded |
| `Plan` | `plan` | `plan_update` with `planId` `plan` |
| `Usage` with context | `usage_update` (`used`, `size`, `cost` in USD) | the same |
| `Commands` | `available_commands_update` | the same, each tagged `type: "command"` |
| `Options` | `config_option_update` | `configOptions`: every choice with its current value |
| `Title` | `session_info_update` | the same |
| `Notice`, `Denied` | nothing | `notice` |
| `Compaction` | nothing | `compaction_summary_chunk`, `compaction_update` |
| `Exited` | the running turn fails | the same |

`Settings` (`#[non_exhaustive]`, `new()` and `Default`; set its fields): `max_line` (64 MiB; a longer client line ends the connection), `max_sessions` (8), `max_requests` (64 at once), `max_prompt_bytes` (16 MiB), `max_version` (2), `cancel_grace` (10 s), `permission_timeout` (`None`: as long as the person takes), `max_tools` (4096 tool calls tracked per session for version 1; past it the oldest is forgotten, and a later report of it is a new `tool_call`), `max_queued_events` (4096 events queued for a client that has not read them; past it the harness's `EventSink::send` waits, so a client that does not read holds the harness back; the same many are kept of what a session says before its answer, and past it they are dropped and the client told how many), `max_open_questions` (64 open at once per session, each holding a thread; past it one is answered cancelled without being asked), `client_mcp_servers` (true), `working_roots` (empty: any; links followed; also on `OpenRequest`, whose `path_admitted` holds what a harness reads from the request's other parts to the same rule, and for a harness that finds a session by its id alone), `network_paths` (false: an MCP command, and what is read from a request besides its directories, may not be on a network path as written or where links lead; a session's directories are not held to it), `max_tool_content_bytes` (1 MiB) and `max_tool_content_items` (256) (version 1 sends a tool call's content whole each time; past them the rest is left out and the client told), `on_notice: Option<NoticeHook>` (called with each notice and each harness exit, for a host that logs them; `Serving` sets it to the spawner's `notice` when unset), `on_usage: Option<UsageHook>` (called with the connection's report, as `_coqui_acp/usage` answers it with no session, each time it changes, with no lock held; none), `account: Option<String>` (the host's id for the account, echoed into usage reports; none).

## Modes (`coqui_acp_harness::mode`)

```rust
#[non_exhaustive] pub enum Mode { Manual, AcceptEdits, Auto, Plan, Bypass }   // Clone, Copy, Debug, PartialEq, Eq, Hash, ToValue, FromValue, Schema; as a setting, its id in snake case
pub const ALL: [Mode; 5]   // Mode::ALL, in the order a client cycles them
pub const fn id(self) -> &'static str          // manual, accept_edits, auto, plan, bypass
pub const fn name(self) -> &'static str;  pub const fn description(self) -> &'static str
pub fn from_id(id: &str) -> Option<Mode>
pub const OPTION_ID: &str = "mode"
pub fn option(current: Mode, offered: &[Mode]) -> ConfigOption          // category `mode`; values in `Mode::ALL` order; `current` is listed even when not offered
pub fn chosen(value: &str, offered: &[Mode]) -> Result<Mode, String>    // the reason, for `HarnessError::Refused`
pub const fn looseness(self) -> u8                                       // 0 for Plan up to 4 for Bypass, loosest last
pub const ACCESS_META: &str = "dev.boriken.coqui.acp/access"   // a session request's `_meta` key: {"sandbox": "none"|"workspace"|"read-only", "network": bool, "approvals": "ask"|"edits"|"never"}; a part left out is STANDARD's
pub struct Access { pub sandbox: Sandbox, pub network: bool, pub approvals: Approvals }   // ToValue, FromValue, Schema, Copy; Default = Access::STANDARD (no sandbox, network on, ask)
pub fn Access::from_json(&Value) -> Result<Access, String>;  pub fn Access::from_meta(&Map) -> Result<Option<Access>, String>;  pub fn Access::to_json(self) -> Value
pub struct Target;   // Clone, Copy, Debug, Eq (by id): a harness's id and its table; Target::new(harness: &'static str, table: fn(Access) -> Translation) (const); .harness() -> the AOS id; .well_formed() -> Result<(), String> (over every access: a part the harness cannot express passes nothing, a part it takes says what became of it, its JSON is an object). Each harness's crate defines its own: `coqui_acp_claude::access::TARGET` ("claude-code"), `coqui_acp_codex::access::TARGET` ("codex"), `coqui_acp_pi::access::TARGET` ("pi"), `vendors::{grok, antigravity, opencode}::TARGET`; OpenCode takes its access as one variable, `OPENCODE_PERMISSION` (a JSON object of tool -> allow|ask|deny), and has no sandbox
pub fn Access::translate(self, &Target) -> Translation   // the one table: every launch and every report reads it
pub fn Access::every() -> Vec<Access>   // every access the three settings can make (18)
pub struct Translation { pub sandbox: Part, pub network: Part, pub approvals: Part }   // Translation::new(sandbox, network, approvals) (const); .parts() -> [&Part; 3]; .mode() -> Option<&str> (the sandbox's word, `plan` when read-only, else the approvals'), .args() -> Vec<&str>, .env() -> Vec<(&str, &str)>, .json() -> Option<Map> (the parts' JSON objects, shallow-merged)
pub struct Part { pub word: Option<&'static str>, pub args: &'static [&'static str], pub env: &'static [(&'static str, &'static str)], pub json: Option<&'static str>, pub said: Result<&'static str, &'static str> }   // const constructors Part::{word, said, cannot, json, args, env}, Part::NOTHING; .applies(); an Err `said` (why the harness cannot express it) passes nothing. word: Codex's sandbox/approval policy, Claude Code's and Pi's mode, Codex's `network_access` ("true"/"false"); json: Claude Code's `--settings` sandbox object, Grok's session `_meta`
pub fn up_to(loosest: Option<Mode>, allow_bypass: bool) -> Vec<Mode>    // the host's mode and every stricter one; None: Plan only; Bypass only if allowed
pub fn widened(base: &[Mode], named: &[Mode], allow_bypass: bool) -> Vec<Mode>   // `base` and the modes a host named, Bypass only if allowed
pub const CUSTOM: &str = "custom"
pub fn custom_option(label: &str, offered: &[Mode]) -> ConfigOption     // a state no mode names, shown as `Custom: label` and not choosable
```

How much a harness asks, named once so a client shows one control over every translator. A translator offers only the modes its tool has, and a mode in use is always listed. Each translator's table of what a mode sets is in its own header: `coqui-acp-claude` (`permissionMode`) and `coqui-acp-codex` (approval policy, sandbox, reviewer, collaboration mode).

## Antigravity (`coqui_acp_harness::antigravity`, feature `antigravity`)

Two readers. `AgyLogin` reads the account, the plan and the quota from agy's stored login and Google's service, with no `agy` process; `AgyQuota` reads them from an open session's own language server, the fallback when `AgyLogin` fails while a session is open. Both make the same `AgyAccount` and the same buckets.

### From the stored login (`AgyLogin`)

```rust
pub struct AgyLogin;                     // new(LoginSettings); unmatched() -> u64; Debug (no token, email or secret)
impl AgyLogin { pub fn read(&mut self, models: &[AcpModel]) -> Result<LoginReading, LoginError>; }   // starts no process
#[non_exhaustive] pub enum LoginReading { NotSignedIn, SignedIn(SignedIn) }
#[non_exhaustive] pub struct SignedIn { pub account: AgyAccount, pub usage: Result<Vec<Bucket>, LoginError> }
#[non_exhaustive] pub struct LoginError { pub step: Step, pub cause: Cause }   // new(step, cause); Display, Error; Copy
#[non_exhaustive] pub enum Step { Keyring, LoginFile, Program, Refresh, UserInfo, LoadCodeAssist, FetchAvailableModels }   // Display
#[non_exhaustive] pub enum Cause { NoClient, Refused, Status(u16), Malformed, Io(io::ErrorKind) }   // Display
#[non_exhaustive] pub struct LoginSettings {   // Default, Debug; new(), then a builder method per field
    pub home: Option<PathBuf> /* USERPROFILE on Windows, else HOME */, pub gemini_dir: Option<PathBuf> /* <home>/.gemini */,
    pub keyring: Keyring /* Os */, pub keyring_timeout: Duration /* 5 s */, pub client: ClientSource /* Find */,
    pub token_url: String /* TOKEN_URL */, pub userinfo_url: String /* USERINFO_URL */, pub service: Option<String> /* none */,
    pub consumer_service: String /* CONSUMER_SERVICE */, pub gcp_service: String /* GCP_SERVICE */,
    pub user_agent: String /* USER_AGENT, "antigravity/cli/1.5.15" */, pub timeout: Duration /* 15 s, each request */, pub body: usize /* 4 MiB */ }
#[non_exhaustive] pub enum Keyring { Os, Off, Given(Arc<KeyringLookup>) }   // Default Os; Debug names the variant only
pub type KeyringLookup = dyn Fn(&str, &str) -> io::Result<Option<Vec<u8>>> + Send + Sync;   // (service, user) -> the stored bytes
#[non_exhaustive] pub enum ClientSource { Find, Program(PathBuf), Given(OAuthClient), Off }   // Default Find
#[non_exhaustive] pub struct OAuthClient { pub id: String, pub secret: Secret }   // new(id, secret); Debug shows neither
pub struct Secret;                       // new(value), expose() -> &str; Debug "Secret(..)", Display ".."
pub const KEYRING_SERVICE: &str = "gemini"; pub const KEYRING_USER: &str = "antigravity";
pub const SERVICE_VARIABLE: &str = "CLOUD_CODE_URL";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
pub const CONSUMER_SERVICE: &str = "https://daily-cloudcode-pa.googleapis.com";
pub const GCP_SERVICE: &str = "https://cloudcode-pa.googleapis.com";
```

The login, as agy 1.5.13 keeps it: the keyring entry of service `gemini` and user `antigravity` (Windows' Credential Manager generic credential `gemini:antigravity`, the macOS Keychain's generic password, the Secret Service item with `service` and `username`), and the files `<gemini>/jetski-standalone-oauth-token` and `<gemini>/antigravity-cli/antigravity-oauth-token`. Each is read; when more than one holds a login, the one whose token expires last (the one agy wrote last) is taken, the keyring's on a tie. A keyring with no entry, or none reachable (no session bus), counts as no login there; one that fails or does not answer within `keyring_timeout` (its thread is left to end on its own) is `Step::Keyring` when no file holds a login either. Stored bytes are read as UTF-8, else UTF-16 little-endian. The JSON is `{"token": {access_token, refresh_token, expiry}, auth_method, id_token_email, project_id, user_tier, tier_display_name}` or the older bare token; a login with no refresh token, or an `auth_method` `gcp` one with no `user_tier` or `project_id`, is `NotSignedIn`, as agy counts them; JSON that is not that shape is `Step::LoginFile` `Malformed`. Nothing is ever written: not the files, not the keyring.

A reading: `account.email` is the stored `id_token_email` and `account.plan` the stored `tier_display_name`, with no call. Then an access token: one this reader refreshed from the same refresh token while it lasts, else the stored one while it lasts (an `expiry` 10 s away counts as passed; none, or Go's zero time, never expires), else a refresh (below). With a token: userinfo when the email or name is unknown (its failure is ignored when the stored email is known, else it is the error), then `POST {service}/v1internal:loadCodeAssist` with `{"metadata": {"ideType": "ANTIGRAVITY", "pluginType": "GEMINI"}, "cloudaicompanionProject"?: project_id}` (the plan becomes `paidTier.name`, else `currentTier.name`; the project, when none is stored, its `cloudaicompanionProject`, a string or an object's `id`), then `POST {service}/v1internal:retrieveUserQuotaSummary` with `{"project": …}` (or `{}`), which is what agy's `/usage` lists: groups of models (`Gemini Models`, `Claude and GPT models`, `Gemini 4 Argon`), each with its windows (`weekly`, `5h`: `bucketId`, `displayName`, `remainingFraction`, `resetTime`, and `disabled` for a window that does not apply). Each window is one bucket (id `agy:<bucketId>`, label its `displayName`, `group` the group's name, a fixed window of the length its name says and the service's reset); a `disabled` one is `Status::Disabled` with no figure. The group's models are the names after `Models within this group:` in its description, else the one model the group is named for; they are `model_names`, and `models` is `Only` the agent's ids whose names are those (any effort after it, `Claude Opus 5.5 (High)`), `Unknown` when one has none. The service lists groups by the client that asks: it gives only the `Claude and GPT models` group to a `User-Agent` that is not `antigravity/cli/<version>` (measured 2026-11-23 with `antigravity`, which is why the default is `USER_AGENT`). With no session's models given (`models` empty) the buckets' coverage is matched against the models `POST {service}/v1internal:fetchAvailableModels` lists (each key an id and its `displayName` the name), asked once and kept for 600 s, so a group's buckets cover the ids of its models with nothing started; given models are used as they are and that call is not made. As measured on 1.5.15 (2026-11-25) the service gives one `Gemini Models` group naming `Gemini 4 Argon` with the other Gemini models and no group of its own for it (every Gemini 4 model has the same fraction and reset), so Argon's buckets are the Gemini group's; a `Gemini 4 Argon` group of its own, as earlier answers gave, is read like any other. Where the summary is not answered or holds no window, `POST {service}/v1internal:fetchAvailableModels` with the same body gives each model's own remaining fraction and reset time instead (buckets by reset time, as `AgyQuota` makes them). Each call has `Authorization: Bearer`, `Content-Type: application/json` and `User-Agent`. A call answered 401 with a stored token that has not expired refreshes once and is asked again. `{service}` is `settings.service`, else `CLOUD_CODE_URL`, else `gcp_service` for `auth_method` `gcp` and `consumer_service` for any other. Any failure after the login is read is the reading's `usage`, naming the step, and the account keeps what is known. Errors and every `Debug` hold no token, secret, email or name, and none of an answer. Requests follow no redirect and use the `HTTP(S)_PROXY` variables.

A refresh posts `grant_type=refresh_token`, `refresh_token`, `client_id` and `client_secret` to `token_url` and keeps the answer's `access_token` for `expires_in` in memory. The OAuth client is Antigravity's: `ClientSource::Given`, or read from its program (`Program(path)`, or `Find`: `agy` on this process's `PATH`, then on Windows `%LOCALAPPDATA%\agy\bin\agy.exe`), whose strings hold its client ids (`<number>-<32 lower-case letters and digits>.apps.googleusercontent.com`) and secrets (`GOCSPX-` and 28 characters) but not which is whose: each id is tried with each secret, in the order the program holds them, until the token endpoint answers 200 (a 400 or 401 tries the next; any other status, `Status`; none taken, `Refused`), and the pair that worked is tried first after. The program is read once per file (its path, length and time). With no client (`Off`, no program found, or one with no id and secret) an expired login's usage is `Step::Refresh` `Cause::NoClient`: Antigravity refreshes its own login when it next runs. A refresh never writes anything, so agy's stored token stays as it was and agy refreshes it itself.

Buckets as `AgyQuota`'s below, from `fetchAvailableModels`' `models` (a map: the key is the model's id, `displayName` its label, `quotaInfo {remainingFraction, resetTime}` its figures), in the order of the ids, so a group's bucket id is its first id in that order. `retrieveUserQuotaSummary` is not asked.

Unverified (from the program's code and strings, never seen answered): the request bodies of `loadCodeAssist` and `fetchAvailableModels` beyond `project` (agy's own `ClientMetadata` values and its exact `User-Agent` are unknown), that the map's key is the ACP model id and `displayName` its name, that a Credential Manager blob is UTF-8 (UTF-16 is read too), the Keychain and Secret Service entries (from go-keyring's published source, not this program), where agy is installed outside Windows, and which of `paidTier` and `currentTier` agy shows.

### Models with nothing started (`list_models`)

```rust
pub const MODELS_ARGS: &[&str];                                   // ["models"]
pub fn parse_models(text: &str) -> Vec<AcpModel>;                 // one per line of an id, a tab and a name; other lines left out
pub fn list_models(spawn: &dyn Spawn, program: &str, limit: Duration) -> ModelListing;
```

`agy models` prints `Fetching available models...` and then one `<id>\t<name>` line for each model its ACP session offers (`gemini-4-neon-high\tGemini 4 Neon (High)`), with no turn and no quota spent. `list_models` runs it through `spawn`, ended after `limit`, and answers a known `ModelListing` (id and name for each); no listing is `unavailable` (`NotInstalled` for a program not found, else `Failed`). The account provider of `agy` answers `unknown` for its models, since the harness lists them in a session: a host that wants them before a session calls this. Recorded output: `tests/fixtures/agy-1.5.15/models.txt`.

### From a session (`AgyQuota`)

```rust
pub struct AgyQuota;                     // new(ReadLimits); unmatched() -> u64; Debug
impl AgyQuota { pub fn read(&mut self, owners: &[u32], models: &[AcpModel]) -> Result<Vec<Bucket>, ReadError>;
    pub const fn account(&self) -> Option<&AgyAccount>; }   // who the last reading's answer said is signed in
#[non_exhaustive] pub struct AgyAccount { pub email: Option<String>, pub name: Option<String>, pub plan: Option<String> }   // plan: `userTier.name`, else `planStatus.planInfo.planName`
#[non_exhaustive] pub struct AcpModel { pub id: String, pub name: String }   // new(id, name)
#[non_exhaustive] pub struct ReadLimits { pub body: usize, pub connect: Duration, pub total: Duration, pub min_interval: Duration }
// Default: 256 KiB, 1 s, 3 s, none. new(), then body(), connect(), total(), min_interval().
#[non_exhaustive] pub enum ReadError { NoPort, NotOwned, Status(u16), Malformed, Io(io::ErrorKind) }   // Display, Error
pub fn loopback_listeners(owners: &[u32]) -> io::Result<Vec<SocketAddr>>   // ends the system's tool after 3 s
pub const BUCKET_PREFIX: &str = "agy:";   // pass to UsageTracker::replace
```

`agy acp` serves its language server on two random loopback ports (one plain HTTP, one HTTPS with a self-signed certificate) once a session is open, and answers `POST /exa.language_server_pb.LanguageServerService/GetUserStatus` with body `{}` and no token on both, with `Transfer-Encoding: chunked`. `read` lists the loopback listeners of `owners` (`loopback_listeners`: Windows `GetExtendedTcpTable`, IPv4 and IPv6, in this process and starting no program; Linux `/proc/<pid>/fd` joined with `/proc/net/tcp` and `tcp6`; macOS `/usr/sbin/lsof`), asks each by plain HTTP and then by HTTPS (certificate verification off, for a literal loopback address only, which `read` checks), and takes the first answer that reads as agy's status. `owners` are all the agent's processes: the reader does not find them. The port and scheme that answered are remembered; `read` lists the ports and checks the owner every time, and `read_remembered(models)` (`Option<Vec<Bucket>>`) asks the remembered port without listing and, when it does not answer, forgets it and returns `None`, so a caller that keeps an agent's reader lists once for the agent's life and again only after a failure. A failed reading forgets the port, so the next `read` looks at every port again. Listing the ports counts against `limits.total` (on macOS `lsof` is ended at the deadline).

No redirects, no proxy, the body counted as it is read against `limits.body` (an over-large body is `Malformed`), `limits.connect` for each connection and `limits.total` for the whole reading. An error never holds any of the answer. The answer is read into a struct with only each model's `label`, `modelId` and `quotaInfo`; the account's email, name and plan are kept for `AgyQuota::account`; its picture and every other field never become values. Non-200 is `Status`; an unparsable 200, an answer with no model list or a body past the limit is `Malformed`; no listener is `NoPort`, and a remembered port that no longer belongs to `owners` with nothing else answering is `NotOwned`.

Buckets, `reported`, unit `permille`, `limit` 1000, `used` unset: one for each distinct reset time (the Gemini 4 Argon models share seven days, the others five hours), id `agy:<first model's id>` (its `modelId`, else its label), `remaining` the least of its models' `round(remainingFraction * 1000)` (1000 only for exactly 1, 0 only for exactly 0, and 1 to 999 between), `exhausted` at 0 and `ok` otherwise (agy gives no warning). A fraction left out beside a valid reset is 0: the answer is protobuf JSON, which omits zero values (not yet seen from a real exhausted account). A model with no `quotaInfo`, a fraction that is present but not a number in `0..=1`, or a reset that is not RFC 3339 is left out. At fraction 1 the window has not started and agy reports a reset a whole window away that moves with every reading, so `resets_at` is `None` and the window length (reset less now, to the minute) is remembered for each of the group's models; a used bucket takes the length its models remember, so one that splits off a group keeps it, and carries the real `resets_at`. A reading is complete: pass its buckets to `UsageTracker::replace(BUCKET_PREFIX, ..)`, so a bucket a group no longer makes is dropped. The `label` is the length (`5 h`, `7 d`) when known. Coverage is `only` the ACP ids its models match, by `modelId` then by `label` against `AcpModel::name`, and `unknown` when any does not match (counted by `unmatched()`). `min_interval` answers a reading sooner than that after the last success with its buckets.

Not read: `planStatus` credits (the unit is unclear). Recorded answer: `tests/fixtures/agy-1.5.3/getuserstatus.json`, with the account's address, name and picture replaced.

## Settings (`coqui_acp_harness::settings`)

The sections every harness provider's configuration flattens (`#[map(flatten)]`), so their keys sit beside the provider's own; every number is a setting and any value is taken, and a key left out keeps its default.

```rust
pub struct ConnectionSettings {            // ToValue, FromValue, Schema, Default, Debug, Clone, PartialEq, Eq
    pub access: Option<Access>, pub allow_client_access: bool /* false */, pub allow_bypass: bool /* false */, pub modes: Option<Vec<Mode>>,
    pub max_acp_version: u64 /* 2 */, pub max_sessions: u64 /* 8 */, pub max_requests: u64 /* 32 */, pub max_message_bytes: u64 /* 64 MiB */,
    pub max_prompt_bytes: Option<u64> /* 16 MiB */, pub cancel_grace_ms: Option<u64> /* 10000 */, pub permission_timeout_ms: Option<u64> /* none: waits as long as the person takes */,
    pub max_tracked_tools: Option<u64> /* 4096 */, pub max_queued_events: Option<u64> /* 4096 */, pub max_tool_content_bytes: Option<u64> /* 1 MiB */,
    pub max_tool_content_items: Option<u64> /* 256 */, pub max_open_questions: Option<u64> /* 64 */ }
pub fn versions(&self) -> Vec<u16>          // [1, 2], or [1] when max_acp_version is below 2
pub fn message_limit(&self) -> usize        // max_message_bytes, for `Serving::new`
pub fn serving(&self) -> Settings           // `acp::Settings::new()` with the values given; a provider sets the working roots, the account and `client_mcp_servers` itself
pub struct SessionLimits { /* every field Option<u64>; absent keeps the translator's own default */
    max_line_bytes, tail_lines, tail_line_bytes, initialize_timeout_ms, request_timeout_ms, exit_wait_ms, exit_poll_ms, close_wait_ms,
    max_tools, max_queued_writes, title_bytes, version_wait_ms, version_bytes }   // ToValue, FromValue, Schema, Default
pub fn SessionLimits::{millis, count, bytes}(value: Option<u64>, slot: &mut Duration | usize | u64)   // sets the slot when a value is given
```

A section flattened beside another must not repeat one of its keys: the schema is then invalid and a library built on it offers no provider, which the contract suite shows as a provider that is not there. `coqui-acp-claude`, `-codex` and `-pi` each add a `SessionSettings` that flattens `SessionLimits` and holds what only that translator bounds.

## Versions (`coqui_acp_harness::version`)

```rust
pub fn untested(harness: &str, found: &str, tested: &[&str]) -> Option<String>
```

The warning for a version a driver was not tested with, or `None`. Nothing is refused for it. A release whose `major.minor` is a tested version's counts as tested (point releases come weekly, and a warning on each is noise); a version that is not dotted numbers is compared whole.

## Errors

`serve` fails with `io::Error` when reading the client failed, or `InvalidData` for a line over `max_line`. `HarnessError`'s `Display` is `refused: …` or `failed: …`.

## Gotchas

- Version 1 never gets a version 2 kind, and version 2 never gets `tool_call`.
- `serve` always answers a `Responder`; one dropped unanswered answers `Outcome::Cancelled`.
- A question that comes when no turn is running, or past `max_open_questions`, is answered cancelled and never asked.
- A cancel before the prompt reached the harness has the same grace: the turn is answered cancelled, and the prompt is interrupted if it arrives after all.
- Events sent before `open` returns are told after `session/new`'s answer, since a client drops updates of a session it does not know yet.
