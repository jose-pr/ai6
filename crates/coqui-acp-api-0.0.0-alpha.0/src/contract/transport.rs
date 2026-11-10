//! A real ACP client, `coqui_acp::Client`, over a connection: the channel as a line transport.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use std::io::{self, Read, Write};
use std::sync::mpsc;

use guatiao::Status;
use guatiao::library::{Object, ProviderError};
use guatiao_channel::{Channel, ChannelSink};

use super::host::{PATIENCE, handed};
use crate::{AcpHarness, StartRequest};

/// The sink: hands each message to the transport's reader.
struct Forward(Option<mpsc::Sender<Vec<u8>>>);

impl ChannelSink for Forward {
    fn receive(&mut self, bytes: &[u8]) -> Result<(), ProviderError> {
        self.0
            .as_ref()
            .and_then(|tx| tx.send(bytes.to_vec()).ok())
            .ok_or_else(|| ProviderError::new(Status::GUATIAO_ERR_GONE, "the client is gone"))
    }

    fn closed(&mut self, _status: u32, _reason: &str) {
        self.0 = None;
    }
}

/// The agent's messages as lines.
struct Inbox {
    rx: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl Read for Inbox {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            let Ok(mut message) = self.rx.recv() else {
                return Ok(0);
            };
            message.push(b'\n');
            self.pending = message;
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

/// The client's lines, each sent as one message.
struct Outbox {
    channel: Object<dyn Channel>,
    line: Vec<u8>,
}

impl Write for Outbox {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.line.extend_from_slice(buf);
        while let Some(end) = self.line.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.line.drain(..=end).collect();
            let message = line.trim_ascii();
            if !message.is_empty() {
                self.channel
                    .send(message)
                    .map_err(|e| io::Error::other(e.message().to_owned()))?;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn serves_the_client(harness: &dyn AcpHarness, cwd: &str) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let (_shared, spawner, _unused_sink) = handed();
    let channel = harness
        .start(
            StartRequest::new(),
            spawner,
            Forward(Some(tx)).into_object(),
        )
        .map_err(|e| format!("start refused: {}", e.message()))?;
    let inbox = Inbox {
        rx,
        pending: Vec::new(),
    };
    let outbox = Outbox {
        channel,
        line: Vec::new(),
    };
    let (transport, _) =
        coqui_acp::lines::over_io(inbox, outbox, 0, coqui_acp::lines::DEFAULT_MAX_LINE)
            .map_err(|e| format!("the transport's threads did not start: {e}"))?;
    let session = coqui_acp::Client::builder(cwd)
        .initialize_timeout(Some(PATIENCE))
        .request_timeout(Some(PATIENCE))
        .prompt("hello")
        .connect(transport)
        .map_err(|e| format!("the client could not open a session: {e}"))?;
    let mut text = String::new();
    let outcome = loop {
        match session.next_event(PATIENCE) {
            Some(coqui_acp::Event::Text(more)) => text.push_str(&more),
            Some(coqui_acp::Event::Completed { .. }) => break Ok(()),
            Some(coqui_acp::Event::Failed(why)) => break Err(format!("the turn failed: {why}")),
            Some(_) => {}
            None => break Err("the turn did not end".to_owned()),
        }
    };
    session.close();
    outcome?;
    if text == "echo: hello" {
        Ok(())
    } else {
        Err(format!("the client read {text:?}"))
    }
}
