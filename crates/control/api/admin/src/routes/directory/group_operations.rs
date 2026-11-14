//! Existing group writers and holder readers, bound to their actual group and projection sources.

use super::*;

use crate::key_operations::admission::Prepared;

use crate::key_operations::targets;

use crate::registry::OperationFuture;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use crate::routes::access::page::Names;

use crate::routes::access::page::holder_list_name_sources;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::pages::Holder;

use orch_access_contract::permissions::ObjectKind;

use orch_authz::Caller;

use orch_authz::Principal;
const GROUP: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Group);

const NAMES: NameRoles = NameRoles {
    groups: Some("group_payloads"),
    nodes: [
        (ObjectKind::Org, "group_names_orgs"),
        (ObjectKind::Project, "group_names_projects"),
        (ObjectKind::Topic, "group_names_topics"),
        (ObjectKind::Thread, "group_names_threads"),
    ],
    containers: None,
};

async fn base(ctx: &Ctx, conn: &mut Conn) -> ApiResult<(Prepared, access::GroupFull, Placed)> {
    let (group, place) = the_group(conn, ctx.id("id")?).await?;
    let target = targets::from_group(conn, &group).await?;
    let mut plan = Prepared::new(ctx.route, ctx.route.name)?;
    plan.bind(0, GROUP, &[target])?;
    Ok((plan, group, place))
}

pub(super) fn update<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    body: GroupUpdateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        // Keep the existing writer's input-validation order on ordinary calls.
        if let Some(name) = body.name.as_deref() {
            clean_group_name(name)?;
        }
        Ok(base(ctx, conn).await?.0)
    })
}

struct PreparedHolders {
    members: Vec<Holder>,
    root: Id,
    keeper: Id,
    names: NameBounds,
}

pub(super) fn holders<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: orch_api_contract::common::CollectionQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (mut plan, group, place) = base(ctx, conn).await?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        // A source-bearing denial has no admitted keeper-name projection yet.
        authorization::require_projected(
            conn,
            &principal,
            group.org_id,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
            caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some()),
        )
        .await?;
        let members = access::group_holders(conn, group.id)
            .await?
            .iter()
            .map(group_holder)
            .collect::<ApiResult<Vec<_>>>()?;
        let sources = holder_list_name_sources(std::slice::from_ref(&members), Some(group.org_id));
        let names = projection_names(conn, principal.caller(), &mut plan, sources, &NAMES).await?;
        Ok(plan.with_data(PreparedHolders {
            members,
            root: place.org,
            keeper: group.org_id,
            names,
        }))
    })
}

/// Use the source rows selected by the same actual read-connection preparation.
pub(super) async fn holder_list(
    ctx: Ctx,
    query: orch_api_contract::common::CollectionQuery,
    (): (),
) -> ApiResult<orch_api_contract::common::Page<orch_api_contract::access::Holder>> {
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let prepared = ctx.operation_data::<PreparedHolders>()?;
    let answer = if prepared.names.limited {
        Names::holders_bounded(
            &mut conn,
            &principal,
            Some(prepared.root),
            Some(prepared.keeper),
            &prepared.members,
            &prepared.names,
        )
        .await?
    } else {
        Names::holders(
            &mut conn,
            &principal,
            Some(prepared.root),
            Some(prepared.keeper),
            &prepared.members,
        )
        .await?
    };
    conn.commit().await.map_err(DbError::from)?;
    orch_api_base::collection::snapshot(
        &query,
        &format!("group-holders:{}", ctx.id("id")?),
        answer,
        orch_api_base::collection::holder_key,
        orch_api_base::collection::holder_name,
        false,
    )
}

/// Only the operation's Read may materialize the updated Group and membership count.
/// The optional home qualification uses the existing S02 bounded-name producer.
pub(super) async fn updated_group(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<Option<Group>> {
    let (mut plan, group, _) = base(ctx, conn).await?;
    let mut sources = NameSources::default();
    sources.groups.insert(group.id);
    let bounds = projection_names(conn, principal.caller(), &mut plan, sources, &NAMES).await?;
    let result = plan
        .read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                authorization::require_projected(
                    conn,
                    reader,
                    group.org_id,
                    exact(Permission::MembersManage),
                    Act::Use,
                    Policy::default(),
                    reader
                        .caller()
                        .grant()
                        .is_some_and(|grant| grant.operation_scope().is_some()),
                )
                .await?;
                groups_named_bounded(conn, reader, vec![group], Some(&bounds))
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(ApiError::not_found)
            })
        })
        .await;
    match result {
        // The effect was already admitted. Loss of this optional current view
        // produces the same receipt; it cannot disclose a named denial instead.
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(None),
        other => other,
    }
}
