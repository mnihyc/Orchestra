//! Permanent-loss previews and typed confirmations under fresh resource authority.

use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::{Ctx, Registry};
use orch_api_contract::{common::ErrorCode, loss as api};
use orch_engine::loss::LossError;

/// Binds reviewed declarations of permanent loss.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::LossPreflight, preflight)
        .on(api::LossDeclare, declare)
        .on(api::LossRecoveryList, recoverable)
        .on(api::OperationRecipientsGet, recipients);
}

async fn recoverable(
    ctx: Ctx,
    (): (),
    body: api::LossPreflightBody,
) -> ApiResult<api::RecoverySources> {
    use orch_access_contract::permissions::{Permission, ResourceKind};
    let (_, mut conn, principal) = crate::routes::nodes::action(&ctx).await?;
    match body.target {
        api::LossTarget::Host { id } => {
            authorization::require_resource(
                &mut conn,
                &principal,
                ResourceKind::Host,
                id,
                Permission::HostManage,
                orch_authz::Policy::default(),
            )
            .await?
        }
        api::LossTarget::Environment { id } => {
            authorization::require_resource(
                &mut conn,
                &principal,
                ResourceKind::Environment,
                id,
                Permission::EnvironmentManage,
                orch_authz::Policy::default(),
            )
            .await?
        }
        api::LossTarget::Machine { id } => {
            let owner = orch_db::queries::resources::machine(&mut conn, id)
                .await?
                .and_then(|row| row.owner_id);
            if owner != Some(principal.user())
                && !orch_engine::operation_access::can_bypass(
                    &mut conn,
                    &principal,
                    orch_access_contract::decisions::ObjectRef {
                        kind: orch_access_contract::permissions::ObjectKind::Machine,
                        id,
                    },
                    Permission::MachineManage,
                )
                .await
                .map_err(ApiError::internal)?
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "Only the machine's owner or a reviewed administrative act may recover its environments",
                ));
            }
            authorization::require_resource(
                &mut conn,
                &principal,
                ResourceKind::Machine,
                id,
                Permission::MachineManage,
                orch_authz::Policy::default(),
            )
            .await?;
        }
    }
    let inventory =
        orch_db::queries::loss::inventory(&mut conn, body.target.kind(), body.target.id())
            .await?
            .ok_or_else(ApiError::not_found)?;
    let mut rows = Vec::new();
    let mut others = 0;
    for environment in inventory.environments {
        if environment.status == "removed" {
            continue;
        }
        let access = orch_engine::operation_access::review(
            &mut conn,
            &principal,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Environment,
                id: environment.id,
            },
            Permission::EnvironmentManage,
        )
        .await;
        if matches!(access, Err(orch_engine::updates::UpdateError::Denied)) {
            others += 1;
            continue;
        }
        access.map_err(super::environments::updates::refused)?;
        {
            rows.push(api::LossResource {
                id: environment.id,
                name: Some(environment.name),
            });
        }
    }
    Ok(api::RecoverySources {
        environments: rows,
        others,
    })
}

pub(crate) fn refused(error: LossError) -> ApiError {
    let code = match &error {
        LossError::NotFound => ErrorCode::NotFound,
        LossError::Denied => ErrorCode::AuthzDenied,
        LossError::Online => ErrorCode::PlanBlocked,
        LossError::Stale => ErrorCode::ConflictStaleRevision,
        LossError::Confirmation => ErrorCode::RequestInvalid,
        LossError::Db(_) | LossError::Internal(_) => return ApiError::internal(error),
    };
    ApiError::new(code, error.to_string())
}

async fn preflight(ctx: Ctx, (): (), body: api::LossPreflightBody) -> ApiResult<api::LossPlan> {
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let plan = ctx
        .app
        .engine
        .plan_loss(&mut tx, &principal, body.target)
        .await
        .map_err(refused)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(plan)
}

async fn declare(ctx: Ctx, (): (), body: api::LossDeclareBody) -> ApiResult<api::LossPlan> {
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let reviewed = orch_db::queries::jobs::forks::preflight(&mut tx, body.preflight_id)
        .await?
        .filter(|row| row.kind == "loss" && row.created_by == principal.user())
        .ok_or_else(ApiError::not_found)?;
    if reviewed.expires_at <= ctx.now {
        return Err(refused(LossError::Stale));
    }
    let target: api::LossTarget = serde_json::from_value(reviewed.plan["plan"]["target"].clone())
        .map_err(ApiError::internal)?;
    let machines = orch_db::queries::loss::inventory(&mut tx, target.kind(), target.id())
        .await?
        .ok_or_else(ApiError::not_found)?
        .machines;
    let plan = ctx
        .app
        .engine
        .declare_loss(&mut tx, &principal, &body)
        .await
        .map_err(refused)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    for machine in machines {
        ctx.app.link.let_go(machine).await;
    }
    ctx.app.engine.kick();
    Ok(plan)
}

/// Reads only the caller's private unexpired audience and names its people under current visibility.
async fn recipients(
    ctx: Ctx,
    query: orch_api_contract::common::PageQuery,
    (): (),
) -> ApiResult<api::OperationRecipients> {
    use orch_db::queries::notifications::operations;
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let ids: Option<(Vec<orch_types::Id>,String)>=sqlx::query_as("SELECT p.users,p.notification_kind FROM operation_recipient_previews p JOIN preflights f ON f.id=p.id WHERE p.id=$1 AND f.created_by=$2 AND f.expires_at>$3")
      .bind(id).bind(principal.user()).bind(ctx.now).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let (ids, kind) = ids.ok_or_else(ApiError::not_found)?;
    let ids = operations::receivers(&mut conn, &ids, &kind).await?;
    let visible = operations::visible_names(&mut conn, principal.user(), &ids).await?;
    let after = query
        .cursor
        .as_deref()
        .map(|text| {
            text.parse::<orch_types::Id>()
                .map_err(|_| ApiError::invalid("cursor", "is not a cursor of this list"))
        })
        .transpose()?;
    let limit = usize::try_from(query.limit()).map_err(ApiError::internal)?;
    let page = visible
        .iter()
        .copied()
        .filter(|id| after.is_none_or(|after| *id > after))
        .take(limit + 1)
        .collect::<Vec<_>>();
    let next_cursor = if page.len() > limit {
        page.get(limit - 1).map(ToString::to_string)
    } else {
        None
    };
    let users = crate::middleware::authorization::refusals::user_refs(
        &mut conn,
        &page[..page.len().min(limit)],
    )
    .await?;
    let items = page
        .iter()
        .take(limit)
        .filter_map(|id| users.get(id).cloned())
        .collect();
    Ok(api::OperationRecipients {
        people: orch_api_contract::common::Page { items, next_cursor },
        hidden: (ids.len() - visible.len()) as i64,
        total: ids.len() as i64,
    })
}
