//! Command lifetime and bounded collection shared by native browser file readers.

use super::{Relay, RelayError};
use crate::dispatch::Dispatcher;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use bytes::{Bytes, BytesMut};
use orch_api_contract::common::ErrorCode;
use orch_blob_contract::range::BlobMeta;
use orch_link_contract::commands::{CommandBody, SerialKey};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

/// Resolved operational defaults, never a file-size or admission refusal.
#[derive(Clone, Copy)]
pub struct Options {
    pub concurrency: usize,
    pub idle_secs: u64,
    pub retry_ms: u64,
    pub until: Time,
}
impl Options {
    pub async fn read(ctx: &Ctx) -> ApiResult<Self> {
        let mut conn = ctx.app.db.acquire().await?;
        let names = [
            keys::STORAGE_RELAY_CONCURRENCY,
            keys::STORAGE_RELAY_IDLE_SECS,
            keys::STORAGE_RELAY_RETRY_MS,
            keys::JOB_IO_TIMEOUT_SECS,
        ];
        let definitions = names
            .iter()
            .map(|key| {
                orch_settings_contract::registry::get(key)
                    .ok_or_else(|| ApiError::internal("a relay setting has no definition"))
            })
            .collect::<ApiResult<Vec<_>>>()?;
        let snapshot = orch_settings::Snapshot::load(
            &mut conn,
            Place::System,
            orch_settings::Wanted::Keys(&definitions),
        )
        .await
        .map_err(ApiError::internal)?;
        let get = |key| -> ApiResult<u64> {
            let definition = orch_settings_contract::registry::get(key)
                .ok_or_else(|| ApiError::internal("a relay setting has no definition"))?;
            snapshot
                .resolve(definition)
                .value
                .and_then(|v| v.as_u64())
                .ok_or_else(|| ApiError::internal("a relay setting has no integer default"))
        };
        let until = i64::try_from(get(keys::JOB_IO_TIMEOUT_SECS)?)
            .ok()
            .and_then(chrono::Duration::try_seconds)
            .and_then(|wait| ctx.app.clock.now().checked_add_signed(wait))
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC);
        Ok(Self {
            concurrency: usize::try_from(get(keys::STORAGE_RELAY_CONCURRENCY)?)
                .unwrap_or(usize::MAX),
            idle_secs: get(keys::STORAGE_RELAY_IDLE_SECS)?,
            retry_ms: get(keys::STORAGE_RELAY_RETRY_MS)?,
            until: Time::new(until),
        })
    }
}

/// Owns the native command until the actual browser response/collection is dropped.
pub struct Read {
    pub relay: Arc<Relay>,
    pub options: Options,
    command: Id,
    dispatcher: Dispatcher,
    failure: Arc<Mutex<Option<ApiError>>>,
    size: u64,
}
impl Read {
    pub async fn create(ctx: &Ctx, size: i64) -> ApiResult<Self> {
        let size = u64::try_from(size).map_err(ApiError::internal)?;
        let options = Options::read(ctx).await?;
        let relay = ctx
            .app
            .blobs
            .relays()
            .create(options.concurrency, size)
            .await;
        Ok(Self {
            relay,
            options,
            command: orch_types::new_id(),
            dispatcher: ctx.app.link.dispatcher(),
            failure: Arc::new(Mutex::new(None)),
            size,
        })
    }
    pub fn command_id(&self) -> Id {
        self.command
    }

