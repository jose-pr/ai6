//! Compartments: need-to-know access across Operations.
//!
//! Clearance is how far an Agent may act in its own work; a Compartment is what one may know of,
//! and say to, other Operations' work. A [`Compartment`] names what it grants ([`CompartmentAction`]),
//! what it covers (Operations or Missions) and who is read in (people, groups, accounts,
//! Operations, Missions, Agents). It carries no policy authority: nothing here bypasses cover,
//! leases, secrets, safe houses, green ink, quarantine or Station policy.
//!
//! Each compartment compiles into two `coqui-roles` authorizers, one for people and one for
//! Sessions, so "does this subject hold this action there" has one answer built from the
//! authorizer the rest of AI6 uses. Effective access is the union over compartments.
//!
//! A read or a Cable asks [`current`], which reads the records from the provider on every call:
//! a removed grant holds from the next call, a record that cannot be read grants nothing, and a
//! provider that cannot be read refuses the call rather than answering from memory.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use coqui_roles::RoleLimits;
use guatiao::{FromValue, Schema, ToValue};
use guatiao_intake::Form;

use crate::bounds::{Limit, Rate};
use crate::directorates::m::access::{
    AuthorizationRequest, Authorizer, Lineage, Person, RoleAuthorizer, Rule,
};
use crate::directorates::m::config::{ConfigProvider, ConfigRecord, ConfigScope};
use crate::directorates::m::records;
use crate::directorates::m::roster::{
    ClearanceLevel, Roster, RosterError, RosterErrorKind, RosterSettings, clearance_rank, error,
};

/// The role every compiled rule of one compartment binds. One role per authorizer makes the
/// first matching rule the same answer as any matching rule.
const ROLE: &str = "compartment";
/// An action no request names, for a role with nothing to grant to one kind of subject.
const NOTHING: &str = "ai6.none";
/// The scope kinds compiled rules use: an Operation alone, it and everything under it, a Mission.
const SCOPE_KINDS: [&str; 3] = ["operation", "mission", "subtree"];

/// What a compartment may grant. The vocabulary is closed because a compartment carries no
/// policy authority: any other name makes the record invalid.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, ToValue, FromValue, Schema)]
pub enum CompartmentAction {
    /// IDs, states, roles, Agent codenames and times, with no text.
    #[map(rename = "mission.status")]
    #[schema(label = "See status: IDs, states, roles, times; no text")]
    MissionStatus,
    /// The brief as well; implies [`MissionStatus`](Self::MissionStatus).
    #[map(rename = "mission.read")]
    #[schema(label = "Read briefs")]
    MissionRead,
    /// The filed outcome, the CX report and the exported refs.
    #[map(rename = "product.read")]
    #[schema(label = "Read products: outcome, report, refs")]
    ProductRead,
    /// Send Cables there.
    #[map(rename = "cable.send")]
    #[schema(label = "Send Cables")]
    CableSend,
    /// Have Moneypenny read Cables there; people only.
    #[map(rename = "cable.read")]
    #[schema(label = "Read Cables, through Moneypenny (people)")]
    CableRead,
    /// Propose Missions there, as untrusted intake; Agents only.
    #[map(rename = "mission.propose")]
    #[schema(label = "Propose Missions (Agents)")]
    MissionPropose,
    /// Read the Operation's memory.
    #[map(rename = "memory.read")]
    #[schema(label = "Read its memory")]
    MemoryRead,
}

impl CompartmentAction {
    /// Every action.
    pub const ALL: [Self; 7] = [
        Self::MissionStatus,
        Self::MissionRead,
        Self::ProductRead,
        Self::CableSend,
        Self::CableRead,
        Self::MissionPropose,
        Self::MemoryRead,
    ];

    /// The name written in a record.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissionStatus => "mission.status",
            Self::MissionRead => "mission.read",
            Self::ProductRead => "product.read",
            Self::CableSend => "cable.send",
            Self::CableRead => "cable.read",
            Self::MissionPropose => "mission.propose",
            Self::MemoryRead => "memory.read",
        }
    }

    /// The action written `name`.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.as_str() == name)
    }

    const fn for_people(self) -> bool {
        !matches!(self, Self::MissionPropose)
    }

    const fn for_agents(self) -> bool {
        !matches!(self, Self::CableRead)
    }
}

impl fmt::Display for CompartmentAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a compartment covers.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CoverTarget {
    /// An Operation and its Missions.
    Operation(String),
    /// One Mission. Missions are runtime records: an ID no Mission has simply matches nothing.
    Mission(String),
}

impl fmt::Display for CoverTarget {
    /// `operation:<id>` or `mission:<id>`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operation(id) => write!(formatter, "operation:{id}"),
            Self::Mission(id) => write!(formatter, "mission:{id}"),
        }
    }
}

impl CoverTarget {
    /// The target `text` names: `operation:<id>` or `mission:<id>`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.split_once(':')? {
            ("operation", id) if !id.is_empty() => Some(Self::Operation(id.to_owned())),
            ("mission", id) if !id.is_empty() => Some(Self::Mission(id.to_owned())),
            _ => None,
        }
    }
}

/// One thing a compartment grants access to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompartmentCover {
    /// What is covered.
    pub target: CoverTarget,
    /// Whether a covered Operation includes the Operations under it. A covered Operation
    /// always includes its own Missions.
    pub propagate: bool,
}

impl CompartmentCover {
    /// A cover of `target`, reaching down the tree.
    pub fn new(target: CoverTarget) -> Self {
        Self {
            target,
            propagate: true,
        }
    }

    /// This cover with `propagate` set.
    #[must_use]
    pub fn with_propagate(mut self, propagate: bool) -> Self {
        self.propagate = propagate;
        self
    }
}

