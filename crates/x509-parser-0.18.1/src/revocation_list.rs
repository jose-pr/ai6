use crate::error::{X509Error, X509Result};
use crate::extensions::*;
use crate::time::ASN1Time;
use crate::utils::format_serial;
use crate::x509::{
    parse_serial, parse_signature_value, AlgorithmIdentifier, ReasonCode, X509Name, X509Version,
};

#[cfg(any(feature = "verify", feature = "verify-aws"))]
use crate::verify::verify_signature;
#[cfg(any(feature = "verify", feature = "verify-aws"))]
use crate::x509::SubjectPublicKeyInfo;
use asn1_rs::{BitString, FromDer};
use der_parser::der::*;
use der_parser::num_bigint::BigUint;
use nom::combinator::{all_consuming, complete, map, opt};
use nom::multi::many0;
use nom::Offset;
use oid_registry::*;
use std::collections::HashMap;

/// An X.509 v2 Certificate Revocation List (CRL).
///
/// X.509 v2 CRLs are defined in [RFC5280](https://tools.ietf.org/html/rfc5280).
///
/// # Example
///
/// To parse a CRL and print information about revoked certificates:
///
/// ```rust
/// use x509_parser::prelude::FromDer;
/// use x509_parser::revocation_list::CertificateRevocationList;
///
/// # static DER: &'static [u8] = &[0x30u8,0x82,0x03,0x14,0x30,0x82,0x01,0xfc,0x02,0x01,0x01,0x30,0x0d,0x06,0x09,0x2a,0x86,0x48,0x86,0xf7,0x0d,0x01,0x01,0x05,0x05,0x00,0x30,0x5f,0x31,0x23,0x30,0x21,0x06,0x03,0x55,0x04,0x0a,0x13,0x1a,0x53,0x61,0x6d,0x70,0x6c,0x65,0x20,0x53,0x69,0x67,0x6e,0x65,0x72,0x20,0x4f,0x72,0x67,0x61,0x6e,0x69,0x7a,0x61,0x74,0x69,0x6f,0x6e,0x31,0x1b,0x30,0x19,0x06,0x03,0x55,0x04,0x0b,0x13,0x12,0x53,0x61,0x6d,0x70,0x6c,0x65,0x20,0x53,0x69,0x67,0x6e,0x65,0x72,0x20,0x55,0x6e,0x69,0x74,0x31,0x1b,0x30,0x19,0x06,0x03,0x55,0x04,0x03,0x13,0x12,0x53,0x61,0x6d,0x70,0x6c,0x65,0x20,0x53,0x69,0x67,0x6e,0x65,0x72,0x20,0x43,0x65,0x72,0x74,0x17,0x0d,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x33,0x32,0x30,0x30,0x5a,0x17,0x0d,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x34,0x32,0x30,0x30,0x5a,0x30,0x82,0x01,0x36,0x30,0x3c,0x02,0x03,0x14,0x79,0x47,0x17,0x0d,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x31,0x32,0x5a,0x30,0x26,0x30,0x0a,0x06,0x03,0x55,0x1d,0x15,0x04,0x03,0x0a,0x01,0x03,0x30,0x18,0x06,0x03,0x55,0x1d,0x18,0x04,0x11,0x18,0x0f,0x32,0x30,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x30,0x30,0x5a,0x30,0x3c,0x02,0x03,0x14,0x79,0x48,0x17,0x0d,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x32,0x32,0x5a,0x30,0x26,0x30,0x0a,0x06,0x03,0x55,0x1d,0x15,0x04,0x03,0x0a,0x01,0x06,0x30,0x18,0x06,0x03,0x55,0x1d,0x18,0x04,0x11,0x18,0x0f,0x32,0x30,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x30,0x30,0x5a,0x30,0x3c,0x02,0x03,0x14,0x79,0x49,0x17,0x0d,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x33,0x32,0x5a,0x30,0x26,0x30,0x0a,0x06,0x03,0x55,0x1d,0x15,0x04,0x03,0x0a,0x01,0x04,0x30,0x18,0x06,0x03,0x55,0x1d,0x18,0x04,0x11,0x18,0x0f,0x32,0x30,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x30,0x30,0x5a,0x30,0x3c,0x02,0x03,0x14,0x79,0x4a,0x17,0x0d,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x34,0x32,0x5a,0x30,0x26,0x30,0x0a,0x06,0x03,0x55,0x1d,0x15,0x04,0x03,0x0a,0x01,0x01,0x30,0x18,0x06,0x03,0x55,0x1d,0x18,0x04,0x11,0x18,0x0f,0x32,0x30,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x30,0x30,0x5a,0x30,0x3c,0x02,0x03,0x14,0x79,0x4b,0x17,0x0d,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x35,0x31,0x5a,0x30,0x26,0x30,0x0a,0x06,0x03,0x55,0x1d,0x15,0x04,0x03,0x0a,0x01,0x05,0x30,0x18,0x06,0x03,0x55,0x1d,0x18,0x04,0x11,0x18,0x0f,0x32,0x30,0x31,0x33,0x30,0x32,0x31,0x38,0x31,0x30,0x32,0x32,0x30,0x30,0x5a,0xa0,0x2f,0x30,0x2d,0x30,0x1f,0x06,0x03,0x55,0x1d,0x23,0x04,0x18,0x30,0x16,0x80,0x14,0xbe,0x12,0x01,0xcc,0xaa,0xea,0x11,0x80,0xda,0x2e,0xad,0xb2,0xea,0xc7,0xb5,0xfb,0x9f,0xf9,0xad,0x34,0x30,0x0a,0x06,0x03,0x55,0x1d,0x14,0x04,0x03,0x02,0x01,0x03,0x30,0x0d,0x06,0x09,0x2a,0x86,0x48,0x86,0xf7,0x0d,0x01,0x01,0x05,0x05,0x00,0x03,0x82,0x01,0x01,0x00,0x42,0x21,0xbe,0x81,0xf1,0xc3,0x79,0x76,0x66,0x5b,0xce,0x21,0x13,0x8a,0x68,0xa8,0xb4,0x3c,0xbe,0x16,0xc3,0xaf,0x4b,0xdd,0xcb,0x78,0x35,0x92,0x90,0xd8,0xd7,0x4c,0x6f,0xfe,0x6c,0x68,0x27,0xae,0x6d,0xda,0x42,0x98,0x01,0xee,0x17,0x93,0xf0,0xbd,0xa8,0xee,0xcd,0x90,0xb6,0x35,0xf6,0x0d,0xa4,0xce,0x49,0x82,0xf7,0x9d,0x9f,0xc8,0x6e,0x7f,0xd1,0xf1,0x2d,0x20,0xf8,0x46,0xcd,0x43,0x17,0x64,0xe7,0xf9,0x5a,0xe8,0x21,0x11,0xc6,0x24,0x69,0xf8,0x4d,0x93,0x50,0x6f,0x0b,0x0d,0xbd,0x78,0x61,0x53,0x21,0x44,0x62,0xaf,0x0a,0x0b,0x92,0x23,0x25,0x06,0xd0,0xcc,0x06,0x5b,0xac,0x1a,0xa9,0x5b,0x5d,0xe8,0xae,0xf5,0xbb,0xbb,0xe1,0x21,0x4f,0xd3,0x89,0xd7,0xfa,0x65,0x27,0x6c,0x4c,0xc8,0x69,0x3c,0xf1,0x6e,0x3d,0x48,0x9d,0xe2,0x3d,0xbd,0x53,0x7a,0xb5,0xd1,0x21,0x85,0x17,0xa7,0x02,0xb7,0x50,0xf3,0x8e,0xf5,0x1c,0x0b,0x01,0xc6,0x84,0x70,0x34,0xd8,0xc7,0xa7,0xef,0x41,0x20,0x64,0x50,0x03,0x3c,0xb5,0xa6,0x2e,0x0d,0x07,0x82,0x52,0x94,0x87,0x58,0x99,0x59,0xc0,0x46,0xb5,0xeb,0xff,0xf1,0x5b,0x14,0x8a,0x3c,0xa3,0xb0,0xcd,0x3b,0xd8,0x2e,0x94,0xb7,0x94,0xf0,0x37,0x2a,0xeb,0xb6,0x16,0xfd,0xe7,0x6f,0x9e,0x2a,0x59,0xb1,0x2c,0xd8,0x13,0xd2,0x8e,0x61,0x55,0x8c,0x63,0x5e,0x1b,0x70,0x2d,0x0b,0x0b,0xed,0x06,0x61,0xaf,0x2a,0x40,0x33,0x50,0xcb,0x62,0xa4,0x23,0x92,0x20,0xc8,0xee,0x19,0x6f,0xb7,0xb4,0x2e,0x0c,0x64,0xc9];
/// #
/// # fn main() {
/// let res = CertificateRevocationList::from_der(DER);
/// match res {
///     Ok((_rem, crl)) => {
///         for revoked in crl.iter_revoked_certificates() {
///             println!("Revoked certificate serial: {}", revoked.raw_serial_as_string());
///             println!("  Reason: {}", revoked.reason_code().unwrap_or_default().1);
///         }
///     },
///     _ => panic!("CRL parsing failed: {:?}", res),
/// }
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct CertificateRevocationList<'a> {
    pub tbs_cert_list: TbsCertList<'a>,
    pub signature_algorithm: AlgorithmIdentifier<'a>,
    pub signature_value: BitString<'a>,

    /// Complete raw ASN.1 DER content (TBS certificate list, signature algorithm and signature).
    pub(crate) raw: &'a [u8],
}

