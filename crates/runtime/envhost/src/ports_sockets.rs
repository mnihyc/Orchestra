//! Inside TCP and UDP sockets opened only after fresh scoped port ownership checks.
//!
//! Caller admission and carrier capacity belong to the carrying layer. This last step selects
//! the common current owned binding and checks it again before exposing any program bytes.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use orch_api_contract::ports::PortTransport;
use orch_link_contract::tunnels::OPEN_TIMEOUT_SECS;
use tokio::net::{TcpStream, UdpSocket};

use crate::ports::{InternalPort, OwnedPort, PortReadError, Ports};

/// Why a current owned local connection could not be made.
#[derive(Debug, thiserror::Error)]
pub enum PortSocketError {
    /// The current scope cannot prove the target.
    #[error(transparent)]
    Ownership(#[from] PortReadError),
    /// The local socket could not connect, send or receive.
    #[error("the local program socket is unavailable: {0}")]
    Socket(#[from] std::io::Error),
    /// The ordinary port-open deadline elapsed.
    #[error("the local program did not connect before the open deadline")]
    Timeout,
    /// The native reader task ended unexpectedly.
    #[error("the native socket reader is unavailable")]
    Reader,
}

async fn current(
    ports: &Ports,
    port: u16,
    transport: PortTransport,
) -> Result<OwnedPort, PortSocketError> {
    let ports = ports.clone();
    tokio::task::spawn_blocking(move || ports.owned(port, transport))
        .await
        .map_err(|_| PortSocketError::Reader)?
        .map_err(Into::into)
}

async fn recheck(ports: &Ports, proof: OwnedPort) -> Result<(), PortSocketError> {
    let ports = ports.clone();
    tokio::task::spawn_blocking(move || ports.recheck(&proof))
        .await
        .map_err(|_| PortSocketError::Reader)?
        .map_err(Into::into)
}

/// Connects to the current owned TCP program, with no caller bytes sent under an earlier proof.
pub async fn tcp(ports: &Ports, port: u16) -> Result<TcpStream, PortSocketError> {
    let proof = current(ports, port, PortTransport::Tcp).await?;
    let stream = tokio::time::timeout(
        Duration::from_secs(u64::from(OPEN_TIMEOUT_SECS)),
        TcpStream::connect(proof.address),
    )
    .await
    .map_err(|_| PortSocketError::Timeout)??;
    recheck(ports, proof).await?;
    Ok(stream)
}

/// One outside sender's distinct connected inside UDP socket.
#[derive(Debug)]
pub struct PortUdp {
    ports: Arc<Ports>,
    target_port: u16,
    target_address: SocketAddr,
    socket: UdpSocket,
    _internal: InternalPort,
}

impl PortUdp {
    /// The actual peer address and port the local program sees for this sender.
    pub fn inside_peer(&self) -> Result<SocketAddr, PortSocketError> {
        Ok(self.socket.local_addr()?)
    }

    async fn owned_now(&self) -> Result<(), PortSocketError> {
        let proof = current(&self.ports, self.target_port, PortTransport::Udp).await?;
        if proof.address != self.target_address {
            return Err(PortReadError::Changed.into());
        }
        Ok(())
    }

    /// Sends one datagram only to the presently owned program, without a caller-selected destination.
    pub async fn send(&self, payload: &[u8]) -> Result<usize, PortSocketError> {
        self.owned_now().await?;
        Ok(self.socket.send(payload).await?)
    }

    /// Receives a whole program datagram, checking current ownership before handing it onward.
    pub async fn recv(&self, buffer: &mut [u8]) -> Result<usize, PortSocketError> {
        self.owned_now().await?;
        let received = self.socket.recv(buffer).await?;
        self.owned_now().await?;
        Ok(received)
    }
}

/// Creates one sender's inside UDP socket, keeping it Internal for its lifetime.
pub async fn udp(ports: Arc<Ports>, port: u16) -> Result<PortUdp, PortSocketError> {
    let proof = current(&ports, port, PortTransport::Udp).await?;
    let local = if proof.address.is_ipv4() {
        "127.0.0.1:0"
    } else {
        "[::1]:0"
    };
    let socket = UdpSocket::bind(local).await?;
    let internal = ports.internal_port(socket.local_addr()?.port(), PortTransport::Udp);
    socket.connect(proof.address).await?;
    recheck(&ports, proof.clone()).await?;
    Ok(PortUdp {
        ports,
        target_port: port,
        target_address: proof.address,
        socket,
        _internal: internal,
    })
}
