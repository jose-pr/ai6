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

#[cfg(feature = "tokio")]
pub mod asynch;
mod clock;
mod drive;
mod error;
pub mod file;
mod id;
mod key;
mod method;
pub mod pair;
pub mod pem;
pub mod prelude;
mod random;
mod store;
#[cfg(feature = "coqui-transport")]
pub mod transport;
mod waiting;

pub use clock::Clock;
pub use drive::{
    DriveError, MAX_MESSAGE, MessageTransport, ReadTimeout, WriteTimeout, complete_messages,
    complete_messages_until, complete_stream, complete_stream_until, drive_messages,
    drive_messages_until, drive_stream, drive_stream_until, read_frame, write_frame,
};
pub use error::Error;
pub use id::{Digest, PeerId};
pub use key::{KeyMaterial, MAX_KEY_BYTES};
pub use method::{Established, Framing, Handshake, Method};
pub use random::Random;
pub use store::{
    FORMAT_VERSION, Invitation, InvitationEntry, InvitationId, InvitationOptions, MAX_LABEL,
    Retiring, RotationEvent, RotationOptions, Successor, TrustStore, TrustStoreBuilder,
    TrustStoreParts, TrustStoreSettings, WaitingEntry, WaitingLimits,
};
pub use waiting::{
    Admission, DEFAULT_RECORD_INTERVAL, WaitingQueue, WaitingQueueBuilder, WaitingQueueParts,
    WaitingSettings, WhenFull,
};
