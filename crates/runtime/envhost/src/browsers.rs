//! Kept browser profiles and the one browser running on each, started or announced.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use orch_link_contract::browser::*;
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::sync::{Mutex, RwLock};

use crate::browser_cdp::{Cdp, CdpError, identify, local_endpoint};
use crate::browser_discovery::{
    PORT_POLL, START_WAIT, active_port, browser_command_in, discover, terminate,
};
use crate::environment::{Report, Reports};

/// Browsers are allowed ten seconds to quit before an owned process is stopped.
const QUIT_WAIT: Duration = Duration::from_secs(10);
/// Enough of a failed start's diagnostic to explain it without keeping an unbounded log.
const ERROR_TAIL_BYTES: usize = 16 * 1024;
/// Profile directory names are at most sixty-three ASCII characters.
const PROFILE_MAX: usize = 63;
/// Bounds of a shared browser viewport.
pub const VIEWPORT_MIN_WIDTH: u16 = 320;
/// The largest page width.
pub const VIEWPORT_MAX_WIDTH: u16 = 3840;
/// The smallest page height.
pub const VIEWPORT_MIN_HEIGHT: u16 = 240;
/// The largest page height.
pub const VIEWPORT_MAX_HEIGHT: u16 = 2160;

/// Why a browser lifecycle request failed.
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    /// An argument is invalid.
    #[error("{0}")]
    Invalid(String),
    /// Nothing installed can run the requested browser.
    #[error(
        "no installed browser supports this preset; announce takes a browser you start yourself"
    )]
    Unsupported,
    /// The profile does not exist.
    #[error("profile {0} does not exist; ask to create it")]
    NotFound(String),
    /// A profile already exists.
    #[error("profile {0} exists already")]
    Exists(String),
    /// A browser is using a profile.
    #[error("profile {0} is in use; close its browser first")]
    InUse(String),
    /// An IO operation failed.
    #[error("browser files could not be accessed")]
    Io(#[from] std::io::Error),
    /// A browser did not start in time.
    #[error("{0}")]
    Failed(String),
    /// The browser's driving connection failed.
    #[error(transparent)]
    Protocol(#[from] CdpError),
}

impl BrowserError {
    /// The command error word by which the server reports the cause.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid",
            Self::Unsupported => "unsupported",
            Self::NotFound(_) => "not_found",
            Self::Exists(_) => "conflict",
            Self::InUse(_) => "in_use",
            Self::Io(_) => "internal",
            Self::Failed(_) => "failed",
            Self::Protocol(CdpError::NotLocal) => "not_local",
            Self::Protocol(_) => "not_reachable",
        }
    }
}

/// Names a directory, never a path or a browser's own default profile.
#[must_use]
pub fn profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= PROFILE_MAX
        && (name.as_bytes()[0].is_ascii_lowercase() || name.as_bytes()[0].is_ascii_digit())
        && name.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

/// Parses and bounds a browser's shared window size.
pub fn viewport(value: &str) -> Result<(u16, u16), BrowserError> {
    let dimensions = value
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)));
    match dimensions {
        Some((w, h))
            if (VIEWPORT_MIN_WIDTH..=VIEWPORT_MAX_WIDTH).contains(&w)
                && (VIEWPORT_MIN_HEIGHT..=VIEWPORT_MAX_HEIGHT).contains(&h) =>
        {
            Ok((w, h))
        }
        _ => Err(BrowserError::Invalid(
            "viewport must be 320..3840 by 240..2160, written widthxheight".to_owned(),
        )),
    }
}

/// Extra arguments cannot replace the profile, endpoint, sandbox decision or preset.
pub fn extra_arguments(args: &[String]) -> Result<(), BrowserError> {
    for arg in args {
        let key = arg.split('=').next().unwrap_or(arg);
        if key == "--user-data-dir"
            || key.starts_with("--remote-debugging")
            || matches!(
                key,
                "--no-sandbox"
                    | "--proxy-server"
                    | "--proxy-bypass-list"
                    | "--proxy-pac-url"
                    | "--no-proxy-server"
                    | "--enable-quic"
                    | "--headless"
                    | "--window-size"
                    | "--no-first-run"
                    | "--force-first-run"
                    | "--no-default-browser-check"
                    | "--disable-background-timer-throttling"
                    | "--disable-backgrounding-occluded-windows"
                    | "--disable-renderer-backgrounding"
                    | "--disable-dev-shm-usage"
            )
            || !arg.starts_with("--")
        {
            return Err(BrowserError::Invalid(format!(
                "browser.args cannot replace {key}"
            )));
        }
    }
    Ok(())
}

