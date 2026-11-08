//! Where a Station's configuration comes from: its configuration provider.
//!
//! A [`ConfigProvider`] holds records of each family ([`ConfigRecord`]) at a scope
//! ([`ConfigScope`]): global, or one Station's. A Station's view of a family is its own
//! records over the global ones, key by key; a singleton family's one record replaces the
//! global one whole. The provider hands out that view checked and built
//! ([`ConfigSnapshot`]: the roster and the [`Settings`]) at a [`ConfigRevision`], and says
//! cheaply whether it has changed since.
//!
//! [`FileProvider`] reads a directory of family files (`operations.toml`, `agents.yaml`,
//! `stations/<id>/theaters.json`, ...), read-only; [`MemoryProvider`] holds a roster built in
//! code; with the `sqlite` or `postgres` feature, `SqlProvider` keeps the records in a
//! database every Station shares and takes changes. Any other store builds its roster with
//! [`Roster::from_parts`].
//!
//! A provider that takes changes ([`ConfigProvider::put`], [`ConfigProvider::remove`]) checks
//! every view a change touches before keeping it; a Station polls its provider and applies a
//! new revision of the families that apply live without a restart.

mod file;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod sql;

pub use file::FileProvider;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use sql::SqlProvider;

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Deserialize as _;
use sha2::{Digest, Sha256};

use crate::bounds::Limit;
use crate::directorates::m::access::{AccessPolicy, OperationAccess, compile};
use crate::directorates::m::compartments::Compartments;
use crate::directorates::m::cover::{Covers, cover_from};
use crate::directorates::m::records;
use crate::directorates::m::roster::{
    Roster, RosterDefinitions, RosterError, RosterErrorKind, error, error_of,
    roster_from_definitions,
};
use crate::directorates::m::theaters::Theaters;
use crate::directorates::q::LoadoutCatalog;
use crate::directorates::registry::bona_fides::PinSet;
use crate::directorates::registry::cables::CableConfig;
use crate::station::limits;

/// The families of record a configuration provider holds.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigRecord {
    /// An Operation: the keys of an `[operation.<id>]` table.
    Operation,
    /// An Agent: the keys of an `[agent.<codename>]` table.
    Agent,
    /// An Agent role: the keys of a `[role.<id>]` table.
    Role,
    /// Moneypenny's settings ([`AssistantSettings`](crate::AssistantSettings)): the keys of
    /// the `[assistant]` table; a singleton.
    Assistant,
    /// A cover policy: the keys of a `[cover.<name>]` table.
    Cover,
    /// A Theater: the keys of a `[theater.<id>]` table.
    Theater,
    /// An Agent's loadout: the keys of a `[loadout.<agent>]` table.
    Loadout,
    /// A sandbox profile loadouts name: the keys of a `[sandbox.<name>]` table.
    Sandbox,
    /// An MCP server loadouts name: the keys of an `[mcp.<name>]` table.
    Mcp,
    /// An overlay loadouts and Operations issue, its source and pin: the keys of an
    /// `[overlay.<name>]` table.
    Overlay,
    /// How a loadout secret reaches a Session, never its value: a `[secret.<name>]` table.
    Secret,
    /// A configured secret provider, its settings shared by the secrets that name it: a
    /// `[secret_provider.<name>]` table.
    SecretProvider,
    /// An account pool and its accounts (references to secrets, never values): a
    /// `[pool.<id>]` table.
    Pool,
    /// The Cable relay's grants, channels and limits: the whole `cables` file; a singleton.
    Cables,
    /// One accepted provider library: a `[[library]]` entry of the `plugins` file, keyed by
    /// its `file`.
    Plugin,
    /// The Station's runtime settings ([`StationRuntime`]): the whole `station` file; a
    /// singleton.
    Station,
    /// The Station TLS policy (a coqui-peer-tls `Policy` document): the whole `tls` file; a
    /// singleton.
    Tls,
    /// The global access policy ([`AccessPolicy`]): `[role.<name>]`, `[[rule]]`, `chief` and
    /// `delegable` of the whole `access`
    /// file; a singleton.
    Access,
    /// An Operation's own access policy: the `[role.<name>]` tables and `[[rule]]` entries of an
    /// [`OperationAccess`], keyed by Operation ID, in the `[operation_access.<id>]` tables of the
    /// `operation_access` file; global only.
    OperationAccess,
    /// Universal Exports' sign-in, limits and assistant ([`UnexRecord`]): the whole `unex`
    /// file; a singleton.
    Unex,
    /// Settings of the whole roster ([`RosterSettings`](crate::RosterSettings)), such as how
    /// deep the tree of Operations may be: the keys of the `[roster]` table of the
    /// `operations` file; a singleton.
    Roster,
    /// A Compartment ([`Compartment`](crate::Compartment)): the keys of a `[compartment.<id>]`
    /// table; global only.
    Compartment,
    /// A review policy ([`ReviewPolicy`](crate::ReviewPolicy)): the keys of a `[review.<name>]`
    /// table; global only.
    Review,
    /// A watch ([`WatchDefinition`](crate::WatchDefinition)): the keys of a `[watch.<scope>]`
    /// table, keyed by its scope (`m`, `operation:<codename>`, `station:<id>`); global only.
    Watch,
}

impl ConfigRecord {
    /// Every family, in the order a snapshot is built.
    pub const ALL: [Self; 24] = [
        Self::Roster,
        Self::Operation,
        Self::Agent,
        Self::Role,
        Self::Assistant,
        Self::Cover,
        Self::Theater,
        Self::Loadout,
        Self::Sandbox,
        Self::Mcp,
        Self::Overlay,
        Self::SecretProvider,
        Self::Secret,
        Self::Pool,
        Self::Cables,
        Self::Plugin,
        Self::Station,
        Self::Tls,
        Self::Access,
        Self::OperationAccess,
        Self::Unex,
        Self::Compartment,
        Self::Review,
        Self::Watch,
    ];

    /// The key of a singleton family's one record.
    pub const SINGLETON_KEY: &'static str = "settings";

    /// The family's name: `operation`, `agent`, `role`, `assistant`, `cover`, `theater`,
    /// `loadout`, `sandbox`, `mcp`, `overlay`, `secret`, `secret_provider`, `pool`, `cables`,
    /// `plugin`, `station`,
    /// `tls`, `access`, `unex`, `roster`, `compartment`, `review` or `watch`. For a family that shares its
    /// file, it is also its table there.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Operation => "operation",
            Self::Agent => "agent",
            Self::Role => "role",
            Self::Assistant => "assistant",
            Self::Cover => "cover",
            Self::Theater => "theater",
            Self::Loadout => "loadout",
            Self::Sandbox => "sandbox",
            Self::Mcp => "mcp",
            Self::Overlay => "overlay",
            Self::Secret => "secret",
            Self::SecretProvider => "secret_provider",
            Self::Pool => "pool",
            Self::Cables => "cables",
            Self::Plugin => "plugin",
            Self::Station => "station",
            Self::Tls => "tls",
            Self::Access => "access",
            Self::OperationAccess => "operation_access",
            Self::Unex => "unex",
            Self::Roster => "roster",
            Self::Compartment => "compartment",
            Self::Review => "review",
            Self::Watch => "watch",
        }
    }

    /// The family whose [`name`](Self::name) is `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|record| record.name() == name)
    }

    /// The family file that holds it, without its extension.
    pub const fn file(self) -> &'static str {
        match self {
            Self::Operation | Self::Roster => "operations",
            Self::Agent | Self::Role | Self::Assistant => "agents",
            Self::Cover => "covers",
            Self::Theater => "theaters",
            Self::Loadout
            | Self::Sandbox
            | Self::Mcp
            | Self::Overlay
            | Self::Secret
            | Self::SecretProvider
            | Self::Pool => "loadouts",
            Self::Cables => "cables",
            Self::Plugin => "plugins",
            Self::Station => "station",
            Self::Tls => "tls",
            Self::Access => "access",
            Self::OperationAccess => "operation_access",
            Self::Unex => "unex",
            Self::Compartment => "compartments",
            Self::Review => "reviews",
            Self::Watch => "watches",
        }
    }

    /// Whether the family has one record, keyed [`SINGLETON_KEY`](Self::SINGLETON_KEY).
    pub const fn is_singleton(self) -> bool {
        matches!(
            self,
            Self::Assistant
                | Self::Cables
                | Self::Station
                | Self::Tls
                | Self::Access
                | Self::Unex
                | Self::Roster
        )
    }

    /// Whether the record is its whole family file rather than a table in it.
    pub const fn is_whole_file(self) -> bool {
        self.is_singleton() && !matches!(self, Self::Assistant | Self::Roster)
    }

    /// Whether a Station may have records of its own in this family.
    pub const fn station_scoped(self) -> bool {
        matches!(
            self,
            Self::Theater
                | Self::Loadout
                | Self::Sandbox
                | Self::Mcp
                | Self::Overlay
                | Self::Secret
                | Self::SecretProvider
                | Self::Pool
                | Self::Cables
                | Self::Plugin
                | Self::Station
                | Self::Tls
        )
    }

    /// Whether a running Station, or Universal Exports for `access` and `operation_access`, applies a change
    /// without a restart.
    pub const fn applies_live(self) -> bool {
        !matches!(
            self,
            Self::Theater | Self::Cables | Self::Plugin | Self::Station | Self::Tls | Self::Unex
        )
    }
}

