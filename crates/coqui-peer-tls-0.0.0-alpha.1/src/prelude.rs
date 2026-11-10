//! The setter traits of the settings types.
//!
//! [`ServerBuilder`](crate::ServerBuilder) and [`ClientBuilder`](crate::ClientBuilder) carry the
//! setters of [`TlsSettings`](crate::TlsSettings) as methods. A builder from another crate that
//! holds one of these settings types calls them with these traits in scope.

pub use crate::pairing::PairConfigSetters;
pub use crate::policy::PolicySetters;
pub use crate::settings::TlsSettingsSetters;