/// The browser process and its one protocol connection; page content lives only in that connection.
pub struct RunningBrowser {
    /// Its identifier.
    pub id: Id,
    /// The profile where it has one.
    pub profile: Option<String>,
    /// Its driving address inside the environment.
    pub endpoint: String,
    /// What it says it is.
    pub program: String,
    /// Whether Orch owns its process.
    pub started: bool,
    /// The one protocol connection Orch holds.
    pub cdp: Arc<Cdp>,
    /// Its initial shared window size.
    pub viewport: (u16, u16),
    process: Mutex<Option<Child>>,
    owned_group: Option<(u32, u64)>,
}

struct State {
    browser: EnvironmentFactsBrowser,
    vnc: EnvironmentFactsVnc,
    running: BTreeMap<Id, Arc<RunningBrowser>>,
}

impl RunningBrowser {
    /// The owned process root, so the environment can include its descendants in its process scope.
    pub(crate) fn owned_pid(&self) -> Option<u32> {
        self.owned_group.map(|(pid, _)| pid)
    }
}

/// The browser registry of one attached environment, with an atomic profile handout.
pub struct Browsers {
    launcher: std::sync::RwLock<Vec<String>>,
    environment: Id,
    directory: PathBuf,
    variables: BTreeMap<String, String>,
    managed: bool,
    generation: i64,
    recovered: Mutex<Vec<StoredBrowser>>,
    clock: Arc<dyn Clock>,
    reports: Reports,
    state: Mutex<State>,
    /// A gate can close while a tunnel is open.
    pub allowed: RwLock<bool>,
}

#[derive(Serialize, Deserialize)]
struct StoredBrowser {
    environment: Id,
    generation: i64,
    id: Id,
    profile: Option<String>,
    endpoint: String,
    started: bool,
    viewport: (u16, u16),
    owned_group: Option<(u32, u64)>,
}

fn process_start(pid: u32) -> Option<u64> {
    sysinfo::System::new_all()
        .process(sysinfo::Pid::from_u32(pid))
        .map(sysinfo::Process::start_time)
}

fn process_profile(process: &sysinfo::Process) -> Option<PathBuf> {
    let mut args = process.cmd().iter();
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy();
        if arg == "--user-data-dir" {
            return args.next().map(PathBuf::from);
        }
        if let Some(path) = arg.strip_prefix("--user-data-dir=") {
            return Some(PathBuf::from(path));
        }
    }
    None
}

