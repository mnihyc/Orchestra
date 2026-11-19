//! A bounded replay for explicitly opted-in, effect-free database snapshot readers.
//!
//! A failed repeatable snapshot cannot be refreshed in place. Its read future must own
//! and release the failed transaction before returning. Only a typed database conflict
//! permits one fresh attempt; no RPC, native action or response bookkeeping belongs here.

use crate::middleware::{
    requests::{ApiError, ApiResult},
    transaction_guard,
};
use crate::registry::Ctx;
use std::future::Future;

/// Preserve the existing typed database conflict through an engine/authz read wrapper.
/// Classification alone does not replay anything; only an explicit caller of `run` may do so.
pub(crate) fn engine_error(error: orch_engine::EngineError) -> ApiError {
    match error {
        orch_engine::EngineError::Authz(error) => {
            crate::middleware::authorization::restoration_error(error)
        }
        other => ApiError::engine(other),
    }
}

pub(crate) async fn run<T, F, Fut>(ctx: Ctx, read: F) -> ApiResult<T>
where
    T: Send,
    F: Fn(Ctx) -> Fut + Send,
    Fut: Future<Output = ApiResult<T>> + Send,
{
    let mut first = ctx.for_read_preparation();
    first.now = first.app.clock.now();
    first.clear_operation_data()?;
    if first.snapshot_read() {
        transaction_guard::reset_snapshot_read();
    }
    match read(first).await {
        Err(error) if error.is_database_retry() => {
            let mut fresh = ctx.for_read_preparation();
            fresh.now = fresh.app.clock.now();
            fresh.clear_operation_data()?;
            if fresh.snapshot_read() {
                transaction_guard::reset_snapshot_read();
            }
            read(fresh).await
        }
        answer => answer,
    }
}
