//! Share the projection only while requests for the same ordinary session overlap.
use super::{
    ApiResult, Ctx, Id, Opening, ThreadOpening, ThreadOpeningGetQuery, native, require_view,
};
use crate::middleware::authentication::Via;
use orch_api_contract::nodes::OverviewPart;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;
use tokio::sync::watch;

#[derive(Hash, PartialEq, Eq)]
struct Key {
    app: usize,
    user: Id,
    session: Id,
    thread: Id,
    continuation: Option<Id>,
    revision: Option<i64>,
    limit: Option<u32>,
    headers: blake3::Hash,
    peer: Option<std::net::IpAddr>,
}

impl Key {
    fn for_request(ctx: &Ctx, query: &ThreadOpeningGetQuery) -> Option<Self> {
        if !ctx.ordinary_browser_opening() {
            return None;
        }
        let user = ctx.user().ok()?;
        let Via::Session { session_id } = user.via else {
            return None;
        };
        // Include the exact access token and all header inputs without retaining their values.
        let mut hash = blake3::Hasher::new();
        for (name, value) in &ctx.headers {
            for part in [name.as_str().as_bytes(), value.as_bytes()] {
                hash.update(&part.len().to_le_bytes());
                hash.update(part);
            }
        }
        Some(Self {
            app: Arc::as_ptr(&ctx.app) as usize,
            user: user.id(),
            session: session_id,
            thread: ctx.id("id").ok()?,
            continuation: query.continuation_id,
            revision: query.projection_revision,
            limit: query.limit,
            headers: hash.finalize(),
            peer: ctx.remote,
        })
    }
}

type ResultSlot = Option<Arc<ThreadOpening>>;

struct Flight {
    before_snapshot: Instant,
    reply: watch::Sender<ResultSlot>,
}

#[derive(Default)]
pub(super) struct Openings(Mutex<HashMap<Arc<Key>, Flight>>);

enum Read<'a> {
    Produce(Producer<'a>),
    Follow(watch::Receiver<ResultSlot>),
    Independent,
}

struct Producer<'a> {
    key: Arc<Key>,
    openings: &'a Openings,
    reply: watch::Sender<ResultSlot>,
}

impl Drop for Producer<'_> {
    fn drop(&mut self) {
        self.openings
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

impl Openings {
    fn enter(&self, key: Key, arrived: Instant) -> Read<'_> {
        let mut current = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(work) = current.get(&key) {
            // An HTTP request already in authentication when the producer started
            // can observe that snapshot. Later requests need their own current read.
            return if arrived < work.before_snapshot {
                Read::Follow(work.reply.subscribe())
            } else {
                Read::Independent
            };
        }
        let key = Arc::new(key);
        let (reply, _) = watch::channel(None);
        current.insert(
            key.clone(),
            Flight {
                before_snapshot: Instant::now(),
                reply: reply.clone(),
            },
        );
        Read::Produce(Producer {
            key,
            openings: self,
            reply,
        })
    }

    pub(super) async fn read(
        &self,
        ctx: Ctx,
        query: ThreadOpeningGetQuery,
    ) -> ApiResult<ThreadOpening> {
        let work = Key::for_request(&ctx, &query)
            .map_or(Read::Independent, |key| self.enter(key, ctx.arrived));
        match work {
            Read::Independent => native::selected_read::<Opening>(ctx, query, ()).await,
            Read::Produce(producer) => {
                let reply = native::selected_read::<Opening>(ctx, query, ()).await?;
                if producer.reply.receiver_count() != 0 {
                    drop(producer.reply.send_replace(Some(Arc::new(reply.clone()))));
                }
                Ok(reply)
            }
            Read::Follow(mut receiver) => {
                // Each follower authenticates and checks View in its own native
                // snapshot. Its database connection is returned before it waits.
                native::read(ctx.clone(), |ctx, conn, principal| {
                    Box::pin(require_view(ctx, conn, principal))
                })
                .await?;
                let shared = receiver
                    .wait_for(Option::is_some)
                    .await
                    .ok()
                    .and_then(|slot| slot.clone());
                let Some(shared) = shared else {
                    // A cancelled or unsuccessful producer did not publish a body.
                    return native::selected_read::<Opening>(ctx, query, ()).await;
                };
                let mut reply = (*shared).clone();
                fn correlate<T>(part: &mut OverviewPart<T>, id: &str) {
                    if let OverviewPart::Failed { problem, .. } = part {
                        id.clone_into(&mut problem.request_id);
                    }
                }
                correlate(&mut reply.screen, &ctx.request_id);
                if let Some(part) = &mut reply.gateway {
                    correlate(part, &ctx.request_id);
                }
                if let Some(part) = &mut reply.pending_agent_schedules {
                    correlate(part, &ctx.request_id);
                }
                Ok(reply)
            }
        }
    }
}
