//! How each harness takes an account: its kinds, the variables and files that apply them, and its sign-in.
//!
//! A harness is described as data ([`HarnessDef`]): the catalog, the settings schemas and the checks on a grant all come
//! from the same table, so what a kind lists is what it applies.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use coqui_acp_api::guatiao::library::ProviderError;
use coqui_acp_api::guatiao::{Secret, Status};
use coqui_acp_api::{
    AccountCatalog, AccountGrant, AccountKindInfo, AccountRef, EndpointSlot, Environment,
    HARNESS_LOGIN, LoginMethodInfo,
};
use serde_json::{Map, Value, json};

use crate::provider::{credential, schema};

pub mod antigravity;
pub mod grok;

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
}

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

/// One kind of account a harness takes.
#[derive(Debug)]
pub struct KindDef {
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

fn property(setting: &Setting) -> Value {
    let mut property = json!({"type": "string"});
    if setting.url {
        property["format"] = json!("uri");
    }
    if !setting.choices.is_empty() {
        property["enum"] = json!(setting.choices);
    }
    property
}

impl KindDef {
    fn schema(&self) -> Value {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for credential_def in self.credentials {
            properties.insert(credential_def.key.to_string(), credential());
            if credential_def.required {
                required.push(credential_def.key);
            }
        }
        for setting in self.settings {
            properties.insert(setting.name.to_string(), property(setting));
            if setting.required {
                required.push(setting.name);
            }
        }
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    fn info(&self) -> AccountKindInfo {
        let mut info = AccountKindInfo::new(self.id, self.title);
        info.description = self.description.to_string();
        info.settings = schema(&self.schema());
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
        info
    }
}

/// The catalog of `def`, with these login methods.
#[must_use]
pub fn catalog(def: &HarnessDef, logins: Vec<LoginMethodInfo>) -> AccountCatalog {
    let mut catalog = AccountCatalog::new();
    catalog.kinds.extend(def.kinds.iter().map(KindDef::info));
    catalog.logins = logins;
    catalog.credential_variables = def.variables.iter().map(|v| (*v).to_string()).collect();
    catalog
}

fn bad(why: impl Into<String>) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &why.into())
}

/// Whether `text` is an absolute `http` or `https` URL.
fn is_url(text: &str) -> bool {
    (text.starts_with("https://") || text.starts_with("http://"))
        && text.len() > 8
        && text.len() <= 2048
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
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
    def.kind(&account.kind)
        .map(Some)
        .ok_or_else(|| bad("this harness takes no account of that kind"))
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
        if setting.url && !is_url(text) {
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

/// The names under which a login file may say whose login it is. Not measured: no signed-in Grok file was seen, so these
/// are the usual ones, and a file with none is not checked.
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
        assert!(same_name("GROK_HOME", "GROK_HOME"));
        assert_eq!(same_name("GROK_HOME", "grok_home"), cfg!(windows));
    }

    #[test]
    fn what_is_applied_shows_names_and_no_value() {
        let grant = AccountGrant::new("a", "xai-api-key")
            .credentials(coqui_acp_api::Credentials::from_iter([(
                "api_key",
                "secret-value-1f3a",
            )]))
            .state_dir("/some/dir");
        let applied = apply(&grok::DEF, &grant)
            .expect("it applies")
            .expect("a kind");
        let shown = alloc::format!("{applied:?}");
        assert!(shown.contains("XAI_API_KEY"), "{shown}");
        assert!(!shown.contains("secret-value-1f3a"), "{shown}");
    }
}
