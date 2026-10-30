//! Shared in-flight directory capture, independent readers and joined blocking producer exit.

use super::{Binding, DirectoryPages, Observation, Snapshot, check, lock};
use crate::error::{HostError, Result};
use crate::files::{DirectoryLocation, Files};
use crate::paths::RelPath;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// Keeps a native registration owned until the actual producer and its page callers finish.
pub(crate) type Coverage = Arc<dyn Send + Sync>;

type Outcome = Arc<Result<Arc<Snapshot>>>;
type Work = Shared<BoxFuture<'static, Outcome>>;

pub(super) struct Readers {
    pub(super) count: usize,
    pub(super) closed: bool,
}
pub(super) struct State {
    pub(super) observation: Arc<Observation>,
    pub(super) readers: Mutex<Readers>,
}

pub(super) struct Building {
    pub(super) binding: Binding,
    pub(super) location: DirectoryLocation,
    pub(super) observation: Arc<Observation>,
    covered: bool,
    cancel: CancellationToken,
    state: Arc<State>,
    outcome: Work,
    driver: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    #[cfg(all(test, target_os = "linux"))]
    testing: Arc<super::observations_tests::Hooks>,
}

pub(super) struct Reader {
    build: Arc<Building>,
}

impl Drop for Reader {
    fn drop(&mut self) {
        let mut readers = lock(&self.build.state.readers);
        readers.count -= 1;
        if readers.count == 0 {
            readers.closed = true;
            // The driver still joins the real blocking worker. Dropping a reader never aborts it
            // or releases its permit, native owner or temporary files while it is running.
            self.build.cancel.cancel();
        }
    }
}

impl Building {
    pub(super) fn new(
        cache: Arc<DirectoryPages>,
        binding: Binding,
        files: Files,
        path: RelPath,
        location: DirectoryLocation,
        coverage: Option<Coverage>,
        deadline: Instant,
    ) -> (Arc<Self>, Reader) {
        let observation = Arc::new(Observation {
            location: Some(location.clone()),
            ..Observation::default()
        });
        // Reserve the first reader before its driver can run, using the same state as later
        // admission, last-reader close and final publication.
        let state = Arc::new(State {
            observation: Arc::clone(&observation),
            readers: Mutex::new(Readers {
                count: 1,
                closed: false,
            }),
        });
        let proof = Arc::clone(&state);
        let reason = Arc::clone(&observation);
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let source_binding = binding.clone();
        let covered = coverage.is_some();
        #[cfg(all(test, target_os = "linux"))]
        let testing = Arc::clone(&cache.testing);
        let outcome = async move {
            // This owner spans admission, the scan, sort and its joined exit, including cancellation.
            let _coverage = coverage;
            let result = async {
                check(&token, deadline)?;
                let permit = tokio::select! {
                    biased;
                    () = token.cancelled() => return Err(HostError::Cancelled),
                    permit = tokio::time::timeout(deadline.saturating_duration_since(Instant::now()),
                        Arc::clone(&cache.build).acquire_owned()) => permit
                        .map_err(|_| HostError::Timeout("the directory metadata builder is busy".to_owned()))?
                        .map_err(|_| HostError::Busy("the directory metadata builder stopped".to_owned()))?,
                };
                let worker = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    #[cfg(all(test, target_os = "linux"))]
                    let _exit = cache.testing.exit();
                    cache.capture(source_binding, files, path, &token, deadline, &proof)
                });
                // No timeout drops this handle. Readers have their own bounded waits; this owner
                // always joins the producer, whose cooperative deadline/cancellation is checked
                // while visiting/sorting. A blocked native call keeps its actual resources charged.
                worker.await.map_err(|error| HostError::Internal(format!("the directory metadata worker failed: {error}")))?
            }.await;
            let result = match result {
                Err(HostError::Cancelled) if reason.invalid.load(Ordering::Acquire) =>
                    Err(HostError::Changed("the directory observation was invalidated by a known change".to_owned())),
                other => other,
            };
            Arc::new(result)
        }.boxed().shared();
        // Keep polling the shared future even if every request is cancelled. The original producer
        // is joined and drops its native/filesystem owners before this driver can finish.
        let driven = outcome.clone();
        let driver = tokio::spawn(async move {
            drop(driven.await);
        });
        let build = Arc::new(Self {
            binding,
            location,
            observation,
            covered,
            cancel,
            state,
            outcome,
            driver: tokio::sync::Mutex::new(Some(driver)),
            #[cfg(all(test, target_os = "linux"))]
            testing,
        });
        let reader = Reader {
            build: Arc::clone(&build),
        };
        (build, reader)
    }

    pub(super) fn reader(
        self: &Arc<Self>,
        binding: &Binding,
        location: &DirectoryLocation,
        covered: bool,
    ) -> Result<Option<Reader>> {
        if self.binding != *binding || self.location != *location || (covered && !self.covered) {
            return Ok(None);
        }
        #[cfg(all(test, target_os = "linux"))]
        self.testing.before_reservation()?;
        let mut readers = lock(&self.state.readers);
        let eligible = !readers.closed && !self.cancel.is_cancelled() && !self.observation.invalid.load(Ordering::Acquire)
            // A completed capture is never a fresh first-page cache, even if its async result
            // has not yet been picked up by the first reader.
            && !self.observation.published.load(Ordering::Acquire) && self.outcome.peek().is_none();
        if !eligible {
            return Ok(None);
        }
        readers.count += 1;
        Ok(Some(Reader {
            build: Arc::clone(self),
        }))
    }

    pub(super) fn invalidate(&self) {
        self.observation.invalid.store(true, Ordering::Release);
        self.cancel.cancel();
    }

    pub(super) fn cancel(&self) {
        self.invalidate();
    }

    pub(super) fn reclaimable(&self) -> bool {
        lock(&self.state.readers).count == 0 && self.outcome.peek().is_some()
    }

    pub(super) async fn join(&self) -> Result<()> {
        let mut driver = self.driver.lock().await;
        if let Some(task) = driver.as_mut() {
            let joined = task.await;
            *driver = None;
            joined.map_err(|error| {
                HostError::Internal(format!("the directory capture driver failed: {error}"))
            })?;
        }
        Ok(())
    }
}

