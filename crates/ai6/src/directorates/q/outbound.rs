//! The check text passes before an Agent's words leave AI6 for a tracker.
//!
//! A post reaches people who may be outside AI6 (a public repository's readers), and a rendered
//! image fetches its address. [`check`] refuses what must not leave: the values of secrets the
//! Station holds for the Mission, anything shaped like a credential, text with characters a
//! reader cannot see, and links to hosts the Operation did not name. A refusal names the rule,
//! never the value.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

use crate::bounds::Limit;
use crate::directorates::m::issues::OnSecret;

/// Secrets shorter than this are never matched as exact values: a short one would refuse
/// ordinary words.
const MIN_SECRET_BYTES: usize = 6;

/// The exact secret values the Station holds for a Mission.
#[derive(Debug, Clone, Default)]
pub struct SecretSet(BTreeSet<String>);

impl SecretSet {
    /// A set of values; empty and very short ones are left out.
    pub fn new(values: impl IntoIterator<Item = String>) -> Self {
        Self(
            values
                .into_iter()
                .filter(|value| value.len() >= MIN_SECRET_BYTES)
                .collect(),
        )
    }

    fn found_in(&self, text: &str) -> bool {
        self.0.iter().any(|secret| text.contains(secret.as_str()))
    }

    fn redact(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.0 {
            out = out.replace(secret.as_str(), REDACTED);
        }
        out
    }
}

/// Where links and images may point.
#[derive(Debug, Clone, Default)]
pub struct LinkRule {
    any: bool,
    hosts: BTreeSet<String>,
}

impl LinkRule {
    /// Only these hosts (compared case-insensitively, a port ignored).
    pub fn hosts(hosts: impl IntoIterator<Item = String>) -> Self {
        Self {
            any: false,
            hosts: hosts.into_iter().map(|host| host.to_lowercase()).collect(),
        }
    }

    /// Every host.
    pub fn any() -> Self {
        Self {
            any: true,
            hosts: BTreeSet::new(),
        }
    }
}

/// The rule that refused a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The text holds the value of a secret the Station holds.
    Secret,
    /// The text holds something shaped like a credential.
    CredentialPattern,
    /// The text holds characters a reader cannot see: direction controls, zero-width or tag
    /// characters, other control characters.
    InvisibleCharacters,
    /// The text links to a host the Operation did not name.
    LinkHost,
    /// The text is longer than the Operation allows.
    TooLong,
    /// The text is empty.
    Empty,
}

impl Refusal {
    /// The rule's name, as audit records it.
    pub fn rule(self) -> &'static str {
        match self {
            Self::Secret => "secret",
            Self::CredentialPattern => "credential-pattern",
            Self::InvisibleCharacters => "invisible-characters",
            Self::LinkHost => "link-host",
            Self::TooLong => "too-long",
            Self::Empty => "empty",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Secret => "the text holds the value of a secret",
            Self::CredentialPattern => "the text holds something shaped like a credential",
            Self::InvisibleCharacters => "the text holds characters a reader cannot see",
            Self::LinkHost => "the text links to a host this Operation does not allow",
            Self::TooLong => "the text is longer than this Operation allows",
            Self::Empty => "the text is empty",
        };
        write!(f, "{text} (rule {})", self.rule())
    }
}

const REDACTED: &str = "[redacted]";

#[allow(clippy::expect_used)]
static CREDENTIALS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"gh[pousr]_[A-Za-z0-9]{20,}",
        r"github_pat_[A-Za-z0-9_]{20,}",
        r"ATATT[A-Za-z0-9_=\-]{20,}",
        r"xox[abprs]-[A-Za-z0-9-]{10,}",
        r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
        r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
        r"sk-ant-[A-Za-z0-9_-]{20,}",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("a fixed credential pattern"))
    .collect()
});

#[allow(clippy::expect_used)]
static ADDRESSES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b[a-z][a-z0-9+.\-]*://[^\s)>\]"'`]+|\bdata:[^\s)>\]"'`]+"#)
        .expect("a fixed address pattern")
});