impl Browsers {
    /// Browser children share the environment's proved resource group.
    pub fn set_launcher(&self, launcher: Vec<String>) {
        *self
            .launcher
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = launcher;
    }
    // Kept metadata names processes and local endpoints, never a page, input or image.
    fn store(&self, state: &State) -> Result<(), BrowserError> {
        let records: Vec<_> = state
            .running
            .values()
            .map(|b| StoredBrowser {
                environment: self.environment,
                generation: self.generation,
                id: b.id,
                profile: b.profile.clone(),
                endpoint: b.endpoint.clone(),
                started: b.started,
                viewport: b.viewport,
                owned_group: b.owned_group,
            })
            .collect();
        let bytes =
            serde_json::to_vec(&records).map_err(|e| BrowserError::Failed(e.to_string()))?;
        let temporary = self.directory.join(".known.tmp");
        std::fs::write(&temporary, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(temporary, self.directory.join(".known.json"))?;
        Ok(())
    }

    async fn restore(self: &Arc<Self>) -> Result<(), BrowserError> {
        let records = std::mem::take(&mut *self.recovered.lock().await);
        for record in records {
            if record.environment != self.environment || record.generation != self.generation {
                continue;
            }
            let valid_process = record
                .owned_group
                .is_none_or(|(pid, start)| process_start(pid) == Some(start));
            let address = if valid_process {
                identify(&record.endpoint).await.ok()
            } else {
                None
            };
            if let Some(address) = address
                && let Ok(cdp) = Cdp::connect(&address).await
            {
                let _ = cdp
                    .call("Target.setDiscoverTargets", json!({"discover":true}), None)
                    .await;
                let browser = Arc::new(RunningBrowser {
                    id: record.id,
                    profile: record.profile,
                    endpoint: address.endpoint,
                    program: address.program,
                    started: record.started,
                    cdp,
                    viewport: record.viewport,
                    process: Mutex::new(None),
                    owned_group: record.owned_group,
                });
                self.state
                    .lock()
                    .await
                    .running
                    .insert(browser.id, Arc::clone(&browser));
                self.monitor(&browser);
            } else {
                self.reports
                    .say(
                        self.environment,
                        Report::BrowserState(BrowserState {
                            browser_id: record.id,
                            state: BrowserReportedState::Closed,
                            reason: Some("link_lost".to_owned()),
                            tabs: None,
                            program: None,
                        }),
                    )
                    .await;
            }
        }
        Ok(())
    }

    /// Enforces the machine's current switch and lets go of announced browsers when it closes.
    pub async fn set_allowed(&self, allowed: bool) {
        *self.allowed.write().await = allowed;
        if !allowed {
            let ids: Vec<_> = self.state.lock().await.running.keys().copied().collect();
            for id in ids {
                self.reports
                    .say(
                        self.environment,
                        Report::BrowserState(BrowserState {
                            browser_id: id,
                            state: BrowserReportedState::Closed,
                            reason: Some("switched_off".to_owned()),
                            tabs: None,
                            program: None,
                        }),
                    )
                    .await;
            }
            let owned: Vec<_> = self
                .state
                .lock()
                .await
                .running
                .values()
                .filter(|b| b.started)
                .map(|b| b.id)
                .collect();
            for id in owned {
                let _ = self.close(id).await;
            }
            let mut state = self.state.lock().await;
            state.running.clear();
            let _ = self.store(&state);
        }
    }

    /// Refuses browser acts when the machine's own policy has shut the door.
    pub async fn require_allowed(&self) -> Result<(), BrowserError> {
        if *self.allowed.read().await {
            Ok(())
        } else {
            Err(BrowserError::Invalid(
                "machine policy forbids browser access".to_owned(),
            ))
        }
    }
    /// Creates the private profiles root; it is always outside the workspace.
    pub fn new(
        environment: Id,
        directory: PathBuf,
        variables: BTreeMap<String, String>,
        managed: bool,
        generation: i64,
        clock: Arc<dyn Clock>,
        reports: Reports,
    ) -> Result<Arc<Self>, BrowserError> {
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let recovered = std::fs::read(directory.join(".known.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Ok(Arc::new(Self {
            launcher: std::sync::RwLock::new(Vec::new()),
            environment,
            directory,
            variables,
            managed,
            generation,
            recovered: Mutex::new(recovered),
            clock,
            reports,
            state: Mutex::new(State {
                browser: EnvironmentFactsBrowser::default(),
                vnc: EnvironmentFactsVnc::default(),
                running: BTreeMap::new(),
            }),
            allowed: RwLock::new(managed),
        }))
    }

    /// Where named profiles lie, also given to the environment's child processes.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The timestamp of a completed look, from the host's clock.
    #[must_use]
    pub fn looked_at(&self) -> Time {
        self.clock.time()
    }

