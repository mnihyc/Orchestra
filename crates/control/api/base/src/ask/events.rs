//! Ask keeps its own read admission and frames while sharing committed source wake hints.

use super::*;

use crate::realtime::Source;

use crate::realtime::source_changes::Changes;

use axum::response::IntoResponse;

use axum::response::Sse;

use axum::response::sse::Event;
struct Cursor {
    ctx: Ctx,
    id: Id,
    after: Seq,
    pending: Vec<(Seq, Json)>,
    rows_changed: Changes,
    identity_changed: Changes,
    epoch: tokio::sync::watch::Receiver<i64>,
}

pub(super) async fn get(
    ctx: Ctx,
    query: api::AskEventsQuery,
) -> ApiResult<axum::response::Response> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let (row, _, _) = opening(&ctx, &mut conn, &principal, ctx.id("id")?, false).await?;
    let after = query
        .after
        .or_else(|| {
            ctx.headers
                .get("last-event-id")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(row.seq);
    let rows_changed = ctx.app.realtime.source_changes(Source::Thread(row.id));
    let identity_changed = ctx
        .app
        .realtime
        .source_changes(Source::Inbox(principal.caller().user()));
    let epoch = ctx.app.realtime.inner.epoch.subscribe();
    drop(conn);
    let stream = futures::stream::unfold(
        Some(Box::new(Cursor {
            ctx,
            id: row.id,
            after,
            pending: Vec::new(),
            rows_changed,
            identity_changed,
            epoch,
        })),
        |state| async move {
            let mut cursor = state?;
            let (event, more) = Box::pin(cursor.next()).await?;
            Some((
                Ok::<_, std::convert::Infallible>(event),
                more.then_some(cursor),
            ))
        },
    );
    Ok(Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response())
}

fn event(id: Id, seq: Seq, data: api::AskEventData) -> Option<Event> {
    Event::default()
        .event("ask")
        .id(seq.to_string())
        .json_data(api::AskEvent {
            conversation_id: id,
            seq,
            data,
        })
        .ok()
}

impl Cursor {
    async fn next(&mut self) -> Option<(Event, bool)> {
        loop {
            let original_end = self.ctx.publication.retained_deadline().ok()?;
            if original_end.is_some_and(|end| end <= self.ctx.app.clock.time()) {
                return None;
            }
            if let Some((seq, data)) = self.pending.pop() {
                // A page may have waited behind a slow reader. Recheck this Ask operation for
                // each actual frame, independently of wake hints and the page's old authority.
                let mut conn = self.ctx.app.db.acquire().await.ok()?;
                let principal = self.ctx.operation_principal(&mut conn).await.ok()?;
                let (current, inspecting, days) =
                    opening(&self.ctx, &mut conn, &principal, self.id, false)
                        .await
                        .ok()?;
                drop(conn);
                let mut data: api::AskEventData = serde_json::from_value(data).ok()?;
                if let api::AskEventData::Conversation { conversation } = &mut data {
                    let current = header(&current, inspecting, days).ok()?;
                    conversation.can = current.can;
                    conversation.inspecting = inspecting;
                }
                self.after = seq;
                return Some((event(self.id, seq, data)?, true));
            }
            // Subscribe before reading, and consume existing hints before the snapshot. A commit
            // during any of these reads remains pending when the empty reader begins to wait.
            self.rows_changed.acknowledge();
            self.identity_changed.acknowledge();
            self.epoch.borrow_and_update();
            let mut conn = self.ctx.app.db.acquire().await.ok()?;
            let principal = self.ctx.operation_principal(&mut conn).await.ok()?;
            if rows::get(&mut conn, self.id, false).await.ok()?.is_none() {
                return Some((
                    event(
                        self.id,
                        self.after.saturating_add(1),
                        api::AskEventData::Deleted,
                    )?,
                    false,
                ));
            }
            opening(&self.ctx, &mut conn, &principal, self.id, false)
                .await
                .ok()?;
            let authority_end = principal.read_deadline(&mut conn).await.ok()?;
            let credential_end = self.ctx.publication.retained_deadline().ok()?;
            let end = match (authority_end, credential_end) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            self.pending = rows::events(&mut conn, self.id, self.after, 200)
                .await
                .ok()?;
            self.pending.reverse();
            drop(conn);
            drop(principal);
            #[cfg(any(test, feature = "test-support"))]
            reads::loaded(self.id);
            if self.pending.is_empty() {
                self.wait(end).await?;
            }
        }
    }

    async fn wait(&mut self, end: Option<Time>) -> Option<()> {
        let clock = Arc::clone(&self.ctx.app.clock);
        let stop = self.ctx.app.realtime.inner.stop.clone();
        let expired = async move {
            match end {
                Some(end) => {
                    let remaining = (end.get() - clock.now()).to_std().unwrap_or_default();
                    tokio::time::sleep(remaining).await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            changed = self.rows_changed.changed() => changed.ok()?,
            changed = self.identity_changed.changed() => changed.ok()?,
            changed = self.epoch.changed() => changed.ok()?,
            () = expired => {},
            () = stop.cancelled() => return None,
        }
        Some(())
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod reads {

    use orch_types::Id;

    use std::collections::HashMap;

    use std::sync::atomic::AtomicUsize;

    use std::sync::atomic::Ordering;

    use std::sync::Arc;

    use std::sync::Mutex;

    use std::sync::OnceLock;

    use tokio::sync::Notify;
    #[derive(Default)]
    struct Count {
        reads: AtomicUsize,
        changed: Notify,
    }

    fn probes() -> &'static Mutex<HashMap<Id, Arc<Count>>> {
        static PROBES: OnceLock<Mutex<HashMap<Id, Arc<Count>>>> = OnceLock::new();
        PROBES.get_or_init(Mutex::default)
    }

    pub struct Probe {
        id: Id,
        count: Arc<Count>,
    }

    impl Probe {
        pub(crate) fn new(id: Id) -> Self {
            let count = Arc::new(Count::default());
            probes()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(id, Arc::clone(&count));
            Self { id, count }
        }

        pub(crate) fn reads(&self) -> usize {
            self.count.reads.load(Ordering::SeqCst)
        }

        pub(crate) async fn wait_for(&self, count: usize) {
            loop {
                let changed = self.count.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.reads() >= count {
                    return;
                }
                changed.await;
            }
        }
    }

    pub(super) fn loaded(id: Id) {
        let count = probes()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .cloned();
        if let Some(count) = count {
            count.reads.fetch_add(1, Ordering::SeqCst);
            count.changed.notify_waiters();
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            probes()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.id);
        }
    }
}
