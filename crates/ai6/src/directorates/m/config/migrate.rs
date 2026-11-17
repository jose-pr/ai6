//! Record migrations: a family's stored record in a shape an earlier release wrote, read as the
//! shape this release declares.
//!
//! When a release moves the fields a family declares, it adds a [`Migration`] here. A record is
//! migrated each time it is read, so a database or file nobody has rewritten keeps loading; the
//! snapshot names what was migrated, and saving the record stores it in its new shape.

use serde_json::{Map, Value};

use super::{ConfigRecord, Layer};

/// A record that was read in an older shape, and the migration that made it readable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedRecord {
    /// The family's name, as in [`ConfigRecord::name`].
    pub family: String,
    /// The record's key.
    pub key: String,
    /// What moved, in a sentence.
    pub migration: String,
}

/// One change of a family's declared fields, applied to a stored record that still has the old
/// shape.
struct Migration {
    family: ConfigRecord,
    /// What moved, as the journal says it.
    says: &'static str,
    /// Rewrites the record; `true` when it had the old shape.
    apply: fn(&mut Value) -> bool,
}

/// Every migration, oldest first. A new one goes at the end.
const MIGRATIONS: &[Migration] = &[Migration {
    family: ConfigRecord::Operation,
    says: "workspace.base, branch, isolate, checkpoint_remote and checkpoint_secs moved under \
           workspace.settings",
    apply: workspace_settings,
}];

/// The workspace keys the chosen safe house owns now, which an Operation once declared itself.
const SAFE_HOUSE_KEYS: [&str; 5] = [
    "base",
    "branch",
    "isolate",
    "checkpoint_remote",
    "checkpoint_secs",
];

fn workspace_settings(definition: &mut Value) -> bool {
    let Some(workspace) = definition
        .get_mut("workspace")
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    if !SAFE_HOUSE_KEYS
        .iter()
        .any(|key| workspace.contains_key(*key))
    {
        return false;
    }
    let mut settings = match workspace.get("settings") {
        None => Map::new(),
        Some(Value::Object(settings)) => settings.clone(),
        // Not a table: the record is refused as it stands, not guessed at.
        Some(_) => return false,
    };
    for key in SAFE_HOUSE_KEYS {
        if let Some(value) = workspace.remove(key) {
            settings.entry(key).or_insert(value);
        }
    }
    workspace.insert("settings".to_owned(), Value::Object(settings));
    true
}

/// `view` with every migration applied to the records that need one, and which those were;
/// `None` when no record needed any.
pub(super) fn migrated(view: &Layer) -> Option<(Layer, Vec<MigratedRecord>)> {
    let mut changed = Vec::new();
    let mut layer = view.clone();
    for migration in MIGRATIONS {
        let Some(records) = layer.get_mut(&migration.family) else {
            continue;
        };
        for (key, definition) in records.iter_mut() {
            if (migration.apply)(definition) {
                changed.push(MigratedRecord {
                    family: migration.family.name().to_owned(),
                    key: key.clone(),
                    migration: migration.says.to_owned(),
                });
            }
        }
    }
    (!changed.is_empty()).then_some((layer, changed))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn operation(definition: Value) -> Layer {
        Layer::from([(
            ConfigRecord::Operation,
            [("g".to_owned(), definition)].into(),
        )])
    }

    #[test]
    fn a_record_with_the_workspace_fields_at_the_top_has_them_moved_under_settings() {
        let old = operation(json!({
            "workspace": {"provider": "git", "base": "main", "branch": "m/{id}", "isolate": true}
        }));
        let (layer, changed) = migrated(&old).expect("it needs a migration");
        assert_eq!(
            layer[&ConfigRecord::Operation]["g"],
            json!({"workspace": {
                "provider": "git",
                "settings": {"base": "main", "branch": "m/{id}", "isolate": true}
            }})
        );
        assert_eq!(changed.len(), 1);
        assert_eq!(
            (changed[0].family.as_str(), changed[0].key.as_str()),
            ("operation", "g")
        );
    }

    #[test]
    fn the_new_shape_and_a_setting_already_there_are_left_as_they_are() {
        let current = operation(json!({"workspace": {"settings": {"base": "main"}}}));
        assert!(migrated(&current).is_none());
        let both = operation(json!({
            "workspace": {"base": "old", "settings": {"base": "kept"}}
        }));
        let (layer, _) = migrated(&both).unwrap();
        assert_eq!(
            layer[&ConfigRecord::Operation]["g"]["workspace"]["settings"]["base"],
            "kept"
        );
        let broken = operation(json!({"workspace": {"base": "x", "settings": 3}}));
        assert!(migrated(&broken).is_none(), "refused as it stands");
    }
}
