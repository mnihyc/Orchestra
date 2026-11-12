//! A bounded replay for explicitly opted-in, effect-free database snapshot readers.
//!
//! A failed repeatable snapshot cannot be refreshed in place. Its read future must own
//! and release the failed transaction before returning. Only a typed database conflict
//! permits one fresh attempt; no RPC, native action or response bookkeeping belongs here.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use futures::future::BoxFuture;

use std::future::Future;
/// Effect-free transport gates own their snapshots without repeating request preparation.
/// As for request snapshots, only a typed conflict permits one complete fresh read.
pub(crate) async fn retry_conflict<T, F, Fut>(mut read: F) -> ApiResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ApiResult<T>>,
{
    match read().await {
        Err(error) if error.is_database_retry() => read().await,
        answer => answer,
    }
}

/// Preserve the existing typed database conflict through an engine/authz read wrapper.
/// Classification alone does not replay anything; only an explicit caller of `run` may do so.
pub fn engine_error(error: orch_engine::EngineError) -> ApiError {
    match error {
        orch_engine::EngineError::Authz(error) => {
            crate::middleware::authorization::restoration_error(error)
        }
        other => ApiError::engine(other),
    }
}

pub fn run<'a, T, F, Fut>(ctx: Ctx, read: F) -> BoxFuture<'a, ApiResult<T>>
where
    T: Send,
    F: Fn(Ctx) -> Fut + Send + 'a,
    Fut: Future<Output = ApiResult<T>> + Send + 'a,
{
    Box::pin(async move {
        let mut first = ctx.for_read_preparation();
        first.now = first.app.clock.now();
        first.clear_operation_data()?;
        if first.snapshot_read() {
            transaction_guard::reset_snapshot_read();
        }
        let first_read: BoxFuture<'a, ApiResult<T>> = Box::pin(read(first));
        match first_read.await {
            Err(error) if error.is_database_retry() => {
                let mut fresh = ctx.for_read_preparation();
                fresh.now = fresh.app.clock.now();
                fresh.clear_operation_data()?;
                if fresh.snapshot_read() {
                    transaction_guard::reset_snapshot_read();
                }
                let fresh_read: BoxFuture<'a, ApiResult<T>> = Box::pin(read(fresh));
                fresh_read.await
            }
            answer => answer,
        }
    })
}
