//! The failures an environment host reports, and the code a command answers with for each.
//!
//! Every function of this package that a command of the environment role reaches ends in a
//! [`HostError`]. The variants are the reasons a person or the system can act on; what the
//! operating system said is kept as the detail beside them and is never the reason itself.
//! [`HostError::command_error`] gives the code of the command catalogue and whether asking again
//! may help.

use orch_api_contract::workspace::FileStat;
use orch_link_contract::commands::{CommandError, ErrorCode};

/// What a function of the environment host returns.
pub type Result<T, E = HostError> = std::result::Result<T, E>;

/// Why something an environment was asked to do did not happen.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The caller cancelled an operation before publication.
    #[error("the recording was cancelled")]
    Cancelled,
    /// The path, process, shell or watch named does not exist here, or a path leaves the place it
    /// has to stay in, or is not one a path may be.
    #[error("{0}")]
    NotFound(String),
    /// The workspace's ignore rules leave the path out.
    #[error("{0}")]
    Ignored(String),
    /// The destination of a rename, a copy or a new directory is already there.
    #[error("{0}")]
    Exists(String),
    /// A write was made on a version that is not the file's any more; nothing was written.
    #[error("the file is not the version that was expected")]
    Stale {
        /// The file as it stands now, when there is one.
        current: Option<Box<FileStat>>,
    },
    /// The file was another by the time it was read; asking again reads the new one.
    #[error("{0}")]
    Changed(String),
    /// A journal still owns applied paths; the controller must retain its file and turn hold.
    #[error("{0}")]
    RestoreRecoveryRequired(String),
    /// The content is larger than may be stored.
    #[error("{what} has {size} bytes, more than the {limit} that are allowed")]
    TooLarge {
        /// What is too large.
        what: String,
        /// Its size in bytes.
        size: i64,
        /// The most that is allowed, in bytes.
        limit: i64,
    },
    /// This machine, or this platform, cannot do that, or the request asks for what is not offered.
    #[error("{0}")]
    Unsupported(String),
    /// The machine's owner accepts no files written from outside.
    #[error("this machine accepts no files written from outside")]
    RemoteWriteDisabled,
    /// The machine's owner accepts no files that arrive with a message.
    #[error("this machine accepts no files that arrive with a message")]
    DeliverFilesDisabled,
    /// The machine's owner does not offer that directory.
    #[error("{0}")]
    RootNotAllowed(String),
    /// The machine's owner does not let its exec node offer that.
    #[error("{0}")]
    ExecNotOffered(String),
    /// The machine's owner lets no tool server be imported from it.
    #[error("this machine lets no tool server be imported from it")]
    McpImportDisabled,
    /// The machine's owner lets no tool server be written from Orch for it.
    #[error("this machine lets no tool server be added from Orch")]
    McpAddDisabled,
    /// It took longer than it may.
    #[error("{0}")]
    Timeout(String),
    /// A resource the request needs is held by something else right now, or is full.
    #[error("{0}")]
    Busy(String),
    /// The content store could not give or take content.
    #[error("{0}")]
    Content(String),
    /// The operating system refused, or failed, in a way that is none of the above.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: String,
        /// What the system said.
        #[source]
        source: std::io::Error,
    },
    /// Something that should hold did not; a defect, not a state of the world.
    #[error("{0}")]
    Internal(String),
}

impl HostError {
    /// An operating-system failure while doing `context`, sorted into the reason a person can
    /// act on where there is one.
    #[must_use]
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        use std::io::ErrorKind;
        let context = context.into();
        match source.kind() {
            ErrorKind::NotFound | ErrorKind::NotADirectory => Self::NotFound(context),
            ErrorKind::AlreadyExists => Self::Exists(context),
            ErrorKind::DirectoryNotEmpty => {
                Self::Exists(format!("{context}: the directory is not empty"))
            }
            ErrorKind::TimedOut => Self::Timeout(context),
            _ => Self::Io { context, source },
        }
    }

    /// A `not_found` that says why.
    #[must_use]
    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::NotFound(detail.into())
    }

    /// The code the command catalogue answers with for this failure.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Ignored(_) => ErrorCode::Ignored,
            Self::Exists(_) | Self::Stale { .. } | Self::Cancelled => ErrorCode::Conflict,
            Self::Changed(_) => ErrorCode::Changed,
            Self::RestoreRecoveryRequired(_) => ErrorCode::RestoreRecoveryRequired,
            Self::TooLarge { .. } => ErrorCode::TooLarge,
            Self::Unsupported(_) => ErrorCode::Unsupported,
            Self::RemoteWriteDisabled => ErrorCode::PolicyRemoteWriteDisabled,
            Self::DeliverFilesDisabled => ErrorCode::PolicyDeliverFilesDisabled,
            Self::RootNotAllowed(_) => ErrorCode::PolicyRootNotAllowed,
            Self::ExecNotOffered(_) => ErrorCode::PolicyExecNotOffered,
            Self::McpImportDisabled => ErrorCode::PolicyMcpImportDisabled,
            Self::McpAddDisabled => ErrorCode::PolicyMcpAddDisabled,
            Self::Timeout(_) => ErrorCode::Timeout,
            Self::Busy(_) => ErrorCode::Busy,
            Self::Content(_) | Self::Io { .. } | Self::Internal(_) => ErrorCode::Internal,
        }
    }

    /// Whether the same command may succeed if it is asked again.
    #[must_use]
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Changed(_)
                | Self::RestoreRecoveryRequired(_)
                | Self::Timeout(_)
                | Self::Busy(_)
                | Self::Content(_)
        )
    }

    /// The failure as a command answers with it.
    #[must_use]
    pub fn command_error(&self) -> CommandError {
        let mut error = CommandError::new(self.code(), self.to_string());
        if let Self::Stale { current } = self {
            error = error.with_details(serde_json::json!({ "current": current }));
        }
        if self.retryable() {
            error.retryable()
        } else {
            error
        }
    }
}

