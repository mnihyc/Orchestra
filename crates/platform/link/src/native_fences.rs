//! Ordered native operands survive an empty physical slot without scanning source or historical data.
use orch_link_contract::native_target::{SessionAction, SessionTarget};
use orch_link_contract::tool_sessions::ToolSessionUpdate;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

#[derive(Debug)]
pub struct NativeFences {
    directory: PathBuf,
    changes: Mutex<()>,
    control: Mutex<ToolControl>,
    control_changed: tokio::sync::Notify,
}
#[derive(Debug)]
struct ToolControl {
    round: orch_types::Id,
    available: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    target: SessionTarget,
    closed: bool,
    tools: Option<ToolSessionUpdate>,
    tools_round: Option<orch_types::Id>,
}
impl NativeFences {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            changes: Mutex::new(()),
            control: Mutex::new(ToolControl {
                round: orch_types::new_id(),
                available: true,
            }),
            control_changed: tokio::sync::Notify::new(),
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
                        tools: None,
                        tools_round: None,
                    },
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Retains committed withdrawals even when their native command has not arrived yet.
    pub fn observe_tools(&self, update: &ToolSessionUpdate) -> std::io::Result<bool> {
        let target = SessionTarget {
            thread_id: update.thread_id,
            segment_id: Some(update.segment_id),
            opening_id: update.opening_id,
            opening_epoch: Some(update.opening_epoch),
        };
        if !target.coherent() || update.revision <= 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the tool withdrawal has no physical order",
            ));
        }
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        let round = self.tool_control().0;
        let path = self.path(&target);
        let before = self.read(&path)?;
        if let Some(before) = &before {
            let previous = before.target.opening_epoch.unwrap_or_default();
            if previous > update.opening_epoch
                || previous == update.opening_epoch
                    && before
                        .tools
                        .as_ref()
                        .is_some_and(|old| old.revision > update.revision)
            {
                return Ok(false);
            }
            if previous == update.opening_epoch && before.target != target {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "one opening order names different physical sessions",
                ));
            }
        }
        let closed = !update.live
            || before
                .as_ref()
                .is_some_and(|old| old.target == target && old.closed);
        let mut snapshot = update.clone();
        snapshot.live = !closed;
        if let Some(previous) = before.as_ref().and_then(|marker| marker.tools.as_ref())
            && previous.opening_epoch == snapshot.opening_epoch
            && previous.revision == snapshot.revision
        {
            snapshot
                .members
                .retain(|member| previous.members.contains(member));
        }
        if before.as_ref().is_some_and(|marker| {
            marker.tools_round == Some(round) && marker.tools.as_ref() == Some(&snapshot)
        }) {
            return Ok(true);
        }
        self.write(
            &path,
            &Marker {
                target,
                closed,
                tools: Some(snapshot),
                tools_round: Some(round),
            },
        )?;
        Ok(true)
    }

    /// Latest factual withdrawal for this thread; never an authority to install native routes.
    pub fn tools(&self, thread: orch_types::Id) -> std::io::Result<Option<ToolSessionUpdate>> {
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        self.read(&self.path(&SessionTarget::absent(thread)))
            .map(|marker| marker.and_then(|marker| marker.tools))
    }

    pub fn set_tool_control(&self, available: bool) {
        let mut control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        if !available && control.available {
            control.round = orch_types::new_id();
        }
        control.available = available;
        self.control_changed.notify_waiters();
    }

    #[must_use]
    pub fn tool_control(&self) -> (orch_types::Id, bool) {
        let control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        (control.round, control.available)
    }

    pub fn current_tools(
        &self,
        thread: orch_types::Id,
    ) -> std::io::Result<Option<(ToolSessionUpdate, orch_types::Id)>> {
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        let marker = self.read(&self.path(&SessionTarget::absent(thread)))?;
        let control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        if !control.available {
            return Ok(None);
        }
        Ok(marker
            .filter(|marker| marker.tools_round == Some(control.round))
            .and_then(|marker| marker.tools)
            .map(|tools| (tools, control.round)))
    }

    pub async fn wait_tool_control(&self, cancel: &tokio_util::sync::CancellationToken) -> bool {
        loop {
            let changed = self.control_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.tool_control().1 {
                return true;
            }
            tokio::select! {()=cancel.cancelled()=>return false,()=changed=>{}}
        }
    }
}