impl<'a> CertificateRevocationList<'a> {
    /// Get the version of the encoded certificate
    pub fn version(&self) -> Option<X509Version> {
        self.tbs_cert_list.version
    }

    /// Get the certificate issuer.
    #[inline]
    pub fn issuer(&self) -> &X509Name<'_> {
        &self.tbs_cert_list.issuer
    }

    /// Get the date and time of the last (this) update.
    #[inline]
    pub fn last_update(&self) -> ASN1Time {
        self.tbs_cert_list.this_update
    }

    /// Get the date and time of the next update, if present.
    #[inline]
    pub fn next_update(&self) -> Option<ASN1Time> {
        self.tbs_cert_list.next_update
    }

    /// Return an iterator over the `RevokedCertificate` objects
    pub fn iter_revoked_certificates(&self) -> impl Iterator<Item = &RevokedCertificate<'a>> {
        self.tbs_cert_list.revoked_certificates.iter()
    }

    /// Get the CRL extensions.
    #[inline]
    pub fn extensions(&self) -> &[X509Extension<'_>] {
        &self.tbs_cert_list.extensions
    }

    /// Get the CRL number, if present
    ///
    /// Note that the returned value is a `BigUint`, because of the following RFC specification:
    /// <pre>
    /// Given the requirements above, CRL numbers can be expected to contain long integers.  CRL
    /// verifiers MUST be able to handle CRLNumber values up to 20 octets.  Conformant CRL issuers
    /// MUST NOT use CRLNumber values longer than 20 octets.
    /// </pre>
    pub fn crl_number(&self) -> Option<&BigUint> {
        self.extensions()
            .iter()
            .find(|&ext| ext.oid == OID_X509_EXT_CRL_NUMBER)
            .and_then(|ext| match ext.parsed_extension {
                ParsedExtension::CRLNumber(ref num) => Some(num),
                _ => None,
            })
    }

    /// Verify the cryptographic signature of this certificate revocation list
    ///
    /// `public_key` is the public key of the **signer**.
    ///
    /// Not all algorithms are supported, this function is limited to what `ring` supports.
    #[cfg(any(feature = "verify", feature = "verify-aws"))]
    #[cfg_attr(docsrs, doc(cfg(any(feature = "verify", feature = "verify-aws"))))]
    pub fn verify_signature(&self, public_key: &SubjectPublicKeyInfo) -> Result<(), X509Error> {
        verify_signature(
            public_key,
            &self.signature_algorithm,
            &self.signature_value,
            self.tbs_cert_list.raw,
        )
    }

    ///
    /// Return the raw ASN.1 DER content of the complete signed certificate revocation list that was parsed.
    ///
    /// This includes the to-be-signed (TBS) certificate list, the signature algorithm, and the signature.
    /// If you want just the ASN.1 DER of the TBS certificate list, prefer [`TbsCertList::as_ref()`].
    ///
    /// We avoid the `AsRef` trait in this instance to ensure the full lifetime of the `CertificateRevocationList` is used.
    pub fn as_raw(&self) -> &'a [u8] {
        self.raw
    }
}

