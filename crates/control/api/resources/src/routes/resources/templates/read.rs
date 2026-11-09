//! One native Template projection with the same bounded source names as its collection.

use super::*;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use crate::registry::OptionalOperationFuture;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::resources::collection::modern as scoped;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_authz::Caller;
const TEMPLATE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Template);

pub(super) const NAMES: NameRoles = NameRoles {
    groups: Some("template_groups"),
    nodes: [
        (ObjectKind::Org, "template_names_orgs"),
        (ObjectKind::Project, "template_names_projects"),
        (ObjectKind::Topic, "template_names_topics"),
        (ObjectKind::Thread, "template_names_threads"),
    ],
    containers: None,
};

struct Read {
    response: OperationPlan,
    projection: PreparedListProjection,
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        // The original legacy scalar/kept-out/native projection remains unchanged.
        // No empty source evidence is invented for ordinary compatibility.
        if !scoped(caller) {
            return Ok(None);
        }
        let id = ctx.id("id")?;
        let target = targets::page_object(
            conn,
            ObjectRef {
                kind: ResourceKind::Template.into(),
                id,
            },
        )
        .await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.bind(0, TEMPLATE, &[target])?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        require_read(conn, &principal, id).await?;
        let mut projection = prepare_projection(conn, &principal, id).await?;
        projection.bounds =
            projection_names(conn, caller, &mut plan, projection.sources.clone(), &NAMES).await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Read {
            response,
            projection,
        })))
    })
}

pub(super) async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Template> {
    crate::key_operations::snapshot_read::run(ctx, snapshot).await
}

async fn snapshot(ctx: Ctx) -> ApiResult<Template> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let id = ctx.id("id")?;
    if !scoped(principal.caller()) {
        require_read(&mut tx, &principal, id).await?;
        let shown = show(&mut tx, &principal, id).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(shown);
    }
    let prepared = ctx.operation_data::<Read>()?;
    let shown = prepared
        .response
        .clone()
        .read_response(&ctx, &mut tx, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                require_read(conn, reader, id).await?;
                show_list_prepared(conn, reader, &prepared.projection)
                    .await?
                    .pop()
                    .ok_or_else(ApiError::not_found)
            })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the template response content has not been admitted",
            )
        })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}
