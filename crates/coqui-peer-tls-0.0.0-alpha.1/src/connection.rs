//! Server configurations built for one connection each, from one listener's settings.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::net::SocketAddr;
use std::sync::{Mutex, PoisonError};

use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme};

use crate::config::Base;
use crate::error::{PeerError, Refusal};
use crate::trust::PeerClientVerifier;
use coqui_builder::NonDefault;

/// The server configurations of one listener whose connections reach rustls without this
/// crate's accept helpers, such as quinn, which runs the verifier on threads of its own and
/// takes a configuration per incoming connection.
///
/// Built once, by [`ServerBuilder::configs`](crate::ServerBuilder::configs), from the identity, ALPN list and [`Policy`](crate::Policy)
/// (checked then), it gives a
/// configuration for each connection's addresses with [`for_connection`](Self::for_connection).
/// Those configurations share one session cache and ticketer, so with
/// [`Policy::allow_resumption`](crate::Policy::allow_resumption) a client resumes across connections; waiting entries are
/// recorded under the connection's source, and a policy's access control list is decided from
/// its addresses. Cloning is cheap and shares the session state.
#[derive(Clone)]
pub struct ServerConfigs {
    base: Base,
    client: Client,
    non_defaults: Vec<NonDefault>,
}

#[derive(Clone)]
pub(crate) enum Client {
    Trust(Arc<PeerClientVerifier>),
    Verifier(Arc<dyn ClientCertVerifier>),
    None,
}

impl fmt::Debug for ServerConfigs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let client = match self.client {
            Client::Trust(_) => "trust",
            Client::Verifier(_) => "verifier",
            Client::None => "none",
        };
        formatter
            .debug_struct("ServerConfigs")
            .field("client", &client)
            .field("non_defaults", &self.non_defaults)
            .finish_non_exhaustive()
    }
}

impl ServerConfigs {
    pub(crate) fn from_parts(base: Base, client: Client, non_defaults: Vec<NonDefault>) -> Self {
        Self {
            base,
            client,
            non_defaults,
        }
    }

    /// Every setting of the policy that differs from the default, as the configuration builders
    /// report it.
    pub fn non_defaults(&self) -> &[NonDefault] {
        &self.non_defaults
    }

    /// The configuration of one connection from `source` to `local`.
    ///
    /// Waiting entries are recorded under `source`; a policy's access control list is decided
    /// from these addresses, by address when the `ClientHello` arrives and by the client
    /// certificate's fingerprint before the trust store is consulted. The first refusal of
    /// the handshake is kept in the returned [`ConnectionConfig`], since a host such as quinn
    /// reports a failed server handshake only by its alert.
    ///
    /// # Errors
    ///
    /// [`PeerError::Tls`] when rustls refuses the configuration, which the checks at
    /// construction rule out.
    pub fn for_connection(
        &self,
        source: SocketAddr,
        local: SocketAddr,
    ) -> Result<ConnectionConfig, PeerError> {
        let slot = Arc::new(Slot::default());
        let verifier: Option<Arc<dyn ClientCertVerifier>> = match &self.client {
            Client::Trust(verifier) => Some(Arc::new(verifier.for_source(source))),
            Client::Verifier(verifier) => Some(Arc::clone(verifier)),
            Client::None => None,
        };
        let config = self
            .base
            .config(verifier, Some((source, local)), Some(Arc::clone(&slot)))?;
        Ok(ConnectionConfig {
            config: Arc::new(config),
            refused: slot,
        })
    }
}

/// The server configuration of one connection, and the refusal its handshake ended with.
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    config: Arc<ServerConfig>,
    refused: Arc<Slot>,
}

impl ConnectionConfig {
    /// The configuration, to hand to the connection (for quinn,
    /// `QuicServerConfig::try_from(config)`).
    pub fn config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.config)
    }

    /// Why this side refused the client, once the handshake failed on a [`Refusal`]: by
    /// address at the `ClientHello`, or by the client verifier. `None` before, after a
    /// success, or when the handshake failed for another reason.
    pub fn refusal(&self) -> Option<Refusal> {
        self.refused.get()
    }
}

/// The first refusal of one connection's handshake.
#[derive(Debug, Default)]
pub(crate) struct Slot(Mutex<Option<Refusal>>);

impl Slot {
    pub(crate) fn keep(&self, refusal: Refusal) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(refusal);
    }

    fn get(&self) -> Option<Refusal> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A client verifier that keeps the first [`Refusal`] its inner verifier returns.
#[derive(Debug)]
pub(crate) struct Keeping {
    pub(crate) inner: Arc<dyn ClientCertVerifier>,
    pub(crate) slot: Arc<Slot>,
}

impl Keeping {
    fn kept<T>(&self, result: Result<T, rustls::Error>) -> Result<T, rustls::Error> {
        if let Err(error) = &result
            && let Some(refusal) = Refusal::from_rustls(error)
        {
            self.slot.keep(refusal);
        }
        result
    }
}

impl ClientCertVerifier for Keeping {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        certificate: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.kept(
            self.inner
                .verify_client_cert(certificate, intermediates, now),
        )
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.kept(
            self.inner
                .verify_tls12_signature(message, certificate, signed),
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signed: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.kept(
            self.inner
                .verify_tls13_signature(message, certificate, signed),
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }

    fn requires_raw_public_keys(&self) -> bool {
        self.inner.requires_raw_public_keys()
    }
}
