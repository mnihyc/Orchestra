//! Stored native import custody, live labels and ordered saved source file names.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::artifact_sources as sources;

use orch_api_contract::artifact_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_authz::engine::Graph;

use orch_db::queries::imports::ImportLineRow;

use orch_db::queries::imports::ImportRow;

use orch_engine::imports::PreparedImportLines;

use orch_engine::imports::PreparedImportViews;

use orch_engine::imports::PreparedReportPage;

use orch_types::Id;

use std::collections::BTreeSet;

use std::collections::HashMap;
pub(super) const NODE_SOURCES: [(ObjectKind, Source); 4] = [
    (ObjectKind::Org, Source::Orgs),
    (ObjectKind::Project, Source::Projects),
    (ObjectKind::Topic, Source::Topics),
    (ObjectKind::Thread, Source::Threads),
];

async fn load(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
) -> ApiResult<(ImportRow, Vec<native::Facts<Source>>, Principal)> {
    let reader = Principal::load(conn, caller.clone(), ctx.app.clock.time())
        .await
        .map_err(ApiError::internal)?;
    let row = match ctx
        .app
        .engine
        .read_import(conn, &reader, ctx.id("id")?)
        .await
    {
        Ok(row) => row,
        Err(error) => return Err(failed(conn, error).await),
    };
    let facts = if native::modern(caller) {
        vec![native::Facts::one(
            Source::Artifact,
            KeyTargetKind::Identity(KeyIdentityKind::Import),
            targets::from_import(conn, &row).await?,
        )]
    } else {
        Vec::new()
    };
    let known = native::bind_known(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    Ok((row, facts, principal))
}

pub(super) fn preserves(old: &ImportRow, now: &ImportRow) -> bool {
    old.id == now.id
        && old.user_id == now.user_id
        && old.node_id == now.node_id
        && old.client_id == now.client_id
        && old.request == now.request
        && old.job_id == now.job_id
        && old.files == now.files
        && old.created_at == now.created_at
}

fn row(value: &PreparedImportViews) -> ApiResult<&ImportRow> {
    let [row] = value.rows() else {
        return Err(ApiError::internal(
            "the native import metadata did not select one stored row",
        ));
    };
    Ok(row)
}

pub(super) async fn require_own(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<()> {
    require_own_import(
        &ctx.app.engine,
        ctx.user()?.id(),
        ctx.id("id")?,
        conn,
        principal,
    )
    .await
}

pub(super) async fn require_read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<()> {
    if ctx.user()?.id() != principal.user() {
        return Err(ApiError::not_found());
    }
    match ctx
        .app
        .engine
        .read_import(conn, principal, ctx.id("id")?)
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => Err(failed(conn, error).await),
    }
}

pub(super) async fn require_own_import(
    engine: &orch_engine::Engine,
    user: Id,
    import: Id,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<()> {
    if user != principal.user() {
        return Err(ApiError::not_found());
    }
    match engine.import_of(conn, principal.user(), import).await {
        Ok(_) => Ok(()),
        Err(error) => Err(failed(conn, error).await),
    }
}

pub(super) struct Imported;

impl native::Selected for Imported {
    type Source = Source;
    type Query = ();
    type Value = PreparedImportViews;
    type Reply = Import;
    const DECLARATION: &'static Declaration<Source> = &sources::IMPORT_GET;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(PreparedImportViews, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (row, mut facts, principal) = load(ctx, conn, caller, Self::DECLARATION).await?;
            let mut value = match ctx
                .app
                .engine
                .prepare_import_views(conn, &principal, vec![row])
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            if native::modern(caller) {
                let plan = native::bind_known(ctx, Self::DECLARATION, &facts)?;
                let nodes = value.named_places();
                let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
                    .await
                    .map_err(ApiError::internal)?;
                let (names, allowed) = native::node_name_facts(
                    &plan,
                    Self::DECLARATION,
                    caller,
                    &graph,
                    &nodes,
                    &NODE_SOURCES,
                )?;
                value.bound_names(&allowed);
                facts.extend(names);
            }
            Ok((value, facts))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_read(ctx, conn, principal))
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a PreparedImportViews,
    ) -> BoxFuture<'a, ApiResult<Import>> {
        Box::pin(async move {
            match value.render() {
                Ok(mut values) => values.pop().ok_or_else(ApiError::not_found),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a (),
        opened: &'a PreparedImportViews,
        fresh: &'a PreparedImportViews,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(preserves(row(opened)?, row(fresh)?)
                && opened.named_places().is_subset(&fresh.named_places()))
        })
    }
}