    /// Discovers programs and probes the sandbox, without changing whether browsers are wanted.
    pub async fn look(self: &Arc<Self>) -> Result<ScreenLookResult, BrowserError> {
        self.restore().await?;
        let cached = {
            let state = self.state.lock().await;
            state
                .browser
                .programs
                .first()
                .and_then(|p| state.browser.sandbox.map(|s| (p.path.clone(), s)))
        };
        let (browser, vnc) = discover(&self.directory, &self.variables, self.managed, cached).await;
        let mut state = self.state.lock().await;
        state.browser = browser;
        state.vnc = vnc;
        Ok(ScreenLookResult {
            browser: state.browser.clone(),
            vnc: state.vnc.clone(),
            profiles: self.profiles_in(&state)?,
            browsers: state
                .running
                .values()
                .map(|b| ScreenLookBrowser {
                    browser_id: b.id,
                    endpoint: b.endpoint.clone(),
                    profile: b.profile.clone(),
                })
                .collect(),
            displays: Vec::new(),
        })
    }

    /// The last discovered support.
    pub async fn support(&self) -> (EnvironmentFactsBrowser, EnvironmentFactsVnc) {
        let state = self.state.lock().await;
        (state.browser.clone(), state.vnc.clone())
    }

    fn profile_path(&self, name: &str) -> Result<PathBuf, BrowserError> {
        if !profile_name(name) {
            return Err(BrowserError::Invalid(
                "profile must match [a-z0-9][a-z0-9._-]{0,62}".to_owned(),
            ));
        }
        let path = self.directory.join(name);
        if let Ok(metadata) = std::fs::symlink_metadata(&path)
            && (!metadata.is_dir() || metadata.file_type().is_symlink())
        {
            return Err(BrowserError::Invalid(
                "a profile must be a real directory".to_owned(),
            ));
        }
        Ok(path)
    }

    fn profiles_in(&self, state: &State) -> Result<Vec<BrowserProfile>, BrowserError> {
        let mut profiles = Vec::new();
        for entry in std::fs::read_dir(&self.directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !profile_name(&name) || !entry.file_type()?.is_dir() {
                continue;
            }
            let metadata = entry.metadata()?;
            let used_at = metadata
                .modified()
                .ok()
                .map(chrono::DateTime::<chrono::Utc>::from)
                .map(orch_types::Time::from);
            profiles.push(BrowserProfile {
                name: name.clone(),
                size_bytes: None,
                used_at,
                browser: state
                    .running
                    .values()
                    .find(|b| b.profile.as_deref() == Some(&name))
                    .map(|b| b.id),
            });
        }
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(profiles)
    }

    /// Lists profiles without walking their private contents.
    pub async fn profiles(&self) -> Result<Vec<BrowserProfile>, BrowserError> {
        let state = self.state.lock().await;
        self.profiles_in(&state)
    }

    async fn used(&self, name: &str, state: &State) -> bool {
        if state
            .running
            .values()
            .any(|b| b.profile.as_deref() == Some(name))
        {
            return true;
        }
        let path = self.directory.join(name);
        if sysinfo::System::new_all().processes().values().any(|p| {
            process_profile(p).as_ref().is_some_and(|profile| {
                std::fs::canonicalize(profile).ok().is_some_and(|profile| {
                    std::fs::canonicalize(&path).is_ok_and(|path| profile == path)
                })
            })
        }) {
            return true;
        }
        if let Some((port, socket)) = active_port(&path)
            && identify(&format!("http://127.0.0.1:{port}"))
                .await
                .is_ok_and(|address| address.socket.ends_with(&socket))
        {
            return true;
        }
        #[cfg(unix)]
        if !self.managed
            && let Ok(lock) = std::fs::read_link(path.join("SingletonLock"))
            && let Some(pid) = lock
                .to_string_lossy()
                .rsplit('-')
                .next()
                .and_then(|p| p.parse::<u32>().ok())
        {
            return sysinfo::System::new_all()
                .process(sysinfo::Pid::from_u32(pid))
                .is_some();
        }
        false
    }

