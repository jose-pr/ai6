//! LDAP filter and result helpers, independent of a live directory.
use std::net::IpAddr;

/// Active Directory's matching rule for nested group membership.
pub const IN_CHAIN: &str = "(member:1.2.840.113556.1.4.1941:={dn})";

/// Escapes a value for a filter (RFC 4515): the metacharacters, controls and every byte of
/// a non-ASCII character as `\xx`. The matcher's own function, so one rule escapes everywhere.
pub fn escaped(value: &str) -> String {
    coqui_access_kit::matcher::escape_filter_value(value)
}

/// `(&<account filter><given filter>)`: a filter a caller supplied can only narrow the
/// accounts the provider is configured to read.
pub fn all_of(account_filter: &str, given: &str) -> String {
    format!("(&{account_filter}{given})")
}

/// Whether `filter` is one balanced parenthesised group: `(...)` whose first parenthesis
/// closes at the end. Escaped values hold no parenthesis, so this is the shape of a filter
/// the matcher wrote.
pub fn one_group(filter: &str) -> bool {
    let mut depth = 0_i32;
    for (at, character) in filter.char_indices() {
        if at == 0 && character != '(' {
            return false;
        }
        match character {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth < 0 || (depth == 0 && at + character.len_utf8() != filter.len()) {
            return false;
        }
    }
    depth == 0 && !filter.is_empty()
}

/// Substitutes an escaped value for each `{field}` in an LDAP filter.
pub fn filter(template: &str, field: &str, value: &str) -> String {
    template.replace(&format!("{{{field}}}"), &escaped(value))
}

/// Reads the first RDN value of a group DN, unescaping comma and backslash.
pub fn group_name(dn: &str) -> String {
    let mut first = String::new();
    let mut chars = dn.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => first.extend(chars.next()),
            ',' => break,
            character => first.push(character),
        }
    }
    match first.split_once('=') {
        Some((_, name)) if !name.trim().is_empty() => name.trim().to_owned(),
        _ => dn.to_owned(),
    }
}

/// Whether Active Directory account flags include `ACCOUNTDISABLE`.
pub fn disabled(flags: Option<&str>) -> bool {
    flags
        .and_then(|flags| flags.trim().parse::<u64>().ok())
        .is_some_and(|flags| flags & 0x2 != 0)
}

