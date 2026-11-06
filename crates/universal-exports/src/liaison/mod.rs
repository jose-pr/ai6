//! Liaison: A2A 1.0 at AI6's boundary, as the wire format of Cables.
//!
//! A peer's A2A message becomes a Cable to one AI6 Agent or an Operation's role, submitted to the
//! Cable relay as the peer's configured principal: the same records, grants, compartments, quotas,
//! TTL, deduplication and audit as any Cable. The addressee's reply is a Cable reply, read back as
//! the A2A answer. Nothing else is kept: an A2A Task is the peer's Cable (its ID is the Cable's),
//! its context the Cable conversation, and its state the Cable's delivery. Nothing is placed or
//! scheduled. A thread stays with the Session that first took it ([`CableSendRequest::with_affinity`]).
//!
//! A peer arrives on a proven coqui-agentic-peer link ([`LinkListener`]) or on the gateway's
//! HTTP JSON-RPC binding (`/v1/a2a`), signed in as its principal. Cables to `a2a:<peer>` leave as
//! A2A the other way.

mod http;
mod link;
mod outbound;
#[cfg(test)]
mod tests;

pub(crate) use http::routes;
pub use link::LinkListener;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ai6::directorates::registry::cables::{
    Cable, CableAuditRecord, CableDelivery, CableEndpoint, CablePrincipal, CableRelay,
    CableRelayError, CableScope, CableSendRequest, CableState, CableTarget,
};
pub use ai6::directorates::registry::cables::{LIAISON_PREFIX, LIAISON_STATION};
use ai6::directorates::registry::store::AuditRecord;
use ai6::{Authorizer, Limit, Person, RegistryBackend};
use coqui_a2a::error::{A2aError, ErrorKind};
use coqui_a2a::model::{
    AgentCapabilities, AgentCard, AgentSkill, Artifact, CancelTaskRequest, GetTaskRequest,
    ListTasksRequest, ListTasksResponse, Message, Part, Role, SendMessageRequest,
    SendMessageResponse, StreamResponse, SubscribeToTaskRequest, Task, TaskArtifactUpdateEvent,
    TaskState, TaskStatus, TaskStatusUpdateEvent,
};
use coqui_a2a::{Agent, Caller, Events};
use serde_json::Value;

use crate::Gateway;

/// Where an addressee's Cable is said to wait: in the Registry, for any Station of its Agent.
const ADDRESSEE_STATION: &str = "registry";

/// The message metadata member that names an addressee.
pub const ADDRESSEE_MEMBER: &str = "addressee";

/// Why Liaison could not be set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiaisonError(String);

impl LiaisonError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for LiaisonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LiaisonError {}

/// What a peer may write to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Addressee {
    /// One Agent of one Operation: `agent:<operation>/<agent>`.
    Agent {
        /// The Operation.
        operation: String,
        /// The Agent's codename.
        agent: String,
    },
    /// The Agent the roster selects for a role in one Operation: `role:<operation>/<role>`.
    Role {
        /// The Operation.
        operation: String,
        /// The role.
        role: String,
    },
}

impl Addressee {
    /// Its Operation.
    pub fn operation(&self) -> &str {
        match self {
            Self::Agent { operation, .. } | Self::Role { operation, .. } => operation,
        }
    }
}

impl fmt::Display for Addressee {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent { operation, agent } => write!(f, "agent:{operation}/{agent}"),
            Self::Role { operation, role } => write!(f, "role:{operation}/{role}"),
        }
    }
}

impl FromStr for Addressee {
    type Err = LiaisonError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let refused = || {
            LiaisonError::new(format!(
                "{text:?} is not agent:<operation>/<agent> or role:<operation>/<role>"
            ))
        };
        let (kind, rest) = text.split_once(':').ok_or_else(refused)?;
        let (operation, name) = rest.split_once('/').ok_or_else(refused)?;
        let valid = ai6::directorates::section_00::assistant::valid_identity;
        if !valid(operation) || !valid(name) {
            return Err(refused());
        }
        let (operation, name) = (operation.to_owned(), name.to_owned());
        match kind {
            "agent" => Ok(Self::Agent {
                operation,
                agent: name,
            }),
            "role" => Ok(Self::Role {
                operation,
                role: name,
            }),
            _ => Err(refused()),
        }
    }
}

/// One paired peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiaisonPeer {
    /// Its configured name.
    pub name: String,
    /// Its peer id, `sha256:<hex>` of its link key; `None` for a peer that only signs in over
    /// HTTP.
    pub id: Option<String>,
    /// The subject it acts as (`person:<id>`, `account:<name>`, `group:<name>`); over HTTP, the
    /// person signed in as this subject is this peer.
    pub principal: String,
    /// What it may write to.
    pub addressees: Vec<Addressee>,
    /// Whether its link grants it `a2a/1`.
    pub a2a: bool,
    /// Where Liaison dials it to carry Cables sent to it, when it holds no link open itself.
    pub url: Option<url::Url>,
}

impl LiaisonPeer {
    /// The sender its Cables carry: `a2a:<name>`.
    pub fn sender(&self) -> String {
        format!("{LIAISON_PREFIX}{}", self.name)
    }

    /// Whether `person` is this peer's principal.
    fn is(&self, person: &Person) -> bool {
        let Ok(subject) = self.principal.parse::<ai6::Subject>() else {
            return false;
        };
        match subject.kind() {
            "person" => person.id == subject.value(),
            "account" => person.name == subject.value(),
            "group" => person.groups.iter().any(|group| group == subject.value()),
            _ => false,
        }
    }
}