    /// Changes a profile only while no browser uses it.
    pub async fn profile(
        &self,
        body: &BrowserProfileCommand,
    ) -> Result<BrowserProfileResult, BrowserError> {
        self.require_allowed().await?;
        let state = self.state.lock().await;
        let path = self.profile_path(&body.name)?;
        if self.used(&body.name, &state).await {
            return Err(BrowserError::InUse(body.name.clone()));
        }
        match body.action {
            BrowserProfileAction::Create => {
                if body.to.is_some() {
                    return Err(BrowserError::Invalid("create takes no to".to_owned()));
                }
                if path.exists() {
                    return Err(BrowserError::Exists(body.name.clone()));
                }
                std::fs::create_dir(&path)?;
            }
            BrowserProfileAction::Rename => {
                let to = body
                    .to
                    .as_ref()
                    .ok_or_else(|| BrowserError::Invalid("rename needs to".to_owned()))?;
                let target = self.profile_path(to)?;
                if !path.exists() {
                    return Err(BrowserError::NotFound(body.name.clone()));
                }
                if target.exists() {
                    return Err(BrowserError::Exists(to.clone()));
                }
                std::fs::rename(path, target)?;
            }
            BrowserProfileAction::Delete => {
                if body.to.is_some() {
                    return Err(BrowserError::Invalid("delete takes no to".to_owned()));
                }
                if !path.exists() {
                    return Err(BrowserError::NotFound(body.name.clone()));
                }
                std::fs::remove_dir_all(path)?;
            }
        }
        Ok(BrowserProfileResult {
            profiles: self.profiles_in(&state)?,
        })
    }

