//! The signature verification tables of the cipher profiles. rustls takes `&'static` slices,
//! so each combination a set of profiles can need is a constant here.

use rustls::crypto::WebPkiSupportedAlgorithms;

use super::{Crypto, Signatures};
use crate::error::PeerError;

/// The table for `signatures` (plus ML-DSA-87 when `ml_dsa_87`) on `backend`; `all` is the
/// provider's own, every scheme it verifies.
pub(super) fn table(
    backend: Crypto,
    signatures: Signatures,
    ml_dsa_87: bool,
    all: WebPkiSupportedAlgorithms,
) -> Result<WebPkiSupportedAlgorithms, PeerError> {
    match (backend, signatures, ml_dsa_87) {
        (_, Signatures::All, _) => Ok(all),
        (Crypto::Ring, _, true) => Err(PeerError::Policy(
            "ML-DSA needs crypto = aws-lc: ring has no ML-DSA".to_owned(),
        )),
        (Crypto::Ring, Signatures::Fips, false) => Ok(ring_tables::FIPS),
        (Crypto::Ring, Signatures::Cnsa1 | Signatures::None, false) => Ok(ring_tables::CNSA_1),
        #[cfg(feature = "aws-lc")]
        (Crypto::AwsLc, Signatures::Fips, false) => Ok(aws_lc_tables::FIPS),
        #[cfg(feature = "aws-lc")]
        (Crypto::AwsLc, Signatures::Cnsa1, false) => Ok(aws_lc_tables::CNSA_1),
        #[cfg(feature = "aws-lc")]
        (Crypto::AwsLc, Signatures::None, _) => Ok(aws_lc_tables::ML_DSA_87),
        #[cfg(feature = "aws-lc")]
        (Crypto::AwsLc, Signatures::Fips, true) => Ok(aws_lc_tables::FIPS_ML_DSA_87),
        #[cfg(feature = "aws-lc")]
        (Crypto::AwsLc, Signatures::Cnsa1, true) => Ok(aws_lc_tables::CNSA_1_ML_DSA_87),
        #[cfg(not(feature = "aws-lc"))]
        (Crypto::AwsLc, _, _) => Err(PeerError::Policy(
            "crypto = \"aws-lc\" needs this build's aws-lc feature".to_owned(),
        )),
    }
}

mod ring_tables {
    use rustls::SignatureScheme;
    use rustls::crypto::WebPkiSupportedAlgorithms;
    use webpki::ring as alg;