/// Who is read in.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReadInSubject {
    /// A person, by ID.
    Person(String),
    /// A group of people.
    Group(String),
    /// A sign-in account.
    Account(String),
    /// Every Session of an Operation.
    Operation(String),
    /// Every Session of one Mission.
    Mission(String),
    /// An Agent, in every Operation it works in.
    Agent(String),
}

impl fmt::Display for ReadInSubject {
    /// `person:<id>`, `group:<name>`, `account:<name>`, `operation:<id>`, `mission:<id>` or
    /// `agent:<codename>`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, id) = match self {
            Self::Person(id) => ("person", id),
            Self::Group(id) => ("group", id),
            Self::Account(id) => ("account", id),
            Self::Operation(id) => ("operation", id),
            Self::Mission(id) => ("mission", id),
            Self::Agent(id) => ("agent", id),
        };
        write!(formatter, "{kind}:{id}")
    }
}

impl ReadInSubject {
    /// The subject `text` names, split at its first `:`.
    pub fn parse(text: &str) -> Option<Self> {
        let (kind, id) = text.split_once(':')?;
        if id.is_empty() {
            return None;
        }
        let id = id.to_owned();
        Some(match kind {
            "person" => Self::Person(id),
            "group" => Self::Group(id),
            "account" => Self::Account(id),
            "operation" => Self::Operation(id),
            "mission" => Self::Mission(id),
            "agent" => Self::Agent(id),
            _ => return None,
        })
    }

    /// Whether the subject is a person, a group or an account, as opposed to a Session's.
    pub const fn is_people(&self) -> bool {
        matches!(self, Self::Person(_) | Self::Group(_) | Self::Account(_))
    }
}

/// One subject read into a compartment.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReadIn {
    /// Who is read in.
    pub subject: ReadInSubject,
    /// Whether an Operation read in includes the Operations under it.
    pub propagate: bool,
}

impl ReadIn {
    /// `subject` read in, reaching down the tree.
    pub fn new(subject: ReadInSubject) -> Self {
        Self {
            subject,
            propagate: true,
        }
    }

    /// This read-in with `propagate` set.
    #[must_use]
    pub fn with_propagate(mut self, propagate: bool) -> Self {
        self.propagate = propagate;
        self
    }
}

/// How much one compartment may carry. Every bound can be lifted with `none`; a value looser
/// than its default is reported by [`Compartment::weakenings`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct CompartmentLimits {
    /// The most recipients one Cable to the compartment may reach.
    #[schema(title = "Recipients of one Cable")]
    #[map(default = CompartmentLimits::default().max_fanout)]
    pub max_fanout: Limit,
    /// Cables a second to the compartment, from everyone, across Stations.
    #[schema(title = "Cables a second")]
    #[map(default = CompartmentLimits::default().rate)]
    pub rate: Rate,
    /// Cables to the compartment in a burst.
    #[schema(title = "Burst of Cables")]
    #[map(default = CompartmentLimits::default().burst)]
    pub burst: Limit,
    /// Cables a second from one sender; `none` keeps no per-sender count.
    #[schema(title = "Cables a second, per sender")]
    #[map(default = CompartmentLimits::default().sender_rate)]
    pub sender_rate: Rate,
    /// Cables from one sender in a burst; applies with `sender_rate`.
    #[schema(title = "Burst per sender")]
    #[map(default = CompartmentLimits::default().sender_burst)]
    pub sender_burst: Limit,
    /// Bytes of one text an Agent reads from a product; the rest is cut with a marker.
    #[schema(title = "Size of a product text", format = "bytes")]
    #[map(default = CompartmentLimits::default().max_product_bytes)]
    pub max_product_bytes: Limit,
}

impl Default for CompartmentLimits {
    /// 100 recipients, 10 Cables a second with a burst of 50, no per-sender count, 64 KiB of text.
    fn default() -> Self {
        Self {
            max_fanout: Limit::at(100),
            rate: Rate::at(10.0),
            burst: Limit::at(50),
            sender_rate: Rate::UNBOUNDED,
            sender_burst: Limit::UNBOUNDED,
            max_product_bytes: Limit::at(65_536),
        }
    }
}

impl CompartmentLimits {
    fn validate(&self) -> Result<(), String> {
        for (key, rate) in [("rate", self.rate), ("sender_rate", self.sender_rate)] {
            if rate.get().is_some_and(|rate| rate <= 0.0) {
                return Err(format!("limits.{key} must be above 0, or \"none\""));
            }
        }
        for (key, limit) in [
            ("max_fanout", self.max_fanout),
            ("burst", self.burst),
            ("sender_burst", self.sender_burst),
        ] {
            if limit == Limit::at(0) {
                return Err(format!("limits.{key} must be at least 1, or \"none\""));
            }
        }
        Ok(())
    }
}

/// One Compartment.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct Compartment {
    /// Its ID, a codeword by convention (`KESTREL`).
    pub id: String,
    /// What it is for.
    pub description: String,
    /// What it grants.
    pub actions: BTreeSet<CompartmentAction>,
    /// What it grants access to.
    pub covers: Vec<CompartmentCover>,
    /// Who holds it.
    pub read_in: Vec<ReadIn>,
    /// The clearance a Session needs for the compartment to apply to it; `None` asks none.
    pub min_clearance: Option<String>,
    /// How much it may carry.
    pub limits: CompartmentLimits,
}

