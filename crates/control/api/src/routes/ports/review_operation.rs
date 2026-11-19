//! Native reviewed operands and source observations, before protected plan publication.

use super::*;

pub(super) mod origin;
use crate::key_operations::{
    native::{
        self,
        effects::{self, Apply, Effect, FirstEffect, Outcome},
    },
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::{KeyObjectRef, KeyResolvedTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    operation_sources::Declaration,
    port_sources::{self as sources, Source},
};
use orch_authz::Caller;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Default, Clone)]
pub(super) struct CapturedSources {
    pub(super) openings: Vec<rows::Opening>,
    pub(super) publisher: Option<(Id, Option<Id>)>,
    pub(super) processes: BTreeSet<Id>,
}

const ENV: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);
const HOST: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Host);
const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);
const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::ReviewOrgs),
    (ObjectKind::Project, Source::ReviewProjects),
    (ObjectKind::Topic, Source::ReviewTopics),
    (ObjectKind::Thread, Source::ReviewThreads),
];

pub(super) struct Review;
#[derive(Clone)]
pub(super) struct PreparedReview {
    environment: EnvironmentFull,
    body: PortOpeningRequest,
    plan: PortOpeningPlan,
    sources: CapturedSources,
    names: BTreeSet<Id>,
    observed_facts: Vec<native::Facts<Source>>,
}

fn collection(
    source: Source,
    groups: impl IntoIterator<Item = (KeyTargetKind, KeyResolvedTarget)>,
) -> native::Facts<Source> {
    let mut kinds: BTreeMap<KeyTargetKind, BTreeMap<KeyObjectRef, KeyResolvedTarget>> =
        BTreeMap::new();
    for (kind, target) in groups {
        kinds.entry(kind).or_default().insert(target.object, target);
    }
    native::Facts::collection(
        source,
        kinds
            .into_iter()
            .map(|(kind, values)| (kind, values.into_values().collect()))
            .collect(),
    )
}

async fn primary(ctx: &Ctx, conn: &mut Conn, environment: Id) -> ApiResult<native::Facts<Source>> {
    let _ = ctx;
    Ok(native::Facts::one(
        Source::ReviewEnvironment,
        ENV,
        targets::page_object(
            conn,
            orch_access_contract::decisions::ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
        )
        .await?,
    ))
}