/// What Liaison serves and to whom.
#[derive(Debug, Clone)]
pub struct LiaisonConfig {
    /// The paired peers, by name.
    pub peers: BTreeMap<String, LiaisonPeer>,
    /// How often a waiting operation looks for a reply, and the outbound side for Cables.
    pub poll: Duration,
    /// Links served at once.
    pub max_links: Limit,
    /// Whether a `ws://` listener or dial may go beyond loopback.
    pub plain_beyond_loopback: bool,
    /// How often a running stream's grant is asked about again.
    pub recheck: Duration,
}

impl Default for LiaisonConfig {
    fn default() -> Self {
        Self {
            peers: BTreeMap::new(),
            poll: Duration::from_secs(1),
            max_links: Limit::at(64),
            plain_beyond_loopback: false,
            recheck: Duration::from_secs(1),
        }
    }
}

impl LiaisonConfig {
    /// Adds a peer.
    #[must_use]
    pub fn with_peer(mut self, peer: LiaisonPeer) -> Self {
        self.peers.insert(peer.name.clone(), peer);
        self
    }

    /// Reads `[unex.liaison]`.
    ///
    /// # Errors
    ///
    /// A peer's name, id, principal, addressee or url is not valid, or a duration is negative.
    pub fn from_settings(settings: &ai6::bootstrap::LiaisonSettings) -> Result<Self, LiaisonError> {
        let mut config = Self::default();
        if let Some(poll) = settings.poll {
            config.poll = Duration::try_from_secs_f64(poll).map_err(|_| {
                LiaisonError::new("[unex.liaison] poll must be seconds, not negative")
            })?;
        }
        if let Some(most) = settings.max_links {
            config.max_links = Limit::at(most);
        }
        config.plain_beyond_loopback = settings.plain_beyond_loopback.unwrap_or(false);
        for (name, peer) in &settings.peer {
            let at = |what: &str| LiaisonError::new(format!("[unex.liaison.peer.{name}] {what}"));
            if !ai6::directorates::section_00::assistant::valid_identity(name) {
                return Err(at("the name must be letters, digits, - and _"));
            }
            if peer
                .id
                .as_ref()
                .is_some_and(|id| !id.starts_with("sha256:"))
            {
                return Err(at("id must be the peer's sha256:<hex>"));
            }
            let principal = peer
                .principal
                .clone()
                .filter(|principal| principal.parse::<ai6::Subject>().is_ok())
                .ok_or_else(|| {
                    at("principal must be person:<id>, account:<name> or group:<name>")
                })?;
            let addressees = peer
                .addressees
                .iter()
                .map(|addressee| {
                    addressee
                        .parse()
                        .map_err(|error| at(&format!("addressees: {error}")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let url = peer
                .url
                .as_deref()
                .map(|url| {
                    url::Url::parse(url)
                        .ok()
                        .filter(|url| matches!(url.scheme(), "ws" | "wss"))
                        .ok_or_else(|| at("url must be ws:// or wss://"))
                })
                .transpose()?;
            if url.is_some() && peer.id.is_none() {
                return Err(at(
                    "a peer Liaison dials needs its id, which its proof must show",
                ));
            }
            config = config.with_peer(LiaisonPeer {
                name: name.clone(),
                id: peer.id.clone(),
                principal,
                addressees,
                a2a: peer.a2a.unwrap_or(true),
                url,
            });
        }
        Ok(config)
    }
}

/// Who is calling: a configured peer, and what proved it (its key on a link, its sign-in over
/// HTTP). The proof goes on its Cables.
#[derive(Debug, Clone)]
pub(crate) struct Calling<'a> {
    pub(crate) peer: &'a LiaisonPeer,
    pub(crate) proof: String,
}

/// Liaison's A2A agent: what a peer reaches, over a link or over HTTP. It keeps nothing of its
/// own: every answer is read from the Registry.
#[derive(Clone)]
pub struct Liaison {
    inner: Arc<Inner>,
}

struct Inner {
    gateway: Gateway,
    config: Arc<LiaisonConfig>,
}

impl fmt::Debug for Liaison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Liaison")
            .field("peers", &self.inner.config.peers.len())
            .finish_non_exhaustive()
    }
}

/// A Task as the Registry holds it: the peer's Cable and the replies to it.
struct Exchange {
    cable: Cable,
    replies: Vec<CableDelivery>,
    state: TaskState,
    reason: Option<&'static str>,
}

/// Something an operation asks, between waits, whether its caller is still there.
pub(crate) type Open = Arc<dyn Fn() -> bool + Send + Sync>;

impl Liaison {
    /// Serves `config`'s peers through `gateway`: its Registry, configuration provider, Directory
    /// and authorizer. Without them every message is refused.
    pub fn new(gateway: Gateway, config: LiaisonConfig) -> Self {
        Self::shared(gateway, Arc::new(config))
    }

    pub(crate) fn shared(gateway: Gateway, config: Arc<LiaisonConfig>) -> Self {
        Self {
            inner: Arc::new(Inner { gateway, config }),
        }
    }

    /// The peer the link proved, by its key; a key nobody configured never gets this far.
    fn peer_by_id(&self, id: &str) -> Option<&LiaisonPeer> {
        self.inner
            .config
            .peers
            .values()
            .find(|peer| peer.id.as_deref() == Some(id))
    }

