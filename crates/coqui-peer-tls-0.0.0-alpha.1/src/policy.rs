use alloc::sync::Arc;
use core::fmt;
use core::str::FromStr;

use rustls::crypto::CryptoProvider;

use crate::error::PeerError;
use crate::profiles::{CipherProfile, Crypto};
use coqui_peer::{Digest, NonDefault, PeerId, WaitingLimits};

/// A TLS protocol version. rustls offers 1.2 and 1.3 only: SSL 3, TLS 1.0 and 1.1 cannot be
/// offered by this crate, and asking for them is an error, never ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum TlsVersion {
    /// TLS 1.2, text `"1.2"`.
    V1_2,
    /// TLS 1.3, text `"1.3"`.
    V1_3,
}

impl TlsVersion {
    /// The text form: `1.2` or `1.3`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1_2 => "1.2",
            Self::V1_3 => "1.3",
        }
    }
}

impl fmt::Display for TlsVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TlsVersion {
    type Err = PeerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "1.2" => Ok(Self::V1_2),
            "1.3" => Ok(Self::V1_3),
            "1.0" | "1.1" | "ssl3" | "SSL3" => Err(PeerError::Policy(
                "TLS before 1.2 is not supported: rustls offers only TLS 1.2 and 1.3".to_owned(),
            )),
            _ => Err(PeerError::Policy(
                "unknown TLS version; supported: \"1.2\", \"1.3\"".to_owned(),
            )),
        }
    }
}

/// What a TLS configuration and a trust decision are allowed to do.
///
/// Start from [`Policy::modern`], [`Policy::intermediate`], [`Policy::legacy`] or a policy of
/// your own, then change any field. [`Default`] is intermediate (TLS 1.2 and 1.3). Nothing is
/// silently non-default: [`non_defaults`](Policy::non_defaults) lists every setting that differs from
/// the default, and the configuration builders return that list.
///
/// A policy is a guatiao value with a schema whose `default`s are the intermediate preset's: a
/// document names only what differs, and an unknown key is refused. With the `serde` feature it
/// also reads and writes through any serde format.
///
/// The struct is `#[non_exhaustive]`: start from a preset and assign the fields to change.
#[derive(Debug, Clone, PartialEq, Eq, guatiao::ToValue, guatiao::FromValue, guatiao::Schema)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct Policy {
    /// TLS versions offered and accepted. Non-empty.
    #[map(default = base().tls_versions, with = crate::settings::texts)]
    pub tls_versions: Vec<TlsVersion>,
    /// An allow-list of cipher suites by IANA name, for example `TLS13_AES_256_GCM_SHA384`.
    /// `None` allows every suite the cipher profiles give for [`tls_versions`](Self::tls_versions).
    /// An unknown name is an error that lists the supported ones.
    pub cipher_suites: Option<Vec<String>>,
    /// An allow-list of key-exchange groups by name, for example `X25519`; `None` allows all
    /// the cipher profiles give.
    pub kx_groups: Option<Vec<String>>,
    /// The algorithm profiles allowed; what is offered is their union, in the order of the
    /// first listed, narrowed by the versions, the provider, [`cipher_suites`](Self::cipher_suites)
    /// and [`kx_groups`](Self::kx_groups). Every preset allows `[CipherProfile::Mozilla]`. Text
    /// key `cipher_profiles`.
    #[map(default = base().cipher_profiles, with = crate::settings::texts)]
    pub cipher_profiles: Vec<CipherProfile>,
    /// The crypto provider; `Crypto::Ring` in every preset. Text key `crypto`.
    #[map(default = base().crypto, as_text)]
    pub crypto: Crypto,
    /// Digests a pin may use. Pins are compared in constant time whatever their digest.
    #[map(default = base().pin_digests)]
    pub pin_digests: Vec<Digest>,
    /// Whether a pinned certificate must be inside its validity dates.
    #[map(default = base().check_validity)]
    pub check_validity: bool,
    /// Whether a pinned peer may present a chain; strict refuses one.
    #[map(default = base().allow_chain_when_pinned)]
    pub allow_chain_when_pinned: bool,
    /// Whether TLS sessions may be resumed. A resumed handshake skips the certificate
    /// verifiers, so a peer removed from the trust store or no longer pinned can reconnect
    /// on a session it already holds, and the access control list's fingerprint rules are not
    /// checked again (its address rules are). Every preset leaves it off; `non_defaults` reports it.
    #[map(default = base().allow_resumption)]
    pub allow_resumption: bool,
    /// Whether a TLS 1.2 connection must use the extended master secret (RFC 7627). Without it
    /// a TLS 1.2 session's keys and exporter are not bound to its handshake, which an attacker
    /// can use to synchronise two sessions (the triple handshake). `modern` and `intermediate`
    /// set it; `legacy` does not, for TLS 1.2 stacks older than RFC 7627. TLS 1.3 needs nothing
    /// like it.
    #[map(default = base().require_ems)]
    pub require_ems: bool,
    /// The bounds of a store-backed pin set's waiting queue: entries per source, entries in
    /// all, and how long before the same certificate from the same source is recorded again.
    /// In the text form a table: `per_source`, `total` and `rerecord` (a duration, a number of
    /// seconds in a hand-written document).
    #[map(default = base().waiting)]
    pub waiting: WaitingLimits,
    /// An ordered access control list for connecting clients, applied by every server
    /// configuration built under this policy: by address when the `ClientHello` arrives
    /// ([`Refusal::AddressDenied`](crate::Refusal::AddressDenied)) and by the client
    /// certificate's fingerprint before the trust store is consulted
    /// ([`Refusal::PeerDenied`](crate::Refusal::PeerDenied)). Its fingerprints may be written
    /// in any spelling a [`PeerId`] parses, and are made canonical where a configuration is
    /// built or a policy is read through [`from_value_over`](Policy::from_value_over) or serde; a
    /// rule naming something else, or a digest other than SHA-256, is an error there and in
    /// [`check`](Policy::check). `None` for none.
    #[cfg(feature = "acl")]
    pub acl: Option<coqui_net_acl::Acl>,
}

