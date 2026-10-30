//! Scope authorization, audience expansion and quota enforcement for Cables.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde::Serialize;

use crate::directorates::registry::backend::*;

use super::{
    Cable, CableDraft, CableError, CableRecipient, CableScope, CableState, CableTarget,
    MAX_CABLE_BODY_BYTES, MAX_CABLE_TTL,
};

/// Default Cable lifetime in seconds.
pub const DEFAULT_CABLE_TTL: f64 = 15.0 * 60.0;
/// Maximum number of Agent Sessions one broadcast may reach.
pub const MAX_CABLE_FANOUT: usize = 100;
const DEFAULT_SENDER_RATE: f64 = 2.0;
const DEFAULT_SENDER_BURST: u32 = 20;
const DEFAULT_SCOPE_RATE: f64 = 10.0;
const DEFAULT_SCOPE_BURST: u32 = 50;

fn default_max_fanout() -> usize {
    MAX_CABLE_FANOUT
}

fn default_sender_rate() -> f64 {
    DEFAULT_SENDER_RATE
}

fn default_sender_burst() -> u32 {
    DEFAULT_SENDER_BURST
}

fn default_scope_rate() -> f64 {
    DEFAULT_SCOPE_RATE
}

fn default_scope_burst() -> u32 {
    DEFAULT_SCOPE_BURST
}

/// Why the relay denied a Cable request; protected recipient details are omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CableRelayError {
    /// The target is unavailable or not authorized.
    Unavailable,
    /// The request is invalid.
    InvalidRequest,
    /// The Registry or quota store could not be reached.
    RegistryUnavailable,
}

impl fmt::Display for CableRelayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "Cable target is unavailable or not authorized",
            Self::InvalidRequest => "Cable request is invalid",
            Self::RegistryUnavailable => "Cable relay is unavailable",
        })
    }
}

impl Error for CableRelayError {}

/// Relay action controlled by an explicit grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum CableAction {
    /// Send a Cable to this scope.
    Send,
    /// Receive a Cable addressed to this scope.
    Receive,
    /// Discover peers in this scope.
    Discover,
    /// List and release held Cables addressed to this scope.
    Release,
}

/// Policy applied to one recipient before the Cable is persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum CableInboundMode {
    /// Place the Cable in the recipient's inbox.
    Deliver,
    /// Hold the Cable for an authorized release path.
    Hold,
    /// Refuse the Cable without revealing the recipient's policy.
    Refuse,
}

/// Identity bound by the managed-operation adapter; senders cannot supply it in a request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CablePrincipal {
    agent: String,
    operation: String,
    theater: Option<String>,
    station: String,
    session_id: Option<String>,
    mission_id: Option<String>,
    on_behalf_of: Option<String>,
}

impl CablePrincipal {
    /// Constructs a validated principal from trusted Session binding data.
    pub fn try_new(
        agent: String,
        operation: String,
        theater: Option<String>,
        station: String,
        session_id: Option<String>,
        mission_id: Option<String>,
    ) -> Result<Self, CableError> {
        super::validate_text("Cable principal agent", Some(&agent), 256, true)?;
        super::validate_text("Cable principal operation", Some(&operation), 256, true)?;
        super::validate_text("Cable principal station", Some(&station), 256, true)?;
        if let Some(value) = theater.as_deref() {
            super::validate_text("Cable principal theater", Some(value), 256, true)?;
        }
        if let Some(value) = session_id.as_deref() {
            super::validate_text("Cable principal session_id", Some(value), 512, true)?;
        }
        if let Some(value) = mission_id.as_deref() {
            super::validate_text("Cable principal mission_id", Some(value), 512, true)?;
        }
        Ok(Self {
            agent,
            operation,
            theater,
            station,
            session_id,
            mission_id,
            on_behalf_of: None,
        })
    }

    /// Binds the person an assistant acts for. Its Cables carry the person, reach only
    /// Agents of the principal's Operation other than itself, and replies to them reach only
    /// the sending Session.
    ///
    /// # Errors
    ///
    /// Returns an error when `person` is empty, over 256 characters or has control
    /// characters.
    pub fn with_on_behalf_of(mut self, person: String) -> Result<Self, CableError> {
        super::validate_text("Cable principal on_behalf_of", Some(&person), 256, true)?;
        self.on_behalf_of = Some(person);
        Ok(self)
    }

    /// Returns the person this principal acts for, when it is an assistant's.
    pub fn on_behalf_of(&self) -> Option<&str> {
        self.on_behalf_of.as_deref()
    }

    /// Returns the bound Agent identity.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// Returns the bound Operation identity.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Returns the bound Theater identity, when present.
    pub fn theater(&self) -> Option<&str> {
        self.theater.as_deref()
    }

    /// Returns the AI6 station identity.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// Returns the provider-issued Session identity, when bound.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Returns the AI6 Mission identity, when bound.
    pub fn mission_id(&self) -> Option<&str> {
        self.mission_id.as_deref()
    }
}

/// A configured Agent identity, optionally attached to a live managed Session.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CableEndpoint {
    agent: String,
    operation: String,
    theater: Option<String>,
    station: String,
    session_id: Option<String>,
    mission_id: Option<String>,
    conversation_only: bool,
}

impl CableEndpoint {
    /// Marks this endpoint as one conversation of an assistant that acts for a person: it is
    /// never discovered and never in the audience of a Cable addressed to an Agent, a Station,
    /// a Theater, a channel or everyone; only a reply to a Cable it sent reaches it.
    #[must_use]
    pub fn with_conversation_only(mut self) -> Self {
        self.conversation_only = true;
        self
    }

    /// Whether only replies to its own Cables reach this endpoint.
    pub fn conversation_only(&self) -> bool {
        self.conversation_only
    }

    /// Constructs a validated directory endpoint.
    pub fn try_new(
        agent: String,
        operation: String,
        theater: Option<String>,
        station: String,
        session_id: Option<String>,
        mission_id: Option<String>,
    ) -> Result<Self, CableError> {
        let principal =
            CablePrincipal::try_new(agent, operation, theater, station, session_id, mission_id)?;
        Ok(Self {
            agent: principal.agent,
            operation: principal.operation,
            theater: principal.theater,
            station: principal.station,
            session_id: principal.session_id,
            mission_id: principal.mission_id,
            conversation_only: false,
        })
    }

    /// Returns the Agent identity.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// Returns the Operation identity.
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Returns the Theater identity, when present.
    pub fn theater(&self) -> Option<&str> {
        self.theater.as_deref()
    }

    /// Returns the station identity.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// Returns the live provider Session identity, or `None` for offline delivery.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Returns the active AI6 Mission identity, when present.
    pub fn mission_id(&self) -> Option<&str> {
        self.mission_id.as_deref()
    }
}

/// One explicit permission for an Agent to send, receive or discover a scope.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CableGrant {
    agent: String,
    action: CableAction,
    target: CableTarget,
}

impl CableGrant {
    /// Constructs a grant; `*` applies to every Agent identity.
    pub fn try_new(
        agent: String,
        action: CableAction,
        target: CableTarget,
    ) -> Result<Self, CableError> {
        if agent != "*" {
            super::validate_text("Cable grant agent", Some(&agent), 256, true)?;
        }
        Ok(Self {
            agent,
            action,
            target,
        })
    }
}

/// Explicit grants and receiver-side delivery choices; there are no implicit grants.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct CablePolicy {
    grants: Vec<CableGrant>,
    inbound: BTreeMap<String, CableInboundMode>,
}

/// Strict TOML configuration for the daemon Cable relay.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CableConfig {
    #[serde(default)]
    grants: Vec<CableGrantConfig>,
    #[serde(default)]
    inbound: BTreeMap<String, CableInboundMode>,
    #[serde(default)]
    channels: BTreeMap<String, BTreeSet<String>>,
    #[serde(default = "default_max_fanout")]
    max_fanout: usize,
    #[serde(default = "default_sender_rate")]
    sender_rate: f64,
    #[serde(default = "default_sender_burst")]
    sender_burst: u32,
    #[serde(default = "default_scope_rate")]
    scope_rate: f64,
    #[serde(default = "default_scope_burst")]
    scope_burst: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CableGrantConfig {
    agent: String,
    action: CableAction,
    scope: CableScope,
    #[serde(default)]
    name: Option<String>,
}

impl Default for CableConfig {
    fn default() -> Self {
        Self {
            grants: Vec::new(),
            inbound: BTreeMap::new(),
            channels: BTreeMap::new(),
            max_fanout: MAX_CABLE_FANOUT,
            sender_rate: DEFAULT_SENDER_RATE,
            sender_burst: DEFAULT_SENDER_BURST,
            scope_rate: DEFAULT_SCOPE_RATE,
            scope_burst: DEFAULT_SCOPE_BURST,
        }
    }
}

impl CableConfig {
    /// Parses strict relay configuration from TOML.
    pub fn parse_toml(source: &str) -> Result<Self, CableError> {
        let config: Self = toml::from_str(source)
            .map_err(|error| CableError::new(format!("invalid Cable config: {error}")))?;
        config.policy()?;
        if !(1..=1_000).contains(&config.max_fanout)
            || !config.sender_rate.is_finite()
            || config.sender_rate <= 0.0
            || config.sender_burst == 0
            || !config.scope_rate.is_finite()
            || config.scope_rate <= 0.0
            || config.scope_burst == 0
        {
            return Err(CableError::new(
                "Cable config limits must be positive and bounded",
            ));
        }
        for (channel, members) in &config.channels {
            CableTarget::try_new(CableScope::Channel, Some(channel.clone()))?;
            for member in members {
                super::validate_text("Cable channel Agent", Some(member), 256, true)?;
            }
        }
        Ok(config)
    }