pub(super) struct Files;

pub(super) struct FilePage {
    row: ImportRow,
    items: Vec<String>,
    next_cursor: Option<String>,
}

impl native::Selected for Files {
    type Source = Source;
    type Query = PageQuery;
    type Value = FilePage;
    type Reply = Page<String>;
    const DECLARATION: &'static Declaration<Source> = &sources::IMPORT_FILES;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(FilePage, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (row, facts, principal) = load(ctx, conn, caller, Self::DECLARATION).await?;
            let (items, next_cursor) = match ctx
                .app
                .engine
                .import_file_names_for(
                    conn,
                    &principal,
                    row.id,
                    query.cursor.as_deref(),
                    i64::from(query.limit()),
                )
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            Ok((
                FilePage {
                    row,
                    items,
                    next_cursor,
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
        Box::pin(require_read(ctx, conn, principal))
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a FilePage,
    ) -> BoxFuture<'a, ApiResult<Page<String>>> {
        Box::pin(async move {
            Ok(Page {
                items: value.items.clone(),
                next_cursor: value.next_cursor.clone(),
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a PageQuery,
        opened: &'a FilePage,
        fresh: &'a FilePage,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(preserves(&opened.row, &fresh.row)
                && opened.items == fresh.items
                && opened.next_cursor == fresh.next_cursor)
        })
    }
}

pub(super) struct Report;

async fn report_names(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    mut value: PreparedReportPage,
    mut facts: Vec<native::Facts<Source>>,
) -> ApiResult<(PreparedReportPage, Vec<native::Facts<Source>>)> {
    if native::modern(caller) {
        let plan = native::bind_known(ctx, &sources::IMPORT_REPORT, &facts)?;
        let nodes = value.named_places();
        let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
        let (names, allowed) = native::node_name_facts(
            &plan,
            &sources::IMPORT_REPORT,
            caller,
            &graph,
            &nodes,
            &NODE_SOURCES,
        )?;
        value.bound_names(allowed);
        facts.extend(names);
    }
    Ok((value, facts))
}

impl native::Selected for Report {
    type Source = Source;
    type Query = ImportReportListQuery;
    type Value = PreparedReportPage;
    type Reply = Page<ImportReportRow>;
    const DECLARATION: &'static Declaration<Source> = &sources::IMPORT_REPORT;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a ImportReportListQuery,
    ) -> BoxFuture<'a, ApiResult<(PreparedReportPage, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (row, facts, _principal) = load(ctx, conn, caller, Self::DECLARATION).await?;
            let value = match ctx
                .app
                .engine
                .prepare_report_records(conn, row, query)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            report_names(ctx, conn, caller, value, facts).await
        })
    }

    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a ImportReportListQuery,
        opened: &'a PreparedReportPage,
    ) -> BoxFuture<'a, ApiResult<(PreparedReportPage, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let reader = Principal::load(conn, caller.clone(), ctx.app.clock.time())
                .await
                .map_err(ApiError::internal)?;
            let value = match ctx
                .app
                .engine
                .refresh_report_for(conn, &reader, opened)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let facts = vec![native::Facts::one(
                Source::Artifact,
                KeyTargetKind::Identity(KeyIdentityKind::Import),
                targets::from_import(conn, value.import()).await?,
            )];
            report_names(ctx, conn, caller, value, facts).await
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_read(ctx, conn, principal))
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a PreparedReportPage,
    ) -> BoxFuture<'a, ApiResult<Page<ImportReportRow>>> {
        Box::pin(async move {
            let (items, next_cursor) = value.render();
            Ok(Page { items, next_cursor })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a ImportReportListQuery,
        opened: &'a PreparedReportPage,
        fresh: &'a PreparedReportPage,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(preserves(opened.import(), fresh.import()) && opened.preserves_entries(fresh))
        })
    }
}

