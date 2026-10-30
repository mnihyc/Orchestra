//! One distributed reader of existing native sessions, projected under each recipient's current rights.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use orch_engine::background::Capture;
use orch_realtime_contract::feeds::Feed;
use orch_types::{Id, Json};
use tokio::time::Instant;

use super::ephemeral::{Message, broadcast};
use super::server::Connection;
use super::{Fail, Inner};

/// Sample while watched every five seconds, with one-second change coalescing.
const SAMPLE_GAP: Duration = Duration::from_secs(5);
/// Source sequences and native presence are rechecked at most once per second.
const CHANGE_GAP: Duration = Duration::from_secs(1);

struct Watch {
    sampled: Instant,
    sequence: i64,
    running: Arc<AtomicBool>,
}

/// Owned only by the existing elected ephemeral coordinator, including its departure grace.
#[derive(Default)]
pub(crate) struct Sources {
    watches: HashMap<Id, Watch>,
    looked: Option<Instant>,
}

impl Sources {
    /// Coalesces every API process's readers into one bounded native read.
    pub(crate) async fn reconcile(
        &mut self,
        inner: &Arc<Inner>,
        active: &HashSet<Feed>,
    ) -> Result<(), Fail> {
        if self.looked.is_some_and(|at| at.elapsed() < CHANGE_GAP) {
            return Ok(());
        }
        self.looked = Some(Instant::now());
        let wanted: HashSet<_> = active
            .iter()
            .filter_map(|feed| match feed {
                Feed::ThreadBackground(thread) => Some(*thread),
                _ => None,
            })
            .collect();
        self.watches.retain(|thread, _| wanted.contains(thread));
        let mut conn = inner.db.acquire().await?;
        for thread in wanted {
            let sequence = orch_db::sequences::current(&mut conn, thread)
                .await?
                .unwrap_or_default();
            let watch = self.watches.entry(thread).or_insert_with(|| Watch {
                sampled: Instant::now() - SAMPLE_GAP,
                sequence: -1,
                running: Arc::new(AtomicBool::new(false)),
            });
            if (watch.sequence == sequence && watch.sampled.elapsed() < SAMPLE_GAP)
                || watch.running.swap(true, Ordering::SeqCst)
            {
                continue;
            }
            watch.sequence = sequence;
            watch.sampled = Instant::now();
            let running = Arc::clone(&watch.running);
            let engine = inner.engine()?.clone();
            let inner = Arc::clone(inner);
            inner.tasks.clone().spawn(async move {
                // Native I/O runs outside the actor, so a slow machine cannot delay a permission closure.
                let result = async {
                    let mut capture = engine
                        .capture_background(thread)
                        .await
                        .map_err(Fail::other)?;
                    // The feed carries the first ordinary page and the complete native count. The list
                    // dialog transfers every page through the caller-bound HTTP review.
                    const FIRST_PAGE: usize = 50;
                    let mut remaining = FIRST_PAGE;
                    for (_, native) in &mut capture.sessions {
                        native.items.truncate(remaining);
                        remaining = remaining.saturating_sub(native.items.len());
                    }
                    while serde_json::to_vec(&capture).map_err(Fail::other)?.len()
                        > orch_link_contract::background::PAGE_BYTES
                    {
                        let Some((_, native)) = capture
                            .sessions
                            .iter_mut()
                            .rev()
                            .find(|(_, native)| !native.items.is_empty())
                        else {
                            return Err(Fail::other(
                                "the native source metadata exceeds the reply bound",
                            ));
                        };
                        native.items.pop();
                    }
                    let data = serde_json::to_value(capture).map_err(Fail::other)?;
                    broadcast(
                        &inner.db,
                        &Message::List {
                            feed: Feed::ThreadBackground(thread),
                            data,
                        },
                    )
                    .await
                }
                .await;
                running.store(false, Ordering::SeqCst);
                if let Err(error) = result {
                    tracing::warn!(%error,%thread,"a live native roster could not be sampled");
                }
            });
        }
        Ok(())
    }
}

/// Applies the source's current masking and directory rules at frame delivery, with no stored roster.
pub(crate) async fn project(
    inner: &Arc<Inner>,
    connection: &Connection,
    thread: Id,
    data: &Json,
) -> Result<Json, Fail> {
    let capture: Capture = serde_json::from_value(data.clone()).map_err(Fail::other)?;
    let mut conn = inner.db.acquire().await?;
    let principal = super::subscriptions::principal_for_connection(inner, &mut conn, connection)
        .await?
        .ok_or_else(|| Fail::other("the reader no longer exists"))?;
    let (mut roster, _) = inner
        .engine()?
        .project_background(&mut conn, &principal, thread, &capture)
        .await
        .map_err(Fail::other)?;
    // The count belongs to the complete native observation; a realtime frame is one bounded page.
    while serde_json::to_vec(&roster).map_err(Fail::other)?.len()
        > orch_link_contract::background::PAGE_BYTES
    {
        if roster.items.pop().is_none() {
            return Err(Fail::other(
                "the live roster metadata exceeds its frame bound",
            ));
        }
    }
    serde_json::to_value(roster).map_err(Fail::other)
}
