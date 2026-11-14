//! Initial local identity and endpoint setup, in a data directory of its own, so that several connectors run on one computer.
//!
//! A connector is one process with one data directory, and nothing of it is shared with another or
//! written outside: its identity, its configuration, its spool, its journal and the homes of its
//! harnesses are all inside. This module makes that directory ([`DataDir`]), enrolls into a system with a
//! token ([`enroll`]), keeps what the system answered, renews the certificate before it runs out
//! ([`renew_if_due`]), and works the same out from the launch variables a driver gives a connector it
//! starts in a managed runtime ([`ManagedInputs`]).
//!
//! **Enrolling.** The connector makes a key that never leaves it and asks for a certificate over it with
//! the token and the system's address. The token carries the pin of the system's authority: the connector
//! believes no answer from an authority with another. What it keeps, only once the answer has been
//! checked, is the key and the certificate and the authority's certificate (`identity/`), the answer's
//! endpoints and the roles and name it enrolled with (`config.toml`), and a record of the enrollment
//! with the pin (`identity/enrollment.json`). A refused or distrusted answer leaves nothing behind.
//!
//! **A new unit on an old volume.** A managed runtime's data directory is on a volume that outlives the
//! unit, so that a person's sign-in survives a unit that is made again, and a replacement runs on the
//! volume of the unit it replaces. The machine is the unit's own: the directories that belong to it
//! (`identity/`, `secrets/`, `spool/`, `journal/` and `state/`) are marked with the unit's identifier
//! (`unit`). A unit that finds another's puts them aside, in a directory of that unit's name below
//! `previous/`, and begins with none, which enrolls it as a new machine; a unit that finds its own there (a
//! replacement that failed gave the volume back) takes them again ([`DataDir::claim_machine_state`]). Only
//! what a machine really holds is put aside, one earlier machine is kept, and every machine that is left
//! behind or dropped is named to whoever starts the unit. What a person signed in (`harness/`) and what
//! they set up stay where they are.
//!
//! **Where the answer comes from** is a [`EnrollTransport`]: the HTTPS requests of the system in a
//! running connector, and in a test whatever stands for the system.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use hyper::{Method, StatusCode};
use orch_config::ConnectorConfig;
use orch_driver_contract::runtime::ConnectorRole;
use orch_link_contract::enrollment::{
    ENROLL_PATH, Endpoint, EnrollRefusal, EnrollReply, EnrollReplySystem, EnrollRequest,
    EnrollmentToken, RENEW_PATH, REQUEST_TIMEOUT_SECS, RenewReply, RenewRequest, Trust,
};
use orch_link_contract::enrollment_choices::EnrollmentChoices;
use orch_link_contract::session::{Platform, Role};
use orch_link_contract::version::PROTOCOL_VERSION;
use orch_pki::certificates::{Certificate, LeafKey, ServiceName};
use orch_pki::enrollment::{MachineIdentity, PendingEnrollment, TrustFault};
use orch_pki::renewal;
use orch_pki::verification::{ServerTrust, machine_client_config};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

use crate::https::{HttpsClient, HttpsError, enrollment_base, https_base};
use crate::policy::write_atomically;

/// The subdirectories of a data directory, which the connector makes.
const SUBDIRECTORIES: [&str; 9] = [
    "identity", "spool", "journal", "secrets", "mcp", "harness", "cache", "logs", "state",
];
/// The file in `identity/` that records the enrollment.
pub const RECORD_FILE: &str = "enrollment.json";
/// The file of the data directory that names the unit whose machine the directories of
/// [`MACHINE_DIRECTORIES`] hold.
pub const UNIT_FILE: &str = "unit";
/// The directories of a data directory that belong to the machine enrolled in it: its identity, what its
/// key sealed, what it told the system and was told, and the harness processes it hosted.
pub const MACHINE_DIRECTORIES: [&str; 5] = ["identity", "secrets", "spool", "journal", "state"];
/// Where the machine state of an earlier unit is kept, in a directory of the unit's name, so that it
/// comes back with its unit.
pub const PREVIOUS_DIR: &str = "previous";
/// The name a machine nobody marked is kept under: no unit is named so.
const UNMARKED: &str = "unmarked";
/// The configuration file of a connector.
pub const CONFIG_FILE: &str = "config.toml";
/// The permissions of the data directory and of the directories that hold secrets: the user's alone.
#[cfg(unix)]
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
// The physical-runner lease is private just like the directory that contains it.
#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;

/// The data directory a connector uses when it is given none: the platform's place for a program's data
/// of one user, in a directory of its own.
#[must_use]
pub fn default_data_dir(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    if cfg!(target_os = "windows") {
        return PathBuf::from(var("LOCALAPPDATA").unwrap_or_default())
            .join("Orch")
            .join("connector");
    }
    let home = PathBuf::from(var("HOME").unwrap_or_else(|| ".".to_owned()));
    if cfg!(target_os = "macos") {
        return home
            .join("Library")
            .join("Application Support")
            .join("Orch")
            .join("connector");
    }
    var("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| home.join(".local").join("share"), PathBuf::from)
        .join("orch-connector")
}

/// Why something could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    /// A file or directory could not be made, read or written.
    #[error("{path}: {source}")]
    Io {
        /// What it was about.
        path: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// The token is not one.
    #[error("that is not an enrollment token: it is `oenr_`, a secret, a dot and a pin")]
    BadToken,
    /// The system refused the enrollment.
    #[error("the system refused the enrollment: {0}")]
    Refused(EnrollRefusal),
    /// The system could not be reached or answered something else.
    #[error("the system could not be reached: {0}")]
    Unreachable(String),
    /// The answer is not one this connector believes.
    #[error("the answer is not to be trusted: {0}")]
    Distrusted(TrustFault),
    /// Something the connector keeps could not be made.
    #[error("{0}")]
    Pki(#[from] orch_pki::PkiError),
    /// The configuration could not be read or written.
    #[error("{0}")]
    Config(String),
    /// The launch variables of a managed runtime are not what they should be.
    #[error("the launch of this runtime is incomplete: {0}")]
    Launch(String),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> EnrollError + '_ {
    move |source| EnrollError::Io {
        path: path.to_owned(),
        source,
    }
}

/// What a file that names a unit says, without the line end; `None` where there is no such file or it is empty.
fn read_marker(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let word = text.trim();
    (!word.is_empty()).then(|| word.to_owned())
}

/// Removes a directory and what is in it; nothing is wrong if it is not there.
fn remove_directory(path: &Path) -> Result<(), EnrollError> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(io(path)(error)),
        _ => Ok(()),
    }
}

/// Whether a directory holds anything.
fn holds_something(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}

/// Makes a directory below the data directory, and where the platform has permissions it is the user's alone:
/// what is put aside is a machine's key and what its key sealed.
fn private_directory(path: &Path) -> Result<(), EnrollError> {
    std::fs::create_dir_all(path).map_err(io(path))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE),
        )
        .map_err(io(path))?;
    }
    Ok(())
}

/// Moves a directory to a place that is not there yet, or is there empty (a move that was cut short made it).
fn move_directory(from: &Path, to: &Path) -> Result<(), EnrollError> {
    if let Some(parent) = to.parent() {
        private_directory(parent)?;
    }
    if to.exists() {
        // Only an empty place makes way: what is in it is not ours to overwrite, and `remove_dir` says so.
        std::fs::remove_dir(to).map_err(io(to))?;
    }
    std::fs::rename(from, to).map_err(io(from))
}

/// A unit's name as a name of a directory: the identifiers the system gives are letters, digits and dashes.
fn unit_name(name: &str) -> Option<&str> {
    (!name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then_some(name)
}

/// What a unit found of the machine state in its data directory ([`DataDir::claim_machine_state`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// The state is the unit's own, or there is none yet.
    Own,
    /// What the directory held of another's machine was put aside, and the unit begins with none.
    Fresh {
        /// The machine put aside: the unit it was of, or `unmarked`.
        put_aside: String,
        /// The machines kept before that were dropped, because one earlier machine is kept.
        dropped: Vec<String>,
    },
    /// The unit's own machine, put aside for another unit before, has been brought back.
    Restored {
        /// The machine put aside to make room for it, when the directory held one.
        put_aside: Option<String>,
        /// The machines kept before that were dropped, because one earlier machine is kept.
        dropped: Vec<String>,
    },
}

// ---------------------------------------------------------------------------------------------------------
// The data directory

