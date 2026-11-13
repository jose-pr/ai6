//! A secret form: a JSON Schema object whose properties are the questions, with the secret ones marked `writeOnly` or `x-sensitive`.

use super::error::Error;
use super::json::strict;
use crate::elicit::{self, Elicitation, Field, FieldKind};
use alloc::string::String;
use alloc::vec::Vec;
use base64::Engine as _;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig, general_purpose};
use serde_json::{Map, Value};

/// How much of a form is taken. A form past any limit is refused. Every value from 0 up is taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Questions in one form. Default 64.
    pub fields: usize,
    /// Choices of one question. Default 256.
    pub options: usize,
    /// The schema, written as JSON, in bytes. Default 64 KiB.
    pub schema_bytes: usize,
    /// A field's name, in bytes. Default 128.
    pub name_bytes: usize,
    /// One secret value, in bytes. Default 64 KiB.
    pub value_bytes: usize,
}

impl Limits {
    /// The defaults.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fields: 64,
            options: 256,
            schema_bytes: 64 * 1024,
            name_bytes: 128,
            value_bytes: 64 * 1024,
        }
    }

    /// Sets the most questions in one form.
    #[must_use]
    pub const fn fields(mut self, fields: usize) -> Self {
        self.fields = fields;
        self
    }

    /// Sets the most choices of one question.
    #[must_use]
    pub const fn options(mut self, options: usize) -> Self {
        self.options = options;
        self
    }

    /// Sets the most bytes of a schema written as JSON.
    #[must_use]
    pub const fn schema_bytes(mut self, bytes: usize) -> Self {
        self.schema_bytes = bytes;
        self
    }

    /// Sets the most bytes of a field's name.
    #[must_use]
    pub const fn name_bytes(mut self, bytes: usize) -> Self {
        self.name_bytes = bytes;
        self
    }

    /// Sets the most bytes of one secret value.
    #[must_use]
    pub const fn value_bytes(mut self, bytes: usize) -> Self {
        self.value_bytes = bytes;
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

/// One question of a secret form.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct FormField {
    /// The question as a plain form has it: name, title, kind, bounds, whether an answer requires it.
    pub field: Field,
    /// Whether the value is a secret: it travels sealed, and no one in between reads it.
    pub secret: bool,
    /// The schema's `contentEncoding`, such as `base64` for bytes; a frontend picks how to prompt for or show the value from it.
    pub encoding: Option<String>,
    /// The schema's `contentMediaType`.
    pub media_type: Option<String>,
}

/// A form read from a schema, the questions in the schema's order.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Form {
    /// The form's title.
    pub title: Option<String>,
    /// The form's description.
    pub description: Option<String>,
    /// Its questions.
    pub fields: Vec<FormField>,
    limits: Limits,
}

/// Standard base64, canonical: padded, with zero trailing bits.
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
);

/// The largest whole number every language the extension is written in holds exactly: 2^53 - 1.
const MAX_SAFE: i64 = 9_007_199_254_740_991;

/// A whole number as text: an optional minus, then `0` or a digit 1 to 9 and more digits, within `MAX_SAFE`.
pub(crate) fn json_integer(text: &str) -> Option<i64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let plain = !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0'));
    if !plain || digits.len() > 16 {
        return None;
    }
    text.parse::<i64>().ok().filter(|at| at.abs() <= MAX_SAFE)
}

/// A number as text in JSON's grammar (an integer part as above, an optional fraction of digits, an optional exponent), finite.
pub(crate) fn json_number(text: &str) -> Option<f64> {
    let rest = text.strip_prefix('-').unwrap_or(text);
    let int_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    let int = &rest[..int_len];
    if int.is_empty() || (int.len() > 1 && int.starts_with('0')) {
        return None;
    }
    let mut rest = &rest[int_len..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let len = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if len == 0 {
            return None;
        }
        rest = &fraction[len..];
    }
    if let Some(exponent) = rest.strip_prefix(['e', 'E']) {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if exponent.is_empty() || !exponent.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        rest = "";
    }
    if !rest.is_empty() {
        return None;
    }
    text.parse::<f64>().ok().filter(|at| at.is_finite())
}

/// Canonical unpadded base64url: no padding, an alphabet of its own, zero trailing bits.
pub(crate) fn base64url_canonical(text: &str) -> bool {
    general_purpose::URL_SAFE_NO_PAD.decode(text).is_ok()
}

/// Canonical standard base64.
pub(crate) fn base64_canonical(text: &str) -> bool {
    BASE64.decode(text).is_ok()
}

