//! Native history rows are scoped before the output window; live names remain optional.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use crate::routes::access::page::NameBounds;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::access_sources as sources;

use orch_api_contract::access_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use std::collections::BTreeMap;

use std::collections::BTreeSet;
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
        Box::pin(select_filtered(ctx, conn, caller, query))
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

async fn select_page(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &api::HistoryListQuery,
) -> ApiResult<(Selected, Vec<native::Facts<Source>>)> {
    let named = subject::named(ctx)?;
    let limit = usize::try_from(
        query
            .limit
            .unwrap_or(orch_api_contract::common::DEFAULT_LIMIT),
    )
    .map_err(ApiError::internal)?;
    if limit == 0 {
        return Err(invalid("limit", "must be greater than zero"));
    }
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
    let known = native::bind_known(
        ctx,
        <History as native::Selected>::DECLARATION,
        std::slice::from_ref(&root),
    )?;
    let principal = known.preparation_principal(ctx, conn, caller).await?;
    subject::require_view(conn, &principal, named.thing, policy).await?;
    let plan = OperationPlan::new(
        ctx.route,
        <History as native::Selected>::DECLARATION.operation,
    )?;
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
                    orch_engine::operation_scope::targets::from_node_identity(&graph, thread)
                else {
                    continue;
                };
                let family = KeyTargetKind::Object(ObjectKind::Thread);
                if target.object.kind != family
                    || !native::permits(
                        &plan,
                        <History as native::Selected>::DECLARATION,
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
        <History as native::Selected>::DECLARATION,
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
                row.actor_id
                    .filter(|id| row.actor_kind == "agent" && selected_threads.contains(id)),
            )
        })
        .collect::<BTreeSet<_>>();
    let graph = Graph::load(conn, &ids.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(authorization::restoration_error)?;
    let (names, allowed) = native::node_name_facts(
        &plan,
        <History as native::Selected>::DECLARATION,
        caller,
        &graph,
        &ids,
        NAMES,
    )?;
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
}

async fn select_filtered(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &api::HistoryListQuery,
) -> ApiResult<(Selected, Vec<native::Facts<Source>>)> {
    let text = query.q.as_deref().unwrap_or("").trim();
    if text.is_empty() || query.upload.is_some() {
        return select_page(ctx, conn, caller, query).await;
    }
    let words = orch_db::queries::search::Query::parse(text);
    if words.terms().is_empty() {
        return Err(invalid("q", "give at least one word or quoted phrase"));
    }
    let wanted = usize::try_from(
        query
            .limit
            .unwrap_or(orch_api_contract::common::DEFAULT_LIMIT),
    )
    .map_err(ApiError::internal)?;
    if wanted == 0 {
        return Err(invalid("limit", "must be greater than zero"));
    }
    let mut batches = orch_settings::get::<u64>(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::SEARCH_HISTORY_CANDIDATE_BATCHES,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("the history search work setting has no value"))?;
    let mut scan = query.clone();
    scan.limit = Some(
        query
            .limit
            .unwrap_or(orch_api_contract::common::DEFAULT_LIMIT)
            .max(orch_api_contract::common::DEFAULT_LIMIT),
    );
    let mut rows = Vec::new();
    let mut names = NameBounds::default();
    let mut all_facts = Vec::new();
    let continuation;
    loop {
        let (value, facts) = select_page(ctx, conn, caller, &scan).await?;
        let plan = native::bind_known(ctx, <History as native::Selected>::DECLARATION, &facts)?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        <History as native::Selected>::require(ctx, conn, &principal).await?;
        let visible = <History as native::Selected>::render(ctx, conn, &principal, &value).await?;
        let matched = visible
            .items
            .iter()
            .filter(|line| history_matches(&words, line))
            .map(|line| line.id)
            .collect::<BTreeSet<_>>();
        let next = value.next_cursor.clone();
        names.limited |= value.names.limited;
        names.nodes.extend(value.names.nodes);
        merge_facts(&mut all_facts, facts);
        let mut extra = false;
        for row in value.rows {
            if matched.contains(&row.id) {
                if rows.len() == wanted {
                    extra = true;
                    break;
                }
                rows.push(row);
            }
        }
        if extra {
            continuation = selected_history_cursor(&rows, true);
            break;
        }
        batches = batches.saturating_sub(1);
        if next.is_none() || batches == 0 || rows.len() == wanted {
            continuation = next;
            break;
        }
        if next == scan.cursor {
            return Err(ApiError::internal(
                "the history search repeated its native continuation",
            ));
        }
        scan.cursor = next;
    }
    let threads = rows.iter().filter_map(thread_of).collect::<BTreeSet<_>>();
    let named = rows
        .iter()
        .flat_map(|row| {
            row.node_id.into_iter().chain(thread_of(row)).chain(
                row.actor_id
                    .filter(|id| row.actor_kind == "agent" && threads.contains(id)),
            )
        })
        .collect::<BTreeSet<_>>();
    names.nodes.retain(|id| named.contains(id));
    for fact in &mut all_facts {
        if fact.source != Source::Page {
            let ids = if fact.source == Source::Threads {
                &threads
            } else {
                &named
            };
            for (_, targets) in &mut fact.groups {
                targets.retain(|target| target.object.id.is_some_and(|id| ids.contains(&id)));
            }
            fact.groups.retain(|(_, targets)| !targets.is_empty());
        }
    }
    Ok((
        Selected {
            rows,
            next_cursor: continuation,
            names,
        },
        all_facts,
    ))
}

fn merge_facts(all: &mut Vec<native::Facts<Source>>, more: Vec<native::Facts<Source>>) {
    for fact in more {
        if let Some(found) = all.iter_mut().find(|known| known.source == fact.source) {
            for (family, targets) in fact.groups {
                if let Some((_, held)) = found.groups.iter_mut().find(|(kind, _)| *kind == family) {
                    for target in targets {
                        if !held.iter().any(|known| known.object == target.object) {
                            held.push(target);
                        }
                    }
                } else {
                    found.groups.push((family, targets));
                }
            }
        } else {
            all.push(fact);
        }
    }
}

fn history_matches(words: &orch_db::queries::search::Query, line: &api::HistoryLine) -> bool {
    let actor = match &line.actor {
        Some(api::HistoryActor::User(user)) => format!("{} {}", user.display_name, user.username),
        Some(api::HistoryActor::Agent(agent)) => {
            if agent.agent.redacted {
                String::new()
            } else {
                agent.agent.name.clone().unwrap_or_default()
            }
        }
        None => String::new(),
    };
    let about = line.about.as_ref();
    words.matches(&format!(
        "{} {} {} {} {} {} {} {}",
        line.text,
        line.action,
        actor,
        about.and_then(|item| item.path.as_deref()).unwrap_or(""),
        about
            .and_then(|item| item.paths.as_ref())
            .map(|paths| paths.join(" "))
            .unwrap_or_default(),
        about
            .and_then(|item| item.thread.as_ref())
            .filter(|thread| !thread.redacted)
            .and_then(|thread| thread.name.as_deref())
            .unwrap_or(""),
        line.wording
            .as_ref()
            .and_then(|words| words.target.as_ref())
            .filter(|target| !target.redacted)
            .and_then(|target| target.name.as_deref())
            .unwrap_or(""),
        line.upload
            .as_ref()
            .map(|upload| {
                std::iter::once(upload.folder.as_str())
                    .chain(upload.files.iter().flat_map(|file| {
                        std::iter::once(file.name.as_str()).chain(file.path.as_deref())
                    }))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    ))
}
