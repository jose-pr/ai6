//! The providers a host reports: what each serves and the schema and form of its configuration.

use serde::{Deserialize, Serialize};

/// One loaded provider as a Station reports it: what it serves, and the schema and form of its
/// configuration, so a person can configure it without the app knowing its kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderEntry {
    /// The provider ID.
    pub id: String,
    /// A human-readable provider name.
    pub name: String,
    /// The provider's version.
    pub version: String,
    /// Every Guatiao kind it serves, such as `ai6_agent_handler`.
    pub kinds: Vec<String>,
    /// The `sha256:HEX` digest of the library it came from, when the scan read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    /// Why the provider is unavailable on this host, if it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// The JSON Schema of its configuration; absent when it takes none, or when the catalog was
    /// too large to carry it (`schema_omitted`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    /// Guatiao's form over that schema: the one it declares, else the one the schema implies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<serde_json::Value>,
    /// Whether the schema and form were left out to keep the catalog within its budget.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub schema_omitted: bool,
    /// `sha256:HEX` of this entry's schema and form, to tell a changed provider without reading
    /// them.
    pub digest: String,
}

impl ProviderEntry {
    /// The entry without its schema and form.
    #[must_use]
    pub fn without_schema(mut self) -> Self {
        self.schema_omitted = self.schema.is_some() || self.form.is_some() || self.schema_omitted;
        self.schema = None;
        self.form = None;
        self
    }

    /// Whether it serves the Guatiao kind `kind`.
    #[must_use]
    pub fn serves(&self, kind: &str) -> bool {
        self.kinds.iter().any(|served| served == kind)
    }
}

/// The prefix of a field's `format` that makes it a choice of the loaded providers of one
/// Guatiao kind: `provider:ai6_safehouse`.
pub const PROVIDER_FORMAT: &str = "provider:";

/// The prefix of the `format` of a field holding the settings of the provider a sibling field
/// chose: `settings-of:provider`. A `#section` after the name limits the settings to the fields of
/// the provider's schema marked with that `x-section`, as an Agent's handler settings are
/// (`settings-of:handler#agent`).
pub const SETTINGS_FORMAT: &str = "settings-of:";

/// The prefix of the `format` of a field standing for the settings written beside the provider's
/// name in the same object, as a secret provider's are: `flat-settings-of:provider`.
pub const FLAT_SETTINGS_FORMAT: &str = "flat-settings-of:";

/// The sibling field and the section a `settings-of:` format names.
#[must_use]
pub fn settings_of(format: &str) -> Option<(&str, Option<&str>)> {
    let rest = format.strip_prefix(SETTINGS_FORMAT)?;
    Some(match rest.split_once('#') {
        Some((field, section)) => (field, Some(section)),
        None => (rest, None),
    })
}

/// What the hosts report: each host with its providers.
pub type Reports = [(String, Vec<ProviderEntry>)];

/// Refuses a record whose provider settings are not the settings of the provider it chose,
/// against what the hosts report.
///
/// For every field of `schema` whose format is `settings-of:<sibling>`, the sibling's
/// `provider:<kind>` format names the kind and the sibling's value the provider. A provider that
/// no host reports is judged by nobody (the hosts may be down) unless some host reports other
/// providers of that kind, in which case it is refused as not installed. Settings are refused
/// when the chosen provider's schema does not declare a key (or, with a section, does not mark it
/// for that section), naming the provider and the key.
///
/// # Errors
///
/// A message naming the field, the provider and the key.
pub fn check_provider_settings(
    schema: &serde_json::Value,
    definition: &serde_json::Value,
    reports: &Reports,
) -> Result<(), String> {
    walk(schema, definition, reports, "")
}

fn object_arm(schema: &serde_json::Value) -> &serde_json::Value {
    schema
        .get("anyOf")
        .and_then(serde_json::Value::as_array)
        .and_then(|arms| arms.iter().find(|arm| arm.get("properties").is_some()))
        .unwrap_or(schema)
}

