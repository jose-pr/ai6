// Copyright 2015 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

#[cfg(feature = "alloc")]
use alloc::vec::Vec;
use core::fmt;
use core::ops::ControlFlow;

use pki_types::{CertificateDer, SignatureVerificationAlgorithm, TrustAnchor, UnixTime};

use crate::cert::Cert;
use crate::crl::RevocationOptions;
use crate::der::{self, FromDer};
use crate::end_entity::EndEntityCert;
use crate::error::Error;
use crate::{public_values_eq, signed_data, subject_name};

// Use `'a` for lifetimes that we don't care about, `'p` for lifetimes that become a part of
// the `VerifiedPath`.
pub(crate) struct ChainOptions<'a, 'p, V> {
    pub(crate) eku: V,
    pub(crate) supported_sig_algs: &'a [&'a dyn SignatureVerificationAlgorithm],
    pub(crate) trust_anchors: &'p [TrustAnchor<'p>],
    pub(crate) intermediate_certs: &'p [CertificateDer<'p>],
    pub(crate) revocation: Option<RevocationOptions<'a>>,
    pub(crate) distrusted_spkis: &'a [&'a [u8]],
}

impl<'a, 'p: 'a, V: ExtendedKeyUsageValidator> ChainOptions<'a, 'p, V> {
    pub(crate) fn build_chain(
        &self,
        end_entity: &'p EndEntityCert<'p>,
        time: UnixTime,
        verify_path: Option<&dyn Fn(&VerifiedPath<'_>) -> Result<(), Error>>,
    ) -> Result<VerifiedPath<'p>, Error> {
        let mut path = PartialPath::new(end_entity);
        match self.build_chain_inner(&mut path, time, verify_path, 0, &mut Budget::default()) {
            Ok(anchor) => Ok(VerifiedPath::new(end_entity, anchor, path)),
            Err(ControlFlow::Break(err)) | Err(ControlFlow::Continue(err)) => Err(err),
        }
    }

    fn build_chain_inner(
        &self,
        path: &mut PartialPath<'p>,
        time: UnixTime,
        verify_path: Option<&dyn Fn(&VerifiedPath<'_>) -> Result<(), Error>>,
        sub_ca_count: usize,
        budget: &mut Budget,
    ) -> Result<&'p TrustAnchor<'p>, ControlFlow<Error, Error>> {
        if self
            .distrusted_spkis
            .contains(&path.head().spki.as_slice_less_safe())
        {
            return Err(Error::CertDistrusted.into());
        }

        let role = path.node().role();

        check_issuer_independent_properties(path.head(), time, role, sub_ca_count, &self.eku)?;

        // TODO: HPKP checks.

        let result =
            loop_while_non_fatal_error(Error::UnknownIssuer, self.trust_anchors, |trust_anchor| {
                let trust_anchor_subject = untrusted::Input::from(trust_anchor.subject.as_ref());
                if !public_values_eq(path.head().issuer, trust_anchor_subject) {
                    return Err(Error::UnknownIssuer.into());
                }

                if self
                    .distrusted_spkis
                    .contains(&trust_anchor.subject_public_key_info.as_ref())
                {
                    return Err(Error::CertDistrusted.into());
                }

                let node = path.node();
                self.check_signed_chain(&node, time, trust_anchor, budget)?;
                check_signed_chain_name_constraints(&node, trust_anchor, budget)?;

                let verify = match verify_path {
                    Some(verify) => verify,
                    None => return Ok(trust_anchor),
                };

                let candidate = VerifiedPath {
                    end_entity: path.end_entity,
                    intermediates: Intermediates::Borrowed(&path.intermediates[..path.used]),
                    anchor: trust_anchor,
                };

                match verify(&candidate) {
                    Ok(()) => Ok(trust_anchor),
                    Err(err) => Err(ControlFlow::Continue(err)),
                }
            });

        let err = match result {
            Ok(anchor) => return Ok(anchor),
            // Fatal errors should halt further path building.
            res @ Err(ControlFlow::Break(_)) => return res,
            // Non-fatal errors should be carried forward as the default_error for subsequent
            // loop_while_non_fatal_error processing and only returned once all other path-building
            // options have been exhausted.
            Err(ControlFlow::Continue(err)) => err,
        };

        loop_while_non_fatal_error(err, self.intermediate_certs, |cert_der| {
            let potential_issuer = Cert::from_der(untrusted::Input::from(cert_der))?;
            if !public_values_eq(potential_issuer.subject, path.head().issuer) {
                return Err(Error::UnknownIssuer.into());
            }

            // Prevent loops; see RFC 4158 section 5.2.
            if path.node().iter().any(|prev| {
                public_values_eq(potential_issuer.spki, prev.cert.spki)
                    && public_values_eq(potential_issuer.subject, prev.cert.subject)
            }) {
                return Err(Error::UnknownIssuer.into());
            }

            let next_sub_ca_count = match role {
                Role::EndEntity => sub_ca_count,
                Role::Issuer => sub_ca_count + 1,
            };

            budget.consume_build_chain_call()?;
            path.push(potential_issuer)?;
            let result = self.build_chain_inner(path, time, verify_path, next_sub_ca_count, budget);
            if result.is_err() {
                path.pop();
            }

            result
        })
    }

    fn check_signed_chain(
        &self,
        path: &PathNode<'_>,
        time: UnixTime,
        trust_anchor: &TrustAnchor<'_>,
        budget: &mut Budget,
    ) -> Result<(), ControlFlow<Error, Error>> {
        let mut spki_value = untrusted::Input::from(trust_anchor.subject_public_key_info.as_ref());
        let mut issuer_subject = untrusted::Input::from(trust_anchor.subject.as_ref());
        let mut issuer_key_usage = None; // TODO(XXX): Consider whether to track TrustAnchor KU.
        for path in path.iter() {
            signed_data::verify_signed_data(
                self.supported_sig_algs,
                spki_value,
                &path.cert.signed_data,
                budget,
            )?;

            if let Some(revocation_opts) = &self.revocation {
                revocation_opts.check(
                    &path,
                    issuer_subject,
                    spki_value,
                    issuer_key_usage,
                    self.supported_sig_algs,
                    budget,
                    time,
                )?;
            }

            spki_value = path.cert.spki;
            issuer_subject = path.cert.subject;
            issuer_key_usage = path.cert.key_usage;
        }

        Ok(())
    }
}

/// Path from end-entity certificate to trust anchor that's been verified.
///
/// See [`EndEntityCert::verify_for_usage()`] for more details on what verification entails.
pub struct VerifiedPath<'p> {
    end_entity: &'p EndEntityCert<'p>,
    intermediates: Intermediates<'p>,
    anchor: &'p TrustAnchor<'p>,
}

impl<'p> VerifiedPath<'p> {
    fn new(
        end_entity: &'p EndEntityCert<'p>,
        anchor: &'p TrustAnchor<'p>,
        partial: PartialPath<'p>,
    ) -> Self {
        Self {
            end_entity,
            intermediates: Intermediates::Owned {
                certs: partial.intermediates,
                used: partial.used,
            },
            anchor,
        }
    }

    /// Yields a (double-ended) iterator over the intermediate certificates in this path.
    pub fn intermediate_certificates(&'p self) -> IntermediateIterator<'p> {
        IntermediateIterator {
            intermediates: self.intermediates.as_ref(),
        }
    }

    /// Yields the end-entity certificate for this path.
    pub fn end_entity(&self) -> &'p EndEntityCert<'p> {
        self.end_entity
    }

    /// Yields the trust anchor for this path.
    pub fn anchor(&self) -> &'p TrustAnchor<'p> {
        self.anchor
    }
}

/// Iterator over a path's intermediate certificates.
///
/// Implements [`DoubleEndedIterator`] so it can be traversed in both directions.
pub struct IntermediateIterator<'a> {
    /// Invariant: all of these `Option`s are `Some`.
    intermediates: &'a [Option<Cert<'a>>],
}

