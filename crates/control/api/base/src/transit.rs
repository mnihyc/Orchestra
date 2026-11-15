//! Browser-backed command transfers keep one durable release fence and physical custody.
//! Process-local liveness follows the same single-API deployment as the memory rendezvous.

use crate::{blobs::Blobs, dispatch::Dispatcher, middleware::requests::ApiResult};
use axum::{body::Body, response::Response};
use bytes::Bytes;
use hyper::body::{Body as HttpBody, Frame, SizeHint};
use orch_db::{Db, DbError, queries::storage};
use orch_types::{Clock, Id, Time};
use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError, Weak},
    task::{Context, Poll},
};

pub struct Transfers {
    process: Id,
    active: Mutex<HashMap<Id, Weak<Held>>>,
}
impl Default for Transfers {
    fn default() -> Self {
        Self {
            process: orch_types::new_id(),
            active: Mutex::new(HashMap::new()),
        }
    }
}
impl Transfers {
    fn alive(&self, id: Id) -> bool {
        self.active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .is_some_and(|token| token.strong_count() > 0)
    }
}

#[derive(Clone)]
pub struct CommandTransfer(Arc<Held>);
struct Held {
    blobs: Blobs,
    db: Db,
    clock: Arc<dyn Clock>,
    dispatcher: Dispatcher,
    id: Id,
    until: Time,
}
impl Drop for Held {
    fn drop(&mut self) {
        let (blobs, db, clock, dispatcher, id, until) = (
            self.blobs.clone(),
            self.db.clone(),
            Arc::clone(&self.clock),
            self.dispatcher.clone(),
            self.id,
            self.until,
        );
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = dispatcher.cancel(id).await;
                if let Err(error) = release(&blobs, &db, id, until, clock.now()).await {
                    tracing::warn!(%id,%error,"a command transfer remains held for cleanup");
                }
            });
        }
    }
}
async fn release(
    blobs: &Blobs,
    db: &Db,
    id: Id,
    until: Time,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError> {
    // The marker is closed before blob rows are acquired, excluding both late pinners and lock inversion.
    {
        let mut conn = db.acquire().await?;
        storage::close_command_transfer(&mut conn, id, blobs.transfers().process, at, until.get())
            .await?;
    }
    let hashes = {
        let mut tx = db.begin().await?;
        let hashes = storage::release_command_transfer(&mut tx, id, at).await?;
        tx.commit().await.map_err(DbError::from)?;
        hashes
    };
    blobs.forget_after_transfer(db, &hashes, at).await;
    let mut conn = db.acquire().await?;
    storage::forget_command_transfer(&mut conn, id, at).await?;
    Ok(())
}
impl Blobs {
    pub async fn command_transfer(
        &self,
        db: Db,
        clock: Arc<dyn Clock>,
        dispatcher: Dispatcher,
        id: Id,
        until: Time,
    ) -> ApiResult<CommandTransfer> {
        let held = {
            let mut active = self
                .transfers()
                .active
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            active.retain(|_, held| held.strong_count() > 0);
            match active.get(&id).and_then(Weak::upgrade) {
                Some(held) => CommandTransfer(held),
                None => {
                    let held = Arc::new(Held {
                        blobs: self.clone(),
                        db,
                        clock,
                        dispatcher,
                        id,
                        until,
                    });
                    active.insert(id, Arc::downgrade(&held));
                    CommandTransfer(held)
                }
            }
        };
        {
            let mut conn = held.0.db.acquire().await?;
            storage::begin_command_transfer(
                &mut conn,
                id,
                self.transfers().process,
                held.0.clock.now(),
                until.get(),
            )
            .await?;
        }
        Ok(held)
    }
    pub async fn reap_command_transfers(
        &self,
        db: &Db,
        clock: &dyn Clock,
        dispatcher: &Dispatcher,
    ) -> Result<(), DbError> {
        let mut after = None;
        loop {
            let rows = {
                let mut conn = db.acquire().await?;
                storage::command_transfer_page(&mut conn, after, storage::PIN_BATCH as i64).await?
            };
            if rows.is_empty() {
                break;
            }
            // Select rows first: a new guard is registered before its durable marker can appear.
            for (id, owner, released) in &rows {
                if *released || *owner != self.transfers().process || !self.transfers().alive(*id) {
                    let _ = dispatcher.cancel(*id).await;
                    let until = {
                        let mut conn = db.acquire().await?;
                        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
                            "SELECT expires_at FROM command_transfers WHERE command_id=$1",
                        )
                        .bind(id)
                        .fetch_optional(&mut *conn)
                        .await?
                        .unwrap_or_else(|| clock.now())
                    };
                    release(self, db, *id, Time::new(until), clock.now()).await?;
                }
            }
            after = rows.last().map(|row| row.0);
        }
        Ok(())
    }
}
impl CommandTransfer {
    pub fn response(self, response: Response) -> Response {
        let (parts, body) = response.into_parts();
        Response::from_parts(
            parts,
            Body::new(HeldBody {
                body: Box::pin(body),
                held: Some(self),
            }),
        )
    }
}
struct HeldBody {
    body: Pin<Box<Body>>,
    held: Option<CommandTransfer>,
}
impl HttpBody for HeldBody {
    type Data = Bytes;
    type Error = axum::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        match self.body.as_mut().poll_frame(cx) {
            Poll::Ready(None) => {
                self.held.take();
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                self.held.take();
                Poll::Ready(Some(Err(error)))
            }
            other => other,
        }
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
