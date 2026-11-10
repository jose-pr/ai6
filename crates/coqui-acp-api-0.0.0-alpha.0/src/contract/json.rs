//! Guatiao values and JSON, for the parts of the suite that read a schema or build settings.

use alloc::string::{String, ToString};

use guatiao::Value;

/// `value` as JSON; `null` when it holds something JSON cannot.
pub(super) fn to_json(value: &Value) -> serde_json::Value {
    serde_json::to_value(guatiao_serde::Serializable::from(value))
        .unwrap_or(serde_json::Value::Null)
}

/// `json` as a Guatiao value.
pub(super) fn from_json(json: &serde_json::Value) -> Result<Value, String> {
    guatiao_serde::from_serde(json.clone()).map_err(|e| e.to_string())
}