impl Compartment {
    /// A compartment with nothing in it yet, invalid until it has actions and covers.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: String::new(),
            actions: BTreeSet::new(),
            covers: Vec::new(),
            read_in: Vec::new(),
            min_clearance: None,
            limits: CompartmentLimits::default(),
        }
    }

    /// This compartment with `description`.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// This compartment granting `actions`.
    #[must_use]
    pub fn with_actions(mut self, actions: impl IntoIterator<Item = CompartmentAction>) -> Self {
        self.actions = actions.into_iter().collect();
        self
    }

    /// This compartment covering `cover` as well.
    #[must_use]
    pub fn with_cover(mut self, cover: CompartmentCover) -> Self {
        self.covers.push(cover);
        self
    }

    /// This compartment with `read_in` read in as well.
    #[must_use]
    pub fn with_read_in(mut self, read_in: ReadIn) -> Self {
        self.read_in.push(read_in);
        self
    }

    /// This compartment asking `clearance` of a Session.
    #[must_use]
    pub fn with_min_clearance(mut self, clearance: impl Into<String>) -> Self {
        self.min_clearance = Some(clearance.into());
        self
    }

    /// This compartment with `limits`.
    #[must_use]
    pub fn with_limits(mut self, limits: CompartmentLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Checks everything that does not need the roster: a usable ID, at least one action, known
    /// names, usable limits. A compartment that covers nothing grants nothing and stays valid:
    /// an Operation may withdraw the last cover, and covers are added one at a time.
    ///
    /// # Errors
    ///
    /// [`RosterErrorKind::Invalid`] naming what is wrong.
    pub fn validate(&self) -> Result<(), RosterError> {
        let invalid =
            |reason: &dyn fmt::Display| error(format!("compartment {:?}: {reason}", self.id));
        if !ConfigScope::is_plain(&self.id) || self.id.chars().any(char::is_whitespace) {
            return Err(invalid(&"its ID must be one word with no ':', '/' or '\\'"));
        }
        if self.actions.is_empty() {
            return Err(invalid(&"it grants no action"));
        }
        for cover in &self.covers {
            let (CoverTarget::Operation(id) | CoverTarget::Mission(id)) = &cover.target;
            if id.is_empty() {
                return Err(invalid(&"a cover names nothing"));
            }
            if !cover.propagate && matches!(cover.target, CoverTarget::Mission(_)) {
                return Err(invalid(
                    &"propagate applies only to a cover of an Operation",
                ));
            }
        }
        for read_in in &self.read_in {
            let subject = read_in.subject.to_string();
            if subject.ends_with(':') {
                return Err(invalid(&"a read-in names no subject"));
            }
            if !read_in.propagate && !matches!(read_in.subject, ReadInSubject::Operation(_)) {
                return Err(invalid(&"propagate applies only to an Operation read in"));
            }
        }
        if let Some(clearance) = &self.min_clearance {
            clearance_rank(clearance).map_err(|reason| invalid(&reason))?;
        }
        self.limits.validate().map_err(|reason| invalid(&reason))
    }

    /// Each limit looser than its default: unbounded where the default bounds it, or larger.
    /// Actions, covers and read-ins are grants, as Cable grants are, and never weakenings.
    pub fn weakenings(&self) -> Vec<String> {
        let current = serde_json::to_value(LimitsWords::of(&self.limits)).unwrap_or_default();
        let default = serde_json::to_value(LimitsWords::of(&CompartmentLimits::default()))
            .unwrap_or_default();
        crate::bounds::looser(
            &format!("compartment {}: limits", self.id),
            &current,
            &default,
            &[],
        )
    }

    /// The JSON form of the record a configuration provider stores.
    pub fn to_definition(&self) -> serde_json::Value {
        let cover_entry = |cover: &CompartmentCover| {
            let (key, id) = match &cover.target {
                CoverTarget::Operation(id) => ("operation", id),
                CoverTarget::Mission(id) => ("mission", id),
            };
            let mut entry = serde_json::Map::new();
            entry.insert(key.to_owned(), serde_json::Value::String(id.clone()));
            if !cover.propagate {
                entry.insert("propagate".to_owned(), serde_json::Value::Bool(false));
            }
            serde_json::Value::Object(entry)
        };
        let read_in_entry = |read_in: &ReadIn| {
            let mut entry = serde_json::Map::new();
            entry.insert(
                "subject".to_owned(),
                serde_json::Value::String(read_in.subject.to_string()),
            );
            if !read_in.propagate {
                entry.insert("propagate".to_owned(), serde_json::Value::Bool(false));
            }
            serde_json::Value::Object(entry)
        };
        let mut definition = serde_json::json!({
            "description": self.description,
            "actions": self.actions.iter().map(|action| action.as_str()).collect::<Vec<_>>(),
            "covers": self.covers.iter().map(cover_entry).collect::<Vec<_>>(),
            "read_in": self.read_in.iter().map(read_in_entry).collect::<Vec<_>>(),
            "limits": records::to_json(&self.limits).unwrap_or_default(),
        });
        if let Some(clearance) = &self.min_clearance {
            definition["min_clearance"] = serde_json::Value::String(clearance.clone());
        }
        definition
    }

    /// A compartment from the keys of its `[compartment.<id>]` table.
    ///
    /// # Errors
    ///
    /// [`RosterErrorKind::Invalid`] naming an action outside the vocabulary or what
    /// [`validate`](Self::validate) refuses.
    pub fn from_definition(
        id: impl Into<String>,
        definition: &serde_json::Value,
    ) -> Result<Self, RosterError> {
        let id = id.into();
        let invalid = |reason: &dyn fmt::Display| error(format!("compartment {id:?}: {reason}"));
        if let Some(actions) = definition
            .get("actions")
            .and_then(|actions| actions.as_array())
        {
            for action in actions {
                let name = action.as_str().unwrap_or_default();
                if CompartmentAction::parse(name).is_none() {
                    return Err(invalid(&format!(
                        "action {name:?} is not one a compartment grants: a compartment grants no policy authority"
                    )));
                }
            }
        }
        let parsed: CompartmentDefinition =
            records::from_json(definition).map_err(|reason| invalid(&reason))?;
        let mut covers = Vec::with_capacity(parsed.covers.len());
        for entry in parsed.covers {
            let target = match (entry.operation, entry.mission) {
                (Some(operation), None) => CoverTarget::Operation(operation),
                (None, Some(mission)) => CoverTarget::Mission(mission),
                _ => return Err(invalid(&"a cover names one Operation or one Mission")),
            };
            covers.push(CompartmentCover {
                target,
                propagate: entry.propagate,
            });
        }
        let mut read_in = Vec::with_capacity(parsed.read_in.len());
        for entry in parsed.read_in {
            let subject = ReadInSubject::parse(&entry.subject).ok_or_else(|| {
                invalid(&format!(
                    "read_in {:?} is not person:, group:, account:, operation:, mission: or agent:",
                    entry.subject
                ))
            })?;
            read_in.push(ReadIn {
                subject,
                propagate: entry.propagate,
            });
        }
        let compartment = Self {
            id: id.clone(),
            description: parsed.description,
            actions: parsed.actions.into_iter().collect(),
            covers,
            read_in,
            min_clearance: parsed.min_clearance.map(|level| level.as_str().to_owned()),
            limits: parsed.limits,
        };
        compartment.validate()?;
        Ok(compartment)
    }
}