/// A connector's data directory.
#[derive(Debug, Clone)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    /// Reads an existing data directory without making directories or changing permissions.
    pub fn inspect(root: &Path) -> Result<Self, EnrollError> {
        let root = std::fs::canonicalize(root).map_err(io(root))?;
        let metadata = std::fs::symlink_metadata(&root).map_err(io(&root))?;
        if !metadata.is_dir() {
            return Err(EnrollError::Config(
                "the data directory must be a real directory".into(),
            ));
        }
        Ok(Self { root })
    }

    /// Opens the directory, and makes it and what it holds if they are not there. Where the platform
    /// has permissions, the directory is the user's alone.
    pub fn open(root: &Path) -> Result<Self, EnrollError> {
        std::fs::create_dir_all(root).map_err(io(root))?;
        for sub in SUBDIRECTORIES {
            let path = root.join(sub);
            std::fs::create_dir_all(&path).map_err(io(&path))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [
                root.to_owned(),
                root.join("identity"),
                root.join("secrets"),
                root.join("mcp"),
            ] {
                std::fs::set_permissions(
                    &path,
                    std::fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE),
                )
                .map_err(io(&path))?;
            }
        }
        Ok(Self {
            root: root.to_owned(),
        })
    }

    /// The directory itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `identity/`: the key, the certificate, the authority's certificate and the record.
    #[must_use]
    pub fn identity_dir(&self) -> PathBuf {
        self.root.join("identity")
    }

    /// The configuration file.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    /// Whether this connector is enrolled: its identity is there.
    #[must_use]
    pub fn is_enrolled(&self) -> bool {
        Self::enrolled_at(&self.root)
    }

    /// Whether a connector is enrolled in the directory `root`, without making the directory or anything
    /// in it: for a command that only looks.
    #[must_use]
    pub fn enrolled_at(root: &Path) -> bool {
        let identity = root.join("identity");
        identity.join(orch_pki::enrollment::KEY_FILE).exists()
            && identity.join(RECORD_FILE).exists()
    }

    /// Holds one physical runner on this identity directory, including through reconnects.
    pub fn try_run_lock(&self) -> Result<Option<std::fs::File>, EnrollError> {
        let path = self.root.join("run.lock");
        let mut options = std::fs::OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(PRIVATE_FILE_MODE);
        }
        let file = options.open(&path).map_err(io(&path))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(io(&path)(error)),
        }
    }

    /// Whether any directory of [`MACHINE_DIRECTORIES`] holds something.
    fn holds_machine_state(&self) -> bool {
        MACHINE_DIRECTORIES
            .iter()
            .any(|name| holds_something(&self.root.join(name)))
    }

    /// Where the machine of the unit `unit` is put aside.
    fn put_aside_place(&self, unit: &str) -> PathBuf {
        self.root.join(PREVIOUS_DIR).join(unit)
    }

    /// Brings the machine put aside for `unit` back into the places that hold nothing, and answers whether
    /// anything came back. What is there already is not overwritten; what is not brought back stays aside.
    fn bring_back(&self, unit: &str) -> Result<bool, EnrollError> {
        let aside = self.put_aside_place(unit);
        if !aside.is_dir() {
            return Ok(false);
        }
        let mut brought = false;
        for name in MACHINE_DIRECTORIES {
            let from = aside.join(name);
            let to = self.root.join(name);
            if !from.exists() || holds_something(&to) {
                continue;
            }
            // The empty place that `DataDir::open` or an earlier claim left makes way.
            match std::fs::remove_dir(&to) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(&to)(error)),
            }
            std::fs::rename(&from, &to).map_err(io(&from))?;
            brought = true;
        }
        if !holds_something(&aside) {
            remove_directory(&aside)?;
            // `previous/` itself goes when nothing is kept in it; it is not empty otherwise, and stays.
            let _ = std::fs::remove_dir(self.root.join(PREVIOUS_DIR));
        }
        Ok(brought)
    }

    /// Drops every machine put aside except those of the units named, and answers which were dropped.
    fn drop_put_aside_except(&self, keep: &[&str]) -> Result<Vec<String>, EnrollError> {
        let place = self.root.join(PREVIOUS_DIR);
        let Ok(entries) = std::fs::read_dir(&place) else {
            return Ok(Vec::new());
        };
        let mut dropped = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if keep.contains(&name.as_str()) || !entry.path().is_dir() {
                continue;
            }
            remove_directory(&entry.path())?;
            dropped.push(name);
        }
        dropped.sort();
        Ok(dropped)
    }

    /// Makes the machine state of this directory the state of the unit `unit`, which a managed runtime
    /// does at its start, before anything of the machine is read.
    ///
    /// What the directories of the machine hold of another unit's machine (a new unit made on the volume of
    /// a unit that failed or was replaced), or of a machine nobody marked, is put aside in `previous/`, under
    /// the name of that unit, and the unit begins with none, so that it enrolls as a new machine and not as
    /// the old one, which the system has revoked. Only a machine that holds something is put aside: a unit
    /// that never enrolled leaves what was put aside before where it is. A unit whose own machine was put
    /// aside (a replacement that failed has given the volume back) takes it again. One earlier machine is
    /// kept, and any other that was kept is dropped and named. Every step moves a whole directory, so that
    /// a start that is cut short is finished by the next, and nothing it had moved is lost. What a person
    /// signed in, and the configuration, are not touched.
    pub fn claim_machine_state(&self, unit: &str) -> Result<Claim, EnrollError> {
        let unit = unit_name(unit).ok_or_else(|| {
            EnrollError::Launch(format!("ORCH_RUNTIME_ID {unit:?} is not a unit identifier"))
        })?;
        let marker = self.root.join(UNIT_FILE);
        let owner = read_marker(&marker);
        if owner.as_deref() == Some(unit) {
            // A return of its own machine that was cut short is finished.
            self.bring_back(unit)?;
            return Ok(Claim::Own);
        }
        let mut put_aside = None;
        let mut dropped = Vec::new();
        if self.holds_machine_state() {
            let label = owner
                .as_deref()
                .and_then(unit_name)
                .unwrap_or(UNMARKED)
                .to_owned();
            let place = self.put_aside_place(&label);
            for name in MACHINE_DIRECTORIES {
                let from = self.root.join(name);
                if holds_something(&from) {
                    move_directory(&from, &place.join(name))?;
                }
            }
            dropped = self.drop_put_aside_except(&[label.as_str(), unit])?;
            put_aside = Some(label);
        }
        std::fs::write(&marker, unit).map_err(io(&marker))?;
        let restored = self.bring_back(unit)?;
        // The directories that are missing are made, with their permissions.
        Self::open(&self.root)?;
        Ok(match (restored, put_aside) {
            (true, put_aside) => Claim::Restored { put_aside, dropped },
            (false, Some(put_aside)) => Claim::Fresh { put_aside, dropped },
            (false, None) => Claim::Own,
        })
    }

    /// The identity this connector enrolled with.
    pub fn identity(&self) -> Result<MachineIdentity, EnrollError> {
        Ok(MachineIdentity::load(&self.identity_dir())?)
    }

    /// The record of the enrollment.
    pub fn record(&self) -> Result<EnrollmentRecord, EnrollError> {
        let path = self.identity_dir().join(RECORD_FILE);
        let bytes = std::fs::read(&path).map_err(io(&path))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| EnrollError::Config(format!("{}: {e}", path.display())))
    }

    /// The configuration, as the file has it with the environment's overrides; the defaults where there
    /// is no file.
    pub fn config(
        &self,
        var: impl Fn(&str) -> Option<String>,
    ) -> Result<ConnectorConfig, EnrollError> {
        let path = self.config_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io(&path)(e)),
        };
        let mut config: ConnectorConfig =
            orch_config::load_text(&text, &path.display().to_string(), var)
                .map_err(|e| EnrollError::Config(e.to_string()))?;
        if config.policy.local_keys.is_none() {
            config.policy.local_keys = Some(
                crate::policy::explicit_local_keys(&text)
                    .map_err(|e| EnrollError::Config(e.to_string()))?,
            );
        }
        Ok(config)
    }

    /// The machine's own addresses, or the most recently received system addresses.
    pub fn endpoints(&self, config: &ConnectorConfig) -> Result<Vec<Endpoint>, EnrollError> {
        let local = endpoints_of(config);
        Ok(if local.is_empty() {
            self.record()?.server_endpoints
        } else {
            local
        })
    }

    /// Keeps the system list without modifying any local address override or policy.
    pub fn store_server_endpoints(&self, endpoints: Vec<Endpoint>) -> Result<(), EnrollError> {
        self.store_server_advertisement(Some(endpoints), None, None, None)
    }

    /// Persist one primary's advertisement atomically, without rewriting local gateway choice.
    pub fn store_server_advertisement(
        &self,
        endpoints: Option<Vec<Endpoint>>,
        gateways: Option<Vec<orch_link_contract::gateways::GatewayInstance>>,
        measurements: Option<Vec<orch_link_contract::gateways::GatewayMeasurement>>,
        measured_at: Option<Time>,
    ) -> Result<(), EnrollError> {
        let mut record = self.record()?;
        if let Some(endpoints) = endpoints {
            record.server_endpoints = endpoints;
        }
        if let Some(gateways) = gateways {
            record.gateways = gateways;
        }
        if let Some(measurements) = measurements {
            record.gateway_measurements = measurements;
            record.gateway_measured_at = measured_at;
        }
        let path = self.identity_dir().join(RECORD_FILE);
        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|error| EnrollError::Config(error.to_string()))?;
        write_atomically(&path, &bytes).map_err(io(&path))
    }

    /// Changes the configuration file: reads it, lets `change` edit it, writes it back whole.
    pub fn update_config(
        &self,
        change: impl FnOnce(&mut ConnectorConfig),
    ) -> Result<(), EnrollError> {
        let mut config = self.config(|_| None)?;
        let before = crate::policy::policy_of(&config)
            .map_err(|error| EnrollError::Config(error.to_string()))?;
        let mut local = crate::policy::local_keys(&config, Some(&self.config_path()));
        change(&mut config);
        let after = crate::policy::policy_of(&config)
            .map_err(|error| EnrollError::Config(error.to_string()))?;
        for key in orch_link_contract::policy::PolicyKey::ALL
            .iter()
            .filter(|key| **key != orch_link_contract::policy::PolicyKey::PolicyDelegated)
        {
            if before.get(*key) != after.get(*key) && !local.iter().any(|name| name == key.as_str())
            {
                local.push(key.as_str().into());
            }
        }
        config.policy.local_keys = Some(local.clone());
        let text = crate::policy::sparse_config(&config, &local)
            .map_err(|e| EnrollError::Config(e.to_string()))?;
        let path = self.config_path();
        write_atomically(&path, text.as_bytes()).map_err(io(&path))
    }
}

/// What the connector keeps of its enrollment, beside the keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentRecord {
    /// Time of the retained probe result; old cached records remain explicitly undated.
    #[serde(default)]
    pub gateway_measured_at: Option<Time>,
    /// Latest labelled gateways, never a replacement for the locally selected primary.
    #[serde(default)]
    pub gateways: Vec<orch_link_contract::gateways::GatewayInstance>,
    /// Verified enrollment measurements, retained for installation/status display.
    #[serde(default)]
    pub gateway_measurements: Vec<orch_link_contract::gateways::GatewayMeasurement>,
    /// The last system preference list; distinct from the machine's own config.endpoints.
    #[serde(default)]
    pub server_endpoints: Vec<Endpoint>,
    /// The machine.
    pub machine_id: Id,
    /// What it was called when it enrolled.
    pub name: String,
    /// The system it belongs to.
    pub system: EnrollReplySystem,
    /// The pin of that system's authority: the SHA-256 of its public key, as unpadded base64url.
    pub pin: String,
    /// The protocol version the two speak.
    pub protocol_version: u32,
    /// When it enrolled.
    pub enrolled_at: Time,
    /// Whether a person connected it or the system made it.
    pub managed: bool,
}

