//! The Station's side of a Session's memory tools.
//!
//! An Agent reads its own Operation's memory and the global one, and another Operation's through a
//! compartment granting `memory.read`, which is written to the audit log before anything is read.
//! It reads no person's. Its write is a proposal a person decides, unless the Operation lets its
//! clearance write (`memory_write`); it proposes to the global memory and to no other Operation's.
//!
//! Every call asks the configuration what is granted now, so a read out holds from the next call,
//! and a call works from the Session as the Station started it, not its provider Session ID, which
//! a harness reports only on its stream. What it reads is fenced as data, and an entry written by
//! an exposed Session exposes this one.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{Value, json};
use uuid::Uuid;

use super::compartments::Reader;
use super::{DaemonState, agents};
use crate::bounds::Limit;
use crate::directorates::m::compartments::{
    CompartmentAction, CompartmentResource, CompiledCompartments, FieldPrincipal,
};
use crate::directorates::m::memories::{
    self, GLOBAL_RESOURCE, MemoryCaller, MemoryChange, MemoryForgotten, MemoryOutcome,
    MemoryRefusal, MemoryTarget, MemoryWriteRight, bounded, entry_json, fenced, header_json,
    proposal_json,
};
use crate::directorates::m::roster::clearance_rank;
use crate::directorates::registry::memories::MemoryScope;
use crate::directorates::registry::store::AuditRecord;

/// What a call answers when who is asking cannot be settled.
const UNAVAILABLE: &str = "memory is unavailable";

/// What an audit record's text may hold.
const AUDIT_BYTES: Limit = Limit::at(512);

/// A Session, asking of the memory.
struct SessionCaller<'a> {
    state: &'a DaemonState,
    reader: &'a Reader,
    principal: FieldPrincipal,
    /// What is granted now; `None` when it cannot be read, which grants nothing.
    compiled: Option<CompiledCompartments>,
    /// Each crossing asked of this call, so one is audited once.
    crossed: Mutex<BTreeMap<String, bool>>,
}

impl SessionCaller<'_> {
    /// Whether a compartment lets it read `operation`, written to the audit log first.
    fn crosses_into(&self, operation: &str) -> bool {
        let mut crossed = self
            .crossed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(known) = crossed.get(operation) {
            return *known;
        }
        let resource = CompartmentResource::Operation(operation.to_owned());
        let through = self.compiled.as_ref().and_then(|compiled| {
            compiled.field_allows(&self.principal, CompartmentAction::MemoryRead, &resource)
        });
        let allowed = through.is_some_and(|compartment| {
            self.record(
                "compartment.read",
                self.reader.operation.clone(),
                Some(format!("operation:{operation}")),
                &format!("{compartment} memory.read"),
            )
            .is_ok()
        });
        crossed.insert(operation.to_owned(), allowed);
        allowed
    }

    fn record(
        &self,
        action: &str,
        operation: String,
        resource: Option<String>,
        target: &str,
    ) -> Result<(), String> {
        self.state
            .intake
            .record_audit(&AuditRecord {
                id: format!("memory-{}", Uuid::new_v4().simple()),
                actor: format!("agent:{}", self.reader.agent),
                action: action.to_owned(),
                operation,
                resource,
                target: bounded(target, AUDIT_BYTES),
                at: self.now(),
            })
            .map_err(|_| "it could not be audited, so it was not done".to_owned())
    }
}