impl<'a> AsRef<[u8]> for CertificateRevocationList<'a> {
    fn as_ref(&self) -> &[u8] {
        self.as_raw()
    }
}

/// <pre>
/// CertificateList  ::=  SEQUENCE  {
///      tbsCertList          TBSCertList,
///      signatureAlgorithm   AlgorithmIdentifier,
///      signatureValue       BIT STRING  }
/// </pre>
impl<'a> FromDer<'a, X509Error> for CertificateRevocationList<'a> {
    fn from_der(i: &'a [u8]) -> X509Result<'a, Self> {
        let start_i = i;
        let (rem, mut crl) = parse_der_sequence_defined_g(|i, _| {
            let (i, tbs_cert_list) = TbsCertList::from_der(i)?;
            let (i, signature_algorithm) = AlgorithmIdentifier::from_der(i)?;
            let (i, signature_value) = parse_signature_value(i)?;
            let crl = CertificateRevocationList {
                tbs_cert_list,
                signature_algorithm,
                signature_value,
                raw: &[],
            };
            Ok((i, crl))
        })(i)?;
        let len = start_i.offset(rem);
        crl.raw = &start_i[..len];
        Ok((rem, crl))
    }
}

/// The sequence TBSCertList contains information about the certificates that have
/// been revoked by the CA that issued the CRL.
///
/// RFC5280 definition:
///
/// <pre>
/// TBSCertList  ::=  SEQUENCE  {
///         version                 Version OPTIONAL,
///                                      -- if present, MUST be v2
///         signature               AlgorithmIdentifier,
///         issuer                  Name,
///         thisUpdate              Time,
///         nextUpdate              Time OPTIONAL,
///         revokedCertificates     SEQUENCE OF SEQUENCE  {
///             userCertificate         CertificateSerialNumber,
///             revocationDate          Time,
///             crlEntryExtensions      Extensions OPTIONAL
///                                      -- if present, version MUST be v2
///                                   } OPTIONAL,
///         crlExtensions           [0]  EXPLICIT Extensions OPTIONAL
///                                      -- if present, version MUST be v2
///                             }
/// </pre>
#[derive(Clone, Debug, PartialEq)]
pub struct TbsCertList<'a> {
    pub version: Option<X509Version>,
    pub signature: AlgorithmIdentifier<'a>,
    pub issuer: X509Name<'a>,
    pub this_update: ASN1Time,
    pub next_update: Option<ASN1Time>,
    pub revoked_certificates: Vec<RevokedCertificate<'a>>,
    extensions: Vec<X509Extension<'a>>,
    pub(crate) raw: &'a [u8],
}

