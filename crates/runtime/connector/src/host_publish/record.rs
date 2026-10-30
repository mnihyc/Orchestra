//! Numeric lifetime and traffic capture for a host socket, without retaining any traffic content.

use orch_api_contract::ports::{PortConnectionInside, PortConnectionOwner};
use orch_link::ports::PortRecords;
use orch_link_contract::tunnels::Ticket;
use orch_types::{Clock, Id};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Clone)]
pub(super) struct Capture {
    pub records: Arc<PortRecords>,
    pub clock: Arc<dyn Clock>,
}

impl Capture {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            records: Arc::new(PortRecords::new(PortConnectionOwner::Node)),
            clock,
        }
    }

    pub fn accepted(
        &self,
        ticket: &str,
        id: Id,
        peer: std::net::SocketAddr,
        inside: PortConnectionInside,
    ) -> Option<Record> {
        let mut claims = Ticket::parse(ticket).ok()?.claims().port.clone()?;
        claims.host_template.as_ref()?;
        claims.source.address = peer.ip().to_string();
        claims.source.port = peer.port();
        self.records.opened(id, &claims, inside, self.clock.time());
        Some(Record {
            capture: self.clone(),
            environment: claims.environment_id,
            id,
        })
    }
}

pub(super) struct Record {
    capture: Capture,
    environment: Id,
    id: Id,
}
impl Record {
    pub fn bytes(&self, incoming: bool, bytes: usize) {
        // An empty UDP packet is still accepted traffic; an idle TCP read/write never calls this.
        self.capture.records.traffic(
            self.environment,
            self.id,
            incoming,
            bytes,
            false,
            self.capture.clock.time(),
        );
    }
}
impl Drop for Record {
    fn drop(&mut self) {
        self.capture
            .records
            .ended(self.environment, self.id, self.capture.clock.time());
    }
}

pub(super) struct Socket<S> {
    pub socket: S,
    pub record: Record,
}
impl<S: AsyncRead + Unpin> AsyncRead for Socket<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.socket).poll_read(cx, buffer);
        let bytes = buffer.filled().len() - before;
        if matches!(&result, Poll::Ready(Ok(()))) && bytes > 0 {
            self.record.bytes(true, bytes);
        }
        result
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for Socket<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.socket).poll_write(cx, bytes);
        if let Poll::Ready(Ok(bytes)) = &result
            && *bytes > 0
        {
            self.record.bytes(false, *bytes);
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.socket).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.socket).poll_shutdown(cx)
    }
}