pub(super) struct History;

pub(super) struct HistoryPage {
    views: PreparedImportLines,
    next_cursor: Option<String>,
}

async fn prepare_history(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    rows: Vec<ImportLineRow>,
    primary: Vec<native::Facts<Source>>,
    next_cursor: Option<String>,
) -> ApiResult<(HistoryPage, Vec<native::Facts<Source>>)> {
    let mut facts = primary;
    let plan = native::bind_known(ctx, &sources::IMPORT_LIST, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    let mut views = match ctx
        .app
        .engine
        .prepare_import_lines(conn, &principal, rows)
        .await
    {
        Ok(value) => value,
        Err(error) => return Err(failed(conn, error).await),
    };
    if native::modern(caller) {
        let nodes = views.named_places();
        let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
        let (names, allowed) = native::node_name_facts(
            &plan,
            &sources::IMPORT_LIST,
            caller,
            &graph,
            &nodes,
            &NODE_SOURCES,
        )?;
        views.bound_names(&allowed);
        facts.extend(names);
    }
    Ok((HistoryPage { views, next_cursor }, facts))
}

impl native::Selected for History {
    type Source = Source;
    type Query = PageQuery;
    type Value = HistoryPage;
    type Reply = Page<ImportLine>;
    const DECLARATION: &'static Declaration<Source> = &sources::IMPORT_LIST;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(HistoryPage, Vec<native::Facts<Source>>)>> {
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
                    .import_line_rows(
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
                    let target = targets::from_import_line_graph(&graph, &row)?;
                    if native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::Artifact,
                        KeyTargetKind::Identity(KeyIdentityKind::Import),
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
                    next_cursor = selected.last().map(orch_engine::imports::history_cursor);
                    break;
                }
                let Some(next) = next else {
                    next_cursor = None;
                    break;
                };
                if after.as_deref() == Some(next.as_str()) {
                    return Err(ApiError::internal(
                        "the native import history cursor did not advance",
                    ));
                }
                after = Some(next);
            }
            let primary = if modern {
                vec![native::Facts::collection(
                    Source::Artifact,
                    if targets.is_empty() {
                        Vec::new()
                    } else {
                        vec![(KeyTargetKind::Identity(KeyIdentityKind::Import), targets)]
                    },
                )]
            } else {
                Vec::new()
            };
            prepare_history(ctx, conn, caller, selected, primary, next_cursor).await
        })
    }

    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a PageQuery,
        opened: &'a HistoryPage,
    ) -> BoxFuture<'a, ApiResult<(HistoryPage, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ids = opened
                .views
                .rows()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>();
            let rows = orch_db::queries::imports::selected_lines_of_user(conn, caller.user(), &ids)
                .await?;
            let nodes = rows
                .iter()
                .filter_map(|row| row.node_id)
                .collect::<BTreeSet<_>>();
            let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
            let targets = rows
                .iter()
                .map(|row| targets::from_import_line_graph(&graph, row))
                .collect::<ApiResult<Vec<_>>>()?;
            let primary = vec![native::Facts::collection(
                Source::Artifact,
                if targets.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Identity(KeyIdentityKind::Import), targets)]
                },
            )];
            prepare_history(ctx, conn, caller, rows, primary, opened.next_cursor.clone()).await
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
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a HistoryPage,
    ) -> BoxFuture<'a, ApiResult<Page<ImportLine>>> {
        Box::pin(async move {
            Ok(Page {
                items: value.views.render(),
                next_cursor: value.next_cursor.clone(),
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a PageQuery,
        opened: &'a HistoryPage,
        fresh: &'a HistoryPage,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let now: HashMap<_, _> = fresh.views.rows().iter().map(|row| (row.id, row)).collect();
            Ok(opened.views.rows().iter().all(|old| {
                now.get(&old.id).is_some_and(|row| {
                    old.node_id == row.node_id
                        && old.job_id == row.job_id
                        && old.first_file == row.first_file
                        && old.file_count == row.file_count
                        && old.created_at == row.created_at
                })
            }) && opened
                .views
                .named_places()
                .is_subset(&fresh.views.named_places()))
        })
    }
}