impl MemoryCaller for SessionCaller<'_> {
    fn by(&self) -> String {
        format!("agent:{}", self.reader.agent)
    }

    fn session(&self) -> Option<String> {
        self.reader.mission.clone()
    }

    fn reads_scope(&self, scope: &MemoryScope) -> bool {
        match scope {
            MemoryScope::Global => true,
            MemoryScope::Operation(operation) if *operation == self.reader.operation => true,
            MemoryScope::Operation(operation) => self.crosses_into(operation),
            _ => false,
        }
    }

    fn writes_scope(&self, scope: &MemoryScope) -> MemoryWriteRight {
        match scope {
            MemoryScope::Global => MemoryWriteRight::Propose,
            MemoryScope::Operation(operation) if *operation == self.reader.operation => {
                let allowed = self
                    .state
                    .roster
                    .operation(operation)
                    .ok()
                    .and_then(|operation| operation.memory_write.clone())
                    .is_some_and(|floor| {
                        let held = clearance_rank(&self.principal.clearance).unwrap_or(0);
                        clearance_rank(&floor).is_ok_and(|floor| held >= floor)
                    });
                if allowed {
                    MemoryWriteRight::Write
                } else {
                    MemoryWriteRight::Propose
                }
            }
            _ => MemoryWriteRight::Refused,
        }
    }

    fn read_in(&self, compartment: &str) -> bool {
        self.compiled
            .as_ref()
            .is_some_and(|compiled| compiled.field_read_in(compartment, &self.principal))
    }

    fn sees_all_compartments(&self) -> bool {
        false
    }

    fn acts_in_person(&self) -> bool {
        false
    }

    fn exposures(&self) -> Vec<String> {
        self.reader
            .exposure
            .exposures()
            .into_iter()
            .map(|exposure| exposure.source)
            .collect()
    }

    fn saw_marked(&self, source: &str) {
        self.reader.exposure.mark(source, self.now());
    }

    fn mark_bytes(&self) -> Limit {
        self.state.intake.limits().sessions.exposure_source_bytes
    }

    fn now(&self) -> f64 {
        self.state.clock.now() as f64
    }

    fn audit(&self, action: &str, scope: &MemoryScope, target: &str) -> Result<(), String> {
        let (operation, resource) = match scope {
            MemoryScope::Operation(operation) => (operation.clone(), None),
            _ => (
                self.reader.operation.clone(),
                Some(GLOBAL_RESOURCE.to_owned()),
            ),
        };
        self.record(action, operation, resource, target)
    }
}

/// A Session's way to its memory.
#[derive(Clone)]
pub(super) struct MemoryDesk {
    state: DaemonState,
}

impl MemoryDesk {
    pub(super) fn new(state: DaemonState) -> Self {
        Self { state }
    }