    /// Returns the deny-by-default policy described by this configuration.
    pub fn policy(&self) -> Result<CablePolicy, CableError> {
        let grants = self
            .grants
            .iter()
            .map(|grant| {
                CableGrant::try_new(
                    grant.agent.clone(),
                    grant.action,
                    CableTarget::try_new(grant.scope, grant.name.clone())?,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        CablePolicy::try_new(grants, self.inbound.clone())
    }

    /// Returns configured custom channel membership.
    pub fn channels(&self) -> &BTreeMap<String, BTreeSet<String>> {
        &self.channels
    }

    /// Returns the configured fan-out limit.
    pub fn max_fanout(&self) -> usize {
        self.max_fanout
    }

    /// Returns the persistent sender and scope token-bucket limits.
    pub fn quota_limits(&self) -> (f64, u32, f64, u32) {
        (
            self.sender_rate,
            self.sender_burst,
            self.scope_rate,
            self.scope_burst,
        )
    }
}

impl CablePolicy {
    /// Creates a policy with only the supplied grants and receiver choices.
    pub fn try_new(
        grants: Vec<CableGrant>,
        inbound: BTreeMap<String, CableInboundMode>,
    ) -> Result<Self, CableError> {
        for agent in inbound.keys() {
            super::validate_text("Cable inbound Agent", Some(agent), 256, true)?;
        }
        Ok(Self { grants, inbound })
    }

    /// Whether a grant lets `agent` take `action` on `target`. A grant on an Operation also
    /// covers each Operation under it, named by `ancestors` (the target's, nearest first).
    fn allows(
        &self,
        agent: &str,
        action: CableAction,
        target: &CableTarget,
        ancestors: &[String],
    ) -> bool {
        self.grants.iter().any(|grant| {
            let covers = grant.target == *target
                || (target.scope() == CableScope::Operation
                    && grant.target.scope() == CableScope::Operation
                    && grant
                        .target
                        .name()
                        .is_some_and(|name| ancestors.iter().any(|ancestor| ancestor == name)));
            grant.action == action && covers && (grant.agent == "*" || grant.agent == agent)
        })
    }

    fn inbound_mode(&self, agent: &str) -> CableInboundMode {
        self.inbound
            .get(agent)
            .copied()
            .unwrap_or(CableInboundMode::Deliver)
    }
}

/// Metadata-only audit record; it never contains message body bytes.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CableAuditRecord {
    /// Event kind, such as `cable_send` or `cable_retry`.
    pub event: &'static str,
    /// Authenticated sender Agent.
    pub agent: String,
    /// Authenticated sender Operation.
    pub operation: String,
    /// Authenticated sender Theater.
    pub theater: Option<String>,
    /// Authenticated sender station.
    pub station: String,
    /// Bound Mission identity.
    pub mission_id: Option<String>,
    /// Bound provider Session identity.
    pub session_id: Option<String>,
    /// Requested target scope.
    pub target_scope: CableScope,
    /// Requested target name, when present.
    pub target_name: Option<String>,
    /// Relay decision.
    pub decision: &'static str,
    /// Decision time as Unix seconds.
    pub at: f64,
    /// Accepted Cable ID, when present.
    pub cable_id: Option<String>,
    /// The person the sender acted for, when it is an assistant acting for one.
    pub on_behalf_of: Option<String>,
}

/// Recipient identity shown to an authorized Cable approver without the message body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct CableHeldRecipient {
    /// Resolved Agent identity.
    pub agent: String,
    /// Provider Session the Cable was addressed to, if one was active at send time.
    pub session_id: Option<String>,
}

/// Metadata for a held Cable visible to an Agent with a matching release grant.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct CableHeldDelivery {
    /// Relay-generated Cable identity.
    pub cable_id: String,
    /// Authenticated sender Agent.
    pub from_agent: String,
    /// Original explicit audience.
    pub target: CableTarget,
    /// Recipients still held for approval.
    pub recipients: Vec<CableHeldRecipient>,
    /// Expiration time as Unix seconds.
    pub expires_at: f64,
}

/// Outcome of releasing one held Cable to its still-authorized recipients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct CableRelease {
    /// Recipients moved to delivered.
    pub delivered: usize,
    /// Recipients whose receive grant was revoked before release.
    pub refused: usize,
}

/// Explicit send request; authenticated sender identity is never included.
#[derive(Clone)]
#[non_exhaustive]
pub struct CableSendRequest {
    target: CableTarget,
    body: String,
    idempotency_key: String,
    ttl: f64,
    conversation_id: Option<String>,
    reply_to: Option<String>,
    /// For a reply to a Cable sent for a person: only the Session that sent it.
    to_session: Option<String>,
}

impl fmt::Debug for CableSendRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CableSendRequest")
            .field("target", &self.target)
            .field("body", &"[redacted]")
            .field("idempotency_key", &self.idempotency_key)
            .field("ttl", &self.ttl)
            .field("conversation_id", &self.conversation_id)
            .field("reply_to", &self.reply_to)
            .field("to_session", &self.to_session)
            .finish()
    }
}

/// Agent-facing view of one delivered Cable; recipient lists and retry keys are omitted.
#[derive(Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct CableDelivery {
    /// Relay-generated Cable identity.
    pub cable_id: String,
    /// Authenticated sender Agent.
    pub from_agent: String,
    /// Authenticated sender Operation.
    pub from_operation: String,
    /// Authenticated sender Theater, when present.
    pub from_theater: Option<String>,
    /// Authenticated sender Station.
    pub from_station: String,
    /// AI6 Mission identity, when present.
    pub mission_id: Option<String>,
    /// Provider Session identity, when present.
    pub session_id: Option<String>,
    /// Explicit audience from the accepted Cable.
    pub target: CableTarget,
    /// Message body; treat as untrusted input.
    pub body: String,
    /// Conversation identity, when present.
    pub conversation_id: Option<String>,
    /// Parent Cable identity, when this is a reply.
    pub reply_to: Option<String>,
    /// The person an assistant sent this Cable for, when it acted for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<String>,
    /// How the sender is shown: `<from_agent> for <on_behalf_of>`, when sent for a person.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_as: Option<String>,
    /// Creation time as Unix seconds.
    pub created_at: f64,
    /// Expiration time as Unix seconds.
    pub expires_at: f64,
    /// This Agent's delivery state.
    pub state: CableState,
}

impl fmt::Debug for CableDelivery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CableDelivery")
            .field("cable_id", &self.cable_id)
            .field("from_agent", &self.from_agent)
            .field("from_operation", &self.from_operation)
            .field("from_theater", &self.from_theater)
            .field("from_station", &self.from_station)
            .field("mission_id", &self.mission_id)
            .field("session_id", &self.session_id)
            .field("target", &self.target)
            .field("body", &"[redacted]")
            .field("conversation_id", &self.conversation_id)
            .field("reply_to", &self.reply_to)
            .field("on_behalf_of", &self.on_behalf_of)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .field("state", &self.state)
            .finish()
    }
}

impl CableSendRequest {
    /// Creates a request with the default 15-minute lifetime.
    pub fn new(target: CableTarget, body: String, idempotency_key: String) -> Self {
        Self {
            target,
            body,
            idempotency_key,
            ttl: DEFAULT_CABLE_TTL,
            conversation_id: None,
            reply_to: None,
            to_session: None,
        }
    }

    /// Sets a requested lifetime in seconds, validated when the relay accepts the request.
    pub fn with_ttl(mut self, ttl: f64) -> Self {
        self.ttl = ttl;
        self
    }

    /// Groups this request with a Cable conversation.
    pub fn with_conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    /// Marks this request as a reply to an existing Cable.
    pub fn with_reply_to(mut self, cable_id: impl Into<String>) -> Self {
        self.reply_to = Some(cable_id.into());
        self
    }
}

type EndpointSource = dyn Fn() -> Vec<CableEndpoint> + Send + Sync;
type LineageSource = dyn Fn(&str) -> Vec<String> + Send + Sync;
type AuditSink = dyn Fn(&CableAuditRecord) + Send + Sync;
type PersonAudit = dyn Fn(&CableAuditRecord) -> Result<(), String> + Send + Sync;
type EndpointKey = (String, String, Option<String>, Option<String>);

/// Authorizes sends and receives, snapshots broadcast audiences and stores accepted Cables.
pub struct CableRelay {
    registry: Arc<dyn RegistryBackend>,
    policy: RwLock<CablePolicy>,
    endpoints: Arc<EndpointSource>,
    lineage: Arc<LineageSource>,
    channels: BTreeMap<String, BTreeSet<String>>,
    max_fanout: usize,
    sender_rate: f64,
    sender_burst: u32,
    scope_rate: f64,
    scope_burst: u32,
    now: Arc<dyn Fn() -> f64 + Send + Sync>,
    audit: Option<Arc<AuditSink>>,
    person_audit: Option<Arc<PersonAudit>>,
    live_endpoints: Mutex<BTreeMap<EndpointKey, CableEndpoint>>,
}

impl CableRelay {
    /// Creates a relay over the file Registry and an authoritative Agent directory.
    pub fn new(
        registry: Arc<dyn RegistryBackend>,
        policy: CablePolicy,
        endpoints: impl Fn() -> Vec<CableEndpoint> + Send + Sync + 'static,
        channels: BTreeMap<String, BTreeSet<String>>,
    ) -> Self {
        Self {
            registry,
            policy: RwLock::new(policy),
            endpoints: Arc::new(endpoints),
            lineage: Arc::new(|_: &str| Vec::new()),
            channels,
            max_fanout: MAX_CABLE_FANOUT,
            sender_rate: DEFAULT_SENDER_RATE,
            sender_burst: DEFAULT_SENDER_BURST,
            scope_rate: DEFAULT_SCOPE_RATE,
            scope_burst: DEFAULT_SCOPE_BURST,
            now: Arc::new(system_time),
            audit: None,
            person_audit: None,
            live_endpoints: Mutex::new(BTreeMap::new()),
        }
    }

    /// Sets bounded fan-out and persistent sender/scope token-bucket limits.
    pub fn with_limits(
        mut self,
        max_fanout: usize,
        sender_rate: f64,
        sender_burst: u32,
        scope_rate: f64,
        scope_burst: u32,
    ) -> Result<Self, CableError> {
        if !(1..=1_000).contains(&max_fanout)
            || !sender_rate.is_finite()
            || sender_rate <= 0.0
            || sender_burst == 0
            || !scope_rate.is_finite()
            || scope_rate <= 0.0
            || scope_burst == 0
        {
            return Err(CableError::new(
                "Cable relay limits must be positive and bounded",
            ));
        }
        self.max_fanout = max_fanout;
        self.sender_rate = sender_rate;
        self.sender_burst = sender_burst;
        self.scope_rate = scope_rate;
        self.scope_burst = scope_burst;
        Ok(self)
    }

    /// Sets where the tree of Operations comes from: `lineage` names the Operations one is
    /// subordinate to, nearest first. Without it, an `operation` audience is that Operation's
    /// Agents alone and a grant covers only the Operation it names.
    #[must_use]
    pub fn with_lineage(
        mut self,
        lineage: impl Fn(&str) -> Vec<String> + Send + Sync + 'static,
    ) -> Self {
        self.lineage = Arc::new(lineage);
        self
    }

