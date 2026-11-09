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
use coqui_peer::NonDefault;

/// rustls's ring provider with its defaults. The configurations built here take the provider a
/// [`Policy`] selects ([`Policy::crypto_provider`]); nothing in this crate installs a
/// process-wide default.
pub fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A built configuration and the settings of its [`Policy`] that differ from the default
/// policy's, so a host can surface them. Nothing is printed by this crate.
#[derive(Debug)]
#[non_exhaustive]
pub struct Built<T> {
    /// The configuration.
    pub config: T,
    /// Every setting that differs from the default policy's; empty under the default policy.
    pub non_defaults: Vec<NonDefault>,
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

/// Builds a server configuration, or the configurations of a listener, that presents an
/// identity: [`ServerBuilder::new`], the settings, then [`build`](Self::build).
///
/// Session resumption is off unless [`Policy::allow_resumption`] is set, which then keeps
/// rustls's default session cache and TLS 1.3 tickets. A policy's access control list reads each
/// connection's addresses from the scope this crate's accept helpers set
/// ([`accept`](crate::accept), [`with_addresses`](crate::with_addresses)); a host whose
/// connections reach rustls another way (quinn) takes [`configs`](Self::configs).
#[derive(Debug, Clone)]
#[must_use]
pub struct ServerBuilder<'a> {
    identity: &'a Identity,
    client_auth: ClientAuth,
    alpn: Vec<Vec<u8>>,
    policy: Policy,
}

impl<'a> ServerBuilder<'a> {
    /// A builder for a server that presents `identity` and treats client certificates as
    /// `client_auth` says, with no ALPN and the default policy.
    pub fn new(identity: &'a Identity, client_auth: ClientAuth) -> Self {
        Self {
            identity,
            client_auth,
            alpn: Vec::new(),
            policy: Policy::default(),
        }
    }

    /// The application protocols offered, most preferred first. Replaces any set before.
    pub fn alpn<I>(mut self, protocols: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<[u8]>,
    {
        self.alpn = protocols
            .into_iter()
            .map(|protocol| protocol.as_ref().to_vec())
            .collect();
        self
    }

    /// The policy: versions, suites, groups, the extended master secret, resumption and the
    /// access control list. Default [`Policy::default`].
    pub fn policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// The ALPN list and the policy from `settings`, as a file or a form gave them. Replaces any
    /// set before by [`alpn`](Self::alpn) and [`policy`](Self::policy), and is replaced by those
    /// called after it.
    pub fn settings(mut self, settings: TlsSettings) -> Self {
        self.alpn = settings.alpn_bytes();
        self.policy = settings.policy;
        self
    }

    /// Every setting of the policy that differs from the default, as the built configuration
    /// reports it.
    pub fn non_defaults(&self) -> Vec<NonDefault> {
        self.policy.non_defaults()
    }

    /// The configuration, shared by every connection it accepts.
    ///
    /// # Errors
    ///
    /// [`PeerError::Certificate`], [`PeerError::TrustPattern`], [`PeerError::CaFile`],
    /// [`PeerError::Unsupported`] for trust only clients apply, [`PeerError::Policy`] or
    /// [`PeerError::Tls`] when the configuration cannot be built.
    pub fn build(self) -> Result<Built<ServerConfig>, PeerError> {
        let non_defaults = self.policy.non_defaults();
        let verifier = match &self.client_auth {
            ClientAuth::None => None,
            ClientAuth::Required(trust) => {
                Some(Arc::new(trust.client_verifier(&self.policy, None)?)
                    as Arc<dyn ClientCertVerifier>)
            }
            ClientAuth::Verifier(verifier) => Some(Arc::clone(verifier)),
        };
        let base = Base::new(self.identity, &self.alpn, &self.policy)?;
        Ok(Built {
            config: base.config(verifier, None, None)?,
            non_defaults,
        })
    }

    /// The configurations of a listener, one per connection: for a host such as quinn, whose
    /// connections reach rustls without this crate's accept helpers. See [`ServerConfigs`].
    ///
    /// # Errors
    ///
    /// As [`build`](Self::build).
    pub fn configs(self) -> Result<ServerConfigs, PeerError> {
        let non_defaults = self.policy.non_defaults();
        let client = match self.client_auth {
            ClientAuth::None => Client::None,
            ClientAuth::Required(trust) => {
                Client::Trust(Arc::new(trust.client_verifier(&self.policy, None)?))
            }
            ClientAuth::Verifier(verifier) => Client::Verifier(verifier),
        };
        Ok(ServerConfigs::from_parts(
            Base::new(self.identity, &self.alpn, &self.policy)?,
            client,
            non_defaults,
        ))
    }
}

/// Builds a client configuration: [`ClientBuilder::new`], the settings, then
/// [`build`](Self::build).
///
/// The server is verified as the [`ServerAuth`] given to `new` says. Session resumption is off
/// unless [`Policy::allow_resumption`] is set, which then keeps rustls's default session cache.
#[derive(Debug, Clone)]
#[must_use]
pub struct ClientBuilder<'a> {
    server: ServerAuth,
    identity: Option<&'a Identity>,
    alpn: Vec<Vec<u8>>,
    policy: Policy,
}

