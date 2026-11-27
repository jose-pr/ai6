//! How each harness takes an account: its kinds, the variables and files that apply them, and its sign-in.
//!
//! A harness is described as data ([`HarnessDef`]): the catalog, the settings schemas and the checks on a grant all come
//! from the same table, so what a kind lists is what it applies.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use coqui_acp_api::guatiao::library::ProviderError;
use coqui_acp_api::guatiao::{Secret, Status};
use coqui_acp_api::{
    Accepted, AcceptedSlot, AccountCatalog, AccountGrant, AccountKindInfo, AccountRef,
    EndpointSlot, Environment, HARNESS_LOGIN,
};
use serde_json::{Map, Value, json};

use coqui_acp_api::guatiao::schema::{FieldBuilder, KindBuilder, SchemaBuilder};

use crate::provider::schema;

mod keeping;

pub use keeping::Keeping;

/// The longest text a setting or a credential may be.
const MAX_TEXT: usize = 1 << 20;

/// A credential a kind takes: a key of the grant's credentials.
#[derive(Debug)]
pub struct Credential {
    /// The key.
    pub key: &'static str,
    /// Whether a grant without it is refused.
    pub required: bool,
}

/// A plain setting a kind takes.
#[derive(Debug)]
pub struct Setting {
    /// The key.
    pub name: &'static str,
    /// Whether a grant without it is refused.
    pub required: bool,
    /// Whether it is an absolute `http` or `https` URL.
    pub url: bool,
    /// The only values it takes, when it takes only some.
    pub choices: &'static [&'static str],
    /// Values a form offers while it takes any other: listed as the schema's `examples`, never checked.
    pub suggestions: &'static [&'static str],
    /// The standards an account provides for each value of this setting, as `(value, standard)` pairs: listed as the
    /// schema's [`PROVIDES_BY_VALUE`] and read by [`provided`]. A value with no pair provides what the kind does.
    pub provides: &'static [(&'static str, &'static str)],
}

pub use coqui_acp_api::PROVIDES_BY_VALUE;

/// A URL a kind talks to, as [`EndpointSlot`].
#[derive(Debug)]
pub struct Endpoint {
    /// `model_api`, `auth` or `proxy`.
    pub purpose: &'static str,
    /// The setting holding it.
    pub key: Option<&'static str>,
    /// What it is without the setting.
    pub default: Option<&'static str>,
}

/// How one credential key of a standard reaches a harness, as [`AcceptedSlot`].
#[derive(Debug)]
pub struct SlotDef {
    /// The credential key of the standard.
    pub credential: &'static str,
    /// The harness's own kind that applies it.
    pub kind: &'static str,
    /// The variable the credential is set in.
    pub variable: Option<&'static str>,
    /// The file in the directory its text is written to.
    pub file: Option<&'static str>,
    /// The setting of the kind that holds the account's `base_url`.
    pub base_url_setting: Option<&'static str>,
    /// The variable the base URL ends up in.
    pub base_url_variable: Option<&'static str>,
    /// The variable that names the private directory.
    pub dir_variable: Option<&'static str>,
    /// Settings of the kind the standard fixes.
    pub settings: &'static [(&'static str, &'static str)],
}

/// A standard a harness starts on, as [`Accepted`].
#[derive(Debug)]
pub struct AcceptDef {
    /// The standard.
    pub standard: &'static str,
    /// How each credential key of it is applied.
    pub slots: &'static [SlotDef],
}

impl SlotDef {
    fn info(&self) -> AcceptedSlot {
        let mut slot = AcceptedSlot::new(self.credential, self.kind);
        slot.variable = self.variable.map(ToString::to_string);
        slot.file = self.file.map(ToString::to_string);
        slot.base_url_setting = self.base_url_setting.map(ToString::to_string);
        slot.base_url_variable = self.base_url_variable.map(ToString::to_string);
        slot.dir_variable = self.dir_variable.map(ToString::to_string);
        slot.settings = self
            .settings
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        slot
    }
}

