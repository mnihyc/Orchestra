//! One bounded native opening. Selection, ordinary visibility and credential
//! scope precede each projection; all suppliers share the current snapshot.

use super::child_list_in;

use super::projection;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Answer;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use chrono::Datelike;

use chrono::Duration;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::Listed;

use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::NodeChildListQuery;

use orch_api_contract::nodes::NodeOverview;

use orch_api_contract::nodes::NodeOverviewGetQuery;

use orch_api_contract::nodes::OrgOverview;

use orch_api_contract::nodes::OrgOverviewGetQuery;

use orch_api_contract::nodes::OrgOverviewResponse;

use orch_api_contract::nodes::OverviewPart;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use std::collections::BTreeSet;

use std::sync::Mutex;

pub use orch_api_base::routes::nodes::overview::Reading;
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
    let outside_principal = reading
        .outside_principal(ctx, conn, &target, outside && !nodes.is_empty())
        .await?;
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
            | orch_api_contract::usage::UsageDimension::Offering
            | orch_api_contract::usage::UsageDimension::Purpose
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
                    service: None,
                    purpose: None,
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
    let outside_openings = if let Some(principal) = &outside_principal {
        part(
            ctx,
            crate::routes::ports::outside_in(
                ctx,
                conn,
                principal,
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

struct PreparedNodeOverview {
    data: NodeOverview,
    response: OperationPlan,
}

/// The same node/source projection machinery, with no organization-only suppliers.
pub(super) fn node_prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: NodeOverviewGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let bounded = caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some());
        node_prepare_inner(ctx, conn, caller, query)
            .await
            .map_err(|error| {
                if bounded {
                    authorization::problem_without_content(error)
                } else {
                    error
                }
            })
    })
}

async fn node_prepare_inner(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: NodeOverviewGetQuery,
) -> ApiResult<OperationPlan> {
    let id = ctx.id("id")?;
    let target = targets::node(conn, id).await?;
    if !matches!(
        target.object.kind,
        KeyTargetKind::Object(ObjectKind::Project | ObjectKind::Topic)
    ) {
        return Err(ApiError::not_found());
    }
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(2, target.object.kind, std::slice::from_ref(&target))?;
    for slot in [3, 5] {
        plan.bind_empty(slot)?;
    }
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
    let covered =
        plan.permits_binding(caller, 4, target.object.kind, std::slice::from_ref(&target))?;
    let mut reading = Reading {
        caller,
        delivery: false,
        nodes: projection::Projection::with_plan(ctx, caller, plan.clone()),
        plan,
        primary: Mutex::new(Vec::new()),
        sources: Mutex::new(Vec::new()),
    };
    let node = reading
        .visible(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if !matches!(&node, Node::Project(_) | Node::Topic(_)) {
        return Err(ApiError::not_found());
    }
    let outside_principal = reading
        .outside_principal(ctx, conn, &target, covered)
        .await?;
    let today = ctx.now.date_naive();
    let from = query.from.unwrap_or_else(|| {
        (today - Duration::days(i64::from(today.weekday().num_days_from_monday()))).to_string()
    });
    let to = query.to.unwrap_or_else(|| today.to_string());
    let usage = part(
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
                breakdowns: query.breakdowns,
                limit: query.limit,
                user_id: None,
                model: None,
                service: None,
                purpose: None,
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
    );
    let outside_openings = if let Some(principal) = &outside_principal {
        part(
            ctx,
            crate::routes::ports::outside_in(
                ctx,
                conn,
                principal,
                orch_api_contract::ports::NodePortOpeningListQuery::default(),
                &reading,
            )
            .await,
            bounded,
        )
    } else {
        OverviewPart::Omitted
    };
    let response = reading.finish()?;
    Ok(response.clone().with_data(PreparedNodeOverview {
        data: NodeOverview {
            node,
            usage,
            outside_openings,
        },
        response,
    }))
}

pub(super) async fn node_get(
    ctx: Ctx,
    _query: NodeOverviewGetQuery,
    (): (),
) -> ApiResult<Answer<NodeOverview>> {
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let prepared = ctx.operation_data::<PreparedNodeOverview>()?;
    let data = projection::disclose_read(
        &ctx,
        &mut conn,
        principal.caller(),
        prepared.response.clone(),
        prepared.data.clone(),
        "the node overview is no longer readable",
    )
    .await?;
    conn.commit().await.map_err(DbError::from)?;
    Ok(Answer::from(data).header(http::header::CACHE_CONTROL, "no-store"))
}
