//! The tunnels other machines open to this one, handed to whatever serves their kind, and the opening of the ones this machine needs.
//!
//! The link gives a machine the tunnels that others open to it as one queue, and gives it out once. Several
//! roles serve tunnels (the provider role serves `llm`, the environment role `exec`, `mcp` and `pty`), so the
//! connector takes the queue once and hands each tunnel to the server registered for its kind
//! ([`Tunnels::register`]); a kind nobody serves is refused at once with `no_such_service`, and the machine at
//! the other end is told so instead of waiting for an answer. The tunnels a role opens itself are opened here
//! too ([`Tunnels::open`]), with the ticket the system signed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use orch_link::machine::MachineLink;
use orch_link::tunnels::channel::{IncomingTunnel, OpenError, Tunnel};
use orch_link_contract::tunnels::{OpenErrorCode, TunnelKind};
use tokio::task::JoinHandle;

/// Something that serves the tunnels of one kind.
#[async_trait]
pub trait TunnelServer: Send + Sync {
    /// Serves a tunnel that another machine asks to open here: it accepts it or refuses it.
    async fn serve(&self, tunnel: IncomingTunnel);
}

/// The tunnels of a connector.
pub struct Tunnels {
    link: MachineLink,
    servers: Mutex<HashMap<TunnelKind, Arc<dyn TunnelServer>>>,
}

impl std::fmt::Debug for Tunnels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnels").finish_non_exhaustive()
    }
}

impl Tunnels {
    /// The tunnels of the machine whose link this is.
    #[must_use]
    pub fn new(link: MachineLink) -> Arc<Self> {
        Arc::new(Self {
            link,
            servers: Mutex::new(HashMap::new()),
        })
    }

    fn servers(&self) -> MutexGuard<'_, HashMap<TunnelKind, Arc<dyn TunnelServer>>> {
        self.servers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers what serves the tunnels of a kind, in place of what did.
    pub fn register(&self, kind: TunnelKind, server: Arc<dyn TunnelServer>) {
        self.servers().insert(kind, server);
    }

    /// Opens a tunnel with a ticket the system signed, and waits for the other end to take it.
    pub async fn open(&self, ticket: &str) -> Result<Tunnel, OpenError> {
        self.link.open_tunnel(ticket).await
    }

    /// Opens one socket accepted by the node, without allowing it to supply an Orch identity.
    pub async fn open_host_port(
        &self,
        ticket: &str,
        peer: orch_link_contract::ports::HostPortPeer,
    ) -> Result<Tunnel, OpenError> {
        self.link.open_host_port(ticket, peer).await
    }

    /// Current negotiated capacities for this node's carrier.
    #[must_use]
    pub fn port_limits(&self) -> orch_link_contract::version::PortLimits {
        self.link.port_limits()
    }

    /// Reserves a pending host datagram without a second independent per-sender byte store.
    pub fn reserve_port_datagram(
        &self,
        environment: orch_types::Id,
        payload: bytes::Bytes,
    ) -> Option<orch_link::ports::Datagram> {
        self.link.reserve_port_datagram(environment, payload)
    }

    /// Serves the host role's bounded, memory-only record without a command journal or spool.
    pub fn serve_port_records(&self, records: Arc<orch_link::ports::PortRecords>) {
        self.link.serve_port_records(records);
    }

    /// Takes the queue of incoming tunnels, once, and serves each tunnel with the server of its kind until
    /// the returned task is stopped.
    #[must_use]
    pub fn run(self: &Arc<Self>) -> JoinHandle<()> {
        let tunnels = Arc::clone(self);
        let mut incoming = self.link.incoming_tunnels();
        tokio::spawn(async move {
            while let Some(next) = incoming.next().await {
                let server = tunnels.servers().get(&next.kind).cloned();
                match server {
                    Some(server) => {
                        tokio::spawn(async move { server.serve(next).await });
                    }
                    None => next.refuse(
                        OpenErrorCode::NoSuchService,
                        "this machine does not serve tunnels of that kind",
                    ),
                }
            }
        })
    }
}