// ---------------------------------------------------------------------------------------------------------
// Where the answer comes from

/// The system's answers to the connector's two requests.
#[async_trait]
pub trait EnrollTransport: Send + Sync {
    /// Read installation metadata without issuing or spending an enrollment token.
    /// Older systems and in-memory fixtures may have no discovery endpoint.
    async fn gateways(
        &self,
        _address: &str,
        _token: &EnrollmentToken,
    ) -> Result<Option<orch_link_contract::gateways::GatewayDiscovery>, EnrollError> {
        Ok(None)
    }

    /// A verified probe before the machine has a credential of its own.
    async fn probe_gateway(
        &self,
        _authority: &Certificate,
        _endpoint: &Endpoint,
    ) -> Result<orch_link_contract::gateways::GatewayMeasurement, orch_link::reconnect::DialError>
    {
        Err(orch_link::reconnect::DialError::Connect(
            "gateway probing is unavailable on this enrollment transport".into(),
        ))
    }

    /// Enroll at the selected verified entrance, retaining the entrance's declared trust mode.
    async fn enroll_at(
        &self,
        endpoint: &Endpoint,
        token: &EnrollmentToken,
        request: &EnrollRequest,
    ) -> Result<EnrollReply, EnrollError> {
        self.enroll(&endpoint.url, token, request).await
    }
    /// Asks to be enrolled at `address`. `token` carries the pin to trust the answer by.
    async fn enroll(
        &self,
        address: &str,
        token: &EnrollmentToken,
        request: &EnrollRequest,
    ) -> Result<EnrollReply, EnrollError>;

    /// Asks for a new certificate at `endpoint`, over a new key, as the holder of `identity`.
    async fn renew(
        &self,
        endpoint: &Endpoint,
        identity: &MachineIdentity,
        request: &RenewRequest,
    ) -> Result<RenewReply, EnrollError>;
}

/// The requests over HTTPS.
pub struct HttpsEnroll {
    clock: Arc<dyn Clock>,
    only: Option<Trust>,
}

impl std::fmt::Debug for HttpsEnroll {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpsEnroll").finish_non_exhaustive()
    }
}

impl HttpsEnroll {
    /// Requests that read the time from `clock`. `only` fixes the way the system is trusted; without it
    /// an address is tried as a public one first (a certificate any browser would take, and the pin
    /// checked in the answer) and then as the system's own.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>, only: Option<Trust>) -> Self {
        Self { clock, only }
    }

    fn client(
        &self,
        trust: Trust,
        pin: &[u8; 32],
        identity: Option<&MachineIdentity>,
        authority: Option<&Certificate>,
    ) -> Result<HttpsClient, EnrollError> {
        let server = authority.map_or(ServerTrust::Pin(*pin), |ca| {
            ServerTrust::Authority(Box::new(ca.clone()))
        });
        let tls = if trust == Trust::Pinned {
            identity.map(MachineIdentity::tls).transpose()?
        } else {
            None
        };
        let expected = if trust == Trust::Pinned {
            ServiceName::Gateway
        } else {
            ServiceName::Frontend
        };
        let config = machine_client_config(
            Some(server),
            expected,
            tls.as_ref(),
            Arc::clone(&self.clock),
        )?;
        Ok(HttpsClient::new(config))
    }
}

fn refusal(status: StatusCode) -> Option<EnrollRefusal> {
    [
        EnrollRefusal::Gone,
        EnrollRefusal::Invalid,
        EnrollRefusal::ProtocolUnsupported,
    ]
    .into_iter()
    .find(|refusal| refusal.status() == status.as_u16())
}

#[async_trait]
impl EnrollTransport for HttpsEnroll {
    async fn gateways(
        &self,
        address: &str,
        token: &EnrollmentToken,
    ) -> Result<Option<orch_link_contract::gateways::GatewayDiscovery>, EnrollError> {
        let base = enrollment_base(address)
            .map_err(|error| EnrollError::Unreachable(error.to_string()))?;
        let order = self
            .only
            .map_or_else(|| vec![Trust::Public, Trust::Pinned], |only| vec![only]);
        for trust in order {
            let response = self
                .client(trust, token.pin(), None, None)?
                .for_enrollment()
                .send(
                    &base,
                    Method::GET,
                    orch_link_contract::gateways::DISCOVERY_PATH,
                    &[],
                    Bytes::new(),
                    std::time::Duration::from_secs(u64::from(REQUEST_TIMEOUT_SECS)),
                )
                .await;
            match response {
                Ok(reply)
                    if reply.status == StatusCode::NOT_FOUND
                        || reply.status == StatusCode::METHOD_NOT_ALLOWED =>
                {
                    return Ok(None);
                }
                Ok(reply) if reply.status == StatusCode::OK => {
                    let discovery: orch_link_contract::gateways::GatewayDiscovery =
                        serde_json::from_slice(&reply.body).map_err(|_| {
                            EnrollError::Unreachable(
                                "the gateway catalogue could not be read".into(),
                            )
                        })?;
                    let authority = Certificate::from_pem(&discovery.ca_pem)?;
                    if !token.pins(&authority.spki_sha256()) {
                        return Err(EnrollError::Unreachable(
                            "the gateway catalogue belongs to another installation".into(),
                        ));
                    }
                    return Ok(Some(discovery));
                }
                Err(HttpsError::Tls(_)) => continue,
                Ok(_) => {
                    return Err(EnrollError::Unreachable(
                        "the verified enrollment entrance refused gateway discovery".into(),
                    ));
                }
                Err(error) => return Err(EnrollError::Unreachable(error.to_string())),
            }
        }
        Err(EnrollError::Unreachable(
            "the gateway catalogue's TLS identity could not be verified".into(),
        ))
    }

    async fn probe_gateway(
        &self,
        authority: &Certificate,
        endpoint: &Endpoint,
    ) -> Result<orch_link_contract::gateways::GatewayMeasurement, orch_link::reconnect::DialError>
    {
        use orch_link::gateways::GatewayProber as _;
        let prober = crate::gateway_selection::HttpsGatewayProber::for_enrollment(
            authority,
            Arc::clone(&self.clock),
        )
        .map_err(|error| orch_link::reconnect::DialError::Tls(error.to_string()))?;
        prober.measure(endpoint).await
    }

    async fn enroll_at(
        &self,
        endpoint: &Endpoint,
        token: &EnrollmentToken,
        request: &EnrollRequest,
    ) -> Result<EnrollReply, EnrollError> {
        HttpsEnroll::new(Arc::clone(&self.clock), Some(endpoint.trust))
            .enroll(&endpoint.url, token, request)
            .await
    }
    async fn enroll(
        &self,
        address: &str,
        token: &EnrollmentToken,
        request: &EnrollRequest,
    ) -> Result<EnrollReply, EnrollError> {
        let base = enrollment_base(address).map_err(|e| EnrollError::Unreachable(e.to_string()))?;
        let body = serde_json::to_vec(request).map_err(|e| EnrollError::Config(e.to_string()))?;
        let order = match self.only {
            Some(only) => vec![only],
            None => vec![Trust::Public, Trust::Pinned],
        };
        let mut last = EnrollError::Unreachable("no way to trust the address".to_owned());
        for trust in order {
            let client = self
                .client(trust, token.pin(), None, None)?
                .for_enrollment();
            let reply = client
                .send(
                    &base,
                    Method::POST,
                    ENROLL_PATH,
                    &[("content-type", "application/json".to_owned())],
                    Bytes::from(body.clone()),
                    std::time::Duration::from_secs(u64::from(REQUEST_TIMEOUT_SECS)),
                )
                .await;
            match reply {
                Ok(reply)
                    if reply.status == StatusCode::CREATED || reply.status == StatusCode::OK =>
                {
                    return serde_json::from_slice(&reply.body).map_err(|e| {
                        EnrollError::Unreachable(format!("the answer cannot be read: {e}"))
                    });
                }
                Ok(reply) => {
                    if serde_json::from_slice::<serde_json::Value>(&reply.body)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("code")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        })
                        .as_deref()
                        == Some("enroll_again_required")
                    {
                        return Err(EnrollError::Config("This token cannot replace the identity in this data directory. Use Enroll again on this machine's page.".to_owned()));
                    }
                    return Err(refusal(reply.status).map_or_else(
                        || {
                            EnrollError::Unreachable(format!(
                                "the system answered {}",
                                reply.status
                            ))
                        },
                        EnrollError::Refused,
                    ));
                }
                // A server whose certificate is not trusted this way may be trusted the other way.
                Err(HttpsError::Tls(why)) => last = EnrollError::Unreachable(format!("TLS: {why}")),
                Err(other) => return Err(EnrollError::Unreachable(other.to_string())),
            }
        }
        Err(last)
    }

    async fn renew(
        &self,
        endpoint: &Endpoint,
        identity: &MachineIdentity,
        request: &RenewRequest,
    ) -> Result<RenewReply, EnrollError> {
        let base = endpoint
            .https_base()
            .ok_or_else(|| EnrollError::Unreachable(format!("{} is not an address", endpoint.url)))
            .and_then(|text| {
                https_base(&text).map_err(|e| EnrollError::Unreachable(e.to_string()))
            })?;
        let pin = identity.authority.spki_sha256();
        let client = self.client(
            endpoint.trust,
            &pin,
            Some(identity),
            Some(&identity.authority),
        )?;
        let mut headers = vec![("content-type", "application/json".to_owned())];
        if endpoint.trust == Trust::Public {
            let host = base.host_str().unwrap_or_default().to_ascii_lowercase();
            let proof = renewal::sign_request(
                &identity.key,
                &identity.certificate,
                identity.machine_id,
                &request.csr_pem,
                &host,
                self.clock.as_ref(),
            );
            headers.push((
                "x-orch-certificate",
                URL_SAFE_NO_PAD.encode(proof.certificate_pem),
            ));
            headers.push(("x-orch-signature", proof.signature));
            headers.push(("x-orch-sent-at", proof.sent_at.to_string()));
            headers.push(("x-orch-machine", identity.machine_id.to_string()));
        }
        let body = serde_json::to_vec(request).map_err(|e| EnrollError::Config(e.to_string()))?;
        let reply = client
            .send(
                &base,
                Method::POST,
                RENEW_PATH,
                &headers,
                Bytes::from(body),
                std::time::Duration::from_secs(u64::from(REQUEST_TIMEOUT_SECS)),
            )
            .await
            .map_err(|e| EnrollError::Unreachable(e.to_string()))?;
        if reply.status != StatusCode::OK {
            return Err(EnrollError::Unreachable(format!(
                "the system answered {} to the request for a new certificate",
                reply.status
            )));
        }
        serde_json::from_slice(&reply.body)
            .map_err(|e| EnrollError::Unreachable(format!("the answer cannot be read: {e}")))
    }
}

