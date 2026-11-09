//! The loops of [`drive_stream`](crate::drive_stream) and
//! [`drive_messages`](crate::drive_messages) over tokio streams and async message transports.

use core::future::Future;
use std::io;
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::drive::{DriveError, check_whole, frame_header, frame_length, transport_error};
use crate::method::{Established, Framing, Handshake};

const CHUNK: usize = 16 * 1024;

/// An asynchronous transport of whole messages.
pub trait MessageTransport {
    /// Sends one message.
    fn send(&mut self, message: &[u8]) -> impl Future<Output = io::Result<()>> + Send;

    /// Receives the next message.
    fn recv(&mut self) -> impl Future<Output = io::Result<Vec<u8>>> + Send;
}

/// [`write_frame`](crate::write_frame) over a tokio stream.
///
/// # Errors
///
/// `InvalidInput` when `message` is longer than [`MAX_MESSAGE`](crate::MAX_MESSAGE);
/// otherwise the stream's error.
pub async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(
    io: &mut W,
    message: &[u8],
) -> io::Result<()> {
    io.write_all(&frame_header(message.len(), io::ErrorKind::InvalidInput)?)
        .await?;
    io.write_all(message).await?;
    io.flush().await
}

/// [`read_frame`](crate::read_frame) over a tokio stream: the buffer grows as bytes arrive.
///
/// # Errors
///
/// `InvalidData` when the declared length is above [`MAX_MESSAGE`](crate::MAX_MESSAGE),
/// `UnexpectedEof` when the stream ends inside the frame; otherwise the stream's error.
pub async fn read_frame<R: AsyncRead + Unpin + ?Sized>(io: &mut R) -> io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    io.read_exact(&mut header).await?;
    let length = frame_length(header)?;
    let mut message = Vec::new();
    io.take(length).read_to_end(&mut message).await?;
    check_whole(&message, length)?;
    Ok(message)
}

/// [`drive_stream`](crate::drive_stream) over a tokio stream.
///
/// # Errors
///
/// [`DriveError`] when the stream fails or closes, or the handshake refuses the peer.
pub async fn drive_stream<H, T>(
    handshake: H,
    io: &mut T,
) -> Result<Established<H::Output>, DriveError<H::Error>>
where
    H: Handshake,
    T: AsyncRead + AsyncWrite + Unpin,
{
    complete_stream(handshake, io)
        .await?
        .finish()
        .map_err(DriveError::Handshake)
}

/// [`complete_stream`](crate::complete_stream) over a tokio stream: the handshake once
/// complete, not yet finished.
///
/// # Errors
///
/// As [`drive_stream`].
pub async fn complete_stream<H, T>(mut handshake: H, io: &mut T) -> Result<H, DriveError<H::Error>>
where
    H: Handshake,
    T: AsyncRead + AsyncWrite + Unpin,
{
    let framed = handshake.framing() == Framing::Message;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        send_all(&mut handshake, io, framed).await?;
        if handshake.is_complete() {
            return Ok(handshake);
        }
        let received = if framed {
            let message = read_frame(io).await.map_err(transport_error)?;
            handshake.receive(&message)
        } else {
            let read = io.read(&mut buffer).await.map_err(DriveError::Io)?;
            if read == 0 {
                return Err(DriveError::Io(io::ErrorKind::UnexpectedEof.into()));
            }
            handshake.receive(&buffer[..read])
        };
        if let Err(error) = received {
            // A method may have queued a refusal to tell the peer (a TLS alert).
            let _ = send_all(&mut handshake, io, framed).await;
            return Err(DriveError::Handshake(error));
        }
    }
}

/// Sends everything the handshake has queued, framed when it is message-oriented.
async fn send_all<H, T>(
    handshake: &mut H,
    io: &mut T,
    framed: bool,
) -> Result<(), DriveError<H::Error>>
where
    H: Handshake,
    T: AsyncWrite + Unpin,
{
    while let Some(out) = handshake.transmit().map_err(DriveError::Handshake)? {
        if framed {
            write_frame(io, &out).await.map_err(transport_error)?;
        } else {
            io.write_all(&out).await.map_err(DriveError::Io)?;
        }
    }
    io.flush().await.map_err(DriveError::Io)
}

