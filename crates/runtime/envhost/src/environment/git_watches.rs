//! Git subscriptions share the workspace's native file notices and retain only their last status.

use super::{Environment, HostError, Id, Mutex, RelPath, Result, lock};
use orch_api_contract::workspace::GitStatus;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The directory-watch bound also bounds Git processes per native change batch.
const MAX_WATCHES: usize = super::DIR_WATCHES_MAX;

/// How long a watch waits after reading a status before it reads another, while files keep changing:
/// each status read that differs is a report that the system writes down, so a burst of changes costs
/// a few writes and not one for each batch. A change after a quiet time is read at once, and the last
/// change of a burst is always read.
const STATUS_GAP: std::time::Duration = std::time::Duration::from_secs(2);

struct Watch {
    path: Option<RelPath>,
    last: Option<GitStatus>,
    gate: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// A change came that the last status read did not see.
    dirty: Arc<AtomicBool>,
    /// A task is reading this watch's status: further changes only mark it dirty.
    running: Arc<AtomicBool>,
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
                    dirty: Arc::default(),
                    running: Arc::default(),
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

    /// Reads the status of every watched repository that these changes touch, each in a task of its
    /// own: the watch's batches are never held up by a status of a large repository, and a status that
    /// takes longer than the changes take to come is read once more when it ends, not once per batch.
    pub(super) fn changed(&self, environment: &Arc<Environment>, paths: &[RelPath], rescan: bool) {
        let watches: Vec<(Id, Arc<AtomicBool>, Arc<AtomicBool>)> = lock(&self.0)
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
            .map(|(id, watch)| (*id, Arc::clone(&watch.dirty), Arc::clone(&watch.running)))
            .collect();
        for (id, dirty, running) in watches {
            dirty.store(true, Ordering::SeqCst);
            if running.swap(true, Ordering::SeqCst) {
                continue;
            }
            let reading = Arc::clone(environment);
            let task = tokio::spawn(async move {
                loop {
                    dirty.store(false, Ordering::SeqCst);
                    let _ = reading.git_watches.read(&reading, id).await;
                    tokio::time::sleep(STATUS_GAP).await;
                    if !dirty.load(Ordering::SeqCst) {
                        running.store(false, Ordering::SeqCst);
                        // A change that came between the last look and the end of this task.
                        if dirty.load(Ordering::SeqCst) && !running.swap(true, Ordering::SeqCst) {
                            continue;
                        }
                        break;
                    }
                }
            });
            environment.track(task);
        }
    }
}
