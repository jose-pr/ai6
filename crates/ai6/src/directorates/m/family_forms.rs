//! The forms of the configuration families that no other module declares a record type for:
//! Theaters and the loadout families. Each type here describes the keys a record holds, for
//! the schema and form Outer Office draws; the records themselves are read by their own parsers
//! ([`Theaters`](crate::directorates::m::theaters::Theaters), the loadout catalog), which are
//! still what accepts or refuses a write.

use std::collections::BTreeMap;

use guatiao::{FromValue, Schema, ToValue};
use guatiao_intake::Form;

use crate::directorates::q::RunEnv;

/// How a Station reaches a Theater: one of the groups below.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct TheaterAccessForm {
    /// A host reached over SSH.
    #[schema(title = "SSH", advanced)]
    ssh: Option<SshForm>,
    /// A host reached by any other transport, with that transport's own settings.
    #[schema(title = "Transport")]
    transport: Option<TransportForm>,
    /// A template: each Mission gets a fresh Theater made from it.
    #[schema(title = "Made for each Mission")]
    ephemeral: Option<EphemeralForm>,
}

/// A transport provider and its settings.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct TransportForm {
    /// The transport that reaches the Theater, from those the Stations report.
    #[schema(title = "Transport", format = "provider:ai6_transport")]
    provider: String,
    /// That transport's settings.
    #[schema(title = "Transport settings", format = "settings-of:provider")]
    #[map(with = guatiao_serde::adapt::object, default)]
    config: serde_json::Map<String, serde_json::Value>,
}

/// SSH connection settings, as the `ssh` transport takes them.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct SshForm {
    /// Host name or address.
    #[schema(title = "Host")]
    host: String,
    /// Remote user; the Station's own user name when empty.
    #[schema(title = "User")]
    user: Option<String>,
    /// Remote port; 22 when empty.
    #[schema(title = "Port")]
    port: Option<u16>,
    /// Path of the private key file on the Station.
    #[schema(title = "Identity file")]
    identity: Option<String>,
    /// The host's key as `sha256:HEX`; a connection whose key differs is refused.
    #[schema(title = "Host key")]
    host_key: Option<String>,
    /// Accept the host's key at first contact when none is pinned. Weaker than pinning.
    #[schema(title = "Trust the key at first contact")]
    #[map(default)]
    trust_on_first_use: bool,
}

/// A template for Theaters a Theater provider makes per Mission.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct EphemeralForm {
    /// The Theater provider that makes them, from those the Stations report.
    #[schema(title = "Theater provider", format = "provider:ai6_theater_provider")]
    provider: String,
    /// That provider's settings.
    #[schema(title = "Provider settings", format = "settings-of:provider")]
    #[map(with = guatiao_serde::adapt::object, default)]
    config: serde_json::Map<String, serde_json::Value>,
    /// How many failed Missions' Theaters to keep.
    #[schema(title = "Failed Theaters kept")]
    keep_failed: Option<u64>,
    /// How long a kept Theater stays, in hours.
    #[schema(title = "Hours a kept Theater stays")]
    keep_failed_hours: Option<u64>,
}

/// Where a Theater lets the agent harness run.
#[derive(Debug, Clone, Copy, Default, ToValue, FromValue, Schema)]
pub(crate) enum AgentModeForm {
    /// Field mode when the Theater can host the harness, remote-hands mode otherwise.
    #[default]
    #[map(rename = "auto")]
    #[schema(label = "Auto: on the Theater when it can host the harness")]
    Auto,
    /// The harness runs on the Theater.
    #[map(rename = "local")]
    #[schema(label = "On the Theater: refused when it cannot host it")]
    Local,
    /// The harness runs on the Station and acts on the Theater through its transport.
    #[map(rename = "remote-hands")]
    #[schema(label = "On the Station, acting through the transport")]
    RemoteHands,
}