/// Runs `work` until `deadline`, a passed deadline being [`DriveError::TimedOut`].
async fn until<T, E>(
    deadline: Instant,
    work: impl Future<Output = Result<T, DriveError<E>>>,
) -> Result<T, DriveError<E>> {
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), work)
        .await
        .unwrap_or(Err(DriveError::TimedOut))
}

/// [`drive_stream`] that ends at `deadline`, through `tokio::time`: the runtime must have its
/// timer enabled (tokio panics otherwise).
///
/// # Errors
///
/// [`DriveError::TimedOut`] when `deadline` passes first; otherwise as [`drive_stream`].
pub async fn drive_stream_until<H, T>(
    handshake: H,
    io: &mut T,
    deadline: Instant,
) -> Result<Established<H::Output>, DriveError<H::Error>>
where
    H: Handshake,
    T: AsyncRead + AsyncWrite + Unpin,
{
    until(deadline, drive_stream(handshake, io)).await
}

/// [`complete_stream`] that ends at `deadline`, as [`drive_stream_until`] does.
///
/// # Errors
///
/// As [`drive_stream_until`].
pub async fn complete_stream_until<H, T>(
    handshake: H,
    io: &mut T,
    deadline: Instant,
) -> Result<H, DriveError<H::Error>>
where
    H: Handshake,
    T: AsyncRead + AsyncWrite + Unpin,
{
    until(deadline, complete_stream(handshake, io)).await
}

/// [`drive_messages`] that ends at `deadline`, as [`drive_stream_until`] does.
///
/// # Errors
///
/// [`DriveError::TimedOut`] when `deadline` passes first; otherwise as [`drive_messages`].
pub async fn drive_messages_until<H, T>(
    handshake: H,
    transport: &mut T,
    deadline: Instant,
) -> Result<Established<H::Output>, DriveError<H::Error>>
where
    H: Handshake,
    T: MessageTransport,
{
    until(deadline, drive_messages(handshake, transport)).await
}

/// [`complete_messages`] that ends at `deadline`, as [`drive_stream_until`] does.
///
/// # Errors
///
/// As [`drive_messages_until`].
pub async fn complete_messages_until<H, T>(
    handshake: H,
    transport: &mut T,
    deadline: Instant,
) -> Result<H, DriveError<H::Error>>
where
    H: Handshake,
    T: MessageTransport,
{
    until(deadline, complete_messages(handshake, transport)).await
}

/// [`drive_messages`](crate::drive_messages) over an async message transport.
///
/// # Errors
///
/// [`DriveError`] when the transport fails or the handshake refuses the peer.
pub async fn drive_messages<H, T>(
    handshake: H,
    transport: &mut T,
) -> Result<Established<H::Output>, DriveError<H::Error>>
where
    H: Handshake,
    T: MessageTransport,
{
    complete_messages(handshake, transport)
        .await?
        .finish()
        .map_err(DriveError::Handshake)
}

/// [`complete_messages`](crate::complete_messages) over an async message transport: the
/// handshake once complete, not yet finished.
///
/// # Errors
///
/// As [`drive_messages`].
pub async fn complete_messages<H, T>(
    mut handshake: H,
    transport: &mut T,
) -> Result<H, DriveError<H::Error>>
where
    H: Handshake,
    T: MessageTransport,
{
    loop {
        while let Some(out) = handshake.transmit().map_err(DriveError::Handshake)? {
            transport.send(&out).await.map_err(DriveError::Io)?;
        }
        if handshake.is_complete() {
            return Ok(handshake);
        }
        let message = transport.recv().await.map_err(DriveError::Io)?;
        if let Err(error) = handshake.receive(&message) {
            while let Ok(Some(out)) = handshake.transmit() {
                if transport.send(&out).await.is_err() {
                    break;
                }
            }
            return Err(DriveError::Handshake(error));
        }
    }
}
