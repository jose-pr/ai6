//! The addresses a handshake came from and arrived at, made visible to the verifiers.
//!
//! rustls shares one verifier between every connection of a `ServerConfig` and does not
//! expose it, so the verifier cannot be given a per-connection value. Its callbacks run
//! synchronously inside the handshake call that the accept helpers make, on the calling
//! thread, so the helpers scope the addresses around exactly those calls.

use core::cell::Cell;
use core::net::SocketAddr;

use crate::error::Refusal;

/// What the calls on this thread are scoped to.
#[derive(Clone, Copy, Default)]
struct Scope {
    source: Option<SocketAddr>,
    #[cfg_attr(
        not(feature = "acl"),
        expect(dead_code, reason = "only the access control list reads it")
    )]
    local: Option<SocketAddr>,
    /// The refusal of a check that cannot carry one in its own error (the certificate resolver).
    refused: Option<Refusal>,
}

thread_local! {
    static CURRENT: Cell<Scope> = const {
        Cell::new(Scope {
            source: None,
            local: None,
            refused: None,
        })
    };
}

struct Restore(Scope);

impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.with(|current| current.set(self.0));
    }
}

/// Runs `work` with the connection's addresses in scope: `source` is where the verifiers record
/// waiting entries and what a policy's access control list decides by, and `local` the address
/// the connection arrived at, which the list needs for rules that name a destination or a port.
///
/// The accept helpers call this around the handshake; a host that drives rustls itself wraps
/// each call that can reach a verifier (for example `process_new_packets`). Outside it, entries
/// are recorded without a source.
pub fn with_addresses<R>(
    source: Option<SocketAddr>,
    local: Option<SocketAddr>,
    work: impl FnOnce() -> R,
) -> R {
    scoped(source, local, work).0
}

/// Runs `work` in a scope of its own, and returns what it returned with the refusal a check
/// inside it recorded.
pub(crate) fn scoped<R>(
    source: Option<SocketAddr>,
    local: Option<SocketAddr>,
    work: impl FnOnce() -> R,
) -> (R, Option<Refusal>) {
    let _restore = Restore(CURRENT.with(|current| {
        current.replace(Scope {
            source,
            local,
            refused: None,
        })
    }));
    let result = work();
    (result, CURRENT.with(Cell::get).refused)
}

/// The source of the current scope.
pub(crate) fn current() -> Option<SocketAddr> {
    CURRENT.with(Cell::get).source
}

/// The local address of the current scope.
#[cfg(feature = "acl")]
pub(crate) fn local() -> Option<SocketAddr> {
    CURRENT.with(Cell::get).local
}

/// Records `refusal` in the current scope, for the accept helpers to report.
#[cfg(feature = "acl")]
pub(crate) fn refuse(refusal: Refusal) {
    CURRENT.with(|current| {
        let mut scope = current.get();
        scope.refused.get_or_insert(refusal);
        current.set(scope);
    });
}
