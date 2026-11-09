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
            #[cfg(test)]
            if super::background_read_tests::manual_watch(thread) {
                continue;
            }
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

/// A current body, a changed transport origin, or the ordinary native read closure.
pub(crate) enum Projection {
    Current(Json),
    Changed,
    Closed(orch_realtime_contract::frames::FeedClosedReason),
}

fn projection_failure(error: orch_engine::background::BackgroundError) -> Result<Projection, Fail> {
    use orch_engine::background::BackgroundError;
    use orch_realtime_contract::frames::FeedClosedReason;
    match error {
        BackgroundError::NotFound
        | BackgroundError::Internal(orch_engine::EngineError::Authz(
            orch_authz::Error::NotFound(_),
        )) => Ok(Projection::Closed(FeedClosedReason::Deleted)),
        BackgroundError::Scope
        | BackgroundError::Denied(_)
        | BackgroundError::Internal(orch_engine::EngineError::Authz(
            orch_authz::Error::Suspended { .. }
            | orch_authz::Error::Archived { .. }
            | orch_authz::Error::NoSuchUser(_),
        )) => Ok(Projection::Closed(FeedClosedReason::Revoked)),
        other => Err(Fail::other(other)),
    }
}

/// Applies the source's current masking and directory rules at frame delivery, with no stored roster.
pub(crate) async fn project(
    inner: &Arc<Inner>,
    connection: &Connection,
    thread: Id,
    data: &Json,
) -> Result<Projection, Fail> {
    let origin = connection.credential();
    let Some(accepted) =
        super::key_reads::admit(inner, connection, &Feed::ThreadBackground(thread)).await?
    else {
        return Ok(Projection::Closed(
            orch_realtime_contract::frames::FeedClosedReason::Revoked,
        ));
    };
    let capture: Capture = serde_json::from_value(data.clone()).map_err(Fail::other)?;
    let app = inner
        .app()
        .ok_or_else(|| Fail::other("the realtime application is unavailable"))?;
    let mut conn = inner.db.acquire().await?;
    let prepared = match inner
        .engine()?
        .prepare_background_projection(&mut conn, thread, &capture)
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            let identity = super::server::current_identity(&app, &mut conn, &origin)
                .await
                .map_err(Fail::other)?;
            identity.check_deadline(&app).map_err(Fail::other)?;
            return projection_failure(error);
        }
    };
    #[cfg(test)]
    super::background_read_tests::at(thread, super::background_read_tests::Cut::SocketPrepared)
        .await;
    loop {
        if !connection.still_current(&origin) {
            return Ok(Projection::Changed);
        }
        let identity = super::server::current_identity(&app, &mut conn, &origin)
            .await
            .map_err(Fail::other)?;
        let reader = match crate::key_operations::background_read::prepare(
            &app,
            &mut conn,
            identity,
            Some(&accepted),
            thread,
            &capture,
        )
        .await
        {
            Ok(reader) => reader,
            Err(error) if error.code() == orch_api_contract::common::ErrorCode::AuthzDenied => {
                return Ok(Projection::Closed(
                    orch_realtime_contract::frames::FeedClosedReason::Revoked,
                ));
            }
            Err(error) => return Err(Fail::other(error)),
        };
        let projected = inner
            .engine()?
            .render_background_projection(
                &mut conn,
                &reader.principal,
                &prepared,
                &reader.directories,
            )
            .await;
        #[cfg(test)]
        super::background_read_tests::at(
            thread,
            super::background_read_tests::Cut::SocketProjected,
        )
        .await;
        let identity = super::server::current_identity(&app, &mut conn, &origin)
            .await
            .map_err(Fail::other)?;
        let current = match crate::key_operations::background_read::prepare(
            &app,
            &mut conn,
            identity,
            Some(&accepted),
            thread,
            &capture,
        )
        .await
        {
            Ok(reader) => reader,
            Err(error) if error.code() == orch_api_contract::common::ErrorCode::AuthzDenied => {
                return Ok(Projection::Closed(
                    orch_realtime_contract::frames::FeedClosedReason::Revoked,
                ));
            }
            Err(error) => return Err(Fail::other(error)),
        };
        current.identity.check_deadline(&app).map_err(Fail::other)?;
        if !connection.still_current(&origin) {
            return Ok(Projection::Changed);
        }
        if !reader.live(&app) || !current.live(&app) || !reader.same_context(&current) {
            continue;
        }
        let (mut roster, _) = match projected {
            Ok(projected) => projected,
            Err(error) => return projection_failure(error),
        };
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
        return serde_json::to_value(roster)
            .map(Projection::Current)
            .map_err(Fail::other);
    }
}
