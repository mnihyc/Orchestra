//! Virtual displays used by browsers with a window, kept until the environment ends.

use crate::browser_discovery::{find_program, terminate};
use crate::browsers::{BrowserError, viewport};
use crate::displays::windows::{Identity, Record, Store};
use orch_link_contract::browser::ScreenLookDisplay;
use orch_types::Id;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;
use tokio::process::Child;
use tokio::sync::Mutex;

/// A virtual display normally answers in a moment; ten seconds gives a busy unit room.
const START_WAIT: Duration = Duration::from_secs(10);
/// Poll a local server without holding an unbounded request open.
const POLL: Duration = Duration::from_millis(100);
/// Four browsers use very few of the unit's one thousand test/service ports.
const PORT_MIN: u16 = 41000;
const PORT_MAX: u16 = 41999;
/// X display numbers avoid the native display at zero and leave room for the agent's own servers.
const X_DISPLAY_MAX: u32 = 100;
/// A terminating X server has time to remove its socket and lock.
const STOP_WAIT: Duration = Duration::from_secs(5);

struct Window {
    endpoint: String,
    x_display: String,
    server: Option<Child>,
    manager: Option<Child>,
    record: Option<Record>,
    internal: Option<crate::ports::InternalPort>,
}

/// Virtual displays opened for browser windows in one managed environment.
pub struct Windows {
    launcher: RwLock<Vec<String>>,
    variables: BTreeMap<String, String>,
    running: Mutex<BTreeMap<Id, Window>>,
    store: Option<Store>,
    ports: RwLock<Option<crate::ports::Ports>>,
}

impl Windows {
    /// A private display and its window manager share the environment's resource ceiling.
    pub fn set_launcher(&self, launcher: Vec<String>) {
        *self
            .launcher
            .write()
            .unwrap_or_else(PoisonError::into_inner) = launcher;
    }
    /// A registry that starts no process until a window is requested.
    #[must_use]
    pub fn new(variables: BTreeMap<String, String>) -> Arc<Self> {
        Arc::new(Self {
            launcher: RwLock::new(Vec::new()),
            variables,
            running: Mutex::new(BTreeMap::new()),
            store: None,
            ports: RwLock::new(None),
        })
    }

    /// Restores private owned identities before another open can allocate a screen.
    pub fn with_store(
        variables: BTreeMap<String, String>,
        directory: std::path::PathBuf,
    ) -> Result<Arc<Self>, BrowserError> {
        let store = Store::new(directory)?;
        let mut running = BTreeMap::new();
        for mut record in store.load()? {
            record.recover();
            if record.closing {
                record.stop();
            }
            if record.live() {
                running.insert(
                    record.id,
                    Window {
                        endpoint: record.endpoint.clone(),
                        x_display: record.x_display.clone(),
                        server: None,
                        manager: None,
                        record: Some(record),
                        internal: None,
                    },
                );
            }
        }
        store.save(running.values().filter_map(|w| w.record.clone()).collect())?;
        Ok(Arc::new(Self {
            launcher: RwLock::new(Vec::new()),
            variables,
            running: Mutex::new(running),
            store: Some(store),
            ports: RwLock::new(None),
        }))
    }

    fn save(&self, running: &BTreeMap<Id, Window>) -> Result<(), BrowserError> {
        if let Some(store) = &self.store {
            store.save(running.values().filter_map(|w| w.record.clone()).collect())?;
        }
        Ok(())
    }

    /// Keeps the generic Ports door from bypassing a display's VNC ticket and watch filter.
    pub async fn protect_ports(&self, ports: crate::ports::Ports) {
        *self.ports.write().unwrap_or_else(PoisonError::into_inner) = Some(ports.clone());
        for window in self.running.lock().await.values_mut() {
            if let Some(port) = window
                .endpoint
                .strip_prefix("127.0.0.1:")
                .and_then(|port| port.parse::<u16>().ok())
            {
                window.internal =
                    Some(ports.internal_port(port, orch_api_contract::ports::PortTransport::Tcp));
            }
        }
    }