    /// Starts the browser on a profile, or returns the browser that already runs on it.
    pub async fn open(
        self: &Arc<Self>,
        body: &BrowserOpen,
    ) -> Result<BrowserOpenResult, BrowserError> {
        self.require_allowed().await?;
        extra_arguments(&body.args)?;
        let size = viewport(&body.viewport)?;
        let mut state = self.state.lock().await;
        if let Some(running) = state
            .running
            .values()
            .find(|b| b.profile.as_deref() == Some(&body.profile))
        {
            return Ok(BrowserOpenResult {
                service: format!("browser/{}", running.id),
                endpoint: running.endpoint.clone(),
                program: running.program.clone(),
                started: false,
                running: Some(running.id),
            });
        }
        let path = self.profile_path(&body.profile)?;
        if !path.exists() {
            if !body.create {
                return Err(BrowserError::NotFound(body.profile.clone()));
            }
            std::fs::create_dir(&path)?;
        }
        if self.used(&body.profile, &state).await {
            return Err(BrowserError::InUse(body.profile.clone()));
        }
        // The home volume is exclusive to one managed unit. With no process on this profile, these are stale locks of its retired unit.
        if self.managed {
            for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
                match std::fs::remove_file(path.join(name)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        let program = state
            .browser
            .programs
            .first()
            .ok_or(BrowserError::Unsupported)?
            .path
            .clone();
        if body.preset == BrowserPreset::Window && !state.browser.window {
            return Err(BrowserError::Unsupported);
        }
        // A previous port file must not let a new process adopt a stale address. The process itself owns this file after launch.
        match std::fs::remove_file(path.join("DevToolsActivePort")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut command = browser_command_in(
            Path::new(&program),
            &self.variables,
            &self
                .launcher
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        command
            .arg(format!("--user-data-dir={}", path.display()))
            .args([
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-timer-throttling",
                "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding",
            ])
            .arg(format!("--window-size={},{}", size.0, size.1));
        if self.managed {
            command.arg("--disable-dev-shm-usage");
            if let Ok(proxy) = std::env::var("ORCH_EGRESS_PROXY") {
                let proxy = reqwest::Url::parse(&proxy).map_err(|_| {
                    BrowserError::Invalid("the managed proxy address is invalid".to_owned())
                })?;
                if proxy.scheme() != "http"
                    || proxy.host().is_none()
                    || proxy.port().is_none()
                    || !proxy.username().is_empty()
                    || proxy.password().is_some()
                    || proxy.path() != "/"
                    || proxy.query().is_some()
                    || proxy.fragment().is_some()
                {
                    return Err(BrowserError::Invalid(
                        "the managed proxy address is invalid".to_owned(),
                    ));
                }
                command
                    .arg(format!("--proxy-server={proxy}"))
                    .arg("--proxy-bypass-list=<-loopback>;localhost;127.0.0.1;[::1]")
                    .arg("--disable-quic");
            }
        }
        if state.browser.sandbox == Some(false) {
            command.arg("--no-sandbox");
        }
        match body.preset {
            BrowserPreset::Headless => {
                command.arg("--headless");
            }
            BrowserPreset::Window => {
                if let Some(display) = &body.display {
                    command.env("DISPLAY", &display.x_display);
                } else if self.managed {
                    return Err(BrowserError::Unsupported);
                } else {
                    for key in [
                        "DISPLAY",
                        "WAYLAND_DISPLAY",
                        "XDG_RUNTIME_DIR",
                        "XAUTHORITY",
                        "DBUS_SESSION_BUS_ADDRESS",
                    ] {
                        if let Some(value) = std::env::var_os(key) {
                            command.env(key, value);
                        }
                    }
                }
            }
        }
        command
            .args(&body.args)
            .arg(body.url.as_deref().unwrap_or("about:blank"))
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stderr = child.stderr.take();
        let tail = Arc::new(Mutex::new(Vec::new()));
        if let Some(mut stderr) = stderr {
            let tail = Arc::clone(&tail);
            tokio::spawn(async move {
                let mut buffer = [0; 4096];
                while let Ok(n) = stderr.read(&mut buffer).await {
                    if n == 0 {
                        break;
                    }
                    let mut kept = tail.lock().await;
                    kept.extend_from_slice(&buffer[..n]);
                    if kept.len() > ERROR_TAIL_BYTES {
                        let excess = kept.len() - ERROR_TAIL_BYTES;
                        kept.drain(..excess);
                    }
                }
            });
        }
        let deadline = tokio::time::Instant::now() + START_WAIT;
        let address = loop {
            if let Some((port, socket)) = active_port(&path)
                && let Ok(Ok(address)) =
                    tokio::time::timeout_at(deadline, identify(&format!("http://127.0.0.1:{port}")))
                        .await
                && address.socket.ends_with(&socket)
            {
                break Some(address);
            }
            if child.try_wait()?.is_some() || tokio::time::Instant::now() >= deadline {
                break None;
            }
            tokio::time::sleep(PORT_POLL).await;
        };
        let Some(address) = address else {
            terminate(&mut child).await;
            return Err(BrowserError::Failed(
                String::from_utf8_lossy(&tail.lock().await).into_owned(),
            ));
        };
        let owned_group = child
            .id()
            .and_then(|pid| process_start(pid).map(|start| (pid, start)));
        let cdp = match Cdp::connect(&address).await {
            Ok(cdp) => cdp,
            Err(e) => {
                terminate(&mut child).await;
                return Err(e.into());
            }
        };
        if let Err(error) = cdp
            .call("Target.setDiscoverTargets", json!({"discover":true}), None)
            .await
        {
            terminate(&mut child).await;
            return Err(error.into());
        }
        if let Err(error) = initial_viewport(&cdp, size).await {
            terminate(&mut child).await;
            return Err(error.into());
        }
        let browser = Arc::new(RunningBrowser {
            id: body.browser_id,
            profile: Some(body.profile.clone()),
            endpoint: address.endpoint.clone(),
            program: address.program.clone(),
            started: true,
            cdp,
            viewport: size,
            process: Mutex::new(Some(child)),
            owned_group,
        });
        state.running.insert(browser.id, Arc::clone(&browser));
        if let Err(error) = self.store(&state) {
            state.running.remove(&browser.id);
            if let Some(mut child) = browser.process.lock().await.take() {
                terminate(&mut child).await;
            }
            return Err(error);
        }
        drop(state);
        self.monitor(&browser);
        Ok(BrowserOpenResult {
            service: format!("browser/{}", browser.id),
            endpoint: address.endpoint,
            program: address.program,
            started: true,
            running: None,
        })
    }

    /// Takes in a verified local browser and discovers its profile from its processes.
    pub async fn adopt(
        self: &Arc<Self>,
        body: &BrowserAdopt,
    ) -> Result<BrowserAdoptResult, BrowserError> {
        self.require_allowed().await?;
        let address = identify(&body.endpoint).await?;
        let mut state = self.state.lock().await;
        if let Some(browser) = state
            .running
            .values()
            .find(|b| b.endpoint == address.endpoint)
        {
            return Ok(BrowserAdoptResult {
                service: format!("browser/{}", browser.id),
                program: browser.program.clone(),
                profile: browser.profile.clone(),
            });
        }
        let port = local_endpoint(&body.endpoint)?
            .port_or_known_default()
            .ok_or(CdpError::NotLocal)?;
        let profile = self.profile_of(port, &address.socket);
        if let Some(profile) = &profile
            && state
                .running
                .values()
                .any(|b| b.profile.as_ref() == Some(profile))
        {
            return Err(BrowserError::InUse(profile.clone()));
        }
        let cdp = Cdp::connect(&address).await?;
        cdp.call("Target.setDiscoverTargets", json!({"discover":true}), None)
            .await?;
        let browser = Arc::new(RunningBrowser {
            id: body.browser_id,
            profile: profile.clone(),
            endpoint: address.endpoint,
            program: address.program.clone(),
            started: false,
            cdp,
            viewport: (1280, 800),
            process: Mutex::new(None),
            owned_group: None,
        });
        state.running.insert(browser.id, Arc::clone(&browser));
        if let Err(error) = self.store(&state) {
            state.running.remove(&browser.id);
            return Err(error);
        }
        drop(state);
        self.monitor(&browser);
        Ok(BrowserAdoptResult {
            service: format!("browser/{}", browser.id),
            program: address.program,
            profile,
        })
    }

    fn profile_of(&self, port: u16, socket: &str) -> Option<String> {
        let processes = sysinfo::System::new_all();
        for process in processes.processes().values() {
            let mut args = process.cmd().iter();
            while let Some(arg) = args.next() {
                let value = arg.to_string_lossy();
                let path = if value == "--user-data-dir" {
                    args.next().map(PathBuf::from)
                } else {
                    value.strip_prefix("--user-data-dir=").map(PathBuf::from)
                };
                if let Some(path) = path
                    && active_port(&path)
                        .is_some_and(|(p, path)| p == port && socket.ends_with(&path))
                {
                    let path = std::fs::canonicalize(path).ok()?;
                    let relative = path
                        .strip_prefix(std::fs::canonicalize(&self.directory).ok()?)
                        .ok()?;
                    if relative.components().count() == 1
                        && let Some(name) = relative.to_str().filter(|n| profile_name(n))
                    {
                        return Some(name.to_owned());
                    }
                }
            }
        }
        None
    }

    /// Gets a known running browser for its tunnel.
    pub async fn get(&self, id: Id) -> Option<Arc<RunningBrowser>> {
        self.state.lock().await.running.get(&id).cloned()
    }

    /// Quits a browser, killing a reluctant process only when Orch started it.
    pub async fn close(&self, id: Id) -> Result<(), BrowserError> {
        let Some(browser) = self.get(id).await else {
            return Ok(());
        };
        let deadline = tokio::time::Instant::now() + QUIT_WAIT;
        let _ =
            tokio::time::timeout_at(deadline, browser.cdp.call("Browser.close", json!({}), None))
                .await;
        let mut process = browser.process.lock().await;
        if let Some(child) = process.as_mut()
            && tokio::time::timeout_at(deadline, child.wait())
                .await
                .is_err()
        {
            terminate(child).await;
        }
        if process.is_none()
            && browser.started
            && let Some((pid, start)) = browser.owned_group
        {
            while process_start(pid) == Some(start) && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(PORT_POLL).await;
            }
            #[cfg(unix)]
            if process_start(pid) == Some(start)
                && pid > 1
                && let Some(pid) = i32::try_from(pid)
                    .ok()
                    .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
        }
        let mut state = self.state.lock().await;
        state.running.remove(&id);
        self.store(&state)?;
        Ok(())
    }

    /// Ends owned browsers when the environment ends, without deleting profiles.
    pub async fn close_all(&self) {
        let ids: Vec<_> = self.state.lock().await.running.keys().copied().collect();
        for id in ids {
            let _ = self.close(id).await;
        }
    }

    fn monitor(self: &Arc<Self>, browser: &Arc<RunningBrowser>) {
        let mut events = browser.cdp.events();
        let host = Arc::downgrade(self);
        let browser = Arc::downgrade(browser);
        tokio::spawn(async move {
            let mut initial = true;
            let mut known = None;
            loop {
                let event = if std::mem::take(&mut initial) {
                    json!({"method":"Target.targetInfoChanged"})
                } else {
                    match events.recv().await {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                };
                let (Some(host), Some(browser)) = (host.upgrade(), browser.upgrade()) else {
                    break;
                };
                let closed = event["method"] == "Orch.connectionClosed";
                let method = event["method"].as_str().unwrap_or_default();
                if closed || method.starts_with("Target.target") {
                    let tabs = if closed {
                        None
                    } else {
                        let Ok(found) =
                            browser.cdp.call("Target.getTargets", json!({}), None).await
                        else {
                            continue;
                        };
                        let Some(targets) = found["targetInfos"].as_array() else {
                            continue;
                        };
                        // Opening, closing or navigating a tab is activity. A page changing its title is not.
                        let current: BTreeMap<String, String> = targets
                            .iter()
                            .filter(|target| target["type"] == "page")
                            .filter_map(|target| {
                                Some((
                                    target["targetId"].as_str()?.to_owned(),
                                    target["url"].as_str().unwrap_or_default().to_owned(),
                                ))
                            })
                            .collect();
                        if known.as_ref() == Some(&current) {
                            continue;
                        }
                        let count = i64::try_from(current.len()).unwrap_or(i64::MAX);
                        known = Some(current);
                        Some(count)
                    };
                    host.reports
                        .say(
                            host.environment,
                            Report::BrowserState(BrowserState {
                                browser_id: browser.id,
                                state: if closed {
                                    BrowserReportedState::Closed
                                } else {
                                    BrowserReportedState::Open
                                },
                                reason: closed.then(|| "exited".to_owned()),
                                tabs,
                                program: Some(browser.program.clone()),
                            }),
                        )
                        .await;
                    if closed {
                        let mut state = host.state.lock().await;
                        state.running.remove(&browser.id);
                        let _ = host.store(&state);
                        break;
                    }
                }
            }
        });
    }
}

// Setting the initial layout belongs to starting a browser, before anybody watches it.
async fn initial_viewport(cdp: &Cdp, size: (u16, u16)) -> Result<(), CdpError> {
    let found = cdp.call("Target.getTargets", json!({}), None).await?;
    for target in found["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| target["type"] == "page")
    {
        let Some(tab) = target["targetId"].as_str() else {
            continue;
        };
        let attached = cdp
            .call(
                "Target.attachToTarget",
                json!({"targetId":tab,"flatten":true}),
                None,
            )
            .await?;
        let session = attached["sessionId"]
            .as_str()
            .ok_or_else(|| CdpError::Protocol("attach".to_owned()))?;
        // This session belongs to the running browser and remains while viewer sessions come and go.
        cdp.call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":size.0,"height":size.1,"deviceScaleFactor":1,"mobile":false}),
            Some(session),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_are_names_and_never_paths_to_a_persons_browser() {
        for valid in ["a", "0", "team-1", "thread-12345678", "a.b_c"] {
            assert!(profile_name(valid));
        }
        for invalid in ["", ".", "..", "A", "../team", "a/b", "a\\b", "/tmp/a"] {
            assert!(!profile_name(invalid));
        }
    }
    #[test]
    fn extra_args_cannot_reassign_the_profile_or_driving_endpoint() {
        for arg in [
            "--user-data-dir=/tmp/a",
            "--remote-debugging-port=9222",
            "--remote-debugging-pipe",
            "--no-sandbox",
            "--proxy-server=http://elsewhere:3128",
            "--proxy-bypass-list=*",
            "--proxy-pac-url=http://elsewhere/policy",
            "--no-proxy-server",
            "--enable-quic",
        ] {
            assert!(extra_arguments(&[arg.to_owned()]).is_err());
        }
        assert!(extra_arguments(&["--lang=zh-CN".to_owned()]).is_ok());
    }
}