    /// The peer whose principal `person` is, for a caller signed in over HTTP.
    pub(crate) fn peer_for(&self, person: &Person) -> Option<&LiaisonPeer> {
        self.inner
            .config
            .peers
            .values()
            .find(|peer| peer.is(person))
    }

    fn calling<'a>(&'a self, caller: &Caller<'_>) -> Result<Calling<'a>, A2aError> {
        let peer = self
            .peer_by_id(caller.peer)
            .ok_or_else(|| A2aError::standard(ErrorKind::PermissionDenied))?;
        Ok(Calling {
            peer,
            proof: caller.peer.to_owned(),
        })
    }

    fn registry(&self) -> Result<Arc<dyn RegistryBackend>, A2aError> {
        self.inner
            .gateway
            .registry
            .clone()
            .ok_or_else(|| unavailable("the Registry"))
    }

    fn snapshot(&self) -> Result<ai6::ConfigSnapshot, A2aError> {
        self.inner
            .gateway
            .roster
            .as_ref()
            .ok_or_else(|| unavailable("the configuration provider"))?
            .load("")
            .map_err(|_| unavailable("the configuration provider"))
    }

    /// The person `peer` acts as: the Directory's when it knows the subject, else the subject
    /// alone, which access rules name as they would anyone's.
    fn person(&self, peer: &LiaisonPeer) -> Option<Person> {
        let access = self.inner.gateway.access.as_ref()?;
        let subject: ai6::Subject = peer.principal.parse().ok()?;
        let lookup = match subject.kind() {
            "account" => Some(("name", subject.value())),
            "person" => Some(("id", subject.value())),
            _ => None,
        };
        let found = lookup.and_then(|(by, value)| {
            access
                .directory
                .lookup(ai6::Lookup {
                    by: by.to_owned(),
                    value: value.to_owned(),
                })
                .ok()
                .flatten()
        });
        Some(found.unwrap_or_else(|| match subject.kind() {
            "account" => Person {
                name: subject.value().to_owned(),
                ..Person::default()
            },
            "person" => Person {
                id: subject.value().to_owned(),
                ..Person::default()
            },
            _ => Person {
                groups: vec![subject.value().to_owned()],
                ..Person::default()
            },
        }))
    }

    /// Whether `peer`'s principal holds `action` on `operation` now: no, whenever that cannot be
    /// read.
    fn allowed(&self, peer: &LiaisonPeer, action: &str, operation: &str) -> bool {
        let (Some(person), Some(registry), Ok(authorizer)) = (
            self.person(peer),
            self.inner.gateway.registry.clone(),
            self.inner.gateway.authorizer(),
        ) else {
            return false;
        };
        let Ok(records) = registry.stations(Duration::MAX) else {
            return false;
        };
        let tree = crate::tree::OperationTree::from_check_ins(&records);
        authorizer
            .authorize(crate::tree::operation_request(
                person, action, operation, &tree,
            ))
            .is_ok_and(|decision| decision.allowed)
    }

    /// The Operations the Stations serve.
    fn operations(&self) -> BTreeSet<String> {
        let Ok(registry) = self.registry() else {
            return BTreeSet::new();
        };
        registry
            .stations(Duration::MAX)
            .unwrap_or_default()
            .iter()
            .filter_map(|station| station.info().get("operations")?.as_array().cloned())
            .flatten()
            .filter_map(|operation| operation.as_str().map(str::to_owned))
            .collect()
    }

    /// The Agent an addressee is now.
    fn resolve(roster: &ai6::Roster, addressee: &Addressee) -> Option<String> {
        match addressee {
            Addressee::Agent { operation, agent } => roster
                .agents_for_operation(operation)
                .ok()?
                .into_iter()
                .find(|candidate| &candidate.id == agent)
                .map(|candidate| candidate.id),
            Addressee::Role { operation, role } => {
                let mut query = ai6::AgentQuery::new()
                    .operation(Some(operation))
                    .role(Some(role));
                query.now = i64::try_from(now() as u64).ok();
                roster.select_agent(&query).ok().map(|agent| agent.id)
            }
        }
    }

    /// The Cable relay over the shared Registry, as the `cables` record sets it, reaching
    /// `endpoints`.
    fn relay(
        &self,
        snapshot: &ai6::ConfigSnapshot,
        endpoints: Vec<CableEndpoint>,
    ) -> Result<CableRelay, A2aError> {
        let registry = self.registry()?;
        let cables = &snapshot.settings.cables;
        let policy = cables
            .policy()
            .map_err(|_| unavailable("the cables record"))?;
        let (sender_rate, sender_burst, scope_rate, scope_burst) = cables.quota_limits();
        let roster = snapshot.roster.clone();
        let audit = registry.clone();
        CableRelay::new(
            registry,
            policy,
            move || endpoints.clone(),
            cables.channels().clone(),
        )
        .with_lineage(move |operation| {
            roster
                .lineage(operation)
                .into_iter()
                .map(str::to_owned)
                .collect()
        })
        .with_durable_audit(move |record| write_cable_audit(audit.as_ref(), record))
        .with_cross_operation(cables.cross_operation())
        .with_limits(
            cables.max_fanout(),
            sender_rate,
            sender_burst,
            scope_rate,
            scope_burst,
        )
        .and_then(|relay| relay.with_message_limits(cables.message_limits()))
        .map_err(|_| unavailable("the cables record"))
    }

