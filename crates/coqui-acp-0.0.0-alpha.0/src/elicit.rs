//! An agent's questions to the person: ACP's `elicitation/create`, in form mode. What the agent sends is read into fields a client
//! can render, and an answer is sent only when it fits them, as `vectors/elicitation.json` says for this client and the Dart one.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde_json::{Map, Value};

/// One choice a field offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceOption {
    /// What the answer holds when this is chosen.
    pub value: String,
    /// The agent's words for it; the value when it gave none.
    pub title: String,
    /// More of its words, when it gave them.
    pub description: Option<String>,
}

/// What a field takes, with the bounds the agent set that this client keeps. A bound or default of the wrong type is left out.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum FieldKind {
    /// Text, its length counted in characters.
    Text {
        /// The fewest characters.
        min_length: Option<u64>,
        /// The most characters.
        max_length: Option<u64>,
        /// `email`, `uri`, `date`, `date-time` or another the agent named, as an annotation.
        format: Option<String>,
        /// The text a form starts with.
        default: Option<String>,
    },
    /// One of the options, as text.
    Choice {
        /// What may be chosen, in the agent's order.
        options: Vec<ChoiceOption>,
        /// The option a form starts with.
        default: Option<String>,
    },
    /// Some of the options, as a list of text with none twice.
    Choices {
        /// What may be chosen, in the agent's order.
        options: Vec<ChoiceOption>,
        /// The fewest chosen.
        min_items: Option<u64>,
        /// The most chosen.
        max_items: Option<u64>,
        /// What a form starts with chosen.
        default: Option<Vec<String>>,
    },
    /// A number.
    Number {
        /// The least, inclusive.
        minimum: Option<f64>,
        /// The most, inclusive.
        maximum: Option<f64>,
        /// The number a form starts with.
        default: Option<f64>,
    },
    /// A whole number.
    Integer {
        /// The least, inclusive.
        minimum: Option<i64>,
        /// The most, inclusive.
        maximum: Option<i64>,
        /// The number a form starts with.
        default: Option<i64>,
    },
    /// Yes or no.
    Boolean {
        /// The answer a form starts with.
        default: Option<bool>,
    },
    /// A type this client does not know, which it never renders as one it does: no answer can hold it.
    Unsupported {
        /// The type the agent named, or empty when it named none.
        kind: String,
    },
}

/// One question of a form.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Its key in the answer.
    pub name: String,
    /// The agent's title for it.
    pub title: Option<String>,
    /// More of the agent's words about it.
    pub description: Option<String>,
    /// Whether an accepted answer has to hold it.
    pub required: bool,
    /// What it takes.
    pub kind: FieldKind,
}

/// An agent's question to the person, as a form. All of its text is the agent's, untrusted, to be shown as data.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Elicitation {
    /// The session it belongs to.
    pub session_id: String,
    /// The tool call it is part of, when the agent said.
    pub tool_call_id: Option<String>,
    /// What the agent asks.
    pub message: String,
    /// The form's title.
    pub title: Option<String>,
    /// The form's description.
    pub description: Option<String>,
    /// Its questions, in the agent's order.
    pub fields: Vec<Field>,
}

/// The person's answer to an [`Elicitation`].
#[derive(Clone, Debug, PartialEq)]
pub enum ElicitationAnswer {
    /// They answered: a value for each field they filled in, by its name. It is sent only if it fits the form; otherwise the
    /// agent is told the question was cancelled, and an [`Event::Blocked`](crate::Event::Blocked) says why.
    Accept(Map<String, Value>),
    /// They refused to answer.
    Decline,
    /// They dismissed it without choosing.
    Cancel,
}

/// Puts an agent's questions to the person. It may block, for instance to wait for them: it runs on a thread of its own.
pub trait Elicitor: Send + Sync {
    /// Answers one question.
    fn elicit(&self, request: &Elicitation) -> ElicitationAnswer;
}

/// How much of a form this client takes. A form past either is answered cancelled without asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) fields: usize,
    pub(crate) options: usize,
}

