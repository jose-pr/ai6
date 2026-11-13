//! The extension's messages: what the host sends the frontend for a `request` or a `show`, and what it answers.

use super::binding::Binding;
use super::context::Context;
use super::error::Error;
use super::seal::{Recipient, Sealed};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde_json::{Map, Value};

/// The key of `_meta` in which a frontend says it takes the extension (`clientCapabilities._meta`, or `capabilities._meta` in ACP
/// version 2), and so a host serves secret tools only to a client that offers it.
pub const CAPABILITY: &str = "dev.boriken.coqui.acp/secrets";
/// The request that asks the person for values, some of them secret: the host sends it to the session's owner's frontend.
pub const REQUEST_METHOD: &str = "_coqui_acp/secret_request";
/// The first step of showing the person values, some of them secret: the host says what it will show, the frontend answers with its key.
pub const SHOW_OPEN_METHOD: &str = "_coqui_acp/secret_show_open";
/// The second step: the host sends the secret values sealed to the key the first step gave. A frontend answers it only for a session
/// and request id it has an open step pending for, and once.
pub const SHOW_DELIVER_METHOD: &str = "_coqui_acp/secret_show_deliver";
/// The extension's version, which `info` and every binding carry in their labels.
pub const VERSION: u64 = 1;

/// What a frontend offers, as it writes it under [`CAPABILITY`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Capability {
    /// It answers [`REQUEST_METHOD`].
    pub request: bool,
    /// It answers [`SHOW_OPEN_METHOD`] and [`SHOW_DELIVER_METHOD`].
    pub show: bool,
    /// The binding methods it verifies, as `request` needs, and makes, as `show` needs.
    pub bindings: Vec<String>,
}

impl Capability {
    /// Both methods and both binding methods.
    #[must_use]
    pub fn all() -> Self {
        Self {
            request: true,
            show: true,
            bindings: alloc::vec![super::BINDING_ECDSA.into(), super::BINDING_DH.into()],
        }
    }

    /// `{"version": 1, "methods": ["request", "show"], "bindings": [..]}`.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut methods = Vec::new();
        if self.request {
            methods.push(Value::from("request"));
        }
        if self.show {
            methods.push(Value::from("show"));
        }
        let mut map = Map::new();
        map.insert("version".into(), Value::from(VERSION));
        map.insert("methods".into(), Value::Array(methods));
        map.insert(
            "bindings".into(),
            Value::Array(self.bindings.iter().cloned().map(Value::String).collect()),
        );
        Value::Object(map)
    }

    /// Reads the capability from a `_meta` object, or `None` when it is absent, of another version, or offers no method: a host then
    /// serves no secret tools to that client.
    #[must_use]
    pub fn from_meta(meta: &Map<String, Value>) -> Option<Self> {
        let map = meta.get(CAPABILITY)?.as_object()?;
        if map.get("version")?.as_u64()? != VERSION {
            return None;
        }
        let methods = map.get("methods")?.as_array()?;
        let has = |name: &str| methods.iter().any(|method| method.as_str() == Some(name));
        let bindings = map
            .get("bindings")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|binding| binding.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let capability = Self {
            request: has("request"),
            show: has("show"),
            bindings,
        };
        (capability.request || capability.show).then_some(capability)
    }
}

fn object<'a>(value: &'a Value, what: &'static str) -> Result<&'a Map<String, Value>, Error> {
    value.as_object().ok_or(Error::Malformed(what))
}

fn text(map: &Map<String, Value>, key: &str, what: &'static str) -> Result<String, Error> {
    map.get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or(Error::Malformed(what))
}

fn optional_form(map: &Map<String, Value>) -> Result<Option<String>, Error> {
    match map.get("form") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(form)) => Ok(Some(form.clone())),
        Some(_) => Err(Error::Malformed("the form overlay is not JSON text")),
    }
}

fn context_of(map: &Map<String, Value>) -> Result<Context, Error> {
    Ok(Context {
        session_id: text(map, "sessionId", "the message has no session")?,
        request_id: text(map, "requestId", "the message has no id")?,
        message: text(map, "message", "the message has no message")?,
        schema: text(map, "schema", "the schema is not JSON text")?,
        form: optional_form(map)?,
        values: match map.get("values") {
            None | Some(Value::Null) => None,
            Some(Value::String(values)) => Some(values.clone()),
            Some(_) => return Err(Error::Malformed("the values are not JSON text")),
        },
    })
}

fn put_context(map: &mut Map<String, Value>, context: &Context) {
    map.insert(
        "sessionId".into(),
        Value::String(context.session_id.clone()),
    );
    map.insert(
        "requestId".into(),
        Value::String(context.request_id.clone()),
    );
    map.insert("message".into(), Value::String(context.message.clone()));
    map.insert("schema".into(), Value::String(context.schema.clone()));
    if let Some(values) = &context.values {
        map.insert("values".into(), Value::String(values.clone()));
    }
    if let Some(form) = &context.form {
        map.insert("form".into(), Value::String(form.clone()));
    }
}

