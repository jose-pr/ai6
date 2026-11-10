//! Where an overlay comes from: `[git+]<location>[@<ref>][#<path>]`
//! (`spec/v0/distribution.md`).

use crate::error::{Class, Error};
use crate::names::valid_relative_path;

/// What kind of location a source names.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A directory on the consumer's machine.
    Directory,
    /// A Git repository.
    Git,
    /// Any other URL.
    Url,
}

/// A parsed source string.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Source {
    /// What the location is.
    pub kind: SourceKind,
    /// The directory, repository or URL, as written.
    pub location: String,
    /// A Git location's branch, tag or commit; `None` is the remote's default branch.
    pub reference: Option<String>,
    /// The overlay's root inside the location, a relative path; `None` is its root.
    pub path: Option<String>,
}

/// Shows the source as [`Source::shown`] does, never the user information or query its location
/// may hold.
impl core::fmt::Debug for Source {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Source")
            .field("kind", &self.kind)
            .field("shown", &self.shown())
            .finish()
    }
}

fn is_git_location(location: &str) -> bool {
    location.ends_with(".git")
        || ["git@", "ssh://", "git://"]
            .iter()
            .any(|prefix| location.starts_with(prefix))
}

/// Characters no source holds: controls, line separators and what reorders displayed text.
fn is_source_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}')
        || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn has_drive(location: &str) -> bool {
    let mut chars = location.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next() == Some(':')
}

/// A URL's scheme, authority and the rest: the authority ends at the first `/` or `?`.
fn authority_of(location: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = location.split_once("://")?;
    let cut = rest.find(['/', '?']).unwrap_or(rest.len());
    Some((scheme, &rest[..cut], &rest[cut..]))
}

