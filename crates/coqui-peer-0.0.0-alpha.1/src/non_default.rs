//! A setting that differs from its default, reported as data.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use guatiao::schema::{Difference, SchemaRef};
use guatiao::{FromValue, Schema, ToValue, Value};

/// A setting whose value differs from its default, as plain data for the caller to show or log.
///
/// Every crate of the family reports its non-default settings this way: a settings type's
/// [`non_defaults`](NonDefault::of_settings), a `coqui-peer-tls` `Policy` there. A non-default
/// setting is never refused for it; it is listed, and the application that knows the use case
/// decides what it means.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NonDefault {
    /// The setting's key path in its settings type (`pin_attempts`, `waiting.per_source`).
    pub setting: String,
    /// Its value, as text.
    pub value: String,
    /// The default value, as text.
    pub default: String,
}

/// What stands for a secret value in a report.
const PLACEHOLDER: &str = "***";

impl NonDefault {
    /// A non-default `setting`, now `value`, against `default`.
    pub fn new(
        setting: impl Into<String>,
        value: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        Self {
            setting: setting.into(),
            value: value.into(),
            default: default.into(),
        }
    }

    /// The non-default setting a guatiao [`Difference`] reports: `setting` is its
    /// [`key`](Difference::key) (`waiting.per_source`), and `value` and `default` are rendered by
    /// one rule.
    ///
    /// A text value is its text, any other value is its compact JSON (`true`, `16`, `[60,0]`,
    /// `["1.2","1.3"]`), and a side the difference has no value for is `unset`. A secret arrives as
    /// the placeholder the difference was made with, and is shown as that.
    pub fn from_difference(difference: &Difference) -> Self {
        Self::new(
            difference.key(),
            render(difference.value.as_ref()),
            render(difference.baseline.as_ref()),
        )
    }

    /// Every key of `settings` whose value differs from the default its schema declares, named by
    /// key path and in key order: empty for [`Default`] settings.
    pub fn of_settings<T: ToValue + Schema>(settings: &T) -> Vec<Self> {
        Self::walk(settings, None)
    }

    /// As [`of_settings`](Self::of_settings), against `baseline` instead of the schema's defaults,
    /// for a consumer whose own baseline differs from them.
    pub fn of_settings_against<T: ToValue + Schema>(settings: &T, baseline: &T) -> Vec<Self> {
        Self::walk(settings, Some(baseline))
    }

    /// The schema, the value and the baseline's are all derived from one type, so the walk cannot
    /// fail unless the allocator does.
    fn walk<T: ToValue + Schema>(settings: &T, baseline: Option<&T>) -> Vec<Self> {
        let derived = "a settings type is a value that builds";
        let schema = T::schema().expect(derived);
        let schema = SchemaRef::new(&schema).expect(derived);
        let value = settings.to_value().expect(derived);
        let baseline = baseline.map(|baseline| baseline.to_value().expect(derived));
        guatiao::schema::differences(schema, &value, baseline.as_ref(), PLACEHOLDER)
            .expect(derived)
            .iter()
            .map(Self::from_difference)
            .collect()
    }
}

fn render(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "unset".to_owned();
    };
    String::from_value(value).unwrap_or_else(|_| {
        guatiao_serde::text::json::to_string(value, guatiao_serde::Presentation::new())
            .unwrap_or_else(|_| "unrepresentable".to_string())
    })
}