    /// The principal a peer's Cables go as: its sender, acting for its principal, stamped with
    /// `proof`, with `route` as its Session when replies are to come back to one.
    fn principal_for(
        peer: &LiaisonPeer,
        proof: &str,
        operation: &str,
        route: Option<&str>,
    ) -> Result<CablePrincipal, A2aError> {
        CablePrincipal::try_new(
            peer.sender(),
            operation.to_owned(),
            None,
            LIAISON_STATION.to_owned(),
            route.map(str::to_owned),
            None,
        )
        .and_then(|principal| principal.with_on_behalf_of(peer.principal.clone()))
        .and_then(|principal| principal.with_peer(proof.to_owned()))
        .map_err(|_| A2aError::standard(ErrorKind::Internal))
    }

    /// Writes the audit record of one A2A call: the principal as actor, the peer and what it
    /// asked as target. The relay writes its own record of each Cable before storing it; this
    /// one is best effort.
    fn audit(&self, peer: &LiaisonPeer, call: &str, operation: &str, target: &str) {
        let Some(registry) = self.inner.gateway.registry.clone() else {
            return;
        };
        let mut target = format!("peer:{} {target}", peer.name);
        cut(&mut target, 500);
        let record = AuditRecord {
            id: format!("liaison-{}", uuid::Uuid::new_v4().simple()),
            actor: peer.principal.clone(),
            action: format!("liaison.{call}"),
            operation: operation.to_owned(),
            resource: operation.is_empty().then(|| "config:liaison".to_owned()),
            target,
            at: now(),
        };
        if let Err(error) = registry.record_audit(&record) {
            eprintln!("universal-exports: Liaison's audit record of a {call} was lost: {error}");
        }
    }

    /// The peer's own Cables: its messages, newest first, never the replies it carried back.
    fn cables_of(&self, peer: &LiaisonPeer) -> Result<Vec<Cable>, A2aError> {
        let registry = self.registry()?;
        let page = registry.limits().cables.page.or_max();
        let mut cables: Vec<Cable> = registry
            .cables_from(&peer.sender(), page, None)
            .map_err(|_| unavailable("the Registry"))?
            .into_iter()
            .filter(|cable| cable.reply_to().is_none() && cable.peer().is_some())
            .collect();
        cables.reverse();
        Ok(cables)
    }

    /// Takes a peer's message: who it is for, that the principal may send there, and the Session
    /// its context is bound to; then submits it to the relay as a Cable.
    pub(crate) fn submit(
        &self,
        calling: &Calling<'_>,
        request: &SendMessageRequest,
    ) -> Result<Cable, A2aError> {
        let peer = calling.peer;
        request.validate()?;
        let message = &request.message;
        if let Some(task) = &message.task_id {
            return Err(match self.exchange(peer, task) {
                Ok(_) => A2aError::new(
                    ErrorKind::UnsupportedOperation,
                    "a task is one message and its replies: send a new message in its context",
                ),
                Err(error) => error,
            });
        }
        // A message ID sent again is the Cable it made, as A2A asks of a resend.
        let key = idempotency_key(&peer.name, &message.message_id);
        if let Some(sent) = self
            .registry()?
            .find_cable(&peer.sender(), &key)
            .map_err(|_| unavailable("the Registry"))?
        {
            return Ok(sent);
        }
        let text = message.joined_text();
        if text.trim().is_empty() || message.parts.iter().any(|part| part.text.is_none()) {
            return Err(A2aError::new(
                ErrorKind::ContentTypeNotSupported,
                "Liaison carries text parts only",
            ));
        }
        // The context's first Cable decides its addressee, and the Session it is bound to.
        let earlier = match &message.context_id {
            Some(context) => Some(
                self.cables_of(peer)?
                    .into_iter()
                    .rev()
                    .find(|cable| cable.conversation_id() == Some(context.as_str()))
                    .ok_or_else(|| A2aError::new(ErrorKind::InvalidParams, "unknown contextId"))?,
            ),
            None => None,
        };
        let named = [message.metadata.as_ref(), request.metadata.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|metadata| metadata.get(ADDRESSEE_MEMBER)?.as_str().map(str::to_owned));
        let addressee: Addressee = match (named, &earlier, peer.addressees.as_slice()) {
            (Some(named), _, _) => named.parse().map_err(|_| {
                A2aError::new(
                    ErrorKind::InvalidParams,
                    "the addressee is not one of the card's skills",
                )
            })?,
            (None, Some(earlier), _) => Addressee::Agent {
                operation: earlier.sender_operation().to_owned(),
                agent: earlier.target().name().unwrap_or_default().to_owned(),
            },
            (None, None, [only]) => only.clone(),
            (None, None, _) => {
                return Err(A2aError::new(
                    ErrorKind::InvalidParams,
                    "name the addressee in the message's metadata.addressee: one of the card's \
                     skills",
                ));
            }
        };
        let operation = addressee.operation().to_owned();
        let denied = || {
            A2aError::new(
                ErrorKind::PermissionDenied,
                "that addressee is not reachable, or not for this peer",
            )
        };
        let listed = peer.addressees.contains(&addressee)
            || earlier.as_ref().is_some_and(|earlier| {
                peer.addressees
                    .iter()
                    .any(|listed| listed.operation() == earlier.sender_operation())
            });
        if !listed || !self.allowed(peer, "cable.send", &operation) {
            self.audit(
                peer,
                "send_message",
                &operation,
                &format!("{addressee} refused"),
            );
            return Err(denied());
        }
        let snapshot = self.snapshot()?;
        let agent = match &earlier {
            Some(earlier) => earlier.target().name().map(str::to_owned),
            None => Self::resolve(&snapshot.roster, &addressee),
        };
        let Some(agent) = agent else {
            self.audit(
                peer,
                "send_message",
                &operation,
                &format!("{addressee} refused"),
            );
            return Err(denied());
        };
        let context = earlier
            .as_ref()
            .and_then(|earlier| earlier.conversation_id().map(str::to_owned))
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let affinity = earlier
            .as_ref()
            .and_then(|earlier| self.bound_session(earlier));
        let endpoint = CableEndpoint::try_new(
            agent.clone(),
            operation.clone(),
            None,
            ADDRESSEE_STATION.to_owned(),
            None,
            None,
        )
        .map_err(|_| denied())?;
        let relay = self.relay(&snapshot, vec![endpoint])?;
        let principal =
            Self::principal_for(peer, &calling.proof, &operation, Some(&route(&context)))?;
        let target =
            CableTarget::try_new(CableScope::Agent, Some(agent.clone())).map_err(|_| denied())?;
        let mut cable = CableSendRequest::new(target, text, key).with_conversation(context);
        if let Some(session) = affinity {
            cable = cable.with_affinity(session);
        }
        let cable = relay.send(&principal, cable).map_err(|error| {
            self.audit(
                peer,
                "send_message",
                &operation,
                &format!("{addressee} refused"),
            );
            match error {
                CableRelayError::InvalidRequest => A2aError::new(
                    ErrorKind::InvalidParams,
                    "the message is not one a Cable carries",
                ),
                CableRelayError::RegistryUnavailable => unavailable("the Cable relay"),
                _ => denied(),
            }
        })?;
        self.audit(
            peer,
            "send_message",
            &operation,
            &format!("agent:{operation}/{agent} cable {}", cable.id()),
        );
        Ok(cable)
    }

