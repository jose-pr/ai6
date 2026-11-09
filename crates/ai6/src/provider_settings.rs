//! What crosses to a provider as its configuration: the record's settings as a Guatiao value,
//! and the check of those settings against the provider's own schema.

use guatiao::schema::read::SchemaRef;
use guatiao::schema::validate::validate_value;
use guatiao::{FromValue, Map, TryAsRef, Value};

/// A JSON object (an Agent's `handler_settings`, a Theater's `access` settings) as a value.
///
/// # Errors
///
/// A message when the object holds a number a value cannot.
pub fn settings_value(
    settings: &serde_json::Map<String, serde_json::Value>,
) -> Result<Value, String> {
    guatiao_serde::from_serde(serde_json::Value::Object(settings.clone()))
        .map_err(|error: serde_json::Error| error.to_string())
}

/// A TOML table (an Operation's source settings) as a value.
///
/// # Errors
///
/// A message when the table holds what a value cannot.
pub fn table_value(table: &toml::Table) -> Result<Value, String> {
    guatiao_serde::from_serde(toml::Value::Table(table.clone()))
        .map_err(|error: toml::de::Error| error.to_string())
}

/// The first key of `settings` that `schema` (a provider's configuration schema) does not
/// declare, or whose value is not what the schema says, as a message naming `provider`. A key the
/// settings leave out is not required here: the Station adds its own defaults before building.
///
/// # Errors
///
/// The message above.
pub fn check_settings(provider: &str, schema: &Value, settings: &Value) -> Result<(), String> {
    let Some(schema) = SchemaRef::new(schema) else {
        return Ok(());
    };
    let Some(map) = TryAsRef::<Map>::try_as_ref(settings) else {
        return Err(format!("provider {provider:?}: settings must be a table"));
    };
    for entry in map {
        let key = entry.key();
        let Some(field) = schema.find(key) else {
            let known: Vec<String> = schema.fields().map(|f| f.key().to_string()).collect();
            return Err(format!(
                "provider {provider:?}: unknown setting {key:?} (it has {})",
                known.join(", ")
            ));
        };
        validate_value(field, entry.value())
            .map_err(|error| format!("provider {provider:?}: setting {key:?}: {error}"))?;
    }
    Ok(())
}

/// A provider's settings type read from a JSON object, as its schema declares it.
///
/// # Errors
///
/// A message naming the first key that is unknown or not acceptable.
pub fn settings_of<T: FromValue>(
    settings: &serde_json::Map<String, serde_json::Value>,
) -> Result<T, String> {
    T::from_value(&settings_value(settings)?).map_err(|error| error.to_string())
}

/// A provider's settings type read from TOML text, for a provider's own tests.
///
/// # Errors
///
/// A message naming the first key that is unknown or not acceptable.
pub fn settings_from_text<T: FromValue>(text: &str) -> Result<T, String> {
    let table: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
    T::from_value(&table_value(&table)?).map_err(|error| error.to_string())
}
