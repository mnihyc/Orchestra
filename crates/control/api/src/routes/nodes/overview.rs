//! One bounded native opening. Selection, ordinary visibility and credential
//! scope precede each projection; all suppliers share the current snapshot.

use super::{child_list_in, node_id, projection, read_sources};
use crate::key_operations::targets;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx, OperationFuture, OperationPlan};
use chrono::{Datelike, Duration};
use orch_access_contract::key_scope::{
    KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::Listed;
use orch_api_contract::nodes::{
    Node, NodeChildListQuery, NodeRef, OrgOverview, OrgOverviewGetQuery, OrgOverviewResponse,
    OverviewPart,
};
use orch_authz::{Caller, Policy};
use orch_db::{Conn, DbError};
use orch_types::Id;
use std::collections::BTreeSet;
use std::sync::Mutex;

/// Factual sources of this exact declaration, shared with the native readers.
/// No wider principal is reconstructed and no serialized response is scrubbed.
pub(crate) struct Reading<'a> {
    caller: &'a Caller,
    delivery: bool,
    plan: OperationPlan,
    pub(crate) nodes: projection::Projection<'a>,
    primary: Mutex<Vec<KeyResolvedTarget>>,
    sources: Mutex<Vec<(KeyTargetKind, KeyResolvedTarget)>>,
}