impl fmt::Display for ConfigRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Where a record applies: to every Station, or to one.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigScope {
    /// Every Station, unless it has a record of its own with the same key.
    #[default]
    Global,
    /// One Station, by ID.
    Station(String),
}

impl ConfigScope {
    /// The text form a provider stores: `""` for [`Global`](Self::Global), `station:<id>`.
    pub fn as_text(&self) -> String {
        match self {
            Self::Global => String::new(),
            Self::Station(id) => format!("station:{id}"),
        }
    }

    /// The scope of a text form: `""` or `station:<id>`; `None` for anything else, and for an
    /// ID that is not one plain path component ([`ConfigScope::is_plain`]).
    pub fn parse(text: &str) -> Option<Self> {
        if text.is_empty() {
            return Some(Self::Global);
        }
        text.strip_prefix("station:")
            .filter(|id| Self::is_plain(id))
            .map(|id| Self::Station(id.to_owned()))
    }

    /// Whether `id` can name a Station's directory of family files: not empty, not `.` or
    /// `..`, and no `/`, `\` or `:`.
    pub fn is_plain(id: &str) -> bool {
        !id.is_empty() && id != "." && id != ".." && !id.contains(['/', '\\', ':'])
    }

    /// The scope a Station's own records have; [`Global`](Self::Global) for `""`.
    pub fn of_station(station: &str) -> Self {
        if station.is_empty() {
            Self::Global
        } else {
            Self::Station(station.to_owned())
        }
    }
}

impl fmt::Display for ConfigScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global => formatter.write_str("global"),
            Self::Station(id) => write!(formatter, "station:{id}"),
        }
    }
}

/// The records of one family at one scope as a provider holds them, at one revision.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigDefinitions {
    /// The revision they were read at.
    pub revision: ConfigRevision,
    /// Each record's key and definition: a JSON object with the keys of its table in its
    /// family file, ordered by key.
    pub records: Vec<(String, serde_json::Value)>,
    /// Whether the provider takes [`put`](ConfigProvider::put) and
    /// [`remove`](ConfigProvider::remove).
    pub writable: bool,
}

impl ConfigDefinitions {
    /// `records`, read at `revision`, for a provider built outside this crate.
    pub fn new(
        revision: ConfigRevision,
        records: Vec<(String, serde_json::Value)>,
        writable: bool,
    ) -> Self {
        Self {
            revision,
            records,
            writable,
        }
    }
}

/// The configuration a Station runs, as it reports it at check-in (key `config`).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfigStatus {
    /// The [`ConfigRevision`] it runs, in decimal: a digest can pass the 2^53 a JSON number
    /// holds exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Why it refused the newest configuration, while it keeps running the last good one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<String>,
    /// Families changed since it started that only a restart applies, such as `theater`, or
    /// files a Station still reads beside its provider, such as `cables.toml`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs_restart: Vec<String>,
}

/// The record changed since it was read.
fn conflict(record: ConfigRecord, key: &str) -> RosterError {
    error_of(
        RosterErrorKind::Conflict,
        format!("the {record} {key:?} changed since it was read"),
    )
}

fn read_only() -> RosterError {
    error_of(
        RosterErrorKind::ReadOnly,
        "this configuration provider does not take changes",
    )
}

/// Which state of a [`ConfigProvider`] a [`ConfigSnapshot`] is: opaque, compared only for
/// equality, and chosen by the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigRevision(u64);

impl ConfigRevision {
    /// A revision with this number.
    pub const fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// How much the configuration provider reads of a document: `[provider]` in the bootstrap
/// file. Each default is the value the provider applies when the file says nothing; `null`
/// (or `"none"` in TOML) takes the bound away.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, guatiao::ToValue)]
pub struct ConfigLimits {
    /// Bytes of the loadout document (`loadout`, `sandbox`, `mcp`, `overlay`, `secret`,
    /// `secret_provider` and `pool` records together).
    pub loadout_bytes: Limit,
    /// Characters of one text value in a loadout document.
    pub loadout_value_chars: Limit,
    /// Bytes of the access policy (the `access` record).
    pub access_policy_bytes: Limit,
    /// How long a directory provider's replacement of a family file waits for a reader that has
    /// the file open.
    pub replace_retry: crate::bounds::TimeLimit,
}

impl Default for ConfigLimits {
    fn default() -> Self {
        Self {
            loadout_bytes: Limit::at(1024 * 1024),
            loadout_value_chars: Limit::at(4096),
            access_policy_bytes: Limit::at(1024 * 1024),
            replace_retry: crate::bounds::TimeLimit::secs(2),
        }
    }
}

impl ConfigLimits {
    /// One line per limit looser than its default, for the bootstrap file's weakenings.
    pub fn weakenings(&self) -> Vec<String> {
        let (Some(mut current), Some(default)) =
            (records::to_json(self), records::to_json(&Self::default()))
        else {
            return Vec::new();
        };
        // It paces a write and bounds nothing a person sends.
        if let Some(table) = current.as_object_mut() {
            table.remove("replace_retry");
        }
        crate::bounds::looser("provider", &current, &default, &[])
    }
}

/// What a snapshot holds besides the roster, resolved for one Station.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// The Theaters the Station declares (`theater` records).
    pub theaters: Theaters,
    /// Loadouts, sandbox profiles, MCP servers, secret references and account pools
    /// (`loadout`, `sandbox`, `mcp`, `secret` and `pool` records).
    pub loadouts: LoadoutCatalog,
    /// The Cable relay's settings (the `cables` record); the defaults deny every action.
    pub cables: CableConfig,
    /// The accepted provider libraries (`plugin` records); `None` when there are none.
    pub pins: Option<PinSet>,
    /// The Station's runtime settings (the `station` record).
    pub station: StationRuntime,
    /// The Station TLS policy document (the `tls` record); `None` is the library's default.
    pub tls: Option<serde_json::Value>,
    /// Universal Exports' role policy document (the `access` record); `None` denies every
    /// action.
    pub access: Option<serde_json::Value>,
    /// Each Operation's own access policy (the `operation_access` records), by Operation ID.
    pub operation_access: BTreeMap<String, OperationAccess>,
    /// Universal Exports' sign-in, limits and assistant (the `unex` record).
    pub unex: UnexRecord,
    /// Need-to-know access across Operations (the `compartment` records).
    pub compartments: Compartments,
    /// The limits the provider read these settings within.
    pub limits: ConfigLimits,
}

impl Settings {
    /// The global access policy, the `access` record; `None` when there is none, which is not
    /// the same as an empty one (a first start seeds the built-in roles only when there is no
    /// record).
    ///
    /// # Errors
    ///
    /// The policy's error for a document it does not accept.
    pub fn access_policy(&self) -> Result<Option<AccessPolicy>, String> {
        self.access
            .as_ref()
            .map(|document| {
                let text = json_to_toml(document.clone())
                    .and_then(|value| toml::to_string(&value).map_err(|error| error.to_string()))?;
                let policy = AccessPolicy::from_toml(&text, self.limits.access_policy_bytes)
                    .map_err(|error| error.to_string())?;
                compile(&policy, &[]).map_err(|error| error.to_string())?;
                Ok(policy)
            })
            .transpose()
            .map_err(|reason: String| format!("access: {reason}"))
    }

    /// The Station TLS policy: the `tls` record read over the library's default policy
    /// (`profile` picks a preset).
    ///
    /// # Errors
    ///
    /// The policy's own error for a document it does not accept.
    #[cfg(feature = "station-tls")]
    pub fn tls_policy(&self) -> Result<coqui_peer_tls::Policy, String> {
        match &self.tls {
            Some(document) => coqui_peer_tls::Policy::deserialize(document)
                .map_err(|reason| format!("tls: {reason}")),
            None => Ok(coqui_peer_tls::Policy::default()),
        }
    }
}

