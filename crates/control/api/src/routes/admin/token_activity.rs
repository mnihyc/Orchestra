//! Administrative token history reuses the same bounded native activity projector.
use crate::key_operations::{credentials, native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan, Registry};
use futures::future::BoxFuture;
use orch_access_contract::{
    audit::AuditAction,
    key_scope::{KeyIdentityKind, KeyTargetKind},
};
use orch_api_contract::{
    admin::{AdminTokenActivity, AdminUserTokenActivityList},
    admin_sources::{self as sources, Source},
    auth::ApiTokenActivityQuery,
    common::Page,
    operation_sources::Declaration,
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError};
use orch_types::Id;
use serde_json::json;
const KEY: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Key);
pub(super) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        AdminUserTokenActivityList,
        native::selected::<Activity>,
        read,
    );
}
async fn read(
    ctx: Ctx,
    query: ApiTokenActivityQuery,
    (): (),
) -> ApiResult<Page<AdminTokenActivity>> {
    native::selected_read::<Activity>(ctx, query, ()).await
}
struct Activity;
struct Selected {
    owner: Id,
    token: Id,
    page: Page<AdminTokenActivity>,
    plan: OperationPlan,
}
async fn source(
    ctx: &Ctx,
    conn: &mut Conn,
) -> ApiResult<(Id, Id, OperationPlan, Vec<native::Facts<Source>>)> {
    let owner = ctx.id("id")?;
    super::native::person(conn, owner).await?;
    let token = ctx.id("token_id")?;
    let target = targets::retained_key_of_account(conn, token, owner).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(0, KEY, std::slice::from_ref(&target))?;
    Ok((
        owner,
        token,
        plan,
        vec![native::Facts::one(Source::Token, KEY, target)],
    ))
}
impl native::Selected for Activity {
    type Source = Source;
    type Query = ApiTokenActivityQuery;
    type Value = Selected;
    type Reply = Page<AdminTokenActivity>;
    const DECLARATION: &'static Declaration<Source> = &sources::USER_TOKEN_ACTIVITY;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a ApiTokenActivityQuery,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (owner, token, plan, facts) =
                native::observed(ctx, "credential.activity.source", source(ctx, conn).await)?;
            let principal = native::observed(
                ctx,
                "credential.activity.preparation",
                native::preparation_principal(ctx, conn, caller, &plan).await,
            )?;
            native::observed(
                ctx,
                "credential.activity.administration",
                super::native::require(ctx, &principal),
            )?;
            let page = native::observed(
                ctx,
                "credential.activity.page",
                crate::routes::auth::token_activity::recent(
                    ctx,
                    conn,
                    &principal,
                    token,
                    None,
                    query.clone(),
                    true,
                )
                .await,
            )?;
            let page = Page {
                items: page
                    .items
                    .into_iter()
                    .map(|info| AdminTokenActivity {
                        info,
                        admin_access: true,
                    })
                    .collect(),
                next_cursor: page.next_cursor,
            };
            Ok((
                Selected {
                    owner,
                    token,
                    page,
                    plan,
                },
                facts,
            ))
        })
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a ApiTokenActivityQuery,
        old: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (owner, token, plan, facts) = native::observed(
                ctx,
                "credential.activity.current_source",
                source(ctx, conn).await,
            )?;
            if owner != old.owner || token != old.token {
                return Err(ApiError::not_found());
            }
            let principal = native::observed(
                ctx,
                "credential.activity.current_preparation",
                native::preparation_principal(ctx, conn, caller, &plan).await,
            )?;
            native::observed(
                ctx,
                "credential.activity.current_administration",
                super::native::require(ctx, &principal),
            )?;
            let emitted: Vec<_> = old.page.items.iter().map(|row| row.info.clone()).collect();
            if emitted.iter().any(|row| {
                row.at.get()
                    < ctx.app.clock.now()
                        - chrono::Duration::days(orch_db::queries::key_activity::KEPT_DAYS)
            }) || !crate::key_operations::activity_store::current_names(
                conn, &principal, &emitted,
            )
            .await?
            {
                return native::observed(
                    ctx,
                    "credential.activity.current_names",
                    Err(credentials::refused()),
                );
            }
            Ok((
                Selected {
                    owner,
                    token,
                    plan,
                    page: old.page.clone(),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            super::native::person(conn, ctx.id("id")?).await?;
            super::native::require(ctx, principal)
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(value.page.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a ApiTokenActivityQuery,
        old: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.owner == fresh.owner && old.token == fresh.token) })
    }
    fn after_read<'a>(
        ctx: &'a Ctx,
        _accepted: &'a Caller,
        query: &'a ApiTokenActivityQuery,
        old: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let (mut tx, guard) = native::observed(
                ctx,
                "credential.activity.audit_guard",
                super::action(ctx).await,
            )?;
            let (fresh, _) =
                Self::refresh(ctx, &mut tx, guard.principal().caller(), query, old).await?;
            if !fresh.plan.shared().covers_sources(old.plan.shared()) {
                return native::observed(
                    ctx,
                    "credential.activity.audit_association",
                    Err(credentials::refused()),
                );
            }
            let admitted = native::observed(
                ctx,
                "credential.activity.audit_admission",
                fresh.plan.admit(guard.principal().caller()),
            )?;
            if !admitted.content {
                return native::observed(
                    ctx,
                    "credential.activity.audit_read",
                    Err(credentials::refused()),
                );
            }
            guard.check_deadline(ctx)?;
            super::record(
                ctx,
                &mut tx,
                guard.user().id(),
                orch_db::audit::action!(AuditAction::AdminOpened),
                Some(old.owner),
                "opened API token activity",
                json!({"operation":ctx.route.name,"owner_id":old.owner,"token":old.token}),
            )
            .await?;
            guard.check_deadline(ctx)?;
            tx.commit().await.map_err(DbError::from)?;
            guard.check_deadline(ctx)
        })
    }
}