impl Default for Policy {
    fn default() -> Self {
        Self::intermediate()
    }
}

impl Policy {
    /// The intermediate preset, the default: TLS 1.2 and 1.3 (TLS 1.2 with ECDHE, AEAD suites
    /// and the extended master secret); pins SHA-256, SHA-384 or SHA-512; validity dates
    /// checked; no chain on a pinned path; no session resumption; waiting queues capped.
    ///
    /// Two peers that both allow TLS 1.3 negotiate it, and the TLS 1.3 downgrade protection
    /// keeps an attacker from forcing 1.2 between them; allowing 1.2 only admits peers that
    /// cannot speak 1.3.
    pub fn intermediate() -> Self {
        Self {
            tls_versions: vec![TlsVersion::V1_2, TlsVersion::V1_3],
            cipher_suites: None,
            kx_groups: None,
            cipher_profiles: vec![CipherProfile::Mozilla],
            crypto: Crypto::Ring,
            pin_digests: vec![Digest::Sha256, Digest::Sha384, Digest::Sha512],
            check_validity: true,
            allow_chain_when_pinned: false,
            allow_resumption: false,
            require_ems: true,
            waiting: WaitingLimits::default(),
            #[cfg(feature = "acl")]
            acl: None,
        }
    }

    /// The modern preset: intermediate with TLS 1.3 only. A choice, never required: it
    /// refuses peers that speak only TLS 1.2, and keeps client certificates from passive
    /// observers when such peers would otherwise be served (TLS 1.2 sends the client's
    /// certificate unencrypted).
    pub fn modern() -> Self {
        Self {
            tls_versions: vec![TlsVersion::V1_3],
            ..Self::intermediate()
        }
    }

    /// The legacy preset: intermediate, and SHA-1 pins and TLS 1.2 without the extended master
    /// secret accepted, for interoperability with old peers only. A peer that cannot take a
    /// TLS 1.3 `ClientHello` at all is served by setting `tls_versions` to TLS 1.2 alone.
    pub fn legacy() -> Self {
        let mut policy = Self::intermediate();
        policy.pin_digests.push(Digest::Sha1);
        policy.require_ems = false;
        policy
    }

    /// The built-in preset called `name`: `modern`, `intermediate` or `legacy`.
    ///
    /// # Errors
    ///
    /// [`PeerError::Policy`] for any other name.
    pub fn preset(name: &str) -> Result<Self, PeerError> {
        match name.trim() {
            "modern" => Ok(Self::modern()),
            "intermediate" => Ok(Self::intermediate()),
            "legacy" => Ok(Self::legacy()),
            _ => Err(PeerError::Policy(
                "unknown profile; built in: modern, intermediate, legacy".to_owned(),
            )),
        }
    }
}

fn list<T: fmt::Display>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

impl Policy {
    /// Every setting that differs from the default (intermediate) preset, as plain data named by
    /// its key (`waiting.per_source`, `tls_versions`): the places where this policy's guatiao
    /// value departs from the schema's defaults. Empty for `intermediate`; `modern` reports
    /// `tls_versions` and `legacy` reports `pin_digests` and `require_ems`. Values are rendered
    /// as [`NonDefault::from_difference`] renders them. With the `acl` feature, the list's own
    /// non-default settings are added (`acl.default`).
    pub fn non_defaults(&self) -> Vec<NonDefault> {
        self.differences(None)
    }