/// Whether a Run on a Theater must reach its Cable endpoint.
#[derive(Debug, Clone, Copy, Default, ToValue, FromValue, Schema)]
pub(crate) enum CableReachForm {
    /// A way to the endpoint is part of what the Theater must offer.
    #[default]
    #[map(rename = "required")]
    #[schema(label = "Required")]
    Required,
    /// A Run with no way to its endpoint starts without one, a reported weakening.
    #[map(rename = "optional")]
    #[schema(label = "Optional: a reported weakening")]
    Optional,
}

/// The runner that supervises harnesses on a Theater.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct RunnerForm {
    /// The `ai6` command on the Theater; `ai6` on its `PATH` when empty.
    #[schema(title = "Command")]
    command: Option<String>,
    /// The runner's state directory on the Theater.
    #[schema(title = "State directory")]
    state_dir: Option<String>,
    /// Directories or library files on the Theater that the runner loads providers from.
    #[schema(title = "Provider paths")]
    #[map(default)]
    plugin_paths: Vec<String>,
    /// The terminal provider the runner uses; one that suits the handler when empty.
    #[schema(title = "Terminal", format = "provider:ai6_terminal")]
    terminal: Option<String>,
    /// Milliseconds a Station waits to connect to the runner.
    #[schema(title = "Connect timeout (ms)", advanced)]
    connect_timeout_ms: Option<u64>,
    /// Milliseconds the runner may take to answer `initialize`.
    #[schema(title = "Initialize timeout (ms)", advanced)]
    initialize_timeout_ms: Option<u64>,
    /// Milliseconds the runner may take to answer any other request.
    #[schema(title = "Request timeout (ms)", advanced)]
    request_timeout_ms: Option<u64>,
}

/// What a Theater must have before Runs run there.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct RequiresForm {
    /// The operating system: `linux`, `macos`, `freebsd` or `windows`.
    #[schema(title = "Operating system")]
    os: Option<String>,
    /// Harness CLIs, as `claude>=2.1` or just `claude`.
    #[schema(title = "Harnesses")]
    #[map(default)]
    harnesses: Vec<String>,
    /// Other programs Runs use there, as `git` or `cargo>=1.89`.
    #[schema(title = "Tools")]
    #[map(default)]
    tools: Vec<String>,
    /// Free space, in MiB, in the Theater's working directory.
    #[schema(title = "Free disk (MiB)")]
    disk_mb: Option<u64>,
}

/// How a Theater is provisioned with what it lacks.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct ProvisionForm {
    /// The `ai6` build, a path on the Station, to install as the Theater's runner.
    #[schema(title = "Runner build")]
    runner: Option<String>,
    /// Provider libraries, paths on the Station, copied for the runner.
    #[schema(title = "Provider libraries")]
    #[map(default)]
    plugins: Vec<String>,
    /// The shell command that installs each program, by name.
    #[schema(title = "Install commands")]
    #[map(default)]
    install: BTreeMap<String, String>,
    /// Seconds an install command may run.
    #[schema(title = "Install timeout (seconds)")]
    install_timeout_secs: Option<u64>,
}

/// A Theater: the keys of a `[theater.<id>]` table that a form draws.
/// The egress key is kept as stored and edited as configuration.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct TheaterForm {
    /// How the Station reaches it.
    #[schema(title = "Reached by")]
    access: TheaterAccessForm,
    /// Labels an Operation's `station_labels` may match.
    #[schema(title = "Labels")]
    #[map(default)]
    labels: BTreeMap<String, String>,
    /// Where the harness may run.
    #[schema(title = "Harness runs")]
    #[map(default)]
    agents: AgentModeForm,
    /// Harnesses installed on the Theater.
    #[schema(title = "Harnesses installed", format = "provider:ai6_agent_handler")]
    #[map(default)]
    harnesses: Vec<String>,
    /// The directory a remote-hands Mission works in when its Operation names none.
    #[schema(title = "Working directory")]
    workdir: Option<String>,
    /// The runner that supervises harnesses on the Theater.
    #[schema(title = "Runner", advanced)]
    runner: Option<RunnerForm>,
    /// What the Theater must have before Runs run there.
    #[schema(title = "Requirements", advanced)]
    requires: Option<RequiresForm>,
    /// How to install what the Theater lacks.
    #[schema(title = "Provisioning", advanced)]
    provision: Option<ProvisionForm>,
    /// Whether a Run here must reach its Cable endpoint.
    #[schema(title = "Cable endpoint", advanced)]
    #[map(default)]
    cables: CableReachForm,
}

