use alloc::sync::Arc;
use core::fmt;

use crate::error::Error;

/// What fills a buffer with random bytes.
type Fill = dyn Fn(&mut [u8]) -> Result<(), Error> + Send + Sync;

/// A source of cryptographically secure random bytes: invitation secrets, user codes and pairing
/// slots are drawn from it.
///
/// This crate keeps no random number generator of its own in a default build, so the caller
/// passes one: `coqui-peer-tls` gives the one of its TLS provider, and with the `pin` feature
/// `Random::os` reads the operating system's. A test may pass a deterministic one.
#[derive(Clone)]
pub struct Random(Arc<Fill>);

impl Random {
    /// A source that fills a buffer with `fill`, which must be cryptographically secure.
    pub fn from_fn(fill: impl Fn(&mut [u8]) -> Result<(), Error> + Send + Sync + 'static) -> Self {
        Self(Arc::new(fill))
    }

    /// The operating system's generator.
    #[cfg(feature = "pin")]
    pub fn os() -> Self {
        Self::from_fn(|out| {
            use rand_core::RngCore;
            rand_core::OsRng
                .try_fill_bytes(out)
                .map_err(|_| Error::Random)
        })
    }

    /// Fills `out` with random bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Random`], or what the source returns, when it cannot.
    pub fn fill(&self, out: &mut [u8]) -> Result<(), Error> {
        (self.0)(out)
    }
}

impl fmt::Debug for Random {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Random")
    }
}