    /// As [`non_defaults`](Self::non_defaults), against `baseline` instead of the defaults, for a
    /// consumer whose own baseline differs from them (`non_defaults_against(&Policy::modern())`
    /// reports TLS 1.2).
    pub fn non_defaults_against(&self, baseline: &Self) -> Vec<NonDefault> {
        self.differences(Some(baseline))
    }

    /// The generic walk: the schema, this policy's value and the baseline's (the schema's
    /// defaults when `None`). Both are values of a derived type, so the walk cannot fail unless
    /// the allocator does.
    fn differences(&self, baseline: Option<&Self>) -> Vec<NonDefault> {
        use guatiao::{Schema, ToValue};
        const PLACEHOLDER: &str = "***";
        let unbuildable = "a policy is a value that builds";
        let schema = Policy::schema().expect(unbuildable);
        let schema = guatiao::schema::SchemaRef::new(&schema).expect(unbuildable);
        let value = self.to_value().expect(unbuildable);
        let baseline = baseline.map(|baseline| baseline.to_value().expect(unbuildable));
        #[cfg_attr(not(feature = "acl"), expect(unused_mut))]
        let mut found: Vec<NonDefault> =
            guatiao::schema::differences(schema, &value, baseline.as_ref(), PLACEHOLDER)
                .expect(unbuildable)
                .iter()
                // The list is coqui-net-acl's to report, below.
                .filter(|difference| difference.path.first().is_none_or(|key| key != "acl"))
                .map(NonDefault::from_difference)
                .collect();
        #[cfg(feature = "acl")]
        if let Some(acl) = &self.acl {
            // coqui-net-acl has no dependencies and keeps a NonDefault of its own, field for field.
            found.extend(
                acl.non_defaults()
                    .into_iter()
                    .map(|w| NonDefault::new(w.setting, w.value, w.default)),
            );
        }
        found
    }

    /// Whether the access control list admits, by address, a client from `source` to `local`
    /// that will present a certificate: a rule naming a fingerprint defers to the handshake.
    /// Without a list, every address is admitted. For a host that refuses a connection before
    /// its handshake, such as a quinn `Incoming`.
    ///
    /// # Errors
    ///
    /// [`Refusal::AddressDenied`](crate::Refusal::AddressDenied) with the deciding rule.
    #[cfg(feature = "acl")]
    pub fn admits_address(
        &self,
        source: core::net::SocketAddr,
        local: core::net::SocketAddr,
    ) -> Result<(), crate::Refusal> {
        match &self.acl {
            Some(acl) => crate::gate::decide_address(acl, source, local, true),
            None => Ok(()),
        }
    }

    /// [`admits_address`](Policy::admits_address) for a client that proves no identity at the
    /// TLS layer: rules naming a fingerprint cannot match it.
    ///
    /// # Errors
    ///
    /// [`Refusal::AddressDenied`](crate::Refusal::AddressDenied) with the deciding rule.
    #[cfg(feature = "acl")]
    pub fn admits_anonymous(
        &self,
        source: core::net::SocketAddr,
        local: core::net::SocketAddr,
    ) -> Result<(), crate::Refusal> {
        match &self.acl {
            Some(acl) => crate::gate::decide_address(acl, source, local, false),
            None => Ok(()),
        }
    }

    /// Whether this policy lets a pin use the digest of `fingerprint`.
    ///
    /// # Errors
    ///
    /// [`PeerError::Policy`] naming the digest and the allowed ones.
    pub fn check_pin(&self, fingerprint: &PeerId) -> Result<(), PeerError> {
        if self.pin_digests.contains(&fingerprint.digest()) {
            return Ok(());
        }
        Err(PeerError::Policy(format!(
            "a {} pin is not allowed by this policy; allowed digests: {}",
            fingerprint.digest(),
            list(&self.pin_digests)
        )))
    }

    /// Checks this policy as building a configuration under it does, without building one:
    /// its versions, cipher profiles, cipher suites and key-exchange groups against the crypto
    /// provider and this build, and, with the `acl` feature, that every fingerprint of its
    /// access control list is a SHA-256 peer id. A host that builds its configurations later
    /// (one per connection) checks the policy it read with this.
    ///
    /// # Errors
    ///
    /// [`PeerError::Policy`] naming the first setting that fails, as a configuration builder
    /// would report it.
    pub fn check(&self) -> Result<(), PeerError> {
        crate::profiles::select(self)?;
        #[cfg(feature = "acl")]
        if let Some(acl) = &self.acl {
            crate::gate::canonical(acl)?;
        }
        Ok(())
    }

