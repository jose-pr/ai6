//! How the provider binds: the methods, what each needs of the build, and what the settings
//! that weaken a bind cost.
use std::fmt;
use std::net::IpAddr;

/// How the search account binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BindMethod {
    /// A distinguished name and a password (`bind`, `bind_password`); none binds anonymously.
    Simple,
    /// Kerberos through SASL GSSAPI, as the process's own credentials. Needs the `gssapi`
    /// feature. Over plain `ldap://` the connection is sealed; over TLS the certificate's hash
    /// is sent as the channel binding.
    Gssapi,
    /// NTLM through SASL GSS-SPNEGO with `bind` as `DOMAIN\user` or `user@domain` and
    /// `bind_password`. Needs the `ntlm` feature. It cannot seal: without TLS the session
    /// that follows is unprotected.
    Ntlm,
    /// SASL EXTERNAL: the identity the TLS client certificate (`client_cert`) or an
    /// `ldapi://` socket's peer credentials already proved.
    External,
}

impl BindMethod {
    /// The method a configuration value names: `simple`, `gssapi`, `ntlm` or `external`.
    ///
    /// # Errors
    ///
    /// Any other text; the error names the choices.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "simple" => Ok(Self::Simple),
            "gssapi" => Ok(Self::Gssapi),
            "ntlm" => Ok(Self::Ntlm),
            "external" => Ok(Self::External),
            other => Err(format!(
                "bind_method {other:?} is not one of simple, gssapi, ntlm, external"
            )),
        }
    }

    /// The configuration value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::Gssapi => "gssapi",
            Self::Ntlm => "ntlm",
            Self::External => "external",
        }
    }

    /// The Cargo feature this build needs for the method, if it needs one.
    pub fn feature(self) -> Option<&'static str> {
        match self {
            Self::Gssapi => Some("gssapi"),
            Self::Ntlm => Some("ntlm"),
            Self::Simple | Self::External => None,
        }
    }

    /// Whether this build carries the method.
    pub fn built(self) -> bool {
        match self {
            Self::Gssapi => cfg!(feature = "gssapi"),
            Self::Ntlm => cfg!(feature = "ntlm"),
            Self::Simple | Self::External => true,
        }
    }
}

impl fmt::Display for BindMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a person's password is checked at the directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UserBind {
    /// A simple bind as the person's distinguished name, on a connection of its own when the
    /// search account bound with a SASL method.
    Simple,
    /// NTLM (SASL GSS-SPNEGO) with the name the person typed, which must be `user`,
    /// `DOMAIN\user` or `user@domain`. Needs the `ntlm` feature.
    Ntlm,
}

impl UserBind {
    /// The method a configuration value names: `simple` or `ntlm`.
    ///
    /// # Errors
    ///
    /// Any other text; the error names the choices.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "simple" => Ok(Self::Simple),
            "ntlm" => Ok(Self::Ntlm),
            other => Err(format!("user_bind {other:?} is not one of simple, ntlm")),
        }
    }

    /// The configuration value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::Ntlm => "ntlm",
        }
    }

    /// Whether this build carries the method.
    pub fn built(self) -> bool {
        match self {
            Self::Simple => true,
            Self::Ntlm => cfg!(feature = "ntlm"),
        }
    }
}

impl fmt::Display for UserBind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Checks that `name` is one `sasl_ntlm_bind` accepts: `user`, `DOMAIN\user` or `user@domain`,
/// each part present and free of control characters. ldap3 parses the name with `unwrap`, and a
/// name with a second separator or a mixed form panics there, so it is refused here first.
///
/// # Errors
///
/// What is wrong with the name; never the name itself.
pub fn ntlm_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("an NTLM name must be present and hold no control characters".to_owned());
    }
    let mixed = "an NTLM name is user, DOMAIN\\user or user@domain";
    if let Some((domain, account)) = name.split_once('\\') {
        if domain.is_empty() || account.is_empty() {
            return Err(mixed.to_owned());
        }
        if domain.contains(['\\', '@']) || account.contains(['\\', '@']) {
            return Err(mixed.to_owned());
        }
    } else if let Some((account, suffix)) = name.rsplit_once('@')
        && (account.is_empty() || suffix.is_empty() || suffix.contains(['\\', '@']))
    {
        return Err(mixed.to_owned());
    }
    Ok(())
}

/// The host of an `ldap://` or `ldaps://` URL, without port or brackets.
pub fn url_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("ldap://")
        .or_else(|| url.strip_prefix("ldaps://"))?;
    let authority = rest.split(['/', '?']).next()?;
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next()?,
        None => authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host),
    };
    (!host.is_empty()).then_some(host)
}

/// Whether `host` is an address, which names no Kerberos service.
pub fn is_address(host: &str) -> bool {
    host.parse::<IpAddr>().is_ok()
}

/// Whether `url` is an `ldapi://` socket, which no network can read.
pub fn is_socket(url: &str) -> bool {
    url.starts_with("ldapi://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_are_read_by_name_and_the_error_names_the_choices() {
        for method in [
            BindMethod::Simple,
            BindMethod::Gssapi,
            BindMethod::Ntlm,
            BindMethod::External,
        ] {
            assert_eq!(BindMethod::parse(method.as_str()), Ok(method));
        }
        assert!(
            BindMethod::parse("kerberos")
                .unwrap_err()
                .contains("simple, gssapi, ntlm, external")
        );
        assert_eq!(UserBind::parse("ntlm"), Ok(UserBind::Ntlm));
        assert!(
            UserBind::parse("gssapi")
                .unwrap_err()
                .contains("simple, ntlm")
        );
    }

    #[test]
    fn an_ntlm_name_ldap3_would_panic_on_is_an_error() {
        for good in ["ana", "EXAMPLE\\ana", "ana@example.org", "a@b@example.org"] {
            assert!(ntlm_name(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "\\ana",
            "EXAMPLE\\",
            "EXAMPLE\\ana\\b",
            "EXAMPLE\\ana@x",
            "a@b\\c@d",
            "ana@",
            "@example.org",
            "ana@exa\\mple",
            "ana\n",
            "EXAMPLE\\ana\0",
        ] {
            assert!(ntlm_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_host_of_a_url_is_read_without_port_or_brackets() {
        assert_eq!(
            url_host("ldap://dc.example.org:389/x"),
            Some("dc.example.org")
        );
        assert_eq!(url_host("ldaps://dc.example.org"), Some("dc.example.org"));
        assert_eq!(url_host("ldap://[::1]:389"), Some("::1"));
        assert_eq!(url_host("ldapi://%2Frun%2Fldapi"), None);
        assert_eq!(url_host("ldap://"), None);
        assert!(is_address("::1") && is_address("10.0.0.1") && !is_address("dc.example.org"));
    }
}