    /// The provider Session a context is bound to: the one that took its first Cable, else the
    /// one that first replied in it.
    fn bound_session(&self, first: &Cable) -> Option<String> {
        if let Some(session) = first.affinity() {
            return Some(session.to_owned());
        }
        let current = self.registry().ok()?.get_cable(first.id()).ok()??;
        current
            .recipients()
            .iter()
            .find_map(|recipient| recipient.taken_by().map(str::to_owned))
    }

    /// The Task `id` as the Registry holds it now, when it is `peer`'s. Replies are read from the
    /// relay as delivered to the thread's route, and stay there until their Cable expires.
    fn exchange(&self, peer: &LiaisonPeer, id: &str) -> Result<Exchange, A2aError> {
        let registry = self.registry()?;
        let cable = registry
            .get_cable(id)
            .map_err(|_| unavailable("the Registry"))?
            .filter(|cable| {
                cable.sender_agent() == peer.sender()
                    && cable.peer().is_some()
                    && cable.reply_to().is_none()
            })
            .ok_or_else(|| A2aError::task_not_found(id))?;
        let snapshot = self.snapshot()?;
        let relay = self.relay(&snapshot, Vec::new())?;
        let context = cable.conversation_id().unwrap_or_default().to_owned();
        let principal = Self::principal_for(
            peer,
            cable.peer().unwrap_or_default(),
            cable.sender_operation(),
            Some(&route(&context)),
        )?;
        let page = registry.limits().cables.page.or_max();
        let replies: Vec<CableDelivery> = relay
            .inbox(&principal, page)
            .map_err(|_| unavailable("the Cable relay"))?
            .into_iter()
            .filter(|delivery| delivery.reply_to.as_deref() == Some(cable.id()))
            .collect();
        let delivered = cable
            .recipients()
            .first()
            .map(ai6::directorates::registry::cables::CableRecipient::state);
        let expired = cable.expires_at() <= now();
        let (state, reason) = if !replies.is_empty() {
            (TaskState::Completed, None)
        } else {
            match delivered {
                None => (TaskState::Failed, Some("it reached nobody")),
                Some(CableState::Queued | CableState::Held) if expired => (
                    TaskState::Failed,
                    Some("it expired before the addressee had it"),
                ),
                Some(CableState::Queued | CableState::Held) => (TaskState::Submitted, None),
                Some(CableState::Delivered | CableState::Acked) if expired => {
                    (TaskState::Failed, Some("no reply came before it expired"))
                }
                Some(CableState::Delivered | CableState::Acked) => (TaskState::Working, None),
                Some(CableState::Refused) => {
                    (TaskState::Rejected, Some("the addressee does not take it"))
                }
                Some(CableState::Withdrawn) => (TaskState::Canceled, Some("withdrawn")),
                Some(_) => (TaskState::Failed, Some("it was not delivered")),
            }
        };
        Ok(Exchange {
            cable,
            replies,
            state,
            reason,
        })
    }

    /// Waits until the Task ends, or the caller goes.
    fn settle(&self, peer: &LiaisonPeer, id: &str, open: &Open) -> Result<Exchange, A2aError> {
        loop {
            let exchange = self.exchange(peer, id)?;
            if exchange.state.is_terminal() || !open() {
                return Ok(exchange);
            }
            pause(self.inner.config.poll, open);
        }
    }

