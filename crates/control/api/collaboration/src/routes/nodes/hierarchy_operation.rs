//! Actual native tree facts; the shared adapter owns snapshots and publication.

use crate::key_operations::native;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::hierarchy_sources as sources;

use orch_api_contract::hierarchy_sources::Source;

use orch_api_contract::nodes::OrgTreeGetQuery;

use orch_api_contract::nodes::OrgTreeGetReply;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_types::Id;

use std::collections::BTreeMap;

use std::collections::BTreeSet;
pub(super) fn collection(source: Source, rows: Vec<KeyResolvedTarget>) -> native::Facts<Source> {
    let mut groups = BTreeMap::new();
    for row in rows {
        groups
            .entry(row.object.kind)
            .or_insert_with(Vec::new)
            .push(row);
    }
    native::Facts::collection(source, groups.into_iter().collect())
}

pub(super) fn permits(
    ctx: &Ctx,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    source: Source,
    family: KeyTargetKind,
    target: &KeyResolvedTarget,
) -> ApiResult<bool> {
    if !native::prepared_read(ctx, caller) {
        return Ok(true);
    }
    let plan = native::bind_known(ctx, declaration, &[])?;
    let usage = declaration
        .bindings
        .iter()
        .find(|b| b.source == source)
        .ok_or_else(|| ApiError::internal("the native hierarchy source is undeclared"))?
        .usage;
    native::permits_usage(&plan, declaration, caller, &source, usage, family, target)
}

pub(super) async fn names(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    wanted: NameSources,
) -> ApiResult<(NameBounds, native::Facts<Source>)> {
    let graph =
        orch_authz::engine::Graph::load(conn, &wanted.nodes.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
    let ids = graph.nodes().map(|n| n.id).collect::<Vec<_>>();
    let native_names =
        super::view::refs(conn, principal, &ids, orch_authz::Policy::default()).await?;
    let mut bounds = NameBounds {
        limited: native::prepared_read(ctx, caller),
        ..NameBounds::default()
    };
    let mut facts = Vec::new();
    for node in graph.nodes() {
        if native_names
            .get(&node.id)
            .is_some_and(|n| n.name.is_some() || n.thread_display.is_some())
            && let Some(target) =
                orch_engine::operation_scope::targets::from_node_identity(&graph, node.id)
            && permits(
                ctx,
                caller,
                declaration,
                Source::Names,
                target.object.kind,
                &target,
            )?
        {
            bounds.nodes.insert(node.id);
            facts.push(target);
        }
    }
    Ok((bounds, collection(Source::Names, facts)))
}

pub(super) struct Tree;

pub(super) struct PreparedTree {
    pub reply: OrgTreeGetReply,
    pub ids: BTreeSet<Id>,
    pub visible: BTreeSet<Id>,
    pub hidden: BTreeSet<Id>,
    pub docs: BTreeSet<Id>,
}

impl native::Selected for Tree {
    type Source = Source;
    type Query = OrgTreeGetQuery;
    type Value = PreparedTree;
    type Reply = OrgTreeGetReply;
    const DECLARATION: &'static Declaration<Source> = &sources::TREE;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(PreparedTree, Vec<native::Facts<Source>>)>> {
        Box::pin(super::tree::org_tree_in(ctx, conn, caller, query))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(value.reply.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _query: &'a Self::Query,
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if !old.ids.is_subset(&fresh.ids)
                || !old.visible.is_subset(&fresh.visible)
                || !old.docs.is_subset(&fresh.docs)
            {
                return Ok(false);
            }
            let managed = orch_authz::nodes::holding(
                conn,
                principal,
                &old.hidden.iter().copied().collect::<Vec<_>>(),
                orch_access_contract::permissions::PermissionName::Exact(
                    orch_access_contract::permissions::Permission::AccessManage,
                ),
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            Ok(old
                .hidden
                .iter()
                .all(|id| managed.iter().any(|row| row.id == *id)))
        })
    }
}

pub(super) async fn selected_rows(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    rows: Vec<orch_db::queries::hierarchy::TreeRow>,
) -> ApiResult<(
    Vec<orch_db::queries::hierarchy::TreeRow>,
    Vec<KeyResolvedTarget>,
)> {
    let graph =
        orch_authz::engine::Graph::load(conn, &rows.iter().map(|r| r.id).collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
    let mut selected = Vec::new();
    let mut targets = Vec::new();
    for row in rows {
        let target = orch_engine::operation_scope::targets::from_node_identity(&graph, row.id)
            .ok_or_else(ApiError::not_found)?;
        if permits(
            ctx,
            caller,
            &sources::TREE,
            Source::Results,
            target.object.kind,
            &target,
        )? {
            selected.push(row);
            targets.push(target);
        }
    }
    Ok((selected, targets))
}
