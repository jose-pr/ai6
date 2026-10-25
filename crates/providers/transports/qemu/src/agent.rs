//! The QEMU guest agent protocol: one JSON object per line over a socket.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ai6::guatiao::Status;
use ai6::guatiao::library::ProviderError;
use serde_json::{Value, json};

use crate::refuse;

/// A byte stream with a read timeout.
trait Stream: Read + Write + Send {
    fn set_timeout(&self, timeout: Duration) -> std::io::Result<()>;
}

impl Stream for TcpStream {
    fn set_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(timeout))?;
        self.set_write_timeout(Some(timeout))
    }
}

#[cfg(unix)]
impl Stream for std::os::unix::net::UnixStream {
    fn set_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(timeout))?;
        self.set_write_timeout(Some(timeout))
    }
}

/// Where the guest agent listens.
#[derive(Debug, Clone)]
pub(crate) enum Endpoint {
    Tcp(String),
    #[cfg_attr(
        not(unix),
        allow(dead_code, reason = "a Unix socket needs a Unix host")
    )]
    Unix(String),
}

/// One guest agent, reached on demand and again after a failure.
pub(crate) struct Agent {
    endpoint: Endpoint,
    timeout: Duration,
    connection: Mutex<Option<BufReader<Box<dyn Stream>>>>,
    next_id: AtomicU64,
}

impl Agent {
    pub(crate) fn new(endpoint: Endpoint, timeout: Duration) -> Self {
        Self {
            endpoint,
            timeout,
            connection: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }

    /// Sends `command` and answers its `return` value. Calls are serialized: the agent
    /// answers one at a time.
    pub(crate) fn call(&self, command: &str, arguments: Value) -> Result<Value, ProviderError> {
        let mut slot = self
            .connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(self.connect()?);
        }
        let connection = slot.as_mut().expect("connected above");
        let outcome = exchange(connection, command, arguments);
        if let Err(Failure::Io(_)) = &outcome {
            // The next call starts a new connection and synchronizes again.
            *slot = None;
        }
        outcome.map_err(|failure| match failure {
            Failure::Io(why) => refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!("guest agent connection: {why}"),
            ),
            Failure::Agent(why) => {
                refuse(Status::GUATIAO_ERR_INTERNAL, format!("{command}: {why}"))
            }
        })
    }

    fn connect(&self) -> Result<BufReader<Box<dyn Stream>>, ProviderError> {
        let stream: Box<dyn Stream> = match &self.endpoint {
            Endpoint::Tcp(address) => Box::new(
                TcpStream::connect(address).map_err(|error| connect_error(address, &error))?,
            ),
            #[cfg(unix)]
            Endpoint::Unix(path) => Box::new(
                std::os::unix::net::UnixStream::connect(path)
                    .map_err(|error| connect_error(path, &error))?,
            ),
            #[cfg(not(unix))]
            Endpoint::Unix(path) => {
                return Err(refuse(
                    Status::GUATIAO_ERR_UNSUPPORTED,
                    format!("{path}: a Unix socket needs a Unix host"),
                ));
            }
        };
        stream
            .set_timeout(self.timeout)
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        let mut connection = BufReader::new(stream);

        // The agent may hold the tail of an earlier request: 0xFF makes it discard that, and a
        // sync with a fresh ID tells our reply from any left over.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) ^ u64::from(std::process::id()) << 20;
        connection
            .get_mut()
            .write_all(&[0xFF])
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        let reply = exchange(&mut connection, "guest-sync-delimited", json!({ "id": id }))
            .map_err(|failure| match failure {
                Failure::Io(why) | Failure::Agent(why) => refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    format!("the guest agent did not answer a sync: {why}"),
                ),
            })?;
        if reply.as_u64() != Some(id) {
            return Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                "the guest agent answered a sync with the wrong ID",
            ));
        }
        Ok(connection)
    }
}

fn connect_error(address: &str, error: &std::io::Error) -> ProviderError {
    let status = if error.kind() == std::io::ErrorKind::NotFound {
        Status::GUATIAO_ERR_NOT_FOUND
    } else {
        Status::GUATIAO_ERR_INTERNAL
    };
    refuse(status, format!("{address}: {error}"))
}

enum Failure {
    /// The connection failed or the agent said something unreadable.
    Io(String),
    /// The agent answered with an error.
    Agent(String),
}

fn exchange(
    connection: &mut BufReader<Box<dyn Stream>>,
    command: &str,
    arguments: Value,
) -> Result<Value, Failure> {
    let mut line = json!({ "execute": command, "arguments": arguments }).to_string();
    line.push('\n');
    connection
        .get_mut()
        .write_all(line.as_bytes())
        .map_err(|error| Failure::Io(error.to_string()))?;
    loop {
        let mut reply = Vec::new();
        let count = connection
            .read_until(b'\n', &mut reply)
            .map_err(|error| Failure::Io(error.to_string()))?;
        if count == 0 {
            return Err(Failure::Io("the agent closed the connection".to_owned()));
        }
        // A delimited reply starts with 0xFF bytes, which are not UTF-8.
        let start = reply
            .iter()
            .position(|&byte| byte != 0xFF)
            .unwrap_or(reply.len());
        let text = String::from_utf8_lossy(&reply[start..]);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        if let Some(value) = message.get("return") {
            return Ok(value.clone());
        }
        if let Some(error) = message.get("error") {
            let why = error
                .get("desc")
                .and_then(Value::as_str)
                .unwrap_or("the agent reported an error");
            return Err(Failure::Agent(why.to_owned()));
        }
        // Anything else (an event) is not the answer.
    }
}
