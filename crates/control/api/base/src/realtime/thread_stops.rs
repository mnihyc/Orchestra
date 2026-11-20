//! Accepted Stop observations reach each authorized thread reader without moving its history cursor.

use super::subscriptions::{Subscription, viewing};
use super::{Fail, Inner, Source};
use orch_realtime_contract::feeds::{EventData, Feed};
use orch_realtime_contract::frames::{EventFrame, ServerFrame};
use orch_types::Id;
use std::sync::Arc;

pub(crate) fn changed(inner: &Arc<Inner>, thread: Id) {
    let subscribers = inner.subscribers_of(Source::Thread(thread));
    if subscribers.is_empty() {
        return;
    }
    let inner = Arc::clone(inner);
    inner.clone().tasks.spawn(async move {
        for sub in subscribers {
            let result = viewing(sub.connection(), deliver(&inner, &sub, thread)).await;
            if let Err(error) = result {
                tracing::warn!(%error, %thread, "accepted Stop state awaits its next live projection");
            }
        }
    });
}

async fn deliver(inner: &Arc<Inner>, sub: &Arc<Subscription>, thread: Id) -> Result<(), Fail> {
    let Some(()) = super::watches::delivery_guard(inner, sub.connection()).await? else {
        return Ok(());
    };
    let mut state = sub.state.lock().await;
    loop {
        if inner.stop.is_cancelled() || !sub.is_live() || state.thread_reset {
            return Ok(());
        }
        let epoch = super::sync::read_epoch(inner).await?;
        if !inner.recheck(sub, &mut state, epoch).await? || state.thread_reset {
            return Ok(());
        }
        let cut = (
            sub.decided(),
            sub.connection().decision_identity(),
            sub.connection().as_admin(),
        );
        let Some(stopping) = inner
            .engine()?
            .thread_stopping(thread)
            .await
            .map_err(Fail::other)?
        else {
            return Ok(());
        };
        let latest = super::sync::read_epoch(inner).await?;
        if !inner.recheck(sub, &mut state, latest).await? || state.thread_reset {
            return Ok(());
        }
        if latest != epoch
            || sub.decided() != cut.0
            || sub.connection().decision_identity() != cut.1
            || sub.connection().as_admin() != cut.2
        {
            continue;
        }
        {
            let mut registry = inner.registry();
            if stopping.pending {
                registry.stop_pending.insert(thread);
            } else {
                registry.stop_pending.remove(&thread);
            }
        }
        let observed = (stopping.version, stopping.thread_seq);
        if state.thread_stop.is_some_and(|held| held >= observed) {
            return Ok(());
        }
        let feed = Feed::Thread(thread);
        let frame = EventFrame::new(
            sub.tag().id,
            feed.clone(),
            &EventData::ThreadStopping(stopping),
        )
        .map_err(Fail::other)?;
        let text = ServerFrame::Event(frame).encode().map_err(Fail::other)?;
        sub.connection().outbox.push_event(sub.tag(), &feed, text);
        state.thread_stop = Some(observed);
        return Ok(());
    }
}