/// A Station's runtime settings, the `station` record: each key a Station applies at start
/// in place of its built-in default.
#[non_exhaustive]
#[derive(
    Debug, Clone, Default, PartialEq, guatiao::ToValue, guatiao::FromValue, guatiao::Schema,
)]
#[map(deny_unknown_fields)]
pub struct StationRuntime {
    /// Agent Sessions it runs at once. Default: 4.
    #[schema(title = "Sessions at once")]
    pub max_concurrent_missions: Option<usize>,
    /// Seconds without provider or terminal activity before a Session stalls. Default: 300.
    #[schema(title = "Stall a silent Session after", format = "seconds")]
    pub stall_timeout: Option<f64>,
    /// Seconds without novel terminal output before repeated output stalls a Session.
    /// Default: 300.
    #[schema(title = "Stall a repeating Session after", format = "seconds")]
    pub loop_timeout: Option<f64>,
    /// Repeated terminal lines required before loop detection applies. Default: 50.
    #[schema(title = "Repeated lines before a loop counts")]
    pub loop_min_lines: Option<usize>,
    /// Quiet seconds before a prompt-shaped terminal tail marks a Session blocked. Default: 10.
    #[schema(title = "Mark a Session blocked after", format = "seconds")]
    pub blocked_after: Option<f64>,
    /// Longest a Session runs, in seconds. Default: none.
    #[schema(title = "Longest a Session runs", format = "seconds")]
    pub max_runtime: Option<f64>,
    /// Distinct terminal lines kept for repeated-output detection. Default: 4096.
    #[schema(title = "Lines remembered for loop detection")]
    pub history: Option<usize>,
    /// Whether digit runs are normalized when comparing terminal lines for loops. Default: false.
    #[schema(title = "Ignore digits when comparing lines")]
    pub normalize_digits: Option<bool>,
    /// `control` (elects Headquarters) and `execution` (runs assigned Missions). Default: both.
    #[schema(title = "What this Station does")]
    pub roles: Option<Vec<String>>,
    /// Placement labels.
    #[schema(title = "Placement labels")]
    #[map(default)]
    pub labels: BTreeMap<String, String>,
    /// The terminal provider harnesses run in. Default: one that suits the handler.
    #[schema(title = "Terminal provider")]
    pub terminal: Option<String>,
    /// Seconds between samples of each remote Theater's resources; 0 leaves only the
    /// requirements probe. Default: 60.
    #[schema(title = "Sample a Theater's resources every", format = "seconds")]
    pub resources_every: Option<f64>,
    /// Directory names the Head of Station may remove from a finished Mission's safe house.
    /// Default: `target`, `node_modules` and `.dart_tool`.
    #[schema(title = "Rebuildable cache directories")]
    pub safe_house_caches: Option<Vec<String>>,
    /// Seconds a finished Mission's safe house is left alone before its caches may go.
    /// Default: 604800.
    #[schema(title = "Leave a finished safe house alone for", format = "seconds")]
    pub safe_house_cache_age: Option<f64>,
    /// `[scheduler]`: the scheduler and its check-ins.
    #[schema(title = "The scheduler")]
    #[map(default)]
    pub scheduler: limits::SchedulerLimits,
    /// `[publisher]`: publication to Universal Exports.
    #[schema(title = "Publishing to Universal Exports")]
    #[map(default)]
    pub publisher: limits::PublisherLimits,
    /// `[sources]`: Source polling, Sleepers and the tracker outbox.
    #[schema(title = "Sources and Sleepers")]
    #[map(default)]
    pub sources: limits::SourceLimits,
    /// `[transports]`: how often Theaters are looked over.
    #[schema(title = "Looking over Theaters")]
    #[map(default)]
    pub transports: limits::TransportLimits,
    /// `[recovery]`: re-attaching to Sessions after a restart.
    #[schema(title = "Re-attaching to Sessions")]
    #[map(default)]
    pub recovery: limits::RecoveryLimits,
    /// `[vetting]`: vetting briefs and Mission proposals.
    #[schema(title = "Vetting")]
    #[map(default)]
    pub vetting: limits::VettingLimits,
    /// `[review]`: reviews of Missions' plans and output.
    #[schema(title = "Reviews")]
    #[map(default)]
    pub review: limits::ReviewLimits,
    /// `[accounts]`: pool accounts.
    #[schema(title = "Pool accounts")]
    #[map(default)]
    pub accounts: limits::AccountLimits,
    /// `[approvals]`: green ink steps.
    #[schema(title = "Green ink steps")]
    #[map(default)]
    pub approvals: limits::ApprovalLimits,
    /// `[harness]`: a working directory's harness configuration.
    #[schema(title = "Harness configuration")]
    #[map(default)]
    pub harness: limits::HarnessLimits,
    /// `[overlays]`: reading the overlays loadouts and Operations issue.
    #[schema(title = "Overlays")]
    #[map(default)]
    pub overlays: limits::OverlayLimits,
    /// `[provision]`: probing and provisioning Theaters.
    #[schema(title = "Probing and provisioning Theaters")]
    #[map(default)]
    pub provision: limits::ProvisionLimits,
    /// `[secrets]`: loadout secret leases.
    #[schema(title = "Secret leases")]
    #[map(default)]
    pub secrets: limits::SecretLimits,
    /// `[terminal_output]`: decoding harness output.
    #[schema(title = "Harness output")]
    #[map(default)]
    pub terminal_output: limits::TerminalOutputLimits,
    /// `[green_ink]`: the harness gate.
    #[schema(title = "The harness gate")]
    #[map(default)]
    pub green_ink: limits::GreenInkLimits,
    /// `[detached]`: harnesses run detached over a transport.
    #[schema(title = "Detached harnesses")]
    #[map(default)]
    pub detached: limits::DetachedLimits,
    /// `[runner]`: field runners, the Station's client of them and `ai6 runner serve`.
    #[schema(title = "Field runners")]
    #[map(default)]
    pub runner: limits::RunnerLimits,
    /// `[api]`: the Station's own HTTP API.
    #[schema(title = "The Station's own API")]
    #[map(default)]
    pub api: limits::ApiLimits,
    /// `[audit]`: audit record details.
    #[schema(title = "Audit details")]
    #[map(default)]
    pub audit: limits::AuditLimits,
    /// `[theaters]`: ephemeral Theaters.
    #[schema(title = "Ephemeral Theaters")]
    #[map(default)]
    pub theaters: limits::TheaterLimits,
    /// `[theater_tools]`: the `theater_*` tools of remote-hands Sessions.
    #[schema(title = "Remote-hands tools")]
    #[map(default)]
    pub theater_tools: limits::TheaterToolLimits,
    /// `[head_of_station]`: the Head of Station's rounds.
    #[schema(title = "The Head of Station's rounds")]
    #[map(default)]
    pub head_of_station: limits::HeadOfStationLimits,
    /// `[probes]`: reading a Mission's deliverables back.
    #[schema(title = "Deliverable probes")]
    #[map(default)]
    pub probes: limits::ProbeLimits,
    /// Bytes of a terminal line the watchdog holds before it counts as a line. Default: 4096.
    #[schema(title = "Longest partial terminal line", format = "bytes")]
    pub partial_line_bytes: Option<Limit>,
    /// `[providers.<id>]`: provider `<id>`'s Guatiao configuration when the Station builds it with
    /// none of its own (handlers, terminals and safe houses).
    #[schema(title = "Provider configurations")]
    #[map(with = guatiao_serde::adapt::object, default)]
    pub providers: BTreeMap<String, serde_json::Value>,
}

impl StationRuntime {
    /// Checks every value: seconds finite and not negative, a capacity of at least 1, known
    /// roles, non-empty labels, and the tables as [`StationLimits::validate`] does.
    ///
    /// [`StationLimits::validate`]: crate::station::limits::StationLimits::validate
    ///
    /// # Errors
    ///
    /// Names the first key that is not acceptable.
    pub fn validate(&self) -> Result<(), String> {
        for (key, seconds) in [
            ("stall_timeout", self.stall_timeout),
            ("loop_timeout", self.loop_timeout),
            ("blocked_after", self.blocked_after),
            ("max_runtime", self.max_runtime),
            ("resources_every", self.resources_every),
            ("safe_house_cache_age", self.safe_house_cache_age),
        ] {
            if let Some(seconds) = seconds
                && std::time::Duration::try_from_secs_f64(seconds).is_err()
            {
                return Err(format!(
                    "station.{key}: seconds must be finite and not negative"
                ));
            }
        }
        if self.max_concurrent_missions == Some(0) {
            return Err("station.max_concurrent_missions: at least 1".to_owned());
        }
        if let Some(roles) = &self.roles
            && (roles.is_empty()
                || roles
                    .iter()
                    .any(|role| role != "control" && role != "execution"))
        {
            return Err("station.roles: control and/or execution".to_owned());
        }
        if self
            .labels
            .iter()
            .any(|(key, value)| key.trim().is_empty() || value.trim().is_empty())
        {
            return Err("station.labels: keys and values must be non-empty".to_owned());
        }
        self.limits().validate()
    }