    /// Whether `policy` lets `agent` take `action` on `target`, a grant on an Operation
    /// covering the Operations under it.
    fn allows(
        &self,
        policy: &CablePolicy,
        agent: &str,
        action: CableAction,
        target: &CableTarget,
    ) -> bool {
        let ancestors = match (target.scope(), target.name()) {
            (CableScope::Operation, Some(operation)) => (self.lineage)(operation),
            _ => Vec::new(),
        };
        policy.allows(agent, action, target, &ancestors)
    }

    /// Sets the relay clock, primarily for deterministic operation and tests.
    pub fn with_clock(mut self, now: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        self.now = Arc::new(now);
        self
    }

    /// Installs a metadata-only audit sink.
    pub fn with_audit_sink(
        mut self,
        sink: impl Fn(&CableAuditRecord) + Send + Sync + 'static,
    ) -> Self {
        self.audit = Some(Arc::new(sink));
        self
    }

    /// Installs the durable audit log of Cables sent for a person: `write` is given the
    /// record of each such Cable before it is stored, and the send is refused when it
    /// errs. Without one, nothing is sent for a person.
    pub fn with_person_audit(
        mut self,
        write: impl Fn(&CableAuditRecord) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.person_audit = Some(Arc::new(write));
        self
    }

    /// Replaces relay policy; new send and delivery checks use the updated rules.
    pub fn update_policy(&self, policy: CablePolicy) -> Result<(), CableRelayError> {
        *self
            .policy
            .write()
            .map_err(|_| CableRelayError::RegistryUnavailable)? = policy;
        Ok(())
    }

    /// Publishes one trusted, currently managed Agent Session.
    pub fn register_endpoint(&self, endpoint: CableEndpoint) -> Result<(), CableRelayError> {
        let key = endpoint_key(&endpoint);
        self.live_endpoints
            .lock()
            .map_err(|_| CableRelayError::RegistryUnavailable)?
            .insert(key, endpoint);
        Ok(())
    }

    /// Removes one managed Session without changing the configured Agent directory.
    pub fn unregister_endpoint(&self, endpoint: &CableEndpoint) -> Result<(), CableRelayError> {
        self.live_endpoints
            .lock()
            .map_err(|_| CableRelayError::RegistryUnavailable)?
            .remove(&endpoint_key(endpoint));
        Ok(())
    }

