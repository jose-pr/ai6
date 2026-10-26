//! Exposure: which Sessions have read input an attacker could have written.
//!
//! A Session is marked the moment it reads such input and stays marked; a later attempt at
//! the same Mission inherits the mark. Marking is always on. What a marked Session may then
//! do is the Operation's [`ExposurePolicy`]: the `strict` preset (the default) restricts it,
//! `observe` only records it, and every restriction can be chosen individually.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// The source of a Session whose Mission came from a Source item.
pub const BRIEF_SOURCE: &str = "brief:source";
/// The source of a Session whose Mission came from a brief dropped into the Registry's queue
/// with no provenance.
pub const BRIEF_REGISTRY_DROP: &str = "brief:registry_drop";
/// The source of a Session that read review text written by someone else.
pub const REVIEW: &str = "review";
/// The source of a Head of Station round that read its Station's report, which quotes what
/// other Missions' Agents wrote (commit subjects, branch names).
pub const STATION_REPORT: &str = "station:report";
/// The source of a Head of Station round given its Station's logbook: reports earlier rounds
/// wrote after reading other Missions' text.
pub const STATION_LOGBOOK: &str = "station:logbook";
/// The Brief `data` key under which a proposal carries its parent's exposure sources.
pub const BRIEF_EXPOSURE_KEY: &str = "exposure";

/// Tools whose results are untrusted unless the Operation lists others.
pub const DEFAULT_EXPOSING_TOOLS: [&str; 2] = ["WebFetch", "WebSearch"];

const MAX_EXPOSURES: usize = 32;
const MAX_SOURCE_BYTES: usize = 128;

/// The source for a Session that read a Cable from `sender`.
pub fn cable_source(sender: &str) -> String {
    bounded(format!("cable:{sender}"))
}

/// The source for a Session whose tool call named `tool`.
pub fn tool_source(tool: &str) -> String {
    bounded(format!("tool:{tool}"))
}

fn bounded(mut source: String) -> String {
    if source.len() > MAX_SOURCE_BYTES {
        let mut end = MAX_SOURCE_BYTES;
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        source.truncate(end);
    }
    source
}

/// One way a Session was exposed, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exposure {
    /// `brief:source`, `cable:SENDER`, `tool:NAME` or `review`.
    pub source: String,
    /// When it happened, as Unix seconds.
    pub at: f64,
}

/// What an exposed Session may not do.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Restriction {
    /// Its Mission proposals are marked exposed, so they are never auto-promoted.
    Proposals,
    /// Its requests for authorization always go to a human.
    Approvals,
    /// It may send a Cable only as a reply inside an existing conversation.
    Cables,
    /// It is given no new secret leases.
    Secrets,
}

impl Restriction {
    /// Every restriction.
    pub const ALL: [Self; 4] = [
        Self::Proposals,
        Self::Approvals,
        Self::Cables,
        Self::Secrets,
    ];

    /// The configuration spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposals => "proposals",
            Self::Approvals => "approvals",
            Self::Cables => "cables",
            Self::Secrets => "secrets",
        }
    }
}

/// Named sets of restrictions.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExposurePreset {
    /// Every restriction applies. The default.
    Strict,
    /// Exposure is recorded and nothing is restricted.
    Observe,
}

/// An Operation's response to exposure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposurePolicy {
    restrictions: BTreeSet<Restriction>,
    tools: Vec<String>,
}

impl Default for ExposurePolicy {
    fn default() -> Self {
        Self::preset(ExposurePreset::Strict)
    }
}

impl ExposurePolicy {
    /// The policy of a named preset, with the default exposing tools.
    pub fn preset(preset: ExposurePreset) -> Self {
        Self {
            restrictions: match preset {
                ExposurePreset::Strict => Restriction::ALL.into_iter().collect(),
                ExposurePreset::Observe => BTreeSet::new(),
            },
            tools: DEFAULT_EXPOSING_TOOLS.map(str::to_owned).to_vec(),
        }
    }

    /// Whether an exposed Session is held to `restriction`.
    pub fn restricts(&self, restriction: Restriction) -> bool {
        self.restrictions.contains(&restriction)
    }

    /// The restrictions in force, in a stable order.
    pub fn restrictions(&self) -> impl Iterator<Item = Restriction> + '_ {
        self.restrictions.iter().copied()
    }

    /// The tools whose use exposes a Session.
    pub fn tools(&self) -> &[String] {
        &self.tools
    }

    /// Whether a Session that called `tool` is exposed by it.
    pub fn exposed_by_tool(&self, tool: &str) -> bool {
        self.tools.iter().any(|listed| listed == tool)
    }
}

/// The exposure settings as written in configuration (`[operation.ID.exposure]`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExposureDefinition {
    preset: Option<ExposurePreset>,
    restrict: Option<Vec<String>>,
    tools: Option<Vec<String>>,
}

impl ExposureDefinition {
    /// Validates the written values and applies them over the preset.
    ///
    /// # Errors
    ///
    /// Returns a message naming the key when `restrict` lists an unknown restriction or
    /// `tools` holds an empty name.
    pub fn resolve(&self) -> Result<ExposurePolicy, String> {
        let mut policy = ExposurePolicy::preset(self.preset.unwrap_or(ExposurePreset::Strict));
        if let Some(names) = &self.restrict {
            policy.restrictions = names
                .iter()
                .map(|name| {
                    Restriction::ALL
                        .into_iter()
                        .find(|restriction| restriction.as_str() == name)
                        .ok_or_else(|| {
                            format!(
                                "exposure restrict has unknown restriction {name:?}; use proposals, approvals, cables or secrets"
                            )
                        })
                })
                .collect::<Result<_, _>>()?;
        }
        if let Some(tools) = &self.tools {
            if tools.iter().any(|tool| tool.trim().is_empty()) {
                return Err("exposure tools must not hold an empty name".to_owned());
            }
            policy.tools.clone_from(tools);
        }
        Ok(policy)
    }
}