    /// Opens a display with a loopback server and the installed window manager.
    pub async fn open(&self, id: Id, geometry: &str) -> Result<(String, String), BrowserError> {
        viewport(geometry)?;
        let mut running = self.running.lock().await;
        if let Some(window) = running.get(&id) {
            if window.record.as_ref().is_some_and(|record| record.closing) {
                return Err(BrowserError::Failed("the display is closing".to_owned()));
            }
            return Ok((window.endpoint.clone(), window.x_display.clone()));
        }
        let server = find_program("Xvnc")
            .or_else(|| find_program("Xtigervnc"))
            .ok_or(BrowserError::Unsupported)?;
        let display = (1..=X_DISPLAY_MAX)
            .find(|n| !std::path::Path::new(&format!("/tmp/.X{n}-lock")).exists())
            .ok_or(BrowserError::Unsupported)?;
        let mut port = None;
        for candidate in PORT_MIN..=PORT_MAX {
            if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", candidate)).await {
                drop(listener);
                port = Some(candidate);
                break;
            }
        }
        let port = port.ok_or(BrowserError::Unsupported)?;
        let ports = self
            .ports
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let internal = ports
            .map(|ports| ports.internal_port(port, orch_api_contract::ports::PortTransport::Tcp));
        let x_display = format!(":{display}");
        let endpoint = format!("127.0.0.1:{port}");
        let mut record = Record {
            id,
            endpoint: endpoint.clone(),
            x_display: x_display.clone(),
            program: std::fs::canonicalize(&server)?,
            server: None,
            manager: None,
            closing: false,
        };
        if let Some(store) = &self.store {
            let mut records: Vec<_> = running.values().filter_map(|w| w.record.clone()).collect();
            records.push(record.clone());
            store.save(records)?;
        }
        let label = record.label();
        let mut command = crate::browser_discovery::browser_command_in(
            &server,
            &self.variables,
            &self.launcher.read().unwrap_or_else(PoisonError::into_inner),
        );
        if self.store.is_some() {
            command.kill_on_drop(false);
        }
        command
            .args([
                &x_display,
                "-geometry",
                geometry,
                "-depth",
                "24",
                "-rfbport",
                &port.to_string(),
                "-localhost",
                "-nolisten",
                "tcp",
                "-AlwaysShared",
                "-SecurityTypes",
                "None",
                "-SendCutText=0",
                "-desktop",
                &label,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.save(&running)?;
                return Err(error.into());
            }
        };
        record.server = child.id().and_then(Identity::capture);
        if let Some(store) = &self.store {
            let mut records: Vec<_> = running.values().filter_map(|w| w.record.clone()).collect();
            records.push(record.clone());
            if let Err(error) = store.save(records) {
                terminate(&mut child).await;
                return Err(error);
            }
        }
        let deadline = tokio::time::Instant::now() + START_WAIT;
        loop {
            let answer = tokio::time::timeout_at(deadline, async {
                let connected = crate::displays::rfb::authenticate(
                    &crate::displays::rfb::Endpoint::Port(port),
                    None,
                )
                .await?;
                crate::displays::rfb::describe(connected).await
            })
            .await;
            if matches!(answer,Ok(Ok((Some(ref name),ref found_geometry))) if name==&label&&found_geometry==geometry)
                && child.try_wait()?.is_none()
            {
                break;
            }
            if child.try_wait()?.is_some() || tokio::time::Instant::now() >= deadline {
                terminate(&mut child).await;
                self.save(&running)?;
                return Err(BrowserError::Failed(
                    "the display did not answer".to_owned(),
                ));
            }
            tokio::time::sleep(POLL).await;
        }
        let manager = if let Some(program) = ["openbox", "fluxbox", "twm"]
            .into_iter()
            .find_map(find_program)
        {
            let mut command = crate::browser_discovery::browser_command_in(
                &program,
                &self.variables,
                &self.launcher.read().unwrap_or_else(PoisonError::into_inner),
            );
            if self.store.is_some() {
                command.kill_on_drop(false);
            }
            command
                .env("DISPLAY", &x_display)
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            match command.spawn() {
                Ok(manager) => Some(manager),
                Err(error) => {
                    terminate(&mut child).await;
                    self.save(&running)?;
                    return Err(error.into());
                }
            }
        } else {
            None
        };
        record.manager = manager
            .as_ref()
            .and_then(Child::id)
            .and_then(Identity::capture);
        running.insert(
            id,
            Window {
                endpoint: endpoint.clone(),
                x_display: x_display.clone(),
                server: Some(child),
                manager,
                record: Some(record),
                internal,
            },
        );
        if let Err(error) = self.save(&running) {
            if let Some(mut window) = running.remove(&id) {
                if let Some(mut child) = window.server.take() {
                    terminate(&mut child).await;
                }
                if let Some(mut manager) = window.manager.take() {
                    terminate(&mut manager).await;
                }
            }
            return Err(error);
        }
        Ok((endpoint, x_display))
    }

    /// The live display services for connector reconciliation.
    pub async fn list(&self) -> Vec<ScreenLookDisplay> {
        let mut running = self.running.lock().await;
        let ended: Vec<_> = running
            .iter_mut()
            .filter_map(|(id, window)| {
                let ended = match &mut window.server {
                    Some(server) => server.try_wait().ok().flatten().is_some(),
                    None => !window.record.as_ref().is_some_and(Record::live),
                };
                ended.then_some(*id)
            })
            .collect();
        let changed = !ended.is_empty();
        for id in ended {
            if let Some(mut window) = running.remove(&id) {
                if let Some(mut manager) = window.manager.take() {
                    terminate(&mut manager).await;
                } else if let Some(record) = window.record {
                    record.stop();
                }
            }
        }
        if changed {
            let _ = self.save(&running);
        }
        running
            .iter()
            .filter(|(_, window)| !window.record.as_ref().is_some_and(|record| record.closing))
            .map(|(id, w)| ScreenLookDisplay {
                display_id: *id,
                endpoint: w.endpoint.clone(),
            })
            .collect()
    }

    /// Stops one owned virtual screen; announced display servers never enter this registry.
    pub async fn close(&self, id: Id) {
        let mut running = self.running.lock().await;
        if let Some(window) = running.get_mut(&id)
            && let Some(record) = &mut window.record
        {
            record.closing = true;
        }
        let _ = self.save(&running);
        let window = running.remove(&id);
        if let Some(mut window) = window {
            if let Some(mut manager) = window.manager.take() {
                terminate(&mut manager).await;
            }
            if let Some(mut server) = window.server.take() {
                terminate(&mut server).await;
            } else if let Some(record) = window.record {
                record.stop();
            }
        }
        let _ = self.save(&running);
    }

    /// Ends the display groups with their environment.
    pub async fn stop_all(&self) {
        let mut locked = self.running.lock().await;
        for window in locked.values_mut() {
            if let Some(record) = &mut window.record {
                record.closing = true;
            }
        }
        let _ = self.save(&locked);
        let running = std::mem::take(&mut *locked);
        for (_, mut window) in running {
            if let Some(mut manager) = window.manager.take() {
                terminate(&mut manager).await;
            }
            #[cfg(unix)]
            if let Some(pid) = window
                .server
                .as_ref()
                .and_then(Child::id)
                .and_then(|p| i32::try_from(p).ok())
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
            }
            if let Some(mut server) = window.server.take() {
                if tokio::time::timeout(STOP_WAIT, server.wait())
                    .await
                    .is_err()
                {
                    terminate(&mut server).await;
                }
            } else if let Some(record) = window.record {
                record.stop();
            }
        }
        let _ = self.save(&locked);
    }
}