/// [`CompartmentLimits`] as the numbers [`crate::bounds::looser`] compares: a lifted bound is
/// `null`.
#[derive(serde::Serialize)]
struct LimitsWords {
    max_fanout: Option<u64>,
    rate: Option<f64>,
    burst: Option<u64>,
    max_product_bytes: Option<u64>,
}

impl LimitsWords {
    fn of(limits: &CompartmentLimits) -> Self {
        Self {
            max_fanout: limits.max_fanout.get(),
            rate: limits.rate.get(),
            burst: limits.burst.get(),
            max_product_bytes: limits.max_product_bytes.get(),
        }
    }
}

/// One `covers` entry as written: `{ operation = "coqui" }` or `{ mission = "coqui-41" }`.
#[derive(Debug, Clone, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
struct CoverEntry {
    /// An Operation and its Missions.
    #[schema(title = "Operation", format = "record:operations")]
    operation: Option<String>,
    /// One Mission.
    #[schema(title = "Mission")]
    mission: Option<String>,
    /// Whether a covered Operation includes the Operations under it.
    #[schema(title = "Include the Operations under it")]
    #[map(default = true)]
    propagate: bool,
}

/// One `read_in` entry as written: `{ subject = "operation:ai6" }`.
#[derive(Debug, Clone, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
struct ReadInEntry {
    /// `person:`, `group:`, `account:`, `operation:`, `mission:` or `agent:`, then its name.
    #[schema(title = "Subject")]
    subject: String,
    /// Whether an Operation read in includes the Operations under it.
    #[schema(title = "Include the Operations under it")]
    #[map(default = true)]
    propagate: bool,
}

/// A compartment as its record holds it.
#[derive(Debug, Clone, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub(crate) struct CompartmentDefinition {
    #[map(default)]
    description: String,
    #[map(default)]
    actions: Vec<CompartmentAction>,
    #[map(default)]
    covers: Vec<CoverEntry>,
    #[map(default)]
    read_in: Vec<ReadInEntry>,
    min_clearance: Option<ClearanceLevel>,
    #[map(default)]
    limits: CompartmentLimits,
}

/// What a person edits with a form. Covers and read-ins are entries of their own, so one the
/// editor may not see survives an edit.
#[derive(Debug, Clone, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct CompartmentSettings {
    /// What this compartment is for.
    #[schema(title = "Description")]
    #[map(default)]
    #[form(widget = "textarea")]
    pub description: String,
    /// What it grants on what it covers.
    #[schema(title = "What it grants")]
    #[map(default)]
    pub actions: Vec<CompartmentAction>,
    /// The clearance a Session needs for the compartment to apply to it.
    #[schema(title = "Clearance floor")]
    min_clearance: Option<ClearanceLevel>,
    /// How much it may carry.
    #[schema(title = "Limits")]
    #[map(default)]
    pub limits: CompartmentLimits,
}

impl CompartmentSettings {
    /// Settings as a person submitted them.
    ///
    /// # Errors
    ///
    /// Names an action outside the vocabulary, or the key that is not acceptable.
    pub fn from_json(definition: &serde_json::Value) -> Result<Self, String> {
        if let Some(actions) = definition
            .get("actions")
            .and_then(|actions| actions.as_array())
        {
            for action in actions {
                let name = action.as_str().unwrap_or_default();
                if CompartmentAction::parse(name).is_none() {
                    return Err(format!(
                        "action {name:?} is not one a compartment grants: a compartment grants no policy authority"
                    ));
                }
            }
        }
        records::from_json(definition)
    }

    /// `base`, or a new compartment `id`, with these settings over it; its covers and read-ins
    /// stay.
    pub fn apply(self, id: &str, base: Option<Compartment>) -> Compartment {
        let mut compartment = base.unwrap_or_else(|| Compartment::new(id));
        compartment.description = self.description;
        compartment.actions = self.actions.into_iter().collect();
        compartment.min_clearance = self.min_clearance.map(|level| level.as_str().to_owned());
        compartment.limits = self.limits;
        compartment
    }
}

/// The Compartment records of one configuration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compartments {
    records: BTreeMap<String, Compartment>,
}

impl Compartments {
    /// No compartments.
    pub fn new() -> Self {
        Self::default()
    }