    /// The tables, each over its defaults, and the provider configurations.
    pub fn limits(&self) -> limits::StationLimits {
        limits::StationLimits {
            scheduler: self.scheduler.clone(),
            publisher: self.publisher.clone(),
            sources: self.sources.clone(),
            transports: self.transports.clone(),
            recovery: self.recovery.clone(),
            vetting: self.vetting.clone(),
            review: self.review.clone(),
            accounts: self.accounts.clone(),
            approvals: self.approvals.clone(),
            harness: self.harness.clone(),
            overlays: self.overlays.clone(),
            provision: self.provision.clone(),
            secrets: self.secrets.clone(),
            terminal_output: self.terminal_output.clone(),
            green_ink: self.green_ink.clone(),
            detached: self.detached.clone(),
            runner: self.runner.clone(),
            api: self.api.clone(),
            audit: self.audit.clone(),
            theaters: self.theaters.clone(),
            theater_tools: self.theater_tools.clone(),
            head_of_station: self.head_of_station.clone(),
            probes: self.probes.clone(),
            providers: self.providers.clone(),
        }
    }

    /// A number of seconds as a duration; values were checked by [`validate`](Self::validate).
    pub fn duration(seconds: Option<f64>) -> Option<std::time::Duration> {
        seconds.and_then(|seconds| std::time::Duration::try_from_secs_f64(seconds).ok())
    }
}

/// A provider's configuration: a JSON object handed to its Guatiao provider as it is. A
/// credential inside one names a file, never a value.
pub type ProviderConfig =
    guatiao_serde::Serde<serde_json::Map<String, serde_json::Value>, guatiao::Map>;

/// Universal Exports' settings that are not host-local, the `unex` record.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, guatiao::ToValue, guatiao::FromValue, guatiao::Schema)]
#[map(deny_unknown_fields)]
pub struct UnexRecord {
    /// The Authenticator provider ID for user sign-in. Default: none; nobody signs in.
    #[schema(title = "Sign-in provider")]
    pub authenticator: Option<String>,
    /// The Authenticator's configuration. Default: none.
    #[schema(title = "Sign-in provider's configuration", sensitive)]
    pub authenticator_config: Option<ProviderConfig>,
    /// The Directory provider ID; required when user access is configured. Default: none.
    #[schema(title = "Directory provider")]
    pub directory: Option<String>,
    /// The Directory's configuration. Default: none.
    #[schema(title = "Directory provider's configuration", sensitive)]
    pub directory_config: Option<ProviderConfig>,
    /// A custom Authorizer provider ID, in place of the `access` record's role policy; the
    /// gateway's chief tier still applies beside it. Default: none.
    #[schema(title = "Custom authorizer")]
    pub authorizer: Option<String>,
    /// The custom Authorizer's configuration. Default: none.
    #[schema(title = "Custom authorizer's configuration", sensitive)]
    pub authorizer_config: Option<ProviderConfig>,
    /// The Accounts provider ID that keeps the accounts people sign in with, for the account
    /// routes. Default: the Authenticator when it offers accounts too.
    #[schema(title = "Accounts provider")]
    pub accounts: Option<String>,
    /// The Accounts provider's configuration. Default: none.
    #[schema(title = "Accounts provider's configuration", sensitive)]
    pub accounts_config: Option<ProviderConfig>,
    /// The sanitized header a trusted reverse proxy sets to the signed-in user. Default: none.
    #[schema(title = "Reverse proxy's user header")]
    pub proxy_user_header: Option<String>,
    /// The Directory attribute that resolves the reverse proxy's user.
    #[schema(title = "Directory attribute of the proxy's user")]
    #[map(default = UnexRecord::default_lookup_by())]
    pub proxy_lookup_by: String,
    /// The Authenticator provider ID that names the person asking for one of Moneypenny's
    /// pages from the request itself: `coqui_access_mtls` on the gateway's own TLS,
    /// `coqui_access_proxy` behind a front door. It answers only under the pages' base,
    /// never for `/v1`. Default: none; pages know a person by the page cookie alone.
    #[schema(title = "Page viewers' authenticator")]
    pub page_authenticator: Option<String>,
    /// The page authenticator's configuration. Default: none.
    #[schema(title = "Page viewers' authenticator's configuration", sensitive)]
    pub page_authenticator_config: Option<ProviderConfig>,
    /// Password or key tries a minute from one client address. Default: 5.
    #[schema(title = "Tries a minute per client address")]
    pub login_tries_per_address: Option<u32>,
    /// Tries in ten minutes for one account name, from anywhere. Default: 10.
    #[schema(title = "Tries in ten minutes per account name")]
    pub login_tries_per_name: Option<u32>,
    /// The header a trusted proxy sets to the client address. Default: none.
    #[schema(title = "Reverse proxy's client address header")]
    pub client_address_header: Option<String>,
    /// Seconds a found person or a granted action is kept; 0 keeps none. Default: 5.
    #[schema(title = "Keep a found person or a grant for", format = "seconds")]
    pub access_cache_ttl: Option<f64>,
    /// Seconds "no such person" or a denial is kept; 0 keeps none. Default: 1.
    #[schema(title = "Keep a miss or a denial for", format = "seconds")]
    pub access_cache_negative_ttl: Option<f64>,
    /// The most lookups, and decisions, kept; 0 keeps none. Default: 10000.
    #[schema(title = "Lookups and decisions kept")]
    pub access_cache_entries: Option<u64>,
    /// Whether Moneypenny is served.
    #[schema(title = "Serve Moneypenny")]
    #[map(default = UnexRecord::default_assistant())]
    pub assistant: bool,
    /// The roster Agent that is Moneypenny.
    #[schema(title = "The Agent that is Moneypenny")]
    #[map(default = UnexRecord::default_assistant_agent())]
    pub assistant_agent: String,
    /// The longest `text` filter an Outer Office route may carry, in bytes: in Moneypenny's
    /// links and cards, and in the page a message was sent from. 0 reads as 1. Default: 128.
    #[schema(title = "Longest route text filter", format = "bytes")]
    pub route_text_bytes: Option<u64>,
    /// How deeply nested a provider configuration may be.
    #[schema(title = "Deepest provider configuration")]
    #[map(default = UnexRecord::default_provider_config_depth())]
    pub provider_config_depth: crate::bounds::Limit,
    /// `[gateway]`: live Sessions, connections and frames.
    #[schema(title = "The gateway")]
    #[map(default)]
    pub gateway: super::unex_limits::UnexGatewayLimits,
    /// `[controls]`: the operator controls waiting for a Station.
    #[schema(title = "Operator controls")]
    #[map(default)]
    pub controls: super::unex_limits::ControlLimits,
    /// `[timeline]`: a Mission's timeline pages.
    #[schema(title = "Timeline pages")]
    #[map(default)]
    pub timeline: super::unex_limits::UnexTimelineLimits,
    /// `[login]`: how password and key tries are told apart and counted.
    #[schema(title = "Counting sign-in tries")]
    #[map(default)]
    pub login: super::unex_limits::LoginTable,
    /// `[text]`: the longest single request fields.
    #[schema(title = "Longest request fields")]
    #[map(default)]
    pub text: super::unex_limits::TextLimits,
}

impl Default for UnexRecord {
    fn default() -> Self {
        Self {
            authenticator: None,
            authenticator_config: None,
            directory: None,
            directory_config: None,
            authorizer: None,
            authorizer_config: None,
            accounts: None,
            accounts_config: None,
            proxy_user_header: None,
            proxy_lookup_by: Self::default_lookup_by(),
            page_authenticator: None,
            page_authenticator_config: None,
            login_tries_per_address: None,
            login_tries_per_name: None,
            client_address_header: None,
            access_cache_ttl: None,
            access_cache_negative_ttl: None,
            access_cache_entries: None,
            assistant: Self::default_assistant(),
            assistant_agent: Self::default_assistant_agent(),
            route_text_bytes: None,
            provider_config_depth: Self::default_provider_config_depth(),
            gateway: Default::default(),
            controls: Default::default(),
            timeline: Default::default(),
            login: Default::default(),
            text: Default::default(),
        }
    }
}

