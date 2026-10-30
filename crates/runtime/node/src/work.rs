//! Bounded latest-value filesystem progress on the node feed, with the command's cooperative cancellation.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use orch_api_contract::workspace::{JobActivity, JobProgress};
use orch_driver_contract::volumes::{VolumeProgress, VolumeWork};
use orch_link_contract::commands::WorkReport;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::facts::Reports;

/// The transport's maximum report frequency; the latest count replaces intermediate samples.
const REPORT_EVERY: Duration = Duration::from_secs(1);
/// The controller records completion only after the command returns its verified complete result.
const LAST_ACTIVE_PERCENT: i128 = 99;

struct Work {
    command: Option<orch_types::Id>,
    counts: watch::Sender<Option<VolumeProgress>>,
    cancel: CancellationToken,
    timeout_secs: u64,
}

struct OffsetWork {
    parent: Arc<dyn VolumeWork>,
    done: i64,
    total: i64,
}

struct ScopedWork {
    parent: Arc<dyn VolumeWork>,
    cancel: CancellationToken,
    command: orch_types::Id,
}

#[async_trait]
impl VolumeWork for ScopedWork {
    fn command_id(&self) -> Option<orch_types::Id> {
        Some(self.command)
    }
    fn report(&self, progress: VolumeProgress) {
        if !self.is_cancelled() {
            self.parent.report(progress);
        }
    }
    fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled() || self.parent.is_cancelled()
    }
    async fn cancelled(&self) {
        tokio::select! {
            () = self.cancel.cancelled() => {},
            () = self.parent.cancelled() => {},
        }
    }
    fn timeout_secs(&self) -> u64 {
        self.parent.timeout_secs()
    }
}

/// A dropped waiter requests cancellation; the tracked worker keeps its locks and joins cleanup.
pub(crate) async fn owned<T, F, Fut>(
    tasks: &TaskTracker,
    stop: &CancellationToken,
    command: orch_types::Id,
    parent: Arc<dyn VolumeWork>,
    operation: F,
) -> Result<T, orch_link_contract::commands::CommandError>
where
    T: Send + 'static,
    F: FnOnce(Arc<dyn VolumeWork>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, orch_link_contract::commands::CommandError>>
        + Send
        + 'static,
{
    let cancel = stop.child_token();
    let on_drop = cancel.clone().drop_guard();
    let work: Arc<dyn VolumeWork> = Arc::new(ScopedWork {
        parent,
        cancel,
        command,
    });
    check(&work)?;
    let result = tasks
        .spawn(async move { operation(work).await })
        .await
        .map_err(|_| {
            orch_link_contract::commands::CommandError::new(
                orch_link_contract::commands::ErrorCode::Internal,
                "the filesystem worker failed",
            )
        });
    let _ = on_drop.disarm();
    result?
}

/// Refuses the next filesystem step after either the command or its waiter was cancelled.
pub(crate) fn check(
    work: &Arc<dyn VolumeWork>,
) -> Result<(), orch_link_contract::commands::CommandError> {
    if work.is_cancelled() {
        Err(cancelled())
    } else {
        Ok(())
    }
}

/// The stable refusal for filesystem work stopped before its next step.
pub(crate) fn cancelled() -> orch_link_contract::commands::CommandError {
    orch_link_contract::commands::CommandError::new(
        orch_link_contract::commands::ErrorCode::Conflict,
        "the filesystem work was cancelled",
    )
}
#[async_trait]
impl VolumeWork for OffsetWork {
    fn command_id(&self) -> Option<orch_types::Id> {
        self.parent.command_id()
    }
    fn report(&self, mut progress: VolumeProgress) {
        progress.bytes_done = self.done.saturating_add(progress.bytes_done);
        progress.bytes_total = Some(self.total);
        self.parent.report(progress);
    }
    fn is_cancelled(&self) -> bool {
        self.parent.is_cancelled()
    }
    async fn cancelled(&self) {
        self.parent.cancelled().await;
    }
    fn timeout_secs(&self) -> u64 {
        self.parent.timeout_secs()
    }
}

/// A blob part reports its place in the entire archive, rather than starting the bar over per part.
pub(crate) fn offset(parent: Arc<dyn VolumeWork>, done: i64, total: i64) -> Arc<dyn VolumeWork> {
    Arc::new(OffsetWork {
        parent,
        done,
        total,
    })
}

/// An unobserved compatibility call still uses the same cooperative helper boundary.
pub(crate) fn silent(timeout_secs: u64) -> Arc<dyn VolumeWork> {
    let (counts, _) = watch::channel(None);
    Arc::new(Work {
        command: None,
        counts,
        cancel: CancellationToken::new(),
        timeout_secs,
    })
}

#[async_trait]
impl VolumeWork for Work {
    fn command_id(&self) -> Option<orch_types::Id> {
        self.command
    }
    fn report(&self, progress: VolumeProgress) {
        self.counts.send_replace(Some(progress));
    }
    fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
    async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }
    fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }
}