// ---------------------------------------------------------------------------------------------------------
// Enrolling

/// What a connector enrolls with.
#[derive(Debug, Clone)]
pub struct EnrollInputs {
    /// The system's HTTPS address or gateway's WSS address; local development enrollment also accepts loopback HTTP.
    pub address: String,
    /// The token.
    pub token: String,
    /// What the machine is to be called.
    pub name: String,
    /// The roles it plays.
    pub roles: Vec<Role>,
    /// What it is built for.
    pub platform: Platform,
    /// The connector's own version.
    pub connector_version: String,
    /// Whether the system made the machine.
    pub managed: bool,
}

/// An enrollment that was kept.
#[derive(Debug)]
pub struct Enrolled {
    /// The identity, as it is on disk.
    pub identity: MachineIdentity,
    /// What was recorded.
    pub record: EnrollmentRecord,
    /// The endpoints the system gave, in the order to try them.
    pub endpoints: Vec<Endpoint>,
}

/// Enrolls into a system and keeps the answer. Nothing is kept unless the answer is believed.
pub async fn enroll(
    inputs: &EnrollInputs,
    data_dir: &DataDir,
    transport: &dyn EnrollTransport,
    clock: &dyn Clock,
) -> Result<Enrolled, EnrollError> {
    enroll_inner(inputs, data_dir, transport, clock, None).await
}

/// Enrolls with the roles, handover and dedicated declaration the local command explicitly accepted.
pub async fn enroll_with_choices(
    inputs: &EnrollInputs,
    data_dir: &DataDir,
    transport: &dyn EnrollTransport,
    clock: &dyn Clock,
    choices: &EnrollmentChoices,
) -> Result<Enrolled, EnrollError> {
    let choices = EnrollmentChoices::new(
        choices.roles.clone(),
        choices.delegate.clone(),
        choices.dedicated,
    )
    .map_err(|message| EnrollError::Config(message.into()))?;
    enroll_inner(inputs, data_dir, transport, clock, Some(&choices)).await
}

async fn enroll_inner(
    inputs: &EnrollInputs,
    data_dir: &DataDir,
    transport: &dyn EnrollTransport,
    clock: &dyn Clock,
    choices: Option<&EnrollmentChoices>,
) -> Result<Enrolled, EnrollError> {
    let token: EnrollmentToken = inputs
        .token
        .trim()
        .parse()
        .map_err(|_| EnrollError::BadToken)?;
    if let Some(bound) = token.choices().map_err(|_| EnrollError::BadToken)? {
        let local = match choices {
            Some(local) => {
                EnrollmentChoices::new(local.roles.clone(), local.delegate.clone(), local.dedicated)
            }
            None => EnrollmentChoices::new(inputs.roles.clone(), Vec::new(), false),
        }
        .map_err(|message| EnrollError::Config(message.into()))?;
        if !bound.matches(&local) {
            return Err(EnrollError::Config("The roles, handover or dedicated choice does not match this token. Run the complete enrollment command shown by Orch.".into()));
        }
    }
    if let Some(local) = choices
        && (local.roles.len() != inputs.roles.len()
            || !local.roles.iter().all(|role| inputs.roles.contains(role)))
    {
        return Err(EnrollError::Config(
            "The local enrollment roles disagree with the command.".into(),
        ));
    }
    let previous = if data_dir.is_enrolled() {
        Some((data_dir.record()?, data_dir.identity()?))
    } else {
        None
    };
    if let Some((_, identity)) = &previous
        && !token.pins(&identity.authority.spki_sha256())
    {
        return Err(EnrollError::Config("This data directory belongs to another system. Use Enroll again on its machine's page, or a new data directory for another system.".to_owned()));
    }
    let config = data_dir.config(|_| None)?;
    let discovery = transport.gateways(&inputs.address, &token).await?;
    let catalogue = discovery
        .as_ref()
        .map(|reply| reply.gateways.clone())
        .or_else(|| previous.as_ref().map(|(record, _)| record.gateways.clone()))
        .unwrap_or_default();
    let selection = orch_link::gateways::PrimaryGateway {
        choice: config.connector.gateway.clone(),
        catalogue,
    };
    if selection.choice.selection == orch_link_contract::gateways::GatewaySelection::Selected
        && !selection
            .catalogue
            .iter()
            .any(|gateway| Some(gateway.id.as_str()) == selection.choice.gateway_id.as_deref())
    {
        return Err(EnrollError::Config(
            "the selected gateway is not advertised by this installation".into(),
        ));
    }
    let mut selected = Vec::new();
    let mut gateway_measurements = Vec::new();
    if !selection.catalogue.is_empty()
        || selection.choice.selection == orch_link_contract::gateways::GatewaySelection::Selected
    {
        let local = endpoints_of(&config);
        let candidates = if local.is_empty() {
            selection
                .catalogue
                .iter()
                .flat_map(|gateway| gateway.endpoints.clone())
                .collect()
        } else {
            local
        };
        let authority = discovery
            .as_ref()
            .map(|reply| Certificate::from_pem(&reply.ca_pem))
            .transpose()?
            .or_else(|| {
                previous
                    .as_ref()
                    .map(|(_, identity)| identity.authority.clone())
            })
            .ok_or_else(|| {
                EnrollError::Unreachable("the catalogue has no verified authority".into())
            })?;
        let prober = EnrollmentProbe {
            transport,
            authority: &authority,
        };
        let (ordered, measurements) = selection
            .candidates(&prober, &candidates, None)
            .await
            .map_err(EnrollError::Unreachable)?;
        for candidate in ordered {
            if !selected.contains(&candidate.endpoint) {
                selected.push(candidate.endpoint);
            }
        }
        gateway_measurements = measurements;
    }
    let pending = PendingEnrollment::start()?;
    let request = EnrollRequest {
        machine_id: previous.as_ref().map(|(record, _)| record.machine_id),
        token: inputs.token.trim().to_owned(),
        csr_pem: pending.csr_pem().to_owned(),
        name: inputs.name.clone(),
        platform: inputs.platform,
        connector_version: inputs.connector_version.clone(),
        protocol_versions: vec![PROTOCOL_VERSION],
        roles: inputs.roles.clone(),
    };
    let reply = if selected.is_empty() {
        transport.enroll(&inputs.address, &token, &request).await?
    } else {
        let mut result = Err(EnrollError::Unreachable(
            "no advertised gateway entrance answered enrollment".into(),
        ));
        for endpoint in selected {
            match transport.enroll_at(&endpoint, &token, &request).await {
                Err(error @ EnrollError::Unreachable(_)) => result = Err(error),
                answer => {
                    result = answer;
                    break;
                }
            }
        }
        result?
    };
    if previous
        .as_ref()
        .is_some_and(|(record, _)| record.machine_id != reply.machine_id)
    {
        return Err(EnrollError::Config("This token enrolled another machine. The existing identity was kept; use Enroll again on this machine's page.".to_owned()));
    }
    let identity = pending
        .complete(&token, &reply, clock)
        .map_err(EnrollError::Distrusted)?;
    identity.save(&data_dir.identity_dir())?;
    let record = EnrollmentRecord {
        gateway_measured_at: (!gateway_measurements.is_empty()).then(|| clock.time()),
        gateways: reply.gateways.clone(),
        gateway_measurements,
        server_endpoints: reply.endpoints.clone(),
        machine_id: reply.machine_id,
        name: previous
            .as_ref()
            .map_or_else(|| inputs.name.clone(), |(record, _)| record.name.clone()),
        system: reply.system.clone(),
        pin: URL_SAFE_NO_PAD.encode(token.pin()),
        protocol_version: reply.protocol_version,
        enrolled_at: clock.time(),
        managed: previous
            .as_ref()
            .map_or(inputs.managed, |(record, _)| record.managed),
    };
    let path = data_dir.identity_dir().join(RECORD_FILE);
    let bytes =
        serde_json::to_vec_pretty(&record).map_err(|e| EnrollError::Config(e.to_string()))?;
    write_atomically(&path, &bytes).map_err(io(&path))?;
    let local = endpoints_of(&data_dir.config(|_| None)?);
    let endpoints = if local.is_empty() {
        reply.endpoints.clone()
    } else {
        local
    };
    let roles: Vec<String> = inputs.roles.iter().map(role_name).collect();
    let name = inputs.name.clone();
    if previous.is_none() {
        data_dir.update_config(|config| {
            config.connector.name = name;
            config.connector.roles = roles;
            if let Some(local) = choices {
                config.policy.delegated = local.delegate.clone();
                if !local.roles.contains(&Role::Node) {
                    config.connector.dedicated = local.dedicated;
                }
            }
        })?;
    }
    if previous.is_some() {
        match crate::local_socket::ask(
            data_dir.root(),
            &crate::local_socket::Request::ReloadIdentity,
        )
        .await
        {
            Ok(crate::local_socket::Reply::Done { .. })
            | Err(crate::local_socket::LocalError::NotRunning(_)) => {}
            Ok(_) | Err(_) => tracing::warn!(
                "the replacement identity is saved; restart this machine's runner to use it"
            ),
        }
    }
    Ok(Enrolled {
        identity,
        record,
        endpoints,
    })
}

