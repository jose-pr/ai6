//! Validated Cable wire values shared by the daemon and managed Agents.

mod relay;

pub use relay::{
    CableAction, CableAuditRecord, CableConfig, CableDelivery, CableEndpoint, CableGrant,
    CableHeldDelivery, CableHeldRecipient, CableInboundMode, CablePolicy, CablePrincipal,
    CableRelay, CableRelayError, CableRelease, CableSendRequest, DEFAULT_CABLE_TTL,
    MAX_CABLE_FANOUT,
};

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use super::limits::CableLimits;

/// Errors returned when a Cable value violates the wire contract.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CableError(String);

impl CableError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CableError {}

/// The scope used to resolve a Cable audience.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum CableScope {
    /// Deliver to one named Agent.
    Agent,
    /// Broadcast to eligible Agents at one Station.
    Station,
    /// Broadcast to eligible Agents in one Theater.
    Theater,
    /// Broadcast to every eligible Agent in the relay's global audience.
    Global,
    /// Broadcast to eligible subscribers of one custom channel.
    Channel,
    /// Broadcast to eligible Agents of one Operation and of every Operation subordinate to
    /// it.
    Operation,
}

/// A typed unicast Agent or broadcast audience.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CableTargetWire")]
pub struct CableTarget {
    scope: CableScope,
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CableTargetWire {
    scope: CableScope,
    #[serde(deserialize_with = "deserialize_required_option")]
    name: Option<String>,
}

impl TryFrom<CableTargetWire> for CableTarget {
    type Error = CableError;

    fn try_from(wire: CableTargetWire) -> Result<Self, Self::Error> {
        Self::try_new(wire.scope, wire.name)
    }
}

impl CableTarget {
    /// Constructs a validated Cable audience.
    pub fn try_new(scope: CableScope, name: Option<String>) -> Result<Self, CableError> {
        match scope {
            CableScope::Global if name.is_some() => {
                return Err(CableError::new("global Cable target does not take a name"));
            }
            CableScope::Global => {}
            _ => validate_text("Cable target name", name.as_deref(), 256, true)?,
        }
        Ok(Self { scope, name })
    }

    /// Returns the audience scope.
    pub fn scope(&self) -> CableScope {
        self.scope
    }

    /// Returns the audience name, or `None` for the global scope.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// The delivery state of one resolved recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum CableState {
    /// Accepted and waiting for the recipient's next read.
    Queued,
    /// Presented to the recipient's Agent Session.
    Delivered,
    /// Temporarily held by relay policy.
    Held,
    /// Refused by relay policy or the recipient.
    Refused,
    /// Acknowledged by the recipient.
    Acked,
    /// Expired before acknowledgement.
    Expired,
    /// Failed during delivery.
    Failed,
}

/// One relay-resolved Agent Session and its delivery state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CableRecipientWire")]
pub struct CableRecipient {
    agent: String,
    session_id: Option<String>,
    state: CableState,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CableRecipientWire {
    agent: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    session_id: Option<String>,
    state: CableState,
}

impl TryFrom<CableRecipientWire> for CableRecipient {
    type Error = CableError;

    fn try_from(wire: CableRecipientWire) -> Result<Self, Self::Error> {
        Self::try_new(wire.agent, wire.session_id, wire.state)
    }
}

impl CableRecipient {
    /// Constructs a validated delivery recipient.
    pub fn try_new(
        agent: String,
        session_id: Option<String>,
        state: CableState,
    ) -> Result<Self, CableError> {
        validate_text("Cable recipient agent", Some(&agent), 256, true)?;
        if let Some(session_id) = session_id.as_deref() {
            validate_text("Cable recipient session_id", Some(session_id), 512, true)?;
        }
        Ok(Self {
            agent,
            session_id,
            state,
        })
    }