/// One kind of account a harness takes.
#[derive(Debug)]
pub struct KindDef {
    /// The standards an account of it provides.
    pub provides: &'static [&'static str],
    /// The kind's id.
    pub id: &'static str,
    /// Its name for people.
    pub title: &'static str,
    /// What it is.
    pub description: &'static str,
    /// The credentials it takes.
    pub credentials: &'static [Credential],
    /// The settings it takes.
    pub settings: &'static [Setting],
    /// Whether it needs a private directory where the harness runs.
    pub state_dir: bool,
    /// Whether a renewal replaces a credential so that an older copy stops working.
    pub rotates: bool,
    /// Whether a session resumes on another account of the kind.
    pub history_portable: bool,
    /// The harness can renew it with no model turn (see [`HarnessDef::renew_args`]).
    pub renews: bool,
    /// The URLs it talks to.
    pub endpoints: &'static [Endpoint],
    /// What using it weakens.
    pub notices: &'static [&'static str],
    /// Credential key and the variable that carries it.
    pub credential_variables: &'static [(&'static str, &'static str)],
    /// Setting and the variable that carries it.
    pub setting_variables: &'static [(&'static str, &'static str)],
    /// A setting that names the variable the credential `.1` is carried in.
    pub named_variable: Option<(&'static str, &'static str)>,
    /// The variable that names the directory the harness keeps its state in; set to the grant's `state_dir`.
    pub home_variable: Option<&'static str>,
    /// The file the login is kept in, in `state_dir`, and the credential key holding its text.
    pub file: Option<(&'static str, &'static str)>,
    /// Why it cannot be renewed with no model turn, when `renews` is false.
    pub renew_unsupported: &'static str,
}

/// One harness: how it starts and what accounts it takes.
#[derive(Debug)]
pub struct HarnessDef {
    /// Its name.
    pub name: &'static str,
    /// The program started when the settings name none.
    pub program: &'static str,
    /// What follows the program for an ACP connection.
    pub args: &'static [&'static str],
    /// Its kinds, [`HARNESS_LOGIN`] aside.
    pub kinds: &'static [KindDef],
    /// The standards it starts on, most preferred first, and how each is applied.
    pub accepts: &'static [AcceptDef],
    /// The standard its own login provides ([`HARNESS_LOGIN`]), when it names one.
    pub own_standard: Option<&'static str>,
    /// Every variable any kind reads.
    pub variables: &'static [&'static str],
    /// What follows the program to renew a login with no model turn.
    pub renew_args: Option<&'static [&'static str]>,
    /// Why the harness lists no login method, when it lists none.
    pub login_unsupported: &'static str,
}

impl HarnessDef {
    /// The kind `id`.
    #[must_use]
    pub fn kind(&self, id: &str) -> Option<&KindDef> {
        self.kinds.iter().find(|kind| kind.id == id)
    }
}

