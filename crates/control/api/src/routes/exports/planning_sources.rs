//! Native export plan candidates, scoped before aggregate construction and response rendering.
use super::*;
use crate::key_operations::{native, targets};
use crate::registry::OperationPlan;
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    export_sources::{self as sources, Source},
    exports::ExportPlan,
    operation_sources::Declaration,
};
use orch_authz::{Caller, engine::Graph};
use orch_engine::exports::{ExportBounds, PreparedExportPlan};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Plan;
type Subject = (KeyTargetKind, Id);
#[derive(PartialEq, Eq)]
struct Association {
    node: Id,
    exportable: bool,
}
pub(super) struct Ready {
    plan: PreparedExportPlan,
    subjects: BTreeMap<Subject, Association>,
    names: BTreeSet<Id>,
}
const NAMES: [(ObjectKind, Source); 4] = [
    (ObjectKind::Org, Source::Orgs),
    (ObjectKind::Project, Source::Projects),
    (ObjectKind::Topic, Source::Topics),
    (ObjectKind::Thread, Source::Threads),
];

struct Selection<'a> {
    plan: &'a OperationPlan,
    caller: &'a Caller,
    opened: Option<&'a Ready>,
    subjects: BTreeMap<Subject, Association>,
    groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
}
impl Selection<'_> {
    fn keep(&mut self, target: KeyResolvedTarget, node: Id, exportable: bool) -> ApiResult<bool> {
        let id = target
            .object
            .id
            .ok_or_else(|| ApiError::internal("a native export object has no identity"))?;
        let subject = (target.object.kind, id);
        if self
            .opened
            .is_some_and(|opened| !opened.subjects.contains_key(&subject))
            || !native::permits(
                self.plan,
                &sources::PREFLIGHT,
                self.caller,
                &Source::Contents,
                target.object.kind,
                &target,
            )?
        {
            return Ok(false);
        }
        self.subjects
            .insert(subject, Association { node, exportable });
        self.groups
            .entry(target.object.kind)
            .or_default()
            .push(target);
        Ok(true)
    }
}
async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    request: &ExportRequest,
    opened: Option<&Ready>,
) -> ApiResult<(Ready, Vec<native::Facts<Source>>)> {
    let place = ctx.id("id")?;
    let target = targets::node_identity(conn, place).await?;
    let root = native::Facts::one(Source::Place, target.object.kind, target);
    let plan = native::bind_known(ctx, &sources::PREFLIGHT, std::slice::from_ref(&root))?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    let mut prepared = match ctx
        .app
        .engine
        .prepare_export_plan(conn, &principal, place, request)
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => return Err(failed(conn, error).await),
    };
    if !native::modern(caller) {
        return Ok((
            Ready {
                plan: prepared,
                subjects: BTreeMap::new(),
                names: BTreeSet::new(),
            },
            Vec::new(),
        ));
    }
    let raw = &prepared.selection;
    let mut graph = Graph::load(
        conn,
        &raw.scope.iter().map(|node| node.id).collect::<Vec<_>>(),
    )
    .await
    .map_err(ApiError::internal)?;
    let mut selected = Selection {
        plan: &plan,
        caller,
        opened,
        subjects: BTreeMap::new(),
        groups: BTreeMap::new(),
    };
    let mut bounds = ExportBounds::default();
    for id in raw
        .threads
        .iter()
        .filter(|id| raw.access.reads_conversations.contains(id))
    {
        let target = orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
            .ok_or_else(ApiError::not_found)?;
        if selected.keep(target, *id, raw.access.exports_conversations.contains(id))? {
            bounds.threads.insert(*id);
        }
    }
    let ids = raw
        .chats
        .iter()
        .filter(|chat| raw.access.reads_chats.contains(&chat.node_id))
        .map(|chat| chat.chat_id)
        .collect::<Vec<_>>();
    let chats = orch_db::queries::chats::chats(conn, &ids).await?;
    let chat_targets = targets::from_chat_rows(conn, &chats).await?;
    for chat in chats {
        let Some(node) = chat.node_id else {
            return Err(ApiError::not_found());
        };
        if !raw
            .chats
            .iter()
            .any(|row| row.chat_id == chat.id && row.node_id == node)
        {
            return Err(ApiError::not_found());
        }
        let target = chat_targets
            .get(&chat.id)
            .cloned()
            .ok_or_else(ApiError::not_found)?;
        if selected.keep(target, node, raw.access.exports_chats.contains(&node))? {
            bounds.chats.insert(chat.id);
        }
    }
    let ids = raw
        .documents
        .iter()
        .filter(|document| raw.access.reads_documents.contains(&document.id))
        .map(|document| document.id)
        .collect::<Vec<_>>();
    for document in orch_authz::documents::read_into(conn, &mut graph, &ids)
        .await
        .map_err(ApiError::internal)?
    {
        if document.kind != ObjectKind::Document
            || document.deleted
            || !raw
                .documents
                .iter()
                .any(|row| row.id == document.id && row.node_id == document.node)
        {
            continue;
        }
        let site =
            orch_authz::documents::site_of(&graph, &document).ok_or_else(ApiError::not_found)?;
        let target = targets::from_site(&graph, &site)?;
        if selected.keep(
            target,
            document.node,
            raw.access.exports_documents.contains(&document.id),
        )? {
            bounds.documents.insert(document.id);
        }
    }
    if prepared.wanted.package {
        for node in raw.scope.iter().filter(|node| {
            node.kind != "thread"
                && raw.included.contains(&node.id)
                && raw.viewable.contains_key(&node.id)
        }) {
            let target = orch_engine::operation_scope::targets::from_node_identity(&graph, node.id)
                .ok_or_else(ApiError::not_found)?;
            selected.keep(target, node.id, true)?;
        }
    }
    let native_names = raw
        .viewable
        .keys()
        .copied()
        .filter(|id| opened.is_none_or(|opened| opened.names.contains(id)))
        .collect::<BTreeSet<_>>();
    let (_, names) = native::node_name_facts(
        &plan,
        &sources::PREFLIGHT,
        caller,
        &graph,
        &native_names,
        &NAMES,
    )?;
    bounds.nodes = names.iter().copied().collect();
    prepared
        .selection
        .restrict(&bounds, prepared.wanted.package);
    let needed_names = prepared
        .selection
        .viewable
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let (name_facts, names) = native::node_name_facts(
        &plan,
        &sources::PREFLIGHT,
        caller,
        &graph,
        &needed_names,
        &NAMES,
    )?;
    let mut facts = vec![
        root,
        native::Facts::collection(Source::Contents, selected.groups.into_iter().collect()),
    ];
    facts.extend(name_facts);
    Ok((
        Ready {
            plan: prepared,
            subjects: selected.subjects,
            names,
        },
        facts,
    ))
}
impl native::Selected for Plan {
    type Source = Source;
    type Query = ((), ExportRequest);
    type Value = Ready;
    type Reply = ExportPlan;
    const DECLARATION: &'static Declaration<Source> = &sources::PREFLIGHT;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, &query.1, None))
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
        opened: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare(ctx, conn, caller, &query.1, Some(opened)))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .export_place_seen(conn, principal, ctx.id("id")?)
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<ExportPlan>> {
        Box::pin(async move {
            match ctx.app.engine.render_export_plan(conn, &value.plan).await {
                Ok(plan) => Ok(plan),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        old: &'a Ready,
        fresh: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.plan.selection.place.id == fresh.plan.selection.place.id
                && old.subjects == fresh.subjects
                && old.names.is_subset(&fresh.names))
        })
    }
}
