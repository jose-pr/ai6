//! Credential standards: the names an account and a harness agree on.
//!
//! An account says which standards it **provides** (what it can be used as), a harness which it
//! **accepts** (what it can start on, and how each is applied to its program). A host lists an
//! account for a harness exactly when the two lists meet.
//!
//! A standard is the shape of a credential, not the party that issued it: a key for the Anthropic
//! Messages API is `anthropic-api` whether Claude Code or Pi runs on it, and a login only Claude Code
//! can use is `claude-code`. The list below is the well-known vocabulary; the set is open, and a
//! provider may name its own ([`is_standard`] is the only rule).
//!
//! Each well-known standard fixes the **credential keys** an account of it carries (so any harness
//! that accepts it reads the same keys) and the setting `base_url` for the endpoint a gateway
//! moves. A kind that provides several standards carries the keys of each.

/// A login only Claude Code takes: `credentials_file` (the `.credentials.json` of
/// `claude auth login`) or `token` (a long-lived token of `claude setup-token`).
pub const CLAUDE_CODE: &str = "claude-code";
/// The Anthropic Messages API: `api_key` (sent as `x-api-key`) or `auth_token` (sent as a bearer
/// token, to a gateway); `base_url` moves the endpoint.
pub const ANTHROPIC_API: &str = "anthropic-api";
/// A login only Codex takes: `auth_file` (the `auth.json` of `codex login`) or `access_token`.
pub const CODEX: &str = "codex";
/// The OpenAI API: `api_key`; `base_url` moves the endpoint.
pub const OPENAI_API: &str = "openai-api";
/// A login only Grok takes: `auth_json` (the `auth.json` of `grok login`).
pub const GROK: &str = "grok";
/// The xAI API: `api_key`; `base_url` moves the endpoint.
pub const XAI_API: &str = "xai-api";
/// The Gemini API: `api_key`.
pub const GEMINI_API: &str = "gemini-api";
/// A login only Antigravity takes: the one `agy` keeps itself; no credential crosses.
pub const ANTIGRAVITY: &str = "antigravity";
/// A login only Pi takes: `auth_json` (the `auth.json` of Pi's own sign-in).
pub const PI: &str = "pi";
/// A key for one of OpenCode's providers: `api_key`, read from the variable the account's `variable` setting names.
pub const OPENCODE_KEY: &str = "opencode-key";
/// A login only OpenCode takes: `auth_json` (the `auth.json` its own sign-in wrote, one entry per provider).
pub const OPENCODE: &str = "opencode";

/// One well-known standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StandardInfo {
    /// Its id.
    pub id: &'static str,
    /// Its name for people.
    pub title: &'static str,
    /// What an account of it is and who can use it.
    pub meaning: &'static str,
    /// The keys of [`Credentials`](crate::Credentials) an account of it carries: any one of them.
    pub credentials: &'static [&'static str],
}

/// The well-known standards. The list grows; no id is ever reused for something else.
pub const WELL_KNOWN: &[StandardInfo] = &[
    StandardInfo {
        id: CLAUDE_CODE,
        title: "Claude Code login",
        meaning: "A sign-in that only Claude Code takes: a Claude subscription or Console login, or a long-lived token it made.",
        credentials: &["credentials_file", "token"],
    },
    StandardInfo {
        id: ANTHROPIC_API,
        title: "Anthropic API",
        meaning: "A key or bearer token for the Anthropic Messages API, at Anthropic or at a gateway that speaks it. Any harness that speaks that API can use it.",
        credentials: &["api_key", "auth_token"],
    },
    StandardInfo {
        id: CODEX,
        title: "Codex login",
        meaning: "A sign-in that only Codex takes: a ChatGPT login or an access token it checks.",
        credentials: &["auth_file", "access_token"],
    },
    StandardInfo {
        id: OPENAI_API,
        title: "OpenAI API",
        meaning: "A key for the OpenAI API, at OpenAI or at a gateway that speaks it. Any harness that speaks that API can use it.",
        credentials: &["api_key"],
    },
    StandardInfo {
        id: GROK,
        title: "Grok login",
        meaning: "A sign-in that only Grok takes: the login `grok login` made, or one through an identity provider.",
        credentials: &["auth_json"],
    },
    StandardInfo {
        id: XAI_API,
        title: "xAI API",
        meaning: "A key for the xAI API. Any harness that speaks that API can use it.",
        credentials: &["api_key"],
    },
    StandardInfo {
        id: GEMINI_API,
        title: "Gemini API",
        meaning: "A key for the Gemini API. Any harness that speaks that API can use it.",
        credentials: &["api_key"],
    },
    StandardInfo {
        id: ANTIGRAVITY,
        title: "Antigravity login",
        meaning: "The sign-in Antigravity keeps itself where it runs; an account of it counts use and limits and hands nothing over.",
        credentials: &["auth_file"],
    },
    StandardInfo {
        id: PI,
        title: "Pi sign-in",
        meaning: "A sign-in that only Pi takes: the `auth.json` its own sign-in wrote.",
        credentials: &["auth_json"],
    },
    StandardInfo {
        id: OPENCODE_KEY,
        title: "OpenCode provider key",
        meaning: "A key for one provider OpenCode takes, which it reads from the variable the account names (`variable` setting). Only OpenCode can use it.",
        credentials: &["api_key"],
    },
    StandardInfo {
        id: OPENCODE,
        title: "OpenCode sign-in",
        meaning: "A sign-in that only OpenCode takes: the `auth.json` its own sign-in wrote, holding a key or a login for each provider it was signed in to.",
        credentials: &["auth_json"],
    },
];

/// The longest standard id, in bytes.
pub const STANDARD_MAX: usize = 64;

/// Whether `text` can be a standard id: 1 to [`STANDARD_MAX`] bytes of lowercase ASCII letters,
/// digits and single hyphens inside, starting with a letter. A well-known id is one of
/// [`WELL_KNOWN`]; any other that passes is a provider's own.
#[must_use]
pub fn is_standard(text: &str) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= STANDARD_MAX
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1] != b'-'
        && !text.contains("--")
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// The well-known standard `id`.
#[must_use]
pub fn well_known(id: &str) -> Option<&'static StandardInfo> {
    WELL_KNOWN.iter().find(|standard| standard.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_lowercase_and_hyphenated() {
        for good in ["claude-code", "openai-api", "a", "x2-y3"] {
            assert!(is_standard(good), "{good}");
        }
        for bad in ["", "Claude", "-a", "a-", "a--b", "1a", "a_b", "a b", "é"] {
            assert!(!is_standard(bad), "{bad:?}");
        }
        assert!(!is_standard(&"a".repeat(STANDARD_MAX + 1)));
    }

    #[test]
    fn every_well_known_id_is_valid_and_unique_and_documented() {
        for (at, standard) in WELL_KNOWN.iter().enumerate() {
            assert!(is_standard(standard.id), "{}", standard.id);
            assert!(!standard.meaning.is_empty() && !standard.title.is_empty());
            assert!(!standard.credentials.is_empty(), "{}", standard.id);
            assert!(
                WELL_KNOWN[..at].iter().all(|other| other.id != standard.id),
                "{} twice",
                standard.id
            );
        }
    }
}
