use alloc::sync::Arc;
use core::fmt;
use core::net::SocketAddr;

use rustls::client::Resumption;
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::CryptoProvider;
use rustls::server::danger::ClientCertVerifier;
use rustls::server::{
    NoServerSessionStorage, ProducesTickets, ResolvesServerCert, StoresServerSessions,
};
use rustls::{ClientConfig, ServerConfig, SupportedProtocolVersion};

use crate::connection::{Client, Keeping, ServerConfigs, Slot};
use crate::error::PeerError;
use crate::identity::Identity;
use crate::policy::Policy;
use crate::settings::TlsSettings;
use crate::trust::{PeerServerVerifier, Trust};
use coqui_builder::builder::BuildError;
use coqui_builder::{Builder, Configured};

/// rustls's ring provider with its defaults. The configurations built here take the provider a
/// [`Policy`] selects ([`Policy::crypto_provider`]); nothing in this crate installs a
/// process-wide default.
pub fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A built configuration and the [`Policy`] it was built under, so a host can surface the
/// settings that differ from the default policy's with [`non_defaults`](Self::non_defaults).
/// Nothing is printed by this crate.
#[derive(Debug, Configured)]
#[non_exhaustive]
pub struct Built<T> {
    /// The configuration.
    pub config: T,
    /// The policy the configuration was built under. Its settings that differ from the default
    /// policy's are `non_defaults()`, named as [`Policy::non_defaults`] names them
    /// (`tls_versions`); empty under the default policy.
    #[configured]
    pub policy: Policy,
}

/// How a server treats the client's certificate.
///
/// A value the [`ServerBuilder`] takes, so no default hides the choice.
#[derive(Clone)]
#[non_exhaustive]
pub enum ClientAuth {
    /// Ask for no client certificate: the server proves itself, the client does not at the TLS
    /// layer.
    ///
    /// **The application must authenticate the client inside the encrypted channel** (a key
    /// proof, a token) before trusting it. A handshake with this configuration finishes with no
    /// peer certificate: see
    /// [`TlsHandshake::finish_unauthenticated`](crate::TlsHandshake::finish_unauthenticated).
    None,
    /// Require a client certificate this trust accepts.
    Required(Trust),
    /// Verify clients with a verifier of the host's own (one of this crate's wrapped, for
    /// example).
    ///
    /// A verifier whose client authentication is optional
    /// (`ClientCertVerifier::client_auth_mandatory` is false) admits a client without a
    /// certificate; under an access control list such a client is decided by address alone, as
    /// one that proves no identity, so a rule naming a fingerprint never admits it.
    Verifier(Arc<dyn ClientCertVerifier>),
}

impl fmt::Debug for ClientAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Required(trust) => formatter.debug_tuple("Required").field(trust).finish(),
            Self::Verifier(_) => formatter.write_str("Verifier(..)"),
        }
    }
}

/// How a client verifies the server.
///
/// The value [`ClientBuilder::new`] takes: a [`Trust`] converts into it, so no default hides the
/// choice.
#[derive(Clone)]
#[non_exhaustive]
pub enum ServerAuth {
    /// Verify the server by this trust: pinned, by [`Roots`](crate::Roots) for the name dialled
    /// ([`dial_named`](crate::dial_named)), pinned or roots, or any certificate with key proof.
    Trust(Trust),
    /// Verify the server with a verifier of the host's own.
    Verifier(Arc<dyn ServerCertVerifier>),
}

impl From<Trust> for ServerAuth {
    fn from(trust: Trust) -> Self {
        Self::Trust(trust)
    }
}

impl fmt::Debug for ServerAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trust(trust) => formatter.debug_tuple("Trust").field(trust).finish(),
            Self::Verifier(_) => formatter.write_str("Verifier(..)"),
        }
    }
}

/// What a [`ServerBuilder`] collects: the [`TlsSettings`] and the two inputs, the identity to
/// present and how client certificates are treated. Start with [`ServerBuilder::new`].
///
/// Session resumption is off unless [`Policy::allow_resumption`] is set, which then keeps
/// rustls's default session cache and TLS 1.3 tickets. A policy's access control list reads each
/// connection's addresses from the scope this crate's accept helpers set
/// ([`accept`](crate::accept), [`with_addresses`](crate::with_addresses)); a host whose
/// connections reach rustls another way (quinn) takes [`configs`](ServerBuilder::configs).
///
/// The builder's `non_defaults()` is over the [`TlsSettings`], so its keys carry the `policy.`
/// prefix (`policy.tls_versions`) and `alpn` is listed when set; [`Built::non_defaults`] reports
/// the [`Policy`]'s own (`tls_versions`).
#[derive(Builder, Debug)]
#[builder(build = Built<ServerConfig>, name = "ServerBuilder", entry = "new", fallible)]
pub struct ServerParts<'a> {
    settings: TlsSettings,
    #[builder(input)]
    identity: &'a Identity,
    #[builder(input)]
    client_auth: ClientAuth,
}

