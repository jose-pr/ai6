//! The setter traits of the settings types.
//!
//! [`TrustStoreBuilder`](crate::TrustStoreBuilder), [`WaitingQueueBuilder`](crate::WaitingQueueBuilder)
//! and [`PairingDeskBuilder`](crate::pair::PairingDeskBuilder) carry the setters of their settings
//! as methods. A builder from another crate that holds one of these settings types calls them
//! with these traits in scope.

pub use crate::pair::{
    CodeAlphabetSetters, DeskLimitsSetters, PairingDeskSettingsSetters, PairingOptionsSetters,
};
pub use crate::store::{RotationOptionsSetters, TrustStoreSettingsSetters, WaitingLimitsSetters};
pub use crate::waiting::WaitingSettingsSetters;