fn walk(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    reports: &Reports,
    path: &str,
) -> Result<(), String> {
    let schema = object_arm(schema);
    match value {
        serde_json::Value::Object(map) => {
            let properties = schema
                .get("properties")
                .and_then(serde_json::Value::as_object);
            // Settings written beside the provider's name are the keys the record does not
            // declare itself.
            for (field, member) in properties.into_iter().flatten() {
                let Some(sibling) = member
                    .get("format")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|format| format.strip_prefix(FLAT_SETTINGS_FORMAT))
                else {
                    continue;
                };
                let kind = properties
                    .and_then(|properties| properties.get(sibling))
                    .and_then(|sibling| sibling.get("format"))
                    .and_then(serde_json::Value::as_str)
                    .and_then(|format| format.strip_prefix(PROVIDER_FORMAT));
                let chosen = map.get(sibling).and_then(serde_json::Value::as_str);
                let beside: serde_json::Map<String, serde_json::Value> = map
                    .iter()
                    .filter(|(key, _)| {
                        properties.is_some_and(|properties| !properties.contains_key(*key))
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                if let (Some(kind), Some(chosen)) = (kind, chosen) {
                    let here = if path.is_empty() {
                        field.clone()
                    } else {
                        format!("{path}.{field}")
                    };
                    check_settings(&here, kind, chosen, None, &beside, reports)?;
                }
            }
            for (key, held) in map {
                let here = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let member = properties
                    .and_then(|properties| properties.get(key))
                    .or_else(|| schema.get("additionalProperties"));
                let Some(member) = member else { continue };
                let format = member
                    .get("format")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if let Some((sibling, section)) = settings_of(format) {
                    if let Some(settings) = held.as_object() {
                        let kind = properties
                            .and_then(|properties| properties.get(sibling))
                            .and_then(|sibling| sibling.get("format"))
                            .and_then(serde_json::Value::as_str)
                            .and_then(|format| format.strip_prefix(PROVIDER_FORMAT));
                        let chosen = map.get(sibling).and_then(serde_json::Value::as_str);
                        if let (Some(kind), Some(chosen)) = (kind, chosen) {
                            check_settings(&here, kind, chosen, section, settings, reports)?;
                        }
                    }
                    continue;
                }
                if let (Some(kind), Some(chosen)) = (
                    format.strip_prefix(PROVIDER_FORMAT),
                    held.as_str().filter(|chosen| !chosen.is_empty()),
                ) {
                    check_installed(&here, kind, chosen, reports)?;
                    continue;
                }
                walk(member, held, reports, &here)?;
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            let Some(inner) = schema.get("items") else {
                return Ok(());
            };
            for (index, item) in items.iter().enumerate() {
                walk(inner, item, reports, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn serving<'a>(kind: &'a str, reports: &'a Reports) -> impl Iterator<Item = &'a ProviderEntry> {
    reports
        .iter()
        .flat_map(|(_, entries)| entries)
        .filter(move |entry| entry.serves(kind))
}

fn check_installed(path: &str, kind: &str, chosen: &str, reports: &Reports) -> Result<(), String> {
    let any = serving(kind, reports).next().is_some();
    if any && !serving(kind, reports).any(|entry| entry.id == chosen) {
        let mut known: Vec<&str> = serving(kind, reports)
            .map(|entry| entry.id.as_str())
            .collect();
        known.sort_unstable();
        known.dedup();
        return Err(format!(
            "{path}: no host reports a provider {chosen:?} of kind {kind} (they report {})",
            known.join(", ")
        ));
    }
    Ok(())
}

fn check_settings(
    path: &str,
    kind: &str,
    chosen: &str,
    section: Option<&str>,
    settings: &serde_json::Map<String, serde_json::Value>,
    reports: &Reports,
) -> Result<(), String> {
    check_installed(path, kind, chosen, reports)?;
    if settings.is_empty() {
        return Ok(());
    }
    let Some(entry) = serving(kind, reports)
        .find(|entry| entry.id == chosen && !(entry.schema.is_none() && entry.schema_omitted))
    else {
        return Ok(());
    };
    let properties = entry
        .schema
        .as_ref()
        .and_then(|schema| schema.get("properties"))
        .and_then(serde_json::Value::as_object);
    let allowed: Vec<&str> = properties
        .into_iter()
        .flatten()
        .filter(|(_, property)| {
            section.is_none_or(|section| {
                property
                    .get("x-section")
                    .and_then(serde_json::Value::as_str)
                    == Some(section)
            })
        })
        .map(|(name, _)| name.as_str())
        .collect();
    for key in settings.keys() {
        if !allowed.contains(&key.as_str()) {
            let has = if allowed.is_empty() {
                "it takes none".to_owned()
            } else {
                format!("it takes {}", allowed.join(", "))
            };
            return Err(format!(
                "{path}: provider {chosen:?} has no setting {key:?} ({has})"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entry(id: &str, kind: &str, schema: Option<serde_json::Value>) -> ProviderEntry {
        ProviderEntry {
            id: id.to_owned(),
            name: id.to_owned(),
            version: "0".to_owned(),
            kinds: vec![kind.to_owned()],
            library: None,
            unavailable: None,
            schema,
            form: None,
            schema_omitted: false,
            digest: String::new(),
        }
    }

    fn record_schema() -> serde_json::Value {
        json!({"type": "object", "properties": {
            "workspace": {"type": "object", "properties": {
                "provider": {"type": ["string", "null"], "format": "provider:ai6_safehouse"},
                "settings": {"type": "object", "format": "settings-of:provider"}
            }},
            "handler": {"type": ["string", "null"], "format": "provider:ai6_agent_handler"},
            "handler_settings": {"type": "object", "format": "settings-of:handler#agent"},
            "loose": {"type": "object", "format": "flat-settings-of:handler"}
        }})
    }

    fn reports() -> Vec<(String, Vec<ProviderEntry>)> {
        vec![(
            "worker".to_owned(),
            vec![
                entry(
                    "git",
                    "ai6_safehouse",
                    Some(json!({"properties": {"base": {}, "branch": {}}})),
                ),
                entry("copy", "ai6_safehouse", None),
                entry(
                    "claude",
                    "ai6_agent_handler",
                    Some(json!({"properties": {
                        "allowed_tools": {"x-section": "agent"},
                        "program": {"x-section": "station"}
                    }})),
                ),
            ],
        )]
    }

    #[test]
    fn settings_are_the_chosen_providers_own() {
        let schema = record_schema();
        let reports = reports();
        let check =
            |definition: serde_json::Value| check_provider_settings(&schema, &definition, &reports);
        assert_eq!(
            check(json!({"workspace": {"provider": "git", "settings": {"base": "main"}}})),
            Ok(())
        );
        let refused =
            check(json!({"workspace": {"provider": "copy", "settings": {"base": "main"}}}))
                .unwrap_err();
        assert!(
            refused.contains("workspace.settings")
                && refused.contains("\"copy\"")
                && refused.contains("\"base\"")
                && refused.contains("takes none"),
            "{refused}"
        );
        let refused =
            check(json!({"workspace": {"provider": "git", "settings": {"nope": 1}}})).unwrap_err();
        assert!(
            refused.contains("\"git\"") && refused.contains("base, branch"),
            "{refused}"
        );
    }

    #[test]
    fn an_agents_settings_are_limited_to_its_section() {
        let schema = record_schema();
        let reports = reports();
        assert_eq!(
            check_provider_settings(
                &schema,
                &json!({"handler": "claude", "handler_settings": {"allowed_tools": []}}),
                &reports
            ),
            Ok(())
        );
        let refused = check_provider_settings(
            &schema,
            &json!({"handler": "claude", "handler_settings": {"program": "x"}}),
            &reports,
        )
        .unwrap_err();
        assert!(
            refused.contains("\"claude\"") && refused.contains("\"program\""),
            "{refused}"
        );
    }

    #[test]
    fn a_provider_no_host_reports_is_refused_only_when_hosts_report_its_kind() {
        let schema = record_schema();
        let refused = check_provider_settings(
            &schema,
            &json!({"workspace": {"provider": "worktree"}}),
            &reports(),
        )
        .unwrap_err();
        assert!(
            refused.contains("\"worktree\"") && refused.contains("copy, git"),
            "{refused}"
        );
        // Nothing reported: nobody can judge it.
        assert_eq!(
            check_provider_settings(
                &schema,
                &json!({"workspace": {"provider": "worktree"}}),
                &[]
            ),
            Ok(())
        );
    }

    #[test]
    fn settings_beside_the_providers_name_are_checked_too() {
        let schema = json!({"type": "object", "properties": {
            "provider": {"type": "string", "format": "provider:ai6_safehouse"},
            "settings": {"type": "object", "format": "flat-settings-of:provider"}
        }});
        let reports = reports();
        assert_eq!(
            check_provider_settings(
                &schema,
                &json!({"provider": "git", "base": "main"}),
                &reports
            ),
            Ok(())
        );
        let refused = check_provider_settings(
            &schema,
            &json!({"provider": "git", "colour": "red"}),
            &reports,
        )
        .unwrap_err();
        assert!(
            refused.contains("\"git\"") && refused.contains("\"colour\""),
            "{refused}"
        );
    }

    #[test]
    fn a_settings_format_names_its_sibling_and_section() {
        assert_eq!(
            settings_of("settings-of:handler#agent"),
            Some(("handler", Some("agent")))
        );
        assert_eq!(
            settings_of("settings-of:provider"),
            Some(("provider", None))
        );
        assert_eq!(settings_of("provider:ai6_safehouse"), None);
    }
}