    /// The provider, versions and FIPS requirement to build a configuration with.
    pub(crate) fn selection(&self) -> Result<crate::profiles::Selection, PeerError> {
        crate::profiles::select(self)
    }

    /// The crypto provider ([`crypto`](Self::crypto)) narrowed to what this policy offers: the
    /// union of its [`cipher_profiles`](Self::cipher_profiles), its versions, and its
    /// [`cipher_suites`](Self::cipher_suites) and [`kx_groups`](Self::kx_groups) when set.
    ///
    /// # Errors
    ///
    /// [`PeerError::Policy`] for an unknown suite or group name (listing the supported ones), a
    /// provider or profile this build lacks, or when no suite or no group is left.
    pub fn crypto_provider(&self) -> Result<Arc<CryptoProvider>, PeerError> {
        Ok(self.selection()?.provider)
    }

    /// The cipher suite IANA names the provider this policy selects can offer, for choosing
    /// [`cipher_suites`](Self::cipher_suites) from.
    ///
    /// # Errors
    ///
    /// [`PeerError::Policy`] when this build lacks that provider.
    pub fn supported_cipher_suites(&self) -> Result<Vec<String>, PeerError> {
        Ok(crate::profiles::supported(self)?.0)
    }

    /// The key-exchange group names the provider this policy selects can offer, for choosing
    /// [`kx_groups`](Self::kx_groups) from.
    ///
    /// # Errors
    ///
    /// [`PeerError::Policy`] when this build lacks that provider.
    pub fn supported_kx_groups(&self) -> Result<Vec<String>, PeerError> {
        Ok(crate::profiles::supported(self)?.1)
    }
}

/// The preset every field of a document defaults to.
fn base() -> Policy {
    Policy::intermediate()
}

impl Policy {
    /// Reads a policy document over `base` instead of the intermediate preset: the keys it
    /// names replace the base's (a table such as `waiting` key by key, a list whole), and the
    /// rest are kept. A consumer with its own default profile reads its configuration with
    /// this.
    ///
    /// # Errors
    ///
    /// The reader's refusal of an unknown key or a value of the wrong type or form, and, with the
    /// `acl` feature, of a rule whose fingerprint is not a SHA-256 peer id; the list's
    /// fingerprints are made canonical.
    pub fn from_value_over(
        base: &Policy,
        document: &guatiao::Value,
    ) -> Result<Policy, guatiao::MapError> {
        use guatiao::{FromValue, MergeMode, ToValue};
        let unreadable = |error: &dyn fmt::Display| guatiao::MapError::BadValue {
            key: String::new(),
            expected: error.to_string(),
        };
        let base = base.to_value().map_err(|error| unreadable(&error))?;
        let merged = MergeMode::Substitute
            .merge(&base, document, guatiao::Alloc::rust())
            .map_err(|error| unreadable(&error))?;
        #[cfg_attr(not(feature = "acl"), expect(unused_mut))]
        let mut policy = Policy::from_value(&merged)?;
        #[cfg(feature = "acl")]
        if let Some(acl) = &policy.acl {
            policy.acl = Some(crate::gate::canonical(acl).map_err(|error| unreadable(&error))?);
        }
        Ok(policy)
    }
}

#[cfg(feature = "serde")]
mod text {
    use guatiao_serde::{Presentation, Walked};
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::Policy;

    impl Policy {
        /// Reads a policy document over `base` instead of the intermediate preset: the keys it
        /// names replace the base's, and the rest are kept.
        ///
        /// # Errors
        ///
        /// The deserializer's error for an unknown key or a value of the wrong type or form.
        pub fn deserialize_over<'de, D: Deserializer<'de>>(
            base: Policy,
            deserializer: D,
        ) -> Result<Policy, D::Error> {
            use serde::de::DeserializeSeed;
            let document =
                guatiao_serde::ValueSeed::new(guatiao::Alloc::rust()).deserialize(deserializer)?;
            Policy::from_value_over(&base, &document).map_err(D::Error::custom)
        }
    }

    impl Serialize for Policy {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Walked::new(self, Presentation::new()).serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Policy {
        /// Reads the guatiao form over the intermediate preset.
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            use serde::de::DeserializeSeed;
            let document =
                guatiao_serde::ValueSeed::new(guatiao::Alloc::rust()).deserialize(deserializer)?;
            Policy::from_value_over(&super::base(), &document).map_err(D::Error::custom)
        }
    }
}