impl UnexRecord {
    /// Each value in a provider configuration whose key names a credential (`password`,
    /// `secret`, `token`, `key`) and that is written inline rather than as a file, worded for a
    /// log line.
    pub fn weakenings(&self) -> Vec<String> {
        fn inline(path: &str, value: &serde_json::Value, found: &mut Vec<String>) {
            if let serde_json::Value::Object(entries) = value {
                for (key, value) in entries {
                    let lower = key.to_ascii_lowercase();
                    let credential = [
                        "password",
                        "passphrase",
                        "secret",
                        "token",
                        "apikey",
                        "api_key",
                        "private_key",
                    ]
                    .iter()
                    .any(|word| lower.contains(word))
                        || lower == "key"
                        || lower.ends_with("_key");
                    let names_file = lower.ends_with("_file") || lower.ends_with("_path");
                    if credential && !names_file && value.is_string() {
                        found.push(format!(
                            "unex.{path}.{key} is a credential written inline in the shared \
                             configuration; name a file instead"
                        ));
                    }
                    inline(&format!("{path}.{key}"), value, found);
                }
            }
        }
        let mut found = Vec::new();
        for (name, config) in [
            ("authenticator_config", &self.authenticator_config),
            ("directory_config", &self.directory_config),
            ("authorizer_config", &self.authorizer_config),
            ("accounts_config", &self.accounts_config),
            ("page_authenticator_config", &self.page_authenticator_config),
        ] {
            if let Some(config) = config {
                inline(
                    name,
                    &serde_json::Value::Object(config.0.clone()),
                    &mut found,
                );
            }
        }
        found.extend(self.limits().weakenings());
        if self.provider_config_depth.get().is_none() {
            found.push(format!(
                "unex.provider_config_depth none is looser than the default {}",
                Self::default_provider_config_depth()
            ));
        } else if self.provider_config_depth.get() > Self::default_provider_config_depth().get() {
            found.push(format!(
                "unex.provider_config_depth {} is looser than the default {}",
                self.provider_config_depth,
                Self::default_provider_config_depth()
            ));
        }
        found
    }

    /// The five limit tables together.
    pub fn limits(&self) -> super::unex_limits::UnexLimits {
        super::unex_limits::UnexLimits {
            gateway: self.gateway.clone(),
            controls: self.controls.clone(),
            timeline: self.timeline.clone(),
            login: self.login.clone(),
            text: self.text.clone(),
        }
    }

    fn default_provider_config_depth() -> crate::bounds::Limit {
        crate::bounds::Limit::at(64)
    }

    fn default_lookup_by() -> String {
        "name".to_owned()
    }

    const fn default_assistant() -> bool {
        true
    }

    fn default_assistant_agent() -> String {
        "moneypenny".to_owned()
    }
}

/// The configuration of one Station at one [`ConfigRevision`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConfigSnapshot {
    /// Which state of the provider this is.
    pub revision: ConfigRevision,
    /// The checked roster.
    pub roster: Roster,
    /// Everything else.
    pub settings: Settings,
    /// A digest of each family's records in this view, to tell which families a later
    /// snapshot changed.
    fingerprints: BTreeMap<ConfigRecord, [u8; 32]>,
}

impl ConfigSnapshot {
    /// A snapshot of `roster` and `settings` at `revision`.
    pub fn new(revision: ConfigRevision, roster: Roster, settings: Settings) -> Self {
        Self {
            revision,
            roster,
            settings,
            fingerprints: BTreeMap::new(),
        }
    }

    /// The families whose records differ between this snapshot and `other`. Snapshots built
    /// in code ([`ConfigSnapshot::new`]) carry no records and differ in none.
    pub fn changed_families(&self, other: &Self) -> Vec<ConfigRecord> {
        ConfigRecord::ALL
            .into_iter()
            .filter(|record| self.fingerprints.get(record) != other.fingerprints.get(record))
            .collect()
    }
}

/// Where the Station's Operations, Agents, roles, covers, Moneypenny's settings and its other
/// configuration come from.
///
/// A provider hands out only configuration that passed the same checks as the family files
/// do: an invalid configuration is an error, never a snapshot. Calls are synchronous; the
/// daemon makes them off its async workers.
pub trait ConfigProvider: Send + Sync {
    /// The configuration of `station` (`""`: the global records only), with the revision it
    /// is at.
    ///
    /// # Errors
    ///
    /// Returns an error when the configuration cannot be read or is not acceptable.
    fn load(&self, station: &str) -> Result<ConfigSnapshot, RosterError>;

    /// The configuration of `station` again when the provider is no longer at revision
    /// `since`, else `None`.
    ///
    /// The default loads and compares revisions; a provider that can tell cheaply overrides
    /// it.
    ///
    /// # Errors
    ///
    /// As [`load`](ConfigProvider::load).
    fn changes_since(
        &self,
        station: &str,
        since: ConfigRevision,
    ) -> Result<Option<ConfigSnapshot>, RosterError> {
        let snapshot = self.load(station)?;
        Ok((snapshot.revision != since).then_some(snapshot))
    }

    /// The revision `station`'s configuration is at now, without building it.
    ///
    /// A Station compares it with the revision it runs and the last one it refused, so an
    /// unacceptable revision is checked and reported once. The default loads; a provider
    /// that can tell cheaply overrides it.
    ///
    /// # Errors
    ///
    /// As [`load`](ConfigProvider::load).
    fn revision(&self, station: &str) -> Result<ConfigRevision, RosterError> {
        self.load(station).map(|snapshot| snapshot.revision)
    }

    /// The records of one family at `scope` as the provider holds them, for an administrator
    /// to read and edit.
    ///
    /// # Errors
    ///
    /// [`RosterErrorKind::ReadOnly`] from a provider that cannot list them (the default);
    /// otherwise as [`load`](ConfigProvider::load).
    fn definitions(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
    ) -> Result<ConfigDefinitions, RosterError> {
        let _ = (record, scope);
        Err(read_only())
    }

    /// Stores one record's definition under `key` at `scope`, replacing what was there, and
    /// returns the revision the provider moved to. `definition` is a JSON object with the
    /// keys of the record's table in its family file.
    ///
    /// # Errors
    ///
    /// [`RosterErrorKind::ReadOnly`] from a provider that takes no changes (the default);
    /// [`RosterErrorKind::Invalid`], changing nothing, when a configuration it would leave is
    /// not acceptable or the family has no records at that scope;
    /// [`RosterErrorKind::Unavailable`] when the provider fails.
    fn put(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
        key: &str,
        definition: &serde_json::Value,
    ) -> Result<ConfigRevision, RosterError> {
        let _ = (record, scope, key, definition);
        Err(read_only())
    }

    /// Stores one record only when the stored one is `expected` (`None`: there is none), and
    /// returns the revision the provider moved to. A provider with transactions checks and
    /// stores in one; the default reads and then stores, so two writers can race.
    ///
    /// # Errors
    ///
    /// [`RosterErrorKind::Conflict`] when the stored record is not `expected`; otherwise as
    /// [`put`](ConfigProvider::put).
    fn put_if(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
        key: &str,
        expected: Option<&serde_json::Value>,
        definition: &serde_json::Value,
    ) -> Result<ConfigRevision, RosterError> {
        let stored = self
            .definitions(record, scope)?
            .records
            .into_iter()
            .find(|(stored, _)| stored == key)
            .map(|(_, definition)| definition);
        if stored.as_ref() != expected {
            return Err(conflict(record, key));
        }
        self.put(record, scope, key, definition)
    }

    /// Removes one record; `None` when there was none.
    ///
    /// # Errors
    ///
    /// As [`put`](ConfigProvider::put): a removal that leaves a configuration unacceptable
    /// (an Agent another still extends, say) is refused.
    fn remove(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
        key: &str,
    ) -> Result<Option<ConfigRevision>, RosterError> {
        let _ = (record, scope, key);
        Err(read_only())
    }

    /// [`remove`](ConfigProvider::remove) for someone who runs this host (`ai6 config rm`). The
    /// default is `remove`; a provider whose files people keep by hand takes no edit from
    /// Universal Exports, and takes this one.
    ///
    /// # Errors
    ///
    /// As [`remove`](ConfigProvider::remove).
    fn remove_on_host(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
        key: &str,
    ) -> Result<Option<ConfigRevision>, RosterError> {
        self.remove(record, scope, key)
    }

    /// Stores every record of `records` at `scope`, checked as one change: all of them are
    /// kept, or none. The default stores them one by one with [`put`](ConfigProvider::put),
    /// which a provider that has transactions overrides.
    ///
    /// # Errors
    ///
    /// As [`put`](ConfigProvider::put).
    fn put_all(
        &self,
        scope: &ConfigScope,
        records: &[(ConfigRecord, String, serde_json::Value)],
    ) -> Result<Option<ConfigRevision>, RosterError> {
        let mut revision = None;
        for (record, key, definition) in records {
            revision = Some(self.put(*record, scope, key, definition)?);
        }
        Ok(revision)
    }

