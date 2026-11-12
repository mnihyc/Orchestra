//! Own-operation current Document authority for file and archive content.

use crate::middleware::inspection;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::routes::documents::read_operation as document_read;

use axum::response::Response;

use orch_authz::Caller;

use orch_db::Conn;
struct PreparedRoot {
    inspection: Option<inspection::Info>,
}

/// The Document is the actual governing object. File/blob selection belongs to the ordinary
/// producer at open, not to a new File/hash target or a later file-row liveness rule.
pub(super) fn prepare<'a, Q>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: Q,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let document_read::ReadRoot {
            plan, inspection, ..
        } = document_read::prepare_root(ctx, conn, caller).await?;
        Ok(plan.with_data(PreparedRoot { inspection }))
    })
}

/// The same current root/ordinary authority and original inspection tuple, before work and
/// every public delivery. No connection remains held while a blob/archive source waits.
pub(super) async fn current(ctx: &Ctx) -> ApiResult<()> {
    // Retained-content deadlines are checked at this delivery, not the opening's old time.
    // Keep the original caller, accepted ceiling, input and inspection tuple intact.
    let mut checkpoint = ctx.clone();
    checkpoint.now = ctx.app.clock.now();
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let _principal = checkpoint.operation_principal(&mut conn).await?;
    let root = checkpoint.operation_data::<PreparedRoot>()?;
    document_read::same_inspection_info(&checkpoint, root.inspection.as_ref())?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

pub(super) async fn response(ctx: Ctx, result: ApiResult<Response>) -> ApiResult<Response> {
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            current(&ctx).await?;
            return Err(error);
        }
    };
    crate::response_body::checked(response, move || {
        let current = ctx.clone();
        async move { self::current(&current).await }
    })
    .await
}