tokio::task_local! {
    static ASYNC_CANCEL: tokio_util::sync::CancellationToken;
    static WORKERS: std::sync::Arc<Workers>;
}
thread_local! {
    static BLOCKING_CANCEL: std::cell::RefCell<Option<tokio_util::sync::CancellationToken>> = const { std::cell::RefCell::new(None) };
}

/// Runs filesystem work under the calling command's cancellation token, including its blocking workers.
pub async fn with_cancellation<F: std::future::Future>(
    cancel: tokio_util::sync::CancellationToken,
    work: F,
) -> F::Output {
    let workers = std::sync::Arc::new(Workers::default());
    let result = WORKERS
        .scope(
            std::sync::Arc::clone(&workers),
            ASYNC_CANCEL.scope(cancel, work),
        )
        .await;
    loop {
        let changed = workers.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if workers.count.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            break;
        }
        changed.await;
    }
    result
}

#[derive(Default)]
struct Workers {
    count: std::sync::atomic::AtomicUsize,
    changed: tokio::sync::Notify,
}
struct Worker(Option<std::sync::Arc<Workers>>);
impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(workers) = &self.0 {
            workers
                .count
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            workers.changed.notify_waiters();
        }
    }
}

/// The current command's cancellation, when a command owns this task.
pub fn cancellation() -> Option<tokio_util::sync::CancellationToken> {
    ASYNC_CANCEL
        .try_with(Clone::clone)
        .ok()
        .or_else(|| BLOCKING_CANCEL.with(|value| value.borrow().clone()))
}

/// Stops a filesystem loop before its next entry or bounded I/O chunk after cancellation.
pub(crate) fn check_cancelled() -> Result<()> {
    if cancellation().is_some_and(|cancel| cancel.is_cancelled()) {
        Err(HostError::Cancelled)
    } else {
        Ok(())
    }
}

struct BlockingScope(Option<tokio_util::sync::CancellationToken>);
impl Drop for BlockingScope {
    fn drop(&mut self) {
        BLOCKING_CANCEL.with(|value| {
            value.replace(self.0.take());
        });
    }
}

/// Runs blocking work while retaining its command's cancellation and completion ownership.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let cancel = cancellation();
    let progress = crate::snapshot::current_progress();
    let workers = WORKERS.try_with(std::sync::Arc::clone).ok();
    if let Some(workers) = &workers {
        workers
            .count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    let worker = Worker(workers);
    tokio::task::spawn_blocking(move || {
        let _worker = worker;
        let _progress = crate::snapshot::BlockingProgress::enter(progress);
        let _scope = BlockingScope(BLOCKING_CANCEL.with(|value| value.replace(cancel)));
        check_cancelled()?;
        work()
    })
    .await
    .map_err(|error| HostError::Internal(format!("a task of the host failed: {error}")))?
}

/// Applies the same ownership to positional file reads and writes.
pub(crate) async fn blocking_io<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> std::io::Result<T> {
    blocking(move || work().map_err(|error| HostError::io("transferring file content", error)))
        .await
        .map_err(std::io::Error::other)
}

impl From<HostError> for CommandError {
    fn from(error: HostError) -> Self {
        error.command_error()
    }
}

impl From<serde_json::Error> for HostError {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(format!("a value could not be written or read: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_answers_with_the_code_a_person_can_act_on() {
        let stale = HostError::Stale { current: None };
        assert_eq!(stale.command_error().code, "conflict");
        assert_eq!(
            stale.command_error().details,
            Some(serde_json::json!({"current": null}))
        );
        assert!(
            !stale.command_error().retryable,
            "asking again on the same version fails again"
        );
        let changed = HostError::Changed("it changed while it was read".into());
        assert_eq!(changed.command_error().code, "changed");
        assert!(changed.command_error().retryable);
        assert_eq!(
            HostError::RemoteWriteDisabled.command_error().code,
            "policy.remote_write_disabled"
        );
    }

    #[test]
    fn what_the_system_says_is_sorted_into_a_reason() {
        let gone = HostError::io(
            "reading a.txt",
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert!(matches!(gone, HostError::NotFound(_)));
        let there = HostError::io(
            "making a",
            std::io::Error::from(std::io::ErrorKind::AlreadyExists),
        );
        assert!(matches!(there, HostError::Exists(_)));
        let denied = HostError::io(
            "writing a",
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert_eq!(denied.command_error().code, "internal");
        assert!(denied.to_string().contains("writing a"));
    }
}