    /// These compartments, each checked.
    ///
    /// # Errors
    ///
    /// The first one [`Compartment::validate`] refuses.
    pub fn of(compartments: impl IntoIterator<Item = Compartment>) -> Result<Self, RosterError> {
        let mut records = BTreeMap::new();
        for compartment in compartments {
            compartment.validate()?;
            records.insert(compartment.id.clone(), compartment);
        }
        Ok(Self { records })
    }

    /// Reads the `compartment` family's records, refusing the first that is not acceptable.
    ///
    /// # Errors
    ///
    /// [`Compartment::from_definition`]'s.
    pub fn from_records(
        records: impl IntoIterator<Item = (String, serde_json::Value)>,
    ) -> Result<Self, RosterError> {
        let mut read = BTreeMap::new();
        for (id, definition) in records {
            let compartment = Compartment::from_definition(id.clone(), &definition)?;
            read.insert(id, compartment);
        }
        Ok(Self { records: read })
    }

    /// The compartment named `id`.
    pub fn get(&self, id: &str) -> Option<&Compartment> {
        self.records.get(id)
    }

    /// Every compartment, in ID order.
    pub fn iter(&self) -> impl Iterator<Item = &Compartment> {
        self.records.values()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Each compartment's [`weakenings`](Compartment::weakenings).
    pub fn weakenings(&self) -> Vec<String> {
        self.iter().flat_map(Compartment::weakenings).collect()
    }

    /// Compiles every compartment against `roster`. An Operation a compartment names must
    /// exist: Operations are configuration, unlike Missions.
    ///
    /// # Errors
    ///
    /// [`RosterErrorKind::Invalid`]: a covered or read-in Operation the roster does not have,
    /// or a compartment past the roster's `max_compartment_rules`.
    pub fn compile(&self, roster: &Roster) -> Result<CompiledCompartments, RosterError> {
        let lineage = Lineage::from_roster(roster);
        for compartment in self.iter() {
            for cover in &compartment.covers {
                if let CoverTarget::Operation(id) = &cover.target
                    && !lineage.contains(id)
                {
                    return Err(error(format!(
                        "compartment {:?} covers Operation {id:?}, which does not exist",
                        compartment.id
                    )));
                }
            }
            for read_in in &compartment.read_in {
                if let ReadInSubject::Operation(id) = &read_in.subject
                    && !lineage.contains(id)
                {
                    return Err(error(format!(
                        "compartment {:?} reads in Operation {id:?}, which does not exist",
                        compartment.id
                    )));
                }
            }
        }
        let mut compiled = self.compile_each(&lineage, roster.settings().max_compartment_rules);
        match compiled.refused.pop_first() {
            Some((_, reason)) => Err(error(reason)),
            None => Ok(compiled),
        }
    }

    /// Compiles what can be: a compartment that cannot be is kept out and named in
    /// [`CompiledCompartments::refused`], and grants nothing.
    fn compile_each(&self, lineage: &Lineage, max_rules: Limit) -> CompiledCompartments {
        let mut compiled = BTreeMap::new();
        let mut refused = BTreeMap::new();
        for compartment in self.iter() {
            match CompiledOne::build(compartment, max_rules) {
                Ok(one) => {
                    compiled.insert(compartment.id.clone(), one);
                }
                Err(reason) => {
                    refused.insert(compartment.id.clone(), reason);
                }
            }
        }
        CompiledCompartments {
            compiled,
            refused,
            lineage: lineage.clone(),
        }
    }
}

/// Where a question about access is asked: an Operation or one Mission of it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompartmentResource {
    /// An Operation.
    Operation(String),
    /// A Mission and the Operation it belongs to.
    Mission {
        /// The Mission.
        id: String,
        /// Its Operation; empty when the asker does not know it, so a cover of its Operation
        /// cannot match.
        operation: String,
    },
}

impl fmt::Display for CompartmentResource {
    /// `operation:<id>` or `mission:<id>`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operation(id) => write!(formatter, "operation:{id}"),
            Self::Mission { id, .. } => write!(formatter, "mission:{id}"),
        }
    }
}

/// A Session asking: the Station supplies all of it, and a request never names its own identity.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPrincipal {
    /// The Agent's codename.
    pub agent: String,
    /// The Operation the Session works in.
    pub operation: String,
    /// The Mission the Session works on.
    pub mission: Option<String>,
    /// The Agent's effective clearance now.
    pub clearance: String,
}

impl FieldPrincipal {
    /// A Session of `agent` in `operation` on `mission`, at `clearance`.
    pub fn new(
        agent: impl Into<String>,
        operation: impl Into<String>,
        mission: Option<String>,
        clearance: impl Into<String>,
    ) -> Self {
        Self {
            agent: agent.into(),
            operation: operation.into(),
            mission,
            clearance: clearance.into(),
        }
    }
}

/// A compartment as one Session holds it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldCompartment {
    /// Its ID.
    pub id: String,
    /// What it is for: configuration, not outside text.
    pub description: String,
    /// What it grants a Session.
    pub actions: Vec<CompartmentAction>,
    /// The Operations it reaches.
    pub operations: Vec<String>,
    /// The Missions it names.
    pub missions: Vec<String>,
}

struct CompiledOne {
    compartment: Compartment,
    people: RoleAuthorizer,
    field: RoleAuthorizer,
}