    /// Lists peers discoverable by a principal within its own Station.
    pub fn discover(
        &self,
        principal: &CablePrincipal,
    ) -> Result<Vec<CableEndpoint>, CableRelayError> {
        let station = CableTarget::try_new(CableScope::Station, Some(principal.station.clone()))
            .map_err(|_| CableRelayError::InvalidRequest)?;
        let policy = self
            .policy
            .read()
            .map_err(|_| CableRelayError::RegistryUnavailable)?;
        if !self.allows(&policy, &principal.agent, CableAction::Discover, &station) {
            return Ok(Vec::new());
        }
        let mut visible = self
            .all_endpoints()?
            .into_iter()
            .filter(|endpoint| {
                endpoint.agent != principal.agent
                    && !endpoint.conversation_only
                    && endpoint.station == principal.station
                    && self.allows(&policy, &endpoint.agent, CableAction::Receive, &station)
            })
            .collect::<Vec<_>>();
        visible.sort_by(|left, right| {
            left.agent
                .cmp(&right.agent)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        Ok(visible)
    }

    /// Applies current receive policy, marks accepted queued items delivered and reads an inbox.
    pub fn inbox(
        &self,
        principal: &CablePrincipal,
        limit: usize,
    ) -> Result<Vec<CableDelivery>, CableRelayError> {
        self.read_inbox(principal, None, limit)
    }

    /// As [`inbox`](Self::inbox), for the Cables sent in `operation` only: the others are left
    /// as they are, neither delivered nor counted against `limit`.
    pub fn inbox_from(
        &self,
        principal: &CablePrincipal,
        operation: &str,
        limit: usize,
    ) -> Result<Vec<CableDelivery>, CableRelayError> {
        self.read_inbox(principal, Some(operation), limit)
    }

    fn read_inbox(
        &self,
        principal: &CablePrincipal,
        operation: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CableDelivery>, CableRelayError> {
        let now = (self.now)();
        if !now.is_finite() || now <= 0.0 {
            return Err(CableRelayError::RegistryUnavailable);
        }
        let policy = self
            .policy
            .read()
            .map_err(|_| CableRelayError::RegistryUnavailable)?;
        let queued = self
            .registry
            .cable_inbox(
                &principal.agent,
                principal.session_id.as_deref(),
                if operation.is_some() { 1_000 } else { limit },
                Some(now),
            )
            .map_err(|_| CableRelayError::RegistryUnavailable)?;
        let mut deliveries = Vec::with_capacity(queued.len().min(limit));
        for listed in queued {
            if deliveries.len() >= limit {
                break;
            }
            let Some(cable) = self
                .registry
                .get_cable(listed.id())
                .map_err(|_| CableRelayError::RegistryUnavailable)?
            else {
                continue;
            };
            if operation.is_some_and(|operation| cable.sender_operation() != operation) {
                continue;
            }
            let Some(recipient) = recipient_for(&cable, principal) else {
                continue;
            };
            if recipient.state() == CableState::Queued {
                let receiver_target = receiver_target(&cable, principal)?;
                if !self.allows(
                    &policy,
                    &principal.agent,
                    CableAction::Receive,
                    &receiver_target,
                ) {
                    self.set_delivery_state(&cable, recipient, CableState::Refused, now)?;
                    self.record_event(
                        "cable_delivery",
                        principal,
                        cable.target(),
                        "refused",
                        Some(&cable),
                    );
                    continue;
                }
                let state = match policy.inbound_mode(&principal.agent) {
                    CableInboundMode::Deliver => CableState::Delivered,
                    CableInboundMode::Hold => CableState::Held,
                    CableInboundMode::Refuse => CableState::Refused,
                };
                let Some(updated) = self.set_delivery_state(&cable, recipient, state, now)? else {
                    continue;
                };
                if state != CableState::Delivered {
                    self.record_event(
                        "cable_delivery",
                        principal,
                        cable.target(),
                        "held_or_refused",
                        Some(&updated),
                    );
                    continue;
                }
                let Some(recipient) = recipient_for(&updated, principal) else {
                    continue;
                };
                deliveries.push(delivery_view(&updated, recipient));
                self.record_event(
                    "cable_delivery",
                    principal,
                    cable.target(),
                    "delivered",
                    Some(&updated),
                );
            } else if recipient.state() == CableState::Delivered {
                deliveries.push(delivery_view(&cable, recipient));
            }
        }
        Ok(deliveries)
    }

    /// Lists held Cable metadata covered by the caller's explicit release grants.
    pub fn held(
        &self,
        principal: &CablePrincipal,
        limit: usize,
    ) -> Result<Vec<CableHeldDelivery>, CableRelayError> {
        if !(1..=1_000).contains(&limit) {
            return Err(CableRelayError::InvalidRequest);
        }
        let now = (self.now)();
        if !now.is_finite() || now <= 0.0 {
            return Err(CableRelayError::RegistryUnavailable);
        }
        let policy = self
            .policy
            .read()
            .map_err(|_| CableRelayError::RegistryUnavailable)?;
        let cables = self
            .registry
            .held_cables(1_000, Some(now))
            .map_err(|_| CableRelayError::RegistryUnavailable)?;
        let mut held = Vec::new();
        for cable in cables {
            if !self.allows(
                &policy,
                &principal.agent,
                CableAction::Release,
                cable.target(),
            ) {
                continue;
            }
            let recipients = cable
                .recipients()
                .iter()
                .filter(|recipient| recipient.state() == CableState::Held)
                .map(|recipient| CableHeldRecipient {
                    agent: recipient.agent().to_owned(),
                    session_id: recipient.session_id().map(str::to_owned),
                })
                .collect::<Vec<_>>();
            if !recipients.is_empty() {
                held.push(CableHeldDelivery {
                    cable_id: cable.id().to_owned(),
                    from_agent: cable.sender_agent().to_owned(),
                    target: cable.target().clone(),
                    recipients,
                    expires_at: cable.expires_at(),
                });
            }
            if held.len() == limit {
                break;
            }
        }
        Ok(held)
    }

    /// Releases held recipients covered by the caller's grant and current receive policy.
    pub fn release(
        &self,
        principal: &CablePrincipal,
        cable_id: &str,
    ) -> Result<CableRelease, CableRelayError> {
        let cable = self
            .registry
            .get_cable(cable_id)
            .map_err(|_| CableRelayError::Unavailable)?
            .ok_or(CableRelayError::Unavailable)?;
        let policy = self
            .policy
            .read()
            .map_err(|_| CableRelayError::RegistryUnavailable)?;
        if !self.allows(
            &policy,
            &principal.agent,
            CableAction::Release,
            cable.target(),
        ) {
            return Err(CableRelayError::Unavailable);
        }
        let now = (self.now)();
        if !now.is_finite() || now <= 0.0 {
            return Err(CableRelayError::RegistryUnavailable);
        }
        let held = cable
            .recipients()
            .iter()
            .filter(|recipient| recipient.state() == CableState::Held)
            .cloned()
            .collect::<Vec<_>>();
        if held.is_empty() {
            return Err(CableRelayError::Unavailable);
        }
        if cable.expires_at() <= now {
            for recipient in held {
                self.set_delivery_state(&cable, &recipient, CableState::Expired, now)?;
            }
            return Err(CableRelayError::Unavailable);
        }
        let endpoints = self.all_endpoints()?;
        let mut result = CableRelease {
            delivered: 0,
            refused: 0,
        };
        for recipient in held {
            let endpoint = endpoints.iter().find(|endpoint| {
                endpoint.agent == recipient.agent()
                    && recipient
                        .session_id()
                        .is_none_or(|session| endpoint.session_id.as_deref() == Some(session))
            });
            let receive_target =
                endpoint.and_then(|endpoint| receiver_target_for_endpoint(&cable, endpoint).ok());
            let next_state = if receive_target.is_some_and(|target| {
                self.allows(&policy, recipient.agent(), CableAction::Receive, &target)
            }) {
                CableState::Delivered
            } else {
                CableState::Refused
            };
            if self
                .set_delivery_state(&cable, &recipient, next_state, now)?
                .is_some()
            {
                match next_state {
                    CableState::Delivered => result.delivered += 1,
                    CableState::Refused => result.refused += 1,
                    _ => unreachable!("release chooses only delivered or refused"),
                }
            }
        }
        self.record_event(
            "cable_release",
            principal,
            cable.target(),
            if result.delivered > 0 {
                "released"
            } else {
                "refused"
            },
            Some(&cable),
        );
        Ok(result)
    }

    /// Acknowledges a delivered Cable addressed to the calling managed Session.
    pub fn acknowledge(
        &self,
        principal: &CablePrincipal,
        cable_id: &str,
    ) -> Result<(), CableRelayError> {
        let cable = self
            .registry
            .get_cable(cable_id)
            .map_err(|_| CableRelayError::Unavailable)?
            .ok_or(CableRelayError::Unavailable)?;
        let recipient = recipient_for(&cable, principal).ok_or(CableRelayError::Unavailable)?;
        if recipient.state() == CableState::Acked {
            return Ok(());
        }
        if recipient.state() != CableState::Delivered {
            return Err(CableRelayError::Unavailable);
        }
        let now = (self.now)();
        if !now.is_finite() || now <= 0.0 {
            return Err(CableRelayError::RegistryUnavailable);
        }
        if cable.expires_at() <= now {
            self.set_delivery_state(&cable, recipient, CableState::Expired, now)?;
            return Err(CableRelayError::Unavailable);
        }
        let updated = self
            .set_delivery_state(&cable, recipient, CableState::Acked, now)?
            .ok_or(CableRelayError::Unavailable)?;
        self.record_event(
            "cable_ack",
            principal,
            cable.target(),
            "accepted",
            Some(&updated),
        );
        Ok(())
    }

    /// Replies to a delivered Cable; sender and reply threading come from the stored record.
    pub fn reply(
        &self,
        principal: &CablePrincipal,
        cable_id: &str,
        body: String,
        idempotency_key: String,
    ) -> Result<Cable, CableRelayError> {
        let original = self
            .registry
            .get_cable(cable_id)
            .map_err(|_| CableRelayError::Unavailable)?
            .ok_or(CableRelayError::Unavailable)?;
        let recipient = recipient_for(&original, principal).ok_or(CableRelayError::Unavailable)?;
        if !matches!(recipient.state(), CableState::Delivered | CableState::Acked)
            || original.expires_at() <= (self.now)()
        {
            return Err(CableRelayError::Unavailable);
        }
        let target =
            CableTarget::try_new(CableScope::Agent, Some(original.sender_agent().to_owned()))
                .map_err(|_| CableRelayError::InvalidRequest)?;
        let mut request =
            CableSendRequest::new(target, body, idempotency_key).with_reply_to(original.id());
        request.conversation_id = Some(
            original
                .conversation_id()
                .map(str::to_owned)
                .unwrap_or_else(|| original.id().to_owned()),
        );
        // A Cable sent for a person came from one conversation of a shared assistant Agent;
        // its reply goes back to that conversation alone, not to the Agent's others.
        if original.on_behalf_of().is_some() {
            request.to_session = Some(
                original
                    .session_id()
                    .ok_or(CableRelayError::Unavailable)?
                    .to_owned(),
            );
        }
        self.send(principal, request)
    }

    fn set_delivery_state(
        &self,
        cable: &Cable,
        recipient: &CableRecipient,
        state: CableState,
        now: f64,
    ) -> Result<Option<Cable>, CableRelayError> {
        self.registry
            .update_cable_delivery(
                cable.id(),
                recipient.agent(),
                recipient.session_id(),
                state,
                Some(now),
            )
            .map_err(|_| CableRelayError::Unavailable)
    }

    /// Authorizes, expands and persists one Cable without accepting caller-selected identity.
    pub fn send(
        &self,
        principal: &CablePrincipal,
        request: CableSendRequest,
    ) -> Result<Cable, CableRelayError> {
        if !request.ttl.is_finite() || request.ttl <= 0.0 || request.ttl > MAX_CABLE_TTL {
            return Err(CableRelayError::InvalidRequest);
        }
        if request.body.trim().is_empty()
            || request.body.len() > MAX_CABLE_BODY_BYTES
            || super::validate_text(
                "Cable idempotency_key",
                Some(&request.idempotency_key),
                256,
                true,
            )
            .is_err()
            || request.conversation_id.as_deref().is_some_and(|value| {
                super::validate_text("Cable conversation_id", Some(value), 64, true).is_err()
            })
            || request.reply_to.as_deref().is_some_and(|value| {
                super::validate_text("Cable reply_to", Some(value), 64, true).is_err()
            })
        {
            return Err(CableRelayError::InvalidRequest);
        }
        let now = (self.now)();
        if !now.is_finite() || now <= 0.0 || !(now + request.ttl).is_finite() {
            return Err(CableRelayError::RegistryUnavailable);
        }

        if let Some(existing) = self
            .registry
            .find_cable(&principal.agent, &request.idempotency_key)
            .map_err(|_| CableRelayError::RegistryUnavailable)?
        {
            let mut retry = self.draft(principal, &request, existing.recipients().to_vec(), now)?;
            retry.id = existing.id().to_owned();
            retry.created_at = existing.created_at();
            retry.expires_at = existing.expires_at();
            let retry = Cable::try_new(retry).map_err(|_| CableRelayError::InvalidRequest)?;
            self.audit_for_person("cable_retry", principal, &request.target, &retry)?;
            let accepted = self
                .registry
                .submit_cable(&retry)
                .map_err(|_| CableRelayError::Unavailable)?;
            self.record_event(
                "cable_retry",
                principal,
                &request.target,
                "accepted",
                Some(&accepted),
            );
            return Ok(accepted);
        }

        let policy = self
            .policy
            .read()
            .map_err(|_| CableRelayError::RegistryUnavailable)?;
        let mut candidates = self.candidates(&request.target)?;
        if principal.on_behalf_of.is_some() {
            // Acting for a person, an assistant reaches only the Operation it acts in.
            candidates.retain(|endpoint| {
                endpoint.operation == principal.operation && endpoint.agent != principal.agent
            });
        }
        if let Some(session) = request.to_session.as_deref() {
            candidates.retain(|endpoint| endpoint.session_id.as_deref() == Some(session));
        } else {
            // An assistant's conversations, each a different person's, hear only replies.
            candidates.retain(|endpoint| !endpoint.conversation_only);
        }
        let recipients =
            self.authorized_recipients(&policy, principal, &request.target, candidates)?;
        if recipients.is_empty() || recipients.len() > self.max_fanout {
            self.record_event("cable_send", principal, &request.target, "denied", None);
            return Err(CableRelayError::Unavailable);
        }

        let target_name = request.target.name().unwrap_or("");
        let scope = format!("{}:{target_name}", scope_name(request.target.scope()));
        // An assistant shared by many people has a sender bucket per person, so one person's
        // sends do not use up another's.
        let sender = match principal.on_behalf_of.as_deref() {
            Some(person) => format!("sender:{}:person:{person}", principal.agent),
            None => format!("sender:{}", principal.agent),
        };
        let buckets = [
            (sender, self.sender_rate, self.sender_burst),
            (format!("scope:{scope}"), self.scope_rate, self.scope_burst),
        ];
        let borrowed = buckets
            .iter()
            .map(|(key, rate, capacity)| (key.as_str(), *rate, *capacity))
            .collect::<Vec<_>>();
        if !self
            .registry
            .take_cable_quotas(&borrowed, now)
            .map_err(|_| CableRelayError::RegistryUnavailable)?
        {
            self.record_event(
                "cable_send",
                principal,
                &request.target,
                "rate_limited",
                None,
            );
            return Err(CableRelayError::Unavailable);
        }

        let draft = self.draft(principal, &request, recipients, now)?;
        let cable = Cable::try_new(draft).map_err(|_| CableRelayError::InvalidRequest)?;
        self.audit_for_person("cable_send", principal, &request.target, &cable)?;
        let accepted = self
            .registry
            .submit_cable(&cable)
            .map_err(|_| CableRelayError::Unavailable)?;
        self.record_event(
            "cable_send",
            principal,
            &request.target,
            "accepted",
            Some(&accepted),
        );
        Ok(accepted)
    }

    fn draft(
        &self,
        principal: &CablePrincipal,
        request: &CableSendRequest,
        recipients: Vec<CableRecipient>,
        now: f64,
    ) -> Result<CableDraft, CableRelayError> {
        if request.body.len() > MAX_CABLE_BODY_BYTES {
            return Err(CableRelayError::InvalidRequest);
        }
        let id = new_cable_id();
        let mut draft = CableDraft::new(
            principal.agent.clone(),
            principal.operation.clone(),
            principal.station.clone(),
            request.target.clone(),
            request.body.clone(),
            recipients,
            request.idempotency_key.clone(),
            id,
            now,
            now + request.ttl,
        );
        if let Some(theater) = principal.theater.as_deref() {
            draft = draft.with_sender_theater(theater);
        }
        if let Some(mission) = principal.mission_id.as_deref() {
            draft = draft.with_mission(mission);
        }
        if let Some(session) = principal.session_id.as_deref() {
            draft = draft.with_session(session);
        }
        if let Some(conversation) = request.conversation_id.as_deref() {
            draft = draft.with_conversation(conversation);
        }
        if let Some(reply_to) = request.reply_to.as_deref() {
            draft = draft.with_reply_to(reply_to);
        }
        if let Some(person) = principal.on_behalf_of.as_deref() {
            draft = draft.with_on_behalf_of(person);
        }
        Ok(draft)
    }

    fn candidates(&self, target: &CableTarget) -> Result<Vec<CableEndpoint>, CableRelayError> {
        let endpoints = self.all_endpoints()?;
        let mut candidates = match target.scope() {
            CableScope::Agent => endpoints
                .into_iter()
                .filter(|endpoint| Some(endpoint.agent.as_str()) == target.name())
                .collect(),
            CableScope::Station => endpoints
                .into_iter()
                .filter(|endpoint| Some(endpoint.station.as_str()) == target.name())
                .collect(),
            CableScope::Theater => endpoints
                .into_iter()
                .filter(|endpoint| endpoint.theater.as_deref() == target.name())
                .collect(),
            CableScope::Global => endpoints,
            CableScope::Operation => {
                let operation = target.name().unwrap_or("");
                endpoints
                    .into_iter()
                    .filter(|endpoint| {
                        endpoint.operation == operation
                            || (self.lineage)(&endpoint.operation)
                                .iter()
                                .any(|ancestor| ancestor == operation)
                    })
                    .collect()
            }
            CableScope::Channel => {
                let members = self.channels.get(target.name().unwrap_or(""));
                endpoints
                    .into_iter()
                    .filter(|endpoint| members.is_some_and(|set| set.contains(&endpoint.agent)))
                    .collect()
            }
        };
        candidates.sort_by(|left, right| {
            left.agent
                .cmp(&right.agent)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        Ok(candidates)
    }

    fn all_endpoints(&self) -> Result<Vec<CableEndpoint>, CableRelayError> {
        let mut endpoints = (self.endpoints)();
        endpoints.extend(
            self.live_endpoints
                .lock()
                .map_err(|_| CableRelayError::RegistryUnavailable)?
                .values()
                .cloned(),
        );
        Ok(endpoints)
    }

    fn authorized_recipients(
        &self,
        policy: &CablePolicy,
        sender: &CablePrincipal,
        target: &CableTarget,
        candidates: Vec<CableEndpoint>,
    ) -> Result<Vec<CableRecipient>, CableRelayError> {
        if target.scope() != CableScope::Agent
            && !self.allows(policy, &sender.agent, CableAction::Send, target)
        {
            return Err(CableRelayError::Unavailable);
        }
        // A quarantined sender's Cables are held for an authorized release, never delivered.
        let sender_quarantined = self
            .registry
            .is_quarantined(&sender.agent, Some(&sender.operation))
            .map_err(|_| CableRelayError::RegistryUnavailable)?;
        let mut selected = Vec::new();
        let mut seen = BTreeSet::new();
        for endpoint in candidates {
            let (sender_target, receiver_target) =
                if target.scope() == CableScope::Agent && endpoint.station == sender.station {
                    (
                        CableTarget::try_new(CableScope::Station, Some(sender.station.clone()))
                            .map_err(|_| CableRelayError::InvalidRequest)?,
                        CableTarget::try_new(CableScope::Station, Some(endpoint.station.clone()))
                            .map_err(|_| CableRelayError::InvalidRequest)?,
                    )
                } else if target.scope() == CableScope::Agent {
                    (
                        target.clone(),
                        CableTarget::try_new(CableScope::Agent, Some(sender.agent.clone()))
                            .map_err(|_| CableRelayError::InvalidRequest)?,
                    )
                } else {
                    (target.clone(), target.clone())
                };
            if !self.allows(policy, &sender.agent, CableAction::Send, &sender_target)
                || !self.allows(
                    policy,
                    &endpoint.agent,
                    CableAction::Receive,
                    &receiver_target,
                )
            {
                continue;
            }
            let identity = (endpoint.agent.clone(), endpoint.session_id.clone());
            if !seen.insert(identity) {
                continue;
            }
            let state = match policy.inbound_mode(&endpoint.agent) {
                CableInboundMode::Deliver if sender_quarantined => CableState::Held,
                CableInboundMode::Deliver => CableState::Queued,
                CableInboundMode::Hold => CableState::Held,
                CableInboundMode::Refuse => CableState::Refused,
            };
            selected.push(
                CableRecipient::try_new(endpoint.agent, endpoint.session_id, state)
                    .map_err(|_| CableRelayError::InvalidRequest)?,
            );
        }
        Ok(selected)
    }

    /// For a principal acting for a person, writes the audit record of `cable` before the
    /// Cable is stored: a relay that cannot write it sends nothing for the person.
    fn audit_for_person(
        &self,
        event: &'static str,
        principal: &CablePrincipal,
        target: &CableTarget,
        cable: &Cable,
    ) -> Result<(), CableRelayError> {
        if principal.on_behalf_of.is_none() {
            return Ok(());
        }
        let record = self.audit_record(event, principal, target, "accepted", Some(cable));
        let written = match &self.person_audit {
            Some(write) => write(&record),
            None => Err("this relay keeps no audit log for Cables sent for a person".to_owned()),
        };
        if written.is_err() {
            self.record_event(event, principal, target, "audit_failed", Some(cable));
            return Err(CableRelayError::Unavailable);
        }
        Ok(())
    }

    fn record_event(
        &self,
        event: &'static str,
        principal: &CablePrincipal,
        target: &CableTarget,
        decision: &'static str,
        cable: Option<&Cable>,
    ) {
        let Some(sink) = &self.audit else {
            return;
        };
        sink(&self.audit_record(event, principal, target, decision, cable));
    }

    fn audit_record(
        &self,
        event: &'static str,
        principal: &CablePrincipal,
        target: &CableTarget,
        decision: &'static str,
        cable: Option<&Cable>,
    ) -> CableAuditRecord {
        CableAuditRecord {
            event,
            agent: principal.agent.clone(),
            operation: principal.operation.clone(),
            theater: principal.theater.clone(),
            station: principal.station.clone(),
            mission_id: principal.mission_id.clone(),
            session_id: principal.session_id.clone(),
            target_scope: target.scope(),
            target_name: target.name().map(str::to_owned),
            decision,
            at: (self.now)(),
            cable_id: cable.map(|item| item.id().to_owned()),
            on_behalf_of: principal.on_behalf_of.clone(),
        }
    }
}

fn recipient_for<'a>(cable: &'a Cable, principal: &CablePrincipal) -> Option<&'a CableRecipient> {
    cable.recipients().iter().find(|recipient| {
        recipient.agent() == principal.agent
            && recipient
                .session_id()
                .is_none_or(|session| Some(session) == principal.session_id.as_deref())
    })
}

fn receiver_target(
    cable: &Cable,
    principal: &CablePrincipal,
) -> Result<CableTarget, CableRelayError> {
    if cable.target().scope() != CableScope::Agent {
        return Ok(cable.target().clone());
    }
    if cable.sender_station() == principal.station {
        CableTarget::try_new(CableScope::Station, Some(principal.station.clone()))
            .map_err(|_| CableRelayError::InvalidRequest)
    } else {
        CableTarget::try_new(CableScope::Agent, Some(cable.sender_agent().to_owned()))
            .map_err(|_| CableRelayError::InvalidRequest)
    }
}

fn receiver_target_for_endpoint(
    cable: &Cable,
    endpoint: &CableEndpoint,
) -> Result<CableTarget, CableRelayError> {
    if cable.target().scope() != CableScope::Agent {
        return Ok(cable.target().clone());
    }
    if cable.sender_station() == endpoint.station() {
        CableTarget::try_new(CableScope::Station, Some(endpoint.station().to_owned()))
            .map_err(|_| CableRelayError::InvalidRequest)
    } else {
        CableTarget::try_new(CableScope::Agent, Some(cable.sender_agent().to_owned()))
            .map_err(|_| CableRelayError::InvalidRequest)
    }
}

fn delivery_view(cable: &Cable, recipient: &CableRecipient) -> CableDelivery {
    CableDelivery {
        cable_id: cable.id().to_owned(),
        from_agent: cable.sender_agent().to_owned(),
        from_operation: cable.sender_operation().to_owned(),
        from_theater: cable.sender_theater().map(str::to_owned),
        from_station: cable.sender_station().to_owned(),
        mission_id: cable.mission_id().map(str::to_owned),
        session_id: cable.session_id().map(str::to_owned),
        target: cable.target().clone(),
        body: cable.body().to_owned(),
        conversation_id: cable.conversation_id().map(str::to_owned),
        reply_to: cable.reply_to().map(str::to_owned),
        on_behalf_of: cable.on_behalf_of().map(str::to_owned),
        sent_as: cable
            .on_behalf_of()
            .map(|person| format!("{} for {person}", cable.sender_agent())),
        created_at: cable.created_at(),
        expires_at: cable.expires_at(),
        state: recipient.state(),
    }
}

fn endpoint_key(endpoint: &CableEndpoint) -> EndpointKey {
    (
        endpoint.agent.clone(),
        endpoint.station.clone(),
        endpoint.session_id.clone(),
        endpoint.mission_id.clone(),
    )
}

fn scope_name(scope: CableScope) -> &'static str {
    match scope {
        CableScope::Agent => "agent",
        CableScope::Station => "station",
        CableScope::Theater => "theater",
        CableScope::Global => "global",
        CableScope::Channel => "channel",
        CableScope::Operation => "operation",
    }
}

fn system_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(1.0)
}

fn new_cable_id() -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("cable-{nanos:x}-{:x}-{sequence:x}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::store::FileRegistry;

    fn principal(agent: &str, operation: &str) -> CablePrincipal {
        CablePrincipal::try_new(
            agent.into(),
            operation.into(),
            Some("theater-a".into()),
            "station-a".into(),
            Some(format!("session-{agent}")),
            Some("mission-a".into()),
        )
        .unwrap()
    }

    fn endpoint(agent: &str, operation: &str, session: Option<&str>) -> CableEndpoint {
        endpoint_at(agent, operation, "station-a", session)
    }

    fn endpoint_at(
        agent: &str,
        operation: &str,
        station: &str,
        session: Option<&str>,
    ) -> CableEndpoint {
        CableEndpoint::try_new(
            agent.into(),
            operation.into(),
            Some("theater-a".into()),
            station.into(),
            session.map(str::to_owned),
            None,
        )
        .unwrap()
    }

    fn target(scope: CableScope, name: Option<&str>) -> CableTarget {
        CableTarget::try_new(scope, name.map(str::to_owned)).unwrap()
    }

    fn grant(agent: &str, action: CableAction, target: CableTarget) -> CableGrant {
        CableGrant::try_new(agent.into(), action, target).unwrap()
    }

    fn relay(
        registry: Arc<dyn RegistryBackend>,
        policy: CablePolicy,
        endpoints: Vec<CableEndpoint>,
    ) -> CableRelay {
        CableRelay::new(registry, policy, move || endpoints.clone(), BTreeMap::new())
            .with_clock(|| 1_700_000_000.0)
    }

    fn request(target: CableTarget, key: &str) -> CableSendRequest {
        CableSendRequest::new(target, "Review this change".into(), key.into())
    }

    #[test]
    fn an_operation_cable_reaches_the_subtree_only_with_grants_an_ancestor_may_hold() {
        let root = tempfile::tempdir().unwrap();
        let lineage = |operation: &str| -> Vec<String> {
            match operation {
                "NORTH" | "SOUTH" => vec!["FORTITUDE".into(), "BODYGUARD".into()],
                "FORTITUDE" | "ZEPPELIN" => vec!["BODYGUARD".into()],
                _ => Vec::new(),
            }
        };
        let endpoints = vec![
            endpoint("008", "SOUTH", Some("session-008")),
            endpoint("009", "FORTITUDE", Some("session-009")),
            endpoint("010", "ZEPPELIN", Some("session-010")),
            endpoint("011", "OTHER", Some("session-011")),
        ];
        let fortitude = target(CableScope::Operation, Some("FORTITUDE"));
        let registry: Arc<dyn RegistryBackend> = Arc::new(FileRegistry::open(root.path()));
        let sender = principal("007", "NORTH");

        let ungranted = relay(registry.clone(), CablePolicy::default(), endpoints.clone())
            .with_lineage(lineage);
        assert_eq!(
            ungranted
                .send(&sender, request(fortitude.clone(), "none"))
                .unwrap_err(),
            CableRelayError::Unavailable,
            "no grant, no Cable"
        );

        let bodyguard = target(CableScope::Operation, Some("BODYGUARD"));
        let policy = CablePolicy::try_new(
            vec![
                grant("007", CableAction::Send, bodyguard.clone()),
                grant("*", CableAction::Receive, bodyguard),
            ],
            BTreeMap::new(),
        )
        .unwrap();
        let granted = relay(registry, policy, endpoints).with_lineage(lineage);
        let sent = granted
            .send(&sender, request(fortitude, "subtree"))
            .unwrap();
        let mut reached: Vec<&str> = sent
            .recipients()
            .iter()
            .map(|recipient| recipient.agent())
            .collect();
        reached.sort_unstable();
        assert_eq!(
            reached,
            ["008", "009"],
            "FORTITUDE and SOUTH, not ZEPPELIN or OTHER"
        );
        assert!(
            granted
                .send(
                    &sender,
                    request(target(CableScope::Operation, Some("OTHER")), "outside")
                )
                .is_err(),
            "a grant on BODYGUARD does not cover an Operation outside it"
        );
    }

    #[test]
    fn same_station_unicast_requires_both_grants_and_retries_are_idempotent() {
        assert!(
            !format!(
                "{:?}",
                request(target(CableScope::Agent, Some("008")), "debug-check")
            )
            .contains("Review this change")
        );
        let root = tempfile::tempdir().unwrap();
        let operation = target(CableScope::Station, Some("station-a"));
        let policy = CablePolicy::try_new(
            vec![
                grant("007", CableAction::Send, operation.clone()),
                grant("008", CableAction::Receive, operation),
            ],
            BTreeMap::new(),
        )
        .unwrap();
        let service = relay(
            Arc::new(FileRegistry::open(root.path())),
            policy,
            vec![endpoint("008", "operation-a", Some("session-008"))],
        );
        let sender = principal("007", "operation-a");
        let target = target(CableScope::Agent, Some("008"));
        let first = service
            .send(&sender, request(target.clone(), "request-1"))
            .unwrap();
        let retry = service
            .send(&sender, request(target.clone(), "request-1"))
            .unwrap();
        assert_eq!(first.id(), retry.id());
        assert_eq!(retry.recipients()[0].state(), CableState::Queued);

        let denied = service.send(
            &sender,
            request(
                CableTarget::try_new(CableScope::Agent, Some("009".into())).unwrap(),
                "request-2",
            ),
        );
        assert_eq!(denied.unwrap_err(), CableRelayError::Unavailable);
    }

    #[test]
    fn a_cable_sent_for_a_person_stays_in_its_operation_and_its_reply_in_its_conversation() {
        let root = tempfile::tempdir().unwrap();
        let station = target(CableScope::Station, Some("station-a"));
        let policy = CablePolicy::try_new(
            ["moneypenny", "008", "009"]
                .into_iter()
                .flat_map(|agent| {
                    [CableAction::Send, CableAction::Receive]
                        .map(|action| grant(agent, action, station.clone()))
                })
                .collect(),
            BTreeMap::new(),
        )
        .unwrap();
        let records = Arc::new(Mutex::new(Vec::new()));
        let audit_records = records.clone();
        let service = relay(
            Arc::new(FileRegistry::open(root.path())),
            policy,
            vec![
                endpoint("008", "operation-a", Some("session-008")),
                endpoint("009", "operation-b", Some("session-009")),
            ],
        )
        .with_audit_sink(move |record| audit_records.lock().unwrap().push(record.clone()))
        .with_person_audit(|_| Ok(()));
        // Two conversations of the same assistant Agent, for two people.
        for (session, operation) in [
            ("assistant-a", "operation-a"),
            ("assistant-b", "operation-a"),
        ] {
            service
                .register_endpoint(
                    CableEndpoint::try_new(
                        "moneypenny".into(),
                        operation.into(),
                        None,
                        "station-a".into(),
                        Some(session.into()),
                        None,
                    )
                    .unwrap()
                    .with_conversation_only(),
                )
                .unwrap();
        }
        let assistant = |session: &str, person: &str| {
            CablePrincipal::try_new(
                "moneypenny".into(),
                "operation-a".into(),
                None,
                "station-a".into(),
                Some(session.into()),
                None,
            )
            .unwrap()
            .with_on_behalf_of(person.into())
            .unwrap()
        };
        let for_a = assistant("assistant-a", "person-a");
        let sent = service
            .send(
                &for_a,
                request(target(CableScope::Agent, Some("008")), "k1"),
            )
            .unwrap();
        assert_eq!(sent.on_behalf_of(), Some("person-a"));
        {
            let records = records.lock().unwrap();
            let last = records.last().unwrap();
            assert_eq!(
                (last.decision, last.on_behalf_of.as_deref()),
                ("accepted", Some("person-a"))
            );
        }
        assert_eq!(
            service
                .send(
                    &for_a,
                    request(target(CableScope::Agent, Some("009")), "k2")
                )
                .unwrap_err(),
            CableRelayError::Unavailable,
            "an Agent of another Operation is out of reach"
        );
        let broadcast = service
            .send(&for_a, request(station.clone(), "k3"))
            .unwrap();
        let reached: Vec<_> = broadcast
            .recipients()
            .iter()
            .map(|recipient| recipient.agent())
            .collect();
        assert_eq!(reached, ["008"], "neither another Operation nor itself");

        let agent = CablePrincipal::try_new(
            "008".into(),
            "operation-a".into(),
            None,
            "station-a".into(),
            Some("session-008".into()),
            None,
        )
        .unwrap();
        let delivered = service.inbox(&agent, 10).unwrap();
        let first = delivered
            .iter()
            .find(|delivery| delivery.cable_id == sent.id())
            .unwrap();
        assert_eq!(first.sent_as.as_deref(), Some("moneypenny for person-a"));
        let reply = service
            .reply(&agent, sent.id(), "Done".into(), "r1".into())
            .unwrap();
        let sessions: Vec<_> = reply
            .recipients()
            .iter()
            .map(|recipient| recipient.session_id())
            .collect();
        assert_eq!(
            sessions,
            [Some("assistant-a")],
            "the other person's conversation is not told"
        );
        assert!(
            service
                .inbox(&assistant("assistant-b", "person-b"), 10)
                .unwrap()
                .is_empty()
        );
    }

    type AuditLog = fn(&CableAuditRecord) -> Result<(), String>;

    /// Moneypenny's conversations `assistant-a` (person-a) and `assistant-b` (person-b),
    /// conversation-only, beside 008 and 010 in operation-a; all may send and receive on
    /// station-a. `audit` is the person audit log; `sender_burst` the sender bucket.
    fn assistant_relay(
        root: &std::path::Path,
        audit: Option<AuditLog>,
        sender_burst: u32,
    ) -> (CableRelay, Arc<Mutex<Vec<CableAuditRecord>>>) {
        let station = target(CableScope::Station, Some("station-a"));
        let policy = CablePolicy::try_new(
            ["moneypenny", "008", "010"]
                .into_iter()
                .flat_map(|agent| {
                    [
                        CableAction::Send,
                        CableAction::Receive,
                        CableAction::Discover,
                    ]
                    .map(|action| grant(agent, action, station.clone()))
                })
                .collect(),
            BTreeMap::new(),
        )
        .unwrap();
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = records.clone();
        let mut service = relay(
            Arc::new(FileRegistry::open(root)),
            policy,
            vec![
                endpoint("008", "operation-a", Some("session-008")),
                endpoint("010", "operation-a", Some("session-010")),
            ],
        )
        .with_limits(100, 0.001, sender_burst, 100.0, 1_000)
        .unwrap()
        .with_audit_sink(move |record| sink.lock().unwrap().push(record.clone()));
        if let Some(audit) = audit {
            service = service.with_person_audit(audit);
        }
        for session in ["assistant-a", "assistant-b"] {
            service
                .register_endpoint(
                    CableEndpoint::try_new(
                        "moneypenny".into(),
                        "operation-a".into(),
                        None,
                        "station-a".into(),
                        Some(session.into()),
                        None,
                    )
                    .unwrap()
                    .with_conversation_only(),
                )
                .unwrap();
        }
        (service, records)
    }

    fn for_person(session: &str, person: &str) -> CablePrincipal {
        CablePrincipal::try_new(
            "moneypenny".into(),
            "operation-a".into(),
            None,
            "station-a".into(),
            Some(session.into()),
            None,
        )
        .unwrap()
        .with_on_behalf_of(person.into())
        .unwrap()
    }

    fn agent_008() -> CablePrincipal {
        CablePrincipal::try_new(
            "008".into(),
            "operation-a".into(),
            None,
            "station-a".into(),
            Some("session-008".into()),
            None,
        )
        .unwrap()
    }

    #[test]
    fn an_assistants_conversations_hear_only_replies_to_their_own_cables() {
        let root = tempfile::tempdir().unwrap();
        let (service, _) = assistant_relay(root.path(), Some(|_| Ok(())), 20);
        let sent = service
            .send(
                &for_person("assistant-a", "person-a"),
                request(target(CableScope::Agent, Some("008")), "k1"),
            )
            .unwrap();
        service.inbox(&agent_008(), 10).unwrap();

        // 008 writes afresh to the assistant, to the Station and to everyone.
        assert_eq!(
            service
                .send(
                    &agent_008(),
                    request(target(CableScope::Agent, Some("moneypenny")), "fresh")
                )
                .unwrap_err(),
            CableRelayError::Unavailable,
            "no conversation is the assistant Agent's inbox"
        );
        let broadcast = service
            .send(
                &agent_008(),
                request(target(CableScope::Station, Some("station-a")), "all"),
            )
            .unwrap();
        let reached: Vec<_> = broadcast
            .recipients()
            .iter()
            .map(|recipient| recipient.agent())
            .collect();
        assert_eq!(
            reached,
            ["008", "010"],
            "her conversations are not in the audience"
        );
        assert!(
            service
                .discover(&agent_008())
                .unwrap()
                .iter()
                .all(|peer| peer.agent() != "moneypenny")
        );
        for session in ["assistant-a", "assistant-b"] {
            let person = if session == "assistant-a" {
                "person-a"
            } else {
                "person-b"
            };
            assert!(
                service
                    .inbox(&for_person(session, person), 10)
                    .unwrap()
                    .is_empty(),
                "{session}"
            );
        }
        service
            .reply(&agent_008(), sent.id(), "Done".into(), "r1".into())
            .unwrap();
        assert_eq!(
            service
                .inbox(&for_person("assistant-a", "person-a"), 10)
                .unwrap()
                .len(),
            1,
            "the reply reaches the conversation that asked"
        );
    }

    #[test]
    fn each_person_has_a_sender_bucket_of_their_own() {
        let root = tempfile::tempdir().unwrap();
        let (service, _) = assistant_relay(root.path(), Some(|_| Ok(())), 1);
        let to_008 = |key: &str| request(target(CableScope::Agent, Some("008")), key);
        let person_a = for_person("assistant-a", "person-a");
        service.send(&person_a, to_008("a1")).unwrap();
        assert_eq!(
            service.send(&person_a, to_008("a2")).unwrap_err(),
            CableRelayError::Unavailable,
            "person-a used up their bucket"
        );
        assert!(
            service
                .send(&for_person("assistant-b", "person-b"), to_008("b1"))
                .is_ok(),
            "person-b still has theirs"
        );
    }

    #[test]
    fn a_cable_for_a_person_is_not_stored_unless_its_audit_record_is_written_first() {
        fn refuse(_: &CableAuditRecord) -> Result<(), String> {
            Err("disk full".to_owned())
        }
        let refusing: AuditLog = refuse;
        for audit in [None, Some(refusing)] {
            let root = tempfile::tempdir().unwrap();
            let (service, records) = assistant_relay(root.path(), audit, 20);
            assert_eq!(
                service
                    .send(
                        &for_person("assistant-a", "person-a"),
                        request(target(CableScope::Agent, Some("008")), "k1"),
                    )
                    .unwrap_err(),
                CableRelayError::Unavailable
            );
            assert!(
                service
                    .registry
                    .find_cable("moneypenny", "k1")
                    .unwrap()
                    .is_none()
            );
            assert!(service.inbox(&agent_008(), 10).unwrap().is_empty());
            assert_eq!(
                records.lock().unwrap().last().unwrap().decision,
                "audit_failed"
            );
        }
        // An Agent acting for nobody needs no such log.
        let root = tempfile::tempdir().unwrap();
        let (service, _) = assistant_relay(root.path(), None, 20);
        assert!(
            service
                .send(
                    &agent_008(),
                    request(target(CableScope::Agent, Some("010")), "k")
                )
                .is_ok()
        );
    }

    #[test]
    fn an_inbox_read_for_one_operation_leaves_the_others_undelivered() {
        let root = tempfile::tempdir().unwrap();
        let station = target(CableScope::Station, Some("station-a"));
        let service = relay(
            Arc::new(FileRegistry::open(root.path())),
            CablePolicy::try_new(
                ["007", "008"]
                    .into_iter()
                    .flat_map(|agent| {
                        [CableAction::Send, CableAction::Receive]
                            .map(|action| grant(agent, action, station.clone()))
                    })
                    .collect(),
                BTreeMap::new(),
            )
            .unwrap(),
            vec![endpoint("008", "operation-a", Some("session-008"))],
        );
        let sender = |operation: &str| {
            CablePrincipal::try_new(
                "007".into(),
                operation.into(),
                None,
                "station-a".into(),
                Some(format!("session-{operation}")),
                None,
            )
            .unwrap()
        };
        let to_008 = |key: &str| request(target(CableScope::Agent, Some("008")), key);
        let elsewhere = service.send(&sender("operation-b"), to_008("b1")).unwrap();
        let first = service.send(&sender("operation-a"), to_008("a1")).unwrap();
        service.send(&sender("operation-a"), to_008("a2")).unwrap();

        let read = service.inbox_from(&agent_008(), "operation-a", 1).unwrap();
        assert_eq!(read.len(), 1, "the limit counts this Operation's Cables");
        assert_eq!(read[0].cable_id, first.id());
        let untouched = service.registry.get_cable(elsewhere.id()).unwrap().unwrap();
        assert_eq!(
            untouched.recipients()[0].state(),
            CableState::Queued,
            "another Operation's Cable is not marked delivered"
        );
        assert_eq!(
            service
                .inbox_from(&agent_008(), "operation-a", 10)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn cross_station_unicast_requires_explicit_sender_and_receiver_grants() {
        let root = tempfile::tempdir().unwrap();
        let cross_agent = target(CableScope::Agent, Some("008"));
        let receiver_grant = grant(
            "008",
            CableAction::Receive,
            target(CableScope::Agent, Some("007")),
        );
        let denied = relay(
            Arc::new(FileRegistry::open(root.path())),
            CablePolicy::try_new(vec![receiver_grant.clone()], BTreeMap::new()).unwrap(),
            vec![endpoint_at(
                "008",
                "operation-b",
                "station-b",
                Some("session-008"),
            )],
        );
        assert_eq!(
            denied
                .send(
                    &principal("007", "operation-a"),
                    request(cross_agent.clone(), "denied")
                )
                .unwrap_err(),
            CableRelayError::Unavailable
        );

        let allowed = relay(
            Arc::new(FileRegistry::open(root.path())),
            CablePolicy::try_new(
                vec![
                    grant("007", CableAction::Send, cross_agent.clone()),
                    receiver_grant,
                ],
                BTreeMap::new(),
            )
            .unwrap(),
            vec![endpoint_at(
                "008",
                "operation-b",
                "station-b",
                Some("session-008"),
            )],
        );
        let sent = allowed
            .send(
                &principal("007", "operation-a"),
                request(cross_agent, "allowed"),
            )
            .unwrap();
        assert_eq!(sent.recipients()[0].session_id(), Some("session-008"));
    }

    #[test]
    fn station_broadcast_snapshots_deduplicated_authorized_audience_and_audits_metadata() {
        let root = tempfile::tempdir().unwrap();
        let operation = target(CableScope::Station, Some("station-a"));
        let policy = CablePolicy::try_new(
            vec![
                grant("007", CableAction::Send, operation.clone()),
                grant("008", CableAction::Receive, operation.clone()),
                grant("009", CableAction::Receive, operation.clone()),
                grant("010", CableAction::Receive, operation.clone()),
            ],
            BTreeMap::from([("009".into(), CableInboundMode::Hold)]),
        )
        .unwrap();
        let records = Arc::new(Mutex::new(Vec::new()));
        let audit_records = records.clone();
        let service = relay(
            Arc::new(FileRegistry::open(root.path())),
            policy,
            vec![
                endpoint("007", "operation-a", Some("session-007")),
                endpoint("008", "operation-a", Some("session-008")),
                endpoint("008", "operation-a", Some("session-008")),
                endpoint("009", "operation-a", None),
                endpoint("010", "operation-b", Some("session-010")),
            ],
        )
        .with_audit_sink(move |record| audit_records.lock().unwrap().push(record.clone()));

        let sent = service
            .send(
                &principal("007", "operation-a"),
                request(operation, "broadcast-1"),
            )
            .unwrap();
        assert_eq!(sent.recipients().len(), 3);
        assert_eq!(sent.recipients()[0].state(), CableState::Queued);
        assert_eq!(sent.recipients()[1].state(), CableState::Held);
        let audit = records.lock().unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].decision, "accepted");
        assert_eq!(audit[0].target_scope, CableScope::Station);
        assert!(!format!("{:?}", sent).contains("Review this change"));
    }

    #[test]
    fn a_quarantined_senders_cables_are_held_until_released() {
        use crate::directorates::registry::quarantine::Quarantine;

        let root = tempfile::tempdir().unwrap();
        let registry = Arc::new(FileRegistry::open(root.path()));
        let operation = target(CableScope::Station, Some("station-a"));
        let policy = CablePolicy::try_new(
            vec![
                grant("007", CableAction::Send, operation.clone()),
                grant("008", CableAction::Receive, operation.clone()),
                grant("009", CableAction::Release, operation.clone()),
            ],
            BTreeMap::new(),
        )
        .unwrap();
        let service = relay(
            registry.clone(),
            policy,
            vec![endpoint("008", "operation-a", Some("session-008"))],
        );
        let sender = principal("007", "operation-a");
        let before = service
            .send(&sender, request(operation.clone(), "before"))
            .unwrap();
        assert_eq!(before.recipients()[0].state(), CableState::Queued);

        registry
            .quarantine_agent(&Quarantine::new("007", None, "looping", "ops", 1.0))
            .unwrap();
        let held = service.send(&sender, request(operation, "after")).unwrap();
        assert_eq!(held.recipients()[0].state(), CableState::Held);
        assert!(
            service
                .inbox(&principal("008", "operation-a"), 10)
                .unwrap()
                .iter()
                .all(|delivery| delivery.cable_id != held.id()),
            "the receiver is not given a quarantined sender's Cable"
        );
        let released = service
            .release(&principal("009", "operation-a"), held.id())
            .unwrap();
        assert_eq!(released.delivered, 1);
    }

    #[test]
    fn held_cables_are_metadata_only_and_release_rechecks_recipient_grants() {
        let root = tempfile::tempdir().unwrap();
        let operation = target(CableScope::Station, Some("station-a"));
        let policy = CablePolicy::try_new(
            vec![
                grant("007", CableAction::Send, operation.clone()),
                grant("008", CableAction::Receive, operation.clone()),
                grant("009", CableAction::Release, operation.clone()),
            ],
            BTreeMap::from([("008".into(), CableInboundMode::Hold)]),
        )
        .unwrap();
        let service = relay(
            Arc::new(FileRegistry::open(root.path())),
            policy,
            vec![endpoint("008", "operation-a", Some("session-008"))],
        );
        let sender = principal("007", "operation-a");
        let approver = principal("009", "operation-a");
        let cable = service
            .send(&sender, request(operation, "approval-1"))
            .unwrap();

        assert!(service.held(&sender, 10).unwrap().is_empty());
        let held = service.held(&approver, 10).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].cable_id, cable.id());
        assert_eq!(held[0].recipients[0].agent, "008");
        assert!(!format!("{held:?}").contains("Review this change"));