/// An Agent's loadout: the keys of a `[loadout.<agent>]` table.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct LoadoutForm {
    /// The sandbox profile its Runs run in.
    #[schema(title = "Sandbox", format = "record:sandboxes")]
    sandbox: Option<String>,
    /// The MCP servers its Runs get.
    #[schema(title = "MCP servers", format = "record:mcps")]
    #[map(default)]
    mcp: Vec<String>,
    /// The overlays its Runs are issued.
    #[schema(title = "Overlays", format = "record:overlays")]
    #[map(default)]
    overlays: Vec<String>,
    /// The secrets its Runs may be leased.
    #[schema(title = "Secrets", format = "record:secrets")]
    #[map(default)]
    secrets: Vec<String>,
    /// Values its Runs get in their environment; no secret.
    #[schema(title = "Environment", advanced)]
    #[map(default)]
    env: RunEnv,
}

/// A sandbox profile: the keys of a `[sandbox.<name>]` table.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct SandboxForm {
    /// The program that enforces the profile.
    #[schema(title = "Backend")]
    #[map(default = "bubblewrap".to_owned())]
    backend: String,
    /// Directories the Run may read and write.
    #[schema(title = "Read and write")]
    #[map(default)]
    read_write: Vec<String>,
    /// Directories the Run may only read.
    #[schema(title = "Read only")]
    #[map(default)]
    read_only: Vec<String>,
    /// Hosts the Run may reach.
    #[schema(title = "Network")]
    #[map(default)]
    network: Vec<String>,
}

/// An MCP server a loadout names: the keys of an `[mcp.<name>]` table.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct McpForm {
    /// The program and its arguments.
    #[schema(title = "Command")]
    command: Vec<String>,
    /// Tools of the server the Run may not call.
    #[schema(title = "Tools denied")]
    #[map(default)]
    deny: Vec<String>,
}

/// An overlay: the keys of an `[overlay.<name>]` table. Exactly one of the source and the path.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct OverlayForm {
    /// Where the overlay is fetched from.
    #[schema(title = "Source")]
    source: Option<String>,
    /// A directory on the Station, absolute or relative to the configuration directory.
    #[schema(title = "Path")]
    path: Option<String>,
    /// The digest the overlay must have.
    #[schema(title = "Pinned to")]
    pin: Option<String>,
    /// Secrets the overlay may use.
    #[schema(title = "Secrets", format = "record:secrets")]
    #[map(default)]
    secrets: Vec<String>,
}

/// How a loadout secret reaches a Run, never its value: the keys of a `[secret.<name>]` table.
/// The provider's own further fields are kept as stored and edited as configuration.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct SecretForm {
    /// The variable the value is leased to.
    #[schema(title = "Environment variable")]
    env: Option<String>,
    /// Whether a Mission's commits may carry it past the green ink gate.
    #[schema(title = "Green ink", advanced)]
    #[map(default)]
    green_ink: bool,
    /// The kinds of Mission that may be leased it.
    #[schema(title = "Kinds of Mission")]
    #[map(default)]
    kinds: Vec<String>,
    /// Where it may be delivered.
    #[schema(title = "Delivered to")]
    #[map(default)]
    targets: Vec<String>,
    /// The secret provider that holds it: a configured instance's name or a provider's ID.
    #[schema(title = "Provider", format = "record:secret_providers")]
    provider: Option<String>,
    /// Its key at that provider.
    #[schema(title = "Key")]
    key: Option<String>,
}

/// A configured secret provider: the keys of a `[secret_provider.<name>]` table, the provider and
/// then its settings beside it.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct SecretProviderForm {
    /// The secret provider, from those the Stations report.
    #[schema(title = "Provider", format = "provider:ai6_secret")]
    provider: String,
    /// That provider's settings, written beside the provider.
    #[schema(title = "Provider settings", format = "flat-settings-of:provider")]
    #[map(with = guatiao_serde::adapt::object, default)]
    settings: serde_json::Map<String, serde_json::Value>,
}

