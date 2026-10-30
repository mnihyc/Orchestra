//! Git subscriptions share the workspace's native file notices and retain only their last status.

use super::{Environment, HostError, Id, Mutex, RelPath, Result, lock};
use orch_api_contract::workspace::GitStatus;
use std::collections::HashMap;

/// The directory-watch bound also bounds Git processes per native change batch.
const MAX_WATCHES: usize = super::DIR_WATCHES_MAX;

struct Watch {
    path: Option<RelPath>,
    last: Option<GitStatus>,
    gate: std::sync::Arc<tokio::sync::Mutex<()>>,
}

/// Stable watch identifiers survive a repeated subscription without multiplying native watchers.
#[derive(Default)]
pub(super) struct Watches(Mutex<HashMap<Id, Watch>>);

impl Watches {
    pub(super) fn is_empty(&self) -> bool {
        lock(&self.0).is_empty()
    }

    /// Reattaching the same environment keeps its subscribers and re-observes their new root.
    pub(super) fn take(&self) -> Self {
        let mut watches = std::mem::take(&mut *lock(&self.0));
        for watch in watches.values_mut() {
            watch.last = None;
        }
        Self(Mutex::new(watches))
    }

    pub(super) fn register(&self, id: Id, path: Option<RelPath>) -> Result<()> {
        let mut watches = lock(&self.0);
        if watches.len() >= MAX_WATCHES && !watches.contains_key(&id) {
            return Err(HostError::Busy(format!(
                "at most {MAX_WATCHES} Git directories are watched at once"
            )));
        }
        if watches.get(&id).is_none_or(|watch| watch.path != path) {
            watches.insert(
                id,
                Watch {
                    path,
                    last: None,
                    gate: std::sync::Arc::default(),
                },
            );
        }
        Ok(())
    }

    pub(super) fn remove(&self, id: Id) -> bool {
        lock(&self.0).remove(&id).is_some()
    }

    pub(super) async fn read(&self, environment: &Environment, id: Id) -> Result<GitStatus> {
        let (path, gate) = lock(&self.0)
            .get(&id)
            .map(|watch| (watch.path.clone(), std::sync::Arc::clone(&watch.gate)))
            .ok_or_else(|| HostError::NotFound("the Git watch has ended".into()))?;
        let _reading = gate.lock().await;
        let status = environment
            .files
            .git_status_at(&environment.git, path.as_ref())
            .await?;
        self.seen(environment, id, &path, &status).await;
        Ok(status)
    }

    async fn seen(
        &self,
        environment: &Environment,
        id: Id,
        path: &Option<RelPath>,
        status: &GitStatus,
    ) {
        let changed = {
            let mut watches = lock(&self.0);
            let Some(watch) = watches.get_mut(&id).filter(|watch| watch.path == *path) else {
                return;
            };
            if watch.last.as_ref() == Some(status) {
                false
            } else {
                watch.last = Some(status.clone());
                true
            }
        };
        if changed && let Ok(host) = environment.inner() {
            host.reports
                .say(
                    environment.id,
                    super::Report::GitChanged(orch_link_contract::events::GitChanged {
                        watch_id: id,
                        status: status.clone(),
                    }),
                )
                .await;
        }
    }

    pub(super) async fn changed(&self, environment: &Environment, paths: &[RelPath], rescan: bool) {
        let watches: Vec<_> = lock(&self.0)
            .iter()
            .filter(|(_, watch)| {
                let repository = watch
                    .last
                    .as_ref()
                    .and_then(|last| last.repository_dir.as_deref())
                    .and_then(|path| environment.files.parse(path).ok())
                    .unwrap_or_else(RelPath::root);
                rescan
                    || paths.iter().any(|path| {
                        path.starts_with(&repository)
                            || repository.starts_with(path)
                            || path.components().any(|part| part == ".git")
                    })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in watches {
            let _ = self.read(environment, id).await;
        }
    }
}
