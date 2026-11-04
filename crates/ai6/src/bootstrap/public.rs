//! The public `ai6.cfg` beside a web page, written from a host's file by `ai6 config public`.
//!
//! A page can fetch only what is served, and a host's file names files and addresses that are not
//! the public's business. So this is an allow list, and a new key is private until it is named
//! here: the gateway's address, and of `[oo]` its tables' numbers and booleans and the text settings
//! `PUBLIC_TEXT` names. Every other value a person writes under `[oo]` is left out, so a secret
//! filed there by mistake goes no further.

use std::fmt;

use serde_json::{Map, Value};

use super::Bootstrap;
use super::format::{self, Format, FormatError};

/// The text settings under `[oo]` the public file keeps, as `(table, key)`, each a single word.
const PUBLIC_TEXT: [(&str, &str); 2] = [("appearance", "theme"), ("assets", "cdn")];

/// Why a public file could not be written. It names the key, never the value.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicError {
    /// The `[unex]` key named, `public_origin` or `url`, holds more than an origin, which may be a
    /// credential.
    NotAnOrigin(&'static str),
    /// The document could not be written.
    Format(FormatError),
}

impl fmt::Display for PublicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnOrigin(key) => write!(
                formatter,
                "[unex] {key} holds a credential, a query or a fragment, which a public file must \
                 not; name the origin alone"
            ),
            Self::Format(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PublicError {}

/// The public file of `bootstrap`, and the keys of `[oo]` it left out.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFile {
    /// The document, as TOML.
    pub text: String,
    /// Each key under `[oo]` the allow list refused, such as `oo.chat.note`, never its value.
    pub left_out: Vec<String>,
}

/// Whether `url` is an origin with nothing else that could carry a secret.
fn is_plain_origin(url: &str) -> bool {
    let authority = url.split_once("://").map_or(url, |(_, rest)| rest);
    !authority.contains(['@', '?', '#'])
}

