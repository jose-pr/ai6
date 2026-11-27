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

/// What a directory answers when one lookup finds more than one account (a filter that
/// matches two): a matcher refuses the sign-in rather than choose. A `bad_value` carrying
/// [`AMBIGUOUS`], so no kind has to change for it.
pub fn ambiguous() -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, AMBIGUOUS)
}

/// What a provider answers when it will not take another pending ceremony from a source: a
/// `bad_value` carrying [`BUSY`], so no kind has to change for it. The caller may try again
/// later.
pub fn busy() -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, BUSY)
}

/// The message of [`busy`].
pub const BUSY: &str = "BUSY";

/// Whether `error` is [`busy`].
pub fn is_busy(error: &ProviderError) -> bool {
    error.status == Status::GUATIAO_ERR_BAD_VALUE && error.message() == BUSY
}

/// The message of [`ambiguous`].
pub const AMBIGUOUS: &str = "the lookup finds more than one account";

/// Whether `error` is [`ambiguous`].
pub fn is_ambiguous(error: &ProviderError) -> bool {
    error.status == Status::GUATIAO_ERR_BAD_VALUE && error.message() == AMBIGUOUS
}