impl CompiledOne {
    fn build(compartment: &Compartment, max_rules: Limit) -> Result<Self, String> {
        let pairs = compartment
            .read_in
            .len()
            .saturating_mul(compartment.covers.len());
        if !max_rules.allows(pairs) {
            return Err(format!(
                "compartment {:?}: {pairs} rules is more than the {max_rules} max_compartment_rules allows",
                compartment.id
            ));
        }
        let mut actions = compartment.actions.clone();
        // Reading a brief means seeing its status.
        if actions.contains(&CompartmentAction::MissionRead) {
            actions.insert(CompartmentAction::MissionStatus);
        }
        let scopes: Vec<String> = compartment.covers.iter().map(cover_scope).collect();
        let build = |subjects: Vec<String>, keep: fn(CompartmentAction) -> bool| {
            let mut names: Vec<&str> = actions
                .iter()
                .copied()
                .filter(|action| keep(*action))
                .map(CompartmentAction::as_str)
                .collect();
            // A compartment of only cable.read grants a Session nothing, and is still valid.
            if names.is_empty() {
                names.push(NOTHING);
            }
            let mut builder = RoleAuthorizer::builder()
                .scope_kinds(SCOPE_KINDS)
                .role(ROLE, names)
                .limits(RoleLimits::UNBOUNDED);
            for subject in &subjects {
                for scope in &scopes {
                    builder = builder.rule(Rule::new(subject.clone(), scope.clone(), ROLE));
                }
            }
            builder
                .build()
                .map_err(|reason| format!("compartment {:?}: {reason}", compartment.id))
        };
        let (mut people, mut field) = (Vec::new(), Vec::new());
        for read_in in &compartment.read_in {
            let subject = read_in.subject.to_string();
            match &read_in.subject {
                ReadInSubject::Person(_) | ReadInSubject::Group(_) | ReadInSubject::Account(_) => {
                    people.push(subject);
                }
                ReadInSubject::Agent(agent) => field.push(format!("person:agent:{agent}")),
                ReadInSubject::Mission(mission) => field.push(format!("group:mission:{mission}")),
                ReadInSubject::Operation(operation) if read_in.propagate => {
                    field.push(format!("group:subtree:{operation}"));
                }
                ReadInSubject::Operation(operation) => {
                    field.push(format!("group:operation:{operation}"));
                }
            }
        }
        Ok(Self {
            compartment: compartment.clone(),
            people: build(people, CompartmentAction::for_people)?,
            field: build(field, CompartmentAction::for_agents)?,
        })
    }

    fn asks(
        authorizer: &RoleAuthorizer,
        person: &Person,
        action: CompartmentAction,
        resource: &CompartmentResource,
        lineage: &Lineage,
    ) -> bool {
        authorizer
            .authorize(request(person.clone(), action, resource, lineage))
            .is_ok_and(|decision| decision.allowed)
    }

    /// Whether the clearance floor lets a Session at `clearance` hold this. An unknown
    /// clearance counts as the lowest.
    fn clears(&self, clearance: &str) -> bool {
        let Some(floor) = &self.compartment.min_clearance else {
            return true;
        };
        let held = clearance_rank(clearance).unwrap_or(0);
        clearance_rank(floor).is_ok_and(|floor| held >= floor)
    }

    fn field_allows(
        &self,
        principal: &FieldPrincipal,
        session: &Person,
        action: CompartmentAction,
        resource: &CompartmentResource,
        lineage: &Lineage,
    ) -> bool {
        self.clears(&principal.clearance)
            && Self::asks(&self.field, session, action, resource, lineage)
    }

    /// Every resource the covers name, Operations expanded down the tree.
    fn resources(&self, lineage: &Lineage) -> Vec<CompartmentResource> {
        let mut found = Vec::new();
        for cover in &self.compartment.covers {
            match &cover.target {
                CoverTarget::Operation(id) if cover.propagate => found.extend(
                    lineage
                        .subtree(id)
                        .into_iter()
                        .map(|id| CompartmentResource::Operation(id.to_owned())),
                ),
                CoverTarget::Operation(id) => {
                    if lineage.contains(id) {
                        found.push(CompartmentResource::Operation(id.clone()));
                    }
                }
                CoverTarget::Mission(id) => found.push(CompartmentResource::Mission {
                    id: id.clone(),
                    operation: String::new(),
                }),
            }
        }
        found
    }
}

/// The scope a cover compiles to.
fn cover_scope(cover: &CompartmentCover) -> String {
    match &cover.target {
        CoverTarget::Operation(id) if cover.propagate => format!("subtree:{id}"),
        CoverTarget::Operation(id) => format!("operation:{id}"),
        CoverTarget::Mission(id) => format!("mission:{id}"),
    }
}

/// The request for `person` to take `action` on `resource`, naming what sits above it.
fn request(
    person: Person,
    action: CompartmentAction,
    resource: &CompartmentResource,
    lineage: &Lineage,
) -> AuthorizationRequest {
    let above = |operation: &str| {
        std::iter::once(format!("subtree:{operation}"))
            .chain(
                lineage
                    .ancestors(operation)
                    .iter()
                    .map(|ancestor| format!("subtree:{ancestor}")),
            )
            .collect::<Vec<_>>()
    };
    let (scope, ancestors) = match resource {
        CompartmentResource::Operation(id) => (format!("operation:{id}"), above(id)),
        CompartmentResource::Mission { id, operation } if operation.is_empty() => {
            (format!("mission:{id}"), Vec::new())
        }
        CompartmentResource::Mission { id, operation } => (format!("mission:{id}"), {
            let mut ancestors = vec![format!("operation:{operation}")];
            ancestors.extend(above(operation));
            ancestors
        }),
    };
    AuthorizationRequest {
        person,
        action: action.as_str().to_owned(),
        resource: scope,
        ancestors,
    }
}

/// The [`Person`] a Session asks as. Its groups are named by kind, so no person's group can be
/// taken for them: people and Sessions are asked of different authorizers.
fn session_person(principal: &FieldPrincipal, lineage: &Lineage) -> Person {
    let mut groups = Vec::new();
    if let Some(mission) = &principal.mission {
        groups.push(format!("mission:{mission}"));
    }
    groups.push(format!("operation:{}", principal.operation));
    groups.push(format!("subtree:{}", principal.operation));
    groups.extend(
        lineage
            .ancestors(&principal.operation)
            .iter()
            .map(|ancestor| format!("subtree:{ancestor}")),
    );
    Person {
        id: format!("agent:{}", principal.agent),
        name: principal.agent.clone(),
        groups,
        ..Person::default()
    }
}

