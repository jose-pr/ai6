//! `coqui-transport` over `coqui-peer`'s own wire: the frame the drivers speak as a codec, and a
//! channel as a [`MessageTransport`].

use alloc::vec::Vec;
use core::fmt;
use core::time::Duration;
use std::io;
use std::sync::{Mutex, PoisonError};

use coqui_transport::frame::{Codec, FrameError, LengthDecoder, LengthPrefix};
use coqui_transport::{Channel, Failure};

use crate::drive::{FrameTooLong, MAX_MESSAGE};
use crate::{MessageTransport, ReadTimeout, WriteTimeout};

/// The frame of [`write_frame`](crate::write_frame) and [`read_frame`](crate::read_frame): the
/// message's length as four big-endian bytes, then the message; an empty message is a message.
///
/// It is [`LengthPrefix::u32_be`] by another name, so a `StreamChannel` on `PeerFrame` and a
/// handshake driven by [`drive_stream`](crate::drive_stream) read each other's bytes. The bound on a
/// message is the channel's (1 MiB by default, the number [`MAX_MESSAGE`] is), not this type's.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct PeerFrame;

impl PeerFrame {
    /// The codec this is, for a place that wants a [`LengthPrefix`].
    pub const fn prefix() -> LengthPrefix {
        LengthPrefix::u32_be()
    }
}

impl Codec for PeerFrame {
    type Decoder = LengthDecoder;

    fn encode(&self, message: &[u8], maximum: usize) -> Result<Vec<u8>, FrameError> {
        Self::prefix().encode(message, maximum)
    }

    fn decoder(&self, maximum: usize) -> LengthDecoder {
        Self::prefix().decoder(maximum)
    }
}

/// A [`Channel`] as a [`MessageTransport`], so a handshake can be driven over anything that
/// carries messages.
///
/// A message over [`MAX_MESSAGE`] is refused either way (`InvalidInput` going out, `InvalidData`
/// coming in), and so is a [`Failure::Protocol`]; the message drivers report both as
/// [`DriveError::Io`](crate::DriveError::Io). The read and write timeouts the `_until` drivers
/// set bound `receive` and `send_timeout`; a channel that ended in order is `UnexpectedEof`. Nothing here authenticates the peer: the handshake does.
pub struct ChannelTransport<C> {
    channel: C,
    read: Mutex<Option<Duration>>,
    write: Mutex<Option<Duration>>,
}

impl<C: Channel> ChannelTransport<C> {
    /// A transport over `channel`, with no timeouts.
    pub const fn new(channel: C) -> Self {
        Self {
            channel,
            read: Mutex::new(None),
            write: Mutex::new(None),
        }
    }

    /// The channel, for closing it or asking its capabilities.
    pub const fn channel(&self) -> &C {
        &self.channel
    }

    /// The channel.
    pub fn into_inner(self) -> C {
        self.channel
    }
}

impl<C> fmt::Debug for ChannelTransport<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChannelTransport")
            .finish_non_exhaustive()
    }
}

fn current(timeout: &Mutex<Option<Duration>>) -> Option<Duration> {
    *timeout.lock().unwrap_or_else(PoisonError::into_inner)
}

fn set(timeout: &Mutex<Option<Duration>>, value: Option<Duration>) {
    *timeout.lock().unwrap_or_else(PoisonError::into_inner) = value;
}

/// The `io::Error` of a channel failure, keeping the failure as its source.
fn failure(error: Failure, size: io::ErrorKind) -> io::Error {
    match error {
        Failure::Protocol => io::Error::new(size, FrameTooLong),
        Failure::Timeout | Failure::NoProgress => io::Error::new(io::ErrorKind::TimedOut, error),
        Failure::PathLost => io::Error::new(io::ErrorKind::ConnectionReset, error),
        Failure::Unsupported => io::Error::new(io::ErrorKind::Unsupported, error),
        Failure::Certificate | Failure::PeerProof | Failure::Permission => {
            io::Error::new(io::ErrorKind::PermissionDenied, error)
        }
        _ => io::Error::other(error),
    }
}

impl<C: Channel> MessageTransport for ChannelTransport<C> {
    fn send(&mut self, message: &[u8]) -> io::Result<()> {
        if message.len() > MAX_MESSAGE {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, FrameTooLong));
        }
        self.channel
            .send_timeout(message, current(&self.write))
            .map_err(|error| failure(error, io::ErrorKind::InvalidInput))
    }

    fn recv(&mut self) -> io::Result<Vec<u8>> {
        let message = self
            .channel
            .receive(current(&self.read))
            .map_err(|error| failure(error, io::ErrorKind::InvalidData))?
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        if message.len() > MAX_MESSAGE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, FrameTooLong));
        }
        Ok(message)
    }
}

impl<C> ReadTimeout for ChannelTransport<C> {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        set(&self.read, timeout);
        Ok(())
    }
}

impl<C> WriteTimeout for ChannelTransport<C> {
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        set(&self.write, timeout);
        Ok(())
    }
}
