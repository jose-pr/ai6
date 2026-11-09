use alloc::sync::Arc;
use core::fmt;
use core::net::IpAddr;

use globset::{Glob, GlobSet, GlobSetBuilder};
use rustls::DistinguishedName;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::ClientCertVerifier;

use crate::error::PeerError;
use crate::fields::{CertificateFields, San};
use crate::trust::CaPolicy;

/// A loaded CA policy.
pub(crate) struct Ca {
    verifier: Arc<dyn ClientCertVerifier>,
    subjects: Option<GlobSet>,
    sans: Option<Sans>,
    ip_in_dns: bool,
}

/// The SAN globs, by the kind of name each applies to: an unprefixed glob to any kind.
#[derive(Default)]
struct Sans {
    any: Option<GlobSet>,
    dns: Option<GlobSet>,
    ip: Option<GlobSet>,
    email: Option<GlobSet>,
    uri: Option<GlobSet>,
    upn: Option<GlobSet>,
}

impl fmt::Debug for Ca {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Ca").finish_non_exhaustive()
    }
}

impl Ca {
    pub(crate) fn load(
        policy: &CaPolicy,
        provider: &Arc<CryptoProvider>,
    ) -> Result<Self, PeerError> {
        let text = crate::roots::read_anchors(&policy.ca_file)?;
        let roots = crate::roots::anchors(&text, Some(&policy.ca_file))?;
        let verifier =
            WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                .build()
                .map_err(|_| PeerError::Certificate)?;
        Ok(Self {
            verifier,
            subjects: compile(policy.subject.iter().map(String::as_str))?,
            sans: Sans::compile(&policy.san)?,
            ip_in_dns: policy.ip_in_dns,
        })
    }

    pub(crate) fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.verifier.root_hint_subjects()
    }

    /// Whether the chain verifies against the CA roots and the names match the globs.
    pub(crate) fn trusts(
        &self,
        certificate: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<bool, rustls::Error> {
        Ok(self
            .verifier
            .verify_client_cert(certificate, intermediates, now)
            .is_ok()
            && self.names_match(certificate)?)
    }

    fn names_match(&self, certificate: &CertificateDer<'_>) -> Result<bool, rustls::Error> {
        if self.subjects.is_none() && self.sans.is_none() {
            return Ok(true);
        }
        let fields = CertificateFields::parse(certificate.as_ref()).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;
        if let Some(subjects) = &self.subjects
            && !subjects.is_match(&fields.subject.text)
        {
            return Ok(false);
        }
        if let Some(sans) = &self.sans
            && !fields
                .sans
                .iter()
                .any(|san| sans.matches(san, self.ip_in_dns))
        {
            return Ok(false);
        }
        Ok(true)
    }
}

impl Sans {
    fn compile(patterns: &[String]) -> Result<Option<Self>, PeerError> {
        if patterns.is_empty() {
            return Ok(None);
        }
        let mut kinds: [Vec<String>; 6] = Default::default();
        for pattern in patterns {
            let (kind, glob) = match pattern.split_once(':') {
                Some(("dns", glob)) => (1, glob),
                Some(("ip", glob)) => (2, glob),
                Some(("email", glob)) => (3, glob),
                Some(("uri", glob)) => (4, glob),
                Some(("upn", glob)) => (5, glob),
                _ => (0, pattern.as_str()),
            };
            kinds[kind].push(canonical_ip(glob));
        }
        let [any, dns, ip, email, uri, upn] = kinds;
        let set = |globs: Vec<String>| compile(globs.iter().map(String::as_str));
        Ok(Some(Self {
            any: set(any)?,
            dns: set(dns)?,
            ip: set(ip)?,
            email: set(email)?,
            uri: set(uri)?,
            upn: set(upn)?,
        }))
    }

    /// Whether `san` matches: an unprefixed glob against any DNS, email, URI or IP name, a
    /// prefixed one against its kind. With `ip_in_dns`, a `dNSName` holding an IP literal
    /// also matches as that address.
    fn matches(&self, san: &San, ip_in_dns: bool) -> bool {
        let hit =
            |set: &Option<GlobSet>, text: &str| set.as_ref().is_some_and(|set| set.is_match(text));
        let address = match san {
            San::Ip(_) => san.ip(),
            San::Dns(_) if ip_in_dns => san.ip(),
            _ => None,
        };
        if let Some(address) = address {
            let text = address.to_string();
            if hit(&self.any, &text) || hit(&self.ip, &text) {
                return true;
            }
        }
        match san {
            San::Dns(name) => hit(&self.any, name) || hit(&self.dns, name),
            San::Email(name) => hit(&self.any, name) || hit(&self.email, name),
            San::Uri(name) => hit(&self.any, name) || hit(&self.uri, name),
            San::Upn(name) => hit(&self.upn, name),
            _ => false,
        }
    }
}

/// `glob` in canonical form when it is an IP literal (IPv6 compressed, IPv4-mapped as
/// IPv4), so it matches however the certificate or the pattern spells the address.
fn canonical_ip(glob: &str) -> String {
    glob.parse::<IpAddr>().map_or_else(
        |_| glob.to_owned(),
        |address| address.to_canonical().to_string(),
    )
}

fn compile<'a>(patterns: impl IntoIterator<Item = &'a str>) -> Result<Option<GlobSet>, PeerError> {
    let mut builder = GlobSetBuilder::new();
    let mut any = false;
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|_| PeerError::TrustPattern)?);
        any = true;
    }
    if !any {
        return Ok(None);
    }
    builder
        .build()
        .map(Some)
        .map_err(|_| PeerError::TrustPattern)
}
