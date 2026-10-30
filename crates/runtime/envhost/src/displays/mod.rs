//! Display registrations reuse browser windows and keep adopted passwords off public reports.

pub mod rfb;
pub mod windows;

use crate::browser_window::Windows;
use crate::environment::{Report, Reports};
use orch_link_contract::browser::ScreenLookDisplay;
use orch_link_contract::display::{DisplayAdoptResult, DisplayOpenResult};
use orch_types::Id;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    Arc, PoisonError, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Display failures never include a supplied password or server-supplied error text.
#[derive(Debug, thiserror::Error)]
pub enum DisplayError {
    /// The local policy is off.
    #[error("displays are switched off on this machine")]
    Disabled,
    /// The endpoint is not local.
    #[error("the endpoint must be a loopback port or a local socket")]
    NotLocal,
    /// It did not speak a supported RFB protocol.
    #[error("the endpoint is not a supported VNC server")]
    NotADisplay,
    /// Classic VNC authentication failed.
    #[error("the VNC server refused authentication")]
    BadPassword,
    /// Nothing can open the requested screen.
    #[error("no display server is available")]
    Unsupported,
    /// No display is registered under the identifier.
    #[error("the display no longer exists here")]
    NotFound,
    /// The program did not answer within the bound.
    #[error("the display did not answer in time")]
    Unreachable,
    /// A local operation failed; its raw error remains private.
    #[error("the local display operation failed")]
    Io(#[from] std::io::Error),
    /// The shared virtual-screen launcher refused the request.
    #[error("the virtual screen could not be opened")]
    Window(#[from] crate::browsers::BrowserError),
}

impl DisplayError {
    /// The declared machine refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Disabled => "not_allowed",
            Self::NotLocal => "not_local",
            Self::NotADisplay => "not_a_display",
            Self::BadPassword => "bad_password",
            Self::Unsupported => "unsupported",
            Self::NotFound => "not_found",
            Self::Unreachable | Self::Io(_) => "not_reachable",
            Self::Window(error) => error.code(),
        }
    }
}