/// How a pool picks among its accounts.
#[derive(Debug, Clone, Copy, Default, ToValue, FromValue, Schema)]
pub(crate) enum PoolStrategyForm {
    /// The account with the most quota left.
    #[default]
    #[map(rename = "headroom")]
    #[schema(label = "Headroom: the most quota left")]
    Headroom,
    /// Each in turn.
    #[map(rename = "round_robin")]
    #[schema(label = "Round robin")]
    RoundRobin,
    /// The highest priority that is free.
    #[map(rename = "priority")]
    #[schema(label = "Priority")]
    Priority,
}

/// How a harness reads an account's credential.
#[derive(Debug, Clone, Copy, Default, ToValue, FromValue, Schema)]
pub(crate) enum AccountKindForm {
    /// A Claude Code token from `claude setup-token`.
    #[default]
    #[map(rename = "claude-oauth-token")]
    #[schema(label = "Claude Code token")]
    ClaudeOauthToken,
    /// An Anthropic API key.
    #[map(rename = "anthropic-api-key")]
    #[schema(label = "Anthropic API key")]
    AnthropicApiKey,
    /// An Anthropic bearer token.
    #[map(rename = "anthropic-auth-token")]
    #[schema(label = "Anthropic bearer token")]
    AnthropicAuthToken,
    /// An OpenAI API key.
    #[map(rename = "openai-api-key")]
    #[schema(label = "OpenAI API key")]
    OpenaiApiKey,
    /// A Codex access token.
    #[map(rename = "codex-access-token")]
    #[schema(label = "Codex access token")]
    CodexAccessToken,
    /// The directory of an existing login.
    #[map(rename = "config-dir")]
    #[schema(label = "Directory of an existing login")]
    ConfigDir,
    /// The login the harness keeps for itself.
    #[map(rename = "harness-login")]
    #[schema(label = "The harness's own login")]
    HarnessLogin,
}

/// One provider account of a pool: `[pool.<id>.account.<name>]`.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct AccountForm {
    /// How the harness reads the credential.
    #[schema(title = "Kind")]
    kind: AccountKindForm,
    /// The secret that holds the credential.
    #[schema(title = "Secret", format = "record:secrets")]
    secret: Option<String>,
    /// The directory of the login, for the kinds that use one.
    #[schema(title = "Directory")]
    dir: Option<String>,
    /// The variable the credential is delivered in, when not the kind's own.
    #[schema(title = "Environment variable")]
    env: Option<String>,
    /// Higher is preferred.
    #[schema(title = "Priority")]
    #[map(default)]
    priority: i64,
    /// Runs that may use it at once.
    #[schema(title = "Most at once")]
    max_concurrent: Option<u64>,
    /// Labels a route or Operation may ask for.
    #[schema(title = "Labels", advanced)]
    #[map(default)]
    labels: BTreeMap<String, String>,
    /// Files of the login sealed in the Registry.
    #[schema(title = "Login files", advanced)]
    #[map(default)]
    files: Vec<String>,
    /// The command that renews the login.
    #[schema(title = "Renew command", advanced)]
    #[map(default)]
    renew_command: Vec<String>,
    /// Environment the renew command gets.
    #[schema(title = "Renew environment", advanced)]
    #[map(default)]
    renew_env: BTreeMap<String, String>,
}

/// An account pool: the keys of a `[pool.<id>]` table.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct PoolForm {
    /// The harness the pool's accounts are for, from those the Stations report.
    #[schema(title = "Handler", format = "provider:ai6_agent_handler")]
    handler: String,
    /// The Operations that may draw from it; any when empty.
    #[schema(title = "Operations", format = "record:operations")]
    #[map(default)]
    operations: Vec<String>,
    /// How it picks among its accounts.
    #[schema(title = "Strategy")]
    strategy: Option<PoolStrategyForm>,
    /// The command that logs an account in.
    #[schema(title = "Login command", advanced)]
    login_command: Option<Vec<String>>,
    /// Only a route or Mission that names an account may draw from it.
    #[schema(title = "Named accounts only")]
    #[map(default)]
    named_only: bool,
    /// Its accounts, by name.
    #[schema(title = "Accounts")]
    #[map(default)]
    #[form(form = AccountForm)]
    account: BTreeMap<String, AccountForm>,
}