    /// Returns the recipient Agent identity.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// Returns the targeted provider Session, if the relay resolved one.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Returns the current delivery state.
    pub fn state(&self) -> CableState {
        self.state
    }
}

/// Validated Cable request and its relay-resolved recipients.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CableWire")]
pub struct Cable {
    sender_agent: String,
    sender_operation: String,
    sender_theater: Option<String>,
    sender_station: String,
    target: CableTarget,
    body: String,
    recipients: Vec<CableRecipient>,
    idempotency_key: String,
    mission_id: Option<String>,
    session_id: Option<String>,
    conversation_id: Option<String>,
    reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    on_behalf_of: Option<String>,
    id: String,
    created_at: f64,
    expires_at: f64,
}

/// The validated input fields used to construct one Cable.
#[derive(Clone)]
#[non_exhaustive]
pub struct CableDraft {
    /// Authenticated sender Agent identity.
    pub sender_agent: String,
    /// Authenticated sender Operation identity.
    pub sender_operation: String,
    /// Authenticated sender Theater, when applicable.
    pub sender_theater: Option<String>,
    /// Authenticated sender station identity.
    pub sender_station: String,
    /// Explicit unicast or broadcast audience.
    pub target: CableTarget,
    /// Message body; excluded from `Debug` output.
    pub body: String,
    /// Audience snapshot resolved and authorized by the relay.
    pub recipients: Vec<CableRecipient>,
    /// Sender-scoped retry key.
    pub idempotency_key: String,
    /// AI6 Mission identity, when applicable.
    pub mission_id: Option<String>,
    /// Provider-issued Session identity, when applicable.
    pub session_id: Option<String>,
    /// Conversation grouping identity, when applicable.
    pub conversation_id: Option<String>,
    /// Parent Cable identity, when this is a reply.
    pub reply_to: Option<String>,
    /// The person an assistant sent this Cable for, when it was sent on a person's behalf.
    pub on_behalf_of: Option<String>,
    /// Unique Cable identifier supplied by the relay.
    pub id: String,
    /// Creation time as Unix seconds.
    pub created_at: f64,
    /// Expiration time as Unix seconds.
    pub expires_at: f64,
}

impl CableDraft {
    /// Starts a Cable draft from the required relay-authenticated fields.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sender_agent: String,
        sender_operation: String,
        sender_station: String,
        target: CableTarget,
        body: String,
        recipients: Vec<CableRecipient>,
        idempotency_key: String,
        id: String,
        created_at: f64,
        expires_at: f64,
    ) -> Self {
        Self {
            sender_agent,
            sender_operation,
            sender_theater: None,
            sender_station,
            target,
            body,
            recipients,
            idempotency_key,
            mission_id: None,
            session_id: None,
            conversation_id: None,
            reply_to: None,
            on_behalf_of: None,
            id,
            created_at,
            expires_at,
        }
    }

    /// Sets the sender's Theater identity.
    pub fn with_sender_theater(mut self, theater: impl Into<String>) -> Self {
        self.sender_theater = Some(theater.into());
        self
    }

    /// Associates this Cable with an AI6 Mission.
    pub fn with_mission(mut self, mission_id: impl Into<String>) -> Self {
        self.mission_id = Some(mission_id.into());
        self
    }

    /// Associates this Cable with the sending provider Session.
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Groups this Cable into a conversation.
    pub fn with_conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    /// Identifies the Cable this message replies to.
    pub fn with_reply_to(mut self, cable_id: impl Into<String>) -> Self {
        self.reply_to = Some(cable_id.into());
        self
    }

