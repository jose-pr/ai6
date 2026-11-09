//! Cipher profiles and crypto providers: which algorithms a policy allows, apart from its TLS
//! versions, and the provider that runs them.

use alloc::sync::Arc;
use core::fmt;
use core::str::FromStr;

use rustls::crypto::{CryptoProvider, SupportedKxGroup};
use rustls::{CipherSuite, NamedGroup, SupportedCipherSuite, SupportedProtocolVersion};

use crate::error::PeerError;
use crate::policy::{Policy, TlsVersion};

mod tables;

/// A named set of algorithms a [`Policy`] allows: cipher suites, key-exchange groups and
/// signature schemes, each profile exactly what its name claims.
///
/// A policy lists the profiles it allows ([`Policy::cipher_profiles`]); what it offers is their
/// union, in the order of the first listed, narrowed to its TLS versions, to what the crypto
/// provider has, and to [`Policy::cipher_suites`] and [`Policy::kx_groups`] when set. A TLS
/// version left with no suite is dropped; none left is [`PeerError::Policy`]. Profiles are
/// choices, not non-default settings: [`Policy::non_defaults`] does not report them.
///
/// Text form: `mozilla`, `fips-140-3-compliant`, `fips-140-2-compliant`, `fips-140-3-validated`,
/// `cnsa-1.0`, `cnsa-2.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CipherProfile {
    /// `mozilla`: Mozilla's server-side TLS guideline 6.0, intermediate (which holds modern's):
    /// TLS 1.3 AES-128-GCM, AES-256-GCM and ChaCha20-Poly1305; TLS 1.2 ECDHE-ECDSA and
    /// ECDHE-RSA with the same three, in Mozilla's order; X25519MLKEM768 (`aws-lc` only),
    /// X25519, P-256 and P-384; every signature scheme the provider verifies. The default.
    Mozilla,
    /// `fips-140-3-compliant`: the FIPS 140-3 approved algorithms for TLS, per NIST SP 800-52
    /// Rev. 2 and SP 800-131A Rev. 2: TLS 1.3 AES-128-GCM and AES-256-GCM; TLS 1.2 ECDHE-ECDSA
    /// and ECDHE-RSA with AES-GCM; P-256 and P-384, and with `aws-lc` SecP256r1MLKEM768,
    /// X25519MLKEM768, MLKEM768 and MLKEM1024; RSA-PSS and RSA PKCS#1 with SHA-256/384/512,
    /// ECDSA P-256 with SHA-256 and P-384 with SHA-384. No ChaCha20, no X25519 alone, no
    /// Ed25519, no ML-DSA. **Algorithm selection only; not a FIPS 140-3 validated module;
    /// makes no validation claim.** It works on either provider.
    Fips1403Compliant,
    /// `fips-140-2-compliant`: `fips-140-3-compliant` without the ML-KEM groups, which only a
    /// FIPS 140-3 module can have approved. Algorithm selection only, no validation claim. On
    /// `ring` the two lists are the same. Every FIPS 140-2 certificate moved to the CMVP
    /// Historical List in September 2026 (usable for existing systems only); the TLS rules
    /// themselves (SP 800-52r2, SP 800-131A) are the same for both.
    Fips1402Compliant,
    /// `fips-140-3-validated`: the `fips-140-3-compliant` list on rustls's FIPS provider
    /// (AWS-LC's FIPS module, feature `fips`), whatever [`Policy::crypto`] says. Listed alone
    /// or with others, the configuration uses that provider, keeps only what the module
    /// reports approved (so other listed profiles lose ChaCha20 and X25519), and is refused
    /// unless rustls reports it FIPS (`ClientConfig::fips()` / `ServerConfig::fips()`, which
    /// also needs [`Policy::require_ems`]). Validated only as AWS-LC's CMVP certificate and
    /// its tested operating environments state. Without the feature it is
    /// [`PeerError::Policy`]; [`CipherProfile::FIPS_VALIDATED_AVAILABLE`] says which.
    Fips1403Validated,
    /// `cnsa-1.0`: NSA's CNSA 1.0 (CNSSP 15, RFC 9151) as far as rustls goes, on either
    /// provider: TLS 1.3 AES-256-GCM-SHA384; TLS 1.2 ECDHE-ECDSA and ECDHE-RSA
    /// AES-256-GCM-SHA384; secp384r1; ECDSA P-384 with SHA-384, RSA-PSS with SHA-384 and RSA
    /// PKCS#1 with SHA-384 (keys of 3072 bits or more). Not met: the static-RSA and DHE suites
    /// and the ffdhe3072/4096 groups RFC 9151 also lists (absent from rustls, and SP 800-131A
    /// disallows static RSA), and the 3072-bit floor for RSA-PSS keys (2048-bit keys pass).
    /// Needs an identity of P-384 or RSA 3072+: generated identities are P-256.
    Cnsa1,
    /// `cnsa-2.0`: NSA's CNSA 2.0 (ML-KEM-1024, ML-DSA-87, AES-256, SHA-384) in the TLS shape of
    /// draft-becker-cnsa2-tls-profile-05: TLS 1.3 only, AES-256-GCM-SHA384, MLKEM1024 only,
    /// ML-DSA-87 only. Met in full with [`Crypto::AwsLc`]; with `ring`, which has no ML-KEM
    /// or ML-DSA, it is [`PeerError::Policy`]. The ML-KEM-768 hybrids are category 3, not CNSA
    /// 2.0, and are left out; SecP384r1MLKEM1024 is not in rustls and the draft does not use it.
    /// Certificates must be signed with ML-DSA-87 along the chain, so the consumer brings its
    /// own identity and CA. The TLS profile is an individual draft and this list follows it.
    Cnsa2,
}