    /// The scopes that hold records: [`ConfigScope::Global`] and each Station with records of
    /// its own. The default is the global scope alone.
    ///
    /// # Errors
    ///
    /// As [`load`](ConfigProvider::load).
    fn scopes(&self) -> Result<Vec<ConfigScope>, RosterError> {
        Ok(vec![ConfigScope::Global])
    }
}

/// One record that names another, as [`references_to`] finds it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigReference {
    /// The family of the record that names it.
    pub record: ConfigRecord,
    /// Where that record is held.
    pub scope: ConfigScope,
    /// That record's key.
    pub key: String,
    /// Where in it the name is, dotted; empty when its own key is the name.
    pub path: String,
}

impl fmt::Display for ConfigReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {} ({})", self.record, self.key, self.scope)?;
        if !self.path.is_empty() {
            write!(formatter, ": {}", self.path)?;
        }
        Ok(())
    }
}

/// Settings whose value names a program or a model, which a record of the same name is not.
const NOT_RECORD_NAMES: [&str; 4] = ["handler", "provider", "model", "effort"];

/// Every place in `value` that holds `key` as a whole string or as a table's key, but a field in
/// `skip`.
fn places_naming(
    value: &serde_json::Value,
    key: &str,
    path: &str,
    skip: &[&str],
    found: &mut Vec<String>,
) {
    match value {
        serde_json::Value::String(text) if text == key => {
            let field = path.rsplit('.').next().unwrap_or(path);
            if !skip.contains(&field) {
                found.push(path.to_owned());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                places_naming(item, key, path, skip, found);
            }
        }
        serde_json::Value::Object(table) => {
            for (field, inner) in table {
                let deeper = if path.is_empty() {
                    field.clone()
                } else {
                    format!("{path}.{field}")
                };
                if field == key {
                    found.push(deeper.clone());
                }
                places_naming(inner, key, &deeper, skip, found);
            }
        }
        _ => {}
    }
}

/// Every place in `value`, a record of `family`, that names the loadout secret `key`: an entry of
/// a `secrets` list (a loadout's, a route's), a `secret` value (a pool account's), and a pool account
/// of that name with no `secret` whose kind reads one, since it reads the secret of its own name.
/// Secrets are often named like settings (`path`, `token`), so nothing else counts.
fn secret_places(
    family: ConfigRecord,
    value: &serde_json::Value,
    key: &str,
    path: &str,
    found: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                secret_places(family, item, key, path, found);
            }
        }
        serde_json::Value::Object(table) => {
            for (field, inner) in table {
                let deeper = if path.is_empty() {
                    field.clone()
                } else {
                    format!("{path}.{field}")
                };
                let names = match (field.as_str(), inner) {
                    ("secrets", serde_json::Value::Array(names)) => {
                        names.iter().any(|name| name == key)
                    }
                    ("secret", serde_json::Value::String(name)) => name == key,
                    _ => false,
                };
                if names {
                    found.push(deeper.clone());
                }
                secret_places(family, inner, key, &deeper, found);
            }
        }
        _ => {}
    }
    if family == ConfigRecord::Pool
        && path.is_empty()
        && let Some(account) = value.get("account").and_then(|accounts| accounts.get(key))
        && account.get("secret").is_none()
        && account
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .and_then(crate::directorates::q::AccountKind::parse)
            .is_some_and(crate::directorates::q::AccountKind::reads_secret)
    {
        found.push(format!("account.{key}"));
    }
}

/// What still names the record `key` of `record` at `scope`, in every family at every scope.
///
/// A secret is named only where a record takes a secret's name (`secret_places`). For any
/// other family any whole string or table key equal to its name counts, since a new setting is
/// found with no list to update: a field that only happens to hold the same text is shown too,
/// and a person judges.
///
/// # Errors
///
/// [`RosterErrorKind::Invalid`] when there is no such record, and as
/// [`definitions`](ConfigProvider::definitions) when anything cannot be read: an answer from part
/// of the configuration is no answer.
pub fn references_to(
    provider: &dyn ConfigProvider,
    record: ConfigRecord,
    scope: &ConfigScope,
    key: &str,
) -> Result<Vec<ConfigReference>, RosterError> {
    // A record that is its family's only one has no name for another to use.
    let named = !record.is_singleton() && !record.is_whole_file();
    let mut exists = false;
    let mut references = Vec::new();
    for held in provider.scopes()? {
        for family in ConfigRecord::ALL {
            for (other, definition) in provider.definitions(family, &held)?.records {
                if family == record && held == *scope && other == key {
                    exists = true;
                    continue;
                }
                if !named {
                    continue;
                }
                let mut places = Vec::new();
                // A loadout belongs to the Agent its key names.
                if record == ConfigRecord::Agent && family == ConfigRecord::Loadout && other == key
                {
                    places.push(String::new());
                }
                if record == ConfigRecord::Secret {
                    secret_places(family, &definition, key, "", &mut places);
                } else {
                    // A secret, or an instance's credential, names an instance as its `provider`.
                    let skip: &[&str] = if record == ConfigRecord::SecretProvider {
                        &[]
                    } else {
                        &NOT_RECORD_NAMES
                    };
                    places_naming(&definition, key, "", skip, &mut places);
                }
                places.dedup();
                references.extend(places.into_iter().map(|path| ConfigReference {
                    record: family,
                    scope: held.clone(),
                    key: other.clone(),
                    path,
                }));
            }
        }
    }
    if !exists {
        return Err(error(format!("no {record} record {key:?} at {scope}")));
    }
    Ok(references)
}

/// The families in the order an import stores them, so each refers only to records already
/// stored: roles before the Agents that hold them, Agents before the Operations that name them.
pub const IMPORT_ORDER: [ConfigRecord; 24] = [
    ConfigRecord::Roster,
    ConfigRecord::Role,
    ConfigRecord::Cover,
    ConfigRecord::Assistant,
    ConfigRecord::Agent,
    ConfigRecord::Operation,
    ConfigRecord::Theater,
    ConfigRecord::Sandbox,
    ConfigRecord::Mcp,
    ConfigRecord::SecretProvider,
    ConfigRecord::Secret,
    ConfigRecord::Overlay,
    ConfigRecord::Pool,
    ConfigRecord::Loadout,
    ConfigRecord::Cables,
    ConfigRecord::Plugin,
    ConfigRecord::Station,
    ConfigRecord::Tls,
    ConfigRecord::Access,
    ConfigRecord::OperationAccess,
    ConfigRecord::Unex,
    ConfigRecord::Compartment,
    ConfigRecord::Review,
    ConfigRecord::Watch,
];

/// One family's records as the part of its family file they make: `[name.<key>]` tables, a
/// singleton's `[name]` table, a whole file, or `[[library]]` entries.
fn family_value(
    record: ConfigRecord,
    records: impl IntoIterator<Item = (String, serde_json::Value)>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut document = serde_json::Map::new();
    let records: Vec<_> = records.into_iter().collect();
    if records.is_empty() {
        return document;
    }
    if record.is_whole_file() {
        if let Some((_, serde_json::Value::Object(whole))) = records.into_iter().next() {
            return whole;
        }
        return document;
    }
    let value = if record == ConfigRecord::Plugin {
        serde_json::Value::Array(
            records
                .into_iter()
                .map(|(key, mut definition)| {
                    if let Some(entry) = definition.as_object_mut() {
                        entry.insert("file".to_owned(), serde_json::Value::String(key));
                    }
                    definition
                })
                .collect(),
        )
    } else if record.is_singleton() {
        records
            .into_iter()
            .next()
            .map(|(_, definition)| definition)
            .unwrap_or_default()
    } else {
        serde_json::Value::Object(records.into_iter().collect())
    };
    let name = if record == ConfigRecord::Plugin {
        "library"
    } else {
        record.name()
    };
    document.insert(name.to_owned(), value);
    document
}

/// `records` of `record` as the TOML text of their family file.
///
/// # Errors
///
/// A record TOML cannot hold (a `null`).
pub fn family_document(
    record: ConfigRecord,
    records: impl IntoIterator<Item = (String, serde_json::Value)>,
) -> Result<String, RosterError> {
    let value = json_to_toml(serde_json::Value::Object(family_value(record, records)))
        .map_err(|reason| error(format!("{record}: {reason}")))?;
    toml::to_string(&value).map_err(|reason| error(format!("{record}: {reason}")))
}

/// Writes `bytes` to `path`, readable by its owner only on Unix: an exported family may hold
/// a credential.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

