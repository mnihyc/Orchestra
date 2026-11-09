//! Actual stored archive custody and selected native metadata/name sources.

use super::*;
use crate::key_operations::{native, targets};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::{KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    artifact_sources::{self as sources, Source},
    operation_sources::Declaration,
};
use orch_authz::{Caller, engine::Graph};
use orch_db::queries::exports::ExportRow;
use orch_engine::exports::PreparedExportViews;
use std::collections::{BTreeSet, HashMap};

pub(super) struct Archive;

pub(super) const NODE_SOURCES: [(ObjectKind, Source); 4] = [
    (ObjectKind::Org, Source::Orgs),
    (ObjectKind::Project, Source::Projects),
    (ObjectKind::Topic, Source::Topics),
    (ObjectKind::Thread, Source::Threads),
];

fn row(value: &PreparedExportViews) -> ApiResult<&ExportRow> {
    let [row] = value.rows() else {
        return Err(ApiError::internal(
            "the native archive reader did not select one stored row",
        ));
    };
    Ok(row)
}

impl native::Selected for Archive {
    type Source = Source;
    type Query = ();
    type Value = PreparedExportViews;
    type Reply = Export;
    const DECLARATION: &'static Declaration<Source> = &sources::EXPORT_GET;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(PreparedExportViews, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let reader = Principal::load(conn, caller.clone(), ctx.app.clock.time())
                .await
                .map_err(ApiError::internal)?;
            let stored = match ctx
                .app
                .engine
                .read_export(conn, &reader, ctx.id("id")?)
                .await
            {
                Ok(row) => row,
                Err(error) => return Err(failed(conn, error).await),
            };
            let primary = if native::modern(caller) {
                Some(native::Facts::one(
                    Source::Artifact,
                    KeyTargetKind::Identity(KeyIdentityKind::Export),
                    targets::from_export(conn, &stored).await?,
                ))
            } else {
                None
            };
            prepare_views(ctx, conn, caller, Self::DECLARATION, vec![stored], primary).await
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::not_found());
            }
            match ctx
                .app
                .engine
                .read_export(conn, principal, ctx.id("id")?)
                .await
            {
                Ok(_) => Ok(()),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a PreparedExportViews,
    ) -> BoxFuture<'a, ApiResult<Export>> {
        Box::pin(async move {
            match value.render() {
                Ok(mut views) => views.pop().ok_or_else(ApiError::not_found),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a (),
        opened: &'a PreparedExportViews,
        fresh: &'a PreparedExportViews,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let old = row(opened)?;
            let now = row(fresh)?;
            Ok(preserves(old, now) && opened.named_places().is_subset(&fresh.named_places()))
        })
    }
}

async fn prepare_views(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    rows: Vec<ExportRow>,
    primary: Option<native::Facts<Source>>,
) -> ApiResult<(PreparedExportViews, Vec<native::Facts<Source>>)> {
    let mut facts = primary.into_iter().collect::<Vec<_>>();
    let root = native::bind_known(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &root).await?;
    let mut value = match ctx
        .app
        .engine
        .prepare_export_views(conn, &principal, rows)
        .await
    {
        Ok(value) => value,
        Err(error) => return Err(failed(conn, error).await),
    };
    if native::modern(caller) {
        let nodes = value.named_places();
        let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
        let (names, allowed) =
            native::node_name_facts(&root, declaration, caller, &graph, &nodes, &NODE_SOURCES)?;
        value.bound_names(&allowed);
        facts.extend(names);
    }
    Ok((value, facts))
}

fn preserves(old: &ExportRow, now: &ExportRow) -> bool {
    old.id == now.id
        && old.user_id == now.user_id
        && old.node_id == now.node_id
        && old.request == now.request
        && old.job_id == now.job_id
        && old.created_at == now.created_at
        && old
            .blob
            .as_ref()
            .is_none_or(|blob| now.blob.as_ref() == Some(blob))
}

pub(super) struct Archives;

pub(super) struct PageViews {
    views: PreparedExportViews,
    next_cursor: Option<String>,
}