fn text(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(ToOwned::to_owned)
}

fn count(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(Value::as_u64)
}

/// The options of a `oneOf` or `anyOf` list of `{const, title}`, or of an `enum` list of text; what is not text is left out.
fn options(map: &Map<String, Value>, titled: &str) -> Option<Vec<ChoiceOption>> {
    if let Some(Value::Array(listed)) = map.get(titled) {
        return Some(
            listed
                .iter()
                .filter_map(|item| {
                    let item = item.as_object()?;
                    let value = text(item, "const")?;
                    Some(ChoiceOption {
                        title: text(item, "title").unwrap_or_else(|| value.clone()),
                        description: text(item, "description"),
                        value,
                    })
                })
                .collect(),
        );
    }
    if let Some(Value::Array(listed)) = map.get("enum") {
        return Some(
            listed
                .iter()
                .filter_map(Value::as_str)
                .map(|value| ChoiceOption {
                    value: value.to_owned(),
                    title: value.to_owned(),
                    description: None,
                })
                .collect(),
        );
    }
    None
}

fn kind_of(schema: &Map<String, Value>) -> FieldKind {
    let named = schema.get("type").and_then(Value::as_str).unwrap_or("");
    match named {
        "string" => match options(schema, "oneOf") {
            Some(options) => {
                let default = text(schema, "default")
                    .filter(|value| options.iter().any(|option| &option.value == value));
                FieldKind::Choice { options, default }
            }
            None => FieldKind::Text {
                min_length: count(schema, "minLength"),
                max_length: count(schema, "maxLength"),
                format: text(schema, "format"),
                default: text(schema, "default"),
            },
        },
        "array" => {
            let items = schema.get("items").and_then(Value::as_object);
            let listed = items.and_then(|items| match items.get("type") {
                None | Some(Value::String(_)) if items.contains_key("anyOf") => {
                    options(items, "anyOf")
                }
                Some(Value::String(kind)) if kind == "string" => options(items, "anyOf"),
                _ => None,
            });
            match listed {
                Some(options) => {
                    let default = match schema.get("default") {
                        Some(Value::Array(values)) => values
                            .iter()
                            .map(|value| {
                                value
                                    .as_str()
                                    .filter(|value| {
                                        options.iter().any(|option| option.value == *value)
                                    })
                                    .map(ToOwned::to_owned)
                            })
                            .collect::<Option<Vec<_>>>(),
                        _ => None,
                    };
                    FieldKind::Choices {
                        options,
                        min_items: count(schema, "minItems"),
                        max_items: count(schema, "maxItems"),
                        default,
                    }
                }
                None => FieldKind::Unsupported {
                    kind: named.to_owned(),
                },
            }
        }
        "number" => FieldKind::Number {
            minimum: schema.get("minimum").and_then(Value::as_f64),
            maximum: schema.get("maximum").and_then(Value::as_f64),
            default: schema.get("default").and_then(Value::as_f64),
        },
        "integer" => FieldKind::Integer {
            minimum: schema.get("minimum").and_then(Value::as_i64),
            maximum: schema.get("maximum").and_then(Value::as_i64),
            default: schema.get("default").and_then(Value::as_i64),
        },
        "boolean" => FieldKind::Boolean {
            default: schema.get("default").and_then(Value::as_bool),
        },
        other => FieldKind::Unsupported {
            kind: other.to_owned(),
        },
    }
}