impl<'a> Iterator for IntermediateIterator<'a> {
    type Item = &'a Cert<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.intermediates.split_first() {
            Some((head, tail)) => {
                self.intermediates = tail;
                Some(head.as_ref().unwrap())
            }
            None => None,
        }
    }
}

impl DoubleEndedIterator for IntermediateIterator<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self.intermediates.split_last() {
            Some((head, tail)) => {
                self.intermediates = tail;
                Some(head.as_ref().unwrap())
            }
            None => None,
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum Intermediates<'a> {
    Owned {
        certs: [Option<Cert<'a>>; MAX_SUB_CA_COUNT],
        used: usize,
    },
    Borrowed(&'a [Option<Cert<'a>>]),
}

impl<'a> AsRef<[Option<Cert<'a>>]> for Intermediates<'a> {
    fn as_ref(&self) -> &[Option<Cert<'a>>] {
        match self {
            Intermediates::Owned { certs, used } => &certs[..*used],
            Intermediates::Borrowed(certs) => certs,
        }
    }
}

fn check_signed_chain_name_constraints(
    path: &PathNode<'_>,
    trust_anchor: &TrustAnchor<'_>,
    budget: &mut Budget,
) -> Result<(), ControlFlow<Error, Error>> {
    let mut name_constraints = trust_anchor
        .name_constraints
        .as_ref()
        .map(|der| untrusted::Input::from(der.as_ref()));

    for path in path.iter() {
        untrusted::read_all_optional(name_constraints, Error::BadDer, |value| {
            subject_name::check_name_constraints(value, &path, budget)
        })?;

        name_constraints = path.cert.name_constraints;
    }

    Ok(())
}

pub(crate) struct Budget {
    signatures: usize,
    build_chain_calls: usize,
    name_constraint_comparisons: usize,
}

impl Budget {
    #[inline]
    pub(crate) fn consume_signature(&mut self) -> Result<(), Error> {
        self.signatures = self
            .signatures
            .checked_sub(1)
            .ok_or(Error::MaximumSignatureChecksExceeded)?;
        Ok(())
    }

    #[inline]
    fn consume_build_chain_call(&mut self) -> Result<(), Error> {
        self.build_chain_calls = self
            .build_chain_calls
            .checked_sub(1)
            .ok_or(Error::MaximumPathBuildCallsExceeded)?;
        Ok(())
    }

    #[inline]
    pub(crate) fn consume_name_constraint_comparison(&mut self) -> Result<(), Error> {
        self.name_constraint_comparisons = self
            .name_constraint_comparisons
            .checked_sub(1)
            .ok_or(Error::MaximumNameConstraintComparisonsExceeded)?;
        Ok(())
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            // This limit is taken from the remediation for golang CVE-2018-16875.  However,
            // note that golang subsequently implemented AKID matching due to this limit
            // being hit in real applications (see <https://github.com/spiffe/spire/issues/1004>).
            // So this may actually be too aggressive.
            signatures: 100,

            // This limit is taken from mozilla::pkix, see:
            // <https://github.com/nss-dev/nss/blob/bb4a1d38dd9e92923525ac6b5ed0288479f3f3fc/lib/mozpkix/lib/pkixbuild.cpp#L381-L393>
            build_chain_calls: 200_000,

            // This limit is taken from golang crypto/x509's default, see:
            // <https://github.com/golang/go/blob/ac17bb6f13979f2ab9fcd45f0758b43ed72d0973/src/crypto/x509/verify.go#L588-L592>
            name_constraint_comparisons: 250_000,
        }
    }
}

fn check_issuer_independent_properties(
    cert: &Cert<'_>,
    time: UnixTime,
    role: Role,
    sub_ca_count: usize,
    eku: &impl ExtendedKeyUsageValidator,
) -> Result<(), Error> {
    // TODO: check_distrust(trust_anchor_subject, trust_anchor_spki)?;
    // TODO: Check signature algorithm like mozilla::pkix.
    // TODO: Check SPKI like mozilla::pkix.
    // TODO: check for active distrust like mozilla::pkix.

    // For cert validation, we ignore the KeyUsage extension. For CA
    // certificates, BasicConstraints.cA makes KeyUsage redundant. Firefox
    // and other common browsers do not check KeyUsage for end-entities,
    // though it would be kind of nice to ensure that a KeyUsage without
    // the keyEncipherment bit could not be used for RSA key exchange.

    cert.validity
        .read_all(Error::BadDer, |value| check_validity(value, time))?;
    untrusted::read_all_optional(cert.basic_constraints, Error::BadDer, |value| {
        check_basic_constraints(value, role, sub_ca_count)
    })?;
    untrusted::read_all_optional(cert.eku, Error::BadDer, |input| check_eku(input, eku))?;

    Ok(())
}

fn check_eku(
    input: Option<&mut untrusted::Reader<'_>>,
    eku: &impl ExtendedKeyUsageValidator,
) -> Result<(), Error> {
    match input {
        Some(input) if input.at_end() => Err(Error::EmptyEkuExtension),
        Some(input) => eku.validate(KeyPurposeIdIter { input }),
        None => eku.validate(KeyPurposeIdIter {
            input: &mut untrusted::Reader::new(untrusted::Input::from(&[])),
        }),
    }
}

// https://tools.ietf.org/html/rfc5280#section-4.1.2.5
fn check_validity(input: &mut untrusted::Reader<'_>, time: UnixTime) -> Result<(), Error> {
    let not_before = UnixTime::from_der(input)?;
    let not_after = UnixTime::from_der(input)?;

    if not_before > not_after {
        return Err(Error::InvalidCertValidity);
    }
    if time < not_before {
        return Err(Error::CertNotValidYet { time, not_before });
    }
    if time > not_after {
        return Err(Error::CertExpired { time, not_after });
    }

    // TODO: mozilla::pkix allows the TrustDomain to check not_before and
    // not_after, to enforce things like a maximum validity period. We should
    // do something similar.

    Ok(())
}

// https://tools.ietf.org/html/rfc5280#section-4.2.1.9
fn check_basic_constraints(
    input: Option<&mut untrusted::Reader<'_>>,
    role: Role,
    sub_ca_count: usize,
) -> Result<(), Error> {
    let (is_ca, path_len_constraint) = match input {
        Some(input) => {
            let is_ca = bool::from_der(input)?;

            // https://bugzilla.mozilla.org/show_bug.cgi?id=985025: RFC 5280
            // says that a certificate must not have pathLenConstraint unless
            // it is a CA certificate, but some real-world end-entity
            // certificates have pathLenConstraint.
            let path_len_constraint = if !input.at_end() {
                Some(usize::from(u8::from_der(input)?))
            } else {
                None
            };

            (is_ca, path_len_constraint)
        }
        None => (false, None),
    };

    match (role, is_ca, path_len_constraint) {
        (Role::EndEntity, true, _) => Err(Error::CaUsedAsEndEntity),
        (Role::Issuer, false, _) => Err(Error::EndEntityUsedAsCa),
        (Role::Issuer, true, Some(len)) if sub_ca_count > len => {
            Err(Error::PathLenConstraintViolated)
        }
        _ => Ok(()),
    }
}

