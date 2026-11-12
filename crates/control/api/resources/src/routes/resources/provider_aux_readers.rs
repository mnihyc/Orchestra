//! The existing provider settings and bound-thread readers, with current operation operands.
//!
//! Values and counts stay the provider's ordinary projection. Live names and actual
//! result threads use the same source bounds as the rest of the scoped read producers.

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use crate::routes::access::page::same;

use crate::routes::nodes::invalid;

use crate::routes::nodes::view as node_view;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::PageQuery;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::resources::ProviderThread;

use orch_api_contract::resources::ProviderThreadListReply;

use orch_api_contract::settings::ResolvedSetting;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::engine::Graph;

use orch_authz::resources;

use orch_db::Conn;

use orch_db::queries::threads::binding as thread_rows;

use orch_db::queries::threads::binding::ProviderThread as ThreadRow;

use orch_settings::Place;

use orch_settings::SettingView;

use orch_settings::resolve_all;

use orch_settings_contract::registry::Scope;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use std::collections::BTreeSet;

use std::collections::HashSet;
const PROVIDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Provider);

const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "provider_aux_names_orgs"),
        (ObjectKind::Project, "provider_aux_names_projects"),
        (ObjectKind::Topic, "provider_aux_names_topics"),
        (ObjectKind::Thread, "provider_aux_names_threads"),
    ],
    containers: None,
};

struct Settings {
    views: Vec<SettingView>,
    manages: bool,
    owner: bool,
    names: NameBounds,
}

struct Threads {
    rows: Vec<ThreadRow>,
    hidden: i64,
    next_cursor: Option<String>,
    names: NameBounds,
}

async fn base(ctx: &Ctx, conn: &mut Conn) -> ApiResult<OperationPlan> {
    let id = ctx.id("id")?;
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id,
        },
    )
    .await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(0, PROVIDER, &[target])?;
    Ok(plan)
}

pub(super) fn settings<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = base(ctx, conn).await?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let id = ctx.id("id")?;
        super::providers::provider_for(conn, &principal, id, Permission::ProviderView).await?;
        let manages = resources::check(
            conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Provider,
                id,
            },
            Permission::ProviderManage,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .is_allowed();
        let owner = resources::effective(
            conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Provider,
                id,
            },
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .owner;
        let views = resolve_all(conn, Place::Provider(id))
            .await
            .map_err(ApiError::internal)?;
        let names = NameSources {
            nodes: views.iter().flat_map(SettingView::nodes).collect(),
            ..Default::default()
        };
        let names = projection_names(conn, caller, &mut plan, names, &NAMES).await?;
        Ok(plan.with_data(Settings {
            views,
            manages,
            owner,
            names,
        }))
    })
}