fn field(setting: &Setting) -> FieldBuilder {
    let kind = if setting.choices.is_empty() {
        let text = KindBuilder::string();
        if setting.url {
            text.format("uri")
        } else {
            text
        }
    } else {
        let choices: Vec<(&str, &str)> = setting.choices.iter().map(|c| (*c, *c)).collect();
        KindBuilder::enumeration(&choices)
    };
    let mut field = FieldBuilder::new(setting.name, kind);
    if setting.required {
        field = field.required();
    }
    if !setting.suggestions.is_empty() {
        field = field.examples(setting.suggestions.iter().copied());
    }
    if !setting.provides.is_empty() {
        let mut by_value = Map::new();
        for (value, standard) in setting.provides {
            by_value
                .entry((*value).to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .expect("a list")
                .push(json!(standard));
        }
        field = field.option(PROVIDES_BY_VALUE, schema(&Value::Object(by_value)));
    }
    field
}

impl KindDef {
    fn schema(&self) -> coqui_acp_api::guatiao::Value {
        let mut builder = SchemaBuilder::new();
        for credential_def in self.credentials {
            let mut field =
                FieldBuilder::new(credential_def.key, KindBuilder::string()).sensitive();
            if credential_def.required {
                field = field.required();
            }
            builder = builder.field(field);
        }
        for setting in self.settings {
            builder = builder.field(field(setting));
        }
        builder.finish().expect("a kind's settings schema builds")
    }

    /// Whether an account of the kind provides a standard: the kind lists some, or one of its settings picks them by the
    /// value an account holds ([`PROVIDES_BY_VALUE`]).
    #[must_use]
    pub fn provides_some(&self) -> bool {
        !self.provides.is_empty()
            || self
                .settings
                .iter()
                .any(|setting| !setting.provides.is_empty())
    }

    pub(crate) fn info(&self) -> AccountKindInfo {
        let mut info = AccountKindInfo::new(self.id, self.title);
        info.description = self.description.to_string();
        info.settings = self.schema();
        info.endpoints = self
            .endpoints
            .iter()
            .map(|endpoint| {
                let mut slot = EndpointSlot::new(endpoint.purpose);
                if let Some(key) = endpoint.key {
                    slot = slot.key(key);
                }
                if let Some(default) = endpoint.default {
                    slot = slot.default_url(default);
                }
                slot
            })
            .collect();
        info.rotates = self.rotates;
        info.history_portable = self.history_portable;
        info.state_dir = self.state_dir;
        info.renews = self.renews;
        info.notices = self.notices.iter().map(|n| (*n).to_string()).collect();
        info.provides = self.provides.iter().map(|s| (*s).to_string()).collect();
        info
    }
}

/// The standards the account of kind `info` with `settings` provides: those its setting marked [`PROVIDES_BY_VALUE`] lists
/// for the value the account holds, else the kind's own. A setting that is absent, not text or has no listed value
/// leaves the kind's `provides`, and a listed name that is not a standard is left out.
#[must_use]
pub fn provided(info: &AccountKindInfo, settings: &coqui_acp_api::guatiao::Value) -> Vec<String> {
    let schema = crate::provider::to_json(&info.settings);
    let settings = crate::provider::to_json(settings);
    let listed = schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .find_map(|(name, property)| {
            let held = settings.get(name)?.as_str()?;
            property.get(PROVIDES_BY_VALUE)?.get(held)?.as_array()
        });
    listed.map_or_else(
        || info.provides.clone(),
        |standards| {
            standards
                .iter()
                .filter_map(Value::as_str)
                .filter(|id| coqui_acp_api::standard::is_standard(id))
                .map(ToString::to_string)
                .collect()
        },
    )
}

/// The catalog of `def`: what the harness takes as an account. It signs no one in and renews nothing, so it lists no
/// login method and no kind renews: an account provider ([`Keeping`]) does both.
#[must_use]
pub fn catalog(def: &HarnessDef) -> AccountCatalog {
    let mut catalog = AccountCatalog::new();
    if let Some(own) = def.own_standard
        && let Some(login) = catalog
            .kinds
            .iter_mut()
            .find(|kind| kind.id == HARNESS_LOGIN)
    {
        login.provides = alloc::vec![own.to_string()];
    }
    catalog.kinds.extend(def.kinds.iter().map(|kind| {
        let mut info = kind.info();
        info.renews = false;
        info
    }));
    catalog.accepts = def
        .accepts
        .iter()
        .map(|accept| {
            let mut accepted = Accepted::new(accept.standard);
            accepted.slots = accept.slots.iter().map(SlotDef::info).collect();
            accepted
        })
        .collect();
    catalog.credential_variables = def.variables.iter().map(|v| (*v).to_string()).collect();
    catalog
}

fn bad(why: impl Into<String>) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &why.into())
}

/// Whether `name` is one a consumer of the AOS specification owns: `AOS_*`, `*_OVERLAY_ROOT` and `*_OVERLAY_STATE`,
/// whatever the ASCII case.
fn is_reserved_variable(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("AOS_")
        || upper.ends_with("_OVERLAY_ROOT")
        || upper.ends_with("_OVERLAY_STATE")
}

/// Checks that `name` can be the variable a key is carried in: a name every system and shell takes (ASCII letters,
/// digits and underscores, no digit first) that no overlay's own variable shares. No other name is refused.
///
/// # Errors
///
/// `GUATIAO_ERR_BAD_VALUE`, saying why. The message holds the name, which is no credential.
pub fn check_variable_name(name: &str) -> Result<(), ProviderError> {
    let first = name.bytes().next();
    let usable = first.is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name.len() <= 255
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if !usable {
        return Err(bad(
            "the variable is not an environment variable name: ASCII letters, digits and underscores, no digit first",
        ));
    }
    if is_reserved_variable(name) {
        return Err(bad(alloc::format!(
            "`{name}` is a variable the host owns (`AOS_*`, `*_OVERLAY_ROOT`, `*_OVERLAY_STATE`)"
        )));
    }
    Ok(())
}