    /// Names the person the sending assistant acts for.
    pub fn with_on_behalf_of(mut self, person: impl Into<String>) -> Self {
        self.on_behalf_of = Some(person.into());
        self
    }
}

impl fmt::Debug for CableDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CableDraft")
            .field("sender_agent", &self.sender_agent)
            .field("sender_operation", &self.sender_operation)
            .field("sender_station", &self.sender_station)
            .field("sender_theater", &self.sender_theater)
            .field("target", &self.target)
            .field("body", &"[redacted]")
            .field("recipients", &self.recipients)
            .field("idempotency_key", &self.idempotency_key)
            .field("mission_id", &self.mission_id)
            .field("session_id", &self.session_id)
            .field("conversation_id", &self.conversation_id)
            .field("reply_to", &self.reply_to)
            .field("on_behalf_of", &self.on_behalf_of)
            .field("id", &self.id)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl fmt::Debug for Cable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Cable")
            .field("sender_agent", &self.sender_agent)
            .field("sender_operation", &self.sender_operation)
            .field("sender_station", &self.sender_station)
            .field("sender_theater", &self.sender_theater)
            .field("target", &self.target)
            .field("body", &"[redacted]")
            .field("recipients", &self.recipients)
            .field("idempotency_key", &self.idempotency_key)
            .field("mission_id", &self.mission_id)
            .field("session_id", &self.session_id)
            .field("conversation_id", &self.conversation_id)
            .field("reply_to", &self.reply_to)
            .field("on_behalf_of", &self.on_behalf_of)
            .field("id", &self.id)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CableWire {
    sender_agent: String,
    sender_operation: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    sender_theater: Option<String>,
    sender_station: String,
    target: CableTarget,
    body: String,
    recipients: Vec<CableRecipient>,
    idempotency_key: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    mission_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    session_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    conversation_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    reply_to: Option<String>,
    #[serde(default)]
    on_behalf_of: Option<String>,
    id: String,
    created_at: f64,
    expires_at: f64,
}

impl TryFrom<CableWire> for Cable {
    type Error = CableError;

