//! Ordered native operands survive an empty physical slot without scanning source or historical data.
use orch_link_contract::native_target::{
    LifecycleAction, LifecycleSubject, LifecycleTarget, SessionAction, SessionTarget,
};
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LifecycleMarker {
    target: LifecycleTarget,
    action: LifecycleAction,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpeningEffect {
    target: LifecycleTarget,
    action: LifecycleAction,
    settled: bool,
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
        self.read_typed(path)
    }
    fn read_typed<T: serde::de::DeserializeOwned>(
        &self,
        path: &Path,
    ) -> std::io::Result<Option<T>> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(std::io::Error::other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn write<T: Serialize>(&self, path: &Path, marker: &T) -> std::io::Result<()> {
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

    fn lifecycle_path(&self, subject: &LifecycleSubject) -> PathBuf {
        self.directory
            .join(format!("lifecycle-{}.json", subject.storage_key()))
    }

    fn opening_path(&self, subject: &LifecycleSubject) -> PathBuf {
        self.directory
            .join(format!("opening-{}.json", subject.storage_key()))
    }

    fn read_opening(&self, subject: &LifecycleSubject) -> std::io::Result<Option<OpeningEffect>> {
        let effect = self.read_typed::<OpeningEffect>(&self.opening_path(subject))?;
        if effect.as_ref().is_some_and(|effect| {
            effect.target.subject != *subject
                || !matches!(
                    effect.action,
                    LifecycleAction::Create | LifecycleAction::Start
                )
                || !effect.target.coherent_for(effect.action)
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the saved native opening effect is incoherent",
            ));
        }
        Ok(effect)
    }

    /// Persist before the original first effect, under the subject's native effect lock.
    /// An unresolved old effect cannot be replaced by a new proposed opening.
    pub fn begin_opening(
        &self,
        target: &LifecycleTarget,
        action: LifecycleAction,
    ) -> std::io::Result<bool> {
        if !matches!(action, LifecycleAction::Create | LifecycleAction::Start)
            || !target.coherent_for(action)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the original native opening is incoherent",
            ));
        }
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        let current = self.read_typed::<LifecycleMarker>(&self.lifecycle_path(&target.subject))?;
        if !current.is_some_and(|current| current.target == *target && current.action == action) {
            return Ok(false);
        }
        if self
            .read_opening(&target.subject)?
            .is_some_and(|effect| !effect.settled)
        {
            return Ok(false);
        }
        self.write(
            &self.opening_path(&target.subject),
            &OpeningEffect {
                target: target.clone(),
                action,
                settled: false,
            },
        )?;
        Ok(true)
    }

    /// Actual first-effect evidence recorded by this original, not a matching source name alone.
    pub fn recorded_opening(
        &self,
        target: &LifecycleTarget,
        action: LifecycleAction,
    ) -> std::io::Result<bool> {
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(self
            .read_opening(&target.subject)?
            .is_some_and(|effect| effect.target == *target && effect.action == action))
    }

    /// Record only this original's definite native completion or definite pre-effect refusal.
    /// It is history even after a newer control arrived, never a permission to continue.
    pub fn settle_opening(
        &self,
        target: &LifecycleTarget,
        action: LifecycleAction,
    ) -> std::io::Result<bool> {
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(mut effect) = self.read_opening(&target.subject)? else {
            return Ok(false);
        };
        if effect.target != *target || effect.action != action {
            return Ok(false);
        }
        if !effect.settled {
            effect.settled = true;
            self.write(&self.opening_path(&target.subject), &effect)?;
        }
        Ok(true)
    }

    /// A new control and an empty listing do not settle an earlier daemon effect.
    /// Only this subject's original source/openings are considered; other subjects stay independent.
    pub fn unsettled_opening(&self, target: &LifecycleTarget) -> std::io::Result<bool> {
        let sources: Vec<_> = target
            .observed
            .into_iter()
            .chain(target.pending.iter().copied())
            .collect();
        self.unsettled_sources(&target.subject, &sources)
    }

    /// A native caller's actual retained source, without inventing a control revision.
    pub fn unsettled_source(
        &self,
        subject: &LifecycleSubject,
        source: Option<orch_types::Id>,
    ) -> std::io::Result<bool> {
        self.unsettled_sources(subject, &source.into_iter().collect::<Vec<_>>())
    }

    fn unsettled_sources(
        &self,
        subject: &LifecycleSubject,
        sources: &[orch_types::Id],
    ) -> std::io::Result<bool> {
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(effect) = self.read_opening(subject)? else {
            return Ok(false);
        };
        if effect.settled {
            return Ok(false);
        }
        Ok(sources.is_empty()
            || effect
                .target
                .observed
                .into_iter()
                .chain(effect.target.opening)
                .chain(effect.target.pending)
                .any(|source| sources.contains(&source)))
    }

    /// Retains accepted control order only. Its owner still checks current original authority
    /// and the actual physical operand after taking the resource's native effect lock.
    pub fn observe_lifecycle(
        &self,
        target: &LifecycleTarget,
        action: LifecycleAction,
    ) -> std::io::Result<bool> {
        if !target.coherent_for(action) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the native lifecycle target is incoherent",
            ));
        }
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        let path = self.lifecycle_path(&target.subject);
        if let Some(before) = self.read_typed::<LifecycleMarker>(&path)? {
            if before.target.subject != target.subject || !before.target.coherent_for(before.action)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the saved native lifecycle target is incoherent",
                ));
            }
            if before.target.revision > target.revision {
                return Ok(false);
            }
            if before.target.revision == target.revision {
                if before.target != *target || before.action != action {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "one native control order names different original operands",
                    ));
                }
                return Ok(true);
            }
        }
        self.write(
            &path,
            &LifecycleMarker {
                target: target.clone(),
                action,
            },
        )?;
        Ok(true)
    }

    /// A persistent original snapshot, including explicit absence; it grants no permission.
    pub fn lifecycle_target(
        &self,
        subject: &LifecycleSubject,
    ) -> std::io::Result<Option<LifecycleTarget>> {
        let _one = self.changes.lock().unwrap_or_else(PoisonError::into_inner);
        let marker = self.read_typed::<LifecycleMarker>(&self.lifecycle_path(subject))?;
        match marker {
            Some(marker)
                if marker.target.subject != *subject
                    || !marker.target.coherent_for(marker.action) =>
            {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the saved native lifecycle target is incoherent",
                ))
            }
            marker => Ok(marker.map(|marker| marker.target)),
        }
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