/// Every compartment compiled, with the tree of Operations it was compiled against.
pub struct CompiledCompartments {
    compiled: BTreeMap<String, CompiledOne>,
    refused: BTreeMap<String, String>,
    lineage: Lineage,
}

impl fmt::Debug for CompiledCompartments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompiledCompartments")
            .field("compiled", &self.compiled.len())
            .field("refused", &self.refused.len())
            .finish_non_exhaustive()
    }
}

impl Default for CompiledCompartments {
    /// Nothing granted to anyone.
    fn default() -> Self {
        Self {
            compiled: BTreeMap::new(),
            refused: BTreeMap::new(),
            lineage: Lineage::default(),
        }
    }
}

impl CompiledCompartments {
    /// The first compartment, in ID order, that gives this Session `action` on `resource`;
    /// `None` for no. An unknown compartment, an unknown Operation and a floor above the
    /// Session's clearance all answer `None`.
    pub fn field_allows(
        &self,
        principal: &FieldPrincipal,
        action: CompartmentAction,
        resource: &CompartmentResource,
    ) -> Option<&str> {
        let session = session_person(principal, &self.lineage);
        self.compiled
            .iter()
            .find(|(_, one)| one.field_allows(principal, &session, action, resource, &self.lineage))
            .map(|(id, _)| id.as_str())
    }

    /// Whether compartment `id` gives this Session `action` on `resource`.
    pub fn field_allows_in(
        &self,
        id: &str,
        principal: &FieldPrincipal,
        action: CompartmentAction,
        resource: &CompartmentResource,
    ) -> bool {
        let session = session_person(principal, &self.lineage);
        self.compiled.get(id).is_some_and(|one| {
            one.field_allows(principal, &session, action, resource, &self.lineage)
        })
    }

    /// The first compartment, in ID order, that gives `person` `action` on `resource`.
    pub fn person_allows(
        &self,
        person: &Person,
        action: CompartmentAction,
        resource: &CompartmentResource,
    ) -> Option<&str> {
        self.compiled
            .iter()
            .find(|(_, one)| {
                CompiledOne::asks(&one.people, person, action, resource, &self.lineage)
            })
            .map(|(id, _)| id.as_str())
    }

    /// Whether compartment `id` gives `person` `action` on `resource`.
    pub fn person_allows_in(
        &self,
        id: &str,
        person: &Person,
        action: CompartmentAction,
        resource: &CompartmentResource,
    ) -> bool {
        self.compiled.get(id).is_some_and(|one| {
            CompiledOne::asks(&one.people, person, action, resource, &self.lineage)
        })
    }

    /// Whether `person` is named among those read into compartment `id`, whatever it covers or
    /// grants. An entry of the memory protected by a compartment asks this. A compartment that is
    /// unknown, or could not be read or compiled, reads nobody in.
    pub fn person_read_in(&self, id: &str, person: &Person) -> bool {
        self.compiled.get(id).is_some_and(|one| {
            one.compartment
                .read_in
                .iter()
                .any(|read_in| match &read_in.subject {
                    ReadInSubject::Person(subject) => *subject == person.id,
                    ReadInSubject::Group(subject) => person.groups.contains(subject),
                    ReadInSubject::Account(subject) => *subject == person.name,
                    _ => false,
                })
        })
    }

    /// Whether this Session is named among those read into compartment `id` and meets its
    /// clearance floor, whatever it covers or grants: as [`person_read_in`](Self::person_read_in).
    pub fn field_read_in(&self, id: &str, principal: &FieldPrincipal) -> bool {
        self.compiled.get(id).is_some_and(|one| {
            one.clears(&principal.clearance)
                && one
                    .compartment
                    .read_in
                    .iter()
                    .any(|read_in| match &read_in.subject {
                        ReadInSubject::Agent(agent) => *agent == principal.agent,
                        ReadInSubject::Mission(mission) => {
                            principal.mission.as_deref() == Some(mission.as_str())
                        }
                        ReadInSubject::Operation(operation) => {
                            *operation == principal.operation
                                || (read_in.propagate
                                    && self
                                        .lineage
                                        .ancestors(&principal.operation)
                                        .iter()
                                        .any(|ancestor| ancestor == operation))
                        }
                        _ => false,
                    })
        })
    }

    /// Whether `person` is read into compartment `id`, whatever it covers.
    pub fn person_holds(&self, id: &str, person: &Person) -> bool {
        self.compiled.get(id).is_some_and(|one| {
            one.resources(&self.lineage).iter().any(|resource| {
                CompartmentAction::ALL.into_iter().any(|action| {
                    CompiledOne::asks(&one.people, person, action, resource, &self.lineage)
                })
            })
        })
    }