    fn try_from(wire: CableWire) -> Result<Self, Self::Error> {
        Self::try_new(
            CableDraft {
                sender_agent: wire.sender_agent,
                sender_operation: wire.sender_operation,
                sender_theater: wire.sender_theater,
                sender_station: wire.sender_station,
                target: wire.target,
                body: wire.body,
                recipients: wire.recipients,
                idempotency_key: wire.idempotency_key,
                mission_id: wire.mission_id,
                session_id: wire.session_id,
                conversation_id: wire.conversation_id,
                reply_to: wire.reply_to,
                on_behalf_of: wire.on_behalf_of,
                id: wire.id,
                created_at: wire.created_at,
                expires_at: wire.expires_at,
            },
            &stored_limits(),
        )
    }
}

/// What a Cable read back is checked within: its record's size bound
/// (`[registry.limits.cables] record_bytes`) already holds the body, and the body and
/// lifetime were checked against the limits in force when it was sent.
fn stored_limits() -> CableLimits {
    CableLimits {
        body_bytes: crate::bounds::Limit::UNBOUNDED,
        max_ttl: crate::bounds::TimeLimit::UNBOUNDED,
        ..CableLimits::default()
    }
}

impl Cable {
    /// Constructs a validated Cable from relay-authenticated request data, its body and
    /// lifetime within `limits`.
    pub fn try_new(draft: CableDraft, limits: &CableLimits) -> Result<Self, CableError> {
        validate_text("Cable sender_agent", Some(&draft.sender_agent), 256, true)?;
        validate_text(
            "Cable sender_operation",
            Some(&draft.sender_operation),
            256,
            true,
        )?;
        validate_text(
            "Cable sender_station",
            Some(&draft.sender_station),
            256,
            true,
        )?;
        validate_text(
            "Cable idempotency_key",
            Some(&draft.idempotency_key),
            256,
            true,
        )?;
        if let Some(theater) = draft.sender_theater.as_deref() {
            validate_text("Cable sender_theater", Some(theater), 256, true)?;
        }
        validate_text("Cable id", Some(&draft.id), 64, true)?;
        if !draft
            .id
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '-' | '_'))
        {
            return Err(CableError::new(
                "Cable id must contain only letters, digits, '-' or '_'",
            ));
        }
        if draft.body.trim().is_empty() {
            return Err(CableError::new("Cable body must be non-empty text"));
        }
        if !limits.body_bytes.allows(draft.body.len()) {
            return Err(CableError::new(format!(
                "Cable body exceeds {} UTF-8 bytes",
                limits.body_bytes
            )));
        }
        if draft.recipients.is_empty() {
            return Err(CableError::new(
                "Cable needs at least one resolved recipient",
            ));
        }
        for (index, recipient) in draft.recipients.iter().enumerate() {
            if draft.recipients[..index].iter().any(|previous| {
                previous.agent == recipient.agent && previous.session_id == recipient.session_id
            }) {
                return Err(CableError::new("Cable recipients must be unique"));
            }
        }
        for (label, value, limit) in [
            ("mission_id", draft.mission_id.as_deref(), 512),
            ("session_id", draft.session_id.as_deref(), 512),
            ("conversation_id", draft.conversation_id.as_deref(), 64),
            ("reply_to", draft.reply_to.as_deref(), 64),
            ("on_behalf_of", draft.on_behalf_of.as_deref(), 256),
        ] {
            if let Some(value) = value {
                validate_text(&format!("Cable {label}"), Some(value), limit, true)?;
            }
        }
        if !draft.created_at.is_finite()
            || !draft.expires_at.is_finite()
            || draft.created_at <= 0.0
            || draft.expires_at <= draft.created_at
        {
            return Err(CableError::new(
                "Cable expiry must be after its creation time",
            ));
        }
        if !limits
            .max_ttl
            .allows_secs(draft.expires_at - draft.created_at)
        {
            return Err(CableError::new(format!(
                "Cable TTL exceeds {}",
                limits.max_ttl
            )));
        }
        Ok(Self {
            sender_agent: draft.sender_agent,
            sender_operation: draft.sender_operation,
            sender_theater: draft.sender_theater,
            sender_station: draft.sender_station,
            target: draft.target,
            body: draft.body,
            recipients: draft.recipients,
            idempotency_key: draft.idempotency_key,
            mission_id: draft.mission_id,
            session_id: draft.session_id,
            conversation_id: draft.conversation_id,
            reply_to: draft.reply_to,
            on_behalf_of: draft.on_behalf_of,
            id: draft.id,
            created_at: draft.created_at,
            expires_at: draft.expires_at,
        })
    }

    /// Returns the authenticated sender Agent.
    pub fn sender_agent(&self) -> &str {
        &self.sender_agent
    }

    /// Returns the authenticated sender Operation.
    pub fn sender_operation(&self) -> &str {
        &self.sender_operation
    }

    /// Returns the authenticated sender Theater, when present.
    pub fn sender_theater(&self) -> Option<&str> {
        self.sender_theater.as_deref()
    }

    /// Returns the authenticated sender station.
    pub fn sender_station(&self) -> &str {
        &self.sender_station
    }

    /// Returns the associated AI6 Mission identity, when present.
    pub fn mission_id(&self) -> Option<&str> {
        self.mission_id.as_deref()
    }

    /// Returns the provider-issued sending Session identity, when present.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Returns the explicit audience.
    pub fn target(&self) -> &CableTarget {
        &self.target
    }

    /// Returns the message body.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Returns the audience snapshot and delivery states.
    pub fn recipients(&self) -> &[CableRecipient] {
        &self.recipients
    }

    /// Returns the sender-scoped retry key.
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    /// Returns the conversation grouping identity, when present.
    pub fn conversation_id(&self) -> Option<&str> {
        self.conversation_id.as_deref()
    }

    /// Returns the parent Cable identity, when this Cable is a reply.
    pub fn reply_to(&self) -> Option<&str> {
        self.reply_to.as_deref()
    }

    /// Returns the person an assistant sent this Cable for, when it acted for one;
    /// recipients see the sender as `<sender agent> for <person>`.
    pub fn on_behalf_of(&self) -> Option<&str> {
        self.on_behalf_of.as_deref()
    }

    /// Returns this Cable's relay-generated identity.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns creation time as Unix seconds.
    pub fn created_at(&self) -> f64 {
        self.created_at
    }

    /// Returns expiration time as Unix seconds.
    pub fn expires_at(&self) -> f64 {
        self.expires_at
    }

    /// Compares immutable send-request content, ignoring generated and delivery fields.
    pub fn same_request(&self, other: &Self) -> bool {
        self.sender_agent == other.sender_agent
            && self.sender_operation == other.sender_operation
            && self.sender_theater == other.sender_theater
            && self.sender_station == other.sender_station
            && self.target == other.target
            && self.body == other.body
            && self
                .recipients
                .iter()
                .map(|recipient| (&recipient.agent, &recipient.session_id))
                .eq(other
                    .recipients
                    .iter()
                    .map(|recipient| (&recipient.agent, &recipient.session_id)))
            && self.idempotency_key == other.idempotency_key
            && self.mission_id == other.mission_id
            && self.session_id == other.session_id
            && self.conversation_id == other.conversation_id
            && self.reply_to == other.reply_to
            && self.on_behalf_of == other.on_behalf_of
    }

    /// Returns a copy with one recipient's permitted delivery transition applied.
    pub fn with_recipient_state(
        &self,
        agent: &str,
        session_id: Option<&str>,
        state: CableState,
    ) -> Result<Option<Self>, CableError> {
        let Some(index) = self.recipients.iter().position(|recipient| {
            recipient.agent == agent && recipient.session_id.as_deref() == session_id
        }) else {
            return Ok(None);
        };
        let current = self.recipients[index].state;
        let permitted = match current {
            CableState::Queued => matches!(
                state,
                CableState::Delivered
                    | CableState::Held
                    | CableState::Refused
                    | CableState::Expired
                    | CableState::Failed
            ),
            CableState::Held => matches!(
                state,
                CableState::Queued
                    | CableState::Delivered
                    | CableState::Refused
                    | CableState::Expired
                    | CableState::Failed
            ),
            CableState::Delivered => {
                matches!(
                    state,
                    CableState::Acked | CableState::Expired | CableState::Failed
                )
            }
            CableState::Refused | CableState::Acked | CableState::Expired | CableState::Failed => {
                false
            }
        };
        if !permitted {
            return Err(CableError::new(format!(
                "cannot move Cable delivery from {current:?} to {state:?}"
            )));
        }
        let mut updated = self.clone();
        updated.recipients[index].state = state;
        Ok(Some(updated))
    }
}