/// Opens a latest-value reporter, joined after all work and cleanup have stopped.
pub(crate) fn start(
    reports: Arc<Reports>,
    command: orch_types::Id,
    clock: Arc<dyn orch_types::Clock>,
    report: Option<WorkReport>,
    cancel: CancellationToken,
    fallback_timeout_secs: u64,
) -> (Arc<dyn VolumeWork>, JoinHandle<()>) {
    let (counts, mut rx) = watch::channel(None::<VolumeProgress>);
    let timeout_secs = report
        .as_ref()
        .map_or(fallback_timeout_secs, |report| report.timeout_secs);
    let work: Arc<dyn VolumeWork> = Arc::new(Work {
        command: Some(command),
        counts,
        cancel,
        timeout_secs,
    });
    let task = tokio::spawn(async move {
        let Some(report) = report else {
            return;
        };
        let started = Instant::now();
        let mut interval = tokio::time::interval(REPORT_EVERY);
        loop {
            let closed = tokio::select! {
                _ = interval.tick() => false,
                changed = rx.changed() => {
                    if changed.is_ok() { continue; }
                    true
                }
            };
            let value = rx.borrow_and_update().clone();
            if let Some(counts) = &value {
                let bytes_total = counts.bytes_total.or(report.bytes_total);
                let files_total = counts.files_total.or(report.files_total);
                let seconds = started.elapsed().as_secs_f64();
                let rate = (seconds > 0.0).then(|| (counts.bytes_done as f64 / seconds) as i64);
                let eta = rate.filter(|rate| *rate > 0).and_then(|rate| {
                    bytes_total.map(|total| total.saturating_sub(counts.bytes_done).max(0) / rate)
                });
                let portion = bytes_total
                    .filter(|total| *total > 0)
                    .map(|total| (counts.bytes_done, total))
                    .or_else(|| {
                        files_total
                            .filter(|total| *total > 0)
                            .map(|total| (counts.files_done, total))
                    });
                let percent = portion.map_or(0, |(done, total)| {
                    ((i128::from(done) * 100 / i128::from(total)).clamp(0, LAST_ACTIVE_PERCENT))
                        as u32
                });
                reports
                    .job_command(
                        report.job_id,
                        Some(command),
                        &JobProgress {
                            activity: Some(JobActivity {
                                waiting_for: None,
                                updated_at: clock.time(),
                                stall_after_seconds: 0,
                                directories_done: counts.directories_done,
                                stopping: false,
                            }),
                            phase: report.phase.clone(),
                            phases: Vec::new(),
                            percent,
                            files_done: counts.files_done,
                            files_total,
                            bytes_done: counts.bytes_done,
                            bytes_total,
                            rate_bps: rate,
                            eta_seconds: eta,
                            message: counts.message.clone(),
                        },
                        None,
                    )
                    .await;
            }
            if closed {
                return;
            }
        }
    });
    (work, task)
}

/// Copies through a bounded buffer, stopping a blocked transfer and flushing its last filesystem write before acknowledgement.
pub(crate) async fn copy(
    input: &mut (impl AsyncRead + Unpin + ?Sized),
    output: &mut (impl AsyncWrite + Unpin + ?Sized),
    work: &Arc<dyn VolumeWork>,
) -> std::io::Result<u64> {
    // One filesystem chunk may finish after the request; no whole archive part can outlive its owner.
    const COPY_BYTES: usize = 64 * 1024;
    let copying = async {
        let mut buffer = [0u8; COPY_BYTES];
        let mut total = 0u64;
        loop {
            let read = tokio::select! {
                biased;
                () = work.cancelled() => return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "the transfer was cancelled")),
                read = input.read(&mut buffer) => read?,
            };
            if read == 0 { return Ok(total); }
            tokio::select! {
                biased;
                () = work.cancelled() => return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "the transfer was cancelled")),
                written = output.write_all(&buffer[..read]) => written?,
            }
            total = total.saturating_add(read as u64);
        }
    }.await;
    let flushed = output.flush().await;
    copying.and_then(|bytes| flushed.map(|()| bytes))
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[tokio::test]
    async fn cancelling_a_copy_blocked_on_its_pipe_returns_without_waiting_for_a_reader() {
        let cancel = CancellationToken::new();
        let (counts, _) = watch::channel(None);
        let work: Arc<dyn VolumeWork> = Arc::new(Work {
            command: None,
            counts,
            cancel: cancel.clone(),
            timeout_secs: 30,
        });
        let bytes = [1u8; 4096];
        let mut input = bytes.as_slice();
        let (mut output, mut reader) = tokio::io::duplex(16);
        let copying = copy(&mut input, &mut output, &work);
        tokio::pin!(copying);
        let mut first = [0u8; 1];
        tokio::select! {
            result = &mut copying => panic!("the copy ended without filling its pipe: {result:?}"),
            read = reader.read_exact(&mut first) => { read.unwrap(); },
        }
        cancel.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(1), &mut copying)
            .await
            .unwrap();
        assert_eq!(stopped.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
    }
}
