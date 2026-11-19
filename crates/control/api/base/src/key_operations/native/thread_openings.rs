//! Share the projection only while requests for the same ordinary session overlap.
use super::{ApiResult, Ctx, HttpIdentityMode, Id, Selected, transaction_guard};
use crate::middleware::authentication::Via;
use orch_api_contract::nodes::OverviewPart;
use orch_api_contract::threads::{ThreadOpening, ThreadOpeningGetQuery};
use orch_types::Time;
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

// Only a sealed group whose shared native View check committed receives this value.
struct CheckedOpening {
    body: ThreadOpening,
    deadline: Option<Time>,
}
type ResultSlot = Option<Arc<CheckedOpening>>;

struct Flight {
    before_snapshot: Instant,
    reply: watch::Sender<ResultSlot>,
}

#[derive(Default)]
pub struct Openings(Mutex<HashMap<Arc<Key>, Flight>>);

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

impl Producer<'_> {
    /// Removing the map entry closes enrollment before the group's fresh View check starts.
    fn seal(self) -> watch::Sender<ResultSlot> {
        self.reply.clone()
    }
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

    /// Serve the declared opening with its own native source selection and Thread View gate.
    pub async fn read<F: Selected<Query = ThreadOpeningGetQuery, Reply = ThreadOpening>>(
        &self,
        ctx: Ctx,
        query: ThreadOpeningGetQuery,
    ) -> ApiResult<ThreadOpening> {
        if F::WATCH_ORDINARY || F::DECLARATION.operation != ctx.route.name {
            return super::selected_read::<F>(ctx, query, ()).await;
        }
        crate::testing::note_boundary(ctx.route.name, "opening.handler");
        let work = Key::for_request(&ctx, &query)
            .map_or(Read::Independent, |key| self.enter(key, ctx.arrived));
        match work {
            Read::Independent => super::selected_read::<F>(ctx, query, ()).await,
            Read::Produce(producer) => {
                let body = super::selected_read::<F>(ctx.clone(), query, ()).await?;
                let reply = producer.seal();
                if reply.receiver_count() != 0 {
                    // All subscribers have the producer's exact inputs and were already
                    // in this HTTP read before its selection. Check their common Thread
                    // View once more on a fresh snapshot after the last enrollment.
                    super::read(ctx.clone(), F::require).await?;
                    crate::testing::note_boundary(ctx.route.name, "opening.view_checked");
                    drop(reply.send_replace(Some(Arc::new(CheckedOpening {
                        body: body.clone(),
                        deadline: ctx.publication.retained_deadline()?,
                    }))));
                }
                Ok(body)
            }
            Read::Follow(mut receiver) => {
                {
                    let mut conn = ctx.app.db.acquire().await?;
                    let identity = transaction_guard::current_http_identity(
                        &ctx,
                        &mut conn,
                        HttpIdentityMode::Read,
                    )
                    .await?;
                    transaction_guard::guard_user(&identity.user)?;
                }
                crate::testing::note_boundary(ctx.route.name, "opening.joined");
                crate::testing::hold_boundary(ctx.route.name, "opening.joined").await;
                let shared = receiver
                    .wait_for(Option::is_some)
                    .await
                    .ok()
                    .and_then(|slot| slot.clone());
                let Some(shared) = shared else {
                    // A cancelled or unsuccessful producer did not publish a body.
                    return super::selected_read::<F>(ctx, query, ()).await;
                };
                // The private receiver carries the completed native snapshot check for
                // this participant. The registry also checks its own exact bearer before
                // headers and before yielding the body frame.
                ctx.publication.retain_deadline(shared.deadline)?;
                ctx.publication.check_deadline(&ctx)?;
                transaction_guard::checked_snapshot_read();
                let mut reply = shared.body.clone();
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
                crate::testing::note_boundary(ctx.route.name, "opening.copy_ready");
                Ok(reply)
            }
        }
    }
}