fn validate_text(
    label: &str,
    value: Option<&str>,
    max_chars: usize,
    non_empty: bool,
) -> Result<(), CableError> {
    let Some(value) = value else {
        return Err(CableError::new(format!("{label} must be non-empty text")));
    };
    if (non_empty && value.trim().is_empty()) || value.chars().any(char::is_control) {
        return Err(CableError::new(format!(
            "{label} must be valid non-empty text"
        )));
    }
    if value.chars().count() > max_chars {
        return Err(CableError::new(format!(
            "{label} exceeds {max_chars} characters"
        )));
    }
    Ok(())
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> CableDraft {
        CableDraft::new(
            "007".into(),
            "operation-a".into(),
            "station-a".into(),
            CableTarget::try_new(CableScope::Station, Some("station-a".into())).unwrap(),
            "Need a review".into(),
            vec![
                CableRecipient::try_new("008".into(), Some("session-8".into()), CableState::Queued)
                    .unwrap(),
            ],
            "request-1".into(),
            "cable-1".into(),
            1_700_000_000.0,
            1_700_000_900.0,
        )
        .with_sender_theater("theater-a")
        .with_mission("mission-1")
        .with_session("session-7")
        .with_conversation("conversation-1")
    }

    #[test]
    fn cable_round_trips_through_its_wire_shape() {
        let cable = Cable::try_new(draft(), &CableLimits::default()).unwrap();
        let value = serde_json::to_value(&cable).unwrap();
        assert_eq!(
            value["target"],
            serde_json::json!({"scope":"station","name":"station-a"})
        );
        assert_eq!(value["recipients"][0]["state"], "queued");
        assert_eq!(serde_json::from_value::<Cable>(value).unwrap(), cable);
        assert!(!format!("{cable:?}").contains("Need a review"));
    }

    #[test]
    fn the_person_a_cable_was_sent_for_is_kept_and_omitted_when_none() {
        let plain = serde_json::to_value(Cable::try_new(draft(), &CableLimits::default()).unwrap())
            .unwrap();
        assert!(plain.get("on_behalf_of").is_none());
        assert!(serde_json::from_value::<Cable>(plain).is_ok());

        let cable = Cable::try_new(
            draft().with_on_behalf_of("person-007"),
            &CableLimits::default(),
        )
        .unwrap();
        assert_eq!(cable.on_behalf_of(), Some("person-007"));
        let value = serde_json::to_value(&cable).unwrap();
        assert_eq!(value["on_behalf_of"], "person-007");
        assert_eq!(serde_json::from_value::<Cable>(value).unwrap(), cable);
        assert!(!cable.same_request(&Cable::try_new(draft(), &CableLimits::default()).unwrap()));
        assert!(
            Cable::try_new(draft().with_on_behalf_of("a\nb"), &CableLimits::default()).is_err()
        );
    }

    #[test]
    fn cable_rejects_invalid_targets_duplicate_recipients_and_oversized_body() {
        assert!(CableTarget::try_new(CableScope::Global, Some("all".into())).is_err());
        assert!(CableTarget::try_new(CableScope::Station, None).is_err());

        let mut repeated = draft();
        repeated.recipients.push(repeated.recipients[0].clone());
        assert!(Cable::try_new(repeated, &CableLimits::default()).is_err());

        let mut oversized = draft();
        oversized.body = "x".repeat(16 * 1024 + 1);
        assert!(Cable::try_new(oversized.clone(), &CableLimits::default()).is_err());
        let roomy = CableLimits {
            body_bytes: crate::bounds::Limit::UNBOUNDED,
            ..CableLimits::default()
        };
        assert!(Cable::try_new(oversized, &roomy).is_ok());
    }

    #[test]
    fn recipient_delivery_transitions_are_checked() {
        let cable = Cable::try_new(draft(), &CableLimits::default()).unwrap();
        let delivered = cable
            .with_recipient_state("008", Some("session-8"), CableState::Delivered)
            .unwrap()
            .unwrap();
        let acked = delivered
            .with_recipient_state("008", Some("session-8"), CableState::Acked)
            .unwrap()
            .unwrap();
        assert_eq!(acked.recipients()[0].state(), CableState::Acked);
        assert!(
            acked
                .with_recipient_state("008", Some("session-8"), CableState::Queued)
                .is_err()
        );
        assert!(
            cable
                .with_recipient_state("missing", None, CableState::Delivered)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn deserialization_validates_wire_values_and_rejects_unknown_fields() {
        let mut value =
            serde_json::to_value(Cable::try_new(draft(), &CableLimits::default()).unwrap())
                .unwrap();
        value["target"]["scope"] = serde_json::json!("moon");
        assert!(serde_json::from_value::<Cable>(value).is_err());

        let mut value =
            serde_json::to_value(Cable::try_new(draft(), &CableLimits::default()).unwrap())
                .unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Cable>(value).is_err());

        let mut value =
            serde_json::to_value(Cable::try_new(draft(), &CableLimits::default()).unwrap())
                .unwrap();
        value.as_object_mut().unwrap().remove("reply_to");
        assert!(serde_json::from_value::<Cable>(value).is_err());
    }
}