/// Additional context for the `RequiredEkuNotFoundContext` error variant.
///
/// The contents of this type depend on whether the `alloc` feature is enabled.
#[derive(Clone, PartialEq, Eq)]
pub struct RequiredEkuNotFoundContext {
    /// The required ExtendedKeyUsage.
    #[cfg(feature = "alloc")]
    pub required: KeyUsage,
    /// The ExtendedKeyUsage OIDs present in the certificate.
    #[cfg(feature = "alloc")]
    pub present: Vec<Vec<usize>>,
}

impl fmt::Debug for RequiredEkuNotFoundContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = f.debug_struct("RequiredEkuNotFoundContext");
        #[cfg(feature = "alloc")]
        builder.field(
            "required",
            match &self.required.inner {
                ExtendedKeyUsage::Required(inner) => inner,
                ExtendedKeyUsage::RequiredIfPresent(inner) => inner,
            },
        );
        #[cfg(feature = "alloc")]
        builder.field("present", &EkuListDebug(&self.present));
        builder.finish()
    }
}

#[cfg(feature = "alloc")]
struct EkuListDebug<'a>(&'a [Vec<usize>]);

#[cfg(feature = "alloc")]
impl fmt::Debug for EkuListDebug<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[")?;
        for (i, part) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }

            write!(f, "KeyPurposeId(")?;
            for (j, part) in part.iter().enumerate() {
                if j > 0 {
                    write!(f, ".")?;
                }
                write!(f, "{part}")?;
            }
            write!(f, ")")?;
        }
        write!(f, "]")
    }
}

/// The expected key usage of a certificate.
///
/// This type represents the expected key usage of an end entity certificate. Although for most
/// kinds of certificates the extended key usage extension is optional (and so certificates
/// not carrying a particular value in the EKU extension are acceptable). If the extension
/// is present, the certificate MUST only be used for one of the purposes indicated.
///
/// <https://www.rfc-editor.org/rfc/rfc5280#section-4.2.1.12>
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyUsage {
    inner: ExtendedKeyUsage,
}

impl KeyUsage {
    /// Construct a new [`KeyUsage`] as appropriate for server certificate authentication.
    ///
    /// As specified in <https://www.rfc-editor.org/rfc/rfc5280#section-4.2.1.12>, this does not require the certificate to specify the eKU extension.
    pub const fn server_auth() -> Self {
        Self::required_if_present(EKU_SERVER_AUTH)
    }

    /// Construct a new [`KeyUsage`] as appropriate for client certificate authentication.
    ///
    /// As specified in <>, this does not require the certificate to specify the eKU extension.
    pub const fn client_auth() -> Self {
        Self::required_if_present(EKU_CLIENT_AUTH)
    }

    /// Construct a new [`KeyUsage`] requiring a certificate to support the specified OID.
    pub const fn required(oid: &'static [u8]) -> Self {
        Self {
            inner: ExtendedKeyUsage::Required(KeyPurposeId::new(oid)),
        }
    }

    /// Construct a new [`KeyUsage`] requiring a certificate to support the specified OID, if the certificate has EKUs.
    pub const fn required_if_present(oid: &'static [u8]) -> Self {
        Self {
            inner: ExtendedKeyUsage::RequiredIfPresent(KeyPurposeId::new(oid)),
        }
    }

    /// Yield the OID values of the required extended key usage.
    pub fn oid_values(&self) -> impl Iterator<Item = usize> + '_ {
        OidDecoder::new(
            match &self.inner {
                ExtendedKeyUsage::Required(eku) => eku,
                ExtendedKeyUsage::RequiredIfPresent(eku) => eku,
            }
            .oid_value
            .as_slice_less_safe(),
        )
    }

    /// Human-readable representation of the server authentication OID.
    pub const SERVER_AUTH_REPR: &[usize] = &[1, 3, 6, 1, 5, 5, 7, 3, 1];
    /// Human-readable representation of the client authentication OID.
    pub const CLIENT_AUTH_REPR: &[usize] = &[1, 3, 6, 1, 5, 5, 7, 3, 2];
}

impl ExtendedKeyUsageValidator for KeyUsage {
    // https://tools.ietf.org/html/rfc5280#section-4.2.1.12
    fn validate(&self, iter: KeyPurposeIdIter<'_, '_>) -> Result<(), Error> {
        let mut empty = true;
        #[cfg(feature = "alloc")]
        let mut present = Vec::new();

        for id in iter {
            empty = false;
            let id = id?;
            if self.inner.id() == id {
                return Ok(());
            }

            #[cfg(feature = "alloc")]
            present.push(id.to_decoded_oid());
        }

        match (empty, self.inner) {
            (true, ExtendedKeyUsage::RequiredIfPresent(_)) => Ok(()),
            _ => Err(Error::RequiredEkuNotFoundContext(
                RequiredEkuNotFoundContext {
                    #[cfg(feature = "alloc")]
                    required: Self { inner: self.inner },
                    #[cfg(feature = "alloc")]
                    present,
                },
            )),
        }
    }
}

impl<V: ExtendedKeyUsageValidator> ExtendedKeyUsageValidator for &V {
    fn validate(&self, iter: KeyPurposeIdIter<'_, '_>) -> Result<(), Error> {
        (*self).validate(iter)
    }
}

/// A trait for validating the Extended Key Usage (EKU) extensions of a certificate.
pub trait ExtendedKeyUsageValidator {
    /// Validate the EKU values in a certificate.
    ///
    /// `iter` yields the EKU OIDs in the certificate, or an error if the EKU extension
    /// is malformed. `validate()` should yield `Ok(())` if the EKU values match the
    /// required policy, or an `Error` if they do not. Ideally the `Error` should be
    /// `Error::RequiredEkuNotFoundContext` if the policy is not met.
    fn validate(&self, iter: KeyPurposeIdIter<'_, '_>) -> Result<(), Error>;
}

/// Extended Key Usage (EKU) of a certificate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExtendedKeyUsage {
    /// The certificate must contain the specified [`KeyPurposeId`] as EKU.
    Required(KeyPurposeId<'static>),

    /// If the certificate has EKUs, then the specified [`KeyPurposeId`] must be included.
    RequiredIfPresent(KeyPurposeId<'static>),
}

impl ExtendedKeyUsage {
    fn id(&self) -> KeyPurposeId<'static> {
        match self {
            Self::Required(id) => *id,
            Self::RequiredIfPresent(id) => *id,
        }
    }
}

/// Iterator over [`KeyPurposeId`]s, for use in [`ExtendedKeyUsageValidator`].
pub struct KeyPurposeIdIter<'a, 'r> {
    input: &'r mut untrusted::Reader<'a>,
}

impl<'a, 'r> Iterator for KeyPurposeIdIter<'a, 'r> {
    type Item = Result<KeyPurposeId<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.input.at_end() {
            return None;
        }

        Some(der::expect_tag(self.input, der::Tag::OID).map(|oid_value| KeyPurposeId { oid_value }))
    }
}

impl Drop for KeyPurposeIdIter<'_, '_> {
    fn drop(&mut self) {
        self.input.skip_to_end();
    }
}

/// An OID value indicating an Extended Key Usage (EKU) key purpose.
#[derive(Clone, Copy)]
pub struct KeyPurposeId<'a> {
    oid_value: untrusted::Input<'a>,
}

impl<'a> KeyPurposeId<'a> {
    /// Construct a new [`KeyPurposeId`].
    ///
    /// `oid` is the OBJECT IDENTIFIER in bytes.
    pub const fn new(oid: &'a [u8]) -> Self {
        Self {
            oid_value: untrusted::Input::from(oid),
        }
    }

