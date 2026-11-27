use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use core::fmt;
use core::net::SocketAddr;

use coqui_peer::{KeyMaterial, PeerId, TrustStore};

/// A decision the host makes about a peer's key once the handshake proved the peer holds it.
///
/// It is asked at most once per handshake, never for a key that failed its proof, and records nothing
/// itself: a host that wants to remember a refused stranger does so in its own `admit`.
pub trait Admit: Send + Sync {
    /// Whether the proven `key` is admitted. `answering` is true on the side that accepted the
    /// connection; `source` is that connection's address when the caller gave one to
    /// [`Method::respond`](crate::Method::respond), and is `None` on the dialing side.
    fn admit(&self, key: &KeyMaterial, answering: bool, source: Option<SocketAddr>) -> bool;
}

/// Whose key a side of the handshake admits, once the peer proved it holds that key.
///
/// The peer's id is always SHA-256, so a pin of another digest matches nothing. There is no variant
/// for admitting anyone: a session with an unknown peer exists only as a
/// [`PairingSession`](crate::PairingSession), which can do nothing but pair.
///
/// ```compile_fail
/// let _ = coqui_peer_webcrypto::Trust::AnyWithKeyProof;
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub enum Trust {
    /// These ids, fixed when the method is built. An empty set admits nobody.
    Pinned(BTreeSet<PeerId>),
    /// The keys a store accepts, read at each handshake: a key removed or retired there is refused
    /// from the next handshake on. A responder records a proven stranger as waiting, bounded as
    /// [`TrustStore::record_waiting`] says, and the proven successor of an accepted key is promoted
    /// as the store's rotation options say.
    Store(TrustStore),
    /// The host decides, at each handshake and after the proof, with [`Admit::admit`]. Nothing is
    /// recorded for a refused key unless the host does it.
    Asked(Arc<dyn Admit>),
}

impl fmt::Debug for Trust {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pinned(ids) => f.debug_tuple("Pinned").field(ids).finish(),
            Self::Store(store) => f.debug_tuple("Store").field(store).finish(),
            Self::Asked(_) => f.debug_tuple("Asked").field(&"..").finish(),
        }
    }
}

impl Trust {
    /// [`Trust::Pinned`] from any collection of ids.
    pub fn pinned(ids: impl IntoIterator<Item = PeerId>) -> Self {
        Self::Pinned(ids.into_iter().collect())
    }

    /// [`Trust::Asked`] from a closure with [`Admit::admit`]'s arguments.
    pub fn asked(
        admit: impl Fn(&KeyMaterial, bool, Option<SocketAddr>) -> bool + Send + Sync + 'static,
    ) -> Self {
        struct Closure<F>(F);
        impl<F: Fn(&KeyMaterial, bool, Option<SocketAddr>) -> bool + Send + Sync> Admit for Closure<F> {
            fn admit(
                &self,
                key: &KeyMaterial,
                answering: bool,
                source: Option<SocketAddr>,
            ) -> bool {
                (self.0)(key, answering, source)
            }
        }
        Self::Asked(Arc::new(Closure(admit)))
    }
}

/// What a method's role admits: a [`Trust`], or for pairing any proved key.
#[derive(Debug, Clone)]
pub(crate) enum Admission {
    Trust(Trust),
    Any,
}

impl Admission {
    /// Whether the peer's proven `key` is admitted. A responder passes `waiting` with the
    /// connection's source so a proven stranger may be queued; an initiator queues nobody.
    pub(crate) fn admits(&self, key: &KeyMaterial, waiting: Option<Option<SocketAddr>>) -> bool {
        let store = match self {
            Self::Any => return true,
            Self::Trust(Trust::Pinned(ids)) => return ids.contains(&key.id()),
            Self::Trust(Trust::Asked(host)) => {
                return host.admit(key, waiting.is_some(), waiting.flatten());
            }
            Self::Trust(Trust::Store(store)) => store,
        };
        let id = key.id();
        if store.accepted_key(&id).as_ref() == Some(key) {
            return true;
        }
        if store.successor_key(&id).as_ref() == Some(key) {
            return store.prove_successor(&id).is_ok()
                && !store.rotation().require_approval
                && store.promote_successor(&id).is_ok();
        }
        if let Some(source) = waiting {
            store.record_waiting(key, source);
        }
        false
    }

    /// Whether a peer it admits is somebody rather than only the holder of some key.
    pub(crate) const fn identifies(&self) -> bool {
        matches!(self, Self::Trust(_))
    }
}
