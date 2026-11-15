//! Known private-upload roots survive environment detach and connector restart.
//! Only the sealed root index and each root's private upload metadata directory are inspected.

use crate::{
    environment::{GateState, Gates, Limits},
    error::{HostError, Result, blocking},
    files::Files,
    ignore::IgnoreRules,
    index::HashCache,
    paths::Workspace,
};
use orch_link_contract::events::PathStyle;
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
enum NativePath {
    #[cfg(unix)]
    Unix(Vec<u8>),
    #[cfg(windows)]
    Windows(Vec<u16>),
}
impl NativePath {
    fn capture(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self::Unix(path.as_os_str().as_bytes().to_vec())
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Self::Windows(path.as_os_str().encode_wide().collect())
        }
    }
    fn path(&self) -> PathBuf {
        match self {
            #[cfg(unix)]
            Self::Unix(bytes) => {
                use std::os::unix::ffi::OsStringExt;
                std::ffi::OsString::from_vec(bytes.clone()).into()
            }
            #[cfg(windows)]
            Self::Windows(wide) => {
                use std::os::windows::ffi::OsStringExt;
                std::ffi::OsString::from_wide(wide).into()
            }
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    root: NativePath,
    style: PathStyle,
    case_sensitive: bool,
}
struct Group {
    record: Record,
    instances: Vec<Files>,
}
#[derive(Default)]
struct State {
    loaded: bool,
    dirty: bool,
    groups: Vec<Group>,
}

pub(crate) struct Roots {
    file: PathBuf,
    clock: Arc<dyn Clock>,
    limits: Limits,
    state: Mutex<State>,
}
impl Roots {
    pub(crate) fn new(directory: &Path, clock: Arc<dyn Clock>, limits: Limits) -> Arc<Self> {
        let roots = Arc::new(Self {
            file: directory.join("file-relay-roots.json"),
            clock,
            limits,
            state: Mutex::new(State::default()),
        });
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(&roots);
            runtime.spawn(async move {
                loop {
                    let Some(roots) = weak.upgrade() else {
                        return;
                    };
                    if let Err(error) = roots.prune().await {
                        tracing::warn!(%error,"native upload root cleanup will be retried");
                    }
                    let wait = roots.limits.manifest_interval;
                    drop(roots);
                    tokio::time::sleep(wait).await;
                }
            });
        }
        roots
    }
    async fn load(&self, state: &mut State) -> Result<()> {
        if state.loaded {
            return Ok(());
        }
        let path = self.file.clone();
        let records: Vec<Record> = blocking(move || match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                HostError::Internal(format!(
                    "the native upload root index cannot be read: {error}"
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(HostError::io("reading native upload roots", error)),
        })
        .await?;
        state.groups = records
            .into_iter()
            .map(|record| Group {
                record,
                instances: Vec::new(),
            })
            .collect();
        state.loaded = true;
        Ok(())
    }
    async fn save(&self, state: &State) -> Result<()> {
        let records = state
            .groups
            .iter()
            .map(|group| group.record.clone())
            .collect::<Vec<_>>();
        let bytes =
            serde_json::to_vec(&records).map_err(|error| HostError::Internal(error.to_string()))?;
        let path = self.file.clone();
        blocking(move || {
            let parent = path.parent().ok_or_else(|| {
                HostError::Internal("the native root index has no directory".into())
            })?;
            std::fs::create_dir_all(parent).map_err(|error| {
                HostError::io("making the native upload index directory", error)
            })?;
            let staged = parent.join(format!("file-relay-roots-{}.tmp", orch_types::new_id()));
            let write = || -> Result<()> {
                use std::io::Write;
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options
                    .open(&staged)
                    .map_err(|error| HostError::io("staging native upload roots", error))?;
                file.write_all(&bytes)
                    .map_err(|error| HostError::io("writing native upload roots", error))?;
                file.sync_all()
                    .map_err(|error| HostError::io("flushing native upload roots", error))?;
                std::fs::rename(&staged, &path)
                    .map_err(|error| HostError::io("publishing native upload roots", error))?;
                crate::platform::Dir::open_path(parent)
                    .and_then(|dir| dir.sync())
                    .map_err(|error| {
                        HostError::io("flushing the native root index directory", error)
                    })?;
                Ok(())
            };
            let result = write();
            if result.is_err() {
                let _ = std::fs::remove_file(&staged);
            }
            result
        })
        .await
    }
    pub(crate) async fn remember(&self, files: &Files) -> Result<()> {
        let record = Record {
            root: NativePath::capture(files.root().root_path()),
            style: files.root().style(),
            case_sensitive: files.root().case_sensitive(),
        };
        let mut state = self.state.lock().await;
        self.load(&mut state).await?;
        if let Some(group) = state.groups.iter_mut().find(|group| group.record == record) {
            if !group
                .instances
                .iter()
                .any(|known| known.same_native_source(files))
            {
                group.instances.push(files.clone());
            }
            if state.dirty {
                self.save(&state).await?;
                state.dirty = false;
            }
            return Ok(());
        }
        state.groups.push(Group {
            record,
            instances: vec![files.clone()],
        });
        state.dirty = true;
        self.save(&state).await?;
        state.dirty = false;
        Ok(())
    }
    async fn reopen(&self, group: &mut Group) -> Result<()> {
        if !group.instances.is_empty() {
            return Ok(());
        }
        let record = group.record.clone();
        let limits = self.limits.clone();
        let files = blocking(move || {
            let root = Workspace::open(
                &record.root.path(),
                &[],
                record.style,
                record.case_sensitive,
            )?;
            Ok(Files::workspace(
                root,
                Arc::new(IgnoreRules::new(&[], record.case_sensitive)),
                Arc::new(HashCache::new()),
                limits,
                Arc::new(GateState::new(Gates::OPEN)),
            ))
        })
        .await?;
        files.start_relay_cleanup(Arc::clone(&self.clock));
        group.instances.push(files);
        Ok(())
    }
    pub(crate) async fn find(&self, session: Id) -> Result<Option<Files>> {
        let mut state = self.state.lock().await;
        self.load(&mut state).await?;
        for group in &mut state.groups {
            if let Err(error) = self.reopen(group).await {
                tracing::warn!(%error,"an upload root remains unavailable for cleanup");
                continue;
            }
            for files in &group.instances {
                if files.has_file_relay(session).await? {
                    return Ok(Some(files.clone()));
                }
            }
        }
        Ok(None)
    }
    pub(crate) async fn prune(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        self.load(&mut state).await?;
        let mut kept = Vec::new();
        let mut changed = false;
        for mut group in state.groups.drain(..) {
            if let Err(error) = self.reopen(&mut group).await {
                tracing::warn!(%error,"an upload root remains unavailable for expiry");
                kept.push(group);
                continue;
            }
            let mut instances = Vec::new();
            for files in group.instances.drain(..) {
                if let Err(error) = files.prune_file_relays(Time::new(self.clock.now())).await {
                    tracing::warn!(%error,"private upload expiry will be retried");
                    instances.push(files);
                    continue;
                }
                let pending = match files.has_file_relays().await {
                    Ok(pending) => pending,
                    Err(error) => {
                        tracing::warn!(%error,"an upload root remains held after an index read failed");
                        true
                    }
                };
                if files.native_source_in_use() || pending {
                    instances.push(files);
                }
            }
            group.instances = instances;
            if group.instances.is_empty() {
                changed = true;
            } else {
                kept.push(group);
            }
        }
        state.groups = kept;
        state.dirty |= changed;
        if state.dirty {
            self.save(&state).await?;
            state.dirty = false;
        }
        Ok(())
    }
}