/// What a provider library needs before it loads.
#[derive(Debug, Clone, Copy, Default, ToValue, FromValue, Schema)]
pub(crate) enum PluginRequireForm {
    /// The pin or the publisher's signature: either suffices.
    #[default]
    #[map(rename = "either")]
    #[schema(label = "Either the pin or the signature")]
    Either,
    /// The pin and the publisher's signature.
    #[map(rename = "both")]
    #[schema(label = "Both the pin and the signature")]
    Both,
}

/// One provider library a Station accepts: the keys of a `[[library]]` entry, filed under its
/// `file` (the record's key). At least one of the pin and the publisher is needed.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct PluginForm {
    /// The digest its bytes must have, as `sha256:HEX`. A SHA-1 pin is a reported weakening.
    #[schema(title = "Pinned to")]
    pin: Option<String>,
    /// The key whose signature it must carry, as `ed25519:HEX`; `FILE.sig` beside the library
    /// signs its SHA-256. A publisher alone admits anything it signed, a reported weakening.
    #[schema(title = "Signed by")]
    publisher: Option<String>,
    /// Whether the pin and the publisher are each enough, or both are needed.
    #[schema(title = "Needs")]
    require: Option<PluginRequireForm>,
}

/// A TLS version the policy offers.
#[derive(Debug, Clone, Copy, ToValue, FromValue, Schema)]
pub(crate) enum TlsVersionForm {
    /// TLS 1.2.
    #[map(rename = "1.2")]
    #[schema(label = "TLS 1.2")]
    V12,
    /// TLS 1.3.
    #[map(rename = "1.3")]
    #[schema(label = "TLS 1.3")]
    V13,
}

/// A digest a certificate pin may use.
#[derive(Debug, Clone, Copy, ToValue, FromValue, Schema)]
pub(crate) enum TlsDigestForm {
    /// SHA-256.
    #[map(rename = "sha256")]
    #[schema(label = "SHA-256")]
    Sha256,
    /// SHA-384.
    #[map(rename = "sha384")]
    #[schema(label = "SHA-384")]
    Sha384,
    /// SHA-512.
    #[map(rename = "sha512")]
    #[schema(label = "SHA-512")]
    Sha512,
    /// SHA-1, for legacy interoperability only.
    #[map(rename = "sha1")]
    #[schema(label = "SHA-1: legacy only")]
    Sha1,
}

/// A named set of algorithms the policy allows.
#[derive(Debug, Clone, Copy, ToValue, FromValue, Schema)]
pub(crate) enum TlsProfileForm {
    /// Mozilla's server-side guideline, intermediate.
    #[map(rename = "mozilla")]
    #[schema(label = "Mozilla intermediate")]
    Mozilla,
    /// FIPS 140-3 approved algorithms; algorithm selection only.
    #[map(rename = "fips-140-3-compliant")]
    #[schema(label = "FIPS 140-3 algorithms")]
    Fips1403Compliant,
    /// FIPS 140-2 approved algorithms; algorithm selection only.
    #[map(rename = "fips-140-2-compliant")]
    #[schema(label = "FIPS 140-2 algorithms")]
    Fips1402Compliant,
    /// AWS-LC's FIPS module; needs a build with it.
    #[map(rename = "fips-140-3-validated")]
    #[schema(label = "FIPS 140-3 validated module")]
    Fips1403Validated,
    /// NSA's CNSA 1.0.
    #[map(rename = "cnsa-1.0")]
    #[schema(label = "CNSA 1.0")]
    Cnsa1,
    /// NSA's CNSA 2.0; TLS 1.3 and the aws-lc provider.
    #[map(rename = "cnsa-2.0")]
    #[schema(label = "CNSA 2.0")]
    Cnsa2,
}

