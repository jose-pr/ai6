//! What a host lets a client choose before a session starts, and how it shows as ACP's config options and modes.
//!
//! Every choice is from the host's own list: a value a client names that is not on it is refused, so nothing a
//! client writes reaches the provider through an option.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Map, Value, json};

/// ACP's category for a session mode selector, which the facade also shows as `modes`.
pub(super) const MODE_CATEGORY: &str = "mode";

/// Where a chosen value reaches the provider.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OptionTarget {
    /// [`StartRequest::model`](crate::StartRequest::model) and [`ResumeRequest::model`](crate::ResumeRequest::model).
    Model,
    /// [`StartRequest::effort`](crate::StartRequest::effort) and [`ResumeRequest::effort`](crate::ResumeRequest::effort).
    Effort,
    /// The environment value of this name, over [`Settings::environment`](super::Settings::environment) and
    /// whatever a client set through the launch.
    Environment(String),
}

/// One value a client may choose: ACP's `SessionConfigSelectOption`, or a `SessionMode`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OptionValue {
    /// What a client names it by, and what the provider receives.
    pub id: String,
    /// A name for people.
    pub name: String,
    /// More for people.
    pub description: Option<String>,
}

impl OptionValue {
    /// A value with no description.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
        }
    }

    /// Sets the description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// A choice a client may make before a session starts: ACP's select `SessionConfigOption`. One of category `mode`
/// is also shown as ACP's `modes`, and `session/set_mode` sets it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StartOption {
    /// The option's `id`, which `session/set_config_option` names.
    pub id: String,
    /// A name for people.
    pub name: String,
    /// More for people.
    pub description: Option<String>,
    /// ACP's category: `model`, `thought_level`, `mode`, `model_config`, or one starting with `_`.
    pub category: Option<String>,
    /// What a client may choose from, in the order shown.
    pub values: Vec<OptionValue>,
    /// The `id` of the value a session starts with.
    pub default: String,
    /// Where the chosen value goes.
    pub target: OptionTarget,
}

impl StartOption {
    /// An option with no category and no description.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        target: OptionTarget,
        values: Vec<OptionValue>,
        default: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            category: None,
            values,
            default: default.into(),
            target,
        }
    }

    /// The models the provider may use: id `model`, category `model`, into [`OptionTarget::Model`].
    #[must_use]
    pub fn model(values: Vec<OptionValue>, default: impl Into<String>) -> Self {
        Self::new("model", "Model", OptionTarget::Model, values, default).category("model")
    }

    /// The reasoning efforts the provider takes: id `effort`, category `thought_level`, into
    /// [`OptionTarget::Effort`].
    #[must_use]
    pub fn effort(values: Vec<OptionValue>, default: impl Into<String>) -> Self {
        Self::new("effort", "Effort", OptionTarget::Effort, values, default)
            .category("thought_level")
    }

    /// Sets the category.
    #[must_use]
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Sets the description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    fn offers(&self, value: &str) -> bool {
        self.values.iter().any(|offered| offered.id == value)
    }
}

/// Why a host's options cannot be served, or nothing.
pub(super) fn check(options: &[StartOption]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut modes = 0;
    for option in options {
        if option.id.is_empty() || !ids.insert(option.id.as_str()) {
            return Err(format!(
                "option ids must be unique and not empty: {:?}",
                option.id
            ));
        }
        let target = match &option.target {
            OptionTarget::Model => "model".to_string(),
            OptionTarget::Effort => "effort".to_string(),
            OptionTarget::Environment(name) => {
                if name.is_empty() || name.contains(['=', '\0']) {
                    return Err(format!(
                        "option {} names no usable environment variable",
                        option.id
                    ));
                }
                format!("environment {name}")
            }
        };
        if !targets.insert(target.clone()) {
            return Err(format!("two options set the {target}"));
        }
        if option.category.as_deref() == Some(MODE_CATEGORY) {
            modes += 1;
        }
        let mut values = BTreeSet::new();
        if option.values.is_empty()
            || !option
                .values
                .iter()
                .all(|value| !value.id.is_empty() && values.insert(value.id.as_str()))
        {
            return Err(format!(
                "option {} needs values with unique, non-empty ids",
                option.id
            ));
        }
        if !option.offers(&option.default) {
            return Err(format!(
                "option {}'s default is not one of its values",
                option.id
            ));
        }
    }
    if modes > 1 {
        return Err("at most one option may be of category mode".to_string());
    }
    Ok(())
}

/// What each option of a session is set to, by option id.
pub(super) type Choices = BTreeMap<String, String>;

