//! Private virtual-screen records recover only verified owned process identities.

use crate::browsers::BrowserError;
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::PathBuf;

/// A private metadata file shares the existing one-mebibyte streaming budget, never storing pictures.
const STORE_MAX: u64 = crate::blobs::READ_CHUNK_BYTES as u64;

/// A PID is usable only with its boot, user and kernel start identity.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pid: u32,
    boot: String,
    start: u64,
    uid: u32,
    #[serde(default)]
    namespace: String,
}

impl Identity {
    /// Captures a child that was just started, without deriving an identity from a stored PID alone.
    pub fn capture(pid: u32) -> Option<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            let directory = format!("/proc/{pid}");
            let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
            let stat = std::fs::read_to_string(format!("{directory}/stat")).ok()?;
            let (_, tail) = stat.rsplit_once(')')?;
            if tail
                .split_whitespace()
                .next()
                .is_some_and(|state| matches!(state, "Z" | "X"))
            {
                return None;
            }
            let start = tail.split_whitespace().nth(19)?.parse().ok()?;
            let uid = std::fs::metadata(directory).ok()?.uid();
            let namespace = std::fs::read_link(format!("/proc/{pid}/ns/pid"))
                .ok()?
                .to_string_lossy()
                .into_owned();
            Some(Self {
                pid,
                boot,
                start,
                uid,
                namespace,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            None
        }
    }
    /// A changed boot, reused PID or changed user is never signalled.
    pub fn live(&self) -> bool {
        Self::capture(self.pid).is_some_and(|now| {
            now.boot == self.boot
                && now.start == self.start
                && now.uid == self.uid
                && now.namespace == self.namespace
        })
    }
    /// Stops only this verified identity, never a stored process group.
    pub fn stop(&self) {
        #[cfg(unix)]
        if self.live()
            && let Ok(pid) = i32::try_from(self.pid)
            && let Some(pid) = rustix::process::Pid::from_raw(pid)
        {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
    }
}

/// Nonsecret metadata belongs to the display's UUID, not to a viewer or an arbitrary program.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    /// Its known display.
    pub id: Id,
    /// Its literal loopback endpoint.
    pub endpoint: String,
    /// The allocated X display.
    pub x_display: String,
    /// The exact program selected by discovery.
    pub program: PathBuf,
    /// Its kernel process identity, once observed after spawn.
    pub server: Option<Identity>,
    /// Its window manager, when installed.
    pub manager: Option<Identity>,
    /// A close intent survives a crash before the signal reaches the original process.
    #[serde(default)]
    pub closing: bool,
}

impl Record {
    /// The UUID label is also written to argv, repairing a crash immediately after spawn.
    pub fn label(&self) -> String {
        format!("Orch-display-{}", self.id)
    }

    /// Reconciles the reserved launch label only on this user's exact executable and X display.
    pub fn recover(&mut self) {
        if let Some(server) = &self.server {
            if !server.live() {
                self.server = None;
            }
            return;
        }
        // Commands serialize per environment: at most the in-flight spawn lacks its saved PID.
        // Known ended identities are never replaced by a lookalike process, even with the same argv.
        #[cfg(target_os = "linux")]
        {
            let Ok(entries) = std::fs::read_dir("/proc") else {
                return;
            };
            let label = self.label();
            for entry in entries.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };
                let Some(identity) = Identity::capture(pid) else {
                    continue;
                };
                if identity.uid != rustix::process::geteuid().as_raw() {
                    continue;
                }
                if std::fs::read_link(entry.path().join("exe")).ok().as_deref()
                    != Some(self.program.as_path())
                {
                    continue;
                }
                let Ok(command) = std::fs::read(entry.path().join("cmdline")) else {
                    continue;
                };
                let args: Vec<_> = command.split(|b| *b == 0).collect();
                if args
                    .windows(2)
                    .any(|pair| pair[0] == b"-desktop" && pair[1] == label.as_bytes())
                    && args.contains(&self.x_display.as_bytes())
                {
                    self.server = Some(identity);
                    break;
                }
            }
        }
    }
    /// Whether this recovered screen still has its original server.
    pub fn live(&self) -> bool {
        self.server.as_ref().is_some_and(Identity::live)
    }
    /// Stops only the saved verified children.
    pub fn stop(&self) {
        if let Some(manager) = &self.manager {
            manager.stop();
        }
        if let Some(server) = &self.server {
            server.stop();
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    records: Vec<Record>,
}

/// Atomic private metadata outside the workspace and its snapshots.
pub struct Store {
    directory: PathBuf,
}
impl Store {
    /// Prepares a private state directory for one environment.
    pub fn new(directory: PathBuf) -> Result<Self, BrowserError> {
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { directory })
    }
    /// Loads bounded metadata; a corrupt journal is never treated as empty.
    pub fn load(&self) -> Result<Vec<Record>, BrowserError> {
        let file = match std::fs::File::open(self.directory.join("windows.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(STORE_MAX + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > STORE_MAX {
            return Err(BrowserError::Failed(
                "the private display journal is too large".to_owned(),
            ));
        }
        let journal: Journal = serde_json::from_slice(&bytes).map_err(|_| {
            BrowserError::Failed("the private display journal is unreadable".to_owned())
        })?;
        if journal.version != 1 {
            return Err(BrowserError::Failed(
                "the private display journal version is unavailable".to_owned(),
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for record in &journal.records {
            if !ids.insert(record.id) {
                return Err(BrowserError::Failed(
                    "the private display journal repeats an identifier".to_owned(),
                ));
            }
            if !record.endpoint.starts_with("127.0.0.1:")
                || record
                    .x_display
                    .strip_prefix(':')
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_none()
                || !record.program.is_absolute()
            {
                return Err(BrowserError::Failed(
                    "the private display journal is invalid".to_owned(),
                ));
            }
        }
        Ok(journal.records)
    }
    /// Saves a complete record list before acknowledging a display change.
    pub fn save(&self, records: Vec<Record>) -> Result<(), BrowserError> {
        let bytes = serde_json::to_vec(&Journal {
            version: 1,
            records,
        })
        .map_err(|_| {
            BrowserError::Failed("the private display journal could not be written".to_owned())
        })?;
        if bytes.len() as u64 > STORE_MAX {
            return Err(BrowserError::Failed(
                "the private display journal is too large".to_owned(),
            ));
        }
        let temporary = self
            .directory
            .join(format!(".windows-{}.tmp", orch_types::new_id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| -> std::io::Result<()> {
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, self.directory.join("windows.json"))?;
            std::fs::File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(BrowserError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_corrupt_journal_does_not_start_another_screen() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().join("screens")).unwrap();
        std::fs::write(store.directory.join("windows.json"), b"broken").unwrap();
        assert!(store.load().is_err());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn a_changed_start_or_boot_cannot_be_used_as_a_process_identity() {
        let mut identity = Identity::capture(std::process::id()).unwrap();
        assert!(identity.live());
        identity.start += 1;
        assert!(!identity.live());
        let mut identity = Identity::capture(std::process::id()).unwrap();
        identity.boot = "another-boot".to_owned();
        assert!(!identity.live());
    }
}