impl TryFrom<ServerParts<'_>> for Built<ServerConfig> {
    type Error = PeerError;

    /// The configuration, shared by every connection it accepts.
    ///
    /// # Errors
    ///
    /// [`PeerError::Certificate`], [`PeerError::TrustPattern`], [`PeerError::CaFile`],
    /// [`PeerError::Unsupported`] for trust only clients apply, [`PeerError::Policy`] or
    /// [`PeerError::Tls`] when the configuration cannot be built.
    fn try_from(parts: ServerParts<'_>) -> Result<Self, PeerError> {
        let ServerParts {
            settings,
            identity,
            client_auth,
        } = parts;
        let alpn = settings.alpn_bytes();
        let policy = settings.policy;
        let verifier = match &client_auth {
            ClientAuth::None => None,
            ClientAuth::Required(trust) => {
                Some(Arc::new(trust.client_verifier(&policy, None)?) as Arc<dyn ClientCertVerifier>)
            }
            ClientAuth::Verifier(verifier) => Some(Arc::clone(verifier)),
        };
        let base = Base::new(identity, &alpn, &policy)?;
        Ok(Built {
            config: base.config(verifier, None, None)?,
            policy,
        })
    }
}

impl ServerBuilder<'_> {
    /// The configurations of a listener, one per connection: for a host such as quinn, whose
    /// connections reach rustls without this crate's accept helpers. See [`ServerConfigs`].
    ///
    /// # Errors
    ///
    /// As [`build`](Self::build).
    pub fn configs(self) -> Result<ServerConfigs, BuildError<PeerError>> {
        let ServerParts {
            settings,
            identity,
            client_auth,
        } = self.parts().map_err(BuildError::Missing)?;
        let alpn = settings.alpn_bytes();
        let policy = settings.policy;
        let non_defaults = policy.non_defaults();
        let configs = || {
            let client = match client_auth {
                ClientAuth::None => Client::None,
                ClientAuth::Required(trust) => {
                    Client::Trust(Arc::new(trust.client_verifier(&policy, None)?))
                }
                ClientAuth::Verifier(verifier) => Client::Verifier(verifier),
            };
            Ok(ServerConfigs::from_parts(
                Base::new(identity, &alpn, &policy)?,
                client,
                non_defaults,
            ))
        };
        configs().map_err(BuildError::Failed)
    }
}

/// What a [`ClientBuilder`] collects: the [`TlsSettings`], how the server is verified and the
/// identity to present, if any. Start with [`ClientBuilder::new`].
///
/// The server is verified as the [`ServerAuth`] given says (a [`Trust`] converts). Without an
/// identity this side presents no certificate: it proves itself, if at all, inside the
/// encrypted channel, and a server that requires a client certificate refuses it. Session
/// resumption is off unless [`Policy::allow_resumption`] is set, which then keeps rustls's
/// default session cache.
///
/// As for the server, the builder's `non_defaults()` carries the `policy.` prefix and lists
/// `alpn` when set; [`Built::non_defaults`] reports the [`Policy`]'s own.
#[derive(Builder, Debug)]
#[builder(build = Built<ClientConfig>, name = "ClientBuilder", entry = "new", fallible)]
pub struct ClientParts<'a> {
    settings: TlsSettings,
    #[builder(input)]
    server: ServerAuth,
    #[builder(input)]
    identity: Option<&'a Identity>,
}

impl TryFrom<ClientParts<'_>> for Built<ClientConfig> {
    type Error = PeerError;

    /// The configuration.
    ///
    /// # Errors
    ///
    /// [`PeerError::Unsupported`] for CA trust; [`PeerError::Policy`] when the policy asks for
    /// something the ring provider or this build cannot do; [`PeerError::Io`] or
    /// [`PeerError::Certificate`] when the roots cannot be loaded; [`PeerError::Certificate`] or
    /// [`PeerError::Tls`] when the configuration cannot be built.
    fn try_from(parts: ClientParts<'_>) -> Result<Self, PeerError> {
        let ClientParts {
            settings,
            server,
            identity,
        } = parts;
        let alpn = settings.alpn_bytes();
        let policy = settings.policy;
        let verifier: Arc<dyn ServerCertVerifier> = match server {
            ServerAuth::Trust(trust) => PeerServerVerifier::new(&trust, &policy)?,
            ServerAuth::Verifier(verifier) => verifier,
        };
        let selection = policy.selection()?;
        let builder = ClientConfig::builder_with_provider(Arc::clone(&selection.provider))
            .with_protocol_versions(&selection.versions)
            .map_err(|_| PeerError::Tls)?
            .dangerous()
            .with_custom_certificate_verifier(verifier);
        let mut config = match identity {
            Some(identity) => builder
                .with_client_auth_cert(identity.certificate_chain(), identity.private_key_der())
                .map_err(|_| PeerError::Certificate)?,
            None => builder.with_no_client_auth(),
        };
        config.alpn_protocols = alpn;
        config.require_ems = policy.require_ems;
        if !policy.allow_resumption {
            config.resumption = Resumption::disabled();
        }
        crate::profiles::require_fips(&selection, config.fips())?;
        Ok(Built { config, policy })
    }
}