/// The crypto provider.
#[derive(Debug, Clone, Copy, ToValue, FromValue, Schema)]
pub(crate) enum TlsCryptoForm {
    /// rustls's ring provider.
    #[map(rename = "ring")]
    #[schema(label = "ring")]
    Ring,
    /// rustls's aws-lc-rs provider; needs a build with it.
    #[map(rename = "aws-lc")]
    #[schema(label = "aws-lc")]
    AwsLc,
}

/// The bounds of the queue of peers waiting to be accepted.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct TlsWaitingForm {
    /// Entries one source may hold.
    #[schema(title = "Entries per source")]
    per_source: Option<u64>,
    /// Entries the whole queue may hold.
    #[schema(title = "Entries in all")]
    total: Option<u64>,
    /// Seconds before the same peer from the same source is recorded again.
    #[schema(title = "Record the same peer again after", format = "seconds")]
    rerecord: Option<f64>,
}

/// The Station TLS policy: the keys of the whole `tls` file, each over the intermediate preset.
/// The client access list (`acl`) is kept as stored and edited as configuration.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct TlsForm {
    /// TLS versions offered and accepted. Default: 1.2 and 1.3.
    #[schema(title = "TLS versions")]
    tls_versions: Option<Vec<TlsVersionForm>>,
    /// An allow-list of cipher suites by IANA name; every suite the profiles give when empty.
    #[schema(title = "Cipher suites")]
    cipher_suites: Option<Vec<String>>,
    /// An allow-list of key-exchange groups by name; every group the profiles give when empty.
    #[schema(title = "Key-exchange groups")]
    kx_groups: Option<Vec<String>>,
    /// The algorithm profiles allowed. Default: Mozilla intermediate.
    #[schema(title = "Algorithm profiles")]
    cipher_profiles: Option<Vec<TlsProfileForm>>,
    /// The crypto provider. Default: ring.
    #[schema(title = "Crypto provider")]
    crypto: Option<TlsCryptoForm>,
    /// Digests a certificate pin may use. Default: SHA-256, SHA-384 and SHA-512.
    #[schema(title = "Pin digests")]
    pin_digests: Option<Vec<TlsDigestForm>>,
    /// Whether a pinned certificate must be inside its validity dates. Default: yes.
    #[schema(title = "Check validity dates")]
    check_validity: Option<bool>,
    /// Whether a pinned peer may present a chain. Default: no.
    #[schema(title = "Allow a chain from a pinned peer")]
    allow_chain_when_pinned: Option<bool>,
    /// Whether TLS runs may be resumed, which skips the certificate checks for a peer that
    /// held a run. Default: no.
    #[schema(title = "Allow run resumption")]
    allow_resumption: Option<bool>,
    /// Whether a TLS 1.2 connection must use the extended master secret. Default: yes.
    #[schema(title = "Require the extended master secret")]
    require_ems: Option<bool>,
    /// The bounds of the waiting queue.
    #[schema(title = "Waiting queue", advanced)]
    waiting: Option<TlsWaitingForm>,
}

/// What a Cable grant lets an Agent do.
#[derive(Debug, Clone, Copy, Default, ToValue, FromValue, Schema)]
pub(crate) enum CableActionForm {
    /// Send a Cable to the scope.
    #[default]
    #[map(rename = "send")]
    #[schema(label = "Send")]
    Send,
    /// Receive a Cable addressed to the scope.
    #[map(rename = "receive")]
    #[schema(label = "Receive")]
    Receive,
    /// Discover peers in the scope.
    #[map(rename = "discover")]
    #[schema(label = "Discover")]
    Discover,
    /// List and release held Cables addressed to the scope.
    #[map(rename = "release")]
    #[schema(label = "Release held Cables")]
    Release,
}