    /// Domain admission has just checked its exact current source after queueing.
    pub async fn issue<
        B: CommandBody<Result = orch_link_contract::commands::Empty> + Send + Sync + 'static,
    >(
        &self,
        machine: Id,
        body: &B,
    ) -> ApiResult<()> {
        self.dispatcher
            .issue_as(
                self.command,
                machine,
                &SerialKey::None,
                self.options.until,
                body,
            )
            .await
            .map_err(ApiError::internal)?;
        let (dispatcher, id, relay, failure, size) = (
            self.dispatcher.clone(),
            self.command,
            Arc::clone(&self.relay),
            Arc::clone(&self.failure),
            self.size,
        );
        tokio::spawn(async move {
            let outcome = dispatcher
                .wait_for::<B>(id)
                .await
                .map(|_| ())
                .map_err(|error| {
                    crate::routes::workspace::live::command_failure(error)
                        .problem(crate::routes::workspace::problems::Doing::Reading)
                });
            complete(&relay, &failure, size, outcome);
        });
        Ok(())
    }
    /// The caller supplies its already checked domain source; no blob-store authority is granted.
    pub async fn run<B: CommandBody + Sync>(&self, machine: Id, body: &B) -> ApiResult<B::Result> {
        self.dispatcher
            .issue_as(
                self.command,
                machine,
                &SerialKey::None,
                self.options.until,
                body,
            )
            .await
            .map_err(ApiError::internal)?;
        self.dispatcher
            .wait_for::<B>(self.command)
            .await
            .map_err(|error| {
                crate::routes::workspace::live::command_failure(error)
                    .problem(crate::routes::workspace::problems::Doing::Reading)
            })
    }
    pub async fn next(&self) -> ApiResult<Option<Bytes>> {
        self.relay.next().await.map_err(|error| self.problem(error))
    }
    fn problem(&self, _error: RelayError) -> ApiError {
        self.failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| {
                ApiError::new(
                    ErrorCode::EnvironmentUnavailable,
                    "the file relay ended before its native source completed",
                )
            })
    }
    /// The domain's own authorized producer is polled concurrently with bounded collection.
    pub async fn collect(&self, produce: impl Future<Output = ApiResult<()>>) -> ApiResult<Bytes> {
        let producer = async {
            let outcome = produce.await;
            complete(&self.relay, &self.failure, self.size, outcome.clone());
            outcome
        };
        let (produced, collected) = tokio::join!(producer, self.bytes());
        produced?;
        collected
    }
    async fn bytes(&self) -> ApiResult<Bytes> {
        let mut bytes = BytesMut::new();
        while let Some(piece) = self.next().await? {
            bytes.extend_from_slice(&piece);
        }
        Ok(bytes.freeze())
    }

    /// A decoder knows the real entry size only after opening it. It remains within this
    /// transfer's selected source bound and reports the actual size/hash before any public reply.
    pub async fn collect_bounded(
        &self,
        produce: impl Future<Output = ApiResult<BlobMeta>>,
    ) -> ApiResult<Bytes> {
        let producer = async {
            let outcome = produce.await.and_then(|meta| {
                if u64::try_from(meta.size).is_ok_and(|size| size <= self.size) {
                    Ok(meta)
                } else {
                    Err(ApiError::new(
                        ErrorCode::RequestTooLarge,
                        "the decoded entry exceeds its selected preview bound",
                    ))
                }
            });
            let (size, status) = match &outcome {
                Ok(meta) => (u64::try_from(meta.size).unwrap_or_default(), Ok(())),
                Err(error) => (self.size, Err(error.clone())),
            };
            complete(&self.relay, &self.failure, size, status);
            outcome
        };
        let (produced, bytes) = tokio::join!(producer, self.bytes());
        let meta = produced?;
        let bytes = bytes?;
        if usize::try_from(meta.size).ok() != Some(bytes.len())
            || orch_types::Hash::of(&bytes) != meta.hash
        {
            return Err(ApiError::internal(
                "the native decoder's content did not match its verified result",
            ));
        }
        Ok(bytes)
    }
}
fn complete(relay: &Relay, failure: &Mutex<Option<ApiError>>, size: u64, outcome: ApiResult<()>) {
    let outcome = outcome.and_then(|()| {
        if relay.received() == size {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::EnvironmentUnavailable,
                "the native source ended before sending its complete range",
            ))
        }
    });
    match outcome {
        Ok(()) => relay.finish(),
        Err(error) => {
            *failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
            relay.cancel();
        }
    }
}
impl Drop for Read {
    fn drop(&mut self) {
        self.relay.cancel();
        let (dispatcher, id) = (self.dispatcher.clone(), self.command);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = dispatcher.cancel(id).await;
            });
        }
    }
}
