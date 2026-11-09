//! The identifying fields of a certificate, parsed once for every consumer: names, subject
//! alternative names, serial, fingerprint and validity.

use alloc::collections::BTreeMap;
use core::fmt;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use x509_parser::asn1_rs::{Any, FromDer};
use x509_parser::extensions::GeneralName;
use x509_parser::x509::X509Name;

use crate::error::PeerError;
use coqui_peer::PeerId;

/// The Microsoft user principal name, an `otherName` holding a UTF8String.
const UPN: &str = "1.3.6.1.4.1.311.20.2.3";

/// A distinguished name: its text as x509-parser displays it (`CN=a, O=b`) and its
/// attributes in certificate order, each `(short name, value)`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Name {
    /// The whole name as text.
    pub text: String,
    /// The attributes in order: `cn`, `o`, `ou`, `c`, `st`, `l`, `dc`, `uid`, `email`,
    /// `serialnumber`, or the dotted OID for any other. A value that is not a string type is
    /// `#` and its hex.
    pub attributes: Vec<(String, String)>,
}

/// A subject alternative name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum San {
    /// A `dNSName`. Some issuers put an IP address here, for clients that do not read IP
    /// SANs; [`San::ip`] gives it as an address.
    Dns(String),
    /// An `rfc822Name` (email).
    Email(String),
    /// A `uniformResourceIdentifier`.
    Uri(String),
    /// An `iPAddress`.
    Ip(IpAddr),
    /// A Microsoft user principal name (`otherName` 1.3.6.1.4.1.311.20.2.3).
    Upn(String),
    /// Any other `otherName`: its OID and the DER of its value.
    Other {
        /// The dotted OID.
        oid: String,
        /// The DER encoding of the value, its `[0]` wrapper included.
        der: Vec<u8>,
    },
}

impl San {
    /// The address this name holds: an `iPAddress`, or a `dNSName` whose text is an IP
    /// literal. Canonical: an IPv4-mapped IPv6 address is its IPv4 address.
    pub fn ip(&self) -> Option<IpAddr> {
        match self {
            Self::Ip(address) => Some(address.to_canonical()),
            Self::Dns(name) => name
                .parse::<IpAddr>()
                .ok()
                .map(|address| address.to_canonical()),
            _ => None,
        }
    }
}

/// The identifying fields of a certificate (feature `fields`).
///
/// `Debug` shows only the fingerprint.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CertificateFields {
    /// The subject.
    pub subject: Name,
    /// The issuer.
    pub issuer: Name,
    /// The subject alternative names, in certificate order.
    pub sans: Vec<San>,
    /// The serial number's bytes as encoded.
    pub serial: Vec<u8>,
    /// The certificate's SHA-256 id.
    pub fingerprint: PeerId,
    /// The start of the validity period, Unix seconds.
    pub not_before: i64,
    /// The end of the validity period, Unix seconds.
    pub not_after: i64,
}

impl fmt::Debug for CertificateFields {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CertificateFields")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl CertificateFields {
    /// Parses the DER encoding of one certificate. Nothing is verified: the fields are what
    /// the certificate says.
    ///
    /// # Errors
    ///
    /// [`PeerError::Certificate`] when `der` is not exactly one well-formed certificate, or
    /// its subject alternative names cannot be read.
    pub fn parse(der: &[u8]) -> Result<Self, PeerError> {
        let (rest, certificate) =
            x509_parser::parse_x509_certificate(der).map_err(|_| PeerError::Certificate)?;
        if !rest.is_empty() {
            return Err(PeerError::Certificate);
        }
        let extension = certificate
            .subject_alternative_name()
            .map_err(|_| PeerError::Certificate)?;
        let sans = extension
            .into_iter()
            .flat_map(|extension| extension.value.general_names.iter())
            .filter_map(san)
            .collect();
        let validity = certificate.validity();
        Ok(Self {
            subject: name(certificate.subject()),
            issuer: name(certificate.issuer()),
            sans,
            serial: certificate.raw_serial().to_vec(),
            fingerprint: PeerId::of(der),
            not_before: validity.not_before.timestamp(),
            not_after: validity.not_after.timestamp(),
        })
    }

