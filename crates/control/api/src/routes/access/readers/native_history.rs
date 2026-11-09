//! Native history rows are scoped before the output window; live names remain optional.

use super::*;
use crate::key_operations::{native, targets};
use crate::registry::OperationPlan;
use crate::routes::access::page::NameBounds;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::access_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Caller;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct History;
pub(crate) struct Selected {
    rows: Vec<AuditRow>,
    next_cursor: Option<String>,
    names: NameBounds,
}

const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::OrgNames),
    (ObjectKind::Project, Source::ProjectNames),
    (ObjectKind::Topic, Source::TopicNames),
    (ObjectKind::Thread, Source::ThreadNames),
];

fn thread_facts(targets: &BTreeMap<Id, KeyResolvedTarget>) -> native::Facts<Source> {
    let groups = if targets.is_empty() {
        Vec::new()
    } else {
        vec![(
            KeyTargetKind::Object(ObjectKind::Thread),
            targets.values().cloned().collect(),
        )]
    };
    native::Facts::collection(Source::Threads, groups)
}

impl native::Selected for History {
    type Source = Source;
    type Query = api::HistoryListQuery;
    type Value = Selected;
    type Reply = Page<api::HistoryLine>;
    const DECLARATION: &'static Declaration<Source> = &sources::HISTORY;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let named = subject::named(ctx)?;
            let page = PageQuery {
                limit: query.limit,
                cursor: query.cursor.clone(),
            };
            let limit = usize::try_from(page.limit()).map_err(ApiError::internal)?;
            let batch = i64::try_from(limit + 1).map_err(ApiError::internal)?;
            let mut after = history_after_query(query)?;
            if !native::modern(caller) {
                let mut rows = history_rows(conn, named, query, after, batch).await?;
                let more = rows.len() > limit;
                rows.truncate(limit);
                let next_cursor = selected_history_cursor(&rows, more);
                return Ok((
                    Selected {
                        rows,
                        next_cursor,
                        names: NameBounds::default(),
                    },
                    Vec::new(),
                ));
            }
            let (graph, site, policy) = found(conn, &named).await?;
            let (family, target) = match named.thing {
                Thing::Node(_) => (
                    KeyTargetKind::Object(site.kind),
                    targets::from_site(&graph, &site)?,
                ),
                Thing::Other(object) => (
                    KeyTargetKind::Object(object.kind),
                    crate::routes::resources::projection::single::subject(conn, object).await?,
                ),
            };
            let root = native::Facts::one(Source::Page, family, target);
            let known = native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&root))?;
            let principal = known.preparation_principal(ctx, conn, caller).await?;
            subject::require_view(conn, &principal, named.thing, policy).await?;
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let mut rows = Vec::new();
            let mut threads = BTreeMap::new();
            loop {
                let window = history_rows(conn, named, query, after, batch).await?;
                let exhausted = window.len() < limit + 1;
                let next = window.last().map(|row| AuditCursor {
                    at: row.at,
                    id: row.id,
                });
                let ids = window.iter().filter_map(thread_of).collect::<BTreeSet<_>>();
                let graph = Graph::load(conn, &ids.into_iter().collect::<Vec<_>>())
                    .await
                    .map_err(authorization::restoration_error)?;
                for row in window {
                    if let Some(thread) = thread_of(&row) {
                        let Some(target) =
                            orch_engine::operation_scope::targets::from_node_identity(
                                &graph, thread,
                            )
                        else {
                            continue;
                        };
                        let family = KeyTargetKind::Object(ObjectKind::Thread);
                        if target.object.kind != family
                            || !native::permits(
                                &plan,
                                Self::DECLARATION,
                                caller,
                                &Source::Threads,
                                family,
                                &target,
                            )?
                        {
                            continue;
                        }
                        threads.insert(thread, target);
                    }
                    rows.push(row);
                    if rows.len() > limit {
                        break;
                    }
                }
                if rows.len() > limit || exhausted {
                    break;
                }
                after = next;
            }
            let more = rows.len() > limit;
            rows.truncate(limit);
            let next_cursor = selected_history_cursor(&rows, more);
            let selected_threads = rows.iter().filter_map(thread_of).collect::<BTreeSet<_>>();
            threads.retain(|id, _| selected_threads.contains(id));
            let known = native::bind_known(
                ctx,
                Self::DECLARATION,
                &[root.clone(), thread_facts(&threads)],
            )?;
            let principal = known.preparation_principal(ctx, conn, caller).await?;
            let visible = visible_threads(conn, &principal, &rows).await?;
            threads.retain(|id, _| visible.contains(id));
            let ids = rows
                .iter()
                .filter(|row| thread_of(row).is_none_or(|thread| visible.contains(&thread)))
                .flat_map(|row| {
                    row.node_id.into_iter().chain(thread_of(row)).chain(
                        row.actor_id.filter(|id| {
                            row.actor_kind == "agent" && selected_threads.contains(id)
                        }),
                    )
                })
                .collect::<BTreeSet<_>>();
            let graph = Graph::load(conn, &ids.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(authorization::restoration_error)?;
            let (names, allowed) =
                native::node_name_facts(&plan, Self::DECLARATION, caller, &graph, &ids, NAMES)?;
            let mut facts = vec![root, thread_facts(&threads)];
            facts.extend(names);
            Ok((
                Selected {
                    rows,
                    next_cursor,
                    names: NameBounds {
                        limited: true,
                        nodes: allowed,
                        ..NameBounds::default()
                    },
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
        Box::pin(async move {
            let named = subject::named(ctx)?;
            let (_, _, policy) = found(conn, &named).await?;
            subject::require_view(conn, principal, named.thing, policy).await
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let items = if value.names.limited {
                lines_with_names(conn, principal, &value.rows, Some(&value.names)).await?
            } else {
                lines(conn, principal, &value.rows).await?
            };
            Ok(Page {
                items,
                next_cursor: value.next_cursor.clone(),
            })
        })
    }
}
