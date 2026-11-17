//! `keygen`: an identity file, and its peer id.

use coqui_agentic_peer::auth::{self, Method};
use std::path::Path;

/// Creates the identity file at `path` for `method` (the default method without it) and returns its peer id. An existing file is
/// never replaced.
///
/// # Errors
///
/// The file exists, or could not be written.
pub fn create(path: &Path, method: Option<Method>) -> Result<String, String> {
    auth::create_identity(path, method.unwrap_or(Method::DEFAULT))
        .map(|peer| peer.to_string())
        .map_err(|error| error.to_string())
}