/// Reads `elicitation/create`'s parameters, or says why they cannot be put to a person.
pub(crate) fn read(params: &Value, limits: Limits) -> Result<Elicitation, String> {
    let Some(params) = params.as_object() else {
        return Err("the agent asked a question in a form this client cannot read".to_owned());
    };
    match params.get("mode").and_then(Value::as_str) {
        Some("form") => {}
        Some(mode) => {
            return Err(format!(
                "the agent asked a question by {mode}, which this client does not offer"
            ));
        }
        None => {
            return Err("the agent asked a question in a form this client cannot read".to_owned());
        }
    }
    let Some(session_id) = text(params, "sessionId") else {
        return Err("the agent asked a question tied to no session".to_owned());
    };
    let (Some(message), Some(schema)) = (
        text(params, "message"),
        params.get("requestedSchema").and_then(Value::as_object),
    ) else {
        return Err("the agent asked a question in a form this client cannot read".to_owned());
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Err("the agent asked a question in a form this client cannot read".to_owned());
    };
    if properties.len() > limits.fields {
        return Err(format!(
            "the agent asked {} questions at once, more than the {} this client takes",
            properties.len(),
            limits.fields
        ));
    }
    let required: Vec<&str> = match schema.get("required") {
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let mut fields = Vec::with_capacity(properties.len());
    for (name, field) in properties {
        let field = field.as_object().cloned().unwrap_or_default();
        let kind = kind_of(&field);
        if let FieldKind::Choice { options, .. } | FieldKind::Choices { options, .. } = &kind
            && options.len() > limits.options
        {
            return Err(format!(
                "the agent offered {} choices for one question, more than the {} this client takes",
                options.len(),
                limits.options
            ));
        }
        fields.push(Field {
            name: name.clone(),
            title: text(&field, "title"),
            description: text(&field, "description"),
            required: required.contains(&name.as_str()),
            kind,
        });
    }
    Ok(Elicitation {
        session_id,
        tool_call_id: text(params, "toolCallId"),
        message,
        title: text(schema, "title"),
        description: text(schema, "description"),
        fields,
    })
}

/// Whether `content` is an answer `request`'s form allows, or why not.
pub(crate) fn fits(request: &Elicitation, content: &Map<String, Value>) -> Result<(), String> {
    for name in content.keys() {
        if !request.fields.iter().any(|field| &field.name == name) {
            return Err(format!(
                "the answer has {name}, which the agent did not ask"
            ));
        }
    }
    for field in &request.fields {
        let name = &field.name;
        let Some(value) = content.get(name) else {
            if field.required {
                return Err(format!(
                    "the answer leaves out {name}, which the agent requires"
                ));
            }
            continue;
        };
        let wrong = || {
            Err(format!(
                "the answer's {name} does not fit what the agent asked"
            ))
        };
        let within = |at: f64, least: Option<f64>, most: Option<f64>| {
            least.is_none_or(|least| at >= least) && most.is_none_or(|most| at <= most)
        };
        let fit = match &field.kind {
            FieldKind::Text {
                min_length,
                max_length,
                ..
            } => value.as_str().is_some_and(|text| {
                let length = text.chars().count() as u64;
                min_length.is_none_or(|least| length >= least)
                    && max_length.is_none_or(|most| length <= most)
            }),
            FieldKind::Choice { options, .. } => value
                .as_str()
                .is_some_and(|chosen| options.iter().any(|option| option.value == chosen)),
            FieldKind::Choices {
                options,
                min_items,
                max_items,
                ..
            } => value.as_array().is_some_and(|chosen| {
                let mut seen: Vec<&str> = Vec::with_capacity(chosen.len());
                let each = chosen.iter().all(|item| {
                    item.as_str().is_some_and(|item| {
                        let fresh = !seen.contains(&item);
                        seen.push(item);
                        fresh && options.iter().any(|option| option.value == item)
                    })
                });
                let many = chosen.len() as u64;
                each && min_items.is_none_or(|least| many >= least)
                    && max_items.is_none_or(|most| many <= most)
            }),
            FieldKind::Number {
                minimum, maximum, ..
            } => value
                .as_f64()
                .is_some_and(|at| within(at, *minimum, *maximum)),
            FieldKind::Integer {
                minimum, maximum, ..
            } => value.as_i64().is_some_and(|at| {
                minimum.is_none_or(|least| at >= least) && maximum.is_none_or(|most| at <= most)
            }),
            FieldKind::Boolean { .. } => value.is_boolean(),
            FieldKind::Unsupported { .. } => false,
        };
        if !fit {
            return wrong();
        }
    }
    Ok(())
}