impl native::Selected for Archives {
    type Source = Source;
    type Query = PageQuery;
    type Value = PageViews;
    type Reply = Page<Export>;
    const DECLARATION: &'static Declaration<Source> = &sources::EXPORT_LIST;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(PageViews, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let modern = native::modern(caller);
            let plan = native::bind_known(ctx, Self::DECLARATION, &[])?;
            let limit = usize::try_from(query.limit()).map_err(ApiError::internal)?;
            let mut after = query.cursor.clone();
            let mut selected = Vec::new();
            let mut targets = Vec::new();
            let next_cursor;
            loop {
                let (rows, next) = match ctx
                    .app
                    .engine
                    .exports_of(
                        conn,
                        caller.user(),
                        after.as_deref(),
                        i64::from(query.limit()),
                    )
                    .await
                {
                    Ok(value) => value,
                    Err(error) => return Err(failed(conn, error).await),
                };
                if !modern {
                    selected = rows;
                    next_cursor = next;
                    break;
                }
                let nodes = rows
                    .iter()
                    .filter_map(|row| row.node_id)
                    .collect::<BTreeSet<_>>();
                let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
                    .await
                    .map_err(ApiError::internal)?;
                for row in rows {
                    let target = targets::from_export_graph(&graph, &row)?;
                    if native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::Artifact,
                        KeyTargetKind::Identity(KeyIdentityKind::Export),
                        &target,
                    )? {
                        selected.push(row);
                        targets.push(target);
                    }
                    if selected.len() > limit {
                        break;
                    }
                }
                if selected.len() > limit {
                    selected.truncate(limit);
                    targets.truncate(limit);
                    next_cursor = selected.last().map(orch_engine::exports::metadata_cursor);
                    break;
                }
                let Some(next) = next else {
                    next_cursor = None;
                    break;
                };
                if after.as_deref() == Some(next.as_str()) {
                    return Err(ApiError::internal(
                        "the native archive metadata cursor did not advance",
                    ));
                }
                after = Some(next);
            }
            let primary = modern.then(|| {
                native::Facts::collection(
                    Source::Artifact,
                    if targets.is_empty() {
                        Vec::new()
                    } else {
                        vec![(KeyTargetKind::Identity(KeyIdentityKind::Export), targets)]
                    },
                )
            });
            let (views, facts) =
                prepare_views(ctx, conn, caller, Self::DECLARATION, selected, primary).await?;
            Ok((PageViews { views, next_cursor }, facts))
        })
    }

    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a PageQuery,
        opened: &'a PageViews,
    ) -> BoxFuture<'a, ApiResult<(PageViews, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ids = opened
                .views
                .rows()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>();
            let rows =
                orch_db::queries::exports::selected_of_user(conn, caller.user(), &ids).await?;
            let nodes = rows
                .iter()
                .filter_map(|row| row.node_id)
                .collect::<BTreeSet<_>>();
            let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
            let targets = rows
                .iter()
                .map(|row| targets::from_export_graph(&graph, row))
                .collect::<ApiResult<Vec<_>>>()?;
            let primary = Some(native::Facts::collection(
                Source::Artifact,
                if targets.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Identity(KeyIdentityKind::Export), targets)]
                },
            ));
            let (views, facts) =
                prepare_views(ctx, conn, caller, Self::DECLARATION, rows, primary).await?;
            Ok((
                PageViews {
                    views,
                    next_cursor: opened.next_cursor.clone(),
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a PageViews,
    ) -> BoxFuture<'a, ApiResult<Page<Export>>> {
        Box::pin(async move {
            match value.views.render() {
                Ok(items) => Ok(Page {
                    items,
                    next_cursor: value.next_cursor.clone(),
                }),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a PageQuery,
        opened: &'a PageViews,
        fresh: &'a PageViews,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let now: HashMap<_, _> = fresh.views.rows().iter().map(|row| (row.id, row)).collect();
            Ok(opened
                .views
                .rows()
                .iter()
                .all(|old| now.get(&old.id).is_some_and(|row| preserves(old, row)))
                && opened
                    .views
                    .named_places()
                    .is_subset(&fresh.views.named_places()))
        })
    }
}