/// The audience a Cable grant names.
#[derive(Debug, Clone, Copy, Default, ToValue, FromValue, Schema)]
pub(crate) enum CableScopeForm {
    /// One named Agent.
    #[default]
    #[map(rename = "agent")]
    #[schema(label = "An Agent")]
    Agent,
    /// Every eligible Agent at one Station.
    #[map(rename = "station")]
    #[schema(label = "A Station")]
    Station,
    /// Every eligible Agent in one Theater.
    #[map(rename = "theater")]
    #[schema(label = "A Theater")]
    Theater,
    /// Every eligible Agent the relay reaches.
    #[map(rename = "global")]
    #[schema(label = "Everyone")]
    Global,
    /// The subscribers of one channel.
    #[map(rename = "channel")]
    #[schema(label = "A channel")]
    Channel,
    /// An Operation and every Operation under it.
    #[map(rename = "operation")]
    #[schema(label = "An Operation")]
    Operation,
    /// What one compartment covers; only for releasing held Cables.
    #[map(rename = "compartment")]
    #[schema(label = "A compartment")]
    Compartment,
}

/// One explicit grant of the Cable relay.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct CableGrantForm {
    /// The Agent the grant is for; `*` is every Agent.
    #[schema(title = "Agent")]
    agent: String,
    /// What it lets the Agent do.
    #[schema(title = "May")]
    action: CableActionForm,
    /// The kind of audience.
    #[schema(title = "Audience")]
    scope: CableScopeForm,
    /// Which one, for an audience that has a name: the Station, Theater, channel, Operation or
    /// compartment.
    #[schema(title = "Named")]
    name: Option<String>,
}

/// What a receiving Agent's relay does with a Cable addressed to it.
#[derive(Debug, Clone, Copy, ToValue, FromValue, Schema)]
pub(crate) enum CableInboundForm {
    /// Place it in the inbox.
    #[map(rename = "deliver")]
    #[schema(label = "Deliver")]
    Deliver,
    /// Hold it for an authorized release.
    #[map(rename = "hold")]
    #[schema(label = "Hold")]
    Hold,
    /// Refuse it without revealing the recipient's policy.
    #[map(rename = "refuse")]
    #[schema(label = "Refuse")]
    Refuse,
}

/// What it takes for a Cable to reach an Agent of another Operation.
#[derive(Debug, Clone, Copy, ToValue, FromValue, Schema)]
pub(crate) enum CrossOperationForm {
    /// A compartment giving the sender `cable.send` there, as well as a grant.
    #[map(rename = "compartments")]
    #[schema(label = "A compartment as well as a grant")]
    Compartments,
    /// A grant alone, so a Station grant crosses every Operation. A reported weakening.
    #[map(rename = "station_grants")]
    #[schema(label = "A grant alone: a reported weakening")]
    StationGrants,
}

/// The Cable relay: the keys of the whole `cables` file.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct CablesForm {
    /// Explicit grants; nothing is allowed without one.
    #[schema(title = "Grants")]
    #[map(default)]
    #[form(form = CableGrantForm)]
    grants: Vec<CableGrantForm>,
    /// What each receiving Agent's relay does with a Cable addressed to it, by Agent; deliver
    /// when absent.
    #[schema(title = "Inbound handling")]
    #[map(default)]
    inbound: BTreeMap<String, CableInboundForm>,
    /// The members of each custom channel.
    #[schema(title = "Channels")]
    #[map(default)]
    channels: BTreeMap<String, Vec<String>>,
    /// Recipients one Cable may reach.
    #[schema(title = "Most recipients of a Cable")]
    max_fanout: Option<u64>,
    /// Cables a sender may send each second, over time.
    #[schema(title = "Cables per second from a sender")]
    sender_rate: Option<f64>,
    /// Cables a sender may send at once.
    #[schema(title = "Burst from a sender")]
    sender_burst: Option<u32>,
    /// Cables an audience may be sent each second, over time.
    #[schema(title = "Cables per second to an audience")]
    scope_rate: Option<f64>,
    /// Cables an audience may be sent at once.
    #[schema(title = "Burst to an audience")]
    scope_burst: Option<u32>,
    /// The longest body of a Cable.
    #[schema(title = "Longest body", format = "bytes")]
    body_bytes: Option<crate::bounds::Limit>,
    /// The longest lifetime a sender may ask for.
    #[schema(title = "Longest lifetime", format = "seconds")]
    max_ttl: Option<crate::bounds::TimeLimit>,
    /// The lifetime of a Cable that asks for none.
    #[schema(title = "Lifetime when none is asked", format = "seconds")]
    default_ttl: Option<crate::bounds::Seconds>,
    /// What it takes for a Cable to cross Operations.
    #[schema(title = "Crossing Operations")]
    cross_operation: Option<CrossOperationForm>,
}

