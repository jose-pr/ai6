use alloc::sync::Arc;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use aes_gcm::aead::consts::U12;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::error::Error;
use crate::message::PROFILE;
use crate::transcript::{Keys, field};

/// What one direction may carry by default, before a new handshake: 2^38, against the 2^38.5 bytes
/// of RFC 8446 section 5.5's 2^24.5 full-size AES-GCM records. Each record counts its length and 32.
pub const DEFAULT_SESSION_LIMIT: u64 = 1 << 38;

const INNER_DATA: u8 = 1;
const INNER_CLOSE: u8 = 2;
const TAG_LEN: usize = 16;
/// A close record: its one inner byte and the tag. It stays possible at the limit.
const CLOSE_LEN: usize = 1 + TAG_LEN;
/// A record's blocks beyond its length: the counter block of the tag, and GHASH's length block.
const RECORD_OVERHEAD: u64 = 32;

/// One direction: AES-256-GCM under its own key, the nonce being the record's position. The cipher
/// is on the heap, so moving this value copies no round keys.
struct Direction {
    cipher: Box<Aes256Gcm>,
    sequence: u64,
    charged: u64,
    limit: u64,
    /// Set by either half on a bad record: nothing more is sent or believed.
    failed: Arc<AtomicBool>,
}

impl Direction {
    fn new(key: &[u8], limit: u64, failed: Arc<AtomicBool>) -> Self {
        let mut bytes = Zeroizing::new(Key::<Aes256Gcm>::default());
        bytes.copy_from_slice(key);
        Self {
            cipher: Box::new(Aes256Gcm::new(&bytes)),
            sequence: 0,
            charged: 0,
            limit,
            failed,
        }
    }

    fn check(&self) -> Result<(), Error> {
        if self.failed.load(Ordering::SeqCst) {
            return Err(Error::Record);
        }
        Ok(())
    }

    fn fail(&self) -> Error {
        self.failed.store(true, Ordering::SeqCst);
        Error::Record
    }

    /// Counts a record of `length` bytes against the limit.
    fn charge(&mut self, length: usize) -> Result<(), Error> {
        let cost = u64::try_from(length)
            .unwrap_or(u64::MAX)
            .saturating_add(RECORD_OVERHEAD);
        let total = self.charged.saturating_add(cost);
        if total > self.limit {
            return Err(Error::LimitReached);
        }
        self.charged = total;
        Ok(())
    }

    fn nonce(&mut self) -> Result<Nonce<U12>, Error> {
        let mut nonce = Nonce::<U12>::default();
        nonce[4..].copy_from_slice(&self.sequence.to_be_bytes());
        self.sequence = self.sequence.checked_add(1).ok_or(Error::LimitReached)?;
        Ok(nonce)
    }
}

/// The sending half of a [`Session`].
pub struct Sealer {
    direction: Direction,
    closed: bool,
}

impl Sealer {
    /// One message as one record, to send whole and in this order.
    ///
    /// # Errors
    ///
    /// [`Error::Record`] after a bad record in either direction, [`Error::Closed`] after
    /// [`close`](Sealer::close), [`Error::LimitReached`] when this record would pass the limit.
    pub fn seal(&mut self, message: &[u8]) -> Result<Vec<u8>, Error> {
        self.direction.check()?;
        if self.closed {
            return Err(Error::Closed);
        }
        self.direction
            .charge(message.len().saturating_add(CLOSE_LEN))?;
        let mut inner = Vec::with_capacity(message.len() + 1);
        inner.push(INNER_DATA);
        inner.extend_from_slice(message);
        self.encrypt(&inner)
    }

    /// The record that ends this direction, to send as the last one. It says the stream was not
    /// cut: a peer that never gets it reports [`Error::Truncated`]. Receiving goes on.
    ///
    /// # Errors
    ///
    /// [`Error::Record`] after a bad record in either direction, [`Error::Closed`] when already
    /// closed.
    pub fn close(&mut self) -> Result<Vec<u8>, Error> {
        self.direction.check()?;
        if self.closed {
            return Err(Error::Closed);
        }
        self.closed = true;
        self.encrypt(&[INNER_CLOSE])
    }

    fn encrypt(&mut self, inner: &[u8]) -> Result<Vec<u8>, Error> {
        let nonce = self.direction.nonce()?;
        self.direction
            .cipher
            .encrypt(&nonce, inner)
            .map_err(|_| self.direction.fail())
    }
}

/// The receiving half of a [`Session`].
pub struct Opener {
    direction: Direction,
    peer_closed: bool,
}