    /// The compartments that give `person` something on `operation`, in ID order.
    pub fn person_compartments(&self, person: &Person, operation: &str) -> Vec<&str> {
        let resource = CompartmentResource::Operation(operation.to_owned());
        self.compiled
            .iter()
            .filter(|(_, one)| {
                CompartmentAction::ALL.into_iter().any(|action| {
                    CompiledOne::asks(&one.people, person, action, &resource, &self.lineage)
                })
            })
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// What a Session may reach with `action`: every covered Operation, expanded down the tree
    /// where the cover propagates, and every covered Mission of the compartments that allow it.
    pub fn reachable(
        &self,
        principal: &FieldPrincipal,
        action: CompartmentAction,
    ) -> Vec<CompartmentResource> {
        let session = session_person(principal, &self.lineage);
        let found: BTreeSet<CompartmentResource> = self
            .compiled
            .values()
            .flat_map(|one| {
                one.resources(&self.lineage)
                    .into_iter()
                    .filter(|resource| {
                        one.field_allows(principal, &session, action, resource, &self.lineage)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        found.into_iter().collect()
    }

    /// What a person may reach with `action`, as [`reachable`](Self::reachable).
    pub fn person_reachable(
        &self,
        person: &Person,
        action: CompartmentAction,
    ) -> Vec<CompartmentResource> {
        let found: BTreeSet<CompartmentResource> = self
            .compiled
            .values()
            .flat_map(|one| {
                one.resources(&self.lineage)
                    .into_iter()
                    .filter(|resource| {
                        CompiledOne::asks(&one.people, person, action, resource, &self.lineage)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        found.into_iter().collect()
    }

    /// The compartments this Session holds now, each with what it reaches.
    pub fn held(&self, principal: &FieldPrincipal) -> Vec<HeldCompartment> {
        let session = session_person(principal, &self.lineage);
        self.compiled
            .values()
            .filter_map(|one| {
                let resources = one.resources(&self.lineage);
                let actions: Vec<CompartmentAction> = CompartmentAction::ALL
                    .into_iter()
                    .filter(|action| {
                        resources.iter().any(|resource| {
                            one.field_allows(principal, &session, *action, resource, &self.lineage)
                        })
                    })
                    .collect();
                if actions.is_empty() {
                    return None;
                }
                let (mut operations, mut missions) = (Vec::new(), Vec::new());
                for resource in resources {
                    match resource {
                        CompartmentResource::Operation(id) => operations.push(id),
                        CompartmentResource::Mission { id, .. } => missions.push(id),
                    }
                }
                Some(HeldCompartment {
                    id: one.compartment.id.clone(),
                    description: one.compartment.description.clone(),
                    actions,
                    operations,
                    missions,
                })
            })
            .collect()
    }

    /// The compartment named `id`, if it compiled.
    pub fn get(&self, id: &str) -> Option<&Compartment> {
        self.compiled.get(id).map(|one| &one.compartment)
    }

    /// The compartments that compiled, in ID order.
    pub fn iter(&self) -> impl Iterator<Item = &Compartment> {
        self.compiled.values().map(|one| &one.compartment)
    }

    /// Each compartment that could not be read or compiled and grants nothing, with the reason.
    pub fn refused(&self) -> &BTreeMap<String, String> {
        &self.refused
    }

    /// The tree of Operations these answers use.
    pub fn lineage(&self) -> &Lineage {
        &self.lineage
    }
}

/// The compartments as `provider` holds them at this moment, read and compiled for one call and
/// never kept: a removed grant holds from the next call. One record that cannot be read or
/// compiled grants nothing and is named in [`CompiledCompartments::refused`]; the tree comes
/// from the same read, so an Operation moved out from under a cover is out at once.
///
/// # Errors
///
/// When the provider cannot be read. The caller refuses: no answer comes from memory.
pub fn current(provider: &dyn ConfigProvider) -> Result<CompiledCompartments, RosterError> {
    let scope = ConfigScope::Global;
    let held = match provider.definitions(ConfigRecord::Compartment, &scope) {
        Ok(definitions) => definitions.records,
        Err(reason) if reason.kind() == RosterErrorKind::ReadOnly => {
            // A provider built in code lists nothing: its snapshot is what it holds.
            let snapshot = provider.load("")?;
            let lineage = Lineage::from_roster(&snapshot.roster);
            return Ok(snapshot
                .settings
                .compartments
                .compile_each(&lineage, snapshot.roster.settings().max_compartment_rules));
        }
        Err(reason) => return Err(reason),
    };
    let operations = provider
        .definitions(ConfigRecord::Operation, &scope)?
        .records;
    let settings = provider.definitions(ConfigRecord::Roster, &scope)?.records;
    let max_rules = settings
        .first()
        .and_then(|(_, definition)| records::from_json::<RosterSettings>(definition).ok())
        .unwrap_or_default()
        .max_compartment_rules;
    let lineage = lineage_of(&operations);
    let mut compartments = BTreeMap::new();
    let mut refused = BTreeMap::new();
    for (id, definition) in held {
        match Compartment::from_definition(id.clone(), &definition) {
            Ok(compartment) => {
                compartments.insert(id, compartment);
            }
            Err(reason) => {
                refused.insert(id, reason.to_string());
            }
        }
    }
    let mut compiled = Compartments {
        records: compartments,
    }
    .compile_each(&lineage, max_rules);
    compiled.refused.extend(refused);
    Ok(compiled)
}

/// The tree the `parent` keys of Operation records make. A missing parent ends a lineage and a
/// cycle stops at the first repeat: such a record never passes a provider's checks, and an answer
/// here only has to stay narrow.
fn lineage_of(operations: &[(String, serde_json::Value)]) -> Lineage {
    let parents: BTreeMap<&str, &str> = operations
        .iter()
        .filter_map(|(id, definition)| {
            definition
                .get("parent")
                .and_then(serde_json::Value::as_str)
                .map(|parent| (id.as_str(), parent))
        })
        .collect();
    let known: BTreeSet<&str> = operations.iter().map(|(id, _)| id.as_str()).collect();
    Lineage::new(operations.iter().map(|(id, _)| {
        let mut ancestors: Vec<String> = Vec::new();
        let mut at = id.as_str();
        while let Some(parent) = parents.get(at).copied() {
            if !known.contains(parent)
                || parent == id
                || ancestors.iter().any(|seen| seen == parent)
            {
                break;
            }
            ancestors.push(parent.to_owned());
            at = parent;
        }
        (id.clone(), ancestors)
    }))
}

#[cfg(test)]
mod tests;