/// [`REQUEST_METHOD`]'s parameters: the host asks the person for values, and sends the key to seal the secret ones to. The schema and
/// the layout are JSON text, and these exact bytes are what the receiver's binding covers.
#[derive(Clone, Debug, PartialEq)]
pub struct RequestParams {
    /// What the person is asked.
    pub context: Context,
    /// Where the secret values are sealed to.
    pub recipient: Recipient,
}

impl RequestParams {
    /// The parameters as JSON: `{"sessionId", "requestId", "message", "schema", "form"?, "recipient"}`, with `schema` and `form` as
    /// JSON text.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        put_context(&mut map, &self.context);
        map.insert("recipient".into(), self.recipient.to_value());
        Value::Object(map)
    }

    /// Reads the parameters.
    ///
    /// # Errors
    /// [`Error::Malformed`].
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let map = object(value, "the request is not an object")?;
        Ok(Self {
            context: context_of(map)?,
            recipient: Recipient::from_value(
                map.get("recipient")
                    .ok_or(Error::Malformed("the request has no recipient"))?,
            )?,
        })
    }
}

/// What the person answered to a request: values by field, a secret one sealed, and the sender's binding of the sealed ones.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // a reply is built once and read once; boxing the answer would only cost its users a deref
pub enum RequestReply {
    /// They filled in the form.
    Accept {
        /// The plain answers as the JSON object text that is sent: these exact bytes are what `auth` covers.
        plain: String,
        /// The secret answers, sealed to the request's key.
        sealed: BTreeMap<String, Sealed>,
        /// The sender's binding of the whole answer, as [`Collector::authenticate`](super::Collector::authenticate) makes it.
        auth: Binding,
    },
    /// They refused to answer.
    Decline,
    /// They dismissed it.
    Cancel,
}

impl RequestReply {
    /// `{"action": "accept", "plain": "<JSON text>", "values": {..}, "auth": {..}}`, `{"action": "decline"}` or `{"action": "cancel"}`.
    /// `values` holds only the sealed values, each `{"enc", "ct"}`.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        match self {
            Self::Accept {
                plain,
                sealed,
                auth,
            } => {
                map.insert("action".into(), "accept".into());
                map.insert("plain".into(), Value::String(plain.clone()));
                let values: Map<String, Value> = sealed
                    .iter()
                    .map(|(name, value)| (name.clone(), value.to_value()))
                    .collect();
                map.insert("values".into(), Value::Object(values));
                map.insert("auth".into(), auth.to_value());
            }
            Self::Decline => {
                map.insert("action".into(), "decline".into());
            }
            Self::Cancel => {
                map.insert("action".into(), "cancel".into());
            }
        }
        Value::Object(map)
    }

    /// Reads an answer. `is_secret` says which fields are secret (the form's): `values` must hold only those, sealed.
    ///
    /// # Errors
    /// [`Error::Malformed`].
    pub fn from_value(value: &Value, is_secret: impl Fn(&str) -> bool) -> Result<Self, Error> {
        let map = object(value, "the answer is not an object")?;
        match map.get("action").and_then(Value::as_str) {
            Some("decline") => Ok(Self::Decline),
            Some("cancel") => Ok(Self::Cancel),
            Some("accept") => {
                let values = object(
                    map.get("values")
                        .ok_or(Error::Malformed("the answer has no values"))?,
                    "the values are not an object",
                )?;
                let mut sealed = BTreeMap::new();
                for (name, value) in values {
                    if !is_secret(name) {
                        return Err(Error::Malformed("a plain value outside the plain text"));
                    }
                    sealed.insert(name.clone(), Sealed::from_value(value)?);
                }
                Ok(Self::Accept {
                    plain: text(map, "plain", "the answer has no plain values")?,
                    sealed,
                    auth: Binding::from_value(
                        map.get("auth")
                            .ok_or(Error::Malformed("the answer has no auth"))?,
                    )?,
                })
            }
            _ => Err(Error::Malformed(
                "the answer's action is not accept, decline or cancel",
            )),
        }
    }
}

/// [`SHOW_OPEN_METHOD`]'s parameters: the host tells the frontend what it will show, without the secret values, and asks for the key to
/// seal them to. The values that are not secret are `context.values`, the JSON object text as sent, covered by the receiver's binding.
#[derive(Clone, Debug, PartialEq)]
pub struct ShowOpen {
    /// What the person is shown.
    pub context: Context,
}