/// Every option at its default.
pub(super) fn defaults(options: &[StartOption]) -> Choices {
    options
        .iter()
        .map(|option| (option.id.clone(), option.default.clone()))
        .collect()
}

/// The option of `target`, if the host declared one.
pub(super) fn for_target<'a>(
    options: &'a [StartOption],
    target: &OptionTarget,
) -> Option<&'a StartOption> {
    options.iter().find(|option| &option.target == target)
}

/// The option of category `mode`, if the host declared one.
pub(super) fn mode_option(options: &[StartOption]) -> Option<&StartOption> {
    options
        .iter()
        .find(|option| option.category.as_deref() == Some(MODE_CATEGORY))
}

/// Sets `option` to `value`, or says why it cannot be: the value must be one the host listed.
pub(super) fn choose(
    options: &[StartOption],
    choices: &mut Choices,
    option: &str,
    value: &str,
) -> Result<(), String> {
    let Some(declared) = options.iter().find(|declared| declared.id == option) else {
        return Err(format!("this agent has no option {option}"));
    };
    if !declared.offers(value) {
        return Err(format!("the option {option} has no value {value}"));
    }
    choices.insert(option.to_string(), value.to_string());
    Ok(())
}

fn described(object: &mut Map<String, Value>, description: Option<&String>) {
    if let Some(description) = description {
        object.insert("description".to_string(), json!(description));
    }
}

/// ACP version 1's `configOptions`, each at its current value.
pub(super) fn config_options(options: &[StartOption], choices: &Choices) -> Value {
    options
        .iter()
        .map(|option| {
            let values = option
                .values
                .iter()
                .map(|value| {
                    let mut entry = Map::new();
                    entry.insert("value".to_string(), json!(value.id));
                    entry.insert("name".to_string(), json!(value.name));
                    described(&mut entry, value.description.as_ref());
                    Value::Object(entry)
                })
                .collect::<Vec<_>>();
            let mut entry = Map::new();
            entry.insert("id".to_string(), json!(option.id));
            entry.insert("name".to_string(), json!(option.name));
            described(&mut entry, option.description.as_ref());
            if let Some(category) = &option.category {
                entry.insert("category".to_string(), json!(category));
            }
            entry.insert("type".to_string(), json!("select"));
            entry.insert(
                "currentValue".to_string(),
                json!(choices.get(&option.id).unwrap_or(&option.default)),
            );
            entry.insert("options".to_string(), Value::Array(values));
            Value::Object(entry)
        })
        .collect()
}

/// ACP version 1's `SessionModeState`, from the option of category `mode`.
pub(super) fn modes(options: &[StartOption], choices: &Choices) -> Option<Value> {
    let option = mode_option(options)?;
    let available = option
        .values
        .iter()
        .map(|value| {
            let mut entry = Map::new();
            entry.insert("id".to_string(), json!(value.id));
            entry.insert("name".to_string(), json!(value.name));
            described(&mut entry, value.description.as_ref());
            Value::Object(entry)
        })
        .collect::<Vec<_>>();
    Some(json!({
        "currentModeId": choices.get(&option.id).unwrap_or(&option.default),
        "availableModes": available,
    }))
}

/// What `session/new` and `session/resume` add to their answer: nothing when the host declared nothing.
pub(super) fn session_fields(
    options: &[StartOption],
    choices: &Choices,
    answer: &mut Map<String, Value>,
) {
    if options.is_empty() {
        return;
    }
    answer.insert(
        "configOptions".to_string(),
        config_options(options, choices),
    );
    if let Some(modes) = modes(options, choices) {
        answer.insert("modes".to_string(), modes);
    }
}

/// The answer to [`SESSION_OPTIONS_METHOD`](super::SESSION_OPTIONS_METHOD): every option at its default, and
/// what the host did not declare as unknown, since the kind cannot be asked.
pub(super) fn description(options: &[StartOption]) -> Value {
    let choices = defaults(options);
    let mut answer = Map::new();
    let mut unknown = Map::new();
    if options.is_empty() {
        unknown.insert(
            "configOptions".to_string(),
            json!("the host declared no options for this agent"),
        );
    } else {
        answer.insert(
            "configOptions".to_string(),
            config_options(options, &choices),
        );
    }
    match modes(options, &choices) {
        Some(modes) => {
            answer.insert("modes".to_string(), modes);
        }
        None => {
            unknown.insert(
                "modes".to_string(),
                json!("the host declared no modes for this agent"),
            );
        }
    }
    if !unknown.is_empty() {
        answer.insert("unknown".to_string(), Value::Object(unknown));
    }
    Value::Object(answer)
}