impl Reader {
    pub(super) async fn snapshot(&self, deadline: Instant) -> Result<Arc<Snapshot>> {
        let outcome = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            self.build.outcome.clone(),
        )
        .await
        .map_err(|_| {
            HostError::Timeout("the directory metadata capture took too long".to_owned())
        })?;
        self.build.join().await?;
        match outcome.as_ref() {
            Ok(snapshot) => Ok(Arc::clone(snapshot)),
            Err(error) => Err(copy_error(error)),
        }
    }
}

impl DirectoryPages {
    pub(super) async fn reap(&self) -> Result<()> {
        let completed = {
            let mut builds = lock(&self.captures);
            let mut completed = Vec::new();
            builds.retain(|build| {
                if build.reclaimable() {
                    completed.push(Arc::clone(build));
                    false
                } else {
                    true
                }
            });
            completed
        };
        for build in completed {
            build.join().await?;
        }
        Ok(())
    }

    /// Joins cancelled captures for an attachment before its other native owners are torn down.
    pub(crate) async fn join_attachment(
        &self,
        environment: orch_types::Id,
        attachment: orch_types::Id,
    ) -> Result<()> {
        let builds: Vec<_> = lock(&self.captures)
            .iter()
            .filter(|build| {
                build.binding.environment == environment && build.binding.attachment == attachment
            })
            .cloned()
            .collect();
        for build in builds {
            build.cancel();
            build.join().await?;
        }
        self.reap().await
    }
}

// Shared futures retain one error. Reconstruct the same typed failure for each reader rather
// than erase its normalized code/retryability. IO keeps its OS code, or kind and original words.
fn copy_error(error: &HostError) -> HostError {
    match error {
        HostError::Cancelled => HostError::Cancelled,
        HostError::NotFound(value) => HostError::NotFound(value.clone()),
        HostError::Ignored(value) => HostError::Ignored(value.clone()),
        HostError::Exists(value) => HostError::Exists(value.clone()),
        HostError::Stale { current } => HostError::Stale {
            current: current.clone(),
        },
        HostError::Changed(value) => HostError::Changed(value.clone()),
        HostError::RestoreRecoveryRequired(value) => {
            HostError::RestoreRecoveryRequired(value.clone())
        }
        HostError::TooLarge { what, size, limit } => HostError::TooLarge {
            what: what.clone(),
            size: *size,
            limit: *limit,
        },
        HostError::Unsupported(value) => HostError::Unsupported(value.clone()),
        HostError::RemoteWriteDisabled => HostError::RemoteWriteDisabled,
        HostError::DeliverFilesDisabled => HostError::DeliverFilesDisabled,
        HostError::RootNotAllowed(value) => HostError::RootNotAllowed(value.clone()),
        HostError::ExecNotOffered(value) => HostError::ExecNotOffered(value.clone()),
        HostError::McpImportDisabled => HostError::McpImportDisabled,
        HostError::McpAddDisabled => HostError::McpAddDisabled,
        HostError::Timeout(value) => HostError::Timeout(value.clone()),
        HostError::Busy(value) => HostError::Busy(value.clone()),
        HostError::Content(value) => HostError::Content(value.clone()),
        HostError::Io { context, source } => HostError::Io {
            context: context.clone(),
            source: source.raw_os_error().map_or_else(
                || std::io::Error::new(source.kind(), source.to_string()),
                std::io::Error::from_raw_os_error,
            ),
        },
        HostError::Internal(value) => HostError::Internal(value.clone()),
    }
}
