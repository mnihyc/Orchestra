//! Attachment-scoped admission and joined blocking directory reads.

use super::{Coverage, DirectoryPages, check, lock};
use crate::error::{HostError, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct WorkSet {
    retired: bool,
    workers: Vec<Arc<Worker>>,
}

/// One bounded host registry, including cancelled requests whose native calls have not exited.
/// Completed records keep their allowance until actual-use reaping joins/releases them; another
/// attachment can reap them too. There are never 256 retained worker sets per watch/environment.
pub(super) struct Owners {
    slots: Arc<Semaphore>,
    attachments: Mutex<Vec<Weak<Attachment>>>,
}

impl Owners {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: Arc::new(Semaphore::new(super::SLOTS)),
            attachments: Mutex::new(Vec::new()),
        })
    }

    async fn reap(&self) -> Result<()> {
        let attachments: Vec<_> = lock(&self.attachments)
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for attachment in attachments {
            attachment.reap().await?;
        }
        // All synchronous two-lock operations order registry -> attachment. Neither spans await.
        lock(&self.attachments).retain(|attachment| {
            attachment
                .upgrade()
                .is_some_and(|attachment| !lock(&attachment.work).workers.is_empty())
        });
        Ok(())
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn available(&self) -> usize {
        self.slots.available_permits()
    }
}

/// One actual attachment, retained by its readers; no global retired-ID/tombstone table.
pub(crate) struct Attachment {
    work: Mutex<WorkSet>,
    cancel: CancellationToken,
}

struct Worker {
    done: Arc<AtomicBool>,
    driver: tokio::sync::Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
    // Keep only the host allowance after exit, not the request's Environment owner: retaining
    // that owner here would make an attachment -> worker -> attachment cycle when a host drops.
    _permit: Arc<OwnedSemaphorePermit>,
}

struct Request {
    cancel: CancellationToken,
    deadline: Instant,
    busy: AtomicBool,
    // One of the host's existing directory-owner allowance, retained until actual blocking exit.
    permit: Arc<OwnedSemaphorePermit>,
    // An old plain/no-store attachment must not drop its native owners while work is still in it.
    _owner: Option<Coverage>,
}

struct Phase(Arc<Request>);
impl Drop for Phase {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

pub(crate) struct Operation {
    attachment: Arc<Attachment>,
    request: Arc<Request>,
    owners: Arc<Owners>,
    coverage: Option<Coverage>,
}

impl Drop for Operation {
    fn drop(&mut self) {
        self.request.cancel.cancel();
    }
}

impl Attachment {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            work: Mutex::new(WorkSet::default()),
            cancel: CancellationToken::new(),
        })
    }

    pub(crate) async fn begin(
        self: &Arc<Self>,
        cache: &DirectoryPages,
        owner: Option<Coverage>,
    ) -> Result<Operation> {
        if self.cancel.is_cancelled() {
            return Err(retired());
        }
        let deadline = Instant::now()
            + Duration::from_secs(u64::from(
                orch_link_contract::commands::CommandKind::FsList.deadline_secs(),
            ));
        cache.operations.reap().await?;
        let work = lock(&self.work);
        if work.retired {
            return Err(retired());
        }
        let permit = Arc::clone(&cache.operations.slots)
            .try_acquire_owned()
            .map_err(|_| {
                HostError::Busy("the host has no unused directory read owner".to_owned())
            })?;
        let request = Arc::new(Request {
            cancel: self.cancel.child_token(),
            deadline,
            busy: AtomicBool::new(false),
            permit: Arc::new(permit),
            _owner: owner,
        });
        Ok(Operation {
            attachment: Arc::clone(self),
            request,
            owners: Arc::clone(&cache.operations),
            coverage: None,
        })
    }

    /// Seals admission before collecting owners. Existing operations cannot start a later phase.
    pub(crate) fn retire(&self) {
        let mut work = lock(&self.work);
        work.retired = true;
        self.cancel.cancel();
    }

    pub(crate) async fn join(&self) -> Result<()> {
        self.retire();
        let workers = lock(&self.work).workers.clone();
        let mut failure = None;
        for worker in workers {
            if let Err(error) = worker.join().await {
                failure.get_or_insert(error);
            }
        }
        self.reap().await?;
        failure.map_or(Ok(()), Err)
    }

    async fn reap(&self) -> Result<()> {
        let completed = {
            let mut work = lock(&self.work);
            let mut completed = Vec::new();
            work.workers.retain(|worker| {
                if worker.done.load(Ordering::Acquire) {
                    completed.push(Arc::clone(worker));
                    false
                } else {
                    true
                }
            });
            completed
        };
        for worker in completed {
            worker.join().await?;
        }
        Ok(())
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) async fn retired(&self) {
        self.cancel.cancelled().await;
    }
}