/// Writes every family of every scope `provider` holds into `dir` as TOML family files, laid
/// out as a [`FileProvider`] reads them, and returns how many files it wrote. Files are
/// readable by their owner only on Unix.
///
/// # Errors
///
/// As [`ConfigProvider::definitions`]; `Unavailable` when a file cannot be written.
pub fn export(provider: &dyn ConfigProvider, dir: &Path) -> Result<usize, RosterError> {
    let mut written = 0;
    for scope in provider.scopes()? {
        let target = match &scope {
            ConfigScope::Global => dir.to_path_buf(),
            ConfigScope::Station(id) if ConfigScope::is_plain(id) => dir.join("stations").join(id),
            ConfigScope::Station(id) => {
                return Err(error(format!("station:{id} cannot name a directory")));
            }
        };
        let mut files: BTreeMap<&'static str, serde_json::Map<String, serde_json::Value>> =
            BTreeMap::new();
        for record in ConfigRecord::ALL {
            let records = provider.definitions(record, &scope)?.records;
            if records.is_empty() {
                continue;
            }
            files
                .entry(record.file())
                .or_default()
                .extend(family_value(record, records));
        }
        for (stem, document) in files {
            let value = json_to_toml(serde_json::Value::Object(document))
                .map_err(|reason| error(format!("{stem}: {reason}")))?;
            let text =
                toml::to_string(&value).map_err(|reason| error(format!("{stem}: {reason}")))?;
            let path = target.join(format!("{stem}.toml"));
            std::fs::create_dir_all(&target)
                .and_then(|()| write_private(&path, text.as_bytes()))
                .map_err(|failure| {
                    error_of(
                        RosterErrorKind::Unavailable,
                        format!("{}: {failure}", path.display()),
                    )
                })?;
            written += 1;
        }
    }
    Ok(written)
}

/// Opens the configuration provider at `location`: `sqlite:PATH` (or `sqlite://PATH`) or a
/// `postgres://` URL for a `SqlProvider`, anything else a [`FileProvider`] directory.
///
/// # Errors
///
/// `Unsupported` for a SQL location when this build lacks its cargo feature (`sqlite`,
/// `postgres`); the database's error when it cannot be opened. A directory is not read
/// until it is loaded.
pub fn open(location: &str) -> io::Result<Arc<dyn ConfigProvider>> {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    return open_with(
        location,
        &crate::directorates::registry::sql::SqlOptions::default(),
        ConfigLimits::default(),
    );
    #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
    if crate::bootstrap::is_sql_location(location) {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this build of ai6 has no SQL configuration support (cargo features `sqlite`, `postgres`)",
        ))
    } else {
        Ok(Arc::new(FileProvider::new(location)))
    }
}

/// As [`open`], connecting a SQL provider with `options` (password, timeouts) and reading
/// documents within `limits`.
///
/// # Errors
///
/// As [`open`].
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub fn open_with(
    location: &str,
    options: &crate::directorates::registry::sql::SqlOptions,
    limits: ConfigLimits,
) -> io::Result<Arc<dyn ConfigProvider>> {
    if crate::bootstrap::is_sql_location(location) {
        Ok(Arc::new(
            SqlProvider::open_with(location, options)?.with_limits(limits),
        ))
    } else {
        Ok(Arc::new(FileProvider::new(location).with_limits(limits)))
    }
}

/// Each family's records by key, at one scope or in one Station's view.
pub(crate) type Layer = BTreeMap<ConfigRecord, BTreeMap<String, serde_json::Value>>;

/// `station`'s records over `global`'s, key by key.
pub(crate) fn merged(global: &Layer, station: &Layer) -> Layer {
    let mut view = global.clone();
    for (record, records) in station {
        let family = view.entry(*record).or_default();
        for (key, definition) in records {
            family.insert(key.clone(), definition.clone());
        }
    }
    view
}

/// Refuses a record a family does not take at `scope`, or a singleton with another key.
pub(crate) fn check_placement(
    record: ConfigRecord,
    scope: &ConfigScope,
    key: &str,
    source: &dyn fmt::Display,
) -> Result<(), RosterError> {
    if *scope != ConfigScope::Global && !record.station_scoped() {
        return Err(error(format!(
            "{source}: {record} records are global; {scope} cannot have one"
        )));
    }
    if record.is_singleton() && key != ConfigRecord::SINGLETON_KEY {
        return Err(error(format!(
            "{source}: the {record} record is keyed {:?}, not {key:?}",
            ConfigRecord::SINGLETON_KEY
        )));
    }
    Ok(())
}