pub(crate) struct EnrollmentProbe<'a> {
    pub(crate) transport: &'a dyn EnrollTransport,
    pub(crate) authority: &'a Certificate,
}

#[async_trait]
impl orch_link::reconnect::Dialer for EnrollmentProbe<'_> {
    async fn dial(
        &self,
        _endpoint: &Endpoint,
    ) -> Result<orch_link::framing::Connection, orch_link::reconnect::DialError> {
        Err(orch_link::reconnect::DialError::Connect(
            "enrollment probes do not establish machine sessions".into(),
        ))
    }

    async fn probe(
        &self,
        endpoint: &Endpoint,
    ) -> Result<orch_link_contract::gateways::GatewayMeasurement, orch_link::reconnect::DialError>
    {
        self.transport.probe_gateway(self.authority, endpoint).await
    }
}

/// The name of a role, as a configuration writes it.
#[must_use]
pub fn role_name(role: &Role) -> String {
    match role {
        Role::Provider => "provider",
        Role::Agent => "agent",
        Role::Environment => "environment",
        Role::Node => "node",
    }
    .to_owned()
}

/// The roles a configuration names.
#[must_use]
pub fn roles_of(config: &ConnectorConfig) -> Vec<Role> {
    config
        .connector
        .roles
        .iter()
        .filter_map(|name| match name.as_str() {
            "provider" => Some(Role::Provider),
            "agent" => Some(Role::Agent),
            "environment" => Some(Role::Environment),
            "node" => Some(Role::Node),
            _ => None,
        })
        .collect()
}