/// `host:path`, the form of a Git remote on another computer, but not `C:` or a URL.
fn host_and_path(location: &str) -> Option<(&str, &str)> {
    if has_drive(location) {
        return None;
    }
    let (host, rest) = location.split_once(':')?;
    (!host.is_empty() && !host.contains(['/', '\\'])).then_some((host, rest))
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(pair) = bytes.get(at + 1..at + 3)
            && let Ok(digits) = core::str::from_utf8(pair)
            && let Ok(byte) = u8::from_str_radix(digits, 16)
        {
            out.push(byte);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Source {
    /// Parses `text`, surrounding whitespace removed.
    ///
    /// # Errors
    ///
    /// An [`Error`] of class `invalid-source` for an empty source, one holding a control or
    /// direction-changing character, a `#path` that is no relative path, a backslash in a URL's
    /// authority, a user, host or port that reads as an option once decoded, and for a Git
    /// location: one or a ref that starts with `-`, a remote helper (`::`) or a scheme other than
    /// `https`, `http`, `ssh`, `git` and `file`.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let refuse = |why: &str| Error::new(Class::InvalidSource, why);
        let mut raw = text.trim();
        if raw.is_empty() {
            return Err(refuse("empty"));
        }
        if raw.chars().any(is_source_control) {
            return Err(refuse("control character"));
        }
        let explicit_git = raw.starts_with("git+");
        if explicit_git {
            raw = &raw["git+".len()..];
        }
        let (body, path) = match raw.split_once('#') {
            Some((body, path)) => {
                let path = path.trim().trim_matches('/');
                (body, (!path.is_empty()).then(|| path.to_owned()))
            }
            None => (raw, None),
        };
        let mut location = body.trim();
        let mut reference = None;
        if let Some((head, tail)) = location.rsplit_once('@') {
            let after_authority = authority_of(location)
                .map_or(location.len(), |(_, _, rest)| location.len() - rest.len());
            // An @ inside a URL's authority is user information, never the start of a ref.
            let in_authority = location.contains("://") && head.len() < after_authority;
            if !head.is_empty()
                && !tail.is_empty()
                && !in_authority
                && !tail.contains(['/', ':', '\\'])
                && !head.ends_with("://")
                && !head.ends_with(':')
                && (explicit_git || is_git_location(head))
            {
                location = head;
                reference = Some(tail.to_owned());
            }
        }
        if path
            .as_deref()
            .is_some_and(|path| !valid_relative_path(path))
        {
            return Err(refuse("the #path is not a relative path"));
        }
        let mut parts: Vec<&str> = Vec::new();
        if let Some((_, authority, _)) = authority_of(location) {
            if authority.contains('\\') {
                return Err(refuse("backslash in the authority"));
            }
            let (user, hostport) = authority.rsplit_once('@').unwrap_or(("", authority));
            let (host, port) = if hostport.ends_with(']') {
                (hostport, "")
            } else {
                hostport.rsplit_once(':').unwrap_or((hostport, ""))
            };
            parts = vec![user, host, port];
        } else if let Some((host, rest)) = host_and_path(location) {
            let (user, host) = host.rsplit_once('@').unwrap_or(("", host));
            parts = vec![user, host, rest];
        }
        if parts
            .iter()
            .any(|part| percent_decoded(part).starts_with('-'))
        {
            return Err(refuse("a user, host or port that reads as an option"));
        }
        if explicit_git || is_git_location(location) {
            if location.starts_with('-') || reference.as_deref().is_some_and(|r| r.starts_with('-'))
            {
                return Err(refuse("a location or ref that reads as an option"));
            }
            if location.contains("::") {
                return Err(refuse("a Git remote helper"));
            }
            if let Some((scheme, _)) = location.split_once("://")
                && !["https", "http", "ssh", "git", "file"]
                    .contains(&scheme.to_ascii_lowercase().as_str())
            {
                return Err(refuse("a scheme Git is not given"));
            }
            return Ok(Self {
                kind: SourceKind::Git,
                location: location.to_owned(),
                reference,
                path,
            });
        }
        Ok(Self {
            kind: if location.contains("://") {
                SourceKind::Url
            } else {
                SourceKind::Directory
            },
            location: location.to_owned(),
            reference: None,
            path,
        })
    }

    /// The source as it may be shown or stored: user information and a query removed from a
    /// URL location.
    #[must_use]
    pub fn shown(&self) -> String {
        self.shown_with(self.reference.as_deref())
    }

    /// As [`Source::shown`], with `reference` in place of the source's own ref.
    #[must_use]
    pub fn shown_with(&self, reference: Option<&str>) -> String {
        let mut location = self.location.clone();
        if let Some((scheme, authority, rest)) = authority_of(&self.location) {
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            let rest = rest.split('?').next().unwrap_or_default();
            location = format!("{scheme}://{host}{rest}");
        }
        let mut shown = location;
        if let Some(reference) = reference.filter(|reference| !reference.is_empty()) {
            shown.push('@');
            shown.push_str(reference);
        }
        if let Some(path) = &self.path {
            shown.push('#');
            shown.push_str(path);
        }
        shown
    }

    /// The source in full, as it is fetched: what two entries must share to be one. `reference`
    /// stands in for the source's own ref when given.
    #[must_use]
    pub fn key_with(&self, reference: Option<&str>) -> String {
        let mut key = self.location.clone();
        if let Some(reference) = reference
            .or(self.reference.as_deref())
            .filter(|reference| !reference.is_empty())
        {
            key.push('@');
            key.push_str(reference);
        }
        if let Some(path) = &self.path {
            key.push('#');
            key.push_str(path);
        }
        key
    }

    /// Whether the source is on this computer: a directory, a local Git path, any `file:`
    /// location, or a host that is this machine.
    #[must_use]
    pub fn is_local(&self) -> bool {
        let location = self.location.as_str();
        if location.to_ascii_lowercase().starts_with("file:") || self.kind == SourceKind::Directory
        {
            return true;
        }
        if self.kind == SourceKind::Git && local_git(location) {
            return true;
        }
        host_is_local(source_host(location).as_deref())
    }

    /// Whether the location shows a credential: a query, a password anywhere, or a user over a
    /// web URL. `ssh://git@host` and `git@host:path` name an account, not a secret.
    #[must_use]
    pub fn holds_credentials(&self) -> bool {
        let Some((scheme, authority, rest)) = authority_of(&self.location) else {
            return false;
        };
        let user = authority.rsplit_once('@').map_or("", |(user, _)| user);
        rest.contains('?')
            || user.contains(':')
            || (!user.is_empty()
                && (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")))
    }

    /// Whether the location is a web URL, `http` or `https`.
    #[must_use]
    pub fn is_web_url(&self) -> bool {
        let lower = self.location.to_ascii_lowercase();
        lower.starts_with("http://") || lower.starts_with("https://")
    }
}

/// A Git location on this machine: a path or `file:`, not a URL or `host:path`.
fn local_git(location: &str) -> bool {
    if let Some((scheme, _)) = location.split_once("://") {
        return scheme.eq_ignore_ascii_case("file");
    }
    has_drive(location) || host_and_path(location).is_none()
}

/// The host a source reaches, lowercased and without brackets, or none for a path.
pub(crate) fn source_host(location: &str) -> Option<String> {
    let hostport = if let Some((_, authority, _)) = authority_of(location) {
        authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host)
    } else if let Some((host, _)) = host_and_path(location) {
        let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
        return (!host.is_empty()).then(|| host.to_ascii_lowercase());
    } else {
        return None;
    };
    if let Some(inner) = hostport.strip_prefix('[') {
        return Some(
            inner
                .split(']')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase(),
        );
    }
    Some(
        hostport
            .rsplit_once(':')
            .map_or(hostport, |(host, _)| host)
            .to_ascii_lowercase(),
    )
}

/// One part of a host written as a URL parser reads it: decimal, `0x` hexadecimal, or octal with a
/// leading zero.
fn ipv4_part(part: &str) -> Option<u64> {
    let (digits, base) =
        if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            (if hex.is_empty() { "0" } else { hex }, 16)
        } else if part.len() > 1 && part.starts_with('0') {
            (&part[1..], 8)
        } else {
            (part, 10)
        };
    if digits.len() > 12 {
        return None;
    }
    u64::from_str_radix(digits, base)
        .ok()
        .filter(|_| digits.chars().all(|c| c.is_digit(base)))
}

/// The IPv4 address a URL parser reads `host` as: one to four parts, each decimal, hexadecimal or
/// octal, the last filling the bytes the others leave (`127.1`, `2130706433`, `0x7f.0.0.1`,
/// `0177.0.0.1`), one trailing dot allowed.
fn url_ipv4(host: &str) -> Option<core::net::Ipv4Addr> {
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
    }
    if !(1..=4).contains(&parts.len()) || parts.contains(&"") {
        return None;
    }
    let numbers: Vec<u64> = parts
        .iter()
        .map(|part| ipv4_part(part))
        .collect::<Option<_>>()?;
    let (last, head) = numbers.split_last()?;
    if head.iter().any(|number| *number > 255) || *last >= 256u64.pow(5 - parts.len() as u32) {
        return None;
    }
    let mut value = *last;
    for (position, number) in head.iter().enumerate() {
        value += number * 256u64.pow(3 - position as u32);
    }
    u32::try_from(value).ok().map(core::net::Ipv4Addr::from)
}