impl TbsCertList<'_> {
    /// Returns the certificate extensions
    #[inline]
    pub fn extensions(&self) -> &[X509Extension<'_>] {
        &self.extensions
    }

    /// Returns an iterator over the certificate extensions
    #[inline]
    pub fn iter_extensions(&self) -> impl Iterator<Item = &X509Extension<'_>> {
        self.extensions.iter()
    }

    /// Searches for an extension with the given `Oid`.
    ///
    /// Note: if there are several extensions with the same `Oid`, the first one is returned.
    pub fn find_extension(&self, oid: &Oid) -> Option<&X509Extension<'_>> {
        self.extensions.iter().find(|&ext| ext.oid == *oid)
    }

    /// Builds and returns a map of extensions.
    ///
    /// If an extension is present twice, this will fail and return `DuplicateExtensions`.
    pub fn extensions_map(&self) -> Result<HashMap<Oid<'_>, &X509Extension<'_>>, X509Error> {
        self.extensions
            .iter()
            .try_fold(HashMap::new(), |mut m, ext| {
                if m.contains_key(&ext.oid) {
                    return Err(X509Error::DuplicateExtensions);
                }
                m.insert(ext.oid.clone(), ext);
                Ok(m)
            })
    }
}

impl AsRef<[u8]> for TbsCertList<'_> {
    fn as_ref(&self) -> &[u8] {
        self.raw
    }
}

