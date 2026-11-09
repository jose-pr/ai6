//! LDAP filter and result helpers, independent of a live directory.
use std::net::IpAddr;

/// Active Directory's matching rule for nested group membership.
pub const IN_CHAIN: &str = "(member:1.2.840.113556.1.4.1941:={dn})";

/// Escapes the RFC 4515 filter metacharacters in a user value.
pub fn escaped(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\5c"),
            '*' => output.push_str("\\2a"),
            '(' => output.push_str("\\28"),
            ')' => output.push_str("\\29"),
            '\0' => output.push_str("\\00"),
            character => output.push(character),
        }
    }
    output
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

/// Whether connecting to `url` without StartTLS would expose a password.
pub fn in_the_clear(url: &str, start_tls: bool) -> bool {
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
    }
}
