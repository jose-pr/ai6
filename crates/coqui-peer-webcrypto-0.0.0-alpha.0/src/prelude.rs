//! The setter traits of the settings types.
//!
//! [`WebCryptoBuilder`](crate::WebCryptoBuilder) and
//! [`PairingMethodBuilder`](crate::PairingMethodBuilder) carry the setters of their settings as
//! methods. A builder from another crate that holds one of these settings types calls them with
//! these traits in scope.

pub use crate::settings::{PairingSettingsSetters, WebCryptoSettingsSetters};