/// A distinguished name as its components, each lower-cased with the blanks around `=` and
/// the component dropped; a `,` after a backslash belongs to its component.
fn components(dn: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in dn.chars() {
        match character {
            _ if escaped => {
                current.push(character);
                escaped = false;
            }
            '\\' => {
                current.push(character);
                escaped = true;
            }
            ',' | ';' => parts.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|part| {
            let part = part.trim();
            match part.split_once('=') {
                Some((kind, value)) => format!("{}={}", kind.trim(), value.trim()).to_lowercase(),
                None => part.to_lowercase(),
            }
        })
        .collect()
}

/// Whether `dn` names an entry strictly below `base` (the base itself is not below it).
/// Components compare case-insensitively with blanks around `=` and `,` ignored.
pub fn below(dn: &str, base: &str) -> bool {
    let (dn, base) = (components(dn), components(base));
    !base.iter().any(String::is_empty)
        && !dn.iter().any(String::is_empty)
        && dn.len() > base.len()
        && dn[dn.len() - base.len()..] == base[..]
}

/// A person's name in natural order: `display` when it says anything, else the given name,
/// initials and surname that are present, joined by single spaces; `None` when nothing is.
pub fn display_name(
    display: Option<&str>,
    given: Option<&str>,
    initials: Option<&str>,
    surname: Option<&str>,
) -> Option<String> {
    fn present(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| !value.is_empty())
    }
    if let Some(display) = present(display) {
        return Some(display.to_owned());
    }
    let parts: Vec<&str> = [given, initials, surname]
        .into_iter()
        .filter_map(present)
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Whether connecting to `url` without StartTLS would expose a password. An `ldapi://` socket
/// never leaves the machine.
pub fn in_the_clear(url: &str, start_tls: bool) -> bool {
    if url.starts_with("ldapi://") {
        return false;
    }
    let Some(rest) = url.strip_prefix("ldap://") else {
        return !url.starts_with("ldaps://");
    };
    if start_tls {
        return false;
    }
    let host = rest.split(['/', '?']).next().unwrap_or_default();
    let host = match host.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').map_or(host, |(host, _)| host),
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    !loopback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_in_filters_are_escaped() {
        assert_eq!(
            filter("(uid={name})", "name", "*)(uid=*"),
            "(uid=\\2a\\29\\28uid=\\2a)"
        );
        assert_eq!(escaped("a\\b\0"), "a\\5cb\\00");
        assert_eq!(
            filter(IN_CHAIN, "dn", "CN=Ana (first),OU=People"),
            "(member:1.2.840.113556.1.4.1941:=CN=Ana \\28first\\29,OU=People)"
        );
    }

    #[test]
    fn a_given_filter_is_anded_with_the_account_filter_and_must_be_one_group() {
        assert_eq!(
            all_of("(objectClass=user)", "(altSecurityIdentities=X509:x)"),
            "(&(objectClass=user)(altSecurityIdentities=X509:x))"
        );
        assert!(one_group("(a=1)"));
        assert!(one_group("(&(a=1)(|(b=2)(c=3)))"));
        for bad in ["", "a=1", "(a=1", "(a=1))", "(a=1)(b=2)", ")(", "(a=1)x"] {
            assert!(!one_group(bad), "{bad}");
        }
        // A value escaped by the matcher holds no parenthesis, so it cannot unbalance this.
        assert!(one_group("(a=\\29\\28)"));
    }

    #[test]
    fn escaping_is_the_matchers_and_covers_controls_and_non_ascii() {
        assert_eq!(escaped("caf\u{e9}\t"), "caf\\c3\\a9\\09");
    }

    #[test]
    fn groups_and_disabled_flags_are_read_from_directory_values() {
        assert_eq!(group_name("CN=Smith\\, Ana,OU=Groups"), "Smith, Ana");
        assert_eq!(group_name("household"), "household");
        assert!(disabled(Some("514")));
        assert!(!disabled(Some("512")));
        assert!(!disabled(None));
    }

    #[test]
    fn cleartext_is_refused_outside_loopback() {
        assert!(in_the_clear("ldap://directory.example.org", false));
        assert!(!in_the_clear("ldap://directory.example.org", true));
        assert!(!in_the_clear("ldaps://directory.example.org", false));
        assert!(!in_the_clear("ldap://[::1]:389", false));
        assert!(!in_the_clear("ldapi://%2Frun%2Fldapi", false));
        assert!(in_the_clear("http://directory.example.org", false));
    }

    #[test]
    fn a_name_is_below_a_base_by_its_components() {
        let base = "DC=example,DC=org";
        assert!(below("CN=Ana,OU=People,DC=example,DC=org", base));
        assert!(below("cn=Ana , ou = People,dc=EXAMPLE,dc=Org", base));
        assert!(below("CN=Smith\\, Ana,DC=example,DC=org", base));
        assert!(!below(base, base));
        assert!(!below("DC=org", base));
        assert!(!below("CN=Ana,DC=example,DC=org,DC=evil", base));
        assert!(!below("CN=Ana,DC=notexample,DC=org", base));
        assert!(!below("CN=Ana,,DC=example,DC=org", base));
        assert!(!below("", base));
        assert!(!below("CN=Ana,DC=example,DC=org", ""));
    }

    #[test]
    fn a_display_name_is_the_directorys_or_the_parts_in_natural_order() {
        assert_eq!(
            display_name(Some("Ana Rivera"), Some("A"), None, Some("R")).as_deref(),
            Some("Ana Rivera")
        );
        assert_eq!(
            display_name(Some("  "), Some("Ana"), Some("Q"), Some("Rivera")).as_deref(),
            Some("Ana Q Rivera")
        );
        assert_eq!(
            display_name(None, Some("Ana"), None, Some("Rivera")).as_deref(),
            Some("Ana Rivera")
        );
        assert_eq!(
            display_name(None, None, None, Some("Rivera")).as_deref(),
            Some("Rivera")
        );
        assert_eq!(display_name(None, Some(" "), None, None), None);
    }
}
