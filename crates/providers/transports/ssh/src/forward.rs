//! Reverse forwards: a port on the host that reaches an address on the Station.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{Status, ToValue, Value};
use ai6::{ForwardInfo, ForwardSpec, TransportForward};
use tokio::runtime::Handle;

use crate::refuse;
use crate::session::{Conn, Link};

pub(crate) struct Forward {
    runtime: Handle,
    conn: Arc<Conn>,
    info: ForwardInfo,
    open: AtomicBool,
}

pub(crate) async fn open(link: &Link, spec: &ForwardSpec) -> Result<Forward, ProviderError> {
    let conn = link.conn().await?;
    let port = conn
        .handle
        .tcpip_forward(spec.remote_host.clone(), u32::from(spec.remote_port))
        .await
        .map_err(|error| {
            refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!("the host did not accept the forward: {error}"),
            )
        })?;
    // A request for port 0 is answered with the port the host chose; a fixed one is not.
    let port = if spec.remote_port == 0 {
        port
    } else {
        u32::from(spec.remote_port)
    };
    let remote_port = u16::try_from(port).map_err(|_| {
        refuse(
            Status::GUATIAO_ERR_INTERNAL,
            format!("the host chose the invalid port {port}"),
        )
    })?;
    conn.forwards.add(port, spec.target.clone());
    Ok(Forward {
        runtime: Handle::current(),
        conn,
        info: ForwardInfo {
            remote_host: spec.remote_host.clone(),
            remote_port,
        },
        open: AtomicBool::new(true),
    })
}

impl Forward {
    fn stop(&self) -> Option<tokio::task::JoinHandle<()>> {
        if !self.open.swap(false, Ordering::AcqRel) {
            return None;
        }
        let port = u32::from(self.info.remote_port);
        self.conn.forwards.remove(port);
        let conn = self.conn.clone();
        let host = self.info.remote_host.clone();
        Some(self.runtime.spawn(async move {
            let _ = conn.handle.cancel_tcpip_forward(host, port).await;
        }))
    }
}

impl TransportForward for Forward {
    fn info(&self) -> Result<Value, ProviderError> {
        self.info
            .to_value()
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
    }

    fn close(&self) -> Result<(), ProviderError> {
        if let Some(cancel) = self.stop() {
            // Wait for the host to stop listening, so a caller may reuse the port.
            let _ = self.runtime.block_on(cancel);
        }
        Ok(())
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
