//! Native administrative source facts consumed by the shared adapters.
use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::access::page::{NameBounds, NameSources};
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::admin_sources::Source;
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_types::Id;
use std::collections::BTreeMap;

pub(super) fn collection(
    source: Source,
    rows: Vec<(KeyTargetKind, KeyResolvedTarget)>,
) -> native::Facts<Source> {
    let mut groups = BTreeMap::new();
    for (kind, row) in rows {
        groups.entry(kind).or_insert_with(Vec::new).push(row);
    }
    native::Facts::collection(source, groups.into_iter().collect())
}
/// The completed reader keeps current native standing without repeating its accepted Manage effect.
pub(super) fn standing(ctx: &Ctx, principal: &Principal) -> ApiResult<()> {
    if !principal.has_system_admin_standing()
        || (ctx.user()?.is_session() && !principal.caller().as_admin())
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "current system administration is required",
        ));
    }
    Ok(())
}
pub(super) fn require(ctx: &Ctx, principal: &Principal) -> ApiResult<()> {
    ctx.ordinary_system_admin_with(ctx.user()?, principal.caller())?;
    if !principal.has_system_admin_standing() {
        return Err(
            crate::middleware::authorization::refusals::system_admin_only(
                "current system administration is required",
            ),
        );
    }
    Ok(())
}
pub(super) fn permits(
    ctx: &Ctx,
    declaration: &'static Declaration<Source>,
    caller: &Caller,
    source: Source,
    family: KeyTargetKind,
    target: &KeyResolvedTarget,
) -> ApiResult<bool> {
    if !native::modern(caller) {
        return Ok(true);
    }
    let plan = native::bind_known(ctx, declaration, &[])?;
    native::permits_usage(
        &plan,
        declaration,
        caller,
        &source,
        declaration
            .bindings
            .iter()
            .find(|b| b.source == source)
            .ok_or_else(|| ApiError::internal("the administrative source is undeclared"))?
            .usage,
        family,
        target,
    )
}

pub(super) async fn names(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    mut names: NameSources,
) -> ApiResult<(NameBounds, native::Facts<Source>)> {
    let groups = crate::routes::access::operations::group_targets(
        conn,
        &names.groups.iter().copied().collect::<Vec<_>>(),
    )
    .await?;
    names.nodes.extend(
        groups
            .iter()
            .flat_map(|g| g.node_ancestors.iter().filter_map(|n| n.id)),
    );
    names.nodes.extend(
        orch_db::queries::access::holder_user_addresses(
            conn,
            &names.users.iter().copied().collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .map(|(_, _, domain, _)| domain),
    );
    let graph =
        orch_authz::engine::Graph::load(conn, &names.nodes.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
    let mut bounds = NameBounds {
        limited: native::modern(caller),
        ..NameBounds::default()
    };
    let mut facts = Vec::new();
    for node in graph.nodes() {
        if let Some(target) =
            orch_engine::operation_scope::targets::from_node_identity(&graph, node.id)
            && permits(
                ctx,
                declaration,
                caller,
                Source::Names,
                target.object.kind,
                &target,
            )?
        {
            bounds.nodes.insert(node.id);
            facts.push((target.object.kind, target));
        }
    }
    Ok((bounds, collection(Source::Names, facts)))
}

/// The existing Node factory owns typed optional content and name projection.
pub(super) async fn org_content(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    ids: &[Id],
) -> ApiResult<(Vec<orch_api_contract::nodes::Org>, native::Facts<Source>)> {
    crate::routes::nodes::projection::project_orgs(
        ctx,
        conn,
        principal,
        caller,
        declaration,
        Source::Names,
        ids,
    )
    .await
}

pub(super) async fn resource(
    conn: &mut Conn,
    kind: orch_access_contract::permissions::ObjectKind,
    id: Id,
) -> ApiResult<KeyResolvedTarget> {
    targets::resource_identity(
        conn,
        orch_access_contract::decisions::ObjectRef { kind, id },
    )
    .await
}

/// The Users screen names real people, including suspended people, never service accounts.
pub(super) async fn person(conn: &mut Conn, id: Id) -> ApiResult<()> {
    if orch_db::queries::identity::users_brief(conn, &[id])
        .await?
        .iter()
        .any(|row| row.id == id && row.kind == "human")
    {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}
