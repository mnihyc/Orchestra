//! Native Agent projection/source preparation shared by its individual and collection reads.
use super::*;
use crate::key_operations::targets;
use crate::registry::{OperationPlan, OptionalOperationFuture};
use crate::routes::access::operations::NameRoles;
use crate::routes::resources::collection::modern as scoped;
use crate::routes::resources::projection::{self, Source};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_authz::Caller;

const AGENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Agent);
pub(super) const NAMES: NameRoles = NameRoles {
    groups: Some("agent_groups"),
    nodes: [
        (ObjectKind::Org, "agent_names_orgs"),
        (ObjectKind::Project, "agent_names_projects"),
        (ObjectKind::Topic, "agent_names_topics"),
        (ObjectKind::Thread, "agent_names_threads"),
    ],
    containers: None,
};
struct Read {
    response: OperationPlan,
    projection: PreparedListProjection,
}
/// Both closed declarations use the same actual Provider/model and Machine source roles.
/// These are native content sources; no Provider/MachineView is manufactured by scope.
pub(super) async fn bind_sources(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    projection: &mut PreparedListProjection,
) -> ApiResult<()> {
    // Provider refs and full models share the exact ordinary ProviderView source set.
    // Bind it before either payload is materialized. No other operation lends Read.
    // Factual absence follows machine_refs' deleted predicate, without loading rich
    // MachineRef fields. A live unresolved facade refuses rather than becoming None.
    projection.bounds = projection::bind_sources(
        conn,
        caller,
        plan,
        &[
            Source::Resources {
                role: "agent_providers",
                kind: ResourceKind::Provider,
                ids: projection.provider_ids(),
            },
            Source::Machines {
                role: "agent_machines",
                ids: projection.machine_ids(),
            },
        ],
        projection.sources.clone(),
        &NAMES,
    )
    .await?;
    Ok(())
}
pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let id = ctx.id("id")?;
        let target = targets::page_object(
            conn,
            ObjectRef {
                kind: ResourceKind::Agent.into(),
                id,
            },
        )
        .await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        crate::routes::resources::context::empty(&mut plan)?;
        plan.bind(0, AGENT, &[target])?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        require_read(conn, &principal, id).await?;
        let mut projection = prepare_projection(conn, &principal, id).await?;
        bind_sources(conn, caller, &mut plan, &mut projection).await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Read {
            response,
            projection,
        })))
    })
}
pub(super) async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Agent> {
    crate::key_operations::snapshot_read::run(ctx, snapshot).await
}
async fn snapshot(ctx: Ctx) -> ApiResult<Agent> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let id = ctx.id("id")?;
    if !scoped(principal.caller()) {
        require_read(&mut tx, &principal, id).await?;
        let shown = show(&ctx, &mut tx, &principal, id).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(shown);
    }
    let prepared = ctx.operation_data::<Read>()?;
    let view_ctx = ctx.clone();
    let shown = prepared
        .response
        .clone()
        .read_response(&ctx, &mut tx, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                require_read(conn, reader, id).await?;
                show_list_prepared(&view_ctx, conn, reader, &prepared.projection)
                    .await?
                    .pop()
                    .ok_or_else(ApiError::not_found)
            })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the agent response content has not been admitted",
            )
        })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}