impl<'a> FromDer<'a, X509Error> for TbsCertList<'a> {
    fn from_der(i: &'a [u8]) -> X509Result<'a, Self> {
        let start_i = i;
        parse_der_sequence_defined_g(move |i, _| {
            let (i, version) =
                opt(map(parse_der_u32, X509Version))(i).or(Err(X509Error::InvalidVersion))?;
            let (i, signature) = AlgorithmIdentifier::from_der(i)?;
            let (i, issuer) = X509Name::from_der(i)?;
            let (i, this_update) = ASN1Time::from_der(i)?;
            let (i, next_update) = ASN1Time::from_der_opt(i)?;
            let (i, revoked_certificates) = opt(complete(parse_revoked_certificates))(i)?;
            let (i, extensions) = parse_extensions(i, Tag(0))?;
            let len = start_i.offset(i);
            let tbs = TbsCertList {
                version,
                signature,
                issuer,
                this_update,
                next_update,
                revoked_certificates: revoked_certificates.unwrap_or_default(),
                extensions,
                raw: &start_i[..len],
            };
            Ok((i, tbs))
        })(i)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RevokedCertificate<'a> {
    /// The Serial number of the revoked certificate
    pub user_certificate: BigUint,
    /// The date on which the revocation occurred is specified.
    pub revocation_date: ASN1Time,
    /// Additional information about revocation
    extensions: Vec<X509Extension<'a>>,
    pub(crate) raw_serial: &'a [u8],
}

impl RevokedCertificate<'_> {
    /// Return the serial number of the revoked certificate
    pub fn serial(&self) -> &BigUint {
        &self.user_certificate
    }

    /// Get the CRL entry extensions.
    #[inline]
    pub fn extensions(&self) -> &[X509Extension<'_>] {
        &self.extensions
    }

    /// Returns an iterator over the CRL entry extensions
    #[inline]
    pub fn iter_extensions(&self) -> impl Iterator<Item = &X509Extension<'_>> {
        self.extensions.iter()
    }

    /// Searches for a CRL entry extension with the given `Oid`.
    ///
    /// Note: if there are several extensions with the same `Oid`, the first one is returned.
    pub fn find_extension(&self, oid: &Oid) -> Option<&X509Extension<'_>> {
        self.extensions.iter().find(|&ext| ext.oid == *oid)
    }

    /// Builds and returns a map of CRL entry extensions.
    ///
    /// If an extension is present twice, this will fail and return `DuplicateExtensions`.
    pub fn extensions_map(&self) -> Result<HashMap<Oid<'_>, &X509Extension<'_>>, X509Error> {
        self.extensions
            .iter()
            .try_fold(HashMap::new(), |mut m, ext| {
                if m.contains_key(&ext.oid) {
                    return Err(X509Error::DuplicateExtensions);
                }
                m.insert(ext.oid.clone(), ext);
                Ok(m)
            })
    }

    /// Get the raw bytes of the certificate serial number
    pub fn raw_serial(&self) -> &[u8] {
        self.raw_serial
    }

    /// Get a formatted string of the certificate serial number, separated by ':'
    pub fn raw_serial_as_string(&self) -> String {
        format_serial(self.raw_serial)
    }

    /// Get the code identifying the reason for the revocation, if present
    pub fn reason_code(&self) -> Option<(bool, ReasonCode)> {
        self.find_extension(&OID_X509_EXT_REASON_CODE)
            .and_then(|ext| match ext.parsed_extension {
                ParsedExtension::ReasonCode(code) => Some((ext.critical, code)),
                _ => None,
            })
    }

    /// Get the invalidity date, if present
    ///
    /// The invalidity date is the date on which it is known or suspected that the private
    ///  key was compromised or that the certificate otherwise became invalid.
    pub fn invalidity_date(&self) -> Option<(bool, ASN1Time)> {
        self.find_extension(&OID_X509_EXT_INVALIDITY_DATE)
            .and_then(|ext| match ext.parsed_extension {
                ParsedExtension::InvalidityDate(date) => Some((ext.critical, date)),
                _ => None,
            })
    }
}

// revokedCertificates     SEQUENCE OF SEQUENCE  {
//     userCertificate         CertificateSerialNumber,
//     revocationDate          Time,
//     crlEntryExtensions      Extensions OPTIONAL
//                                   -- if present, MUST be v2
//                          }  OPTIONAL,
impl<'a> FromDer<'a, X509Error> for RevokedCertificate<'a> {
    fn from_der(i: &'a [u8]) -> X509Result<'a, Self> {
        parse_der_sequence_defined_g(|i, _| {
            let (i, (raw_serial, user_certificate)) = parse_serial(i)?;
            let (i, revocation_date) = ASN1Time::from_der(i)?;
            let (i, extensions) = opt(complete(parse_extension_sequence))(i)?;
            let revoked = RevokedCertificate {
                user_certificate,
                revocation_date,
                extensions: extensions.unwrap_or_default(),
                raw_serial,
            };
            Ok((i, revoked))
        })(i)
    }
}

fn parse_revoked_certificates(i: &[u8]) -> X509Result<'_, Vec<RevokedCertificate<'_>>> {
    parse_der_sequence_defined_g(|a, _| {
        all_consuming(many0(complete(RevokedCertificate::from_der)))(a)
    })(i)
}