impl CipherProfile {
    /// Every profile, in the order of the text form's documentation.
    pub const ALL: &'static [CipherProfile] = &[
        Self::Mozilla,
        Self::Fips1403Compliant,
        Self::Fips1402Compliant,
        Self::Fips1403Validated,
        Self::Cnsa1,
        Self::Cnsa2,
    ];

    /// Whether this build has the FIPS module `fips-140-3-validated` needs (feature `fips`).
    pub const FIPS_VALIDATED_AVAILABLE: bool = cfg!(feature = "fips");

    /// The text form, such as `fips-140-3-compliant`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mozilla => "mozilla",
            Self::Fips1403Compliant => "fips-140-3-compliant",
            Self::Fips1402Compliant => "fips-140-2-compliant",
            Self::Fips1403Validated => "fips-140-3-validated",
            Self::Cnsa1 => "cnsa-1.0",
            Self::Cnsa2 => "cnsa-2.0",
        }
    }

    fn spec(self) -> &'static Spec {
        match self {
            Self::Mozilla => &MOZILLA,
            Self::Fips1403Compliant | Self::Fips1403Validated => &FIPS_140_3,
            Self::Fips1402Compliant => &FIPS_140_2,
            Self::Cnsa1 => &CNSA_1,
            Self::Cnsa2 => &CNSA_2,
        }
    }
}

impl fmt::Display for CipherProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CipherProfile {
    type Err = PeerError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|profile| profile.as_str() == text.trim())
            .ok_or_else(|| {
                let known: Vec<&str> = Self::ALL.iter().map(|profile| profile.as_str()).collect();
                PeerError::Policy(format!(
                    "unknown cipher profile; known: {}",
                    known.join(", ")
                ))
            })
    }
}

/// The crypto provider a policy's configurations run on.
///
/// A field of [`Policy`], not a build switch: a feature makes a provider available, and only
/// a policy that asks for it uses it. Text form: `ring`, `aws-lc`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Crypto {
    /// `ring`: rustls's ring provider. The default.
    #[default]
    Ring,
    /// `aws-lc`: rustls's aws-lc-rs provider (feature `aws-lc`), which adds the ML-KEM groups
    /// (X25519MLKEM768, SecP256r1MLKEM768, MLKEM768, MLKEM1024) and ML-DSA signatures. Without
    /// the feature a policy asking for it is [`PeerError::Policy`].
    AwsLc,
}