/// Builds and checks the configuration of one view: the roster, every Agent resolved, and the
/// settings. A relative path in the assistant settings is resolved against `base`.
pub(crate) fn assemble(
    revision: ConfigRevision,
    view: &Layer,
    base: Option<&Path>,
    source: &dyn fmt::Display,
    limits: &ConfigLimits,
) -> Result<ConfigSnapshot, RosterError> {
    let family = |record: ConfigRecord| -> Vec<(String, serde_json::Value)> {
        view.get(&record)
            .map(|records| {
                records
                    .iter()
                    .map(|(key, definition)| (key.clone(), definition.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut covers = Covers::default();
    for (name, definition) in family(ConfigRecord::Cover) {
        covers.insert(
            cover_from(name, &definition).map_err(|reason| error(format!("{source}: {reason}")))?,
        );
    }
    let assistant = family(ConfigRecord::Assistant)
        .into_iter()
        .next()
        .map(|(_, definition)| definition);
    let settings = family(ConfigRecord::Roster)
        .into_iter()
        .next()
        .map(|(_, definition)| definition);
    let roster = roster_from_definitions(
        RosterDefinitions {
            operations: family(ConfigRecord::Operation),
            agents: family(ConfigRecord::Agent),
            roles: family(ConfigRecord::Role),
            reviews: family(ConfigRecord::Review),
            watches: family(ConfigRecord::Watch),
            assistant,
            settings,
        },
        covers,
        base,
        source,
    )?;
    roster
        .check_agents()
        .map_err(|reason| error(format!("{source}: {reason}")))?;
    let theaters = Theaters::from_records(family(ConfigRecord::Theater))
        .map_err(|reason| error(format!("{source}: {reason}")))?;
    let tables = |records: &[ConfigRecord]| {
        let mut document = toml::Table::new();
        for record in records {
            let entries = family(*record);
            if entries.is_empty() {
                continue;
            }
            let table: serde_json::Map<String, serde_json::Value> = entries.into_iter().collect();
            document.insert(
                record.name().to_owned(),
                json_to_toml(serde_json::Value::Object(table))
                    .map_err(|reason| error(format!("{source}: {record}: {reason}")))?,
            );
        }
        Ok::<_, RosterError>(toml::to_string(&document).unwrap_or_default())
    };
    let loadouts = tables(&[
        ConfigRecord::Loadout,
        ConfigRecord::Sandbox,
        ConfigRecord::Mcp,
        ConfigRecord::Overlay,
        ConfigRecord::Secret,
        ConfigRecord::SecretProvider,
        ConfigRecord::Pool,
    ])?;
    let loadouts = if loadouts.trim().is_empty() {
        LoadoutCatalog::default()
    } else {
        LoadoutCatalog::parse(&loadouts, limits)
            .map_err(|reason| error(format!("{source}: loadouts: {reason}")))?
    };
    let singleton = |record: ConfigRecord| -> Result<Option<String>, RosterError> {
        family(record)
            .into_iter()
            .next()
            .map(|(_, definition)| {
                json_to_toml(definition)
                    .map(|value| toml::to_string(&value).unwrap_or_default())
                    .map_err(|reason| error(format!("{source}: {record}: {reason}")))
            })
            .transpose()
    };
    let cables = match singleton(ConfigRecord::Cables)? {
        Some(text) => CableConfig::parse_toml(&text)
            .map_err(|reason| error(format!("{source}: cables: {reason}")))?,
        None => CableConfig::default(),
    };
    let libraries = family(ConfigRecord::Plugin);
    let pins = if libraries.is_empty() {
        None
    } else {
        let library: Vec<serde_json::Value> = libraries
            .into_iter()
            .map(|(key, mut definition)| {
                if let Some(entry) = definition.as_object_mut() {
                    entry.insert("file".to_owned(), serde_json::Value::String(key));
                }
                definition
            })
            .collect();
        let text = json_to_toml(serde_json::json!({ "library": library }))
            .map(|value| toml::to_string(&value).unwrap_or_default())
            .map_err(|reason| error(format!("{source}: plugin: {reason}")))?;
        Some(
            PinSet::from_toml(&text)
                .map_err(|reason| error(format!("{source}: plugin: {reason}")))?,
        )
    };
    let station = match family(ConfigRecord::Station).into_iter().next() {
        Some((_, definition)) => {
            let station = records::from_json::<StationRuntime>(&definition)
                .map_err(|reason| error(format!("{source}: station: {reason}")))?;
            station
                .validate()
                .map_err(|reason| error(format!("{source}: {reason}")))?;
            station
        }
        None => StationRuntime::default(),
    };
    let tls = family(ConfigRecord::Tls)
        .into_iter()
        .next()
        .map(|(_, definition)| definition);
    #[cfg(feature = "station-tls")]
    if let Some(document) = &tls {
        coqui_peer_tls::Policy::deserialize(document)
            .map_err(|reason| error(format!("{source}: tls: {reason}")))?;
    }
    let access = family(ConfigRecord::Access)
        .into_iter()
        .next()
        .map(|(_, definition)| definition);
    if let Some(text) = singleton(ConfigRecord::Access)? {
        AccessPolicy::from_toml(&text, limits.access_policy_bytes)
            .and_then(|policy| compile(&policy, &[]))
            .map_err(|reason| error(format!("{source}: access: {reason}")))?;
    }
    let mut operation_access = BTreeMap::new();
    for (operation, definition) in family(ConfigRecord::OperationAccess) {
        let record = OperationAccess::from_json(&definition).map_err(|reason| {
            error(format!(
                "{source}: operation_access {operation:?}: {reason}"
            ))
        })?;
        operation_access.insert(operation, record);
    }
    let unex = match family(ConfigRecord::Unex).into_iter().next() {
        Some((_, definition)) => records::from_json::<UnexRecord>(&definition)
            .map_err(|reason| error(format!("{source}: unex: {reason}")))?,
        None => UnexRecord::default(),
    };
    let compartments = Compartments::from_records(family(ConfigRecord::Compartment))
        .map_err(|reason| error(format!("{source}: {reason}")))?;
    compartments
        .compile(&roster)
        .map_err(|reason| error(format!("{source}: {reason}")))?;
    let fingerprints = ConfigRecord::ALL
        .into_iter()
        .map(|record| {
            let text = serde_json::to_string(&view.get(&record)).unwrap_or_default();
            (record, Sha256::digest(text.as_bytes()).into())
        })
        .collect();
    Ok(ConfigSnapshot {
        revision,
        roster,
        settings: Settings {
            theaters,
            loadouts,
            cables,
            pins,
            station,
            tls,
            access,
            operation_access,
            unex,
            compartments,
            limits: limits.clone(),
        },
        fingerprints,
    })
}

/// A JSON value as TOML, for the loaders that read a TOML document; `null` has no TOML form
/// and is refused.
fn json_to_toml(value: serde_json::Value) -> Result<toml::Value, String> {
    toml::Value::deserialize(value).map_err(|reason| reason.to_string())
}

/// Configuration kept in memory, built in code. Its revision counts the replacements.
pub struct MemoryProvider {
    state: Mutex<(u64, Roster, Settings)>,
}

impl fmt::Debug for MemoryProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryProvider")
            .finish_non_exhaustive()
    }
}

impl MemoryProvider {
    /// A provider holding `roster` and default settings at revision 0, the same for every
    /// Station.
    pub fn new(roster: Roster) -> Self {
        Self {
            state: Mutex::new((0, roster, Settings::default())),
        }
    }

    /// This provider with `settings` instead of the defaults; their `limits` decide how
    /// much of the access policy [`Settings::access_policy`] reads.
    #[must_use]
    pub fn with_settings(self, settings: Settings) -> Self {
        let mut state = self
            .state
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        state.2 = settings;
        Self {
            state: Mutex::new(state),
        }
    }

    /// This provider with its settings' `limits` replaced.
    #[must_use]
    pub fn with_limits(self, limits: ConfigLimits) -> Self {
        let mut state = self
            .state
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        state.2.limits = limits;
        Self {
            state: Mutex::new(state),
        }
    }

    /// Replaces the roster, keeping the settings; the provider moves to its next revision.
    pub fn replace(&self, roster: Roster) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 += 1;
        state.1 = roster;
    }

    /// Replaces the roster and the settings; the provider moves to its next revision.
    pub fn replace_all(&self, roster: Roster, settings: Settings) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 += 1;
        state.1 = roster;
        state.2 = settings;
    }
}

impl ConfigProvider for MemoryProvider {
    fn load(&self, _station: &str) -> Result<ConfigSnapshot, RosterError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(ConfigSnapshot::new(
            ConfigRevision::new(state.0),
            state.1.clone(),
            state.2.clone(),
        ))
    }

    fn revision(&self, _station: &str) -> Result<ConfigRevision, RosterError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(ConfigRevision::new(state.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::m::roster::{Agent, AgentRole, Operation};

    fn roster(agent: &str) -> Roster {
        let mut operation = Operation::new("G");
        operation.roster = vec![agent.to_owned()];
        let mut officer = Agent::new(agent);
        officer.handler = Some("mock-agent".to_owned());
        officer.roles = vec!["case-officer".to_owned()];
        Roster::builder()
            .operation(operation)
            .agent(officer)
            .role(AgentRole::new(
                "case-officer",
                "Runs a Mission end to end.",
                "Report to M.",
            ))
            .build()
            .unwrap()
    }

    #[test]
    fn a_memory_provider_reports_each_replacement_once() {
        let provider = MemoryProvider::new(roster("007"));
        let first = provider.load("").unwrap();
        assert_eq!(first.roster.agent(Some("G"), "007").unwrap().id, "007");
        assert!(
            provider
                .changes_since("", first.revision)
                .unwrap()
                .is_none()
        );

        provider.replace(roster("008"));
        let second = provider.changes_since("", first.revision).unwrap().unwrap();
        assert_ne!(second.revision, first.revision);
        assert!(second.roster.agent(Some("G"), "007").is_err());
        assert_eq!(second.roster.agent(Some("G"), "008").unwrap().id, "008");
        assert!(
            provider
                .changes_since("", second.revision)
                .unwrap()
                .is_none()
        );
    }

    #[cfg(feature = "station-tls")]
    #[test]
    fn the_tls_record_is_read_over_the_library_default_and_names_its_weakenings() {
        let settings = |document: Option<serde_json::Value>| Settings {
            tls: document,
            ..Settings::default()
        };
        let default = coqui_peer_tls::Policy::default();
        assert_eq!(settings(None).tls_policy().unwrap(), default);
        let read = settings(Some(serde_json::json!({
            "profile": "modern",
            "cipher_profiles": ["fips-140-3-compliant"],
        })))
        .tls_policy()
        .unwrap();
        assert_eq!(read.cipher_profiles.len(), 1);
        assert_eq!(read.cipher_profiles[0].as_str(), "fips-140-3-compliant");
        assert!(
            read.non_defaults_against(&default).is_empty(),
            "modern is stricter"
        );
        let legacy = settings(Some(serde_json::json!({"profile": "legacy"})))
            .tls_policy()
            .unwrap();
        assert!(!legacy.non_defaults_against(&default).is_empty());
        assert!(
            settings(Some(serde_json::json!({"profile": "lax"})))
                .tls_policy()
                .is_err()
        );
    }

    #[test]
    fn scopes_have_a_text_form_and_families_know_where_they_apply() {
        assert_eq!(ConfigScope::Global.as_text(), "");
        assert_eq!(ConfigScope::of_station("a").as_text(), "station:a");
        assert_eq!(
            ConfigScope::parse("station:a"),
            Some(ConfigScope::of_station("a"))
        );
        assert_eq!(ConfigScope::parse(""), Some(ConfigScope::Global));
        assert_eq!(ConfigScope::parse("station:"), None);
        assert_eq!(ConfigScope::parse("hq"), None);
        for unsafe_id in ["station:..", "station:../x", "station:a/b", "station:C:x"] {
            assert_eq!(ConfigScope::parse(unsafe_id), None, "{unsafe_id}");
        }
        assert_eq!(ConfigScope::of_station(""), ConfigScope::Global);
        assert!(ConfigRecord::Theater.station_scoped() && !ConfigRecord::Theater.applies_live());
        assert!(!ConfigRecord::Agent.station_scoped() && ConfigRecord::Agent.applies_live());
        assert_eq!(
            ConfigRecord::from_name("assistant"),
            Some(ConfigRecord::Assistant)
        );
        assert!(
            check_placement(
                ConfigRecord::Agent,
                &ConfigScope::of_station("a"),
                "007",
                &"t"
            )
            .is_err()
        );
        assert!(
            check_placement(
                ConfigRecord::Assistant,
                &ConfigScope::Global,
                "assistant",
                &"t"
            )
            .is_err()
        );
    }
}
