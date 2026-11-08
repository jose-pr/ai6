//! The records a person writes, read and shown from one declaration: the type's derives.
//!
//! A document or a stored definition becomes a [`guatiao::Value`] at the format boundary, and
//! the type comes from that value, so every format, the forms and the wire read one shape. A
//! refusal names the key, never the value.

use guatiao::{FromValue, ToValue};
use guatiao_intake::{Screen, check};
use guatiao_serde::{Serializable, from_serde};
use serde::Serialize;

use crate::directorates::m::assistant::AssistantSettings;
use crate::directorates::m::compartments::CompartmentSettings;
use crate::directorates::m::config::ConfigRecord;
use crate::directorates::m::cover::CoverDefinition;
use crate::directorates::m::review::ReviewPolicy;
use crate::directorates::m::roster::{
    AgentDefinition, AgentRoleDefinition, OperationDefinition, RosterSettings,
};
use crate::directorates::m::watches::WatchDefinition;

/// `T` from a value.
pub(crate) fn from_value<T: FromValue>(value: &guatiao::Value) -> Result<T, String> {
    T::from_value(value).map_err(|error| error.to_string())
}

/// `T` from a definition as a record holds it.
///
/// # Errors
///
/// Names the first key that is not acceptable, never its value.
pub fn from_json<T: FromValue>(definition: &serde_json::Value) -> Result<T, String> {
    let value = from_serde(definition).map_err(|error| error.to_string())?;
    from_value(&value)
}

/// `T` from a TOML document.
pub(crate) fn from_toml<T: FromValue>(text: &str) -> Result<T, String> {
    crate::bootstrap::format::read(std::path::Path::new("document.toml"), text)
        .map_err(|error| error.to_string())
}

/// `value` as JSON, written as a record is.
pub(crate) fn to_json<T: ToValue>(value: &T) -> Option<serde_json::Value> {
    let value = value.to_value().ok()?;
    serde_json::to_value(Serializable::from(&value)).ok()
}

/// How the key of a record is asked for.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordKey {
    /// What the key is called.
    pub title: String,
    /// What it is for.
    pub description: String,
    /// The one key a family with one record has; a person does not choose it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<String>,
}

/// What a form needs to show one kind of record: its schema, how it is shown, and its key.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordForm {
    /// The record's JSON Schema, as its type declares it.
    pub schema: serde_json::Value,
    /// Guatiao's form over that schema: section names and each field's control.
    pub form: serde_json::Value,
    /// How its key is asked for.
    pub key: RecordKey,
}

/// A form could not be built from its declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFormError(String);

impl std::fmt::Display for RecordFormError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RecordFormError {}

fn built<T: guatiao::Schema + Screen>(key: RecordKey) -> Result<RecordForm, RecordFormError> {
    let failed = |error: &dyn std::fmt::Display| RecordFormError(error.to_string());
    let schema = T::schema().map_err(|error| failed(&error))?;
    let form = T::form(guatiao::Alloc::rust()).map_err(|error| failed(&error))?;
    let (Some(schema_ref), Some(form_ref)) = (
        guatiao::schema::read::SchemaRef::new(&schema),
        guatiao_intake::FormRef::new(&form),
    ) else {
        return Err(RecordFormError(
            "a declaration did not build a schema and a form".to_owned(),
        ));
    };
    check(schema_ref, form_ref).map_err(|error| failed(&error))?;
    let json = |value: &guatiao::Value| {
        serde_json::to_value(Serializable::from(value)).map_err(|error| failed(&error))
    };
    Ok(RecordForm {
        schema: json(&schema)?,
        form: json(&form)?,
        key,
    })
}

fn named(title: &str, description: &str) -> RecordKey {
    RecordKey {
        title: title.to_owned(),
        description: description.to_owned(),
        fixed: None,
    }
}

fn the_one(title: &str) -> RecordKey {
    RecordKey {
        title: title.to_owned(),
        description: String::new(),
        fixed: Some(ConfigRecord::SINGLETON_KEY.to_owned()),
    }
}

/// `[oo]`'s schema and form, for Outer Office's settings page.
///
/// # Errors
///
/// When its declaration does not make a schema and a form that fit; a test holds it to that.
pub fn oo_settings_form() -> Result<RecordForm, RecordFormError> {
    built::<crate::bootstrap::oo::OoSettings>(the_one("Outer Office settings"))
}

impl ConfigRecord {
    /// The form of this kind of record, for a family a person edits with one.
    ///
    /// `None` for a family that has none yet.
    ///
    /// # Errors
    ///
    /// When its declaration does not make a schema and a form that fit; a test holds every
    /// family to that.
    pub fn form(self) -> Result<Option<RecordForm>, RecordFormError> {
        match self {
            Self::Operation => built::<OperationDefinition>(named(
                "Codename",
                "The name Missions and Stations know this Operation by. It cannot be changed later.",
            )),
            Self::Agent => built::<AgentDefinition>(named(
                "Codename",
                "The name Missions ask for this Agent by. It cannot be changed later.",
            )),
            Self::Role => built::<AgentRoleDefinition>(named(
                "Role name",
                "Short and without spaces, such as case-officer. It cannot be changed later.",
            )),
            Self::Cover => built::<CoverDefinition>(named(
                "Cover name",
                "What Operations choose this cover by. It cannot be changed later.",
            )),
            Self::Assistant => built::<AssistantSettings>(the_one("Moneypenny's settings")),
            Self::Roster => built::<RosterSettings>(the_one("Roster settings")),
            Self::Compartment => built::<CompartmentSettings>(named(
                "Codeword",
                "What this compartment is known by, such as KESTREL. It cannot be changed later.",
            )),
            Self::Review => built::<ReviewPolicy>(named(
                "Policy name",
                "What Operations, roles and briefs choose this policy by. It cannot be changed later.",
            )),
            Self::Watch => built::<WatchDefinition>(named(
                "Scope",
                "What the watch keeps: m, operation:<codename> or station:<id>. It cannot be changed later.",
            )),
            _ => return Ok(None),
        }
        .map(Some)
    }
}

#[cfg(test)]
mod tests;