/// The words a Mission's progress is written in on a Source instance.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct SourceWordsForm {
    /// While an Agent works.
    #[schema(title = "While working")]
    working: Option<String>,
    /// While the Mission waits for an answer on the issue.
    #[schema(title = "While waiting for an answer")]
    waiting: Option<String>,
    /// While the Mission is paused.
    #[schema(title = "While paused")]
    paused: Option<String>,
    /// When the Mission ended successfully.
    #[schema(title = "When done")]
    done: Option<String>,
    /// When the Mission ended without completing.
    #[schema(title = "When failed")]
    failed: Option<String>,
}

/// What reopens an item whose Mission settled.
#[derive(Debug, Clone, Copy, Default, ToValue, FromValue, Schema)]
pub(crate) enum SourceReopenForm {
    /// A label added after the Mission settled.
    #[default]
    #[map(rename = "label")]
    #[schema(label = "A label added again")]
    Label,
    /// A label, or a comment AI6 did not write. A reported weakening.
    #[map(rename = "comment")]
    #[schema(label = "A label or a comment")]
    Comment,
    /// Nothing: a settled item stays settled.
    #[map(rename = "never")]
    #[schema(label = "Never")]
    Never,
}

/// One Source instance of an Operation, `[operation.<id>.sources.<name>]`: the keys the Station
/// reads, then the chosen provider's own settings beside them. A bare string under `sources` is
/// shorthand for the provider's `target`.
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct SourceInstanceForm {
    /// The Source provider, from those the Stations report; the instance's name when empty.
    #[schema(title = "Provider", format = "provider:ai6_source")]
    provider: Option<String>,
    /// Whether items are polled in; false keeps the instance for posting only.
    #[schema(title = "Take items in")]
    intake: Option<bool>,
    /// Seconds between polls, at least the Station's shortest interval.
    #[schema(title = "Poll every", format = "seconds")]
    interval_secs: Option<u64>,
    /// The labels a Mission's milestones write.
    #[schema(title = "Status labels", advanced)]
    #[map(default)]
    status_labels: SourceWordsForm,
    /// The workflow transitions a Mission's milestones make.
    #[schema(title = "Status transitions", advanced)]
    #[map(default)]
    status_transitions: SourceWordsForm,
    /// The only labels an Agent may add or remove on a linked issue.
    #[schema(title = "Labels Agents may touch", advanced)]
    #[map(default)]
    agent_labels: Vec<String>,
    /// The workflow transitions an Agent may make without green ink.
    #[schema(title = "Transitions Agents may make", advanced)]
    #[map(default)]
    agent_transitions: Vec<String>,
    /// The priority a mirrored issue takes here, by the priority it has where it comes from.
    #[schema(title = "Priority map", advanced)]
    #[map(default)]
    priority_map: BTreeMap<String, String>,
    /// What reopens an item whose Mission settled.
    #[schema(title = "Reopen a settled item on", advanced)]
    reopen: Option<SourceReopenForm>,
    /// The chosen provider's settings, written beside the provider's name.
    #[schema(title = "Provider settings", format = "flat-settings-of:provider")]
    #[map(with = guatiao_serde::adapt::object, default)]
    settings: serde_json::Map<String, serde_json::Value>,
}

/// `#[map(with)]` for an Operation's `sources`: a table of any shape on the way in and out, drawn
/// as a table of [`SourceInstanceForm`]s.
pub(crate) mod source_instances {
    pub use guatiao_serde::adapt::{from_value, to_value_in};

    use super::SourceInstanceForm;

    /// A table of Source instances.
    pub fn kind(alloc: guatiao::Alloc) -> guatiao::schema::KindBuilder {
        use guatiao::Schema;
        guatiao::schema::KindBuilder::map_of_in(alloc, SourceInstanceForm::kind(alloc))
    }
}
