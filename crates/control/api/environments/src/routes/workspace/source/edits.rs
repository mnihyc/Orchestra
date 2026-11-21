//! Pending start files keep the same thread and workspace read boundaries as their destination.

use super::*;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::source::{EnvironmentSourceEditsQuery, WorkspaceFileEdits};

pub(super) fn operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    query: EnvironmentSourceEditsQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let environment = ctx.id("id")?;
        let row = threads::thread(conn, query.thread_id)
            .await?
            .filter(|row| row.deleted_at.is_none() && row.environment_id == Some(environment))
            .ok_or_else(ApiError::not_found)?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        let env = crate::key_operations::targets::page_object(
            conn,
            ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Environment,
                id: environment,
            },
        )
        .await?;
        plan.bind(
            0,
            KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Environment),
            &[env],
        )?;
        let thread = crate::key_operations::targets::node(conn, row.node_id).await?;
        plan.bind(
            1,
            KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Thread),
            &[thread],
        )?;
        Ok(plan)
    })
}

pub(super) async fn read(
    ctx: Ctx,
    query: EnvironmentSourceEditsQuery,
    (): (),
) -> ApiResult<WorkspaceFileEdits> {
    let environment = ctx.id("id")?;
    let (reply, ()) = crate::key_operations::native::read(ctx, move |_ctx, conn, principal| {
        Box::pin(async move {
            require(conn, principal, environment, Permission::FilesView).await?;
            authorization::require(
                conn,
                principal,
                query.thread_id,
                authorization::VIEW,
                orch_authz::Act::Use,
                Policy::default(),
            )
            .await?;
            let row = threads::thread(conn, query.thread_id)
                .await?
                .filter(|row| row.deleted_at.is_none() && row.environment_id == Some(environment))
                .ok_or_else(ApiError::not_found)?;
            let edits = row
                .source
                .as_ref()
                .map(source::staged_edits)
                .transpose()
                .map_err(ApiError::engine)?
                .unwrap_or_default();
            let env = orch_db::queries::environments::workspace::environment(conn, environment)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let revision = source::workspace_edit_revision(&row, env.generation, &edits)
                .map_err(ApiError::engine)?;
            Ok((WorkspaceFileEdits { edits, revision }, ()))
        })
    })
    .await?;
    Ok(reply)
}