    /// The card `peer` sees: one skill for each addressee its principal may send to now.
    pub(crate) fn card(&self, peer: Option<&LiaisonPeer>) -> AgentCard {
        let skills = peer
            .map(|peer| {
                peer.addressees
                    .iter()
                    .filter(|addressee| self.allowed(peer, "cable.send", addressee.operation()))
                    .map(|addressee| AgentSkill {
                        id: addressee.to_string(),
                        name: addressee.to_string(),
                        description: match addressee {
                            Addressee::Agent { operation, agent } => format!(
                                "A message to Agent {agent} of Operation {operation}, as a Cable; \
                                 its reply is the answer"
                            ),
                            Addressee::Role { operation, role } => format!(
                                "A message to the Agent of role {role} in Operation {operation}, \
                                 as a Cable; its reply is the answer"
                            ),
                        },
                        tags: vec!["cable".to_owned()],
                        ..AgentSkill::default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        AgentCard {
            name: "AI6 Liaison".to_owned(),
            description: format!(
                "Messages to AI6 Agents, carried as Cables. Name the addressee in a message's \
                 metadata.{ADDRESSEE_MEMBER}, one of the skills; a peer with one skill may leave \
                 it out, and a message in a known context goes where that context went. A task \
                 is one message and its replies."
            ),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities: AgentCapabilities {
                streaming: Some(true),
                push_notifications: Some(false),
                extended_agent_card: Some(false),
                ..AgentCapabilities::default()
            },
            default_input_modes: vec!["text/plain".to_owned()],
            default_output_modes: vec!["text/plain".to_owned()],
            skills,
            ..AgentCard::default()
        }
    }

    /// The Task as A2A shows it.
    fn task(exchange: &Exchange, history_length: Option<u32>, artifacts: bool) -> Task {
        let cable = &exchange.cable;
        let context = cable.conversation_id().unwrap_or_default().to_owned();
        let replies: Vec<Message> = exchange
            .replies
            .iter()
            .map(|reply| reply_message(reply, &context, cable.id()))
            .collect();
        let mut history = vec![Message {
            message_id: cable.id().to_owned(),
            context_id: Some(context.clone()),
            task_id: Some(cable.id().to_owned()),
            role: Role::User,
            parts: vec![Part::text(cable.body().to_owned())],
            ..Message::default()
        }];
        history.extend(replies.iter().cloned());
        if let Some(length) = history_length {
            let keep = usize::try_from(length).unwrap_or(usize::MAX);
            let skip = history.len().saturating_sub(keep);
            history.drain(..skip);
        }
        let status_message = replies.last().cloned().or_else(|| {
            exchange.reason.map(|reason| Message {
                message_id: format!("{}-status", cable.id()),
                context_id: Some(context.clone()),
                task_id: Some(cable.id().to_owned()),
                role: Role::Agent,
                parts: vec![Part::text(reason)],
                ..Message::default()
            })
        });
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            ADDRESSEE_MEMBER.to_owned(),
            Value::String(format!(
                "agent:{}/{}",
                cable.sender_operation(),
                cable.target().name().unwrap_or_default()
            )),
        );
        Task {
            id: cable.id().to_owned(),
            context_id: Some(context),
            status: TaskStatus {
                state: exchange.state.clone(),
                message: status_message,
                timestamp: Some(timestamp(
                    exchange
                        .replies
                        .last()
                        .map_or(cable.created_at(), |reply| reply.created_at),
                )),
                ..TaskStatus::default()
            },
            artifacts: (artifacts && !replies.is_empty())
                .then(|| replies.iter().map(reply_artifact).collect()),
            history: Some(history),
            metadata: Some(metadata),
            ..Task::default()
        }
    }

    /// The items that follow a Task from where it is now.
    fn follow(&self, peer: &LiaisonPeer, exchange: &Exchange, open: Open) -> Events {
        Box::new(Follow {
            liaison: self.clone(),
            peer: peer.clone(),
            id: exchange.cable.id().to_owned(),
            context: exchange
                .cable
                .conversation_id()
                .unwrap_or_default()
                .to_owned(),
            state: exchange.state.clone(),
            replies: exchange.replies.len(),
            pending: vec![StreamResponse::Task(Self::task(exchange, None, true))],
            open,
            done: false,
        })
    }

    pub(crate) fn send_message(
        &self,
        calling: &Calling<'_>,
        request: &SendMessageRequest,
        open: &Open,
    ) -> Result<SendMessageResponse, A2aError> {
        let cable = self.submit(calling, request)?;
        let configuration = request.configuration.as_ref();
        let immediate = configuration
            .and_then(|configuration| configuration.return_immediately)
            .unwrap_or(false);
        let history = configuration.and_then(|configuration| configuration.history_length);
        let exchange = if immediate {
            self.exchange(calling.peer, cable.id())?
        } else {
            self.settle(calling.peer, cable.id(), open)?
        };
        let context = cable.conversation_id().unwrap_or_default();
        match (&exchange.state, exchange.replies.last()) {
            (TaskState::Completed, Some(reply)) if !immediate => Ok(SendMessageResponse::Message(
                reply_message(reply, context, cable.id()),
            )),
            _ => Ok(SendMessageResponse::Task(Self::task(
                &exchange, history, true,
            ))),
        }
    }

    pub(crate) fn send_streaming(
        &self,
        calling: &Calling<'_>,
        request: &SendMessageRequest,
        open: Open,
    ) -> Result<Events, A2aError> {
        let cable = self.submit(calling, request)?;
        let exchange = self.exchange(calling.peer, cable.id())?;
        Ok(self.follow(calling.peer, &exchange, open))
    }

    pub(crate) fn get_task(
        &self,
        peer: &LiaisonPeer,
        request: &GetTaskRequest,
    ) -> Result<Task, A2aError> {
        let exchange = self.exchange(peer, &request.id);
        let operation = exchange
            .as_ref()
            .map(|exchange| exchange.cable.sender_operation().to_owned())
            .unwrap_or_default();
        let readable = exchange.is_ok() && self.allowed(peer, "cable.read", &operation);
        self.audit(
            peer,
            "get_task",
            &operation,
            &format!(
                "{} {}",
                request.id,
                if readable { "read" } else { "refused" }
            ),
        );
        let exchange = exchange?;
        if !readable {
            return Err(A2aError::task_not_found(&request.id));
        }
        Ok(Self::task(&exchange, request.history_length, true))
    }

    pub(crate) fn list_tasks(
        &self,
        peer: &LiaisonPeer,
        request: &ListTasksRequest,
    ) -> Result<ListTasksResponse, A2aError> {
        let size = request.page_size.unwrap_or(50).clamp(1, 100);
        let start = match request.page_token.as_deref() {
            None | Some("") => 0,
            Some(token) => token
                .parse::<usize>()
                .map_err(|_| A2aError::new(ErrorKind::InvalidParams, "not a page token"))?,
        };
        let mut tasks = Vec::new();
        let mut matched = 0usize;
        for cable in self.cables_of(peer)? {
            if request
                .context_id
                .as_deref()
                .is_some_and(|context| cable.conversation_id() != Some(context))
                || !self.allowed(peer, "cable.read", cable.sender_operation())
            {
                continue;
            }
            let Ok(exchange) = self.exchange(peer, cable.id()) else {
                continue;
            };
            if request
                .status
                .as_ref()
                .is_some_and(|state| state != &exchange.state)
            {
                continue;
            }
            matched += 1;
            if matched > start && tasks.len() < size as usize {
                tasks.push(Self::task(
                    &exchange,
                    request.history_length,
                    request.include_artifacts.unwrap_or(false),
                ));
            }
        }
        let next = start + tasks.len();
        self.audit(
            peer,
            "list_tasks",
            "",
            &format!("{} of {matched}", tasks.len()),
        );
        Ok(ListTasksResponse {
            page_size: size,
            total_size: u32::try_from(matched).unwrap_or(u32::MAX),
            next_page_token: if next < matched {
                next.to_string()
            } else {
                String::new()
            },
            tasks,
            ..ListTasksResponse::default()
        })
    }

    pub(crate) fn cancel_task(
        &self,
        peer: &LiaisonPeer,
        request: &CancelTaskRequest,
    ) -> Result<Task, A2aError> {
        let exchange = self.exchange(peer, &request.id)?;
        let operation = exchange.cable.sender_operation().to_owned();
        let refused = || {
            self.audit(
                peer,
                "cancel_task",
                &operation,
                &format!("{} refused", request.id),
            );
            A2aError::new(
                ErrorKind::TaskNotCancelable,
                "only a message the addressee has not had yet can be withdrawn",
            )
            .with_metadata("taskId", &request.id)
        };
        if exchange.state != TaskState::Submitted {
            return Err(refused());
        }
        let snapshot = self.snapshot()?;
        let relay = self.relay(&snapshot, Vec::new())?;
        let context = exchange.cable.conversation_id().unwrap_or_default();
        let principal = Self::principal_for(
            peer,
            exchange.cable.peer().unwrap_or_default(),
            &operation,
            Some(&route(context)),
        )?;
        if relay.withdraw(&principal, exchange.cable.id()).is_err() {
            return Err(refused());
        }
        self.audit(
            peer,
            "cancel_task",
            &operation,
            &format!("{} withdrawn", request.id),
        );
        Ok(Self::task(&self.exchange(peer, &request.id)?, None, true))
    }

    pub(crate) fn subscribe(
        &self,
        peer: &LiaisonPeer,
        request: &SubscribeToTaskRequest,
        open: Open,
    ) -> Result<Events, A2aError> {
        let exchange = self.exchange(peer, &request.id)?;
        let operation = exchange.cable.sender_operation().to_owned();
        self.audit(peer, "subscribe_to_task", &operation, &request.id);
        if !self.allowed(peer, "cable.read", &operation) {
            return Err(A2aError::task_not_found(&request.id));
        }
        if exchange.state.is_terminal() {
            return Err(A2aError::new(
                ErrorKind::UnsupportedOperation,
                "the task has ended",
            ));
        }
        Ok(self.follow(peer, &exchange, open))
    }
}

/// A stream of one Task's changes: its replies as artifacts, then its state.
struct Follow {
    liaison: Liaison,
    peer: LiaisonPeer,
    id: String,
    context: String,
    state: TaskState,
    replies: usize,
    pending: Vec<StreamResponse>,
    open: Open,
    done: bool,
}

impl Iterator for Follow {
    type Item = Result<StreamResponse, A2aError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if !self.pending.is_empty() {
                let item = self.pending.remove(0);
                if item.ends_task() {
                    self.done = true;
                    self.pending.clear();
                }
                return Some(Ok(item));
            }
            if self.done || !(self.open)() {
                return None;
            }
            let exchange = match self.liaison.exchange(&self.peer, &self.id) {
                Ok(exchange) => exchange,
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            };
            for reply in exchange.replies.iter().skip(self.replies) {
                self.pending
                    .push(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                        task_id: self.id.clone(),
                        context_id: self.context.clone(),
                        artifact: reply_artifact(&reply_message(reply, &self.context, &self.id)),
                        last_chunk: Some(true),
                        ..TaskArtifactUpdateEvent::default()
                    }));
            }
            self.replies = exchange.replies.len();
            if exchange.state != self.state {
                self.state = exchange.state.clone();
                let task = Liaison::task(&exchange, None, false);
                self.pending
                    .push(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                        task_id: self.id.clone(),
                        context_id: self.context.clone(),
                        status: task.status,
                        ..TaskStatusUpdateEvent::default()
                    }));
            }
            if self.pending.is_empty() {
                pause(self.liaison.inner.config.poll, &self.open);
            }
        }
    }
}

