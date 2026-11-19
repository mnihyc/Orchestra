//! Native administered-place window, with scope applied before its existing page boundary.

use super::*;
use crate::key_operations::{native, targets};
use crate::registry::OperationPlan;
use crate::routes::access::page::NameBounds;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyObjectRef, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::{
    operation_sources::Declaration,
    port_sources::{self as sources, Source},
};
use orch_authz::Caller;
use std::collections::BTreeMap;

const ENV: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);
const HOST: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Host);
const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);
const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::NamesOrgs),
    (ObjectKind::Project, Source::NamesProjects),
    (ObjectKind::Topic, Source::NamesTopics),
    (ObjectKind::Thread, Source::NamesThreads),
];

pub(super) struct Outside;
pub(super) struct Selected {
    rows: Vec<Opening>,
    next_cursor: Option<String>,
    names: NameBounds,
}
struct Opening {
    row: rows::Opening,
    environment: EnvironmentFull,
    state: (PortOpeningState, Option<String>),
    hosts: Vec<KeyResolvedTarget>,
    publishers: Vec<KeyResolvedTarget>,
    manage: bool,
    view: bool,
    names: BTreeSet<Id>,
    show: bool,
}

async fn require(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
    ctx.ordinary_node_administration(conn, principal, ctx.id("id")?)
        .await
}

fn collection(
    source: Source,
    family: KeyTargetKind,
    targets: impl IntoIterator<Item = KeyResolvedTarget>,
) -> native::Facts<Source> {
    let distinct: BTreeMap<KeyObjectRef, KeyResolvedTarget> = targets
        .into_iter()
        .map(|target| (target.object, target))
        .collect();
    native::Facts::collection(
        source,
        if distinct.is_empty() {
            Vec::new()
        } else {
            vec![(family, distinct.into_values().collect())]
        },
    )
}

impl native::Selected for Outside {
    type Source = Source;
    type Query = NodePortOpeningListQuery;
    type Value = Selected;
    type Reply = Page<PortOpening>;
    const DECLARATION: &'static Declaration<Source> = &sources::OUTSIDE;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(select_in(ctx, conn, caller, query, None))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal))
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Page<PortOpening>>> {
        Box::pin(render_in(ctx, conn, principal, value))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Selected,
        fresh: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            for old in &opened.rows {
                let Some(new) = fresh.rows.iter().find(|row| row.row.id == old.row.id) else {
                    return Ok(false);
                };
                if old.row.environment_id != new.row.environment_id
                    || old.row.place_id != new.row.place_id
                    || old.row.publish_machine_id != new.row.publish_machine_id
                    || old.row.host_id != new.row.host_id
                    || old.row.requested_by_thread != new.row.requested_by_thread
                    || old.row.admission != new.row.admission
                    || old
                        .hosts
                        .iter()
                        .map(|target| target.object)
                        .ne(new.hosts.iter().map(|target| target.object))
                    || old
                        .publishers
                        .iter()
                        .map(|target| target.object)
                        .ne(new.publishers.iter().map(|target| target.object))
                    || old.manage && !new.manage
                    || old.view && !new.view
                    || old.show && !new.show
                    || !old.names.is_subset(&new.names)
                {
                    return Ok(false);
                }
            }
            Ok(true)
        })
    }
}

/// The consuming overview supplies its own current operation scope and principal.
pub(super) async fn read_for_overview(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: NodePortOpeningListQuery,
    reading: &crate::routes::nodes::overview::Reading<'_>,
) -> ApiResult<Page<PortOpening>> {
    let (selected, _) = select_in(
        ctx,
        conn,
        principal.caller(),
        &query,
        Some((principal, reading)),
    )
    .await?;
    require(ctx, conn, principal).await?;
    render_in(ctx, conn, principal, &selected).await
}