    /// Who `reader` is now and what is granted now, for `work`. An Agent off the roster holds
    /// nothing, and neither does anyone when the grants cannot be read.
    fn with_caller(
        &self,
        reader: &Reader,
        work: impl FnOnce(&DaemonState, &SessionCaller<'_>) -> Result<Value, MemoryRefusal>,
    ) -> Result<Value, String> {
        let state = self.state.current();
        let clearance = agents::effective(&state, Some(&reader.operation), &reader.agent)
            .map_err(|_| UNAVAILABLE.to_owned())?
            .clearance;
        let caller = SessionCaller {
            state: &state,
            reader,
            principal: FieldPrincipal::new(
                reader.agent.clone(),
                reader.operation.clone(),
                reader.mission.clone(),
                clearance,
            ),
            compiled: state.config.compartments().ok(),
            crossed: Mutex::default(),
        };
        work(&state, &caller).map_err(|refusal| refusal.to_string())
    }

    /// The scopes it asked for, or every one it might read.
    fn scopes(&self, reader: &Reader, scope: Option<&str>) -> Result<Vec<MemoryScope>, String> {
        if let Some(scope) = scope {
            return MemoryScope::parse(scope)
                .map(|scope| vec![scope])
                .ok_or_else(|| format!("{scope:?} is not a memory scope: operation:ID or global"));
        }
        let mut scopes = vec![
            MemoryScope::Operation(reader.operation.clone()),
            MemoryScope::Global,
        ];
        let state = self.state.current();
        if let (Ok(compiled), Ok(agent)) = (
            state.config.compartments(),
            agents::effective(&state, Some(&reader.operation), &reader.agent),
        ) {
            let principal = FieldPrincipal::new(
                reader.agent.clone(),
                reader.operation.clone(),
                reader.mission.clone(),
                agent.clearance,
            );
            for reachable in compiled.reachable(&principal, CompartmentAction::MemoryRead) {
                if let CompartmentResource::Operation(operation) = reachable
                    && operation != reader.operation
                {
                    scopes.push(MemoryScope::Operation(operation));
                }
            }
        }
        Ok(scopes)
    }

    /// `memory_search`: IDs, names and summaries, best first, fenced.
    pub(super) fn search(
        &self,
        reader: &Reader,
        query: &str,
        scope: Option<&str>,
    ) -> Result<String, String> {
        let scopes = self.scopes(reader, scope)?;
        self.with_caller(reader, |state, caller| {
            let limits = state.intake.limits().memories.clone();
            let found =
                memories::search_entries(state.intake.as_ref(), caller, &scopes, query, &limits)?;
            Ok(json!({
                "entries": found.hits.iter().map(header_json).collect::<Vec<Value>>(),
                "total": found.total,
            }))
        })
        .map(|value| fenced("memory_search", &value))
    }

    /// `memory_read`: the entry and its content, fenced.
    pub(super) fn read(&self, reader: &Reader, id: &str) -> Result<String, String> {
        self.with_caller(reader, |state, caller| {
            memories::read_entry(state.intake.as_ref(), caller, id).map(|entry| entry_json(&entry))
        })
        .map(|value| fenced("memory_read", &value))
    }

    /// `memory_write`: kept at once where the Operation allows it, else a proposal.
    pub(super) fn write(&self, reader: &Reader, change: MemoryChange) -> Result<String, String> {
        self.with_caller(reader, |state, caller| {
            Ok(
                match memories::write_entry(state.intake.as_ref(), caller, change)? {
                    MemoryOutcome::Kept(written) => json!({
                        "status": "kept",
                        "id": written.entry.header.id,
                        "revision": written.entry.header.revision,
                    }),
                    MemoryOutcome::Proposed(proposal) => json!({
                        "status": "proposed",
                        "proposal": proposal_json(&proposal)["id"],
                        "note": "A person decides; nothing has changed yet.",
                    }),
                },
            )
        })
        .map(|value| value.to_string())
    }

    /// `memory_forget`: as a write.
    pub(super) fn forget(
        &self,
        reader: &Reader,
        id: &str,
        revision: Option<u64>,
    ) -> Result<String, String> {
        self.with_caller(reader, |state, caller| {
            Ok(
                match memories::forget_entry(state.intake.as_ref(), caller, id, revision)? {
                    MemoryForgotten::Forgotten(entry) => json!({
                        "status": "forgotten",
                        "id": entry.header.id,
                    }),
                    MemoryForgotten::Proposed(proposal) => json!({
                        "status": "proposed",
                        "proposal": proposal.id,
                        "note": "A person decides; nothing has changed yet.",
                    }),
                },
            )
        })
        .map(|value| value.to_string())
    }
}

/// What a tool's target is, from its arguments.
pub(super) fn target_of(
    reader: &Reader,
    id: Option<String>,
    revision: Option<u64>,
    scope: Option<&str>,
) -> Result<MemoryTarget, String> {
    match (id, revision, scope) {
        (Some(id), Some(revision), None) => Ok(MemoryTarget::Existing { id, revision }),
        (Some(_), None, _) => Err("a change names the revision it read".to_owned()),
        (Some(_), _, Some(_)) => Err("an entry's scope never changes".to_owned()),
        (None, _, None) => Ok(MemoryTarget::New(MemoryScope::Operation(
            reader.operation.clone(),
        ))),
        (None, _, Some(scope)) => MemoryScope::parse(scope)
            .map(MemoryTarget::New)
            .ok_or_else(|| format!("{scope:?} is not a memory scope: operation:ID or global")),
    }
}

#[cfg(test)]
mod tests;
