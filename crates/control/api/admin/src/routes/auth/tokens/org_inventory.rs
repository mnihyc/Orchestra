//! The existing org-governed token inventory and refusal policy, with actual Key sources.

use super::*;

use crate::key_operations::credentials;

use crate::key_operations::targets;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::PermissionName;

use orch_authz::Act;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::engine::Graph;
const KEY: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Key);

struct Window {
    after: Option<Id>,
    size: usize,
    batch: i64,
}

impl Window {
    fn of(query: &OrgTokenListQuery) -> ApiResult<Self> {
        let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let after = query
            .cursor
            .as_deref()
            .map(|text| {
                text.parse::<Id>()
                    .map_err(|_| ApiError::invalid("cursor", "is not a cursor of this list"))
            })
            .transpose()?;
        Ok(Self {
            after,
            size: usize::try_from(limit).map_err(ApiError::internal)?,
            // The ordinary page and its one-row lookahead supply this batch size.
            // Scope filtering never invents an exhaustion budget.
            batch: i64::from(limit) + 1,
        })
    }
}

/// Key is the operation's resource family; the actual org governs its inventory
/// and refusal policy. This does not place any returned personal Key under it.
async fn governance(ctx: &Ctx, conn: &mut Conn) -> ApiResult<(OperationPlan, KeyResolvedTarget)> {
    let org = targets::node(conn, ctx.id("id")?).await?;
    if org.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
        return Err(ApiError::not_found());
    }
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(0, KEY, std::slice::from_ref(&org))?;
    Ok((plan, org))
}

pub(super) fn refusal<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    // The existing ordinary action verifies the selected currently reaching Key
    // before inserting the exact (org, token) refusal pair on this guarded Tx.
    // Its effect is no global Key revocation or permission on the owning account.
    Box::pin(async move {
        let (mut plan, org) = governance(ctx, conn).await?;
        // Bind the actual source even when this key cannot Read it. A missing
        // optional Read narrows the error through existing disclosure machinery;
        // the route's sole empty reply is already its truthful receipt.
        plan.bind(1, KeyTargetKind::Object(ObjectKind::Org), &[org])?;
        Ok(plan)
    })
}

/// Typed owner facts come from the same candidate query's joined users rows.
/// Human membership and legacy node_id are never invented into Key ancestry.
fn key_targets(rows: &[tokens::ReachingToken], graph: &Graph) -> ApiResult<Vec<KeyResolvedTarget>> {
    rows.iter()
        .map(|row| {
            let account = orch_engine::operation_scope::targets::from_account_identity(
                graph,
                row.key.user_id,
                &row.owner_kind,
                row.owner_org,
            )
            .ok_or_else(ApiError::not_found)?;
            targets::from_key_subject(&account, row.key.id, row.key.user_id)
        })
        .collect()
}

async fn owners(conn: &mut Conn, rows: &[tokens::ReachingToken]) -> ApiResult<Graph> {
    let mut ids: Vec<_> = rows.iter().filter_map(|row| row.owner_org).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        Ok(Graph::default())
    } else {
        Graph::load(conn, &ids).await.map_err(ApiError::internal)
    }
}

async fn selected(
    conn: &mut Conn,
    principal: &Principal,
    plan: &OperationPlan,
    org: Id,
    window: Window,
) -> ApiResult<(Vec<tokens::ReachingToken>, Vec<KeyResolvedTarget>, bool)> {
    let modern = principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some());
    let mut after = window.after;
    let mut kept = Vec::new();
    let mut facts = Vec::new();
    loop {
        let rows = tokens::reaching(conn, org, principal.now().get(), after, window.batch).await?;
        let exhausted = rows.len() < usize::try_from(window.batch).map_err(ApiError::internal)?;
        let next = rows.last().map(|row| row.key.id);
        if let Some(next) = next
            && after.is_some_and(|old| next <= old)
        {
            return Err(ApiError::internal(
                "the ordinary token inventory cursor did not advance",
            ));
        }
        let graph = owners(conn, &rows).await?;
        let targets = key_targets(&rows, &graph)?;
        for (row, target) in rows.into_iter().zip(targets) {
            if !modern
                || plan.permits_binding(
                    principal.caller(),
                    1,
                    KEY,
                    std::slice::from_ref(&target),
                )?
            {
                kept.push(row);
                facts.push(target);
                if kept.len() > window.size {
                    break;
                }
            }
        }
        if kept.len() > window.size || exhausted || !modern {
            break;
        }
        // A full unread batch is progress, not an empty returned inventory.
        // Continue the original ordered query until a real lookahead or exhaustion.
        after = next;
    }
    let more = kept.len() > window.size;
    kept.truncate(window.size);
    facts.truncate(window.size);
    Ok((kept, facts, more))
}

pub(super) fn list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: OrgTokenListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let window = Window::of(&query)?;
        let org = ctx.id("id")?;
        let (mut plan, _) = governance(ctx, conn).await?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        authorization::require_projected(
            conn,
            &principal,
            org,
            PermissionName::Exact(Permission::MembersManage),
            Act::Use,
            Policy::default(),
            caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some()),
        )
        .await?;
        let (rows, facts, more) = selected(conn, &principal, &plan, org, window).await?;
        if facts.is_empty() {
            plan.bind_empty(1)?;
        } else {
            plan.bind(1, KEY, &facts)?;
        }
        let next_cursor = if more {
            rows.last().map(|row| row.key.id.to_string())
        } else {
            None
        };
        let maximum = orch_settings::get::<i64>(conn, Place::Node(org), keys::TOKENS_MAX_DAYS)
            .await
            .map_err(ApiError::internal)?;
        let users: Vec<_> = rows.iter().map(|row| row.key.user_id).collect();
        let people = authorization::refusals::user_refs(conn, &users).await?;
        let mut info = Vec::with_capacity(rows.len());
        let mut context = Vec::with_capacity(rows.len());
        for row in rows {
            let user = people
                .get(&row.key.user_id)
                .cloned()
                .ok_or_else(ApiError::not_found)?;
            let allowed = maximum.is_none_or(|days| {
                row.key
                    .expires_at
                    .is_some_and(|end| end - row.key.created_at <= chrono::Duration::days(days))
            });
            info.push(credentials::key_info(row.key)?);
            context.push((user, row.owner_kind == "service", row.refused_here, allowed));
        }
        // The existing credential metadata rule may name a stored restriction;
        // optional key-scope Read bounds this without adding a universal Node View gate.
        credentials::key_names(conn, principal.caller(), &mut plan, &mut info).await?;
        let items = info
            .into_iter()
            .zip(context)
            .map(
                |(info, (user, service, refused_here, lifetime_allowed))| OrgToken {
                    info,
                    user,
                    service,
                    refused_here,
                    lifetime_allowed,
                },
            )
            .collect();
        let read = credentials::Read {
            plan: plan.clone(),
            data: Page { items, next_cursor },
        };
        Ok(plan.with_data(read))
    })
}

pub(super) async fn get(ctx: Ctx, _query: OrgTokenListQuery, (): ()) -> ApiResult<Page<OrgToken>> {
    crate::key_operations::snapshot_read::run(ctx, snapshot).await
}

async fn snapshot(ctx: Ctx) -> ApiResult<Page<OrgToken>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let read = ctx.operation_data::<credentials::Read<Page<OrgToken>>>()?;
    let data = credentials::complete(&ctx, &mut tx, &principal, read.as_ref()).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(data)
}