impl<'a> ClientBuilder<'a> {
    /// A builder for a client that verifies the server as `server` says (a [`Trust`] converts),
    /// presents no certificate, offers no ALPN and runs under the default policy.
    pub fn new(server: impl Into<ServerAuth>) -> Self {
        Self {
            server: server.into(),
            identity: None,
            alpn: Vec::new(),
            policy: Policy::default(),
        }
    }

    /// Presents `identity` and its intermediates as the client certificate. Without it this side
    /// proves itself, if at all, inside the encrypted channel, and a server that requires a
    /// client certificate refuses it.
    pub fn identity(mut self, identity: &'a Identity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// The application protocols offered, most preferred first. Replaces any set before.
    pub fn alpn<I>(mut self, protocols: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<[u8]>,
    {
        self.alpn = protocols
            .into_iter()
            .map(|protocol| protocol.as_ref().to_vec())
            .collect();
        self
    }

    /// The policy. Default [`Policy::default`].
    pub fn policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// The ALPN list and the policy from `settings`, as a file or a form gave them. Replaces
    /// any set before by [`alpn`](Self::alpn) and [`policy`](Self::policy), and is replaced by
    /// those called after it.
    pub fn settings(mut self, settings: TlsSettings) -> Self {
        self.alpn = settings.alpn_bytes();
        self.policy = settings.policy;
        self
    }

    /// Every setting of the policy that differs from the default, as the built configuration
    /// reports it.
    pub fn non_defaults(&self) -> Vec<NonDefault> {
        self.policy.non_defaults()
    }

    /// The configuration.
    ///
    /// # Errors
    ///
    /// [`PeerError::Unsupported`] for CA trust; [`PeerError::Policy`] when the policy asks for
    /// something the ring provider or this build cannot do; [`PeerError::Io`] or
    /// [`PeerError::Certificate`] when the roots cannot be loaded; [`PeerError::Certificate`] or
    /// [`PeerError::Tls`] when the configuration cannot be built.
    pub fn build(self) -> Result<Built<ClientConfig>, PeerError> {
        let verifier: Arc<dyn ServerCertVerifier> = match self.server {
            ServerAuth::Trust(trust) => PeerServerVerifier::new(&trust, &self.policy)?,
            ServerAuth::Verifier(verifier) => verifier,
        };
        let selection = self.policy.selection()?;
        let builder = ClientConfig::builder_with_provider(Arc::clone(&selection.provider))
            .with_protocol_versions(&selection.versions)
            .map_err(|_| PeerError::Tls)?
            .dangerous()
            .with_custom_certificate_verifier(verifier);
        let mut config = match self.identity {
            Some(identity) => builder
                .with_client_auth_cert(identity.certificate_chain(), identity.private_key_der())
                .map_err(|_| PeerError::Certificate)?,
            None => builder.with_no_client_auth(),
        };
        config.alpn_protocols = self.alpn;
        config.require_ems = self.policy.require_ems;
        if !self.policy.allow_resumption {
            config.resumption = Resumption::disabled();
        }
        crate::profiles::require_fips(&selection, config.fips())?;
        Ok(Built {
            config,
            non_defaults: self.policy.non_defaults(),
        })
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
