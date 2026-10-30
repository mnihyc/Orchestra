//! Stateless machine gateway and tunnel relay.
//!
//! A gateway is where machines connect. It holds their sessions and nothing
//! that has to outlast it: whether a machine may have a session is asked of
//! the system over one connection that the gateway dials itself (the
//! backplane), what a machine says for the system is passed up that
//! connection and what the system says for a machine is passed down it, and
//! the tunnels between two machines are carried without the system ever
//! seeing them. A gateway that is stopped and started again has lost
//! nothing: its machines come back, the system is told which of them it has,
//! and the streams, commands and tunnels they carry go on from where they
//! stood.
//!
//! The pieces:
//!
//! * [`listeners`] takes connections on the two ports, the pinned one that
//!   asks machines for their certificate and the one behind the public edge
//!   that does not, and turns them into sessions or into requests for
//!   [`content`].
//! * [`authentication`] asks the system over the [`backplane`] whether a
//!   session may start, and knows who a request over HTTPS comes from.
//! * [`sessions`] keeps the sessions that stand, one for each machine, and
//!   replaces one by a newer; [`routing`] moves frames between them and the
//!   backplane; [`relay`] holds the tunnels.
//! * [`liveness`] is what runs on the clock: the tunnels' time, and the end
//!   of everything when the gateway is told to stop.
//!
//! Nothing here reads a database or a file at run time. The certificates it
//! shows and trusts are given when it is made.

#![forbid(unsafe_code)]

pub mod authentication;
pub mod backplane;
pub mod content;
pub mod listeners;
pub mod liveness;
pub mod relay;
pub mod routing;
pub mod service_renewal;
pub mod sessions;

#[cfg(test)]
mod forwarded_address_gateway;

use crate::backplane::BackplaneHandle;
use crate::relay::{ApiBridge, SharedRelay};
use crate::sessions::Sessions;
use orch_config::ServerConfig;
use orch_link::session::GatewayConfig;
use orch_link::tunnels::relay::Relay;
use orch_link_contract::version::{ConnectorVersions, Limits, MAX_CHANNELS};
use orch_pki::PkiError;
use orch_pki::certificates::{Certificate, ServiceName};
use orch_pki::service_renew::LiveIdentity;
use orch_pki::verification::{
    ServerTrust, TlsIdentity, pinned_client_config_live, plain_server_config_live,
};
use orch_types::Clock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// How long what is under way is let finish when the gateway is told to
/// stop.
const DRAIN: Duration = Duration::from_secs(10);
/// The oldest connector until the system says otherwise: the first release,
/// which spoke this protocol.
const FIRST_CONNECTOR_VERSION: &str = "0.1.0";

/// What a gateway is configured with, apart from its certificates.
#[derive(Debug, Clone)]
pub struct Settings {
    /// This gateway's name among several.
    pub gateway_id: String,
    /// The gateway's own version, which it tells the system.
    pub version: String,
    /// Where the system's backplane is.
    pub backplane_url: String,
    /// How often a side of a session says it is alive, in seconds.
    pub heartbeat_secs: u32,
    /// The limits of a session, until the system states its own.
    pub limits: Limits,
    /// How long a tunnel whose session dropped may be taken up again.
    pub tunnel_resume_grace: Duration,
    /// How long after it ran out a machine's certificate is still heard, for
    /// the one request that renews it, in days.
    pub expired_renew_grace_days: u64,
    /// Proxy ranges allowed to describe callers on the listener behind the edge.
    pub trusted_proxy_cidrs: Vec<String>,
}

impl Settings {
    /// The settings a deployment's configuration gives.
    #[must_use]
    pub fn from_config(config: &ServerConfig, version: &str) -> Self {
        let defaults = Limits::default();
        Self {
            gateway_id: config.gateway.gateway_id.clone(),
            version: version.to_owned(),
            backplane_url: config.gateway.backplane_url.clone(),
            heartbeat_secs: u32::try_from(config.link.heartbeat_secs)
                .unwrap_or(orch_link_contract::session::HEARTBEAT_SECS),
            limits: Limits {
                max_frame_bytes: u32::try_from(config.link.max_frame_bytes)
                    .unwrap_or(defaults.max_frame_bytes),
                tunnel_window_bytes: u32::try_from(config.link.tunnel_window_bytes)
                    .unwrap_or(defaults.tunnel_window_bytes),
                max_channels: MAX_CHANNELS,
                ports: orch_link::ports::limits_of(&config.link),
            },
            tunnel_resume_grace: Duration::from_secs(config.link.tunnel_resume_grace_secs),
            expired_renew_grace_days: config.pki.expired_renew_grace_days,
            trusted_proxy_cidrs: config.api.trusted_proxy_cidrs.clone(),
        }
    }
}

/// What a gateway is made from.
pub struct GatewayParts {
    /// The settings.
    pub settings: Settings,
    /// The certificate and key the gateway shows, as a service of the
    /// system.
    pub identity: TlsIdentity,
    /// The system's authority, which machines' and the system's
    /// certificates must chain to.
    pub authority: Certificate,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
}

/// Why a gateway could not be made.
#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    /// A certificate or a key cannot be used.
    #[error("{0}")]
    Tls(#[from] PkiError),
}

/// What the system last said about itself.
#[derive(Debug, Clone)]
pub(crate) struct ApiState {
    pub(crate) limits: Limits,
    pub(crate) versions: ConnectorVersions,
}

