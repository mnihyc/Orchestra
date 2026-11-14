//! Existing directory group selections, with complete rows and optional keeper names bounded.

use super::*;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::group_targets;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_authz::Caller;
const GROUP: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Group);

const NAMES: NameRoles = NameRoles {
    groups: Some("listed_groups"),
    nodes: [
        (ObjectKind::Org, "listed_group_orgs"),
        (ObjectKind::Project, "listed_group_projects"),
        (ObjectKind::Topic, "listed_group_topics"),
        (ObjectKind::Thread, "listed_group_threads"),
    ],
    containers: None,
};

/// These are the two existing selectors, not caller-supplied object kinds.
#[derive(Clone, Copy)]
enum Selection {
    Org { under: bool },
    Nearest,
}

struct PreparedList {
    response: OperationPlan,
    rows: Vec<access::GroupFull>,
    names: NameBounds,
    next_cursor: Option<String>,
}

/// Read identities only. Ordinary node visibility and the original nearest-keeper
/// decision run below with the same admitted account before a payload is projected.
async fn candidates(
    conn: &mut Conn,
    id: Id,
    selection: Selection,
) -> ApiResult<(Option<Id>, Vec<access::GroupFull>)> {
    let (keeper, under) = match selection {
        Selection::Org { under } => {
            the_org(conn, id).await?;
            (Some(id), under)
        }
        Selection::Nearest => {
            let rows = hierarchy::with_ancestors(conn, &[id]).await?;
            let node = rows
                .iter()
                .find(|row| row.id == id && row.kind != "thread")
                .ok_or_else(ApiError::not_found)?;
            let keeper = rows
                .iter()
                .filter(|row| row.kind == "org" && node.path.contains(&row.id))
                .max_by_key(|row| row.path.len())
                .map(|row| row.id);
            (keeper, false)
        }
    };
    let rows = match keeper {
        Some(keeper) => access::groups_of_orgs(conn, keeper, under).await?,
        None => Vec::new(),
    };
    Ok((keeper, rows))
}

pub(super) fn org<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: OrgGroupListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare(
        ctx,
        conn,
        caller,
        Selection::Org {
            under: query.under.unwrap_or(false),
        },
        query.collection(),
    ))
}

pub(super) fn node<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: orch_api_contract::common::CollectionQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare(ctx, conn, caller, Selection::Nearest, query))
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    selection: Selection,
    query: orch_api_contract::common::CollectionQuery,
) -> ApiResult<OperationPlan> {
    let id = ctx.id("id")?;
    let (keeper, rows) = candidates(conn, id, selection).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let targets = group_targets(conn, &rows.iter().map(|row| row.id).collect::<Vec<_>>()).await?;
    let targets: HashMap<_, _> = targets
        .into_iter()
        .filter_map(|target| target.object.id.map(|id| (id, target)))
        .collect();
    let mut selected = Vec::new();
    let mut selected_targets = Vec::new();
    // Filter whole Group rows in the ordinary SQL name/id order. The aggregate
    // membership count belongs to that primary Group, not to nested holder payloads.
    for row in rows {
        let target = targets.get(&row.id).ok_or_else(ApiError::not_found)?;
        if plan.permits_binding(caller, 0, GROUP, std::slice::from_ref(target))? {
            selected_targets.push(target.clone());
            selected.push(row);
        }
    }
    let mut initial = plan.clone();
    if selected_targets.is_empty() {
        // A fully scanned scope-filtered selection is empty too. The exact declared
        // ReadGroup operation must still pass the existing checked-empty gate.
        initial.bind_empty(0)?;
    } else {
        initial.bind(0, GROUP, &selected_targets)?;
    }
    let principal = initial.preparation_principal(ctx, conn, caller).await?;
    let bounded = caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some());
    // Repeat the original requested-node View on this current read connection.
    // Scope admission keeps the same account; it supplies no ordinary ACL rights.
    authorization::require_projected(
        conn,
        &principal,
        id,
        authorization::VIEW,
        Act::Use,
        Policy::default(),
        bounded,
    )
    .await?;
    if matches!(selection, Selection::Nearest)
        && let Some(keeper) = keeper
        && authorization::require_projected(
            conn,
            &principal,
            keeper,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
            bounded,
        )
        .await
        .is_err()
    {
        // Existing NodeGroupList behavior: a visible place with a private keeper
        // discloses no groups, even when the key scope names those identities.
        selected.clear();
    }
    let source = format!(
        "{}:{id}:{keeper:?}:{}",
        ctx.route.name,
        matches!(selection, Selection::Org { under: true })
    );
    let page = orch_api_base::collection::snapshot(
        &query,
        &source,
        selected,
        |row| (row.name.clone(), row.id),
        |row| row.name.clone(),
        false,
    )?;
    let selected = page.items;
    let next_cursor = page.next_cursor;
    let mut sources = NameSources::default();
    sources.groups.extend(selected.iter().map(|row| row.id));
    // This shared producer binds the primary Group role once and only the
    // separately permitted live node names used for optional place qualification.
    let names = projection_names(conn, caller, &mut plan, sources, &NAMES).await?;
    let response = plan.clone();
    Ok(plan.with_data(PreparedList {
        response,
        rows: selected,
        names,
        next_cursor,
    }))
}

pub(super) async fn org_list(
    ctx: Ctx,
    _query: OrgGroupListQuery,
    (): (),
) -> ApiResult<orch_api_contract::common::Page<Group>> {
    crate::key_operations::snapshot_read::run(ctx, snapshot).await
}

pub(super) async fn node_list(
    ctx: Ctx,
    _query: orch_api_contract::common::CollectionQuery,
    (): (),
) -> ApiResult<orch_api_contract::common::Page<Group>> {
    crate::key_operations::snapshot_read::run(ctx, snapshot).await
}

async fn snapshot(ctx: Ctx) -> ApiResult<orch_api_contract::common::Page<Group>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    // Re-run this exact producer under the accepted/current credential ceiling,
    // replacing any earlier richer row or name footprint on the actual snapshot.
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<PreparedList>()?;
    let rows = prepared
        .response
        .clone()
        .read_response(&ctx, &mut tx, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                let items = groups_named_bounded(
                    conn,
                    reader,
                    prepared.rows.clone(),
                    Some(&prepared.names),
                )
                .await?;
                Ok(orch_api_contract::common::Page {
                    items,
                    next_cursor: prepared.next_cursor.clone(),
                })
            })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the group list response content has not been admitted",
            )
        })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(rows)
}