impl Crypto {
    /// Whether this build has the aws-lc-rs provider (feature `aws-lc`).
    pub const AWS_LC_AVAILABLE: bool = cfg!(feature = "aws-lc");

    /// The text form: `ring` or `aws-lc`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ring => "ring",
            Self::AwsLc => "aws-lc",
        }
    }
}

impl fmt::Display for Crypto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Crypto {
    type Err = PeerError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim() {
            "ring" => Ok(Self::Ring),
            "aws-lc" => Ok(Self::AwsLc),
            _ => Err(PeerError::Policy(
                "unknown crypto provider; known: ring, aws-lc".to_owned(),
            )),
        }
    }
}

/// Which signature table a set of profiles needs.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Signatures {
    /// ML-DSA-87 only (CNSA 2.0); merged as a flag.
    None,
    Cnsa1,
    Fips,
    /// Every scheme the provider verifies.
    All,
}

/// One profile's lists, in preference order.
struct Spec {
    tls13: &'static [CipherSuite],
    tls12: &'static [CipherSuite],
    groups: &'static [NamedGroup],
    signatures: Signatures,
    ml_dsa_87: bool,
}

const MOZILLA: Spec = Spec {
    tls13: &[
        CipherSuite::TLS13_AES_128_GCM_SHA256,
        CipherSuite::TLS13_AES_256_GCM_SHA384,
        CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
    ],
    tls12: &[
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
    ],
    groups: &[
        NamedGroup::X25519MLKEM768,
        NamedGroup::X25519,
        NamedGroup::secp256r1,
        NamedGroup::secp384r1,
    ],
    signatures: Signatures::All,
    ml_dsa_87: false,
};

const FIPS_140_3: Spec = Spec {
    tls13: &[
        CipherSuite::TLS13_AES_128_GCM_SHA256,
        CipherSuite::TLS13_AES_256_GCM_SHA384,
    ],
    tls12: &[
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    ],
    groups: &[
        NamedGroup::secp256r1MLKEM768,
        NamedGroup::X25519MLKEM768,
        NamedGroup::secp256r1,
        NamedGroup::secp384r1,
        NamedGroup::MLKEM768,
        NamedGroup::MLKEM1024,
    ],
    signatures: Signatures::Fips,
    ml_dsa_87: false,
};

const FIPS_140_2: Spec = Spec {
    groups: &[NamedGroup::secp256r1, NamedGroup::secp384r1],
    ..FIPS_140_3
};

const CNSA_1: Spec = Spec {
    tls13: &[CipherSuite::TLS13_AES_256_GCM_SHA384],
    tls12: &[
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    ],
    groups: &[NamedGroup::secp384r1],
    signatures: Signatures::Cnsa1,
    ml_dsa_87: false,
};

const CNSA_2: Spec = Spec {
    tls13: &[CipherSuite::TLS13_AES_256_GCM_SHA384],
    tls12: &[],
    groups: &[NamedGroup::MLKEM1024],
    signatures: Signatures::None,
    ml_dsa_87: true,
};

/// The provider a policy's configurations are built with, and the TLS versions left.
pub(crate) struct Selection {
    pub(crate) provider: Arc<CryptoProvider>,
    pub(crate) versions: Vec<&'static SupportedProtocolVersion>,
    /// Whether a configuration must be one rustls reports FIPS (`fips-140-3-validated`).
    pub(crate) validated: bool,
}

/// What a provider can offer: its every suite and group, and the provider to narrow.
struct Pool {
    provider: CryptoProvider,
    suites: &'static [SupportedCipherSuite],
    groups: &'static [&'static dyn SupportedKxGroup],
    backend: Crypto,
}