impl Reading<'_> {
    pub(crate) fn delivery_allowed(&self) -> bool {
        self.delivery
    }
    fn remember(rows: &Mutex<Vec<KeyResolvedTarget>>, row: KeyResolvedTarget) -> ApiResult<()> {
        rows.lock()
            .map_err(|_| ApiError::internal("the read sources are unavailable"))?
            .push(row);
        Ok(())
    }

    pub(crate) async fn select_nodes(
        &self,
        conn: &mut Conn,
        ids: &[Id],
    ) -> ApiResult<BTreeSet<Id>> {
        let graph = orch_authz::engine::Graph::load(conn, ids)
            .await
            .map_err(ApiError::internal)?;
        let live: Vec<_> = ids
            .iter()
            .copied()
            .filter(|id| graph.site_of(*id).is_some())
            .collect();
        Ok(self
            .nodes
            .selected(conn, &live)
            .await?
            .iter()
            .filter_map(|row| row.object.id)
            .collect())
    }

    pub(crate) async fn remember_node_ids(&self, conn: &mut Conn, ids: &[Id]) -> ApiResult<()> {
        for id in ids {
            Self::remember(&self.primary, targets::node(conn, *id).await?)?;
        }
        Ok(())
    }

    async fn remember_nodes(&self, conn: &mut Conn, nodes: &[Node]) -> ApiResult<()> {
        self.remember_node_ids(conn, &nodes.iter().map(node_id).collect::<Vec<_>>())
            .await?;
        for node in nodes {
            for (object, target) in read_sources::Sources::of(node)?.targets(conn).await? {
                self.remember_source(KeyTargetKind::Object(object.kind), target)?;
            }
        }
        Ok(())
    }

    pub(crate) async fn visible(&self, conn: &mut Conn, ids: &[Id]) -> ApiResult<Vec<Node>> {
        let nodes = self.nodes.visible(conn, ids).await?;
        self.remember_nodes(conn, &nodes).await?;
        Ok(nodes)
    }

    fn remember_source(&self, family: KeyTargetKind, target: KeyResolvedTarget) -> ApiResult<()> {
        self.sources
            .lock()
            .map_err(|_| ApiError::internal("the read sources are unavailable"))?
            .push((family, target));
        Ok(())
    }

    pub(crate) fn source_target(
        &self,
        family: KeyTargetKind,
        target: &KeyResolvedTarget,
    ) -> ApiResult<bool> {
        let allowed =
            self.plan
                .permits_binding(self.caller, 1, family, std::slice::from_ref(target))?;
        if allowed {
            self.remember_source(family, target.clone())?;
        }
        Ok(allowed)
    }

    /// The native selector supplies an actual typed identity before reading its
    /// protected fields. Scope is checked before its ordinary projection.
    pub(crate) async fn object(
        &self,
        conn: &mut Conn,
        kind: ObjectKind,
        id: Id,
    ) -> ApiResult<bool> {
        let target = match targets::object(
            conn,
            KeyObjectRef {
                kind: KeyTargetKind::Object(kind),
                id: Some(id),
            },
        )
        .await
        {
            Ok(target) => target,
            Err(error) if matches!(error.problem.status, 404 | 410) => return Ok(false),
            Err(error) => return Err(error),
        };
        self.source_target(KeyTargetKind::Object(kind), &target)
    }

    pub(crate) async fn node_source(&self, conn: &mut Conn, id: Id) -> ApiResult<bool> {
        let target = match targets::node(conn, id).await {
            Ok(target) => target,
            Err(error) if matches!(error.problem.status, 404 | 410) => return Ok(false),
            Err(error) => return Err(error),
        };
        let allowed = self.plan.permits_binding(
            self.caller,
            1,
            target.object.kind,
            std::slice::from_ref(&target),
        )?;
        if allowed {
            self.remember_source(target.object.kind, target)?;
        }
        Ok(allowed)
    }

    /// Resolve neutral current paths first; the native name producer receives
    /// this bound before it reads protected labels, never a broader DTO to scrub.
    pub(crate) async fn name_bounds(
        &self,
        conn: &mut Conn,
        ids: &[Id],
    ) -> ApiResult<crate::routes::access::page::NameBounds> {
        let limited = self
            .caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some());
        let mut bounds = crate::routes::access::page::NameBounds {
            limited,
            ..Default::default()
        };
        if !limited {
            return Ok(bounds);
        }
        let graph = orch_authz::engine::Graph::load(conn, ids)
            .await
            .map_err(ApiError::internal)?;
        let all: BTreeSet<_> = ids
            .iter()
            .flat_map(|id| {
                graph
                    .get(*id)
                    .into_iter()
                    .flat_map(|node| node.path.iter().copied())
            })
            .collect();
        for id in all {
            let Some(site) = graph.site_of(id) else {
                continue;
            };
            let target = targets::from_site(&graph, &site)?;
            if self.plan.permits_binding(
                self.caller,
                1,
                target.object.kind,
                std::slice::from_ref(&target),
            )? {
                bounds.nodes.insert(id);
            }
        }
        Ok(bounds)
    }

    /// Record the actual reference selected by the bounded native producer.
    pub(crate) async fn reference(&self, conn: &mut Conn, node: &NodeRef) -> ApiResult<()> {
        if node.redacted || (node.name.is_none() && node.thread_display.is_none()) {
            return Ok(());
        }
        let target = targets::node(conn, node.id).await?;
        if !self.plan.permits_binding(
            self.caller,
            1,
            target.object.kind,
            std::slice::from_ref(&target),
        )? {
            return Err(ApiError::internal(
                "the native reference exceeded its declared source",
            ));
        }
        self.remember_source(target.object.kind, target)
    }

    fn finish(&self) -> ApiResult<OperationPlan> {
        let mut plan = self.plan.clone();
        let primary = self
            .primary
            .lock()
            .map_err(|_| ApiError::internal("the read sources are unavailable"))?;
        let mut seen = BTreeSet::new();
        projection::bind(
            &mut plan,
            0,
            projection::groups(
                primary
                    .iter()
                    .filter(|row| seen.insert((row.object.kind, row.object.id)))
                    .cloned()
                    .map(|row| (row.object.kind, row)),
            ),
        )?;
        let sources = self
            .sources
            .lock()
            .map_err(|_| ApiError::internal("the read sources are unavailable"))?;
        let mut seen = BTreeSet::new();
        projection::bind(
            &mut plan,
            1,
            projection::groups(
                sources
                    .iter()
                    .filter(|(family, row)| seen.insert((*family, row.object.kind, row.object.id)))
                    .cloned(),
            ),
        )?;
        Ok(plan)
    }
}

fn part<T>(ctx: &Ctx, answer: ApiResult<T>, bounded: bool) -> OverviewPart<T> {
    match answer {
        Ok(data) => OverviewPart::Ready { data },
        Err(error) => {
            let error = if bounded {
                authorization::problem_without_content(error)
            } else {
                error
            };
            let (problem, retry_after) = error.into_parts_for(&ctx.request_id);
            OverviewPart::Failed {
                problem,
                retry_after,
            }
        }
    }
}

struct PreparedOverview {
    data: OrgOverviewResponse,
    response: OperationPlan,
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: OrgOverviewGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        prepare_inner(ctx, conn, caller, query)
            .await
            .map_err(|error| {
                if caller
                    .grant()
                    .is_some_and(|grant| grant.operation_scope().is_some())
                {
                    authorization::problem_without_content(error)
                } else {
                    error
                }
            })
    })
}