    /// Yield the OID value as a sequence of `usize` components.
    #[cfg(feature = "alloc")]
    pub fn to_decoded_oid(&self) -> Vec<usize> {
        OidDecoder::new(self.oid_value.as_slice_less_safe()).collect()
    }
}

impl fmt::Debug for KeyPurposeId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyPurposeId(")?;
        let decoder = OidDecoder::new(self.oid_value.as_slice_less_safe());
        for (i, part) in decoder.enumerate() {
            if i > 0 {
                write!(f, ".")?;
            }
            write!(f, "{part}")?;
        }
        write!(f, ")")
    }
}

impl PartialEq<Self> for KeyPurposeId<'_> {
    fn eq(&self, other: &Self) -> bool {
        public_values_eq(self.oid_value, other.oid_value)
    }
}

impl Eq for KeyPurposeId<'_> {}

// id-pkix            OBJECT IDENTIFIER ::= { 1 3 6 1 5 5 7 }
// id-kp              OBJECT IDENTIFIER ::= { id-pkix 3 }

// id-kp-serverAuth   OBJECT IDENTIFIER ::= { id-kp 1 }
const EKU_SERVER_AUTH: &[u8] = &oid!(1, 3, 6, 1, 5, 5, 7, 3, 1);

// id-kp-clientAuth   OBJECT IDENTIFIER ::= { id-kp 2 }
const EKU_CLIENT_AUTH: &[u8] = &oid!(1, 3, 6, 1, 5, 5, 7, 3, 2);

struct OidDecoder<'a> {
    encoded: &'a [u8],
    left: Option<usize>,
    first: bool,
}

impl<'a> OidDecoder<'a> {
    fn new(encoded: &'a [u8]) -> Self {
        Self {
            encoded,
            left: None,
            first: true,
        }
    }
}

impl Iterator for OidDecoder<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(next) = self.left.take() {
            return Some(next);
        }

        let mut cur = 0;
        for (i, &byte) in self.encoded.iter().enumerate() {
            cur = (cur << 8) + usize::from(byte & 0x7f);
            if byte & 0x80 > 0 {
                continue;
            }

            if !self.first {
                self.encoded = &self.encoded[i + 1..];
                return Some(cur);
            }

            let (cur, next) = match cur {
                ..=39 => (0, cur),
                40..=79 => (1, cur - 40),
                _ => (2, cur - 80),
            };

            self.encoded = &self.encoded[i + 1..];
            self.first = false;
            self.left = Some(next);
            return Some(cur);
        }

        None
    }
}

fn loop_while_non_fatal_error<'a, V: IntoIterator + 'a>(
    default_error: Error,
    values: V,
    mut f: impl FnMut(V::Item) -> Result<&'a TrustAnchor<'a>, ControlFlow<Error, Error>>,
) -> Result<&'a TrustAnchor<'a>, ControlFlow<Error, Error>> {
    let mut error = default_error;
    for v in values {
        match f(v) {
            Ok(anchor) => return Ok(anchor),
            // Fatal errors should halt further looping.
            res @ Err(ControlFlow::Break(_)) => return res,
            // Non-fatal errors should be ranked by specificity and only returned
            // once all other path-building options have been exhausted.
            Err(ControlFlow::Continue(new_error)) => error = error.most_specific(new_error),
        }
    }
    Err(error.into())
}

/// A path for consideration in path building.
///
/// This represents a partial path because it does not yet contain the trust anchor. It stores
/// the end-entity certificates, and an array of intermediate certificates.
pub(crate) struct PartialPath<'a> {
    end_entity: &'a EndEntityCert<'a>,
    /// Intermediate certificates, in order from end-entity to trust anchor.
    ///
    /// Invariant: all values below `used` are `Some`.
    intermediates: [Option<Cert<'a>>; MAX_SUB_CA_COUNT],
    /// The number of `Some` values in `intermediates`.
    ///
    /// The next `Cert` passed to `push()` will be placed at `intermediates[used]`.
    /// If this value is 0, the path contains only the end-entity certificate.
    used: usize,
}

impl<'a> PartialPath<'a> {
    pub(crate) fn new(end_entity: &'a EndEntityCert<'a>) -> Self {
        Self {
            end_entity,
            intermediates: Default::default(),
            used: 0,
        }
    }

    pub(crate) fn push(&mut self, cert: Cert<'a>) -> Result<(), ControlFlow<Error, Error>> {
        if self.used >= MAX_SUB_CA_COUNT {
            return Err(Error::MaximumPathDepthExceeded.into());
        }

        self.intermediates[self.used] = Some(cert);
        self.used += 1;
        Ok(())
    }

    fn pop(&mut self) {
        debug_assert!(self.used > 0);
        if self.used == 0 {
            return;
        }

        self.used -= 1;
        self.intermediates[self.used] = None;
    }

    pub(crate) fn node(&self) -> PathNode<'_> {
        PathNode {
            path: self,
            index: self.used,
            cert: self.head(),
        }
    }

    /// Current head of the path.
    pub(crate) fn head(&self) -> &Cert<'a> {
        self.get(self.used)
    }

    /// Get the certificate at index `idx` in the path.
    ///
    // `idx` must be in the range `0..=self.used`; `idx` 0 thus yields the `end_entity`,
    // while subsequent indexes yield the intermediate at `self.intermediates[idx - 1]`.
    fn get(&self, idx: usize) -> &Cert<'a> {
        match idx {
            0 => self.end_entity,
            _ => self.intermediates[idx - 1].as_ref().unwrap(),
        }
    }
}

const MAX_SUB_CA_COUNT: usize = 6;

pub(crate) struct PathNode<'a> {
    /// The path we're iterating.
    path: &'a PartialPath<'a>,
    /// The index of the current node in the path (input for `path.get()`).
    index: usize,
    /// The [`Cert`] at `index`.
    pub(crate) cert: &'a Cert<'a>,
}

impl<'a> PathNode<'a> {
    pub(crate) fn iter(&self) -> PathIter<'a> {
        PathIter {
            path: self.path,
            next: Some(self.index),
        }
    }

    pub(crate) fn role(&self) -> Role {
        match self.index {
            0 => Role::EndEntity,
            _ => Role::Issuer,
        }
    }
}

pub(crate) struct PathIter<'a> {
    path: &'a PartialPath<'a>,
    next: Option<usize>,
}

impl<'a> Iterator for PathIter<'a> {
    type Item = PathNode<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.next?;
        self.next = match next {
            0 => None,
            _ => Some(next - 1),
        };

