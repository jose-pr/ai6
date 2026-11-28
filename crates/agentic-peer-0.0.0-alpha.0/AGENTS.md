# `agentic-peer` — API header

Header-file-style reference for the `agentic-peer` crate, so it can be used without reading the source. It ships inside the package and is self-contained.

No Cargo features. MSRV Rust 1.89. Version `0.0.0-alpha.0`, not published. It depends on `cabuya` (by revision, feature `sealed`), `coqui-transport` (by revision, no default features; its `Channel` and `Failure` are what every stream is generic over), `coqui-peer-webcrypto`, `mdns-sd`, `serde_json` and `url`. No `unsafe` code. Public modules: `companion`, `discovery`, `home`, `mcp_offer`.

The link itself is `cabuya`'s, and this crate re-exports none of it: the proof and its identities (`auth`), pairing by PIN (`pairing`), the streams (`mux`), serving under grants (`link`), the peers' records (`peers`) and TCP forwarding (`forward`, feature `forward`) are `cabuya::{auth, pairing, mux, link, peers, forward}`, documented in that crate's `AGENTS.md`. `Method` and `Identity` below are `cabuya::auth::{Method, Identity}`; `home`'s `Error` is `cabuya::Error`, and `discovery`'s and `companion`'s is `agentic_peer::Error`.

## `agentic_peer::mcp_offer`

```rust
pub const OFFER_PROTOCOL: &str = "mcp-offer/1";
pub const ADDRESS_VARIABLE: &str = "COQUI_MCP_OFFER_ADDRESS";
pub const TOKEN_FILE_VARIABLE: &str = "COQUI_MCP_OFFER_TOKEN_FILE";
pub fn handler(command: PathBuf, args: Vec<String>, tokens: PathBuf) -> cabuya::link::Handler      // what cabuya::link::Services::with takes; command is absolute (a relative one is refused) and starts shim; tokens is made private and holds one file per offer; the agent may start its server again on the same offer
pub fn handler_once(command: PathBuf, args: Vec<String>, tokens: PathBuf) -> cabuya::link::Handler  // the same for exactly one shim connection: when it ends the stream closes in order and the port and file go, so a second start needs a new offer
pub fn shim(input: impl Read + Send + 'static, output: impl Write) -> Result<(), ShimError>   // the stdio server, from its environment
pub enum ShimError { Setting(&'static str), Io(io::Error) }  // non_exhaustive
```

- A peer opens the stream, with a server name in `metadata` or none, and its first message is a stdio `mcpServers` entry to declare to its agent. Lines the shim carries are then that stream's messages, both ways. The wire is in `docs/link.md`.
- The shim refuses an address off this machine, so the token goes nowhere else. It returns when either end stops; call it with standard input and output, and exit.
- The port, the token and its file last as long as the stream. The entry holds no secret, since an agent may pass its environment to a program in arguments another user can read. What it starts has to run as the user who can read that file.

## `agentic_peer::home`

```rust
pub const HOME_ENV: &str = "COQUI_AGENTIC_PEER_HOME";
pub fn dir() -> Option<PathBuf>                        // $COQUI_AGENTIC_PEER_HOME, or coqui-agentic-peer under %LOCALAPPDATA%, $XDG_CONFIG_HOME or ~/.config
pub const fn identity_file(method: Method) -> &'static str          // "identity" (WebCrypto), "identity-noise" (Noise)
pub const METHOD_FILE: &str = "method";                             // the home's chosen method, one line: noise or webcrypto
pub fn chosen_method() -> Result<Option<Method>, Error>
pub fn identity_path(method: Method) -> Result<PathBuf, Error>
pub fn methods() -> Result<Vec<Method>, Error>                      // the methods this user has an identity of
pub fn store_path() -> Result<PathBuf, Error>
pub fn grants_path() -> Result<PathBuf, Error>
pub fn identity(path: Option<&Path>, method: Option<Method>) -> Result<Identity, Error>   // made the first time it is needed
pub fn store(path: Option<&Path>) -> Result<TrustStore, Error>     // this user's, or at path; keys of both methods
```

One identity per method and one trust store per user is what makes one pairing serve every program and capability. `identity`: with `path`, the key there, whose type is its method, or a new one of `method` (else `Method::DEFAULT`); without, this user's identity of `method`, which the home then records in `METHOD_FILE`; with no method, the one `METHOD_FILE` records, else the one identity this user has (then recorded), else a new `Method::DEFAULT` one (then recorded), and `Error::Identity` only for a home from before the record that holds both. So the method named last is the one every later program naming none uses. A key of another method than the one named is refused, never replaced. A file that cannot be written says why: the error carries the system's own.

## `agentic_peer::discovery`

