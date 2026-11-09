//! Administrative token history reuses the same bounded native activity projector.

use crate::key_operations::credentials;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::registry::Registry;

use futures::future::BoxFuture;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::admin::AdminTokenActivity;

use orch_api_contract::admin::AdminUserTokenActivityList;

use orch_api_contract::admin_sources as sources;

use orch_api_contract::admin_sources::Source;

use orch_api_contract::auth::ApiTokenActivityQuery;

use orch_api_contract::common::Page;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

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
            let (owner, token, plan, facts) = source(ctx, conn).await?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            super::native::require(ctx, &principal)?;
            let page = crate::routes::auth::token_activity::recent(
                ctx,
                conn,
                &principal,
                token,
                None,
                query.clone(),
                true,
            )
            .await?;
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
            let (owner, token, plan, facts) = source(ctx, conn).await?;
            if owner != old.owner || token != old.token {
                return Err(ApiError::not_found());
            }
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            super::native::require(ctx, &principal)?;
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
                return Err(credentials::refused());
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
            let (mut tx, guard) = super::action(ctx).await?;
            let (fresh, _) =
                Self::refresh(ctx, &mut tx, guard.principal().caller(), query, old).await?;
            if !fresh.plan.shared().covers_sources(old.plan.shared())
                || !fresh.plan.admit(guard.principal().caller())?.content
            {
                return Err(credentials::refused());
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