/// The only keywords the schema's root may have: a flat object of questions.
const ROOT_ALLOWED: [&str; 5] = ["type", "properties", "required", "title", "description"];

/// The only keywords a secret field may have. Anything else could carry a value, a condition or a hint that makes a frontend draw
/// or treat the field as something other than a secret.
const SECRET_ALLOWED: [&str; 12] = [
    "type",
    "title",
    "description",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "writeOnly",
    "x-sensitive",
    "contentEncoding",
    "contentMediaType",
    "format",
];

/// The widgets of a layout for a question that is not secret (guatiao-intake's suggestions, minus the masking one).
const PLAIN_WIDGETS: [&str; 9] = [
    "text", "textarea", "number", "slider", "checkbox", "toggle", "select", "radio", "dialog",
];

impl Form {
    /// Reads a schema from the JSON text that was sent, or says which rule it breaks. The schema is the agent's, untrusted. Only a
    /// field's own `secret` mark says it is secret: a frontend masks or hides a value from [`FormField::secret`] alone, never from
    /// `format`, a title or a name.
    ///
    /// # Errors
    /// [`Error::Schema`] for a schema that is not an object with properties, one over a limit, a name that is empty, long or holds a
    /// control character, a property that is no object or whose type no control can show, a `required` that is no list of the
    /// properties' names, and a secret field that is a choice, a flag or a list, or carries a `default`, `const`, `enum`,
    /// `oneOf` or `examples` (the form would put a value in the agent's text).
    pub fn parse_text(schema: &str, limits: &Limits) -> Result<Self, Error> {
        if schema.len() > limits.schema_bytes {
            return Err(Error::Schema("the schema is larger than the limit"));
        }
        let value = strict(
            schema,
            Error::Schema,
            "the schema is not JSON",
            "the schema repeats a key",
        )?;
        Self::parse(&value, limits)
    }