fn watching(caller: &Caller<'_>) -> Open {
    let watch = caller.watch();
    Arc::new(move || !watch.is_closed())
}

impl Agent for Liaison {
    fn agent_card(&self, caller: &Caller<'_>) -> AgentCard {
        self.card(self.peer_by_id(caller.peer))
    }

    fn send_message(
        &self,
        caller: &Caller<'_>,
        request: SendMessageRequest,
    ) -> Result<SendMessageResponse, A2aError> {
        let calling = self.calling(caller)?;
        Liaison::send_message(self, &calling, &request, &watching(caller))
    }

    fn send_streaming_message(
        &self,
        caller: &Caller<'_>,
        request: SendMessageRequest,
    ) -> Result<Events, A2aError> {
        let calling = self.calling(caller)?;
        self.send_streaming(&calling, &request, watching(caller))
    }

    fn get_task(&self, caller: &Caller<'_>, request: GetTaskRequest) -> Result<Task, A2aError> {
        Liaison::get_task(self, self.calling(caller)?.peer, &request)
    }

    fn list_tasks(
        &self,
        caller: &Caller<'_>,
        request: ListTasksRequest,
    ) -> Result<ListTasksResponse, A2aError> {
        Liaison::list_tasks(self, self.calling(caller)?.peer, &request)
    }