impl Pool {
    fn of(policy: &Policy, validated: bool) -> Result<Self, PeerError> {
        if validated {
            return Self::fips();
        }
        match policy.crypto {
            Crypto::Ring => Ok(Self {
                provider: rustls::crypto::ring::default_provider(),
                suites: rustls::crypto::ring::ALL_CIPHER_SUITES,
                groups: rustls::crypto::ring::ALL_KX_GROUPS,
                backend: Crypto::Ring,
            }),
            #[cfg(feature = "aws-lc")]
            Crypto::AwsLc => Ok(Self {
                provider: rustls::crypto::aws_lc_rs::default_provider(),
                suites: rustls::crypto::aws_lc_rs::ALL_CIPHER_SUITES,
                groups: rustls::crypto::aws_lc_rs::ALL_KX_GROUPS,
                backend: Crypto::AwsLc,
            }),
            #[cfg(not(feature = "aws-lc"))]
            Crypto::AwsLc => Err(PeerError::Policy(
                "crypto = \"aws-lc\" needs this build's aws-lc feature".to_owned(),
            )),
        }
    }

    #[cfg(feature = "fips")]
    fn fips() -> Result<Self, PeerError> {
        Ok(Self {
            provider: rustls::crypto::default_fips_provider(),
            suites: rustls::crypto::aws_lc_rs::ALL_CIPHER_SUITES,
            groups: rustls::crypto::aws_lc_rs::ALL_KX_GROUPS,
            backend: Crypto::AwsLc,
        })
    }

    #[cfg(not(feature = "fips"))]
    fn fips() -> Result<Self, PeerError> {
        Err(PeerError::Policy(
            "this build has no FIPS module: feature fips".to_owned(),
        ))
    }
}

/// The text name of a suite, its IANA name where rustls has one.
pub(crate) fn suite_name(suite: &SupportedCipherSuite) -> String {
    suite
        .suite()
        .as_str()
        .map_or_else(|| format!("{:?}", suite.suite()), str::to_owned)
}

/// The text name of a key-exchange group, as rustls spells it (`X25519`, `secp256r1`).
pub(crate) fn group_name(group: &&'static dyn SupportedKxGroup) -> String {
    format!("{:?}", group.name())
}

fn version_of(suite: &SupportedCipherSuite) -> TlsVersion {
    match suite {
        SupportedCipherSuite::Tls13(_) => TlsVersion::V1_3,
        SupportedCipherSuite::Tls12(_) => TlsVersion::V1_2,
    }
}

/// The names `pool` knows, for an error listing them.
fn known<T>(items: &[T], name: impl Fn(&T) -> String) -> String {
    items.iter().map(name).collect::<Vec<_>>().join(", ")
}

/// The cipher suite and key-exchange group names the provider `policy` selects can offer.
pub(crate) fn supported(policy: &Policy) -> Result<(Vec<String>, Vec<String>), PeerError> {
    let validated = policy
        .cipher_profiles
        .contains(&CipherProfile::Fips1403Validated);
    let pool = Pool::of(policy, validated)?;
    Ok((
        pool.suites.iter().map(suite_name).collect(),
        pool.groups.iter().map(group_name).collect(),
    ))
}

