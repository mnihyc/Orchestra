//! The actual kept review, never a fresh selection or execution of its removal effects.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::directory_sources as sources;

use orch_api_contract::directory_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;
pub(super) struct Plans;

pub(super) struct Prepared {
    row: PreflightRow,
    kept: KeptPlan,
    org: Id,
    root_visible: bool,
    query: OrgOutsiderRemovePlanGetQuery,
}

async fn require(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
    let org = the_org(conn, ctx.id("id")?).await?;
    super::super::require_on_keeper_projected(
        ctx,
        conn,
        principal,
        org.id,
        authorization::exact(Permission::MembersManage),
    )
    .await
}

impl native::Selected for Plans {
    type Source = Source;
    type Query = OrgOutsiderRemovePlanGetQuery;
    type Value = Prepared;
    type Reply = OutsideRemovalPlan;
    const DECLARATION: &'static Declaration<Source> = &sources::OUTSIDER_PLAN;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a OrgOutsiderRemovePlanGetQuery,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let org = the_org(conn, ctx.id("id")?).await?;
            // The preliminary real Org gate preserves ordinary refusal precedence;
            // only the validated stored row supplies the final factual subject.
            let known = native::bind_known(
                ctx,
                Self::DECLARATION,
                &[native::Facts::one(
                    Source::StoredPlanOrganization,
                    KeyTargetKind::Object(ObjectKind::Org),
                    targets::node(conn, org.id).await?,
                )],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(ctx, conn, &principal).await?;
            let user = outsider(conn, ctx, org.org).await?;
            let (row, kept) =
                kept_plan(conn, ctx, &principal, ctx.id("plan_id")?, org.id, user).await?;
            let stored_org = row.node_id.ok_or_else(ApiError::not_found)?;
            let root_visible = crate::routes::nodes::view::reference_visibility(
                conn,
                &principal,
                &[stored_org],
                Policy::default(),
            )
            .await?
            .contains(&stored_org);
            if !refusals::user_refs(conn, &[kept.user])
                .await?
                .contains_key(&kept.user)
            {
                return Err(ApiError::not_found());
            }
            let facts = vec![native::Facts::one(
                Source::StoredPlanOrganization,
                KeyTargetKind::Object(ObjectKind::Org),
                targets::node(conn, stored_org).await?,
            )];
            Ok((
                Prepared {
                    row,
                    kept,
                    org: stored_org,
                    root_visible,
                    query: query.clone(),
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
        Box::pin(require(ctx, conn, principal))
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<OutsideRemovalPlan>> {
        Box::pin(async move {
            let limit = PageQuery {
                limit: value.query.limit,
                cursor: None,
            }
            .limit();
            plan_page(
                conn,
                principal,
                &value.row,
                &value.kept,
                value.org,
                value.query.cursor.as_deref(),
                limit,
            )
            .await
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a OrgOutsiderRemovePlanGetQuery,
        opened: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.row.id == fresh.row.id
                && opened.row.kind == fresh.row.kind
                && opened.row.node_id == fresh.row.node_id
                && opened.row.created_by == fresh.row.created_by
                && opened.row.created_at == fresh.row.created_at
                && opened.row.expires_at == fresh.row.expires_at
                && opened.row.plan == fresh.row.plan
                && opened.kept.user == fresh.kept.user
                && opened.kept.fingerprint == fresh.kept.fingerprint
                && (!opened.root_visible || fresh.root_visible))
        })
    }
}