/// Same ordinary setting resolver, owner-only writable fields and node-name supplier.
pub(super) async fn settings_get(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<ResolvedSetting>> {
    crate::key_operations::snapshot_read::run(ctx, settings_snapshot).await
}

async fn settings_snapshot(ctx: Ctx) -> ApiResult<Vec<ResolvedSetting>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<Settings>()?;
    let nodes: Vec<Id> = prepared
        .views
        .iter()
        .flat_map(SettingView::nodes)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let refs = if prepared.names.limited {
        node_view::refs_with_name_sources(
            &mut tx,
            &principal,
            &nodes,
            Policy::default(),
            &prepared.names.nodes,
        )
        .await?
    } else {
        node_view::refs(&mut tx, &principal, &nodes, Policy::default()).await?
    };
    let shown = prepared
        .views
        .clone()
        .into_iter()
        .map(|view| {
            let writable = prepared.manages
                && view.def().set_at(Scope::Provider)
                && (!matches!(
                    view.def().key,
                    keys::RUNTIME_AFTER_UP | keys::RUNTIME_TEMPLATE_ID
                ) || prepared.owner);
            let setting = view.into_setting(writable).map_node(|node| {
                refs.get(&node).cloned().unwrap_or(NodeRef {
                    thread_display: None,
                    id: node,
                    kind: NodeKind::Org,
                    name: None,
                    redacted: true,
                })
            });
            same(&setting).ok_or_else(|| ApiError::internal("a setting does not read as the API's"))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(shown)
}

pub(super) fn threads<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: PageQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = base(ctx, conn).await?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let id = ctx.id("id")?;
        super::providers::provider_for(conn, &principal, id, Permission::ProviderView).await?;
        let bound = thread_rows::threads_on_provider(conn, id).await?;
        let bound_ids: Vec<_> = bound.iter().map(|thread| thread.thread_id).collect();
        let readable: HashSet<_> = orch_authz::nodes::holding(
            conn,
            &principal,
            &bound_ids,
            authorization::VIEW,
            orch_authz::Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|thread| thread.id)
        .collect();
        // This existing aggregate belongs to the admitted provider. Scope-filtered result
        // rows do not add to it, and no names/content of ordinary-unreadable threads appear.
        let hidden = i64::try_from(bound.len() - readable.len()).unwrap_or(i64::MAX);
        let graph = Graph::load(conn, &bound_ids)
            .await
            .map_err(ApiError::internal)?;
        let mut visible = Vec::new();
        for row in bound
            .into_iter()
            .filter(|row| readable.contains(&row.thread_id))
        {
            let Some(site) = graph.site_of(row.thread_id) else {
                continue;
            };
            let target = targets::from_site(&graph, &site)?;
            if target.object.kind != THREAD {
                return Err(ApiError::not_found());
            }
            if plan.permits_binding(caller, 1, THREAD, std::slice::from_ref(&target))? {
                visible.push((row, target));
            }
        }
        let after = query
            .cursor
            .as_deref()
            .map(|cursor| {
                cursor
                    .parse::<Id>()
                    .map_err(|_| invalid("cursor", "is not a cursor this list gave"))
            })
            .transpose()?;
        let limit = usize::try_from(query.limit()).unwrap_or(usize::MAX);
        let mut rest = visible
            .into_iter()
            .filter(|(row, _)| after.is_none_or(|after| row.thread_id > after));
        let page: Vec<_> = rest.by_ref().take(limit).collect();
        let next_cursor = rest
            .next()
            .and_then(|_| page.last().map(|(row, _)| row.thread_id.to_string()));
        let (rows, targets): (Vec<ThreadRow>, Vec<KeyResolvedTarget>) = page.into_iter().unzip();
        if targets.is_empty() {
            plan.bind_empty(1)?;
        } else {
            plan.bind(1, THREAD, &targets)?;
        }
        let names = NameSources {
            nodes: rows
                .iter()
                .flat_map(|row| [row.thread_id, row.topic_id, row.org_id])
                .collect(),
            ..Default::default()
        };
        let names = projection_names(conn, caller, &mut plan, names, &NAMES).await?;
        Ok(plan.with_data(Threads {
            rows,
            hidden,
            next_cursor,
            names,
        }))
    })
}

/// Same ordinary visible-thread projection; only admitted results enter the existing page.
pub(super) async fn threads_get(
    ctx: Ctx,
    _query: PageQuery,
    (): (),
) -> ApiResult<ProviderThreadListReply> {
    crate::key_operations::snapshot_read::run(ctx, threads_snapshot).await
}

async fn threads_snapshot(ctx: Ctx) -> ApiResult<ProviderThreadListReply> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<Threads>()?;
    let nodes: Vec<_> = prepared
        .rows
        .iter()
        .flat_map(|row| [row.thread_id, row.topic_id, row.org_id])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let refs = if prepared.names.limited {
        node_view::refs_with_name_sources(
            &mut tx,
            &principal,
            &nodes,
            Policy::default(),
            &prepared.names.nodes,
        )
        .await?
    } else {
        node_view::refs(&mut tx, &principal, &nodes, Policy::default()).await?
    };
    let seen = |node: Id| refs.get(&node).filter(|each| !each.redacted).cloned();
    let owners: Vec<_> = prepared.rows.iter().map(|row| row.owner_id).collect();
    let owners = refusals::user_refs(&mut tx, &owners).await?;
    let redacted = |node: Id| NodeRef {
        thread_display: None,
        id: node,
        kind: NodeKind::Org,
        name: None,
        redacted: true,
    };
    let items = prepared
        .rows
        .iter()
        .map(|row| {
            Ok(ProviderThread {
                thread: seen(row.thread_id).unwrap_or_else(|| redacted(row.thread_id)),
                topic: refs
                    .get(&row.topic_id)
                    .cloned()
                    .unwrap_or_else(|| redacted(row.topic_id)),
                org: refs
                    .get(&row.org_id)
                    .cloned()
                    .unwrap_or_else(|| redacted(row.org_id)),
                state: row.state.clone(),
                blocked_reason: row.blocked_reason.clone(),
                owner: owners
                    .get(&row.owner_id)
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the owner of a thread is not there"))?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(ProviderThreadListReply {
        items,
        hidden: prepared.hidden,
        next_cursor: prepared.next_cursor.clone(),
    })
}
