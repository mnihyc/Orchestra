//! Bounded phase diagnostics around the existing import adapters.

use super::DocumentImportRequest;

use super::operation::Imports;

use crate::key_operations::native::effects;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationInitialFuture;

use orch_authz::Caller;

use orch_db::Conn;
pub(super) fn phase(ctx: &Ctx, stage: &'static str) {
    tracing::debug!(stage, disclosure = ?ctx.disclosure(), "document import checkpoint");
}

pub(super) fn at_stage<T>(ctx: &Ctx, stage: &'static str, result: ApiResult<T>) -> ApiResult<T> {
    match result {
        Ok(value) => {
            phase(ctx, stage);
            Ok(value)
        }
        Err(error) => {
            refused(ctx, stage, &error);
            Err(error)
        }
    }
}

fn refused(ctx: &Ctx, stage: &'static str, error: &ApiError) {
    tracing::debug!(
        stage,
        disclosure = ?ctx.disclosure(),
        code = ?error.code(),
        status = error.problem.status,
        "document import checkpoint refused"
    );
}

pub(super) fn initial<'a>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    query: (),
    body: DocumentImportRequest,
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        phase(ctx, "initial entered");
        at_stage(
            ctx,
            "initial completed",
            effects::initial_common::<Imports>(ctx, caller, query, body).await,
        )
    })
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: DocumentImportRequest,
) -> OperationFuture<'a> {
    Box::pin(async move {
        phase(ctx, "preparation entered");
        at_stage(
            ctx,
            "preparation completed",
            effects::prepare::<Imports>(ctx, conn, caller, query, body).await,
        )
    })
}