```rust
pub const SERVICE_TYPE: &str = "_acp-agent._tcp.local.";
pub const DEFAULT_PATH: &str = "/acp";
pub const DESCRIPTORS_ENV: &str = "COQUI_ACP_DESCRIPTORS";
pub const MAX_CANDIDATES: usize = 64;
pub struct Candidate { pub label: String, pub url: String, pub peer: Option<String>, pub method: Method, pub source: Source, pub webtransport: Option<u16>, pub webtransport_hash: Option<String>, pub acp_versions: Vec<u32>, pub offers: Vec<String> }  // non_exhaustive
pub enum Source { Network, ThisMachine }  // non_exhaustive
pub fn Candidate::to_json(&self) -> Value
pub fn Advertisement::new(label: &str, tls: bool) -> Result<Advertisement, Error>
pub fn Advertisement::with_peer(self, peer: Option<String>) -> Advertisement
pub const fn Advertisement::with_method(self, method: Method) -> Advertisement   // Method::DEFAULT unless set
pub const fn Advertisement::with_webtransport(self, port: Option<u16>) -> Advertisement
pub fn Advertisement::with_webtransport_hash(self, hash: Option<String>) -> Advertisement
pub fn Advertisement::with_host(self, host: Option<String>) -> Result<Advertisement, Error>
pub fn Advertisement::with_offers(self, offers: &[String]) -> Advertisement   // a hint, at most 16; the link's own document decides
pub fn advertise(advertisement: &Advertisement, bound: SocketAddr) -> Result<Advertised, Error>   // withdrawn on drop
pub fn discover(wait: Duration) -> Vec<Candidate>                 // descriptors, then the link; a failed mDNS start leaves the descriptors; prints nothing
pub fn discover_report(wait: Duration) -> Discovery               // the same, with the directory check and why the link could not be searched
pub struct Discovery { pub candidates: Vec<Candidate>, pub directory: Option<DirectoryCheck>, pub network_error: Option<String> }   // non_exhaustive
pub fn browse(wait: Duration) -> Result<Vec<Candidate>, Error>    // the link only
pub fn read_descriptors(dir: &Path) -> Descriptors
pub struct Descriptors { pub candidates: Vec<Candidate>, pub directory: DirectoryCheck }   // non_exhaustive
pub enum DirectoryCheck { Private, SharedWithOthers, NotChecked }   // non_exhaustive; SharedWithOthers: group/other-writable, or owned by neither this user nor root (Unix); NotChecked: on Windows any folder but the default under %LOCALAPPDATA% (an override), whose ACLs are not read
pub fn descriptors_dir() -> Option<PathBuf>
pub fn candidate_from(record: &Record<'_>) -> Option<Candidate>
```

A record and a descriptor carry `method`, and `to_json` gives it as `"method"`. One naming no method is WebCrypto's; one naming a method this version cannot run is skipped. `candidate_from` reads an answer anyone on the link can send: it refuses this machine in any spelling, a link-local or multicast address, anything plain, no or a malformed key, and an unknown `txtvers` or `proto`, and cuts the label down, removing control and format characters. A descriptor may name only loopback, and may have no key. `read_descriptors` takes only regular files, spends at most 5 seconds, and reads nothing from a directory others can write to on Unix: it returns no candidates and `DirectoryCheck::SharedWithOthers`, the finding the application shows (a directory nobody meant to loosen). It prints nothing. On Windows it returns `NotChecked` and reads the files.

## `agentic_peer::companion`

```rust
pub const MAX_REQUEST: usize = 65_536;
pub const MAX_REPLY: usize = 1_048_576;
pub fn started_by_browser(first_argument: &str) -> bool
pub fn read_message(input: &mut impl Read) -> io::Result<Option<Vec<u8>>>                    // at most MAX_REQUEST
pub fn read_message_up_to(input: &mut impl Read, limit: usize) -> io::Result<Option<Vec<u8>>> // for a host that takes longer messages
pub fn write_message(output: &mut impl Write, message: &[u8]) -> io::Result<()>
pub fn run(input: &mut impl Read, output: &mut impl Write) -> io::Result<()>
pub enum Browser { Chrome, Chromium, Edge }          // non_exhaustive; Browser::ALL, name()
Registration::new(name: &str, description: &str, program: impl Into<PathBuf>) -> Result<Registration, Error>   // name: lowercase letters, digits, _ and .
Registration::{extensions(ids), browsers(&[Browser]), manifest_dir(dir), profile(user_data_dir)}   // the program is the caller's, never this library's
pub fn install(registration: &Registration) -> Result<Vec<String>, Error>
pub fn uninstall(name: &str, manifest_dir: Option<&Path>, profiles: &[PathBuf]) -> Result<(), Error>
pub fn installed(name: &str, manifest_dir: Option<&Path>, profiles: &[PathBuf]) -> Result<Vec<String>, Error>
```

There is no default host name: the caller names it. `install` writes the manifest and, on Windows, a registry key per browser under HKCU
(the manifest in `manifest_dir`, else `%LOCALAPPDATA%\coqui-acp\native`); on Linux and macOS a `NativeMessagingHosts/` file under each browser's
profile directory that exists, and under each `profile(user_data_dir)` given (what a browser started with `--user-data-dir` reads). `uninstall`
removes the same places; `installed` lists the ones that exist.

It answers `{ "op": "discover", "seconds": N }`, at most 10 seconds, and nothing else; it carries no agent traffic.

## Errors

`agentic_peer::Error` (`#[non_exhaustive]`): `Discovery(String)`, a peer that could not be advertised or searched for, or a companion that could not be installed. `home` returns `cabuya::Error` (`Identity(String)` for a key, identity file, trust store or directory that could not be used, with `Channel`, `Proof` and `OtherMethod` the link's own). A message never quotes a key file.

## Gotchas

- A browser takes a self-signed certificate only for WebTransport with `serverCertificateHashes`.
- mDNS stays on the link. `discover` does not check that an answer came from the link, so anyone who can reach UDP 5353 here can send a record. A record is a claim either way.
- On Windows, the first time a program listens or joins multicast the firewall may ask, and an answer of no leaves an inbound block rule for that path. `discover` then sees agents on this machine and none on others.
- A key file's permissions are set only on Unix; elsewhere it takes its directory's. Nothing checks them on reading.
