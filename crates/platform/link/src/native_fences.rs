//! Ordered native operands survive an empty physical slot without scanning source or historical data.
use orch_link_contract::native_target::{SessionAction, SessionTarget};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

#[derive(Debug)]
pub struct NativeFences {
    directory: PathBuf,
    changes: Mutex<()>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    target: SessionTarget,
    closed: bool,
}
impl NativeFences {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            changes: Mutex::new(()),
        }
    }
    fn path(&self, target: &SessionTarget) -> PathBuf {
        self.directory
            .join(format!("session-{}.json", target.thread_id))
    }
    fn read(&self, path: &Path) -> std::io::Result<Option<Marker>> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(std::io::Error::other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn write(&self, path: &Path, marker: &Marker) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        let bytes = serde_json::to_vec(marker).map_err(std::io::Error::other)?;
        let mut staged = tempfile::NamedTempFile::new_in(&self.directory)?;
        staged.write_all(&bytes)?;
        staged.as_file().sync_all()?;
        staged.persist(path).map_err(|error| error.error)?;
        #[cfg(unix)]
        std::fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
    /// This is an ordering fact, not a grant. Roles still match the original physical operand.
    pub fn observe_session(
        &self,
        target: &SessionTarget,
        action: SessionAction,
    ) -> std::io::Result<bool> {
        if !target.coherent() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the native target is incoherent",
            ));
        }
        let Some(epoch) = target.opening_epoch else {
            return Ok(action == SessionAction::Close);
        };
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        let path = self.path(target);
        let before = self.read(&path)?;
        if let Some(before) = &before {
            let previous = before.target.opening_epoch.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the saved native target has no order",
                )
            })?;
            if previous > epoch {
                return Ok(false);
            }
            if previous == epoch
                && (before.target.opening_id != target.opening_id
                    || before.target.segment_id != target.segment_id)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "one native opening order names different physical targets",
                ));
            }
            if previous == epoch && action != SessionAction::Close {
                return Ok(!before.closed);
            }
        }
        match action {
            SessionAction::Open | SessionAction::Close => {
                self.write(
                    &path,
                    &Marker {
                        target: target.clone(),
                        closed: action == SessionAction::Close,
                    },
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