        let released = service.release(&approver, cable.id()).unwrap();
        assert_eq!(released.delivered, 1);
        assert_eq!(released.refused, 0);
        let inbox = service.inbox(&principal("008", "operation-a"), 10).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].state, CableState::Delivered);
        assert_eq!(inbox[0].body, "Review this change");
        assert!(service.held(&approver, 10).unwrap().is_empty());
    }

    #[test]
    fn held_release_requires_a_grant_and_refuses_receivers_after_revocation() {
        let root = tempfile::tempdir().unwrap();
        let operation = target(CableScope::Station, Some("station-a"));
        let sender_grant = grant("007", CableAction::Send, operation.clone());
        let receiver_grant = grant("008", CableAction::Receive, operation.clone());
        let release_grant = grant("009", CableAction::Release, operation.clone());
        let service = relay(
            Arc::new(FileRegistry::open(root.path())),
            CablePolicy::try_new(
                vec![
                    sender_grant.clone(),
                    receiver_grant.clone(),
                    release_grant.clone(),
                ],
                BTreeMap::from([("008".into(), CableInboundMode::Hold)]),
            )
            .unwrap(),
            vec![endpoint("008", "operation-a", Some("session-008"))],
        );
        let cable = service
            .send(
                &principal("007", "operation-a"),
                request(operation.clone(), "approval-2"),
            )
            .unwrap();
        assert_eq!(
            service
                .release(&principal("010", "operation-a"), cable.id())
                .unwrap_err(),
            CableRelayError::Unavailable
        );

        service
            .update_policy(
                CablePolicy::try_new(vec![sender_grant, release_grant], BTreeMap::new()).unwrap(),
            )
            .unwrap();
        let released = service
            .release(&principal("009", "operation-a"), cable.id())
            .unwrap();
        assert_eq!(released.delivered, 0);
        assert_eq!(released.refused, 1);
        let stored = FileRegistry::open(root.path())
            .get_cable(cable.id())
            .unwrap()
            .unwrap();
        assert_eq!(stored.recipients()[0].state(), CableState::Refused);
    }

    #[test]
    fn discovery_is_station_limited_and_global_and_channel_sends_need_grants() {
        let root = tempfile::tempdir().unwrap();
        let operation = target(CableScope::Station, Some("station-a"));
        let policy = CablePolicy::try_new(
            vec![
                grant("007", CableAction::Discover, operation.clone()),
                grant("008", CableAction::Receive, operation.clone()),
                grant("009", CableAction::Receive, operation.clone()),
                grant("007", CableAction::Send, target(CableScope::Global, None)),
                grant(
                    "008",
                    CableAction::Receive,
                    target(CableScope::Global, None),
                ),
            ],
            BTreeMap::new(),
        )
        .unwrap();
        let service = relay(
            Arc::new(FileRegistry::open(root.path())),
            policy,
            vec![
                endpoint("008", "operation-a", Some("session-008")),
                endpoint("009", "operation-b", Some("session-009")),
                endpoint_at("010", "operation-c", "station-b", Some("session-010")),
            ],
        );
        let peers = service.discover(&principal("007", "operation-a")).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].agent(), "008");
        assert!(
            service
                .send(
                    &principal("007", "operation-a"),
                    request(target(CableScope::Global, None), "global-1")
                )
                .is_ok()
        );
        assert_eq!(
            service
                .send(
                    &principal("007", "operation-a"),
                    request(target(CableScope::Channel, Some("ops")), "channel-1")
                )
                .unwrap_err(),
            CableRelayError::Unavailable
        );
    }

    #[test]
    fn theater_and_channel_broadcasts_use_scoped_grants_and_subscribers() {
        let root = tempfile::tempdir().unwrap();
        let theater = target(CableScope::Theater, Some("theater-a"));
        let channel = target(CableScope::Channel, Some("ops"));
        let policy = CablePolicy::try_new(
            vec![
                grant("007", CableAction::Send, theater.clone()),
                grant("008", CableAction::Receive, theater.clone()),
                grant("007", CableAction::Send, channel.clone()),
                grant("008", CableAction::Receive, channel.clone()),
            ],
            BTreeMap::new(),
        )
        .unwrap();
        let service = CableRelay::new(
            Arc::new(FileRegistry::open(root.path())),
            policy,
            || {
                vec![
                    endpoint("007", "operation-a", Some("session-007")),
                    endpoint("008", "operation-b", Some("session-008")),
                    CableEndpoint::try_new(
                        "009".into(),
                        "operation-c".into(),
                        Some("theater-b".into()),
                        "station-a".into(),
                        Some("session-009".into()),
                        None,
                    )
                    .unwrap(),
                    CableEndpoint::try_new(
                        "010".into(),
                        "operation-d".into(),
                        Some("theater-b".into()),
                        "station-a".into(),
                        Some("session-010".into()),
                        None,
                    )
                    .unwrap(),
                ]
            },
            BTreeMap::from([(
                "ops".into(),
                BTreeSet::from(["007".into(), "008".into(), "010".into()]),
            )]),
        )
        .with_clock(|| 1_700_000_000.0);
        let sender = principal("007", "operation-a");

        let theater_cable = service
            .send(&sender, request(theater.clone(), "theater-1"))
            .unwrap();
        assert_eq!(theater_cable.target(), &theater);
        assert_eq!(
            theater_cable
                .recipients()
                .iter()
                .map(CableRecipient::agent)
                .collect::<Vec<_>>(),
            ["008"]
        );

        let channel_cable = service
            .send(&sender, request(channel.clone(), "channel-1"))
            .unwrap();
        assert_eq!(channel_cable.target(), &channel);
        assert_eq!(
            channel_cable
                .recipients()
                .iter()
                .map(CableRecipient::agent)
                .collect::<Vec<_>>(),
            ["008"]
        );

        assert_eq!(
            service
                .send(
                    &sender,
                    request(target(CableScope::Channel, Some("private")), "channel-2"),
                )
                .unwrap_err(),
            CableRelayError::Unavailable
        );
    }

    #[test]
    fn inbox_acknowledgement_and_reply_use_bound_recipient_identity() {
        let root = tempfile::tempdir().unwrap();
        let operation = target(CableScope::Station, Some("station-a"));
        let policy = CablePolicy::try_new(
            vec![
                grant("007", CableAction::Send, operation.clone()),
                grant("008", CableAction::Receive, operation.clone()),
                grant("008", CableAction::Send, operation.clone()),
                grant("007", CableAction::Receive, operation),
            ],
            BTreeMap::new(),
        )
        .unwrap();
        let service = relay(
            Arc::new(FileRegistry::open(root.path())),
            policy,
            vec![
                endpoint("007", "operation-a", Some("session-007")),
                endpoint("008", "operation-a", Some("session-008")),
            ],
        );
        let sender = principal("007", "operation-a");
        let receiver = principal("008", "operation-a");
        let original = service
            .send(
                &sender,
                request(target(CableScope::Agent, Some("008")), "message-1"),
            )
            .unwrap();

        let inbox = service.inbox(&receiver, 10).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].cable_id, original.id());
        assert_eq!(inbox[0].body, "Review this change");
        assert_eq!(inbox[0].state, CableState::Delivered);
        assert!(!format!("{:?}", inbox[0]).contains("Review this change"));

        service.acknowledge(&receiver, original.id()).unwrap();
        service.acknowledge(&receiver, original.id()).unwrap();
        assert!(service.inbox(&receiver, 10).unwrap().is_empty());

        let reply = service
            .reply(
                &receiver,
                original.id(),
                "Looks good".into(),
                "reply-1".into(),
            )
            .unwrap();
        assert_eq!(reply.target().scope(), CableScope::Agent);
        assert_eq!(reply.reply_to(), Some(original.id()));
        assert_eq!(reply.conversation_id(), Some(original.id()));
        let sender_inbox = service.inbox(&sender, 10).unwrap();
        assert_eq!(sender_inbox.len(), 1);
        assert_eq!(sender_inbox[0].body, "Looks good");
    }

    #[test]
    fn receiver_grant_revocation_is_checked_on_first_delivery_attempt() {
        let root = tempfile::tempdir().unwrap();
        let operation = target(CableScope::Station, Some("station-a"));
        let sender_grant = grant("007", CableAction::Send, operation.clone());
        let receiver_grant = grant("008", CableAction::Receive, operation.clone());
        let service = relay(
            Arc::new(FileRegistry::open(root.path())),
            CablePolicy::try_new(vec![sender_grant.clone(), receiver_grant], BTreeMap::new())
                .unwrap(),
            vec![endpoint("008", "operation-a", Some("session-008"))],
        );
        let sent = service
            .send(
                &principal("007", "operation-a"),
                request(operation, "revoke-1"),
            )
            .unwrap();

        service
            .update_policy(CablePolicy::try_new(vec![sender_grant], BTreeMap::new()).unwrap())
            .unwrap();
        assert!(
            service
                .inbox(&principal("008", "operation-a"), 10)
                .unwrap()
                .is_empty()
        );
        let stored = FileRegistry::open(root.path())
            .get_cable(sent.id())
            .unwrap()
            .unwrap();
        assert_eq!(stored.recipients()[0].state(), CableState::Refused);
    }

    #[test]
    fn cable_config_parses_exact_grants_and_rejects_unknown_fields() {
        let config = CableConfig::parse_toml(
            r#"
max_fanout = 40

[[grants]]
agent = "007"
action = "send"
scope = "station"
name = "operation-a"

[inbound]
"008" = "hold"

[[grants]]
agent = "009"
action = "release"
scope = "station"
name = "operation-a"

[channels]
ops = ["007", "008"]
"#,
        )
        .unwrap();
        assert_eq!(config.max_fanout(), 40);
        assert_eq!(config.channels()["ops"].len(), 2);
        assert!(config.policy().is_ok());
        assert!(CableConfig::parse_toml("unexpected = true").is_err());
        assert!(CableConfig::parse_toml("max_fanout = 0").is_err());
    }
}
