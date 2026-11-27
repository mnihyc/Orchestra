//! Final existing queue/socket acceptance after capacity waits; no serialized authority/lease.

use super::ticket_origin::CurrentCaller;

use super::ticket_origin::deadline_allows;

use crate::middleware::requests::ApiResult;

use axum::extract::ws::Message;

use futures::Sink;

use futures::SinkExt;

use orch_types::Clock;

use std::future::Future;

use std::future::poll_fn;

use std::pin::Pin;

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

/// Native capacity, separate from a wake hint or an authorization result. Returning
/// Ready must leave room for this sole writer until it writes, or the original stream ends.
pub(super) trait ReadyWrite: tokio::io::AsyncWrite + Unpin {
    fn poll_write_ready(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>>;
}
impl ReadyWrite for orch_link::tunnels::channel::TunnelWriter {
    fn poll_write_ready(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        orch_link::tunnels::channel::TunnelWriter::poll_write_ready(self.get_mut(), cx)
    }
}

/// Keep the existing permission epoch and keeper cadence live while no byte has
/// started. Positive checks during this wait are discarded, never saved for writing.
pub(super) struct WriteWatch {
    epoch: tokio::sync::watch::Receiver<i64>,
    tick: tokio::time::Interval,
}
impl WriteWatch {
    pub(super) fn new(
        epoch: tokio::sync::watch::Receiver<i64>,
        period: std::time::Duration,
    ) -> Self {
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self { epoch, tick }
    }
    async fn changed(&mut self) -> bool {
        tokio::select! {
            changed = self.epoch.changed() => changed.is_ok(),
            _ = self.tick.tick() => true,
        }
    }
}

/// A single existing parser message. Capacity comes before current authorization.
/// Once actual bytes of this exact message enter the native buffer, completing only its
/// remaining bytes preserves protocol framing; the next message obtains its own current check.
pub(super) async fn write_message<W, Make, F>(
    writer: &mut W,
    bytes: &[u8],
    clock: &dyn Clock,
    changes: &mut WriteWatch,
    mut prepare: Make,
) -> ApiResult<Option<std::io::Result<()>>>
where
    W: ReadyWrite,
    Make: FnMut() -> F,
    F: Future<Output = ApiResult<Option<CurrentCaller>>>,
{
    use tokio::io::AsyncWriteExt;
    loop {
        let ready = tokio::select! {
            ready = poll_fn(|cx| Pin::new(&mut *writer).poll_write_ready(cx)) => ready,
            live = changes.changed() => {
                if !live { return Ok(None); }
                let Some((caller, session)) = prepare().await? else { return Ok(None); };
                if !deadline_allows(&caller, session, clock) { return Ok(None); }
                continue;
            }
        };
        if let Err(error) = ready {
            return Ok(Some(Err(error)));
        }
        let Some((caller, session)) = prepare().await? else {
            return Ok(None);
        };
        if !deadline_allows(&caller, session, clock) {
            return Ok(None);
        }
        // Poll the native first byte once. An unexpected Pending owns no bytes and
        // returns to actual readiness plus fresh authorization, never a cached caller.
        let first =
            poll_fn(|cx| std::task::Poll::Ready(Pin::new(&mut *writer).poll_write(cx, bytes)))
                .await;
        match first {
            std::task::Poll::Pending => continue,
            std::task::Poll::Ready(Err(error)) => return Ok(Some(Err(error))),
            std::task::Poll::Ready(Ok(0)) if !bytes.is_empty() => {
                return Ok(Some(Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "native stream accepted no bytes",
                ))));
            }
            std::task::Poll::Ready(Ok(accepted)) => {
                return Ok(Some(writer.write_all(&bytes[accepted..]).await));
            }
        }
    }
}