async fn visible_names(
    conn: &mut Conn,
    principal: &Principal,
    ids: &BTreeSet<Id>,
) -> ApiResult<BTreeSet<Id>> {
    let graph = orch_authz::engine::Graph::load(conn, &ids.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let all: BTreeSet<Id> = ids
        .iter()
        .flat_map(|id| {
            graph
                .get(*id)
                .into_iter()
                .flat_map(|row| row.path.iter().copied())
        })
        .chain(ids.iter().copied())
        .collect();
    let refs = crate::routes::nodes::view::refs(
        conn,
        principal,
        &all.iter().copied().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    Ok(all
        .into_iter()
        .filter(|id| {
            refs.get(id)
                .is_some_and(|reference| !reference.redacted && reference.name.is_some())
        })
        .collect())
}

fn name_ids(request: &PortOpeningRequest, captured: &CapturedSources) -> BTreeSet<Id> {
    let mut ids: BTreeSet<Id> = request
        .place_id
        .into_iter()
        .chain(captured.processes.iter().copied())
        .collect();
    for opening in &captured.openings {
        ids.extend(opening_operation::name_sources(opening).nodes);
    }
    ids
}

async fn facts(
    ctx: &Ctx,
    conn: &mut Conn,
    value: &PreparedReview,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut result = vec![primary(ctx, conn, value.environment.id).await?];
    let named = match value.plan.request.place_id {
        Some(id) if value.plan.request.kind == PortOpeningKind::Place => {
            let target = targets::node(conn, id).await?;
            vec![(target.object.kind, target)]
        }
        _ => Vec::new(),
    };
    result.push(collection(Source::NamedPlace, named));
    let mut hosts = Vec::new();
    let mut publishers = Vec::new();
    for opening in &value.sources.openings {
        let (host, machine) = opening_operation::foreign_sources(conn, opening).await?;
        hosts.extend(host.into_iter().map(|target| (HOST, target)));
        publishers.extend(machine.into_iter().map(|target| (MACHINE, target)));
    }
    if let Some((machine, host)) = value.sources.publisher {
        publishers.push((MACHINE, targets::machine_subject(conn, machine).await?));
        if let Some(host) = host {
            hosts.push((
                HOST,
                targets::page_object(
                    conn,
                    orch_access_contract::decisions::ObjectRef {
                        kind: ObjectKind::Host,
                        id: host,
                    },
                )
                .await?,
            ));
        }
    }
    result.push(collection(Source::ReviewHosts, hosts));
    result.push(collection(Source::ReviewPublishers, publishers));
    let graph =
        orch_authz::engine::Graph::load(conn, &value.names.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
    for &(kind, source) in NAMES {
        let family = KeyTargetKind::Object(kind);
        let mut targets = Vec::new();
        for id in &value.names {
            let target = orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
                .ok_or_else(ApiError::not_found)?;
            if target.object.kind == family {
                targets.push((family, target));
            }
        }
        result.push(collection(source, targets));
    }
    Ok(result)
}

impl Effect for Review {
    type Source = Source;
    type Query = ();
    type Body = PortOpeningRequest;
    type Value = PreparedReview;
    type Output = Arc<PreparedReview>;
    fn declaration(_value: &PreparedReview) -> &'static Declaration<Source> {
        &sources::OPENING_REVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: PortOpeningRequest,
    ) -> BoxFuture<'a, ApiResult<(PreparedReview, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let port = port(ctx)?;
            if body.port != port {
                return Err(ApiError::invalid(
                    "port",
                    "body and path must name the same target",
                ));
            }
            let environment = environment(conn, id).await?;
            let root = primary(ctx, conn, id).await?;
            let known =
                native::bind_known(ctx, &sources::OPENING_REVIEW, std::slice::from_ref(&root))?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(conn, &principal, id, Permission::PortsManage).await?;
            let request = match core::resolve_request(
                conn,
                &principal,
                &environment,
                &body,
                address(ctx, id, port).is_some(),
            )
            .await
            {
                Ok(value) => value,
                Err(error) => return Err(private_error(caller, failure(conn, error).await)),
            };
            let mut captured = CapturedSources::default();
            let disclosure = (body.kind == PortOpeningKind::Link).then(|| {
                "The link is given to you once; its proof is absent from general lists".to_owned()
            });
            let plan = plan_of_captured(
                ctx,
                conn,
                &principal,
                &environment,
                port,
                request,
                disclosure,
                Some(&mut captured),
            )
            .await?;
            let names =
                visible_names(conn, &principal, &name_ids(&plan.request, &captured)).await?;
            let mut value = PreparedReview {
                environment,
                body,
                plan,
                sources: captured,
                names,
                observed_facts: Vec::new(),
            };
            let facts = facts(ctx, conn, &value).await?;
            value.observed_facts = facts.clone();
            Ok((value, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a PreparedReview,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                value.environment.id,
                Permission::PortsManage,
            )
            .await?;
            if let Some(place) = value.plan.request.place_id {
                if !core::place_view(conn, principal, place)
                    .await
                    .map_err(ApiError::internal)?
                {
                    return Err(ApiError::new(
                        ErrorCode::PortsNotAllowed,
                        "current View of the named place is required",
                    ));
                }
            }
            Ok(())
        })
    }
}

impl Apply for Review {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a PreparedReview,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Arc<PreparedReview>>>> {
        Box::pin(async move {
            let origin = orch_authz::CapturedOperation::capture(principal.caller())
                .map_err(ApiError::internal)?;
            let session = match ctx.user()?.via {
                crate::middleware::authentication::Via::Session { session_id } => Some(session_id),
                crate::middleware::authentication::Via::Token { .. } => None,
            };
            let row = orch_db::queries::jobs::forks::PreflightRow {
                id: value.plan.preflight_id,
                kind: "port_opening".into(),
                node_id: None,
                environment_id: Some(value.environment.id),
                created_by: principal.user(),
                plan: serde_json::json!({"plan":value.plan,"body":value.body,"creator_origin":origin,"creator_session":session}),
                expires_at: value.plan.expires_at.get(),
                created_at: principal.now().get(),
            };
            orch_db::queries::jobs::forks::save_preflight_before(conn, &row, move || {
                before.check()
            })
            .await?;
            Ok(Outcome::Applied(Arc::new(value.clone())))
        })
    }
}

fn private_error(caller: &Caller, error: ApiError) -> ApiError {
    if native::modern(caller) {
        error.without_content()
    } else {
        error
    }
}

