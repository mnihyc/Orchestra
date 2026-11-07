//! Optional response content after a writer and its owned cleanup have completed.

use super::resolver::Disclosure;
use crate::middleware::requests::ApiResult;
use crate::registry::Ctx;
use std::future::Future;

/// Await the whole response projection, including its snapshot open and commit.
/// A trusted database conflict cannot replay the already completed effect through
/// Registry's refused-request retry; it only makes this reply's content unavailable.
/// Domain-specific read-loss handling stays with the caller, and all other errors
/// retain their existing status and handling. This helper never retries.
pub(crate) async fn run<T>(
    ctx: &Ctx,
    projection: impl Future<Output = ApiResult<Option<T>>>,
) -> ApiResult<Option<T>> {
    match projection.await {
        Err(error) if error.is_database_retry() => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}