async fn select_in(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &NodePortOpeningListQuery,
    overview: Option<(&Principal, &crate::routes::nodes::overview::Reading<'_>)>,
) -> ApiResult<(Selected, Vec<native::Facts<Source>>)> {
    let id = ctx.id("id")?;
    let limited = native::modern(caller);
    let declaration = <Outside as native::Selected>::DECLARATION;
    let mut primary = Vec::new();
    if limited && overview.is_none() {
        let root = targets::node(conn, id).await?;
        primary.push(native::Facts::one(
            Source::OutsideNode,
            root.object.kind,
            root,
        ));
    }
    let principal = match overview {
        Some((principal, _)) => principal.clone(),
        None => {
            let known = native::bind_known(ctx, declaration, &primary)?;
            native::preparation_principal(ctx, conn, caller, &known).await?
        }
    };
    require(ctx, conn, &principal).await?;
    let plan = if overview.is_none() {
        Some(OperationPlan::new(ctx.route, declaration.operation)?)
    } else {
        None
    };
    let size = limit(query.limit);
    let mut after = cursor::<Id>(query.cursor.as_deref())?;
    let mut selected = Vec::new();
    // The existing limit+1 is also the native forward scan batch. Every candidate
    // advances once; no arbitrary capped/exhaustive list policy is introduced.
    'window: loop {
        let candidates = rows::outside(conn, id, after, (size + 1) as i64).await?;
        let exhausted = candidates.len() < size + 1;
        for row in candidates {
            after = Some(row.id);
            let environment = environment(conn, row.environment_id).await?;
            let environment_target = if limited || overview.is_some() {
                Some(
                    targets::page_object(
                        conn,
                        orch_access_contract::decisions::ObjectRef {
                            kind: ObjectKind::Environment,
                            id: environment.id,
                        },
                    )
                    .await?,
                )
            } else {
                None
            };
            if let Some(target) = &environment_target {
                let allowed = match overview {
                    Some((_, reading)) => reading.source_target(ENV, target)?,
                    None => native::permits(
                        plan.as_ref().ok_or_else(|| {
                            ApiError::internal("the port read declaration is unavailable")
                        })?,
                        declaration,
                        caller,
                        &Source::OutsideEnvironments,
                        ENV,
                        target,
                    )?,
                };
                if !allowed {
                    continue;
                }
            }
            let (hosts, publishers) = if limited || overview.is_some() {
                opening_operation::foreign_sources(conn, &row).await?
            } else {
                (Vec::new(), Vec::new())
            };
            if limited || overview.is_some() {
                let mut admitted = true;
                for (family, source, targets) in [
                    (HOST, Source::OutsideHosts, &hosts),
                    (MACHINE, Source::OutsidePublishers, &publishers),
                ] {
                    for target in targets {
                        admitted &= match overview {
                            Some((_, reading)) => reading.source_target(family, target)?,
                            None => native::permits(
                                plan.as_ref().ok_or_else(|| {
                                    ApiError::internal("the port read declaration is unavailable")
                                })?,
                                declaration,
                                caller,
                                &source,
                                family,
                                target,
                            )?,
                        };
                    }
                }
                if !admitted {
                    continue;
                }
            }
            selected.push((row, environment, environment_target, hosts, publishers));
            if selected.len() > size {
                break 'window;
            }
        }
        if !limited || exhausted {
            break;
        }
    }
    let next_id = (selected.len() > size).then(|| selected[size - 1].0.id);
    let next_cursor = next_id.map(|id| id.to_string());
    selected.truncate(size);
    let mut native_rows = Vec::new();
    let mut envs = Vec::new();
    let mut host_sources = Vec::new();
    let mut publisher_sources = Vec::new();
    let mut all_names = BTreeSet::new();
    for (row, environment, environment_target, hosts, publishers) in selected {
        let kind: PortOpeningKind = parse(&row.kind)?;
        let state = effective_opening_state(
            ctx,
            conn,
            &principal,
            &row,
            &environment,
            kind,
            u16::try_from(row.port).map_err(ApiError::internal)?,
        )
        .await?;
        let show = state.0 != PortOpeningState::Closed
            && query.kind.is_none_or(|value| value == kind)
            && query.state.is_none_or(|value| value == state.0);
        if !show && next_id != Some(row.id) {
            continue;
        }
        let manage = core::holds(conn, &principal, environment.id, Permission::PortsManage)
            .await
            .map_err(ApiError::internal)?;
        let view = core::holds(conn, &principal, environment.id, Permission::PortsView)
            .await
            .map_err(ApiError::internal)?
            || (row.kind == "place"
                && if let Some(place) = row.place_id {
                    core::place_view(conn, &principal, place)
                        .await
                        .unwrap_or(false)
                } else {
                    false
                });
        let names = if show {
            opening_operation::name_sources(&row).nodes
        } else {
            BTreeSet::new()
        };
        all_names.extend(names.iter().copied());
        envs.extend(environment_target);
        host_sources.extend(hosts.iter().cloned());
        publisher_sources.extend(publishers.iter().cloned());
        native_rows.push(Opening {
            row,
            environment,
            state,
            hosts,
            publishers,
            manage,
            view,
            names,
            show,
        });
    }
    let mut facts = primary;
    facts.extend([
        collection(Source::OutsideEnvironments, ENV, envs),
        collection(Source::OutsideHosts, HOST, host_sources),
        collection(Source::OutsidePublishers, MACHINE, publisher_sources),
    ]);
    let mut bounds = NameBounds::default();
    if let Some((principal, reading)) = overview {
        let ids: Vec<_> = all_names.iter().copied().collect();
        bounds = reading.name_bounds(conn, &ids).await?;
        let named_ids: Vec<_> = if bounds.limited {
            bounds.nodes.iter().copied().collect()
        } else {
            ids
        };
        let visible = bounds.refs(conn, principal, &named_ids).await?;
        for node in visible.values() {
            reading.reference(conn, node).await?;
        }
        for row in &mut native_rows {
            row.names.retain(|id| {
                visible
                    .get(id)
                    .is_some_and(|node| !node.redacted && node.name.is_some())
            });
        }
    } else if limited {
        let plan = plan
            .as_ref()
            .ok_or_else(|| ApiError::internal("the port read declaration is unavailable"))?;
        let graph =
            orch_authz::engine::Graph::load(conn, &all_names.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
        for row in &mut native_rows {
            let path_names: BTreeSet<Id> = row
                .names
                .iter()
                .flat_map(|id| {
                    graph
                        .get(*id)
                        .into_iter()
                        .flat_map(|node| node.path.iter().copied())
                })
                .chain(row.names.iter().copied())
                .collect();
            row.names = path_names;
        }
        all_names = native_rows
            .iter()
            .flat_map(|row| row.names.iter().copied())
            .collect();
        let (name_facts, names) =
            native::node_name_facts(plan, declaration, caller, &graph, &all_names, NAMES)?;
        facts.extend(name_facts);
        // Admit the complete factual read before asking ordinary name visibility.
        // A foreign named place need not lie under the administered selection root.
        let complete = native::bind(ctx, declaration, &facts)?;
        let admitted = complete.admit(caller)?;
        let reader =
            authorization::load_principal(conn, admitted.caller, ctx.app.clock.time()).await?;
        let reader = crate::sign_in::second_step::hold(conn, ctx.user()?, reader).await?;
        let visible = crate::routes::nodes::view::refs(
            conn,
            &reader,
            &names.iter().copied().collect::<Vec<_>>(),
            Policy::default(),
        )
        .await?;
        for row in &mut native_rows {
            row.names.retain(|id| {
                names.contains(id)
                    && visible
                        .get(id)
                        .is_some_and(|view| !view.redacted && view.name.is_some())
            });
        }
        bounds.limited = true;
        bounds.nodes = names;
    }
    Ok((
        Selected {
            rows: native_rows,
            next_cursor,
            names: bounds,
        },
        if limited && overview.is_none() {
            facts
        } else {
            Vec::new()
        },
    ))
}

async fn render_in(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Selected,
) -> ApiResult<Page<PortOpening>> {
    let mut items = Vec::new();
    for selected in value.rows.iter().filter(|row| row.show) {
        items.push(
            opening_view_prepared(
                ctx,
                conn,
                principal,
                &selected.row,
                &selected.environment,
                false,
                Some(&value.names),
                Some(selected.state.clone()),
            )
            .await?,
        );
    }
    Ok(Page {
        items,
        next_cursor: value.next_cursor.clone(),
    })
}