impl ShowOpen {
    /// The parameters as JSON, as [`RequestParams`] with `values` (JSON text) and no recipient.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        put_context(&mut map, &self.context);
        Value::Object(map)
    }

    /// Reads the parameters.
    ///
    /// # Errors
    /// [`Error::Malformed`].
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let map = object(value, "the show is not an object")?;
        let context = context_of(map)?;
        if context.values.is_none() {
            return Err(Error::Malformed("the show has no values"));
        }
        Ok(Self { context })
    }
}

/// The answer to [`SHOW_OPEN_METHOD`].
#[derive(Clone, Debug, PartialEq)]
pub enum ShowOpened {
    /// The frontend will show it: the key to seal the secret values to.
    Ready(Recipient),
    /// The person refused to see it.
    Decline,
    /// They dismissed it.
    Cancel,
}

impl ShowOpened {
    /// `{"action": "ready", "recipient": ..}`, `{"action": "decline"}` or `{"action": "cancel"}`.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        match self {
            Self::Ready(recipient) => {
                map.insert("action".into(), "ready".into());
                map.insert("recipient".into(), recipient.to_value());
            }
            Self::Decline => {
                map.insert("action".into(), "decline".into());
            }
            Self::Cancel => {
                map.insert("action".into(), "cancel".into());
            }
        }
        Value::Object(map)
    }

    /// Reads the answer.
    ///
    /// # Errors
    /// [`Error::Malformed`].
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let map = object(value, "the answer is not an object")?;
        match map.get("action").and_then(Value::as_str) {
            Some("ready") => Ok(Self::Ready(Recipient::from_value(
                map.get("recipient")
                    .ok_or(Error::Malformed("the answer has no recipient"))?,
            )?)),
            Some("decline") => Ok(Self::Decline),
            Some("cancel") => Ok(Self::Cancel),
            _ => Err(Error::Malformed(
                "the answer's action is not ready, decline or cancel",
            )),
        }
    }
}

/// [`SHOW_DELIVER_METHOD`]'s parameters: the secret values, sealed to the key the open step gave, and the host's binding of them.
#[derive(Clone, Debug, PartialEq)]
pub struct ShowDeliver {
    /// The session the show belongs to.
    pub session_id: String,
    /// The show's id, as the open step gave it.
    pub request_id: String,
    /// The secret values, by field.
    pub sealed: BTreeMap<String, Sealed>,
    /// The host's binding of `sealed`, as [`Collector::authenticate`](super::Collector::authenticate) makes it.
    pub auth: Binding,
}

impl ShowDeliver {
    /// The parameters as JSON.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("sessionId".into(), Value::String(self.session_id.clone()));
        map.insert("requestId".into(), Value::String(self.request_id.clone()));
        let values: Map<String, Value> = self
            .sealed
            .iter()
            .map(|(name, value)| (name.clone(), value.to_value()))
            .collect();
        map.insert("values".into(), Value::Object(values));
        map.insert("auth".into(), self.auth.to_value());
        Value::Object(map)
    }

    /// Reads the parameters.
    ///
    /// # Errors
    /// [`Error::Malformed`].
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let map = object(value, "the show is not an object")?;
        let values = object(
            map.get("values")
                .ok_or(Error::Malformed("the show has no values"))?,
            "the values are not an object",
        )?;
        Ok(Self {
            session_id: text(map, "sessionId", "the show has no session")?,
            request_id: text(map, "requestId", "the show has no id")?,
            sealed: values
                .iter()
                .map(|(name, value)| Ok((name.clone(), Sealed::from_value(value)?)))
                .collect::<Result<_, Error>>()?,
            auth: Binding::from_value(
                map.get("auth")
                    .ok_or(Error::Malformed("the show has no auth"))?,
            )?,
        })
    }
}

/// The answer to [`SHOW_DELIVER_METHOD`], given when the person has seen the values and closed them, or refused to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowDone {
    /// They saw them.
    Shown,
    /// They dismissed it unseen.
    Cancel,
}

impl ShowDone {
    /// `{"action": "shown"}` or `{"action": "cancel"}`.
    #[must_use]
    pub fn to_value(self) -> Value {
        let mut map = Map::new();
        map.insert(
            "action".into(),
            if self == Self::Shown {
                "shown"
            } else {
                "cancel"
            }
            .into(),
        );
        Value::Object(map)
    }

    /// Reads the answer.
    ///
    /// # Errors
    /// [`Error::Malformed`].
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        match object(value, "the answer is not an object")?
            .get("action")
            .and_then(Value::as_str)
        {
            Some("shown") => Ok(Self::Shown),
            Some("cancel") => Ok(Self::Cancel),
            _ => Err(Error::Malformed(
                "the answer's action is not shown or cancel",
            )),
        }
    }
}
