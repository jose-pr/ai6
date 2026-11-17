//! Records a release can no longer read. A provider hands out the configuration without them and
//! names each, so one stored record that went stale costs that record and not the process.

use std::fmt;

use serde_json::Value;

use super::{ConfigRecord, Layer};
use crate::directorates::m::roster;

/// A stored record left out of a snapshot because it cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedRecord {
    /// The family's name, as in [`ConfigRecord::name`].
    pub family: String,
    /// The record's key.
    pub key: String,
    /// Why it was refused: the key or position it fails at, never its values.
    pub reason: String,
}

impl fmt::Display for RefusedRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {:?}: {}", self.family, self.key, self.reason)
    }
}

fn definitions(view: &Layer, record: ConfigRecord) -> Vec<(String, Value)> {
    view.get(&record)
        .map(|records| {
            records
                .iter()
                .map(|(key, definition)| (key.clone(), definition.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// `view` without the Operation, Agent and role records that cannot be read, and the charges and
/// access records of an Operation left out; and what was left out.
pub(super) fn without_unreadable(
    view: &Layer,
    source: &dyn fmt::Display,
) -> (Layer, Vec<RefusedRecord>) {
    let mut kept = view.clone();
    let mut refused = Vec::new();
    let mut leave_out = |record: ConfigRecord, found: Vec<(String, String)>| {
        for (key, reason) in found {
            if let Some(records) = kept.get_mut(&record) {
                records.remove(&key);
            }
            refused.push(RefusedRecord {
                family: record.name().to_owned(),
                key,
                reason,
            });
        }
    };
    let settings = definitions(view, ConfigRecord::Roster)
        .into_iter()
        .next()
        .map(|(_, definition)| definition);
    if let Ok(settings) = roster::roster_settings(settings.as_ref(), source) {
        leave_out(
            ConfigRecord::Operation,
            roster::unreadable_operations(
                &definitions(view, ConfigRecord::Operation),
                &settings,
                source,
            ),
        );
    }
    leave_out(
        ConfigRecord::Agent,
        roster::unreadable_agents(&definitions(view, ConfigRecord::Agent)),
    );
    leave_out(
        ConfigRecord::Role,
        roster::unreadable_roles(&definitions(view, ConfigRecord::Role), source),
    );
    // What belongs to a refused Operation goes with it, or the rest would be refused for it.
    let operations: Vec<String> = refused
        .iter()
        .filter(|record| record.family == ConfigRecord::Operation.name())
        .map(|record| record.key.clone())
        .collect();
    for operation in operations {
        let reason = format!("its Operation {operation:?} is refused");
        let scope = format!("operation:{operation}");
        for (record, key) in [
            (ConfigRecord::Charge, scope.as_str()),
            (ConfigRecord::OperationAccess, operation.as_str()),
        ] {
            if kept
                .get_mut(&record)
                .is_some_and(|records| records.remove(key).is_some())
            {
                refused.push(RefusedRecord {
                    family: record.name().to_owned(),
                    key: key.to_owned(),
                    reason: reason.clone(),
                });
            }
        }
    }
    (kept, refused)
}