async fn prepare_inner(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: OrgOverviewGetQuery,
) -> ApiResult<OperationPlan> {
    let id = ctx.id("id")?;
    let target = targets::node(conn, id).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
        return Err(ApiError::not_found());
    }
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(2, target.object.kind, std::slice::from_ref(&target))?;
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let bounded = caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some());
    authorization::require_projected(
        conn,
        &principal,
        id,
        authorization::VIEW,
        orch_authz::Act::Use,
        Policy::default(),
        bounded,
    )
    .await?;

    if let Some(requested) = &query.resolve_ids {
        if query.participating.is_some()
            || query.from.is_some()
            || query.to.is_some()
            || query.usage_by.is_some()
        {
            return Err(ApiError::invalid(
                "resolve_ids",
                "cannot be combined with overview controls",
            ));
        }
        let limit = usize::try_from(orch_api_contract::common::DEFAULT_LIMIT).unwrap_or(usize::MAX);
        let raw: Vec<_> = requested.split(',').take(limit + 1).collect();
        if raw.len() > limit {
            return Err(ApiError::invalid("resolve_ids", "must fit one native page"));
        }
        let mut seen = BTreeSet::new();
        let ids = raw
            .into_iter()
            .map(|id| {
                id.parse::<Id>()
                    .map_err(|_| ApiError::invalid("resolve_ids", "contains an invalid node ID"))
            })
            .collect::<ApiResult<Vec<_>>>()?;
        let ids: Vec<_> = ids.into_iter().filter(|id| seen.insert(*id)).collect();
        for slot in 3..plan.variant().facets.len() {
            plan.bind_empty(slot)?;
        }
        let reading = Reading {
            caller,
            delivery: false,
            nodes: projection::Projection::with_plan(ctx, caller, plan.clone()),
            plan,
            primary: Mutex::new(Vec::new()),
            sources: Mutex::new(Vec::new()),
        };
        let allowed = reading.select_nodes(conn, &ids).await?;
        let selected: Vec<_> = ids.into_iter().filter(|id| allowed.contains(id)).collect();
        let nodes = reading.visible(conn, &selected).await?;
        let response = reading.finish()?;
        return Ok(response.clone().with_data(PreparedOverview {
            data: OrgOverviewResponse::Places { nodes },
            response,
        }));
    }

    // The account and outside listing are genuinely optional sources. Missing
    // key coverage omits them rather than lending an unrelated Read/Manage.
    let account = targets::account(conn, principal.user()).await?;
    let setup = plan.permits_binding(
        caller,
        3,
        account.object.kind,
        std::slice::from_ref(&account),
    )?;
    let outside =
        plan.permits_binding(caller, 4, target.object.kind, std::slice::from_ref(&target))?;
    let delivery_kind = KeyTargetKind::Identity(KeyIdentityKind::Notification);
    let delivery =
        plan.permits_binding(caller, 5, delivery_kind, std::slice::from_ref(&account))?;
    let mut reading = Reading {
        caller,
        delivery,
        nodes: projection::Projection::with_plan(ctx, caller, plan.clone()),
        plan,
        primary: Mutex::new(Vec::new()),
        sources: Mutex::new(Vec::new()),
    };
    let org = match reading.visible(conn, &[id]).await?.pop() {
        Some(Node::Org(org)) => org,
        _ => return Err(ApiError::not_found()),
    };
    let window = child_list_in(
        ctx,
        &NodeChildListQuery {
            participating: query.participating,
            ..Default::default()
        },
        conn,
        &principal,
        Some(&reading.nodes),
    )
    .await?;
    let nodes: Vec<_> = window
        .page
        .items
        .iter()
        .filter_map(|row| match row {
            Listed::Visible(row) => Some(row.content.clone()),
            _ => None,
        })
        .collect();
    reading.remember_nodes(conn, &nodes).await?;
    let hidden: Vec<_> = window
        .page
        .items
        .iter()
        .filter_map(|row| match row {
            Listed::Hidden(row) => Some(row.id),
            _ => None,
        })
        .chain(window.more)
        .collect();
    reading.remember_node_ids(conn, &hidden).await?;
    let recent = crate::routes::search::names_in(
        ctx,
        conn,
        &principal,
        orch_api_contract::search::NameSearchListQuery {
            kinds: Some("thread".into()),
            limit: Some(orch_api_contract::search::NAME_SEARCH_LIMIT_MAX),
            org_id: Some(org.base.org_id),
            ..Default::default()
        },
        Some(&reading),
    )
    .await?;
    let setup = setup
        && org.org_kind != orch_api_contract::nodes::OrgType::Personal
        && !org.base.archived
        && org.base.parent_id.is_none()
        && org.depth == 0
        && org.base.access.owns;
    if setup {
        reading
            .plan
            .bind(3, account.object.kind, std::slice::from_ref(&account))?;
    } else {
        reading.plan.bind_empty(3)?;
    }
    let outside = outside && !nodes.is_empty() && org.base.access.administers;
    if outside {
        reading
            .plan
            .bind(4, target.object.kind, std::slice::from_ref(&target))?;
    } else {
        reading.plan.bind_empty(4)?;
    }
    let (first_steps, outside_status) = if setup {
        match crate::routes::first_steps::read_in(
            ctx,
            conn,
            &principal,
            orch_api_contract::first_steps::FirstStepsGetQuery { org_id: Some(id) },
            Some(org.clone()),
            Some(&reading),
        )
        .await
        {
            Ok(observed) => (
                OverviewPart::Ready {
                    data: observed.facts,
                },
                observed
                    .outside
                    .map_or(OverviewPart::Omitted, |data| OverviewPart::Ready { data }),
            ),
            Err(error) => (part(ctx, Err(error), bounded), OverviewPart::Omitted),
        }
    } else {
        (OverviewPart::Omitted, OverviewPart::Omitted)
    };
    if matches!(&outside_status, OverviewPart::Ready { .. }) {
        reading
            .plan
            .bind(5, delivery_kind, std::slice::from_ref(&account))?;
    } else {
        reading.plan.bind_empty(5)?;
    }
    let nonempty = !nodes.is_empty();
    let today = ctx.now.date_naive();
    let from = query.from.unwrap_or_else(|| {
        (today - Duration::days(i64::from(today.weekday().num_days_from_monday()))).to_string()
    });
    let to = query.to.unwrap_or_else(|| today.to_string());
    let by = query
        .usage_by
        .unwrap_or(orch_api_contract::usage::UsageDimension::Model);
    if !matches!(
        by,
        orch_api_contract::usage::UsageDimension::Model
            | orch_api_contract::usage::UsageDimension::Provider
            | orch_api_contract::usage::UsageDimension::Agent
            | orch_api_contract::usage::UsageDimension::Environment
    ) {
        return Err(ApiError::invalid("usage_by", "is not a usage board split"));
    }
    let by = serde_json::to_value(by)
        .map_err(ApiError::internal)?
        .as_str()
        .ok_or_else(|| ApiError::internal("the split has no name"))?
        .to_owned();
    let usage = if nonempty {
        part(
            ctx,
            crate::routes::usage::figures::read_in(
                ctx,
                conn,
                &principal,
                orch_api_contract::usage::UsageGetQuery {
                    health: None,
                    turn_id: None,
                    org_id: None,
                    measure: None,
                    effort: None,
                    tier: None,
                    tier_unknown: None,
                    program: None,
                    outcome: None,
                    scope: format!("node:{id}"),
                    from,
                    to,
                    bucket: None,
                    series_by: None,
                    breakdowns: Some(format!("person,{by}")),
                    limit: Some(10),
                    user_id: None,
                    model: None,
                    provider_id: None,
                    harness_kind: None,
                    environment_kind: None,
                    project_id: None,
                    topic_id: None,
                    thread_id: None,
                },
                Some(&reading),
            )
            .await,
            bounded,
        )
    } else {
        OverviewPart::Omitted
    };
    let activity = if nonempty {
        part(
            ctx,
            super::activity::read_in(
                ctx,
                conn,
                &principal,
                orch_api_contract::nodes::NodeActivityListQuery::default(),
                Some(&reading),
            )
            .await,
            bounded,
        )
    } else {
        OverviewPart::Omitted
    };
    let outside_openings = if outside {
        part(
            ctx,
            crate::routes::ports::outside_in(
                ctx,
                conn,
                &principal,
                orch_api_contract::ports::NodePortOpeningListQuery::default(),
                &reading,
            )
            .await,
            bounded,
        )
    } else {
        OverviewPart::Omitted
    };
    let data = OrgOverview {
        org,
        children: window.page,
        recent,
        first_steps,
        outside_status,
        usage,
        activity,
        outside_openings,
    };
    let response = reading.finish()?;
    Ok(response.clone().with_data(PreparedOverview {
        data: OrgOverviewResponse::Overview {
            data: Box::new(data),
        },
        response,
    }))
}

pub(super) async fn get(
    ctx: Ctx,
    _query: OrgOverviewGetQuery,
    (): (),
) -> ApiResult<Answer<OrgOverviewResponse>> {
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let prepared = ctx.operation_data::<PreparedOverview>()?;
    let data = projection::disclose_read(
        &ctx,
        &mut conn,
        principal.caller(),
        prepared.response.clone(),
        prepared.data.clone(),
        "the organization overview is no longer readable",
    )
    .await?;
    conn.commit().await.map_err(DbError::from)?;
    Ok(Answer::from(data).header(http::header::CACHE_CONTROL, "no-store"))
}