    fn cancel_task(
        &self,
        caller: &Caller<'_>,
        request: CancelTaskRequest,
    ) -> Result<Task, A2aError> {
        Liaison::cancel_task(self, self.calling(caller)?.peer, &request)
    }

    fn subscribe_to_task(
        &self,
        caller: &Caller<'_>,
        request: SubscribeToTaskRequest,
    ) -> Result<Events, A2aError> {
        self.subscribe(self.calling(caller)?.peer, &request, watching(caller))
    }
}

/// The Session a context's replies come back to: Liaison's own name for the thread, never the
/// A2A Task ID.
fn route(context: &str) -> String {
    format!("liaison-{context}")
}

fn reply_message(reply: &CableDelivery, context: &str, task: &str) -> Message {
    let mut metadata = serde_json::Map::new();
    metadata.insert("from".to_owned(), Value::String(reply.from_agent.clone()));
    Message {
        message_id: reply.cable_id.clone(),
        context_id: Some(context.to_owned()),
        task_id: Some(task.to_owned()),
        role: Role::Agent,
        parts: vec![Part::text(reply.body.clone())],
        metadata: Some(metadata),
        ..Message::default()
    }
}

/// The audit log entry of a Cable Liaison sent for a peer: `cable.send`, its principal as actor.
fn write_cable_audit(
    registry: &dyn RegistryBackend,
    record: &CableAuditRecord,
) -> Result<(), String> {
    let scope = format!("{:?}", record.target_scope).to_lowercase();
    let mut target = format!(
        "{} {} {scope}:{} {}",
        record.agent,
        record.decision,
        record.target_name.as_deref().unwrap_or(""),
        record.cable_id.as_deref().unwrap_or("-")
    );
    cut(&mut target, 500);
    registry
        .record_audit(&AuditRecord {
            id: format!("liaison.cable-{}", uuid::Uuid::new_v4().simple()),
            actor: record
                .on_behalf_of
                .clone()
                .unwrap_or_else(|| record.agent.clone()),
            action: if record.event == "cable_read" {
                "cable.read"
            } else {
                "cable.send"
            }
            .to_owned(),
            operation: record.operation.clone(),
            resource: None,
            target,
            at: record.at,
        })
        .map_err(|error| error.to_string())
}

/// A retry key for a peer's message: the same message ID from the same peer is the same Cable.
fn idempotency_key(peer: &str, message_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{peer}\n{message_id}").as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("a2a-{hex}")
}

fn reply_artifact(reply: &Message) -> Artifact {
    Artifact {
        artifact_id: reply.message_id.clone(),
        name: Some("reply".to_owned()),
        parts: reply.parts.clone(),
        ..Artifact::default()
    }
}

fn unavailable(what: &str) -> A2aError {
    A2aError::new(ErrorKind::Internal, format!("{what} is unavailable"))
}

/// Sleeps `poll`, in slices short enough that a caller who left ends it soon.
fn pause(poll: Duration, open: &Open) {
    let slice = Duration::from_millis(50);
    let mut left = poll;
    while !left.is_zero() && open() {
        let step = left.min(slice);
        std::thread::sleep(step);
        left -= step;
    }
}

fn cut(text: &mut String, bytes: usize) {
    if text.len() > bytes {
        let mut end = bytes;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// Unix seconds as A2A writes a time: ISO 8601 in UTC, to the millisecond.
fn timestamp(at: f64) -> String {
    let millis = (at * 1000.0) as i128;
    time::OffsetDateTime::from_unix_timestamp_nanos(millis * 1_000_000)
        .ok()
        .and_then(|time| {
            time.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(coqui_a2a::model::timestamp_now)
}