struct Password(Vec<u8>);
impl Drop for Password {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

struct Registration {
    endpoint: String,
    target: rfb::Endpoint,
    password: Option<Password>,
    stopped: CancellationToken,
    _internal: Option<crate::ports::InternalPort>,
}

/// One display connection, authenticated locally, and its lifetime gate.
pub struct Connection {
    /// Its protocol transport after security.
    pub authenticated: rfb::Authenticated,
    /// Cancelled when the display closes or the machine policy turns off.
    pub stopped: CancellationToken,
}

/// Display registrations of one environment, sharing its virtual screens with browsers.
pub struct Displays {
    environment: Id,
    root: PathBuf,
    windows: Arc<Windows>,
    reports: Reports,
    allowed: AtomicBool,
    adopted: Mutex<BTreeMap<Id, Arc<Registration>>>,
    owned_stops: Mutex<BTreeMap<Id, CancellationToken>>,
    monitored: AtomicBool,
    ports: RwLock<Option<crate::ports::Ports>>,
}

impl Displays {
    /// Makes a registry without starting a program or opening a socket.
    pub fn new(
        environment: Id,
        root: PathBuf,
        windows: Arc<Windows>,
        reports: Reports,
        managed: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            environment,
            root,
            windows,
            reports,
            allowed: AtomicBool::new(managed),
            adopted: Mutex::new(BTreeMap::new()),
            owned_stops: Mutex::new(BTreeMap::new()),
            monitored: AtomicBool::new(false),
            ports: RwLock::new(None),
        })
    }

    /// Refuses unless the machine permits displays.
    pub fn require_allowed(&self) -> Result<(), DisplayError> {
        if self.allowed.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(DisplayError::Disabled)
        }
    }

    /// Protects owned control listeners and future adopted targets from generic port forwarding.
    pub async fn protect_ports(&self, ports: crate::ports::Ports) {
        *self.ports.write().unwrap_or_else(PoisonError::into_inner) = Some(ports.clone());
        self.windows.protect_ports(ports).await;
    }

    /// Reattaches verified original screens without starting another X server.
    pub async fn recover_owned(self: &Arc<Self>) {
        let live = self.windows.list().await;
        if live.is_empty() {
            return;
        }
        for display in live {
            self.owned_stops
                .lock()
                .await
                .entry(display.display_id)
                .or_default();
        }
        self.monitor();
    }

    /// Applies current local policy and ends every owned screen when it is withdrawn.
    pub async fn set_allowed(&self, allowed: bool) {
        self.allowed.store(allowed, Ordering::Release);
        if !allowed {
            for registration in self.adopted.lock().await.values() {
                registration.stopped.cancel();
            }
            for stop in self.owned_stops.lock().await.values() {
                stop.cancel();
            }
            for display in self.list_unchecked().await {
                self.close_reason(display.display_id, "switched_off").await;
            }
        }
    }

    /// Opens the same virtual-screen implementation used by browser windows.
    pub async fn open(
        self: &Arc<Self>,
        id: Id,
        geometry: &str,
    ) -> Result<DisplayOpenResult, DisplayError> {
        self.require_allowed()?;
        let (endpoint, x_display) = self.windows.open(id, geometry).await?;
        self.owned_stops.lock().await.entry(id).or_default();
        self.monitor();
        if let Err(error) = self.require_allowed() {
            self.close_reason(id, "switched_off").await;
            return Err(error);
        }
        Ok(DisplayOpenResult {
            service: format!("rfb/{id}"),
            endpoint,
            x_display,
        })
    }

    /// Checks an agent's endpoint and credential before registering it for viewers.
    pub async fn adopt(
        self: &Arc<Self>,
        id: Id,
        endpoint: &str,
        password: Option<Vec<u8>>,
    ) -> Result<DisplayAdoptResult, DisplayError> {
        self.require_allowed()?;
        let target = rfb::Endpoint::parse(endpoint, &self.root)?;
        let ports = self
            .ports
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let internal = match (&target, ports) {
            (rfb::Endpoint::Port(port), Some(ports)) => {
                Some(ports.internal_port(*port, orch_api_contract::ports::PortTransport::Tcp))
            }
            _ => None,
        };
        let password = password.map(Password);
        let authenticated =
            rfb::authenticate(&target, password.as_ref().map(|p| p.0.as_slice())).await?;
        let (name, geometry) = rfb::describe(authenticated).await?;
        self.require_allowed()?;
        let mut adopted = self.adopted.lock().await;
        if let Some(previous) = adopted.insert(
            id,
            Arc::new(Registration {
                endpoint: endpoint.to_owned(),
                target,
                password,
                stopped: CancellationToken::new(),
                _internal: internal,
            }),
        ) {
            previous.stopped.cancel();
        }
        let registration = adopted.get(&id).cloned();
        drop(adopted);
        if let Some(registration) = registration {
            self.monitor_adopted(id, registration);
        }
        Ok(DisplayAdoptResult {
            service: format!("rfb/{id}"),
            name,
            geometry: Some(geometry),
        })
    }

    /// Connects separately for each viewer and answers security inside the environment.
    pub async fn connect(&self, id: Id) -> Result<Connection, DisplayError> {
        self.require_allowed()?;
        if let Some(display) = self.adopted.lock().await.get(&id).cloned() {
            let authenticated = rfb::authenticate(
                &display.target,
                display.password.as_ref().map(|p| p.0.as_slice()),
            )
            .await?;
            self.require_allowed()?;
            return Ok(Connection {
                authenticated,
                stopped: display.stopped.clone(),
            });
        }
        let endpoint = self
            .windows
            .list()
            .await
            .into_iter()
            .find(|d| d.display_id == id)
            .ok_or(DisplayError::NotFound)?
            .endpoint;
        let target = rfb::Endpoint::parse(&endpoint, &self.root)?;
        let authenticated = rfb::authenticate(&target, None).await?;
        self.require_allowed()?;
        let stopped = self.owned_stops.lock().await.entry(id).or_default().clone();
        Ok(Connection {
            authenticated,
            stopped,
        })
    }

    async fn list_unchecked(&self) -> Vec<ScreenLookDisplay> {
        let mut displays = self.windows.list().await;
        displays.extend(
            self.adopted
                .lock()
                .await
                .iter()
                .map(|(id, display)| ScreenLookDisplay {
                    display_id: *id,
                    endpoint: display.endpoint.clone(),
                }),
        );
        displays
    }

    /// Reconciles both browser-created and announced services on the existing screen look.
    pub async fn list(&self) -> Vec<ScreenLookDisplay> {
        if self.require_allowed().is_err() {
            return Vec::new();
        }
        self.list_unchecked().await
    }

    fn monitor(self: &Arc<Self>) {
        if self.monitored.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            // Matches browser process maintenance; no pictures or server reconnections are required.
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let Some(registry) = weak.upgrade() else {
                    break;
                };
                let live = registry.windows.list().await;
                let ended: Vec<_> = registry
                    .owned_stops
                    .lock()
                    .await
                    .keys()
                    .filter(|id| !live.iter().any(|d| d.display_id == **id))
                    .copied()
                    .collect();
                for id in ended {
                    registry.close_reason(id, "exited").await;
                }
            }
        });
    }

    fn monitor_adopted(self: &Arc<Self>, id: Id, registration: Arc<Registration>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            // Five seconds matches process liveness without retaining an extra viewer or a picture.
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            tick.tick().await;
            loop {
                tokio::select! {_=tick.tick()=>{},_=registration.stopped.cancelled()=>break}
                let Some(registry) = weak.upgrade() else {
                    break;
                };
                let probe = rfb::authenticate(
                    &registration.target,
                    registration.password.as_ref().map(|p| p.0.as_slice()),
                );
                let result =
                    tokio::select! {result=probe=>result,_=registration.stopped.cancelled()=>break};
                if result.is_err() && !registration.stopped.is_cancelled() {
                    registry.close_reason(id, "exited").await;
                    break;
                }
            }
        });
    }

    /// Stops only an owned screen; an announced process is never signalled.
    pub async fn close(&self, id: Id) {
        self.close_reason(id, "closed_by_user").await;
    }

    async fn close_reason(&self, id: Id, reason: &str) {
        let adopted = self.adopted.lock().await.remove(&id);
        if let Some(display) = adopted {
            display.stopped.cancel();
        }
        if let Some(stop) = self.owned_stops.lock().await.remove(&id) {
            stop.cancel();
        }
        self.windows.close(id).await;
        self.reports
            .say(
                self.environment,
                Report::DisplayState(orch_link_contract::display::DisplayState {
                    display_id: id,
                    state: orch_link_contract::browser::BrowserReportedState::Closed,
                    reason: Some(reason.to_owned()),
                    geometry: None,
                }),
            )
            .await;
    }

    /// Ends display services when their environment detaches.
    pub async fn close_all(&self) {
        for display in self.list_unchecked().await {
            self.close_reason(display.display_id, "environment_stopped")
                .await;
        }
    }
}