impl Opener {
    /// The message of the next record, or `None` when it is the peer's close record.
    ///
    /// # Errors
    ///
    /// [`Error::Record`] when the record was altered, replayed, reordered, made under other keys,
    /// of an unknown type, or came after the peer's close. Nothing after it is believed or sent,
    /// in either direction. [`Error::LimitReached`] when it passes the limit.
    pub fn open(&mut self, record: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        self.direction.check()?;
        if self.peer_closed {
            return Err(self.direction.fail());
        }
        if record.len() > CLOSE_LEN {
            self.direction.charge(record.len())?;
        }
        let nonce = self.direction.nonce()?;
        let mut inner = self
            .direction
            .cipher
            .decrypt(&nonce, record)
            .map_err(|_| self.direction.fail())?;
        match inner.first().copied() {
            Some(INNER_DATA) => {
                inner.remove(0);
                Ok(Some(inner))
            }
            Some(INNER_CLOSE) if inner.len() == 1 => {
                self.peer_closed = true;
                Ok(None)
            }
            _ => Err(self.direction.fail()),
        }
    }

    /// Call it when the transport ended: whether the stream ended where the peer meant it to.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] when the peer's close record never came, which is what a relay cutting
    /// the connection looks like, and [`Error::Record`] after a bad record.
    pub fn verify_end(&self) -> Result<(), Error> {
        self.direction.check()?;
        if self.peer_closed {
            Ok(())
        } else {
            Err(Error::Truncated)
        }
    }
}

impl fmt::Debug for Sealer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Sealer")
    }
}

impl fmt::Debug for Opener {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Opener")
    }
}

/// What a finished [`WebCrypto`](crate::WebCrypto) handshake yields: the id and the keys of this
/// session with a peer its [`Trust`](crate::Trust) admitted.
///
/// Only records made with it are tied to the peers that were proved. Anything sent beside them on
/// the same transport comes from whoever controls that transport: a reverse proxy, or a relay that
/// forwarded the whole handshake.
pub struct Session {
    id: [u8; 32],
    sealer: Sealer,
    opener: Opener,
    exporter: Zeroizing<Vec<u8>>,
}

impl Session {
    pub(crate) fn new(id: [u8; 32], keys: &Keys, initiator: bool, limit: u64) -> Self {
        let (sending, receiving) = if initiator {
            (&keys.initiator, &keys.responder)
        } else {
            (&keys.responder, &keys.initiator)
        };
        let failed = Arc::new(AtomicBool::new(false));
        Self {
            id,
            sealer: Sealer {
                direction: Direction::new(sending, limit, failed.clone()),
                closed: false,
            },
            opener: Opener {
                direction: Direction::new(receiving, limit, failed),
                peer_closed: false,
            },
            exporter: keys.exporter.clone(),
        }
    }

    /// The id both ends agreed on: a hash of both keys, both nonces, both ephemeral keys and any
    /// outer binding, all of which both signatures cover. It is not secret, since whoever carried
    /// the handshake can compute it; [`export`](Session::export) is.
    #[must_use]
    pub const fn id(&self) -> &[u8; 32] {
        &self.id
    }

    /// One message as one record, to send whole and in this order.
    ///
    /// # Errors
    ///
    /// See [`Sealer::seal`].
    pub fn seal(&mut self, message: &[u8]) -> Result<Vec<u8>, Error> {
        self.sealer.seal(message)
    }

    /// The record that ends this side's direction, to send as the last one.
    ///
    /// # Errors
    ///
    /// See [`Sealer::close`].
    pub fn close(&mut self) -> Result<Vec<u8>, Error> {
        self.sealer.close()
    }

    /// The message of the next record, or `None` when it is the peer's close record.
    ///
    /// # Errors
    ///
    /// See [`Opener::open`].
    pub fn open(&mut self, record: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        self.opener.open(record)
    }

    /// Call it when the transport ended: whether the stream ended where the peer meant it to.
    ///
    /// # Errors
    ///
    /// See [`Opener::verify_end`].
    pub fn verify_end(&self) -> Result<(), Error> {
        self.opener.verify_end()
    }

    /// The two directions apart, for a reader and a writer on different threads. A bad record at
    /// the opener still stops the sealer. Take any export first.
    #[must_use]
    pub fn split(self) -> (Sealer, Opener) {
        (self.sealer, self.opener)
    }

    /// Fills `out` with key material only this session's two peers can derive. The same `label`
    /// and the same length give the same bytes at both ends; another label or another length gives
    /// unrelated bytes.
    ///
    /// # Errors
    ///
    /// [`Error::TooLong`] above 8160 bytes, what HKDF-SHA256 can give.
    pub fn export(&self, label: &str, out: &mut [u8]) -> Result<(), Error> {
        let mut info = Vec::with_capacity(64 + label.len());
        field(&mut info, format!("{PROFILE}/export").as_bytes());
        field(&mut info, label.as_bytes());
        let length = u32::try_from(out.len()).map_err(|_| Error::TooLong)?;
        info.extend_from_slice(&length.to_be_bytes());
        Hkdf::<Sha256>::new(None, &self.exporter)
            .expand(&info, out)
            .map_err(|_| Error::TooLong)
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Session").finish_non_exhaustive()
    }
}
