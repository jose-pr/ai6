#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

extern crate alloc;

mod error;
mod framed;
mod handshake;
mod identity;
mod message;
mod pairing;
mod session;
mod settings;
mod transcript;
mod trust;

pub use coqui_peer::pair;
pub use coqui_peer::{
    Established, KeyMaterial, MessageTransport, Method, NonDefault, PeerId, Random, TrustStore,
};
pub use error::Error;
pub use framed::FramedStream;
pub use handshake::{METHOD, WebCrypto, WebCryptoBuilder, WebCryptoHandshake, random};
pub use identity::Identity;
pub use message::{DEFAULT_MAX_MESSAGE, PROFILE};
pub use pairing::{
    PAIR_EXPORT_LABEL, PairDial, PairingHandshake, PairingMethod, PairingMethodBuilder,
    PairingSession,
};
pub use session::{DEFAULT_SESSION_LIMIT, Opener, Sealer, Session};
pub use settings::{PairingSettings, WebCryptoSettings};
pub use trust::Trust;
