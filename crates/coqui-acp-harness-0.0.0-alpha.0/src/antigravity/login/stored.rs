//! agy's stored login: where it is, its JSON, and the fields a reading takes from it.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use std::io;
use std::path::Path;

use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{Cause, KEYRING_SERVICE, KEYRING_USER, LoginError, LoginSettings, Step, keyring};

/// The login files, in the `.gemini` folder, in the order agy's own code names them.
const FILES: [&str; 2] = [
    "jetski-standalone-oauth-token",
    "antigravity-cli/antigravity-oauth-token",
];

/// A token or a secret, held in memory only. Its `Debug` and `Display` never show it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// `value`, held.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself, to send.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("..")
    }
}

/// The fields of the login a reading uses.
pub(super) struct Login {
    pub(super) access: Option<Secret>,
    pub(super) refresh: Option<Secret>,
    /// When the access token expires, in Unix seconds; none for a token with no expiry.
    pub(super) expiry: Option<i64>,
    pub(super) auth_method: Option<String>,
    pub(super) email: Option<String>,
    pub(super) project: Option<String>,
    pub(super) tier_name: Option<String>,
}

/// agy's JSON: the current shape (`token` and the sign-in's fields) or the older bare token. Every other field is skipped.
#[derive(Deserialize)]
struct Stored {
    token: Option<Token>,
    #[serde(flatten)]
    bare: Token,
    auth_method: Option<String>,
    id_token_email: Option<String>,
    project_id: Option<String>,
    user_tier: Option<String>,
    tier_display_name: Option<String>,
}

#[derive(Deserialize)]
struct Token {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expiry: Option<String>,
}

/// The stored login: the keyring's entry or a file's, whichever holds the token that expires last (the one agy wrote last),
/// the keyring's on a tie; `None` when none holds a whole sign-in.
pub(super) fn load(settings: &LoginSettings) -> Result<Option<Login>, LoginError> {
    let mut failure = None;
    let mut found: Vec<Login> = Vec::new();
    match keyring::read(
        &settings.keyring,
        settings.keyring_timeout,
        KEYRING_SERVICE,
        KEYRING_USER,
    ) {
        Ok(Some(bytes)) => found.extend(parse(&text_of(&bytes))?),
        Ok(None) => {}
        Err(error) => failure = Some(LoginError::new(Step::Keyring, Cause::Io(error.kind()))),
    }
    if let Some(gemini) = settings.gemini() {
        for name in FILES {
            if let Some(text) = file(&gemini.join(name))? {
                found.extend(parse(&text)?);
            }
        }
    }
    let mut best: Option<Login> = None;
    for login in found {
        let later = best
            .as_ref()
            .is_none_or(|held| match (held.expiry, login.expiry) {
                (Some(held), Some(this)) => this > held,
                _ => false,
            });
        if later {
            best = Some(login);
        }
    }
    match (best, failure) {
        (Some(login), _) => Ok(Some(login)),
        (None, Some(failure)) => Err(failure),
        (None, None) => Ok(None),
    }
}

/// A login file's text, `None` when there is none.
fn file(path: &Path) -> Result<Option<String>, LoginError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(text_of(&bytes))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(LoginError::new(Step::LoginFile, Cause::Io(error.kind()))),
    }
}

/// Stored bytes as text: UTF-8, else UTF-16 (little-endian, Windows' own), with any byte-order mark and trailing NULs gone.
fn text_of(bytes: &[u8]) -> String {
    let utf8 = core::str::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))
        .map(String::from);
    let text = utf8.unwrap_or_else(|| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    });
    text.trim_start_matches('\u{feff}')
        .trim_end_matches('\0')
        .into()
}

/// The login `text` holds: `None` for one with no refresh token (agy counts it signed out) or a Google Cloud sign-in with no
/// tier or project (agy counts its onboarding unfinished).
fn parse(text: &str) -> Result<Option<Login>, LoginError> {
    let stored: Stored = serde_json::from_str(text)
        .map_err(|_| LoginError::new(Step::LoginFile, Cause::Malformed))?;
    let token = stored.token.unwrap_or(stored.bare);
    let text = |value: Option<String>| value.filter(|text| !text.is_empty());
    let Some(refresh) = text(token.refresh_token) else {
        return Ok(None);
    };
    let auth_method = text(stored.auth_method);
    let project = text(stored.project_id);
    if auth_method.as_deref() == Some("gcp")
        && (text(stored.user_tier).is_none() || project.is_none())
    {
        return Ok(None);
    }
    let expiry = match text(token.expiry) {
        None => None,
        Some(at) => {
            let at = OffsetDateTime::parse(&at, &Rfc3339)
                .map_err(|_| LoginError::new(Step::LoginFile, Cause::Malformed))?;
            // Go writes a token with no expiry as its zero time, the year 1.
            (at.year() > 1).then(|| at.unix_timestamp())
        }
    };
    Ok(Some(Login {
        access: text(token.access_token).map(Secret),
        refresh: Some(Secret(refresh)),
        expiry,
        auth_method,
        email: text(stored.id_token_email),
        project,
        tier_name: text(stored.tier_display_name),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_and_utf8_read_the_same() {
        let json = r#"{"token":{"access_token":"a","refresh_token":"r"}}"#;
        let wide: Vec<u8> = json.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(text_of(&wide), json);
        assert_eq!(text_of(json.as_bytes()), json);
    }

    #[test]
    fn the_bare_token_and_the_current_shape_both_read() {
        let bare = parse(
            r#"{"access_token":"a","refresh_token":"r","expiry":"2030-01-01T00:00:00.5+01:00"}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(bare.expiry, Some(1_893_452_400));
        assert!(bare.auth_method.is_none());
        let current = parse(
            r#"{"token":{"access_token":"a","refresh_token":"r","expiry":"0001-01-01T00:00:00Z"},"auth_method":"consumer","id_token_email":"e","tier_display_name":"P"}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(current.expiry, None);
        assert_eq!(current.email.as_deref(), Some("e"));
        assert_eq!(current.tier_name.as_deref(), Some("P"));
    }

    #[test]
    fn no_refresh_token_or_an_unfinished_cloud_sign_in_is_signed_out() {
        assert!(
            parse(r#"{"token":{"access_token":"a"}}"#)
                .unwrap()
                .is_none()
        );
        assert!(
            parse(r#"{"token":{"access_token":"a","refresh_token":"r"},"auth_method":"gcp","user_tier":"t"}"#)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            parse("not json").err(),
            Some(LoginError::new(Step::LoginFile, Cause::Malformed))
        );
    }

    #[test]
    fn secrets_never_show() {
        let secret = Secret::new("abc");
        assert_eq!(alloc::format!("{secret:?} {secret}"), "Secret(..) ..");
    }
}