/// A build failure as the crate's own error, for the places that supply every input: a missing
/// one is a programming error, reported as a policy refusal naming it.
pub(crate) fn failed(error: BuildError<PeerError>) -> PeerError {
    match error {
        BuildError::Failed(error) => error,
        other => PeerError::Policy(other.to_string()),
    }
}

/// What every server configuration under one identity, ALPN list and policy shares: built and
/// checked once, then a configuration per call, all sharing one session cache and ticketer.
#[derive(Debug, Clone)]
pub(crate) struct Base {
    versions: Vec<&'static SupportedProtocolVersion>,
    provider: Arc<CryptoProvider>,
    resolver: Arc<dyn ResolvesServerCert>,
    alpn: Vec<Vec<u8>>,
    require_ems: bool,
    resumption: bool,
    session_storage: Arc<dyn StoresServerSessions>,
    ticketer: Arc<dyn ProducesTickets>,
    #[cfg(feature = "acl")]
    acl: Option<Arc<coqui_net_acl::Acl>>,
}

impl Base {
    pub(crate) fn new(
        identity: &Identity,
        alpn: &[Vec<u8>],
        policy: &Policy,
    ) -> Result<Self, PeerError> {
        let selection = policy.selection()?;
        let (versions, provider) = (selection.versions.clone(), Arc::clone(&selection.provider));
        // Built once to check the key against the certificate and to take the resolver and
        // rustls's session state from.
        let mut template = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&versions)
            .map_err(|_| PeerError::Tls)?
            .with_no_client_auth()
            .with_single_cert(identity.certificate_chain(), identity.private_key_der())
            .map_err(|_| PeerError::Certificate)?;
        template.require_ems = policy.require_ems;
        crate::profiles::require_fips(&selection, template.fips())?;
        let session_storage: Arc<dyn StoresServerSessions> = if policy.allow_resumption {
            template.session_storage
        } else {
            // No cache for TLS 1.2 session ids or TLS 1.3 stateful tickets.
            Arc::new(NoServerSessionStorage {})
        };
        Ok(Self {
            versions,
            provider,
            resolver: template.cert_resolver,
            alpn: alpn.to_vec(),
            require_ems: policy.require_ems,
            resumption: policy.allow_resumption,
            session_storage,
            ticketer: template.ticketer,
            #[cfg(feature = "acl")]
            acl: match &policy.acl {
                Some(acl) => Some(Arc::new(crate::gate::canonical(acl)?)),
                None => None,
            },
        })
    }

    /// A configuration around `verifier` (none: server TLS). With an access control list,
    /// `verifier` is wrapped to decide each certificate and the certificate resolver to decide
    /// each `ClientHello`, from `addresses`, or from the accept helpers' scope when `None`;
    /// a refusal at the `ClientHello` goes to `refused` when given.
    pub(crate) fn config(
        &self,
        verifier: Option<Arc<dyn ClientCertVerifier>>,
        addresses: Option<(SocketAddr, SocketAddr)>,
        refused: Option<Arc<Slot>>,
    ) -> Result<ServerConfig, PeerError> {
        #[cfg(feature = "acl")]
        let gate = self
            .acl
            .as_ref()
            .map(|acl| crate::gate::Gate::new(Arc::clone(acl), addresses, refused.clone()));
        #[cfg(not(feature = "acl"))]
        let _ = addresses;
        #[cfg(feature = "acl")]
        let verifier = match (&gate, verifier) {
            (Some(gate), Some(inner)) => Some(Arc::new(crate::gate::PeerGate {
                inner,
                gate: gate.clone(),
            }) as Arc<dyn ClientCertVerifier>),
            (_, verifier) => verifier,
        };
        let verifier = match (refused, verifier) {
            (Some(slot), Some(inner)) => {
                Some(Arc::new(Keeping { inner, slot }) as Arc<dyn ClientCertVerifier>)
            }
            (_, verifier) => verifier,
        };
        #[cfg(feature = "acl")]
        let identified = verifier
            .as_ref()
            .is_some_and(|verifier| verifier.client_auth_mandatory());
        let builder = ServerConfig::builder_with_provider(Arc::clone(&self.provider))
            .with_protocol_versions(&self.versions)
            .map_err(|_| PeerError::Tls)?;
        let builder = match verifier {
            Some(verifier) => builder.with_client_cert_verifier(verifier),
            None => builder.with_no_client_auth(),
        };
        let mut config = builder.with_cert_resolver(Arc::clone(&self.resolver));
        config.alpn_protocols.clone_from(&self.alpn);
        config.require_ems = self.require_ems;
        config.session_storage = Arc::clone(&self.session_storage);
        config.ticketer = Arc::clone(&self.ticketer);
        if !self.resumption {
            config.send_tls13_tickets = 0;
        }
        #[cfg(feature = "acl")]
        if let Some(gate) = gate {
            config.cert_resolver = Arc::new(crate::gate::AddressGate {
                inner: Arc::clone(&config.cert_resolver),
                gate,
                identified,
            });
        }
        Ok(config)
    }
}