/// The kind `account` names: `None` for [`HARNESS_LOGIN`].
///
/// # Errors
///
/// `GUATIAO_ERR_BAD_VALUE`: the id is not an account id, or the kind is not one `def` lists.
pub fn known<'a>(
    def: &'a HarnessDef,
    account: &AccountRef,
) -> Result<Option<&'a KindDef>, ProviderError> {
    account.check_id()?;
    if account.kind == HARNESS_LOGIN {
        return Ok(None);
    }
    if let Some(kind) = def.kind(&account.kind) {
        return Ok(Some(kind));
    }
    // An account of an accepted standard has no kind of this harness's until `apply` reads its credential.
    catalog(def).accepts_reference(account)?;
    Ok(None)
}

/// The grant of one of `def`'s own kinds that applies `grant`: itself for an own kind, else the slot of the first
/// standard it provides that `def` accepts.
///
/// # Errors
///
/// As [`resolve`](crate::provider::resolve).
pub fn resolve_grant(
    def: &HarnessDef,
    grant: &AccountGrant,
) -> Result<AccountGrant, ProviderError> {
    if grant.kind == HARNESS_LOGIN {
        return Ok(grant.clone());
    }
    crate::provider::resolve(&catalog(def), grant)
}

/// A credential file to look at while the account is in use.
#[derive(Debug, Clone, Copy)]
pub struct Watched {
    /// Where, relative to the directory.
    pub path: &'static str,
    /// The credential key holding its text.
    pub key: &'static str,
}

/// A grant applied: what to set and what to watch. It holds credentials and shows none.
pub struct Applied {
    /// Variables to set, by name; the values are credentials.
    pub environment: Secret<Environment>,
    /// The login file to watch.
    pub watched: Option<Watched>,
    /// Whether the account needs the spawner's files.
    pub needs_files: bool,
}

impl core::fmt::Debug for Applied {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Applied")
            .field(
                "environment",
                &self.environment.expose().names().collect::<Vec<_>>(),
            )
            .field("watched", &self.watched)
            .finish_non_exhaustive()
    }
}

fn settings_of(grant: &AccountGrant) -> Result<Map<String, Value>, ProviderError> {
    match crate::provider::to_json(&grant.settings) {
        Value::Object(map) => Ok(map),
        Value::Null => Ok(Map::new()),
        _ => Err(bad("the settings are not an object")),
    }
}

