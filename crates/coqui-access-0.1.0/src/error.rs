//! The refusals providers and hosts agree on, as [`ProviderError`]s.
//!
//! The status is what the host acts on; the message is for a person and never contains a
//! credential.

use guatiao::Status;
use guatiao::library::ProviderError;

/// The provider does not do this at all. The host does not retry.
pub fn unsupported(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, message)
}

/// The thing asked for does not exist: an account that is not kept.
pub fn not_found(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_NOT_FOUND, message)
}

/// The request or the configuration is malformed.
pub fn bad_value(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

/// Anything else that went wrong inside the provider: storage or a directory was
/// unreachable. A host treats it as "could not find out", never as "denied".
pub fn internal(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, message)
}