pub(crate) struct Inner {
    pub(crate) settings: Settings,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) authority: Certificate,
    pub(crate) sessions: Sessions,
    pub(crate) relay: SharedRelay,
    pub(crate) backplane: BackplaneHandle,
    pub(crate) api: RwLock<ApiState>,
    /// The certificate the gateway shows now, which its renewal replaces: its listeners and its connections to
    /// the system show it from their next handshake.
    pub(crate) identity: Arc<LiveIdentity>,
    pub(crate) pinned_tls: Arc<rustls::ServerConfig>,
    pub(crate) edge_tls: Arc<rustls::ServerConfig>,
    pub(crate) upstream_tls: Arc<rustls::ClientConfig>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) bytes_relayed: AtomicU64,
    /// When this gateway was made, which is when its process started: its heartbeat says it.
    pub(crate) started_at: orch_types::Time,
}

/// A gateway. Cheap to clone; every clone is the same gateway.
#[derive(Clone)]
pub struct Gateway {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Gateway {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gateway")
            .field("id", &self.inner.settings.gateway_id)
            .finish_non_exhaustive()
    }
}

impl Gateway {
    /// Makes a gateway. Nothing runs until [`Gateway::run`].
    pub fn new(parts: GatewayParts) -> Result<Self, GatewayError> {
        let GatewayParts {
            settings,
            mut identity,
            authority,
            clock,
        } = parts;
        // A machine that is enrolling knows nothing of the system but the pin
        // its token carries, and knows the authority by what the gateway
        // sends along with its own certificate: so the authority goes along.
        if !identity
            .chain
            .iter()
            .any(|certificate| certificate.fingerprint() == authority.fingerprint())
        {
            identity.chain.push(authority.clone());
        }
        let backplane = BackplaneHandle::new();
        let mut relay = Relay::new(settings.limits, Arc::clone(&clock));
        relay.set_grace(settings.tunnel_resume_grace);
        relay.set_api(Arc::new(ApiBridge::new(backplane.clone())));
        let identity = LiveIdentity::new(&identity)?;
        let pinned_tls = listeners::pinned_server_config(
            Arc::clone(&identity),
            &authority,
            settings.expired_renew_grace_days,
            Arc::clone(&clock),
        )?;
        let edge_tls = plain_server_config_live(Arc::clone(&identity), Arc::clone(&clock))?;
        let upstream_tls = pinned_client_config_live(
            ServerTrust::Authority(Box::new(authority.clone())),
            Some(ServiceName::Api),
            Arc::clone(&identity),
            Arc::clone(&clock),
        )?;
        let api = ApiState {
            limits: settings.limits,
            versions: ConnectorVersions {
                min: FIRST_CONNECTOR_VERSION.to_owned(),
                recommended: FIRST_CONNECTOR_VERSION.to_owned(),
            },
        };
        let started_at = clock.time();
        Ok(Self {
            inner: Arc::new(Inner {
                started_at,
                settings,
                clock,
                authority,
                sessions: Sessions::default(),
                relay: SharedRelay::new(relay),
                backplane,
                api: RwLock::new(api),
                identity,
                pinned_tls,
                edge_tls,
                upstream_tls,
                shutdown: CancellationToken::new(),
                bytes_relayed: AtomicU64::new(0),
            }),
        })
    }

    /// The certificate the gateway shows now: the one it started with, or the one it renewed it with.
    #[must_use]
    pub fn certificate(&self) -> Option<Certificate> {
        self.inner.identity.certificate()
    }

    /// Runs the gateway on two listeners until [`Gateway::shutdown`]: the
    /// pinned one, which asks machines for their certificate, and the one
    /// behind the public edge, which does not.
    pub async fn run(&self, pinned: TcpListener, edge: TcpListener) {
        let mut tasks = tokio::task::JoinSet::new();
        tasks.spawn(backplane::keep(self.clone()));
        tasks.spawn(liveness::run(self.clone()));
        tasks.spawn(service_renewal::keep(self.clone()));
        tasks.spawn(listeners::serve(
            self.clone(),
            pinned,
            listeners::Kind::Pinned,
        ));
        tasks.spawn(listeners::serve(self.clone(), edge, listeners::Kind::Edge));
        self.inner.shutdown.cancelled().await;
        let drained =
            tokio::time::timeout(DRAIN, async { while tasks.join_next().await.is_some() {} }).await;
        if drained.is_err() {
            tasks.abort_all();
        }
    }

    /// Tells the gateway to stop: its listeners close, its sessions end and
    /// the machines connect again, to this gateway when it is back or to
    /// another.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// Whether the connection to the system is up.
    #[must_use]
    pub fn backplane_up(&self) -> bool {
        self.inner.backplane.is_up()
    }

    /// Waits until the connection to the system is up.
    pub async fn wait_for_backplane(&self) {
        self.inner.backplane.wait_up().await;
    }

    /// How many sessions stand.
    #[must_use]
    pub fn sessions_held(&self) -> usize {
        self.inner.sessions.len()
    }

    /// Cuts a machine's connection as a failing line would, without a word
    /// to the machine, which then connects again. Returns whether the machine
    /// had a session here.
    pub fn drop_connection(&self, machine: orch_types::Id) -> bool {
        self.inner
            .sessions
            .sender_of(machine)
            .map(|sender| sender.end())
            .is_some()
    }

    /// How many bytes of tunnel data the gateway has carried.
    #[must_use]
    pub fn bytes_relayed(&self) -> u64 {
        self.inner.bytes_relayed.load(Ordering::Relaxed)
    }

    /// The clock the gateway reads.
    pub(crate) fn clock(&self) -> &Arc<dyn Clock> {
        &self.inner.clock
    }

    /// What a session is accepted under, as the system last stated it.
    pub(crate) fn session_config(&self) -> GatewayConfig {
        let api = self
            .inner
            .api
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        GatewayConfig {
            gateway_id: self.inner.settings.gateway_id.clone(),
            limits: api.limits,
            versions: api.versions,
            heartbeat_secs: self.inner.settings.heartbeat_secs,
            authority: self.inner.authority.clone(),
        }
    }
}