/// The provider, versions and FIPS requirement `policy` builds its configurations with.
pub(crate) fn select(policy: &Policy) -> Result<Selection, PeerError> {
    if policy.tls_versions.is_empty() {
        return Err(PeerError::Policy("tls_versions is empty".to_owned()));
    }
    if policy.cipher_profiles.is_empty() {
        return Err(PeerError::Policy("cipher_profiles is empty".to_owned()));
    }
    let validated = policy
        .cipher_profiles
        .contains(&CipherProfile::Fips1403Validated);
    let pool = Pool::of(policy, validated)?;
    if pool.backend == Crypto::Ring && policy.cipher_profiles.contains(&CipherProfile::Cnsa2) {
        return Err(PeerError::Policy(
            "cnsa-2.0 needs crypto = aws-lc: ring has no ML-KEM or ML-DSA".to_owned(),
        ));
    }
    if let Some(wanted) = &policy.cipher_suites {
        let names: Vec<String> = pool.suites.iter().map(suite_name).collect();
        if let Some(position) = wanted.iter().position(|name| !names.contains(name)) {
            return Err(PeerError::Policy(format!(
                "cipher_suites[{position}] is not a supported cipher suite; supported: {}",
                known(pool.suites, suite_name)
            )));
        }
    }
    if let Some(wanted) = &policy.kx_groups {
        let names: Vec<String> = pool.groups.iter().map(group_name).collect();
        if let Some(position) = wanted.iter().position(|name| !names.contains(name)) {
            return Err(PeerError::Policy(format!(
                "kx_groups[{position}] is not a supported key-exchange group; supported: {}",
                known(pool.groups, group_name)
            )));
        }
    }
    let specs: Vec<&Spec> = policy
        .cipher_profiles
        .iter()
        .map(|profile| profile.spec())
        .collect();

    let mut suites: Vec<SupportedCipherSuite> = Vec::new();
    for id in specs
        .iter()
        .flat_map(|spec| spec.tls13.iter().chain(spec.tls12))
    {
        let Some(suite) = pool.suites.iter().find(|suite| suite.suite() == *id) else {
            continue;
        };
        let allowed = policy.tls_versions.contains(&version_of(suite))
            && (!validated || suite.fips())
            && policy
                .cipher_suites
                .as_ref()
                .is_none_or(|wanted| wanted.contains(&suite_name(suite)));
        if allowed && !suites.iter().any(|had| had.suite() == *id) {
            suites.push(*suite);
        }
    }
    let mut groups: Vec<&'static dyn SupportedKxGroup> = Vec::new();
    for name in specs.iter().flat_map(|spec| spec.groups) {
        let Some(group) = pool.groups.iter().find(|group| group.name() == *name) else {
            continue;
        };
        let allowed = (!validated || group.fips())
            && policy
                .kx_groups
                .as_ref()
                .is_none_or(|wanted| wanted.contains(&group_name(group)));
        if allowed && !groups.iter().any(|had| had.name() == *name) {
            groups.push(*group);
        }
    }
    // A version the allowed profiles give no suite is dropped.
    let versions: Vec<&'static SupportedProtocolVersion> = policy
        .tls_versions
        .iter()
        .filter(|version| suites.iter().any(|suite| version_of(suite) == **version))
        .map(|version| match version {
            TlsVersion::V1_3 => &rustls::version::TLS13,
            TlsVersion::V1_2 => &rustls::version::TLS12,
        })
        .collect();
    if versions.is_empty() {
        return Err(PeerError::Policy(
            "no cipher suite is left for any allowed TLS version".to_owned(),
        ));
    }
    if groups.is_empty() {
        return Err(PeerError::Policy(
            "no key-exchange group is allowed".to_owned(),
        ));
    }
    let signatures = specs
        .iter()
        .map(|spec| spec.signatures)
        .max()
        .unwrap_or(Signatures::None);
    let ml_dsa_87 = specs.iter().any(|spec| spec.ml_dsa_87);
    let algorithms = tables::table(
        pool.backend,
        signatures,
        ml_dsa_87,
        pool.provider.signature_verification_algorithms,
    )?;
    Ok(Selection {
        provider: Arc::new(CryptoProvider {
            cipher_suites: suites,
            kx_groups: groups,
            signature_verification_algorithms: algorithms,
            ..pool.provider
        }),
        versions,
        validated,
    })
}

/// [`PeerError::Policy`] unless a configuration that must be FIPS is (`fips` is rustls's
/// `ClientConfig::fips()` or `ServerConfig::fips()`).
pub(crate) fn require_fips(selection: &Selection, fips: bool) -> Result<(), PeerError> {
    if selection.validated && !fips {
        return Err(PeerError::Policy(
            "fips-140-3-validated: rustls does not report this configuration FIPS (an \
             unapproved algorithm, or require_ems is off)"
                .to_owned(),
        ));
    }
    Ok(())
}