#[allow(clippy::expect_used)]
static MENTIONS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^`\w@/.])@([A-Za-z0-9][A-Za-z0-9_\-/]*)").expect("a fixed mention pattern")
});

fn invisible(character: char) -> bool {
    let code = character as u32;
    (character.is_control() && !matches!(character, '\n' | '\t' | '\r'))
        || matches!(code, 0x00AD | 0x034F | 0x061C | 0x180E)
        || (0x200B..=0x200F).contains(&code)
        || (0x202A..=0x202E).contains(&code)
        || (0x2060..=0x206F).contains(&code)
        || code == 0xFEFF
        || (0xFE00..=0xFE0F).contains(&code)
        || (0xE0000..=0xE007F).contains(&code)
}

fn host_of(address: &str) -> Option<String> {
    let rest = address.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// Checks `text` and returns it ready to post: mentions put in backticks so nobody is
/// pinged, and, under [`OnSecret::Redact`], secrets replaced.
///
/// # Errors
///
/// The first rule the text breaks, in the order: empty, too long, invisible characters, a
/// link to another host, a secret's value, a credential pattern.
pub fn check(
    text: &str,
    secrets: &SecretSet,
    links: &LinkRule,
    max_bytes: Limit,
    on_secret: OnSecret,
) -> Result<String, Refusal> {
    if text.trim().is_empty() {
        return Err(Refusal::Empty);
    }
    if !max_bytes.allows(text.len()) {
        return Err(Refusal::TooLong);
    }
    if text.chars().any(invisible) {
        return Err(Refusal::InvisibleCharacters);
    }
    if !links.any {
        for found in ADDRESSES.find_iter(text) {
            let allowed = host_of(found.as_str()).is_some_and(|host| links.hosts.contains(&host))
                && found.as_str().to_lowercase().starts_with("http");
            if !allowed {
                return Err(Refusal::LinkHost);
            }
        }
    }
    let mut checked = text.to_owned();
    if secrets.found_in(&checked) {
        match on_secret {
            OnSecret::Refuse => return Err(Refusal::Secret),
            OnSecret::Redact => checked = secrets.redact(&checked),
        }
    }
    for pattern in CREDENTIALS.iter() {
        if pattern.is_match(&checked) {
            match on_secret {
                OnSecret::Refuse => return Err(Refusal::CredentialPattern),
                OnSecret::Redact => {
                    checked = pattern.replace_all(&checked, REDACTED).into_owned();
                }
            }
        }
    }
    Ok(MENTIONS.replace_all(&checked, "$1`@$2`").into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_default(text: &str) -> Result<String, Refusal> {
        check(
            text,
            &SecretSet::new(["s3cr3t-value-123".to_owned()]),
            &LinkRule::hosts(["github.com".to_owned(), "example.atlassian.net".to_owned()]),
            Limit::at(200),
            OnSecret::Refuse,
        )
    }

    #[test]
    fn ordinary_text_passes_and_mentions_are_defused() {
        assert_eq!(
            check_default("Fixed in the branch. cc @maint and @org/team.").unwrap(),
            "Fixed in the branch. cc `@maint` and `@org/team`."
        );
        assert_eq!(
            check_default("`@kept` and a@b.example").unwrap(),
            "`@kept` and a@b.example"
        );
    }

    #[test]
    fn a_held_secret_is_refused_by_value_and_redacted_when_the_operation_says_so() {
        assert_eq!(
            check_default("the key is s3cr3t-value-123").unwrap_err(),
            Refusal::Secret
        );
        let redacted = check(
            "the key is s3cr3t-value-123",
            &SecretSet::new(["s3cr3t-value-123".to_owned()]),
            &LinkRule::any(),
            Limit::at(200),
            OnSecret::Redact,
        )
        .unwrap();
        assert_eq!(redacted, "the key is [redacted]");
        assert!(
            !Refusal::Secret.to_string().contains("s3cr3t"),
            "the refusal names the rule, never the value"
        );
    }

    #[test]
    fn credential_shapes_are_refused_without_a_known_value() {
        for secret in [
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "github_pat_abcdefghijklmnopqrstuvwxyz",
            "ATATT3xFfGF0abcdefghijklmnopqrstuvwx",
            "xoxb-1234567890-abcdefghij",
            "AKIAABCDEFGHIJKLMNOP",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r",
        ] {
            assert_eq!(
                check_default(&format!("here: {secret}")).unwrap_err(),
                Refusal::CredentialPattern,
                "{secret}"
            );
        }
        assert!(check_default("a normal ghp_ reference").is_ok());
    }

    #[test]
    fn links_and_images_stay_on_the_named_hosts() {
        assert!(
            check_default("see https://github.com/o/r/pull/3 and https://GitHub.com/x").is_ok()
        );
        for bad in [
            "![x](https://evil.example/p.png?d=1)",
            "http://localhost:8080/x",
            "https://github.com.evil.example/x",
            "https://user@evil.example/x",
            "ftp://github.com/x",
            "![x](data:image/png;base64,AAAA)",
        ] {
            assert_eq!(check_default(bad).unwrap_err(), Refusal::LinkHost, "{bad}");
        }
        assert!(
            check(
                "![x](https://anywhere.example/p.png)",
                &SecretSet::default(),
                &LinkRule::any(),
                Limit::at(200),
                OnSecret::Refuse
            )
            .is_ok()
        );
    }

    #[test]
    fn invisible_characters_empty_and_long_text_are_refused() {
        for text in [
            "a\u{202E}b",
            "a\u{200B}b",
            "a\u{E0041}b",
            "a\u{0007}b",
            "a\u{FEFF}b",
        ] {
            assert_eq!(
                check_default(text).unwrap_err(),
                Refusal::InvisibleCharacters
            );
        }
        assert!(check_default("two\nlines\tand a tab").is_ok());
        assert_eq!(check_default("   ").unwrap_err(), Refusal::Empty);
        assert_eq!(
            check_default(&"x".repeat(201)).unwrap_err(),
            Refusal::TooLong
        );
    }

    #[test]
    fn short_secrets_are_not_matched_as_values() {
        let set = SecretSet::new(["abc".to_owned(), String::new()]);
        assert!(
            check(
                "abc",
                &set,
                &LinkRule::any(),
                Limit::at(10),
                OnSecret::Refuse
            )
            .is_ok()
        );
    }
}