    /// The fields as a map a consumer picks identities from, every value in certificate order:
    ///
    /// - `subject`, `issuer`: the names as text;
    /// - `subject.<attr>`, `issuer.<attr>`: each attribute, by the short names of
    ///   [`Name::attributes`];
    /// - `san.dns`, `san.email`, `san.uri`, `san.upn`: the names as written;
    /// - `san.ip`: each `iPAddress`, canonical (IPv6 compressed, IPv4-mapped as IPv4);
    /// - `san.dns.ip`: each `dNSName` that is an IP literal, canonical, for a consumer that
    ///   treats such a name as an address;
    /// - `san.other.<oid>`: the hex DER of each other `otherName`;
    /// - `serial`: lowercase hex, leading zero bytes dropped (`0` for zero);
    /// - `fingerprint`: `sha256:<hex>`;
    /// - `not_before`, `not_after`: Unix seconds.
    pub fn to_map(&self) -> BTreeMap<String, Vec<String>> {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut put = |key: String, value: String| map.entry(key).or_default().push(value);
        for (prefix, name) in [("subject", &self.subject), ("issuer", &self.issuer)] {
            put(prefix.to_owned(), name.text.clone());
            for (attribute, value) in &name.attributes {
                put(format!("{prefix}.{attribute}"), value.clone());
            }
        }
        for san in &self.sans {
            match san {
                San::Dns(name) => {
                    put("san.dns".to_owned(), name.clone());
                    if let Some(address) = san.ip() {
                        put("san.dns.ip".to_owned(), address.to_string());
                    }
                }
                San::Email(name) => put("san.email".to_owned(), name.clone()),
                San::Uri(name) => put("san.uri".to_owned(), name.clone()),
                San::Ip(_) => {
                    if let Some(address) = san.ip() {
                        put("san.ip".to_owned(), address.to_string());
                    }
                }
                San::Upn(name) => put("san.upn".to_owned(), name.clone()),
                San::Other { oid, der } => put(format!("san.other.{oid}"), hex(der)),
            }
        }
        let serial = self
            .serial
            .iter()
            .position(|byte| *byte != 0)
            .map_or_else(|| "0".to_owned(), |first| hex(&self.serial[first..]));
        put("serial".to_owned(), serial);
        put("fingerprint".to_owned(), self.fingerprint.canonical());
        put("not_before".to_owned(), self.not_before.to_string());
        put("not_after".to_owned(), self.not_after.to_string());
        map
    }
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn name(name: &X509Name<'_>) -> Name {
    let attributes = name
        .iter_attributes()
        .map(|attribute| {
            let oid = attribute.attr_type().to_id_string();
            let short = match oid.as_str() {
                "2.5.4.3" => "cn",
                "2.5.4.10" => "o",
                "2.5.4.11" => "ou",
                "2.5.4.6" => "c",
                "2.5.4.8" => "st",
                "2.5.4.7" => "l",
                "0.9.2342.19200300.100.1.25" => "dc",
                "0.9.2342.19200300.100.1.1" => "uid",
                "1.2.840.113549.1.9.1" => "email",
                "2.5.4.5" => "serialnumber",
                _ => oid.as_str(),
            }
            .to_owned();
            let value = attribute.as_str().map_or_else(
                |_| format!("#{}", hex(attribute.attr_value().data)),
                str::to_owned,
            );
            (short, value)
        })
        .collect();
    Name {
        text: name.to_string(),
        attributes,
    }
}

fn san(name: &GeneralName<'_>) -> Option<San> {
    Some(match name {
        GeneralName::DNSName(text) => San::Dns((*text).to_owned()),
        GeneralName::RFC822Name(text) => San::Email((*text).to_owned()),
        GeneralName::URI(text) => San::Uri((*text).to_owned()),
        GeneralName::IPAddress(bytes) => San::Ip(match bytes.len() {
            4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(*bytes).ok()?)),
            16 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(*bytes).ok()?)),
            _ => return None,
        }),
        GeneralName::OtherName(oid, value) => {
            let oid = oid.to_id_string();
            match (oid == UPN).then(|| upn(value)).flatten() {
                Some(upn) => San::Upn(upn),
                None => San::Other {
                    oid,
                    der: value.to_vec(),
                },
            }
        }
        _ => return None,
    })
}

/// The UTF8String inside an `otherName` value's `[0] EXPLICIT` wrapper.
fn upn(value: &[u8]) -> Option<String> {
    let (_, wrapper) = Any::from_der(value).ok()?;
    let (_, inner) = Any::from_der(wrapper.data).ok()?;
    (inner.tag() == x509_parser::asn1_rs::Tag::Utf8String)
        .then(|| core::str::from_utf8(inner.data).ok().map(str::to_owned))
        .flatten()
}
