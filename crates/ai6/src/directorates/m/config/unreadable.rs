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
    let fragments = definitions(view, ConfigRecord::Fragment);
    leave_out(
        ConfigRecord::Fragment,
        roster::unreadable_fragments(&fragments),
    );
    leave_out(
        ConfigRecord::Role,
        roster::unreadable_roles(
            &definitions(view, ConfigRecord::Role),
            &roster::fragment_texts(&fragments),
            source,
        ),
    );
    // What names a refused record goes with it, and what names that in turn: the rest would be
    // refused for them, and a start must not exit on a stored record. A record names another as
    // the family's convention has it (`places_naming`), a charge by its `operation:<id>` key, and
    // an Operation's access record by its key.
    let mut frontier: Vec<(ConfigRecord, String)> = refused
        .iter()
        .filter_map(|record| {
            ConfigRecord::from_name(&record.family).map(|family| (family, record.key.clone()))
        })
        .collect();
    while let Some((family, key)) = frontier.pop() {
        for other in ConfigRecord::ALL {
            if other.is_singleton() || other.is_whole_file() {
                continue;
            }
            let Some(records) = kept.get(&other) else {
                continue;
            };
            let dependents: Vec<String> = records
                .iter()
                .filter(|(name, definition)| {
                    if other == family && **name == key {
                        return false;
                    }
                    let keyed = family == ConfigRecord::Operation
                        && match other {
                            ConfigRecord::Charge => **name == format!("operation:{key}"),
                            ConfigRecord::OperationAccess => **name == key,
                            _ => false,
                        };
                    let mut places = Vec::new();
                    if !family.is_singleton() && !family.is_whole_file() {
                        super::places_naming(
                            definition,
                            &key,
                            "",
                            &super::NOT_RECORD_NAMES,
                            &mut places,
                        );
                    }
                    keyed || !places.is_empty()
                })
                .map(|(name, _)| name.clone())
                .collect();
            for name in dependents {
                if let Some(records) = kept.get_mut(&other) {
                    records.remove(&name);
                }
                refused.push(RefusedRecord {
                    family: other.name().to_owned(),
                    key: name.clone(),
                    reason: format!("it names {} {key:?}, which is refused", family.name()),
                });
                frontier.push((other, name));
            }
        }
    }
    (kept, refused)
}