    /// The FIPS profiles' signatures: RSA-PSS, RSA PKCS#1 and ECDSA P-256/P-384.
    pub(super) const FIPS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
        all: &[
            alg::ECDSA_P256_SHA256,
            alg::ECDSA_P256_SHA384,
            alg::ECDSA_P384_SHA256,
            alg::ECDSA_P384_SHA384,
            alg::RSA_PSS_2048_8192_SHA256_LEGACY_KEY,
            alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY,
            alg::RSA_PSS_2048_8192_SHA512_LEGACY_KEY,
            alg::RSA_PKCS1_2048_8192_SHA256,
            alg::RSA_PKCS1_2048_8192_SHA384,
            alg::RSA_PKCS1_2048_8192_SHA512,
            alg::RSA_PKCS1_2048_8192_SHA256_ABSENT_PARAMS,
            alg::RSA_PKCS1_2048_8192_SHA384_ABSENT_PARAMS,
            alg::RSA_PKCS1_2048_8192_SHA512_ABSENT_PARAMS,
        ],
        mapping: &[
            (
                SignatureScheme::ECDSA_NISTP384_SHA384,
                &[alg::ECDSA_P384_SHA384, alg::ECDSA_P256_SHA384],
            ),
            (
                SignatureScheme::ECDSA_NISTP256_SHA256,
                &[alg::ECDSA_P256_SHA256, alg::ECDSA_P384_SHA256],
            ),
            (
                SignatureScheme::RSA_PSS_SHA512,
                &[alg::RSA_PSS_2048_8192_SHA512_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PSS_SHA384,
                &[alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PSS_SHA256,
                &[alg::RSA_PSS_2048_8192_SHA256_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA512,
                &[alg::RSA_PKCS1_2048_8192_SHA512],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA384,
                &[alg::RSA_PKCS1_2048_8192_SHA384],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA256,
                &[alg::RSA_PKCS1_2048_8192_SHA256],
            ),
        ],
    };

    /// CNSA 1.0's signatures: ECDSA P-384 and RSA with SHA-384.
    pub(super) const CNSA_1: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
        all: &[
            alg::ECDSA_P384_SHA384,
            alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY,
            alg::RSA_PKCS1_3072_8192_SHA384,
        ],
        mapping: &[
            (
                SignatureScheme::ECDSA_NISTP384_SHA384,
                &[alg::ECDSA_P384_SHA384],
            ),
            (
                SignatureScheme::RSA_PSS_SHA384,
                &[alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA384,
                &[alg::RSA_PKCS1_3072_8192_SHA384],
            ),
        ],
    };
}

#[cfg(feature = "aws-lc")]
mod aws_lc_tables {
    use rustls::SignatureScheme;
    use rustls::crypto::WebPkiSupportedAlgorithms;
    use webpki::aws_lc_rs as alg;

    /// The FIPS profiles' signatures: RSA-PSS, RSA PKCS#1 and ECDSA P-256/P-384.
    pub(super) const FIPS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
        all: &[
            alg::ECDSA_P256_SHA256,
            alg::ECDSA_P256_SHA384,
            alg::ECDSA_P384_SHA256,
            alg::ECDSA_P384_SHA384,
            alg::RSA_PSS_2048_8192_SHA256_LEGACY_KEY,
            alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY,
            alg::RSA_PSS_2048_8192_SHA512_LEGACY_KEY,
            alg::RSA_PKCS1_2048_8192_SHA256,
            alg::RSA_PKCS1_2048_8192_SHA384,
            alg::RSA_PKCS1_2048_8192_SHA512,
            alg::RSA_PKCS1_2048_8192_SHA256_ABSENT_PARAMS,
            alg::RSA_PKCS1_2048_8192_SHA384_ABSENT_PARAMS,
            alg::RSA_PKCS1_2048_8192_SHA512_ABSENT_PARAMS,
        ],
        mapping: &[
            (
                SignatureScheme::ECDSA_NISTP384_SHA384,
                &[alg::ECDSA_P384_SHA384, alg::ECDSA_P256_SHA384],
            ),
            (
                SignatureScheme::ECDSA_NISTP256_SHA256,
                &[alg::ECDSA_P256_SHA256, alg::ECDSA_P384_SHA256],
            ),
            (
                SignatureScheme::RSA_PSS_SHA512,
                &[alg::RSA_PSS_2048_8192_SHA512_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PSS_SHA384,
                &[alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PSS_SHA256,
                &[alg::RSA_PSS_2048_8192_SHA256_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA512,
                &[alg::RSA_PKCS1_2048_8192_SHA512],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA384,
                &[alg::RSA_PKCS1_2048_8192_SHA384],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA256,
                &[alg::RSA_PKCS1_2048_8192_SHA256],
            ),
        ],
    };

    /// CNSA 1.0's signatures: ECDSA P-384 and RSA with SHA-384.
    pub(super) const CNSA_1: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
        all: &[
            alg::ECDSA_P384_SHA384,
            alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY,
            alg::RSA_PKCS1_3072_8192_SHA384,
        ],
        mapping: &[
            (
                SignatureScheme::ECDSA_NISTP384_SHA384,
                &[alg::ECDSA_P384_SHA384],
            ),
            (
                SignatureScheme::RSA_PSS_SHA384,
                &[alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA384,
                &[alg::RSA_PKCS1_3072_8192_SHA384],
            ),
        ],
    };

    /// CNSA 2.0's signature: ML-DSA-87.
    pub(super) const ML_DSA_87: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
        all: &[alg::ML_DSA_87],
        mapping: &[(SignatureScheme::ML_DSA_87, &[alg::ML_DSA_87])],
    };

    /// The FIPS profiles' signatures and ML-DSA-87.
    pub(super) const FIPS_ML_DSA_87: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
        all: &[
            alg::ECDSA_P256_SHA256,
            alg::ECDSA_P256_SHA384,
            alg::ECDSA_P384_SHA256,
            alg::ECDSA_P384_SHA384,
            alg::RSA_PSS_2048_8192_SHA256_LEGACY_KEY,
            alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY,
            alg::RSA_PSS_2048_8192_SHA512_LEGACY_KEY,
            alg::RSA_PKCS1_2048_8192_SHA256,
            alg::RSA_PKCS1_2048_8192_SHA384,
            alg::RSA_PKCS1_2048_8192_SHA512,
            alg::RSA_PKCS1_2048_8192_SHA256_ABSENT_PARAMS,
            alg::RSA_PKCS1_2048_8192_SHA384_ABSENT_PARAMS,
            alg::RSA_PKCS1_2048_8192_SHA512_ABSENT_PARAMS,
            alg::ML_DSA_87,
        ],
        mapping: &[
            (
                SignatureScheme::ECDSA_NISTP384_SHA384,
                &[alg::ECDSA_P384_SHA384, alg::ECDSA_P256_SHA384],
            ),
            (
                SignatureScheme::ECDSA_NISTP256_SHA256,
                &[alg::ECDSA_P256_SHA256, alg::ECDSA_P384_SHA256],
            ),
            (
                SignatureScheme::RSA_PSS_SHA512,
                &[alg::RSA_PSS_2048_8192_SHA512_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PSS_SHA384,
                &[alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PSS_SHA256,
                &[alg::RSA_PSS_2048_8192_SHA256_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA512,
                &[alg::RSA_PKCS1_2048_8192_SHA512],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA384,
                &[alg::RSA_PKCS1_2048_8192_SHA384],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA256,
                &[alg::RSA_PKCS1_2048_8192_SHA256],
            ),
            (SignatureScheme::ML_DSA_87, &[alg::ML_DSA_87]),
        ],
    };

    /// CNSA 1.0's signatures and ML-DSA-87.
    pub(super) const CNSA_1_ML_DSA_87: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
        all: &[
            alg::ECDSA_P384_SHA384,
            alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY,
            alg::RSA_PKCS1_3072_8192_SHA384,
            alg::ML_DSA_87,
        ],
        mapping: &[
            (
                SignatureScheme::ECDSA_NISTP384_SHA384,
                &[alg::ECDSA_P384_SHA384],
            ),
            (
                SignatureScheme::RSA_PSS_SHA384,
                &[alg::RSA_PSS_2048_8192_SHA384_LEGACY_KEY],
            ),
            (
                SignatureScheme::RSA_PKCS1_SHA384,
                &[alg::RSA_PKCS1_3072_8192_SHA384],
            ),
            (SignatureScheme::ML_DSA_87, &[alg::ML_DSA_87]),
        ],
    };
}