        Some(PathNode {
            path: self.path,
            index: next,
            cert: self.path.get(next),
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Role {
    Issuer,
    EndEntity,
}

// SPKI SEQUENCE contents (without tag/length) that should invalidate any path containing them.
pub(crate) const DISTRUSTED_SPKIS: &[&[u8]] = &[
    &[0x30u8,0x10,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,0x05,0x2b,0x81,0x04,0x00,0x22,0x03,0x62,0x00,0x04,0xe7,0x9e,0xd9,0xc0,0x4a,0x97,0x73,0xc4,0x73,0x5e,0x51,0x11,0x1d,0xa8,0x5a,0xc2,0xbd,0x5e,0x64,0xab,0x37,0x4b,0x80,0xe1,0x2c,0xbc,0x20,0x38,0xec,0x1e,0x4f,0x0e,0x3b,0x81,0x2f,0xc9,0x85,0x9e,0x1e,0x67,0xaf,0xb0,0x4e,0x2f,0x8b,0xc9,0x7f,0x74,0x85,0xba,0x65,0xaa,0x91,0x1e,0xe8,0xa4,0xe4,0x5b,0x62,0xd5,0xcc,0x03,0xd5,0x6b,0xe6,0x78,0xb3,0xc3,0x8c,0xb4,0xdc,0x93,0x7c,0xa2,0x46,0x0e,0x23,0x84,0x53,0xc8,0x92,0x93,0x5d,0x29,0xd4,0x4f,0x91,0xe1,0x56,0x06,0xb6,0x3c,0xce,0xda,0xb4,0x68],
    &[0x30u8,0x10,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,0x05,0x2b,0x81,0x04,0x00,0x22,0x03,0x62,0x00,0x04,0x3a,0x7a,0x78,0x62,0xc8,0xc4,0x4d,0x32,0x4d,0xdd,0x69,0x03,0x3a,0x67,0x82,0xc0,0xef,0xee,0xfe,0x40,0x55,0xe6,0x8a,0x12,0x3f,0xa0,0x37,0x1b,0xa3,0xbd,0x91,0x94,0x51,0x4e,0xe1,0x72,0xa4,0x33,0xe3,0x63,0x0f,0x02,0xbd,0x18,0x80,0x37,0xd9,0xe5,0x14,0x9f,0x9b,0x77,0xdb,0x94,0xa2,0x81,0xdf,0x98,0x7b,0x2f,0xea,0x88,0x8b,0x74,0xbd,0xfe,0x8f,0xda,0x4c,0x10,0x99,0xee,0x6c,0xa1,0x95,0x66,0xfe,0x3e,0xf0,0xe5,0xf6,0xb0,0x9a,0x55,0x6c,0xcc,0x8b,0xa5,0x05,0x61,0xdf,0x1b,0x2a,0x44,0x1d,0x01],
    &[0x30u8,0x10,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,0x05,0x2b,0x81,0x04,0x00,0x22,0x03,0x62,0x00,0x04,0x25,0xdb,0xa8,0x5a,0x64,0xd3,0x97,0xda,0x0b,0x10,0x52,0xe9,0xea,0x54,0x6e,0x48,0x7b,0xef,0xea,0x9c,0x9d,0x9d,0x68,0xa0,0xc5,0xea,0xdc,0xfe,0xef,0x75,0xa6,0x9b,0xb5,0xd6,0xfc,0x01,0xf4,0x2b,0xfb,0xca,0x6e,0xb4,0x0b,0xcd,0x64,0xc0,0x76,0x7e,0x36,0xff,0xd0,0x5d,0x97,0xa8,0x13,0xc5,0xaa,0x0c,0x6c,0x95,0x30,0xe7,0x9c,0xf5,0xf0,0x86,0x64,0x68,0xf1,0x38,0x06,0x42,0x0a,0xa5,0xc8,0x51,0x84,0x0b,0x1e,0x25,0x25,0x40,0xe7,0x90,0xe4,0xb6,0x54,0xc3,0x85,0x2e,0xd6,0x4a,0xf1,0xe8,0x65,0x7d],
    &[0x30u8,0x10,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,0x05,0x2b,0x81,0x04,0x00,0x22,0x03,0x62,0x00,0x04,0x2d,0x56,0xe8,0x29,0xa6,0xfb,0x82,0xc6,0x4a,0x46,0x97,0xd9,0x60,0xaf,0x95,0x1e,0x90,0x67,0xae,0x91,0xf9,0x26,0xd3,0x43,0x06,0xda,0x48,0xa8,0xaa,0xeb,0x78,0x69,0xb6,0xa6,0x7e,0x6d,0x27,0x42,0x6e,0x04,0x42,0xaa,0x03,0xb3,0x6e,0x19,0x88,0xdf,0xf6,0x36,0x51,0x4e,0xe1,0xcf,0x07,0xf5,0xba,0x4b,0x3f,0xf2,0xa0,0x66,0x45,0xae,0x7d,0x31,0xd5,0x86,0x6a,0xd3,0x1a,0xa3,0xd2,0x46,0xeb,0x0b,0x31,0x77,0xee,0x99,0xcb,0x75,0x66,0xc5,0x16,0xce,0x74,0x80,0x8a,0x66,0x8f,0x91,0xf0,0xc4,0x7f,0xca],
    &[0x30u8,0x10,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,0x05,0x2b,0x81,0x04,0x00,0x22,0x03,0x62,0x00,0x04,0x73,0x89,0x7a,0xa0,0xeb,0x3c,0xfd,0xca,0x8b,0x12,0x40,0x2f,0xfa,0xf0,0xab,0xc4,0xe1,0x21,0x2b,0xaf,0x8d,0xee,0x77,0x8a,0xc9,0xee,0x2d,0x6d,0xb8,0x3f,0xac,0x28,0x0c,0x5e,0x05,0x63,0x8a,0xe7,0xa2,0xac,0xd7,0x1c,0x32,0x2a,0x8e,0x36,0x1c,0xde,0xb9,0xf6,0x9c,0x43,0x02,0xb9,0xdd,0x8d,0x73,0xd1,0xcc,0x6c,0xaa,0xb2,0xcc,0x0a,0xee,0x4e,0x21,0x6c,0xe9,0x79,0xdd,0x51,0x8a,0x4a,0xf4,0xb6,0x26,0xe5,0x68,0x4c,0xdd,0x65,0x80,0x37,0xed,0x0c,0x85,0x4e,0x4c,0xc1,0x2f,0x4c,0xf8,0x64,0x6d,0x0d],
    &[0x30u8,0x10,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,0x05,0x2b,0x81,0x04,0x00,0x22,0x03,0x62,0x00,0x04,0x13,0x82,0x51,0xcd,0x52,0xac,0x92,0x98,0xc1,0xc8,0xaa,0xd9,0x77,0x32,0x1d,0xeb,0x97,0xe7,0x09,0xbd,0x0b,0x4c,0xa0,0xac,0xa5,0x5d,0xc8,0xad,0x51,0xdc,0xfc,0x9d,0x15,0x89,0xa1,0x59,0x7e,0x3a,0x51,0x20,0xe1,0xef,0xd6,0x31,0xc6,0x3e,0x18,0x35,0xca,0xca,0xe2,0x98,0x69,0xa6,0x2e,0x16,0x31,0xe8,0xa2,0x81,0x81,0xab,0x56,0x61,0x6d,0xc4,0x5d,0x91,0x8a,0xbc,0x09,0xf3,0xab,0x0e,0x63,0xcf,0x79,0x2a,0xa4,0xdc,0xed,0x73,0x87,0xbe,0x37,0xbb,0xa5,0x69,0x54,0x9f,0x1c,0x02,0xb2,0x70,0xed,0x67],
    &[0x30u8,0x10,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,0x05,0x2b,0x81,0x04,0x00,0x22,0x03,0x62,0x00,0x04,0xaa,0xbb,0x0f,0xb0,0x66,0x6a,0xcf,0xcf,0x01,0xfa,0xd5,0x2b,0xfc,0xfd,0x4c,0x2b,0xf2,0xcb,0x3c,0x34,0x97,0xde,0x73,0x95,0x0e,0x58,0x19,0xb6,0x1f,0xe6,0x95,0x09,0x87,0x41,0xc8,0xc4,0x91,0x43,0x9b,0x34,0xa0,0x43,0x1c,0xaf,0xfe,0x5b,0xf9,0xb4,0xc1,0xc8,0xe8,0xae,0x00,0x01,0xf9,0xf4,0xdc,0x8a,0xd8,0x2c,0x59,0xf3,0x63,0x9e,0x96,0xcd,0x6c,0x3d,0x4d,0xb0,0xce,0xfe,0xc2,0x6c,0x3d,0x62,0x51,0x7b,0x37,0x76,0x13,0x72,0x74,0x3f,0x8e,0x8a,0xdd,0x77,0xb1,0x00,0x64,0xf2,0xab,0xe5,0xe0,0x50],
    &[0x30u8,0x10,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,0x05,0x2b,0x81,0x04,0x00,0x22,0x03,0x62,0x00,0x04,0xb9,0xf0,0xe0,0x63,0x47,0x02,0xa6,0x9c,0xa6,0xaa,0xdd,0x44,0x3c,0xfd,0x62,0xc4,0x1b,0x9b,0xa4,0xb2,0x09,0x24,0x69,0x7d,0x79,0x74,0xe7,0x69,0x1a,0x12,0x6f,0xb9,0x6a,0xfb,0x3e,0x34,0xc2,0xfd,0x3b,0xc5,0xf4,0x83,0x06,0x1f,0x26,0x5d,0xc2,0xa5,0x84,0x36,0x34,0x73,0x23,0x51,0x04,0x36,0xb2,0x8f,0x04,0xf3,0x86,0x23,0x63,0x06,0x79,0x80,0xe2,0x50,0x3e,0x02,0x53,0x2d,0x65,0x46,0x23,0x5a,0x71,0x65,0xa2,0x59,0xa1,0xef,0x49,0x6f,0x74,0x1b,0x5f,0xde,0xe7,0x78,0x72,0x08,0xe4,0xb8,0xfa,0xef],
];

#[cfg(all(test, feature = "alloc", any(feature = "ring", feature = "aws-lc-rs")))]
mod tests {
    use alloc::borrow::ToOwned;
    use alloc::string::{String, ToString};
    use std::dbg;
    use std::slice;

    use super::*;
    use crate::test_utils;
    use crate::test_utils::{issuer_params, make_end_entity, make_issuer};
    use crate::trust_anchor::anchor_from_trusted_cert;
    use rcgen::{CertifiedIssuer, Issuer, KeyPair, SigningKey};

    #[test]
    fn roundtrip() {
        // 2.999.3 -> 1079.3 -> [0x84, 0x37, 0x3]
        const ENCODED: &[u8] = &[0x84, 0x37, 0x3];
        let decoded = OidDecoder::new(ENCODED);
        assert_eq!(decoded.collect::<Vec<_>>(), [2, 999, 3]);
    }

    #[test]
    fn oid_decoding() {
        assert_eq!(
            KeyUsage::server_auth().oid_values().collect::<Vec<_>>(),
            KeyUsage::SERVER_AUTH_REPR
        );
        assert_eq!(
            KeyUsage::client_auth().oid_values().collect::<Vec<_>>(),
            KeyUsage::CLIENT_AUTH_REPR
        );
    }

    #[test]
    fn eku_fail_empty() {
        let err = KeyUsage::required(EKU_SERVER_AUTH)
            .validate(KeyPurposeIdIter {
                input: &mut untrusted::Reader::new(untrusted::Input::from(&[])),
            })
            .unwrap_err();
        assert_eq!(
            err,
            Error::RequiredEkuNotFoundContext(RequiredEkuNotFoundContext {
                #[cfg(feature = "alloc")]
                required: dbg!(KeyUsage::required(EKU_SERVER_AUTH)), // Cover Debug impl
                #[cfg(feature = "alloc")]
                present: Vec::new(),
            })
        );
    }

    #[test]
    fn eku_fail_empty_with_optional() {
        let mut empty_eku = untrusted::Reader::new(untrusted::Input::from(&[]));
        let validator = KeyUsage::required_if_present(EKU_SERVER_AUTH);
        assert_eq!(
            check_eku(Some(&mut empty_eku), &validator).unwrap_err(),
            Error::EmptyEkuExtension,
        );
    }

    #[test]
    fn eku_key_purpose_id() {
        assert!(
            ExtendedKeyUsage::RequiredIfPresent(KeyPurposeId::new(EKU_SERVER_AUTH)).id()
                == KeyPurposeId::new(EKU_SERVER_AUTH)
        )
    }

    #[test]
    fn test_too_many_signatures() {
        assert!(matches!(
            build_and_verify_degenerate_chain(5, ChainTrustAnchor::NotInChain),
            ControlFlow::Break(Error::MaximumSignatureChecksExceeded)
        ));
    }

    #[test]
    fn test_too_many_path_calls() {
        assert!(matches!(
            dbg!(build_and_verify_degenerate_chain(
                10,
                ChainTrustAnchor::InChain
            )),
            ControlFlow::Break(Error::MaximumPathBuildCallsExceeded)
        ));
    }

    #[test]
    fn longest_allowed_path() {
        assert!(build_and_verify_linear_chain(1).is_ok());
        assert!(build_and_verify_linear_chain(2).is_ok());
        assert!(build_and_verify_linear_chain(3).is_ok());
        assert!(build_and_verify_linear_chain(4).is_ok());
        assert!(build_and_verify_linear_chain(5).is_ok());
        assert!(build_and_verify_linear_chain(6).is_ok());
    }

    #[test]
    fn path_too_long() {
        assert!(matches!(
            build_and_verify_linear_chain(7),
            Err(ControlFlow::Continue(Error::MaximumPathDepthExceeded))
        ));
    }

    #[test]
    fn name_constraint_budget() {
        // Issue a trust anchor that imposes name constraints. The constraint should match
        // the end entity certificate SAN.
        let mut ca_cert_params = issuer_params("Constrained Root");
        ca_cert_params.name_constraints = Some(rcgen::NameConstraints {
            permitted_subtrees: vec![rcgen::GeneralSubtree::DnsName(".com".into())],
            excluded_subtrees: vec![],
        });
        let ca_key_pair = KeyPair::generate_for(test_utils::RCGEN_SIGNATURE_ALG).unwrap();
        let ca = CertifiedIssuer::self_signed(ca_cert_params, ca_key_pair).unwrap();

        // Create a series of intermediate issuers. We'll only use one in the actual built path,
        // helping demonstrate that the name constraint budget is not expended checking certificates
        // that are not part of the path we compute.
        let mut intermediates = Vec::with_capacity(5);
        for i in 0..5 {
            let intermediate = issuer_params(format!("Intermediate {i}"));
            let intermediate_key_pair =
                KeyPair::generate_for(test_utils::RCGEN_SIGNATURE_ALG).unwrap();
            // Each intermediate should be issued by the trust anchor.
            intermediates.push(
                CertifiedIssuer::signed_by(intermediate, intermediate_key_pair, &ca).unwrap(),
            );
        }

        // Create an end-entity cert that is issued by the last of the intermediates.
        let ee_cert = make_end_entity(intermediates.last().unwrap());
        let ee_cert = EndEntityCert::try_from(ee_cert.cert.der()).unwrap();

        // We use a custom budget to make it easier to write a test, otherwise it is tricky to
        // stuff enough names/constraints into the potential chains while staying within the path
        // depth limit and the build chain call limit.
        let passing_budget = Budget {
            // One comparison against the intermediate's distinguished name.
            // One comparison against the EE's distinguished name.
            // One comparison against the EE's SAN.
            //  = 3 total comparisons.
            name_constraint_comparisons: 3,
            ..Budget::default()
        };

        let anchors = &[anchor_from_trusted_cert(ca.der()).unwrap()];
        let intermediates_der = intermediates
            .iter()
            .map(|issuer| issuer.der().clone())
            .collect::<Vec<_>>();

        // Validation should succeed with the name constraint comparison budget allocated above.
        // This shows that we're not consuming budget on unused intermediates: we didn't budget
        // enough comparisons for that to pass the overall chain building.
        let path = verify_chain(
            anchors,
            &intermediates_der,
            &ee_cert,
            None,
            Some(passing_budget),
        )
        .unwrap();
        assert_eq!(path.anchor().subject, anchors.first().unwrap().subject);

        let failing_budget = Budget {
            // See passing_budget: 2 comparisons is not sufficient.
            name_constraint_comparisons: 2,
            ..Budget::default()
        };
        // Validation should fail when the budget is smaller than the number of comparisons performed
        // on the validated path. This demonstrates we properly fail path building when too many
        // name constraint comparisons occur.
        let result = verify_chain(
            anchors,
            &intermediates_der,
            &ee_cert,
            None,
            Some(failing_budget),
        );

        assert!(matches!(
            result,
            Err(ControlFlow::Break(
                Error::MaximumNameConstraintComparisonsExceeded
            ))
        ));
    }

    /// This test builds a PKI like the following diagram depicts. We first verify
    /// that we can build a path EE -> B -> A -> TA. Next we supply a custom path verification
    /// function that rejects the B->A path, and verify that we build a path EE -> B -> C -> TA.
    ///
    ///        ┌───────────┐
    ///        │           │
    ///        │     TA    │
    ///        │           │
    ///        └───┬───┬───┘
    ///            │   │
    ///            │   │
    /// ┌────────┐◄┘   └──►┌────────┐
    /// │        │         │        │
    /// │   A    │         │   C    │
    /// │        │         │        │
    /// └────┬───┘         └───┬────┘
    ///      │                 │
    ///      │                 │
    ///      │   ┌─────────┐   │
    ///      └──►│         │◄──┘
    ///          │    B    │
    ///          │         │
    ///          └────┬────┘
    ///               │
    ///               │
    ///               │
    ///          ┌────▼────┐
    ///          │         │
    ///          │    EE   │
    ///          │         │
    ///          └─────────┘
    #[test]
    fn test_reject_candidate_path() {
        // Create a trust anchor, and use it to issue two distinct intermediate certificates, each
        // with a unique subject and keypair.
        let trust_anchor = make_issuer("Trust Anchor");
        let trust_anchor_cert = Cert::from_der(untrusted::Input::from(trust_anchor.der())).unwrap();
        let trust_anchors = &[anchor_from_trusted_cert(trust_anchor.der()).unwrap()];

        let intermediate_a = make_intermediate("Intermediate A", &trust_anchor);
        let intermediate_c = make_intermediate("Intermediate C", &trust_anchor);

        // Next, create an intermediate that is issued by both of the intermediates above.
        // Both should share the same subject, and key pair, but will differ in the issuer.
        let (intermediate_b, intermediate_b_a_cert, intermediate_b_c_cert) = {
            let key = KeyPair::generate_for(test_utils::RCGEN_SIGNATURE_ALG).unwrap();
            let params = issuer_params("Intermediate");
            let intermediate_b_a_cert = params.signed_by(&key, &intermediate_a).unwrap();
            let intermediate_b_c_cert = params.signed_by(&key, &intermediate_c).unwrap();
            let issuer = Issuer::new(params, key);
            (issuer, intermediate_b_a_cert, intermediate_b_c_cert)
        };

        let intermediates = &[
            intermediate_a.der().clone(),
            intermediate_c.der().clone(),
            intermediate_b_a_cert.der().clone(),
            intermediate_b_c_cert.der().clone(),
        ];

        // Create an end entity certificate signed by the keypair of the intermediates created above.
        let ee = make_end_entity(&intermediate_b);
        let ee_cert = &EndEntityCert::try_from(ee.cert.der()).unwrap();

        // We should be able to create a valid path from EE to trust anchor.
        let path = verify_chain(trust_anchors, intermediates, ee_cert, None, None).unwrap();
        let path_intermediates = path.intermediate_certificates().collect::<Vec<_>>();

        // We expect that without applying any additional constraints, that the path will be
        // EE -> intermediate_b_a -> intermediate_a -> trust_anchor.
        let intermediate_a_cert =
            Cert::from_der(untrusted::Input::from(intermediate_a.der())).unwrap();
        assert_eq!(path_intermediates.len(), 2);
        assert_eq!(
            path_intermediates[0].issuer(),
            intermediate_a_cert.subject()
        );
        assert_eq!(path_intermediates[1].issuer(), trust_anchor_cert.subject());

        // Now, we'll create a function that will reject the intermediate_b_a path.
        let expected_chain = |path: &VerifiedPath<'_>| {
            for intermediate in path.intermediate_certificates() {
                // Reject any intermediates issued by intermediate A.
                if intermediate.issuer() == intermediate_a_cert.subject() {
                    return Err(Error::UnknownIssuer);
                }
            }

            Ok(())
        };

        // We should still be able to build a valid path.
        let path = verify_chain(
            trust_anchors,
            intermediates,
            ee_cert,
            Some(&expected_chain),
            None,
        )
        .unwrap();
        let path_intermediates = path.intermediate_certificates().collect::<Vec<_>>();

        // We expect that the path will now be
        // EE -> intermediate_b_c -> intermediate_c -> trust_anchor.
        let intermediate_c_cert =
            Cert::from_der(untrusted::Input::from(intermediate_c.der())).unwrap();
        assert_eq!(path_intermediates.len(), 2);
        assert_eq!(
            path_intermediates[0].issuer(),
            intermediate_c_cert.subject()
        );
        assert_eq!(path_intermediates[1].issuer(), trust_anchor_cert.subject());
    }

    #[test]
    fn distrusted_spki() {
        fn verify(
            ee_cert: &EndEntityCert<'_>,
            intermediates: &[CertificateDer<'_>],
            trust_anchors: &[TrustAnchor<'_>],
            distrusted_spkis: &[&[u8]],
        ) -> Result<usize, Error> {
            use core::time::Duration;

            ChainOptions {
                eku: KeyUsage::server_auth(),
                supported_sig_algs: crate::ALL_VERIFICATION_ALGS,
                trust_anchors,
                intermediate_certs: intermediates,
                revocation: None,
                distrusted_spkis,
            }
            .build_chain(
                ee_cert,
                UnixTime::since_unix_epoch(Duration::from_secs(0x1fed_f00d)),
                None,
            )
            .map(|path| path.intermediate_certificates().count())
        }

        let root = make_issuer("Root");
        let trust_anchors = &[anchor_from_trusted_cert(root.der()).unwrap()];
        let issuing_intermediate = make_intermediate("Issuing Intermediate", &root);
        let unrelated = make_intermediate("Intermediate", &root);

        let params = issuer_params("Intermediate");
        let key = KeyPair::generate_for(test_utils::RCGEN_SIGNATURE_ALG).unwrap();
        // Issue the same SPKI under two different issuers to test cross-signing.
        let intermediate = params.signed_by(&key, &issuing_intermediate).unwrap();
        let cross_signed = params.signed_by(&key, &root).unwrap();
        let ee = make_end_entity(&Issuer::new(params, key));
        let ee_cert = EndEntityCert::try_from(ee.cert.der()).unwrap();
        let intermediates = &[
            unrelated.der().clone(),
            intermediate.der().clone(),
            issuing_intermediate.der().clone(),
        ];
        assert_eq!(
            verify(&ee_cert, intermediates, trust_anchors, DISTRUSTED_SPKIS),
            Ok(2)
        );

        // Distrusting any certificate in the path, including the root or leaf, rejects the chain.
        for cert in [
            root.der(),
            issuing_intermediate.der(),
            intermediate.der(),
            ee.cert.der(),
        ] {
            assert_eq!(
                verify(
                    &ee_cert,
                    intermediates,
                    trust_anchors,
                    &[anchor_from_trusted_cert(cert)
                        .unwrap()
                        .subject_public_key_info
                        .as_ref()]
                ),
                Err(Error::CertDistrusted)
            );
        }

        // Distrusting an unused intermediate leaves the valid path usable.
        assert_eq!(
            verify(
                &ee_cert,
                intermediates,
                trust_anchors,
                &[anchor_from_trusted_cert(unrelated.der())
                    .unwrap()
                    .subject_public_key_info
                    .as_ref()]
            ),
            Ok(2)
        );

        // A different cross-sign provides a valid path around the distrusted issuer.
        let intermediates = &[
            intermediate.der().clone(),
            issuing_intermediate.der().clone(),
            cross_signed.der().clone(),
        ];
        let issuer_spki = anchor_from_trusted_cert(issuing_intermediate.der()).unwrap();
        assert_eq!(
            verify(
                &ee_cert,
                intermediates,
                trust_anchors,
                &[issuer_spki.subject_public_key_info.as_ref()]
            ),
            Ok(1)
        );

        // Distrusting the shared SPKI rejects the original certificate and its cross-sign.
        let intermediate_spki = anchor_from_trusted_cert(intermediate.der()).unwrap();
        assert_eq!(
            verify(
                &ee_cert,
                intermediates,
                trust_anchors,
                &[intermediate_spki.subject_public_key_info.as_ref()]
            ),
            Err(Error::CertDistrusted)
        );

        // Reject a distrusted intermediate before searching for its issuer.
        assert_eq!(
            verify(
                &ee_cert,
                &[intermediate.der().clone()],
                &[],
                &[intermediate_spki.subject_public_key_info.as_ref()]
            ),
            Err(Error::CertDistrusted)
        );
    }

    fn make_intermediate(
        org_name: impl Into<String>,
        issuer: &Issuer<'_, impl SigningKey>,
    ) -> CertifiedIssuer<'static, KeyPair> {
        let params = issuer_params(org_name);
        let key = KeyPair::generate_for(test_utils::RCGEN_SIGNATURE_ALG).unwrap();
        CertifiedIssuer::signed_by(params, key, issuer).unwrap()
    }

    fn build_and_verify_degenerate_chain(
        intermediate_count: usize,
        trust_anchor: ChainTrustAnchor,
    ) -> ControlFlow<Error, Error> {
        let ca = make_issuer("Bogus Subject");
        let mut intermediate_chain = IntermediateChain::new(intermediate_count, true, &ca);

        let verify_trust_anchor = match trust_anchor {
            ChainTrustAnchor::InChain => make_issuer("Bogus Trust Anchor"),
            ChainTrustAnchor::NotInChain => ca,
        };

        let ee_cert = make_end_entity(&intermediate_chain.last_issuer);
        let ee_cert = EndEntityCert::try_from(ee_cert.cert.der()).unwrap();
        let webpki_ta = anchor_from_trusted_cert(verify_trust_anchor.der()).unwrap();
        if matches!(trust_anchor, ChainTrustAnchor::InChain) {
            // Note: we clone the trust anchor DER here because we can't move it into the chain
            // as it's loaned to webpki_ta above.
            intermediate_chain
                .chain
                .insert(0, verify_trust_anchor.der().to_owned())
        }

        verify_chain(
            &[webpki_ta],
            &intermediate_chain.chain,
            &ee_cert,
            None,
            None,
        )
        .map(|_| ())
        .unwrap_err()
    }

    #[cfg(feature = "alloc")]
    enum ChainTrustAnchor {
        NotInChain,
        InChain,
    }

    fn build_and_verify_linear_chain(chain_length: usize) -> Result<(), ControlFlow<Error, Error>> {
        let ca = make_issuer(format!("Bogus Subject {chain_length}"));
        let intermediate_chain = IntermediateChain::new(chain_length, false, &ca);

        let anchor = anchor_from_trusted_cert(ca.der()).unwrap();
        let anchors = slice::from_ref(&anchor);

        let ee_cert = make_end_entity(&intermediate_chain.last_issuer);
        let ee_cert = EndEntityCert::try_from(ee_cert.cert.der()).unwrap();

        let expected_chain = |path: &VerifiedPath<'_>| {
            assert_eq!(path.anchor().subject, anchor.subject);
            assert!(public_values_eq(path.end_entity().subject, ee_cert.subject));
            assert_eq!(path.intermediate_certificates().count(), chain_length);

            let intermediate_certs = intermediate_chain
                .chain
                .iter()
                .map(|der| Cert::from_der(untrusted::Input::from(der)).unwrap())
                .collect::<Vec<_>>();

            for (cert, expected) in path
                .intermediate_certificates()
                .rev()
                .zip(intermediate_certs.iter())
            {
                assert!(public_values_eq(cert.subject, expected.subject));
                assert_eq!(cert.der(), expected.der());
            }

            for (cert, expected) in path
                .intermediate_certificates()
                .zip(intermediate_certs.iter().rev())
            {
                assert!(public_values_eq(cert.subject, expected.subject));
                assert_eq!(cert.der(), expected.der());
            }

            Ok(())
        };

        verify_chain(
            anchors,
            &intermediate_chain.chain,
            &ee_cert,
            Some(&expected_chain),
            None,
        )
        .map(|_| ())
    }

    struct IntermediateChain {
        last_issuer: Issuer<'static, KeyPair>,
        chain: Vec<CertificateDer<'static>>,
    }

    impl IntermediateChain {
        fn new(chain_length: usize, all_same_subject: bool, ca_cert: &Issuer<'_, KeyPair>) -> Self {
            let mut chain = Vec::with_capacity(chain_length);

            let mut prev = None;
            for i in 0..chain_length {
                let issuer = match &prev {
                    Some(prev) => prev,
                    None => ca_cert,
                };

                let intermediate = issuer_params(match all_same_subject {
                    true => "Bogus Subject".to_string(),
                    false => format!("Bogus Subject {i}"),
                });

                let key_pair = KeyPair::generate_for(test_utils::RCGEN_SIGNATURE_ALG).unwrap();
                let cert = intermediate.signed_by(&key_pair, issuer).unwrap();

                chain.push(cert.der().clone());
                prev = Some(Issuer::new(intermediate, key_pair));
            }

            Self {
                last_issuer: prev.unwrap(),
                chain,
            }
        }
    }

    fn verify_chain<'a>(
        trust_anchors: &'a [TrustAnchor<'a>],
        intermediate_certs: &'a [CertificateDer<'a>],
        ee_cert: &'a EndEntityCert<'a>,
        verify_path: Option<&dyn Fn(&VerifiedPath<'_>) -> Result<(), Error>>,
        budget: Option<Budget>,
    ) -> Result<VerifiedPath<'a>, ControlFlow<Error, Error>> {
        use core::time::Duration;

        let time = UnixTime::since_unix_epoch(Duration::from_secs(0x1fed_f00d));
        let mut path = PartialPath::new(ee_cert);
        let opts = ChainOptions {
            eku: KeyUsage::server_auth(),
            supported_sig_algs: crate::ALL_VERIFICATION_ALGS,
            trust_anchors,
            intermediate_certs,
            revocation: None,
            distrusted_spkis: DISTRUSTED_SPKIS,
        };

        match opts.build_chain_inner(
            &mut path,
            time,
            verify_path,
            0,
            &mut budget.unwrap_or_default(),
        ) {
            Ok(anchor) => Ok(VerifiedPath::new(ee_cert, anchor, path)),
            Err(err) => Err(err),
        }
    }
}