async fn read(
    ctx: &Ctx,
    accepted: &Caller,
    value: Arc<PreparedReview>,
) -> ApiResult<Option<PortOpeningPlan>> {
    let projection = accepted.clone();
    native::completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            if !native::modern(current.caller()) {
                validate(ctx, conn, current, &value).await?;
                return Ok(Some(value.plan.clone()));
            }
            let response = native::bind(
                ctx,
                &sources::OPENING_REVIEW,
                &facts(ctx, conn, &value).await?,
            )?;
            check_observed_sources(ctx, &response, &value)?;
            let snapshot_ctx = ctx.clone();
            response
                .read_response(ctx, conn, &projection, move |conn, reader| {
                    Box::pin(async move {
                        validate(&snapshot_ctx, conn, reader, &value).await?;
                        Ok(value.plan.clone())
                    })
                })
                .await
        })
    })
    .await
}

fn check_observed_sources(
    ctx: &Ctx,
    fresh: &crate::registry::OperationPlan,
    value: &PreparedReview,
) -> ApiResult<()> {
    let observed = native::bind(ctx, &sources::OPENING_REVIEW, &value.observed_facts)?;
    if !fresh.shared().covers_sources(observed.shared()) {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the original review source is no longer readable",
        ));
    }
    Ok(())
}

async fn validate(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    value: &PreparedReview,
) -> ApiResult<()> {
    <Review as Effect>::require(ctx, conn, reader, value).await?;
    for old in &value.sources.openings {
        let current = rows::opening(conn, old.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if current.environment_id != old.environment_id
            || current.place_id != old.place_id
            || current.publish_machine_id != old.publish_machine_id
            || current.host_id != old.host_id
            || current.requested_by_thread != old.requested_by_thread
            || current.admission != old.admission
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "the original review opening association changed",
            ));
        }
    }
    if let Some((machine, host)) = value.sources.publisher {
        if !orch_db::queries::environment_facts::publisher_of(conn, value.environment.id)
            .await?
            .is_some_and(|publisher| publisher.machine_id == machine && publisher.host_id == host)
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "the original review publisher association changed",
            ));
        }
    }
    let visible =
        visible_names(conn, reader, &name_ids(&value.plan.request, &value.sources)).await?;
    if !value.names.is_subset(&visible) {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the original review names are no longer readable",
        ));
    }
    let process_refs = crate::routes::nodes::view::refs(
        conn,
        reader,
        &value.sources.processes.iter().copied().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    if value
        .sources
        .processes
        .iter()
        .any(|id| !process_refs.contains_key(id))
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the original port observation is no longer readable",
        ));
    }
    Ok(())
}

pub(super) async fn run(ctx: Ctx) -> ApiResult<PortOpeningPlanResponse> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    initial.check_deadline(&ctx)?;
    if !rows::lock_environment(&mut tx, ctx.id("id")?).await? {
        return Err(ApiError::not_found());
    }
    let completed = effects::apply::<Review>(&ctx, &mut tx).await?;
    let (current, final_guard) = transaction_guard::after_wait(&completed.ctx, &mut tx).await?;
    final_guard.check_deadline(&current)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let ctx = completed.ctx;
    let accepted = completed.accepted;
    let value = completed.output;
    let receipt = PortOpeningPlanReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        preflight_id: value.plan.preflight_id,
        environment_id: value.environment.id,
        port: value.plan.request.port,
        expires_at: value.plan.expires_at,
    };
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    match read(&ctx, &accepted, value.clone()).await? {
        None => Ok(PortOpeningPlanResponse::Receipt(receipt)),
        Some(plan) => {
            crate::key_operations::publication::retain(
                &ctx,
                &accepted,
                (value, accepted.clone()),
                |ctx, conn, _current, (value, accepted)| {
                    Box::pin(async move {
                        let response = native::bind(
                            ctx,
                            &sources::OPENING_REVIEW,
                            &facts(ctx, conn, value).await?,
                        )?;
                        check_observed_sources(ctx, &response, value)?;
                        let snapshot_ctx = ctx.clone();
                        let opened = Arc::clone(value);
                        response
                            .read_response(ctx, conn, accepted, move |conn, reader| {
                                Box::pin(async move {
                                    validate(&snapshot_ctx, conn, reader, &opened).await
                                })
                            })
                            .await?
                            .ok_or_else(|| {
                                ApiError::new(
                                    ErrorCode::AuthzDenied,
                                    "the original review content is no longer admitted",
                                )
                            })
                    })
                },
            )?;
            Ok(PortOpeningPlanResponse::Full(Box::new(plan)))
        }
    }
}
