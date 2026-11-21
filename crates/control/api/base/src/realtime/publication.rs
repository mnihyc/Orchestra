//! Final existing queue/socket acceptance after capacity waits; no serialized authority/lease.

use super::ticket_origin::CurrentCaller;

use super::ticket_origin::deadline_allows;

use crate::middleware::requests::ApiResult;

use axum::extract::ws::Message;

use futures::Sink;

use futures::SinkExt;
use futures::task::AtomicWaker;

use orch_types::Clock;

use std::future::Future;

use std::future::poll_fn;

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Wake, Waker};

use tokio::sync::mpsc;
/// The preparation future is first polled AFTER socket capacity is ready. It runs the
/// owner's actual current checks and builds only that owner's message. The raw caller/
/// session deadline survives all its awaits and is checked immediately before start_send.
pub(super) async fn send<S, F>(sink: &mut S, clock: &dyn Clock, prepare: F) -> ApiResult<bool>
where
    S: Sink<Message> + Unpin,
    F: Future<Output = ApiResult<Option<(Message, CurrentCaller)>>>,
{
    if poll_fn(|cx| Pin::new(&mut *sink).poll_ready(cx))
        .await
        .is_err()
    {
        return Ok(false);
    }
    let Some((message, (caller, session))) = prepare.await? else {
        return Ok(false);
    };
    if !deadline_allows(&caller, session, clock) {
        return Ok(false);
    }
    if Pin::new(&mut *sink).start_send(message).is_err() {
        return Ok(false);
    }
    // This exact message has entered the existing socket buffer. Flushing its accepted
    // bytes creates no new authorization and retains no database transaction.
    Ok(sink.flush().await.is_ok())
}

/// Reserves existing channel capacity before owner authorization; no slot wait can age
/// the returned caller before the actual synchronous send. Activity is recorded by the
/// owner only after true is returned, never for a refused/unaccepted input.
pub(super) async fn enqueue<T, F>(
    sender: &mpsc::Sender<T>,
    clock: &dyn Clock,
    prepare: F,
) -> ApiResult<bool>
where
    F: Future<Output = ApiResult<Option<(T, CurrentCaller)>>>,
{
    let Ok(slot) = sender.reserve().await else {
        return Ok(false);
    };
    let Some((value, (caller, session))) = prepare.await? else {
        return Ok(false);
    };
    if !deadline_allows(&caller, session, clock) {
        return Ok(false);
    }
    slot.send(value);
    Ok(true)
}

/// Only the native writer can announce that its blocked first byte may now proceed.
struct WriterWake {
    ready: AtomicBool,
    task: AtomicWaker,
}

impl Wake for WriterWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.ready.store(true, Ordering::Release);
        self.task.wake();
    }
}

/// A single existing parser message. Pending first bytes recheck after a native writer wake.
/// Once actual bytes of this exact message enter the native buffer, completing only its
/// remaining bytes preserves protocol framing/accepted work; no next message borrows it.
pub(super) async fn write_message<W, Make, F>(
    writer: &mut W,
    bytes: &[u8],
    clock: &dyn Clock,
    mut prepare: Make,
) -> ApiResult<Option<std::io::Result<()>>>
where
    W: tokio::io::AsyncWrite + Unpin,
    Make: FnMut() -> F,
    F: Future<Output = ApiResult<Option<CurrentCaller>>>,
{
    use tokio::io::AsyncWriteExt;
    let wake = Arc::new(WriterWake {
        ready: AtomicBool::new(false),
        task: AtomicWaker::new(),
    });
    let writer_waker = Waker::from(Arc::clone(&wake));
    let mut waiting = false;
    let mut check: Option<Pin<Box<F>>> = None;
    let first = poll_fn(|cx| {
        wake.task.register(cx.waker());
        if waiting && !wake.ready.swap(false, Ordering::AcqRel) {
            return std::task::Poll::Pending;
        }
        waiting = false;
        // A database completion may wake this task after the writer became blocked. It must
        // not start a check against an earlier policy snapshot while capacity is still absent.
        let checked = check
            .get_or_insert_with(|| Box::pin(prepare()))
            .as_mut()
            .poll(cx);
        match checked {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(Err(error)) => std::task::Poll::Ready(Err(error)),
            std::task::Poll::Ready(Ok(None)) => std::task::Poll::Ready(Ok(None)),
            std::task::Poll::Ready(Ok(Some((caller, session)))) => {
                if !deadline_allows(&caller, session, clock) {
                    return std::task::Poll::Ready(Ok(None));
                }
                wake.ready.store(false, Ordering::Release);
                let mut native = Context::from_waker(&writer_waker);
                match Pin::new(&mut *writer).poll_write(&mut native, bytes) {
                    std::task::Poll::Ready(result) => std::task::Poll::Ready(Ok(Some(result))),
                    std::task::Poll::Pending => {
                        check = None;
                        waiting = true;
                        std::task::Poll::Pending
                    }
                }
            }
        }
    })
    .await?;
    match first {
        None => Ok(None),
        Some(Err(error)) => Ok(Some(Err(error))),
        Some(Ok(0)) if !bytes.is_empty() => Ok(Some(Err(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
            "native stream accepted no bytes",
        )))),
        Some(Ok(accepted)) => Ok(Some(writer.write_all(&bytes[accepted..]).await)),
    }
}