/// The endpoints a configuration names.
#[must_use]
pub fn endpoints_of(config: &ConnectorConfig) -> Vec<Endpoint> {
    config
        .endpoints
        .iter()
        .map(|endpoint| Endpoint {
            url: endpoint.url.clone(),
            trust: if endpoint.trust == "public" {
                Trust::Public
            } else {
                Trust::Pinned
            },
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------------------
// Renewing

/// Renews the certificate if two thirds of its life have passed, with a key made anew, and puts the new
/// pair in place of the old. Returns whether it was renewed.
pub async fn renew_if_due(
    identity: &mut MachineIdentity,
    data_dir: &DataDir,
    endpoints: &[Endpoint],
    transport: &dyn EnrollTransport,
    clock: &dyn Clock,
) -> Result<bool, EnrollError> {
    if !renewal::due(&identity.certificate, clock.time()) {
        return Ok(false);
    }
    let (key, csr_pem) = renewal::start()?;
    let request = RenewRequest { csr_pem };
    let mut last = EnrollError::Unreachable("there is no endpoint to renew at".to_owned());
    for endpoint in endpoints {
        match transport.renew(endpoint, identity, &request).await {
            Ok(reply) => {
                accept_renewal(identity, data_dir, key, &reply)?;
                return Ok(true);
            }
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// How often a connector looks at its certificate to see whether it is due: an hour. A certificate
/// lives a year and the renewal has its last third, four months, to succeed in, so an hour is a delay
/// nobody notices, and the look is a comparison of two numbers; looking and not trusting a timer set at
/// start also survives a computer that slept for a night, which a timer of the process does not.
pub const RENEWAL_LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(3_600);

/// Waits until the certificate on disk is due, renews it, and returns the new identity once it is in
/// place on disk. A renewal that fails (the system cannot be reached, the answer is not believed) is
/// logged and tried again at the next look; the function does not return before one succeeds, so a
/// caller that has other things to wait for runs it beside them and drops it.
pub async fn renew_when_due(
    data_dir: &DataDir,
    endpoints: &[Endpoint],
    transport: &dyn EnrollTransport,
    clock: &dyn Clock,
    look_every: std::time::Duration,
) -> MachineIdentity {
    renew_when_due_using(
        data_dir,
        || endpoints.to_vec(),
        transport,
        clock,
        look_every,
    )
    .await
}

/// The supervisor reads its live effective list at each renewal attempt.
pub(crate) async fn renew_when_due_using(
    data_dir: &DataDir,
    endpoints: impl Fn() -> Vec<Endpoint>,
    transport: &dyn EnrollTransport,
    clock: &dyn Clock,
    look_every: std::time::Duration,
) -> MachineIdentity {
    loop {
        match data_dir.identity() {
            Ok(mut identity) => {
                match renew_if_due(&mut identity, data_dir, &endpoints(), transport, clock).await {
                    Ok(true) => return identity,
                    Ok(false) => {}
                    Err(error) => {
                        tracing::warn!(%error, "the certificate is due and could not be renewed; trying again later");
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "the identity could not be read to see whether it is due")
            }
        }
        tokio::time::sleep(look_every).await;
    }
}

fn accept_renewal(
    identity: &mut MachineIdentity,
    data_dir: &DataDir,
    key: LeafKey,
    reply: &RenewReply,
) -> Result<(), EnrollError> {
    let certificate = Certificate::from_pem(&reply.certificate_pem)?;
    let authority = Certificate::from_pem(&reply.ca_pem)?;
    if authority.spki_sha256() != identity.authority.spki_sha256() {
        return Err(EnrollError::Distrusted(TrustFault::WrongPin));
    }
    if !certificate.holds(&key) || certificate.machine_id() != Some(identity.machine_id) {
        return Err(EnrollError::Distrusted(TrustFault::NotMine));
    }
    identity.replace(&data_dir.identity_dir(), key, certificate)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------
// A runtime the system made

/// What a driver gives the connector it starts in a managed runtime: the launch variables of
/// `orch_driver_contract::runtime::LAUNCH_VARIABLES`, read, and the runtime the system says the unit is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedInputs {
    /// The roles it plays.
    pub roles: Vec<Role>,
    /// The runtime this unit is of (`ORCH_RUNTIME_ID`, which the system puts in the unit's variables): the machine
    /// state in the data directory is marked with it.
    pub unit: String,
    /// Where it reaches the gateway.
    pub gateway_url: String,
    /// The file that holds the certificate authority.
    pub ca_file: PathBuf,
    /// The file that holds the enrollment token; used at the first start only.
    pub enroll_file: PathBuf,
    /// The data directory, on the state volume.
    pub data_dir: PathBuf,
    /// The read-only directory of tools.
    pub tools_dir: PathBuf,
    /// The workspace volume, of an environment.
    pub workspace_dir: Option<PathBuf>,
    /// The home volume, of an environment.
    pub home_dir: Option<PathBuf>,
    /// The file that lists the mounts the driver attached, of an environment.
    pub mounts_file: Option<PathBuf>,
}

impl ManagedInputs {
    /// Reads the launch variables through `var`. Every one that is missing is named.
    pub fn from_variables(var: impl Fn(&str) -> Option<String>) -> Result<Self, EnrollError> {
        let mut missing = Vec::new();
        let mut need = |name: &str| {
            let value = var(name).filter(|value| !value.is_empty());
            if value.is_none() {
                missing.push(name.to_owned());
            }
            value
        };
        let roles_text = need("ORCH_ROLES");
        let unit = need("ORCH_RUNTIME_ID");
        let gateway_url = need("ORCH_GATEWAY_URL");
        let ca_file = need("ORCH_CA_FILE");
        let enroll_file = need("ORCH_ENROLL_FILE");
        let data_dir = need("ORCH_DATA_DIR");
        let tools_dir = need("ORCH_TOOLS_DIR");
        let mut roles = Vec::new();
        let mut bad = Vec::new();
        for name in roles_text.iter().flat_map(|text| text.split(',')) {
            match name.trim() {
                "environment" => roles.push((ConnectorRole::Environment, Role::Environment)),
                "provider" => roles.push((ConnectorRole::Provider, Role::Provider)),
                "agent" => roles.push((ConnectorRole::Agent, Role::Agent)),
                other => bad.push(other.to_owned()),
            }
        }
        let environment = roles
            .iter()
            .any(|(driver, _)| *driver == ConnectorRole::Environment);
        let (workspace_dir, home_dir, mounts_file) = if environment {
            (
                need("ORCH_WORKSPACE_DIR"),
                need("ORCH_HOME_DIR"),
                need("ORCH_MOUNTS_FILE"),
            )
        } else {
            (None, None, None)
        };
        if !bad.is_empty() {
            return Err(EnrollError::Launch(format!(
                "ORCH_ROLES names {} which are no roles",
                bad.join(", ")
            )));
        }
        if !missing.is_empty() {
            return Err(EnrollError::Launch(format!(
                "{} not set",
                missing.join(", ")
            )));
        }
        let required = |value: Option<String>| value.unwrap_or_default();
        Ok(Self {
            roles: roles.into_iter().map(|(_, role)| role).collect(),
            unit: required(unit),
            gateway_url: required(gateway_url),
            ca_file: PathBuf::from(required(ca_file)),
            enroll_file: PathBuf::from(required(enroll_file)),
            data_dir: PathBuf::from(required(data_dir)),
            tools_dir: PathBuf::from(required(tools_dir)),
            workspace_dir: workspace_dir.map(PathBuf::from),
            home_dir: home_dir.map(PathBuf::from),
            mounts_file: mounts_file.map(PathBuf::from),
        })
    }

    /// The enrollment this runtime makes at its first start: with the token in the file the driver made,
    /// at the gateway's pinned address. The certificate authority the driver gave has to be the one the
    /// token pins, or nothing is sent.
    pub fn enrollment(
        &self,
        name: &str,
        platform: Platform,
        version: &str,
    ) -> Result<EnrollInputs, EnrollError> {
        let token = std::fs::read_to_string(&self.enroll_file).map_err(io(&self.enroll_file))?;
        let token = token.trim().to_owned();
        let parsed: EnrollmentToken = token.parse().map_err(|_| EnrollError::BadToken)?;
        let ca_text = std::fs::read_to_string(&self.ca_file).map_err(io(&self.ca_file))?;
        let authority = Certificate::from_pem(&ca_text)?;
        if !parsed.pins(&authority.spki_sha256()) {
            return Err(EnrollError::Distrusted(TrustFault::WrongPin));
        }
        Ok(EnrollInputs {
            address: self.gateway_url.clone(),
            token,
            name: name.to_owned(),
            roles: self.roles.clone(),
            platform,
            connector_version: version.to_owned(),
            managed: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use orch_link_contract::enrollment::EnrollReply;
    use orch_link_contract::session::{Arch, Os};
    use orch_pki::ca::CertificateAuthority;
    use orch_pki::enrollment::{MemoryLedger, TokenGrant, TokenPurpose, issue_token};
    use orch_types::{ManualClock, new_id};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A system in memory: an authority, a ledger of tokens, the endpoints it gives.
    struct System {
        authority: CertificateAuthority,
        ledger: MemoryLedger,
        clock: ManualClock,
        sent: Mutex<Vec<EnrollRequest>>,
        renewals: Mutex<usize>,
        lie_about_the_authority: bool,
    }

    impl System {
        fn new(clock: &ManualClock) -> Self {
            Self {
                authority: CertificateAuthority::create("Test", 3650, clock).unwrap(),
                ledger: MemoryLedger::new(),
                clock: clock.clone(),
                sent: Mutex::new(Vec::new()),
                renewals: Mutex::new(0),
                lie_about_the_authority: false,
            }
        }

        fn token(&self) -> String {
            self.token_choices(None)
        }

        fn token_choices(&self, choices: Option<&EnrollmentChoices>) -> String {
            let mut issued = issue_token(
                &self.authority,
                std::time::Duration::from_secs(900),
                &self.clock,
            );
            if let Some(choices) = choices {
                issued.token = issued.token.with_choices(choices).unwrap();
                issued.digest = orch_pki::enrollment::token_digest(issued.token.secret());
            }
            self.ledger.record(
                &issued,
                TokenGrant {
                    purpose: TokenPurpose::Machine,
                    created_by: new_id(),
                },
            );
            issued.token.to_string()
        }
    }

    #[async_trait]
    impl EnrollTransport for System {
        async fn enroll(
            &self,
            _address: &str,
            _token: &EnrollmentToken,
            request: &EnrollRequest,
        ) -> Result<EnrollReply, EnrollError> {
            self.sent.lock().unwrap().push(request.clone());
            let enrolled = orch_pki::enrollment::enroll(
                &self.authority,
                &self.ledger,
                request,
                365,
                &self.clock,
            )
            .map_err(|e| match e {
                orch_pki::enrollment::EnrollError::Refused(refusal) => {
                    EnrollError::Refused(refusal)
                }
                other => EnrollError::Unreachable(other.to_string()),
            })?;
            let other;
            let ca_pem = if self.lie_about_the_authority {
                other = CertificateAuthority::create("Other", 3650, &self.clock).unwrap();
                other.certificate_pem()
            } else {
                self.authority.certificate_pem()
            };
            Ok(EnrollReply {
                gateways: Vec::new(),
                machine_id: enrolled.machine_id,
                certificate_pem: enrolled.certificate.pem(),
                ca_pem,
                endpoints: vec![Endpoint {
                    url: "wss://gw.orch.internal:9443".into(),
                    trust: Trust::Pinned,
                }],
                system: EnrollReplySystem {
                    name: "Orch".into(),
                    public_url: "https://orch.example.com".into(),
                },
                protocol_version: enrolled.protocol_version,
            })
        }

        async fn renew(
            &self,
            _endpoint: &Endpoint,
            identity: &MachineIdentity,
            request: &RenewRequest,
        ) -> Result<RenewReply, EnrollError> {
            *self.renewals.lock().unwrap() += 1;
            let csr =
                orch_pki::certificates::CertificateRequest::from_pem(&request.csr_pem).unwrap();
            let certificate = self
                .authority
                .issue(
                    &csr,
                    &orch_pki::ca::Holder::Machine(identity.machine_id),
                    365,
                    &self.clock,
                )
                .unwrap();
            Ok(RenewReply {
                certificate_pem: certificate.pem(),
                ca_pem: self.authority.certificate_pem(),
            })
        }
    }

    fn inputs(token: String) -> EnrollInputs {
        EnrollInputs {
            address: "https://orch.example.com".into(),
            token,
            name: "alice-laptop".into(),
            roles: vec![Role::Agent, Role::Environment],
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            connector_version: "0.1.0".into(),
            managed: false,
        }
    }

    fn clock() -> ManualClock {
        ManualClock::new(
            chrono::Utc
                .with_ymd_and_hms(2026, 10, 11, 10, 0, 0)
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn enrollment_keeps_the_locally_accepted_roles_handover_and_dedicated_fact_without_widening_a_kept_value()
     {
        let clock = clock();
        let system = System::new(&clock);
        let directory = tempfile::tempdir().unwrap();
        let data = DataDir::open(directory.path()).unwrap();
        std::fs::write(data.config_path(), "[environment]\nremote_write = false\n").unwrap();
        let choices = EnrollmentChoices::new(
            vec![Role::Environment, Role::Provider],
            vec!["environment".into()],
            true,
        )
        .unwrap();
        let mut inputs = inputs(system.token_choices(Some(&choices)));
        inputs.roles = choices.roles.clone();
        enroll_with_choices(&inputs, &data, &system, &clock, &choices)
            .await
            .unwrap();
        let config = data.config(|_| None).unwrap();
        assert_eq!(config.connector.roles, ["environment", "provider"]);
        assert_eq!(config.policy.delegated, ["environment.*"]);
        assert!(config.connector.dedicated);
        assert_eq!(system.sent.lock().unwrap()[0].roles, choices.roles);
        let store =
            crate::policy::PolicyStore::connected(config, Some(data.config_path())).unwrap();
        let policy = store.policy().unwrap();
        assert!(
            !policy.environment_remote_write,
            "an explicit local denial still wins"
        );
        assert!(
            policy.environment_terminal,
            "an unset handed key uses the role's managed value"
        );
        assert!(
            !policy.provider_signin,
            "handing over Environment did not hand over Provider"
        );
    }

    #[tokio::test]
    async fn a_token_cannot_silently_add_handover_or_dedication_to_the_local_command() {
        let clock = clock();
        let system = System::new(&clock);
        let directory = tempfile::tempdir().unwrap();
        let data = DataDir::open(directory.path()).unwrap();
        let bound =
            EnrollmentChoices::new(vec![Role::Agent, Role::Environment], vec!["*".into()], true)
                .unwrap();
        let inputs = inputs(system.token_choices(Some(&bound)));
        let local = EnrollmentChoices::new(inputs.roles.clone(), Vec::new(), false).unwrap();
        let failure = enroll_with_choices(&inputs, &data, &system, &clock, &local)
            .await
            .unwrap_err();
        assert!(failure.to_string().contains("does not match this token"));
        assert!(
            system.sent.lock().unwrap().is_empty(),
            "mismatch refuses before a request or key is sent"
        );
        assert!(!data.is_enrolled());
        assert!(!data.config_path().exists());
    }

    #[tokio::test]
    async fn changing_the_choice_suffix_does_not_reuse_the_issued_tokens_secret_hash() {
        let clock = clock();
        let system = System::new(&clock);
        let directory = tempfile::tempdir().unwrap();
        let data = DataDir::open(directory.path()).unwrap();
        let original = EnrollmentChoices::new(vec![Role::Environment], Vec::new(), false).unwrap();
        let good: EnrollmentToken = system.token_choices(Some(&original)).parse().unwrap();
        let base = good.secret().split_once(".roles1.").unwrap().0;
        let altered =
            EnrollmentChoices::new(vec![Role::Provider], vec!["provider".into()], true).unwrap();
        let forged = EnrollmentToken::new(base, *good.pin())
            .with_choices(&altered)
            .unwrap();
        let mut request = inputs(forged.to_string());
        request.roles = altered.roles.clone();
        let failure = enroll_with_choices(&request, &data, &system, &clock, &altered)
            .await
            .unwrap_err();
        assert!(matches!(failure, EnrollError::Refused(EnrollRefusal::Gone)));
        assert!(
            !data.is_enrolled(),
            "editing a choice is not a new authority grant"
        );
    }

    #[tokio::test]
    async fn a_connector_enrolls_and_keeps_its_key_its_certificate_and_what_the_system_said() {
        let clock = clock();
        let system = System::new(&clock);
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        assert!(!data.is_enrolled());
        let enrolled = enroll(&inputs(system.token()), &data, &system, &clock)
            .await
            .unwrap();
        assert!(data.is_enrolled());
        let again = data.identity().unwrap();
        assert_eq!(again.machine_id, enrolled.record.machine_id);
        assert!(again.certificate.holds(&again.key));
        let record = data.record().unwrap();
        assert_eq!(record.system.name, "Orch");
        assert_eq!(record.name, "alice-laptop");
        let config = data.config(|_| None).unwrap();
        assert_eq!(data.endpoints(&config).unwrap(), enrolled.endpoints);
        assert!(
            config.endpoints.is_empty(),
            "the received list is not an owner override"
        );
        assert_eq!(config.connector.name, "alice-laptop");
        assert_eq!(roles_of(&config), [Role::Agent, Role::Environment]);
        let sent = system.sent.lock().unwrap();
        assert!(sent[0].csr_pem.contains("CERTIFICATE REQUEST"));
        assert!(!sent[0].csr_pem.contains("PRIVATE"), "the key never leaves");
    }

    #[tokio::test]
    async fn the_roles_a_node_enrolled_with_are_read_back_from_its_file() {
        let clock = clock();
        let system = System::new(&clock);
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        let node = EnrollInputs {
            roles: vec![Role::Node],
            managed: true,
            ..inputs(system.token())
        };
        enroll(&node, &data, &system, &clock).await.unwrap();
        let config = data.config(|_| None).unwrap();
        assert_eq!(config.connector.roles, ["node"]);
        assert_eq!(roles_of(&config), [Role::Node]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_identity_is_readable_by_its_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let clock = clock();
        let system = System::new(&clock);
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        enroll(&inputs(system.token()), &data, &system, &clock)
            .await
            .unwrap();
        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(data.identity_dir().join("key.pem")), 0o600);
        assert_eq!(mode(data.identity_dir()), 0o700);
        assert_eq!(mode(dir.path().to_owned()), 0o700);
    }

    #[tokio::test]
    async fn an_answer_from_an_authority_the_token_does_not_pin_is_refused_and_nothing_is_kept() {
        let clock = clock();
        let mut system = System::new(&clock);
        system.lie_about_the_authority = true;
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        let error = enroll(&inputs(system.token()), &data, &system, &clock)
            .await
            .unwrap_err();
        assert!(
            matches!(error, EnrollError::Distrusted(TrustFault::WrongPin)),
            "{error}"
        );
        assert!(!data.is_enrolled());
        assert!(!data.identity_dir().join("key.pem").exists());
        assert!(!data.config_path().exists());
    }

    #[tokio::test]
    async fn a_token_that_was_used_is_refused_and_nothing_is_kept() {
        let clock = clock();
        let system = System::new(&clock);
        let token = system.token();
        let first = tempfile::tempdir().unwrap();
        enroll(
            &inputs(token.clone()),
            &DataDir::open(first.path()).unwrap(),
            &system,
            &clock,
        )
        .await
        .unwrap();
        let second = tempfile::tempdir().unwrap();
        let data = DataDir::open(second.path()).unwrap();
        let error = enroll(&inputs(token), &data, &system, &clock)
            .await
            .unwrap_err();
        assert!(
            matches!(error, EnrollError::Refused(EnrollRefusal::Gone)),
            "{error}"
        );
        assert!(!data.is_enrolled());
    }

    #[tokio::test]
    async fn something_that_is_no_token_is_refused_before_anything_is_sent() {
        let clock = clock();
        let system = System::new(&clock);
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        let error = enroll(&inputs("not-a-token".into()), &data, &system, &clock)
            .await
            .unwrap_err();
        assert!(matches!(error, EnrollError::BadToken));
        assert!(system.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_certificate_is_renewed_after_two_thirds_of_its_life_with_a_new_key() {
        let clock = clock();
        let system = System::new(&clock);
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        let enrolled = enroll(&inputs(system.token()), &data, &system, &clock)
            .await
            .unwrap();
        let mut identity = data.identity().unwrap();
        let old_fingerprint = identity.certificate.fingerprint();
        let old_key = identity.key.public_point();
        assert!(
            !renew_if_due(&mut identity, &data, &enrolled.endpoints, &system, &clock)
                .await
                .unwrap(),
            "a new certificate is not renewed"
        );
        clock.advance(chrono::Duration::days(242));
        assert!(
            !renew_if_due(&mut identity, &data, &enrolled.endpoints, &system, &clock)
                .await
                .unwrap()
        );
        clock.advance(chrono::Duration::days(3));
        assert!(
            renew_if_due(&mut identity, &data, &enrolled.endpoints, &system, &clock)
                .await
                .unwrap()
        );
        assert_ne!(identity.certificate.fingerprint(), old_fingerprint);
        assert_ne!(
            identity.key.public_point(),
            old_key,
            "the new certificate is over a new key"
        );
        let reloaded = data.identity().unwrap();
        assert_eq!(
            reloaded.certificate.fingerprint(),
            identity.certificate.fingerprint()
        );
        assert_eq!(*system.renewals.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_renewal_answered_with_another_authority_or_another_key_is_not_taken() {
        let clock = clock();
        let system = System::new(&clock);
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        enroll(&inputs(system.token()), &data, &system, &clock)
            .await
            .unwrap();
        let mut identity = data.identity().unwrap();
        let (key, _) = renewal::start().unwrap();
        let strange = CertificateAuthority::create("Strange", 3650, &clock).unwrap();
        let reply = RenewReply {
            certificate_pem: identity.certificate.pem(),
            ca_pem: strange.certificate_pem(),
        };
        assert!(matches!(
            accept_renewal(&mut identity, &data, key, &reply),
            Err(EnrollError::Distrusted(TrustFault::WrongPin))
        ));
        let (key, _) = renewal::start().unwrap();
        let reply = RenewReply {
            certificate_pem: identity.certificate.pem(),
            ca_pem: system.authority.certificate_pem(),
        };
        assert!(matches!(
            accept_renewal(&mut identity, &data, key, &reply),
            Err(EnrollError::Distrusted(TrustFault::NotMine))
        ));
    }

    #[test]
    fn a_physical_runner_owns_only_its_directory_until_its_guard_is_dropped() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let data = DataDir::open(first.path()).unwrap();
        let guard = data.try_run_lock().unwrap().unwrap();
        assert!(data.try_run_lock().unwrap().is_none());
        assert!(
            DataDir::open(second.path())
                .unwrap()
                .try_run_lock()
                .unwrap()
                .is_some()
        );
        drop(guard);
        assert!(data.try_run_lock().unwrap().is_some());
    }

    #[test]
    fn the_launch_variables_of_a_runtime_are_read_and_each_that_is_missing_is_named() {
        let vars: HashMap<&str, &str> = HashMap::from([
            ("ORCH_ROLES", "environment"),
            ("ORCH_RUNTIME_ID", "0199abcd-1234"),
            ("ORCH_GATEWAY_URL", "wss://gateway.orch.internal:9443"),
            ("ORCH_CA_FILE", "/run/orch/ca.pem"),
            ("ORCH_ENROLL_FILE", "/run/orch/token"),
            ("ORCH_DATA_DIR", "/var/lib/orch"),
            ("ORCH_TOOLS_DIR", "/opt/orch/tools"),
            ("ORCH_WORKSPACE_DIR", "/workspace"),
            ("ORCH_HOME_DIR", "/home/agent"),
            ("ORCH_MOUNTS_FILE", "/run/orch/mounts.json"),
        ]);
        let var = |name: &str| vars.get(name).map(|v| (*v).to_owned());
        let read = ManagedInputs::from_variables(var).unwrap();
        assert_eq!(read.roles, [Role::Environment]);
        assert_eq!(read.unit, "0199abcd-1234");
        assert_eq!(read.workspace_dir.as_deref(), Some(Path::new("/workspace")));

        let few = |name: &str| (name == "ORCH_ROLES").then(|| "provider,agent".to_owned());
        let error = ManagedInputs::from_variables(few).unwrap_err().to_string();
        for name in [
            "ORCH_RUNTIME_ID",
            "ORCH_GATEWAY_URL",
            "ORCH_CA_FILE",
            "ORCH_ENROLL_FILE",
            "ORCH_DATA_DIR",
            "ORCH_TOOLS_DIR",
        ] {
            assert!(error.contains(name), "{error}");
        }
        assert!(
            !error.contains("ORCH_WORKSPACE_DIR"),
            "a provider's runtime has no workspace"
        );

        let wrong = |name: &str| (name == "ORCH_ROLES").then(|| "agent,janitor".to_owned());
        assert!(
            ManagedInputs::from_variables(wrong)
                .unwrap_err()
                .to_string()
                .contains("janitor")
        );
    }

    #[tokio::test]
    async fn a_runtime_enrolls_with_the_token_in_its_file_only_if_the_authority_it_was_given_is_the_one_pinned()
     {
        let clock = clock();
        let system = System::new(&clock);
        let dir = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        std::fs::write(
            files.path().join("ca.pem"),
            system.authority.certificate_pem(),
        )
        .unwrap();
        std::fs::write(files.path().join("token"), format!("{}\n", system.token())).unwrap();
        let launch = ManagedInputs {
            roles: vec![Role::Agent, Role::Provider],
            unit: "unit-a".into(),
            gateway_url: "wss://gateway.orch.internal:9443".into(),
            ca_file: files.path().join("ca.pem"),
            enroll_file: files.path().join("token"),
            data_dir: dir.path().to_owned(),
            tools_dir: dir.path().join("tools"),
            workspace_dir: None,
            home_dir: None,
            mounts_file: None,
        };
        let platform = Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        };
        let inputs = launch
            .enrollment("orch-provider-0199abcd1234", platform, "0.1.0")
            .unwrap();
        assert!(inputs.managed);
        assert_eq!(inputs.roles, [Role::Agent, Role::Provider]);
        let data = DataDir::open(dir.path()).unwrap();
        enroll(&inputs, &data, &system, &clock).await.unwrap();
        assert!(data.record().unwrap().managed);

        let other = CertificateAuthority::create("Other", 3650, &clock).unwrap();
        std::fs::write(files.path().join("ca.pem"), other.certificate_pem()).unwrap();
        assert!(matches!(
            launch.enrollment("x", platform, "0.1.0"),
            Err(EnrollError::Distrusted(TrustFault::WrongPin))
        ));
    }

    #[tokio::test]
    async fn a_unit_made_on_the_volume_of_an_earlier_one_enrolls_as_a_new_machine_keeping_what_a_person_signed_in_and_the_earlier_unit_gets_its_own_back()
     {
        let clock = clock();
        let system = System::new(&clock);
        let dir = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        std::fs::write(
            files.path().join("ca.pem"),
            system.authority.certificate_pem(),
        )
        .unwrap();
        let launch = ManagedInputs {
            roles: vec![Role::Agent, Role::Provider],
            unit: "unit-a".into(),
            gateway_url: "wss://gateway.orch.internal:9443".into(),
            ca_file: files.path().join("ca.pem"),
            enroll_file: files.path().join("token"),
            data_dir: dir.path().to_owned(),
            tools_dir: dir.path().join("tools"),
            workspace_dir: None,
            home_dir: None,
            mounts_file: None,
        };
        let platform = Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        };
        let data = DataDir::open(dir.path()).unwrap();
        let state_of = |name: &str| dir.path().join(name);

        // The first unit enrolls with its token on a volume that has nothing yet.
        std::fs::write(files.path().join("token"), format!("{}\n", system.token())).unwrap();
        assert_eq!(data.claim_machine_state("unit-a").unwrap(), Claim::Own);
        let inputs = launch
            .enrollment("orch-provider-a", platform, "0.1.0")
            .unwrap();
        enroll(&inputs, &data, &system, &clock).await.unwrap();
        let first = data.record().unwrap().machine_id;
        std::fs::write(state_of("harness").join("login.json"), "signed in").unwrap();
        std::fs::write(state_of("secrets").join("provider-a"), "sealed by a").unwrap();
        std::fs::write(state_of("spool").join("segment"), "events of a").unwrap();
        std::fs::write(state_of("state").join("hosted.json"), "{}").unwrap();

        // The same unit started again keeps its machine, with its token or with it emptied by its stop.
        assert_eq!(data.claim_machine_state("unit-a").unwrap(), Claim::Own);
        std::fs::write(files.path().join("token"), "").unwrap();
        assert_eq!(data.claim_machine_state("unit-a").unwrap(), Claim::Own);
        assert!(data.is_enrolled());

        // A new unit on the same volume has a token of its own: the machine of the first is put aside.
        std::fs::write(files.path().join("token"), format!("{}\n", system.token())).unwrap();
        assert_eq!(
            data.claim_machine_state("unit-b").unwrap(),
            Claim::Fresh {
                put_aside: "unit-a".into(),
                dropped: Vec::new()
            }
        );
        assert!(!data.is_enrolled());
        assert_eq!(
            std::fs::read_to_string(state_of("harness").join("login.json")).unwrap(),
            "signed in",
            "what a person signed in stays on the volume"
        );
        for put_aside in ["identity", "secrets", "spool", "state"] {
            assert_eq!(
                std::fs::read_dir(state_of(put_aside)).unwrap().count(),
                0,
                "{put_aside} of the first unit is not the second's"
            );
        }
        let inputs = launch
            .enrollment("orch-provider-b", platform, "0.1.0")
            .unwrap();
        enroll(&inputs, &data, &system, &clock).await.unwrap();
        let second = data.record().unwrap().machine_id;
        assert_ne!(first, second, "the new unit is a new machine");
        assert_eq!(
            system.sent.lock().unwrap().last().unwrap().machine_id,
            None,
            "and it asks to be enrolled as one, not to replace the first"
        );

        // The second unit fails and the first is given the volume back: it finds its own machine again.
        std::fs::write(files.path().join("token"), "").unwrap();
        assert_eq!(
            data.claim_machine_state("unit-a").unwrap(),
            Claim::Restored {
                put_aside: Some("unit-b".into()),
                dropped: Vec::new()
            }
        );
        assert_eq!(data.record().unwrap().machine_id, first);
        assert_eq!(
            std::fs::read_to_string(state_of("secrets").join("provider-a")).unwrap(),
            "sealed by a"
        );
        assert_eq!(
            std::fs::read_to_string(state_of("harness").join("login.json")).unwrap(),
            "signed in"
        );

        // And the second finds its own when it is given the volume once more.
        assert_eq!(
            data.claim_machine_state("unit-b").unwrap(),
            Claim::Restored {
                put_aside: Some("unit-a".into()),
                dropped: Vec::new()
            }
        );
        assert_eq!(data.record().unwrap().machine_id, second);
    }

    /// Whether any file below `root` holds exactly `text`: where a machine put aside is kept does not matter to
    /// what a person is promised, which is that it is not lost.
    fn somewhere_below(root: &Path, text: &str) -> bool {
        std::fs::read_dir(root).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    somewhere_below(&path, text)
                } else {
                    std::fs::read_to_string(&path).is_ok_and(|found| found == text)
                }
            })
        })
    }

    #[test]
    fn a_unit_that_never_enrolled_does_not_displace_the_machine_put_aside_for_an_earlier_unit() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        let keep = |name: &str, file: &str, text: &str| {
            std::fs::write(dir.path().join(name).join(file), text).unwrap();
        };
        let read = |name: &str, file: &str| {
            std::fs::read_to_string(dir.path().join(name).join(file)).unwrap_or_default()
        };
        assert_eq!(data.claim_machine_state("unit-a").unwrap(), Claim::Own);
        keep("identity", "key", "the key of a");
        keep("secrets", "sealed", "what a sealed");
        // The second unit takes the volume and never enrolls; a third takes it after.
        assert_eq!(
            data.claim_machine_state("unit-b").unwrap(),
            Claim::Fresh {
                put_aside: "unit-a".into(),
                dropped: Vec::new()
            }
        );
        assert_eq!(
            data.claim_machine_state("unit-c").unwrap(),
            Claim::Own,
            "there was no machine of the second unit to put aside"
        );
        assert!(
            somewhere_below(dir.path(), "the key of a"),
            "the machine of the first unit is still on the volume"
        );
        // The first unit is given the volume back, and finds what is its own.
        assert_eq!(
            data.claim_machine_state("unit-a").unwrap(),
            Claim::Restored {
                put_aside: None,
                dropped: Vec::new()
            }
        );
        assert_eq!(read("identity", "key"), "the key of a");
        assert_eq!(read("secrets", "sealed"), "what a sealed");
        assert!(
            !dir.path().join(PREVIOUS_DIR).exists(),
            "nothing is kept aside once it is back"
        );
    }

    #[test]
    fn a_machine_that_a_third_unit_has_to_make_room_for_is_dropped_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        let keep = |name: &str, file: &str, text: &str| {
            std::fs::write(dir.path().join(name).join(file), text).unwrap();
        };
        assert_eq!(data.claim_machine_state("unit-a").unwrap(), Claim::Own);
        keep("identity", "key", "the key of a");
        // The second unit enrolled too, and the first was put aside for it.
        data.claim_machine_state("unit-b").unwrap();
        keep("identity", "key", "the key of b");
        // A third finds two machines and room for one: the later one is kept, and the earlier is named.
        assert_eq!(
            data.claim_machine_state("unit-c").unwrap(),
            Claim::Fresh {
                put_aside: "unit-b".into(),
                dropped: vec!["unit-a".into()]
            }
        );
        assert!(somewhere_below(dir.path(), "the key of b"));
        assert!(!somewhere_below(dir.path(), "the key of a"));
    }

    #[test]
    fn a_change_of_machine_that_was_cut_short_is_finished_by_the_next_start_and_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        assert_eq!(data.claim_machine_state("unit-a").unwrap(), Claim::Own);
        std::fs::write(dir.path().join("secrets").join("sealed"), "what a sealed").unwrap();
        // The process died after it had moved `identity/` aside and before it moved the rest: the
        // next start finds the empty place `DataDir::open` makes, and the marker where it was.
        std::fs::remove_dir(dir.path().join("identity")).unwrap();
        let aside = dir.path().join(PREVIOUS_DIR).join("unit-a");
        std::fs::create_dir_all(aside.join("identity")).unwrap();
        std::fs::write(aside.join("identity").join("key"), "the key of a").unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        assert_eq!(
            data.claim_machine_state("unit-c").unwrap(),
            Claim::Fresh {
                put_aside: "unit-a".into(),
                dropped: Vec::new()
            }
        );
        for text in ["the key of a", "what a sealed"] {
            assert!(
                somewhere_below(dir.path(), text),
                "{text:?} was moved or left, and is not lost"
            );
        }
        // And the first unit, given the volume back, has all of its machine again.
        assert_eq!(
            data.claim_machine_state("unit-a").unwrap(),
            Claim::Restored {
                put_aside: None,
                dropped: Vec::new()
            }
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("identity").join("key")).unwrap(),
            "the key of a"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("secrets").join("sealed")).unwrap(),
            "what a sealed"
        );
    }

    #[test]
    fn a_machine_nobody_marked_is_another_units_and_is_put_aside_not_taken_for_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("identity").join("key"),
            "a key nobody marked",
        )
        .unwrap();
        assert_eq!(
            data.claim_machine_state("unit-a").unwrap(),
            Claim::Fresh {
                put_aside: "unmarked".into(),
                dropped: Vec::new()
            }
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join("identity"))
                .unwrap()
                .count(),
            0,
            "the unit begins with no machine"
        );
        assert!(somewhere_below(dir.path(), "a key nobody marked"));
    }

    #[test]
    fn a_unit_named_so_that_it_would_leave_the_data_directory_claims_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        for name in ["", "../other", "a/b", "unit a"] {
            assert!(
                matches!(data.claim_machine_state(name), Err(EnrollError::Launch(_))),
                "{name:?} is not a unit identifier"
            );
        }
        assert!(!dir.path().join(UNIT_FILE).exists());
    }

    #[test]
    fn the_default_data_directory_is_the_platforms_place_for_one_users_data() {
        let var = |name: &str| match name {
            "HOME" => Some("/home/alice".to_owned()),
            "XDG_DATA_HOME" => Some("/home/alice/.data".to_owned()),
            _ => None,
        };
        if cfg!(target_os = "linux") {
            assert_eq!(
                default_data_dir(var),
                PathBuf::from("/home/alice/.data/orch-connector")
            );
            assert_eq!(
                default_data_dir(|name| (name == "HOME").then(|| "/home/alice".to_owned())),
                PathBuf::from("/home/alice/.local/share/orch-connector")
            );
        }
    }
}