/// Reads `grant` against `def`: its settings and credentials are checked against the kind's schema, and the variables
/// and files that apply it are made. `None` for [`HARNESS_LOGIN`], which hands over nothing.
///
/// # Errors
///
/// `GUATIAO_ERR_BAD_VALUE` for a kind not listed, a setting the schema refuses, a credential missing or unknown, or no
/// directory for a kind that needs one. No message shows a value.
pub fn apply(def: &HarnessDef, grant: &AccountGrant) -> Result<Option<Applied>, ProviderError> {
    let grant = &resolve_grant(def, grant)?;
    let Some(kind) = known(def, &grant.reference())? else {
        return Ok(None);
    };
    let settings = settings_of(grant)?;
    for (name, value) in &settings {
        let Some(setting) = kind.settings.iter().find(|s| s.name == name) else {
            return Err(bad(alloc::format!("`{name}` is no setting of {}", kind.id)));
        };
        let Some(text) = value.as_str() else {
            return Err(bad(alloc::format!("the setting `{name}` is not text")));
        };
        if text.len() > MAX_TEXT || text.contains('\0') {
            return Err(bad(alloc::format!("the setting `{name}` is not usable")));
        }
        if setting.url && !coqui_acp_api::is_http_url(text) {
            return Err(bad(alloc::format!(
                "the setting `{name}` is not an absolute http or https URL"
            )));
        }
        if !setting.choices.is_empty() && !setting.choices.contains(&text) {
            return Err(bad(alloc::format!(
                "the setting `{name}` is none of the values {} takes",
                kind.id
            )));
        }
    }
    if let Some((setting, _)) = kind.named_variable
        && let Some(name) = settings.get(setting).and_then(Value::as_str)
    {
        check_variable_name(name)?;
    }
    for setting in kind.settings.iter().filter(|s| s.required) {
        if !settings.contains_key(setting.name) {
            return Err(bad(alloc::format!(
                "the setting `{}` is missing",
                setting.name
            )));
        }
    }
    let credentials = grant.credentials.expose();
    for (key, value) in credentials {
        if !kind.credentials.iter().any(|c| c.key == key) {
            return Err(bad(alloc::format!(
                "`{key}` is no credential of {}",
                kind.id
            )));
        }
        if value.len() > MAX_TEXT || value.contains('\0') {
            return Err(bad(alloc::format!("the credential `{key}` is not usable")));
        }
    }
    for needed in kind.credentials.iter().filter(|c| c.required) {
        if credentials.get(needed.key).is_none_or(str::is_empty) {
            return Err(bad(alloc::format!(
                "the credential `{}` is missing",
                needed.key
            )));
        }
    }
    if kind.state_dir && grant.state_dir.is_none() {
        return Err(bad(alloc::format!(
            "a {} account needs a state_dir",
            kind.id
        )));
    }
    let text_of = |name: &str| settings.get(name).and_then(Value::as_str);
    let mut environment = Environment::new();
    for (key, variable) in kind.credential_variables {
        if let Some(value) = credentials.get(key) {
            environment.insert(*variable, value);
        }
    }
    for (setting, variable) in kind.setting_variables {
        if let Some(value) = text_of(setting) {
            environment.insert(*variable, value);
        }
    }
    if let Some((setting, key)) = kind.named_variable
        && let (Some(variable), Some(value)) = (text_of(setting), credentials.get(key))
    {
        environment.insert(variable, value);
    }
    if let (Some(variable), Some(dir)) = (kind.home_variable, &grant.state_dir) {
        environment.insert(variable, dir.as_str());
    }
    let mut watched = None;
    if let Some((path, key)) = kind.file {
        // A login that is not in the grant yet (the first start of a provider-run sign-in) has nothing to place or watch.
        if credentials.get(key).is_some() {
            watched = Some(Watched { path, key });
        }
    }
    Ok(Some(Applied {
        environment: Secret::new(environment),
        watched,
        needs_files: kind.state_dir,
    }))
}

/// Whether `a` and `b` name the same variable: exactly, and on Windows, where the system ignores case, whatever the case.
#[must_use]
pub fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Whether the text of a login file holds a login that is settled: not blank, and, when it looks like JSON, whole. A file
/// the harness is in the middle of writing (a truncated object) is not one.
#[must_use]
pub fn login_usable(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    !text.starts_with(['{', '[']) || serde_json::from_str::<Value>(text).is_ok()
}

/// The names under which a login file may say whose login it is. Not measured for every harness, so these are the
/// usual ones, and a file with none is not checked.
const IDENTITY_KEYS: [&str; 4] = ["account_id", "user_id", "sub", "email"];

/// Whose login the file's text says it is, when it says: the first of `account_id`, `user_id`, `sub` or `email` in the object or in an object one
/// level down.
#[must_use]
pub fn identity_of(text: &str) -> Option<String> {
    let Value::Object(top) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    let find = |object: &Map<String, Value>| {
        IDENTITY_KEYS
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_str))
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    };
    find(&top).or_else(|| top.values().filter_map(Value::as_object).find_map(find))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_is_settled_when_whole() {
        assert!(!login_usable(""));
        assert!(!login_usable("  \n"));
        assert!(!login_usable("{\"access"));
        assert!(!login_usable("{\"a\": [1, 2"));
        assert!(login_usable("{\"a\": 1}"));
        assert!(login_usable("some plain text a harness keeps"));
    }

    #[test]
    fn whose_login_it_is_is_read_from_the_usual_keys() {
        assert_eq!(identity_of("{\"account_id\": \"A\"}").as_deref(), Some("A"));
        assert_eq!(
            identity_of("{\"tokens\": {\"email\": \"a@x\"}}").as_deref(),
            Some("a@x")
        );
        assert_eq!(identity_of("{\"token\": \"t\"}"), None);
        assert_eq!(identity_of("not json"), None);
    }

    #[test]
    fn names_compare_by_the_systems_rules() {
        assert!(same_name("EXAMPLE_HOME", "EXAMPLE_HOME"));
        assert_eq!(same_name("EXAMPLE_HOME", "example_home"), cfg!(windows));
    }
}