impl Operation {
    pub(crate) fn deadline(&self) -> Instant {
        self.request.deadline
    }
    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.request.cancel.clone()
    }
    pub(crate) fn cover(&mut self, coverage: Option<Coverage>) {
        self.coverage = coverage;
    }
    pub(super) fn coverage(&self) -> Option<Coverage> {
        self.coverage.clone()
    }

    /// Orders synchronous phase admission (including shared producer creation) against retirement.
    pub(crate) fn admit<T>(&self, action: impl FnOnce() -> Result<T>) -> Result<T> {
        let work = lock(&self.attachment.work);
        if work.retired {
            return Err(retired());
        }
        check(&self.request.cancel, self.request.deadline)?;
        action()
    }

    pub(crate) fn check(&self) -> Result<()> {
        self.admit(|| Ok(()))
    }

    /// The awaiting request may leave on cancellation/deadline; its driver always joins the
    /// real blocking call. The attachment can therefore join even pre-capture stats and cursor
    /// page I/O. No timeout/Drop releases a still-running native owner or its host allowance.
    pub(crate) async fn blocking<T: Send + 'static>(
        &self,
        action: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.owners.reap().await?;
        let (worker, answer) = {
            let mut owners = lock(&self.owners.attachments);
            owners.retain(|attachment| {
                attachment
                    .upgrade()
                    .is_some_and(|attachment| !lock(&attachment.work).workers.is_empty())
            });
            let mut work = lock(&self.attachment.work);
            if work.retired {
                return Err(retired());
            }
            check(&self.request.cancel, self.request.deadline)?;
            // At most one blocking phase per live request, with the same host-wide256 owners.
            // Finished drivers are reaped on actual use; no upkeep timer or per-watch allocation.
            if work.workers.len() >= super::SLOTS {
                return Err(HostError::Busy(
                    "the attachment has no unused directory worker owner".to_owned(),
                ));
            }
            if !owners
                .iter()
                .any(|attachment| attachment.ptr_eq(&Arc::downgrade(&self.attachment)))
            {
                if owners.len() >= super::SLOTS {
                    return Err(HostError::Busy(
                        "the host has no unused directory worker registry slot".to_owned(),
                    ));
                }
                owners.push(Arc::downgrade(&self.attachment));
            }
            if self.request.busy.swap(true, Ordering::AcqRel) {
                return Err(HostError::Busy(
                    "another blocking phase of this directory read is still running".to_owned(),
                ));
            }
            let (send, answer) = oneshot::channel();
            let request = Arc::clone(&self.request);
            let phase = Phase(Arc::clone(&self.request));
            let coverage = self.coverage.clone();
            let done = Arc::new(AtomicBool::new(false));
            let finished = Arc::clone(&done);
            let driver = tokio::spawn(async move {
                let native = tokio::task::spawn_blocking(move || {
                    let _coverage = coverage;
                    check(&request.cancel, request.deadline)?;
                    let result = action()?;
                    check(&request.cancel, request.deadline)?;
                    Ok(result)
                });
                let (result, joined) = match native.await {
                    Ok(result) => (result, Ok(())),
                    Err(error) => {
                        let message = format!("the directory read worker failed: {error}");
                        (
                            Err(HostError::Internal(message.clone())),
                            Err(HostError::Internal(message)),
                        )
                    }
                };
                drop(phase);
                let _ = send.send(result);
                finished.store(true, Ordering::Release);
                joined
            });
            let worker = Arc::new(Worker {
                done,
                driver: tokio::sync::Mutex::new(Some(driver)),
                _permit: Arc::clone(&self.request.permit),
            });
            work.workers.push(Arc::clone(&worker));
            (worker, answer)
        };
        let result = tokio::select! {
            biased;
            () = self.request.cancel.cancelled() => { self.check()?; return Err(HostError::Cancelled); }
            answer = tokio::time::timeout(self.request.deadline.saturating_duration_since(Instant::now()), answer) => match answer {
                Ok(answer) => answer.map_err(|_| HostError::Internal("the directory read driver ended without its answer".to_owned()))?,
                Err(_) => {
                    self.request.cancel.cancel();
                    return Err(HostError::Timeout("the directory metadata read exceeded its command budget".to_owned()));
                }
            },
        };
        worker.join().await?;
        self.attachment.reap().await?;
        self.check()?;
        result
    }
}

impl Worker {
    async fn join(&self) -> Result<()> {
        let mut driver = self.driver.lock().await;
        if let Some(task) = driver.as_mut() {
            let joined = task.await;
            *driver = None;
            joined.map_err(|error| {
                HostError::Internal(format!("the directory read driver failed: {error}"))
            })??;
        }
        Ok(())
    }
}

fn retired() -> HostError {
    HostError::Changed("the attached environment ended during the directory read".to_owned())
}