/// Whether a text setting is one word: what no credential of any strength looks like in practice,
/// and all the page's text settings take.
fn is_word(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What the allow list keeps of one `[oo]` table.
fn public_table(
    name: &str,
    table: &Map<String, Value>,
    left_out: &mut Vec<String>,
) -> Map<String, Value> {
    let mut kept = Map::new();
    for (key, value) in table {
        let allowed = match value {
            Value::Number(_) | Value::Bool(_) => true,
            Value::String(text) => PUBLIC_TEXT.contains(&(name, key.as_str())) && is_word(text),
            _ => false,
        };
        if allowed {
            kept.insert(key.clone(), value.clone());
        } else {
            left_out.push(format!("oo.{name}.{key}"));
        }
    }
    kept
}

/// Writes the public file of `bootstrap`. Its `[unex] url` is the host's `[unex] public_origin`,
/// the address browsers reach the gateway at, else the host's `[unex] url`.
///
/// # Errors
///
/// [`PublicError::NotAnOrigin`] when the address it would write holds a credential, a query or a
/// fragment.
pub fn public_file(bootstrap: &Bootstrap) -> Result<PublicFile, PublicError> {
    let mut document = Map::new();
    let address = match (&bootstrap.unex.public_origin, &bootstrap.unex.url) {
        (Some(origin), _) => Some(("public_origin", origin)),
        (None, Some(url)) => Some(("url", url)),
        (None, None) => None,
    };
    if let Some((key, url)) = address {
        if !is_plain_origin(url) {
            return Err(PublicError::NotAnOrigin(key));
        }
        let mut unex = Map::new();
        unex.insert("url".to_owned(), Value::String(url.clone()));
        document.insert("unex".to_owned(), Value::Object(unex));
    }
    let mut left_out = Vec::new();
    let mut oo = Map::new();
    for (name, value) in &bootstrap.oo {
        let Value::Object(table) = value else {
            left_out.push(format!("oo.{name}"));
            continue;
        };
        let kept = public_table(name, table, &mut left_out);
        if !kept.is_empty() {
            oo.insert(name.clone(), Value::Object(kept));
        }
    }
    if !oo.is_empty() {
        document.insert("oo".to_owned(), Value::Object(oo));
    }
    let text =
        format::to_string(Format::Toml, &Value::Object(document)).map_err(PublicError::Format)?;
    Ok(PublicFile { text, left_out })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;

    /// A value a person could keep in a file, and what no public file may hold.
    const PLANTED: &str = "pl4nted-s3cret-value";

    /// Every section filled, with [`PLANTED`] wherever a person could put a value.
    fn whole() -> String {
        format!(
            r#"
[provider]
location = "postgres://user:{PLANTED}@db/ai6"
password = {{ provider = "inline", value = "{PLANTED}" }}
poll = 5

[registry]
location = "postgres://user:{PLANTED}@db/registry"
password = {{ provider = "file", key = "{PLANTED}" }}
headquarters_lease = 60

[registry.limits.sessions]
records = 100000

[unex]
url = "http://{PLANTED}.internal:7082"
ca_file = "{PLANTED}.pem"
station_token = {{ provider = "inline", value = "{PLANTED}" }}
listen = "127.0.0.1:7082"
public_origin = "https://ai6.example"
proxy_secret = {{ provider = "inline", value = "{PLANTED}" }}
proxy_secret_min_bytes = 32
state_dir = "/{PLANTED}"

[unex.plugins]
paths = ["/{PLANTED}"]
profile = "strict"

[unex.access]
chief = ["account:{PLANTED}"]

[unex.pages]
dir = "/{PLANTED}"
origin = "https://{PLANTED}.example"
listen = "127.0.0.1:7083"

[station]
id = "{PLANTED}"
state_dir = "/{PLANTED}"
listen = "127.0.0.1:7081"

[station.plugins]
paths = ["/{PLANTED}"]
profile = "strict"

[station.identity]
dir = "/{PLANTED}"

[station.secrets]
dir = "/{PLANTED}"

[oo]
note = "{PLANTED}"

[oo.appearance]
theme = "night-ops"
note = "{PLANTED}"

[oo.assets]
cdn = "none"

[oo.chat]
width = 400
token = "{PLANTED}"
nested = {{ value = "{PLANTED}" }}
listed = ["{PLANTED}"]

[oo.refresh]
sessions = 3

[oo.limits]
reason_chars = 512
"#
        )
    }

    fn bootstrap(text: &str) -> Bootstrap {
        format::read(Path::new("ai6.cfg"), text).unwrap()
    }

    /// Every key of a document, as a dotted path.
    fn keys_of(value: &Value, path: &str, found: &mut BTreeSet<String>) {
        if let Value::Object(map) = value {
            for (key, value) in map {
                let at = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                keys_of(value, &at, found);
                if !value.is_object() {
                    found.insert(at);
                }
            }
        }
    }

    #[test]
    fn the_public_file_holds_no_key_outside_the_allow_list_from_a_file_with_every_section_filled() {
        let public = public_file(&bootstrap(&whole())).unwrap();
        assert!(!public.text.contains(PLANTED), "{}", public.text);
        let document: Value = format::from_str(Format::Toml, &public.text).unwrap();
        let mut found = BTreeSet::new();
        keys_of(&document, "", &mut found);
        let expected: BTreeSet<String> = [
            "unex.url",
            "oo.appearance.theme",
            "oo.assets.cdn",
            "oo.chat.width",
            "oo.refresh.sessions",
            "oo.limits.reason_chars",
        ]
        .map(str::to_owned)
        .into();
        assert_eq!(found, expected, "{}", public.text);
        for key in [
            "oo.note",
            "oo.appearance.note",
            "oo.chat.token",
            "oo.chat.nested",
            "oo.chat.listed",
        ] {
            assert!(public.left_out.iter().any(|left| left == key), "{key}");
        }
        assert!(
            !format!("{public:?}").contains(PLANTED),
            "what it left out is named, not quoted"
        );
    }

    #[test]
    fn the_public_file_reads_back_as_a_bootstrap_file() {
        let public = public_file(&bootstrap(&whole())).unwrap();
        let read = bootstrap(&public.text);
        assert_eq!(read.unex.url.as_deref(), Some("https://ai6.example"));
        assert_eq!(read.unex.station_token, None);
        assert_eq!(read.provider.password, None);
    }

    #[test]
    fn a_text_setting_that_is_not_a_word_stays_private() {
        let public = public_file(&bootstrap(&format!(
            "[oo.appearance]\ntheme = \"{PLANTED} and more\"\n"
        )))
        .unwrap();
        assert!(!public.text.contains(PLANTED), "{}", public.text);
        assert_eq!(public.left_out, ["oo.appearance.theme"]);
    }

    #[test]
    fn a_url_that_could_carry_a_credential_is_refused_and_not_quoted() {
        for url in [
            format!("https://user:{PLANTED}@ai6.example"),
            format!("https://ai6.example/?token={PLANTED}"),
            format!("https://ai6.example/#{PLANTED}"),
        ] {
            for key in ["url", "public_origin"] {
                let refused =
                    public_file(&bootstrap(&format!("[unex]\n{key} = \"{url}\"\n"))).unwrap_err();
                assert_eq!(refused, PublicError::NotAnOrigin(key));
                assert!(!format!("{refused} {refused:?}").contains(PLANTED));
            }
        }
    }

    #[test]
    fn the_page_reaches_the_gateway_at_its_public_origin_not_the_host_s_own_address() {
        let public = public_file(&bootstrap(
            "[unex]\nurl = \"http://127.0.0.1:7082\"\npublic_origin = \"https://ai6.example\"\n",
        ))
        .unwrap();
        assert_eq!(
            bootstrap(&public.text).unex.url.as_deref(),
            Some("https://ai6.example")
        );
        assert!(!public.text.contains("127.0.0.1"), "{}", public.text);

        // With no public origin, the host's address is the one there is.
        let public = public_file(&bootstrap("[unex]\nurl = \"https://ai6.example\"\n")).unwrap();
        assert_eq!(
            bootstrap(&public.text).unex.url.as_deref(),
            Some("https://ai6.example")
        );
    }

    #[test]
    fn a_file_with_nothing_public_writes_an_empty_document() {
        let public = public_file(&bootstrap("[station]\nid = \"a\"\n")).unwrap();
        assert_eq!(public.text.trim(), "");
    }

    /// The contract with Outer Office, whose tests read this file.
    #[test]
    fn the_app_s_copy_of_a_public_file_is_current() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config/ai6.cfg");
        let text = std::fs::read_to_string(examples).unwrap();
        let written = public_file(&bootstrap(&text)).unwrap().text;
        let copy = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../clients/outer-office/test/fixtures/public_ai6.cfg");
        if std::env::var_os("AI6_WRITE_PUBLIC_FIXTURE").is_some() {
            std::fs::write(&copy, &written).unwrap();
        }
        assert_eq!(std::fs::read_to_string(copy).unwrap(), written);
    }
}