    /// Reads a schema already parsed; see [`Form::parse_text`], which is what a message's text is read with.
    ///
    /// # Errors
    /// The same as [`Form::parse_text`].
    pub fn parse(schema: &Value, limits: &Limits) -> Result<Self, Error> {
        let schema = schema
            .as_object()
            .ok_or(Error::Schema("the schema is not an object"))?;
        if serde_json::to_vec(schema).map_or(usize::MAX, |bytes| bytes.len()) > limits.schema_bytes
        {
            return Err(Error::Schema("the schema is larger than the limit"));
        }
        if schema
            .keys()
            .any(|key| !ROOT_ALLOWED.contains(&key.as_str()))
        {
            return Err(Error::Schema(
                "the schema has a keyword a flat form does not",
            ));
        }
        if schema
            .get("type")
            .is_some_and(|kind| kind.as_str() != Some("object"))
        {
            return Err(Error::Schema("the schema's type is not object"));
        }
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or(Error::Schema("the schema has no properties"))?;
        if properties.is_empty() {
            return Err(Error::Schema("the schema asks nothing"));
        }
        if properties.len() > limits.fields {
            return Err(Error::Schema(
                "the schema has more questions than the limit",
            ));
        }
        let required: Vec<&str> = match schema.get("required") {
            None => Vec::new(),
            Some(Value::Array(names)) => names
                .iter()
                .map(|name| {
                    name.as_str()
                        .ok_or(Error::Schema("required holds a name that is not text"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(Error::Schema("required is not a list")),
        };
        if required.iter().any(|name| !properties.contains_key(*name)) {
            return Err(Error::Schema("required names a property the schema lacks"));
        }
        let mut fields = Vec::with_capacity(properties.len());
        for (name, property) in properties {
            if name.is_empty()
                || name.len() > limits.name_bytes
                || name.chars().any(char::is_control)
            {
                return Err(Error::Schema(
                    "a property's name is empty, too long or holds a control character",
                ));
            }
            let property = property
                .as_object()
                .ok_or(Error::Schema("a property is not an object"))?;
            let kind = elicit::kind_of(property);
            if let FieldKind::Choice { options, .. } | FieldKind::Choices { options, .. } = &kind
                && options.len() > limits.options
            {
                return Err(Error::Schema("a question has more choices than the limit"));
            }
            if matches!(kind, FieldKind::Unsupported { .. }) {
                return Err(Error::Schema(
                    "a property's type is one no control can show",
                ));
            }
            let secret = flag(property, "writeOnly")? | flag(property, "x-sensitive")?;
            if !secret && property.get("format").and_then(Value::as_str) == Some("password") {
                return Err(Error::Schema(
                    "a password format without writeOnly or x-sensitive",
                ));
            }
            if let FieldKind::Integer {
                minimum, maximum, ..
            } = &kind
                && minimum
                    .iter()
                    .chain(maximum.iter())
                    .any(|bound| bound.abs() > MAX_SAFE)
            {
                return Err(Error::Schema("a whole-number bound beyond 2^53 - 1"));
            }
            if secret {
                if !matches!(
                    kind,
                    FieldKind::Text { .. } | FieldKind::Integer { .. } | FieldKind::Number { .. }
                ) {
                    return Err(Error::Schema("a secret field is not text or a number"));
                }
                if property
                    .keys()
                    .any(|key| !SECRET_ALLOWED.contains(&key.as_str()))
                {
                    return Err(Error::Schema(
                        "a secret field has a keyword a secret field may not",
                    ));
                }
                if property
                    .get("format")
                    .is_some_and(|format| format != "password")
                {
                    return Err(Error::Schema("a secret field's format is not password"));
                }
            }
            fields.push(FormField {
                field: Field {
                    name: name.clone(),
                    title: elicit::text(property, "title"),
                    description: elicit::text(property, "description"),
                    required: required.contains(&name.as_str()),
                    kind,
                },
                secret,
                encoding: elicit::text(property, "contentEncoding"),
                media_type: elicit::text(property, "contentMediaType"),
            });
        }
        Ok(Self {
            title: elicit::text(schema, "title"),
            description: elicit::text(schema, "description"),
            fields,
            limits: *limits,
        })
    }

    /// Checks the layout that came with the form, as the JSON text that was sent: guatiao-intake's form document, `{"sections":
    /// [{"id", "title"?, "description"?}], "fields": {<question>: {"widget"?, "placeholder"?, "unit"?, "visibleWhen"?}}}`, over a flat
    /// form. A layout may place and label the questions; it may not change what a secret question is: a secret question has the
    /// `password` widget or none, no other question has it, and a condition may not read a secret question's value.
    ///
    /// # Errors
    /// [`Error::Schema`] for a layout that is not that, repeats a key, or names a question the form lacks; [`Error::TooLarge`].
    pub fn check_layout(&self, text: &str) -> Result<(), Error> {
        if text.len() > self.limits.schema_bytes {
            return Err(Error::TooLarge("the layout"));
        }
        let value = strict(
            text,
            Error::Schema,
            "the layout is not JSON",
            "the layout repeats a key",
        )?;
        let layout = value
            .as_object()
            .ok_or(Error::Schema("the layout is not an object"))?;
        let only = |map: &Map<String, Value>, keys: &[&str]| {
            map.keys().all(|key| keys.contains(&key.as_str()))
        };
        if !only(layout, &["sections", "fields"]) {
            return Err(Error::Schema(
                "the layout has a key the form document does not define",
            ));
        }
        let wrong = Error::Schema("the layout is not a form document");
        if let Some(sections) = layout.get("sections") {
            for section in sections.as_array().ok_or(wrong.clone())? {
                let section = section.as_object().ok_or(wrong.clone())?;
                let text = |key: &str| section.get(key).is_none_or(Value::is_string);
                if !only(section, &["id", "title", "description"])
                    || !section.get("id").is_some_and(Value::is_string)
                    || !text("title")
                    || !text("description")
                {
                    return Err(wrong);
                }
            }
        }
        if let Some(fields) = layout.get("fields") {
            for (path, hints) in fields.as_object().ok_or(wrong.clone())? {
                let question = self
                    .field(path)
                    .ok_or(Error::Schema("the layout names a question the form lacks"))?;
                let hints = hints.as_object().ok_or(wrong.clone())?;
                if !only(hints, &["widget", "placeholder", "unit", "visibleWhen"]) {
                    return Err(Error::Schema(
                        "the layout has a hint a flat form does not take",
                    ));
                }
                if ["placeholder", "unit"]
                    .iter()
                    .any(|key| hints.get(*key).is_some_and(|hint| !hint.is_string()))
                {
                    return Err(wrong);
                }
                if let Some(widget) = hints.get("widget") {
                    let widget = widget.as_str().ok_or(wrong.clone())?;
                    let fits = if question.secret {
                        widget == "password"
                    } else {
                        PLAIN_WIDGETS.contains(&widget)
                    };
                    if !fits {
                        return Err(Error::Schema(
                            "a widget that hides, shows or relabels a question the schema says otherwise",
                        ));
                    }
                }
                if let Some(when) = hints.get("visibleWhen") {
                    let when = when.as_object().ok_or(wrong.clone())?;
                    let target = when
                        .get("field")
                        .and_then(Value::as_str)
                        .and_then(|name| self.field(name));
                    let one_condition = when.contains_key("equals") != when.contains_key("in");
                    if !only(when, &["field", "equals", "in"])
                        || !one_condition
                        || when.get("in").is_some_and(|list| !list.is_array())
                        || target.is_none_or(|target| target.secret)
                    {
                        return Err(Error::Schema(
                            "a condition that is not on a plain question of the form",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// The limits the form was read under.
    #[must_use]
    pub const fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The question named `name`.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&FormField> {
        self.fields.iter().find(|field| field.field.name == name)
    }

    /// Whether `values` are an answer to the form's plain questions: no name that is not one, none that is secret, every required one
    /// present, each of its field's type and within its bounds.
    ///
    /// # Errors
    /// [`Error::Field`].
    pub fn check_plain(&self, values: &Map<String, Value>) -> Result<(), Error> {
        for name in values.keys() {
            match self.field(name) {
                None => return Err(Error::Field("a value for a question the form lacks")),
                Some(field) if field.secret => {
                    return Err(Error::Field("a secret field holds a plain value"));
                }
                Some(_) => {}
            }
        }
        let plain = Elicitation {
            session_id: String::new(),
            tool_call_id: None,
            message: String::new(),
            title: None,
            description: None,
            fields: self
                .fields
                .iter()
                .filter(|field| !field.secret)
                .map(|field| field.field.clone())
                .collect(),
        };
        elicit::fits(&plain, values)
            .map_err(|_| Error::Field("a plain value does not fit its question"))
    }

    /// Whether `names` are the secret questions an answer sealed: none that is not one, every required one present.
    ///
    /// # Errors
    /// [`Error::Field`].
    pub fn check_sealed<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Result<(), Error> {
        let given: Vec<&str> = names.into_iter().collect();
        for name in &given {
            if !self.field(name).is_some_and(|field| field.secret) {
                return Err(Error::Field(
                    "a sealed value for a question that is not secret",
                ));
            }
        }
        if self.fields.iter().any(|field| {
            field.secret && field.field.required && !given.contains(&field.field.name.as_str())
        }) {
            return Err(Error::Field("a required secret question has no value"));
        }
        Ok(())
    }

    /// Whether `value` fits secret field `name`: UTF-8 text of its length, a whole number or a number as decimal text within its
    /// bounds, and base64 when the schema says `contentEncoding: base64`.
    ///
    /// # Errors
    /// [`Error::Field`].
    pub fn check_secret(&self, name: &str, value: &[u8]) -> Result<(), Error> {
        let field = self
            .field(name)
            .filter(|field| field.secret)
            .ok_or(Error::Field("not a secret question of the form"))?;
        if value.len() > self.limits.value_bytes {
            return Err(Error::TooLarge("a secret value"));
        }
        let text =
            core::str::from_utf8(value).map_err(|_| Error::Field("a secret value is not text"))?;
        let fit = match &field.field.kind {
            FieldKind::Text {
                min_length,
                max_length,
                ..
            } => {
                let length = text.chars().count() as u64;
                min_length.is_none_or(|least| length >= least)
                    && max_length.is_none_or(|most| length <= most)
                    && match field.encoding.as_deref() {
                        Some("base64") => base64_canonical(text),
                        Some("base64url") => base64url_canonical(text),
                        _ => true,
                    }
            }
            FieldKind::Integer {
                minimum, maximum, ..
            } => json_integer(text).is_some_and(|at| {
                minimum.is_none_or(|least| at >= least) && maximum.is_none_or(|most| at <= most)
            }),
            FieldKind::Number {
                minimum, maximum, ..
            } => json_number(text).is_some_and(|at| {
                minimum.is_none_or(|least| at >= least) && maximum.is_none_or(|most| at <= most)
            }),
            _ => false,
        };
        if fit {
            Ok(())
        } else {
            Err(Error::Field("a secret value does not fit its question"))
        }
    }
}

fn flag(property: &Map<String, Value>, key: &str) -> Result<bool, Error> {
    match property.get(key) {
        None => Ok(false),
        Some(Value::Bool(set)) => Ok(*set),
        Some(_) => Err(Error::Schema("writeOnly or x-sensitive is not a boolean")),
    }
}
