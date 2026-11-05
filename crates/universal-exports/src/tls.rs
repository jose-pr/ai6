//! The gateway on TLS of its own, without a reverse proxy in front: it asks each client for a
//! certificate and admits one without. A certificate's key is proven in the handshake, and the
//! chain is handed to the page routes ([`ClientCertificates`]), where the configured page
//! authenticator (coqui-access `mtls`) decides whether it names a person. No `/v1` route reads
//! it.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::ConnectInfo;
use rustls::DistinguishedName;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, ServerConfig, SignatureScheme};
use tower::ServiceExt;

use crate::Gateway;

/// The certificates a client proved it holds the leaf key of, DER, leaf first; empty when it
/// sent none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClientCertificates(pub(crate) Vec<Vec<u8>>);

/// Takes any client certificate whose key the client proves, and a client with none: trust is
/// the page authenticator's to decide, by pin or by CA.
#[derive(Debug)]
struct AnyClientCertificate {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for AnyClientCertificate {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The TLS configuration of a gateway that presents the certificate chain and key in the PEM
/// texts `chain` and `key`, and asks for a client certificate it does not require.
///
/// # Errors
///
/// What is wrong with the PEM or the key, in words.
pub fn tls_server_config(chain: &[u8], key: &[u8]) -> Result<ServerConfig, String> {
    let certificates = CertificateDer::pem_slice_iter(chain)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("the certificate: {error}"))?;
    if certificates.is_empty() {
        return Err("the certificate file holds no certificate".to_owned());
    }
    let key = PrivateKeyDer::from_pem_slice(key).map_err(|error| format!("the key: {error}"))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(AnyClientCertificate {
        provider: provider.clone(),
    });
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| error.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certificates, key)
        .map_err(|error| error.to_string())?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

impl Gateway {
    /// Serves the gateway on `listener` over TLS with `config` (see [`tls_server_config`]) until
    /// `shutdown`: each request carries the client's address and the certificates it proved.
    ///
    /// # Errors
    ///
    /// The listener fails to accept.
    pub async fn serve_tls<F>(
        self,
        listener: tokio::net::TcpListener,
        config: Arc<ServerConfig>,
        shutdown: F,
    ) -> std::io::Result<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let assistant = self.assistant.clone();
        let router: Router = self.router();
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        tokio::pin!(shutdown);
        loop {
            let (stream, address) = tokio::select! {
                () = &mut shutdown => break,
                accepted = listener.accept() => accepted?,
            };
            let (acceptor, router) = (acceptor.clone(), router.clone());
            tokio::spawn(async move {
                let Ok(stream) = acceptor.accept(stream).await else {
                    return;
                };
                let certificates = ClientCertificates(
                    stream
                        .get_ref()
                        .1
                        .peer_certificates()
                        .map(|chain| chain.iter().map(|der| der.to_vec()).collect())
                        .unwrap_or_default(),
                );
                let service = router.map_request(
                    move |mut request: axum::http::Request<hyper::body::Incoming>| {
                        request
                            .extensions_mut()
                            .insert(ConnectInfo::<SocketAddr>(address));
                        request.extensions_mut().insert(certificates.clone());
                        // What a front door would say, and the client's own copy is replaced:
                        // the sign-in routes take a browser's request only over HTTPS.
                        request.headers_mut().insert(
                            "x-forwarded-proto",
                            axum::http::HeaderValue::from_static("https"),
                        );
                        request
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        hyper_util::rt::TokioIo::new(stream),
                        hyper_util::service::TowerToHyperService::new(service),
                    )
                    .with_upgrades()
                    .await;
            });
        }
        if let Some(assistant) = assistant {
            assistant.flush_log().await;
        }
        Ok(())
    }
}