/// The live exposure of one Session, shared between the Station and the Session's tools.
///
/// The Station syncs changes to the Session's Registry record; the Cable tools consult the
/// gate before acting.
#[derive(Debug, Clone, Default)]
pub struct ExposureGate(Arc<GateInner>);

#[derive(Debug, Default)]
struct GateInner {
    policy: ExposurePolicy,
    exposures: Mutex<Vec<Exposure>>,
    changed: AtomicBool,
}

impl ExposureGate {
    /// A gate under `policy`, starting from the exposures a Session inherits.
    pub fn new(policy: ExposurePolicy, inherited: Vec<Exposure>) -> Self {
        let gate = Self(Arc::new(GateInner {
            policy,
            ..GateInner::default()
        }));
        for exposure in inherited {
            gate.mark(&exposure.source, exposure.at);
        }
        gate.0.changed.store(false, Ordering::Release);
        gate
    }

    /// The policy this gate applies.
    pub fn policy(&self) -> &ExposurePolicy {
        &self.0.policy
    }

    /// Marks the Session exposed by `source` at `at`; `false` when it already was.
    pub fn mark(&self, source: &str, at: f64) -> bool {
        let Ok(mut exposures) = self.0.exposures.lock() else {
            return false;
        };
        if exposures.iter().any(|known| known.source == source) || exposures.len() >= MAX_EXPOSURES
        {
            return false;
        }
        exposures.push(Exposure {
            source: bounded(source.to_owned()),
            at,
        });
        self.0.changed.store(true, Ordering::Release);
        true
    }

    /// Every way the Session was exposed, in the order it happened.
    pub fn exposures(&self) -> Vec<Exposure> {
        self.0
            .exposures
            .lock()
            .map(|exposures| exposures.clone())
            .unwrap_or_default()
    }

    /// Whether the Session has read untrusted input.
    pub fn is_exposed(&self) -> bool {
        self.0
            .exposures
            .lock()
            .is_ok_and(|exposures| !exposures.is_empty())
    }

    /// Whether the Session is exposed and the policy holds it to `restriction`.
    pub fn restricts(&self, restriction: Restriction) -> bool {
        self.0.policy.restricts(restriction) && self.is_exposed()
    }

    /// The exposures, once, after they changed since the last call.
    pub fn take_changes(&self) -> Option<Vec<Exposure>> {
        self.0
            .changed
            .swap(false, Ordering::AcqRel)
            .then(|| self.exposures())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marking_is_sticky_and_reports_only_new_sources() {
        let gate = ExposureGate::default();
        assert!(!gate.is_exposed());
        assert!(gate.mark("tool:WebFetch", 10.0));
        assert!(!gate.mark("tool:WebFetch", 20.0));
        assert!(gate.mark(&cable_source("007"), 30.0));
        let sources: Vec<_> = gate.exposures().into_iter().map(|e| e.source).collect();
        assert_eq!(sources, ["tool:WebFetch", "cable:007"]);
        assert_eq!(gate.exposures()[0].at, 10.0);
        assert_eq!(gate.take_changes().map(|list| list.len()), Some(2));
        assert_eq!(gate.take_changes(), None);
    }

    #[test]
    fn an_inherited_exposure_is_not_a_change_and_the_default_policy_is_strict() {
        let inherited = vec![Exposure {
            source: BRIEF_SOURCE.to_owned(),
            at: 1.0,
        }];
        let gate = ExposureGate::new(ExposurePolicy::default(), inherited);
        assert!(gate.is_exposed());
        assert_eq!(gate.take_changes(), None);
        assert!(Restriction::ALL.into_iter().all(|r| gate.restricts(r)));
        let observe = ExposureGate::new(
            ExposurePolicy::preset(ExposurePreset::Observe),
            gate.exposures(),
        );
        assert!(observe.is_exposed());
        assert!(!observe.restricts(Restriction::Cables));
    }

    #[test]
    fn nothing_is_restricted_until_the_session_is_exposed() {
        let gate = ExposureGate::default();
        assert!(!gate.restricts(Restriction::Cables));
        gate.mark(REVIEW, 1.0);
        assert!(gate.restricts(Restriction::Cables));
    }

    #[test]
    fn configuration_picks_a_preset_then_overrides_restrictions_and_tools() {
        let strict: ExposureDefinition = toml::from_str("").unwrap();
        let policy = strict.resolve().unwrap();
        assert!(policy.restricts(Restriction::Secrets));
        assert!(policy.exposed_by_tool("WebSearch"));
        assert!(!policy.exposed_by_tool("Read"));

        let custom: ExposureDefinition = toml::from_str(
            "preset = \"observe\"\nrestrict = [\"cables\"]\ntools = [\"WebFetch\", \"Browse\"]\n",
        )
        .unwrap();
        let policy = custom.resolve().unwrap();
        assert!(policy.restricts(Restriction::Cables));
        assert!(!policy.restricts(Restriction::Proposals));
        assert!(policy.exposed_by_tool("Browse") && !policy.exposed_by_tool("WebSearch"));

        for bad in ["restrict = [\"everything\"]", "tools = [\"\"]"] {
            let definition: ExposureDefinition = toml::from_str(bad).unwrap();
            assert!(definition.resolve().is_err(), "{bad}");
        }
    }
}
