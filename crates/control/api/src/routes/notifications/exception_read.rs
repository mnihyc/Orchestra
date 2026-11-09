//! Native manual exception rows, inherited-level sources and protected path names.

use crate::key_operations::native;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan};
use crate::routes::nodes::view;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::{Page, PageQuery};
use orch_api_contract::notification_sources::{self as sources, Source};
use orch_api_contract::notifications::NotificationException;
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Caller, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::notifications::exceptions::{self as rows, ExceptionRow};
use orch_types::Id;
use std::collections::BTreeSet;

pub(super) struct Exceptions;
pub(super) struct Selected {
    rows: Vec<ExceptionRow>,
    next_cursor: Option<String>,
    names: Option<BTreeSet<Id>>,
}

const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::ExceptionOrgNames),
    (ObjectKind::Project, Source::ExceptionProjectNames),
    (ObjectKind::Topic, Source::ExceptionTopicNames),
    (ObjectKind::Thread, Source::ExceptionThreadNames),
];

fn group(groups: &mut Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>, target: KeyResolvedTarget) {
    let family = target.object.kind;
    if let Some((_, targets)) = groups.iter_mut().find(|(kind, _)| *kind == family) {
        if !targets
            .iter()
            .any(|existing| existing.object == target.object)
        {
            targets.push(target);
        }
    } else {
        groups.push((family, vec![target]));
    }
}

fn cursor(row: &ExceptionRow) -> String {
    URL_SAFE_NO_PAD.encode(format!("{}{}", super::exceptions::CURSOR, row.node_id))
}

async fn facts(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    rows: &[ExceptionRow],
) -> ApiResult<(Vec<native::Facts<Source>>, BTreeSet<Id>)> {
    let ids = rows
        .iter()
        .flat_map(|row| std::iter::once(row.node_id).chain(row.default_from))
        .collect::<Vec<_>>();
    let graph = orch_authz::engine::Graph::load(conn, &ids)
        .await
        .map_err(authorization::restoration_error)?;
    let mut primary = Vec::new();
    let mut defaults = Vec::new();
    let mut names = BTreeSet::new();
    for row in rows {
        group(
            &mut primary,
            orch_engine::operation_scope::targets::from_node_identity(&graph, row.node_id)
                .ok_or_else(ApiError::not_found)?,
        );
        if let Some(id) = row.default_from {
            group(
                &mut defaults,
                orch_engine::operation_scope::targets::from_node_identity(&graph, id)
                    .ok_or_else(ApiError::not_found)?,
            );
        }
        if let Some(node) = graph.get(row.node_id) {
            names.extend(node.path.iter().copied());
        }
    }
    let plan = OperationPlan::new(ctx.route, sources::EXCEPTIONS.operation)?;
    let (name_facts, allowed) =
        native::node_name_facts(&plan, &sources::EXCEPTIONS, caller, &graph, &names, NAMES)?;
    let mut facts = vec![
        native::Facts::collection(Source::ExceptionNodes, primary),
        native::Facts::collection(Source::ExceptionDefaults, defaults),
    ];
    facts.extend(name_facts);
    Ok((facts, allowed))
}

impl native::Selected for Exceptions {
    type Source = Source;
    type Query = PageQuery;
    type Value = Selected;
    type Reply = Page<NotificationException>;
    const DECLARATION: &'static Declaration<Source> = &sources::EXCEPTIONS;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let limit = usize::try_from(query.limit()).map_err(ApiError::internal)?;
            let mut after = super::exceptions::after(query)?;
            let mut selected = Vec::new();
            let modern = native::modern(caller);
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            loop {
                let batch =
                    rows::page(conn, caller.user(), after, i64::from(query.limit()) + 1).await?;
                let exhausted = batch.len() <= limit;
                let next = batch.last().map(|row| row.node_id);
                if !modern {
                    let mut rows = batch;
                    let more = rows.len() > limit;
                    rows.truncate(limit);
                    let next_cursor = more.then(|| rows.last().map(cursor)).flatten();
                    return Ok((
                        Selected {
                            rows,
                            next_cursor,
                            names: None,
                        },
                        Vec::new(),
                    ));
                }
                let ids = batch
                    .iter()
                    .flat_map(|row| std::iter::once(row.node_id).chain(row.default_from))
                    .collect::<Vec<_>>();
                let graph = orch_authz::engine::Graph::load(conn, &ids)
                    .await
                    .map_err(authorization::restoration_error)?;
                let mut candidates = Vec::new();
                let mut primary = Vec::new();
                let mut defaults = Vec::new();
                for row in batch {
                    let Some(target) = orch_engine::operation_scope::targets::from_node_identity(
                        &graph,
                        row.node_id,
                    ) else {
                        continue;
                    };
                    if !native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::ExceptionNodes,
                        target.object.kind,
                        &target,
                    )? {
                        continue;
                    }
                    let default = row
                        .default_from
                        .map(|id| {
                            orch_engine::operation_scope::targets::from_node_identity(&graph, id)
                                .ok_or_else(ApiError::not_found)
                        })
                        .transpose()?;
                    if let Some(default) = &default
                        && !native::permits(
                            &plan,
                            Self::DECLARATION,
                            caller,
                            &Source::ExceptionDefaults,
                            default.object.kind,
                            default,
                        )?
                    {
                        continue;
                    }
                    group(&mut primary, target);
                    if let Some(default) = default {
                        group(&mut defaults, default);
                    }
                    candidates.push(row);
                }
                let known_facts = [
                    native::Facts::collection(Source::ExceptionNodes, primary),
                    native::Facts::collection(Source::ExceptionDefaults, defaults),
                ];
                let known = native::bind_known(ctx, Self::DECLARATION, &known_facts)?;
                let principal = known.preparation_principal(ctx, conn, caller).await?;
                let ids = candidates.iter().map(|row| row.node_id).collect::<Vec<_>>();
                let visible =
                    view::viewable_ways(conn, &principal, &ids, Policy::default()).await?;
                for row in candidates {
                    if visible.contains_key(&row.node_id) {
                        selected.push(row);
                    }
                    if selected.len() > limit {
                        break;
                    }
                }
                if selected.len() > limit || exhausted {
                    break;
                }
                after = next;
            }
            let more = selected.len() > limit;
            selected.truncate(limit);
            let next_cursor = more.then(|| selected.last().map(cursor)).flatten();
            let (facts, names) = facts(ctx, conn, caller, &selected).await?;
            Ok((
                Selected {
                    rows: selected,
                    next_cursor,
                    names: Some(names),
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
                return Err(ApiError::internal("the exception reader changed account"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Page<NotificationException>>> {
        Box::pin(async move {
            let ids = value.rows.iter().map(|row| row.node_id).collect::<Vec<_>>();
            let mut paths = match &value.names {
                Some(names) => {
                    view::viewable_ways_with_name_sources(
                        conn,
                        principal,
                        &ids,
                        Policy::default(),
                        names,
                    )
                    .await?
                }
                None => view::viewable_ways(conn, principal, &ids, Policy::default()).await?,
            };
            let mut items = Vec::new();
            for row in &value.rows {
                if let Some(path) = paths.remove(&row.node_id)
                    && let Some(item) = super::exceptions::project(row, path)?
                {
                    items.push(item);
                }
            }
            Ok(Page {
                items,
                next_cursor: value.next_cursor.clone(),
            })
        })
    }
}