/// The address a host names: an IP literal, or an IPv4 address as a URL parser reads it.
fn address_of(host: &str) -> Option<core::net::IpAddr> {
    use core::net::IpAddr;
    host.parse::<IpAddr>()
        .ok()
        .or_else(|| url_ipv4(host).map(IpAddr::V4))
}

/// Whether a host is this machine: loopback, link-local, unspecified or `localhost`, judged on
/// the text alone.
fn host_is_local(host: Option<&str>) -> bool {
    use core::net::{IpAddr, Ipv6Addr};
    let Some(host) = host else {
        return false;
    };
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match address_of(host) {
        Some(IpAddr::V4(address)) => {
            address.is_loopback() || address.is_link_local() || address.is_unspecified()
        }
        Some(IpAddr::V6(address)) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return mapped.is_loopback() || mapped.is_link_local() || mapped.is_unspecified();
            }
            let link_local = (address.segments()[0] & 0xffc0) == 0xfe80;
            address.is_loopback()
                || address.is_unspecified()
                || link_local
                || address == Ipv6Addr::LOCALHOST
        }
        None => false,
    }
}

/// Whether a host is on a private network, judged on the text alone: not this machine, which
/// [`Source::is_local`] decides first.
pub(crate) fn host_is_private(host: Option<&str>) -> bool {
    use core::net::{IpAddr, Ipv4Addr};
    let Some(host) = host else {
        return false;
    };
    if host_is_local(Some(host)) {
        return false;
    }
    let v4 = |address: Ipv4Addr| {
        let [a, b, c, _] = address.octets();
        a == 0
            || a == 10
            || (a == 100 && (64..128).contains(&b))
            || (a == 172 && (16..32).contains(&b))
            || (a == 192 && b == 168)
            || (a == 169 && b == 254)
            || (a == 192 && b == 0 && c == 0)
            || (a == 192 && b == 0 && c == 2)
            || (a == 198 && (b == 18 || b == 19))
            || (a == 198 && b == 51 && c == 100)
            || (a == 203 && b == 0 && c == 113)
            || a >= 240
    };
    match address_of(host) {
        Some(IpAddr::V4(address)) => v4(address),
        Some(IpAddr::V6(address)) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return v4(mapped);
            }
            let s = address.segments();
            (s[0] & 0xfe00) == 0xfc00
                || (s[0] == 0x2001 && s[1] == 0x0db8)
                || (s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0)
                || s[0] == 0x2002
        }
        None => false,
    }
}
