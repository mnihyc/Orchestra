//! Exact saved native port work, with separate original review and confirming boundaries.

use super::*;
use crate::key_operations::{
    native::{
        self,
        effects::{self, Apply, Effect, FirstEffect, Outcome},
    },
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};
use orch_api_contract::{
    operation_sources::Declaration,
    port_sources::{self as sources, Source},
};
use orch_authz::Caller;
use orch_db::queries::jobs::forks::{self, PreflightRow};

pub(super) mod projection;

pub(super) struct Create;

#[derive(Clone)]
pub(super) struct Work {
    environment: EnvironmentFull,
    stored: PreflightRow,
    plan: PortOpeningPlan,
    full: Json,
    client: Id,
    replay: bool,
    reviewer: Option<review_operation::origin::CurrentReview>,
}

#[derive(Clone)]
pub(super) struct Accepted {
    pub(super) row: rows::Opening,
    pub(super) created: bool,
    pub(super) link: Option<PortLinkDelivery>,
    pub(super) request_thread: Option<Id>,
    retired: Vec<Id>,
    reviewer: Option<review_operation::origin::CurrentReview>,
}

impl Accepted {
    pub(super) fn requested(row: rows::Opening, thread: Id, created: bool) -> Self {
        Self {
            row,
            created,
            link: None,
            request_thread: Some(thread),
            retired: Vec::new(),
            reviewer: None,
        }
    }
}
pub(super) async fn primary(conn: &mut Conn, environment: Id) -> ApiResult<native::Facts<Source>> {
    Ok(native::Facts::one(
        Source::OpeningEnvironment,
        KeyTargetKind::Object(ObjectKind::Environment),
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

pub(super) async fn named(
    conn: &mut Conn,
    request: &PortOpeningRequest,
) -> ApiResult<native::Facts<Source>> {
    let groups = if request.kind == PortOpeningKind::Place {
        let target = targets::node(conn, request.place_id.ok_or_else(ApiError::not_found)?).await?;
        vec![(target.object.kind, vec![target])]
    } else {
        Vec::new()
    };
    Ok(native::Facts::collection(Source::NamedPlace, groups))
}

async fn stored(
    ctx: &Ctx,
    conn: &mut Conn,
    user: Id,
    id: Id,
) -> ApiResult<(PreflightRow, PortOpeningPlan, Json)> {
    let stored = forks::preflight(conn, id)
        .await?
        .filter(|row| {
            row.kind == "port_opening"
                && row.environment_id == ctx.id("id").ok()
                && row.created_by == user
        })
        .ok_or_else(ApiError::not_found)?;
    let plan: PortOpeningPlan = serde_json::from_value(
        stored
            .plan
            .get("plan")
            .cloned()
            .ok_or_else(ApiError::not_found)?,
    )
    .map_err(ApiError::internal)?;
    if plan.request.port != port(ctx)? {
        return Err(ApiError::invalid(
            "preflight_id",
            "the plan names another target",
        ));
    }
    let full = stored
        .plan
        .get("body")
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    Ok((stored, plan, full))
}

impl Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = EnvironmentPortOpeningCreateBody;
    type Value = Work;
    type Output = Accepted;

    fn declaration(_value: &Work) -> &'static Declaration<Source> {
        &sources::OPENING_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: EnvironmentPortOpeningCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Work, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let environment = environment(conn, id).await?;
            let root = primary(conn, id).await?;
            let known =
                native::bind_known(ctx, &sources::OPENING_CREATE, std::slice::from_ref(&root))?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(conn, &principal, id, Permission::PortsManage).await?;
            let (stored, plan, full) =
                stored(ctx, conn, principal.user(), body.preflight_id).await?;
            let facts = vec![root, named(conn, &plan.request).await?];
            let required = native::bind_required(ctx, &sources::OPENING_CREATE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &required).await?;
            let replay = rows::request(conn, id, Some(principal.user()), None, body.client_id)
                .await?
                .is_some();
            if !replay && stored.expires_at <= ctx.app.clock.now() {
                return Err(ApiError::new(
                    ErrorCode::PreflightExpired,
                    "review the current port again",
                ));
            }
            let reviewer = if replay {
                None
            } else {
                review_operation::origin::current(ctx, conn, &stored, &plan.request).await?
            };
            Ok((
                Work {
                    environment,
                    stored,
                    plan,
                    full,
                    client: body.client_id,
                    replay,
                    reviewer,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Work,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                value.environment.id,
                Permission::PortsManage,
            )
            .await?;
            if value.plan.request.kind == PortOpeningKind::Place
                && !core::place_view(
                    conn,
                    principal,
                    value
                        .plan
                        .request
                        .place_id
                        .ok_or_else(ApiError::not_found)?,
                )
                .await
                .map_err(ApiError::internal)?
            {
                return Err(ApiError::new(
                    ErrorCode::PortsNotAllowed,
                    "the current manager must view the named place",
                ));
            }
            Ok(())
        })
    }
}

impl Apply for Create {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Work,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Accepted>>> {
        Box::pin(async move {
            let id = value.environment.id;
            let port = value.plan.request.port;
            let mut first = Some(before);
            let mut check = || -> Result<(), core::Error> {
                if first.is_some()
                    && !value.replay
                    && value.stored.expires_at <= ctx.app.clock.now()
                {
                    return Err(core::Error::BeforeEffect(Box::new(ApiError::new(
                        ErrorCode::PreflightExpired,
                        "review the current port again",
                    ))));
                }
                if let Some(reviewer) = &value.reviewer {
                    reviewer
                        .check(ctx)
                        .map_err(|error| core::Error::BeforeEffect(Box::new(error)))?;
                }
                if let Some(before) = first.take() {
                    before
                        .check()
                        .map_err(|error| core::Error::BeforeEffect(Box::new(error)))?;
                }
                Ok(())
            };
            let mut retired =
                match core::reconcile_before(conn, id, principal.now(), &mut check).await {
                    Ok(retired) => retired,
                    Err(error) => return Err(failure(conn, error).await),
                };
            let (old_program, _) = rows::program(conn, id, port).await?;
            if !value.replay
                && let Some(desired) = &value.plan.request.program
            {
                check_program(ctx, conn, &value.environment, port, &old_program, desired).await?;
            }
            let mut proof = None;
            let preparation = if value.replay {
                // The native client-fence branch returns before it consults a preparation.
                core::Preparation::Place
            } else if value.plan.request.kind == PortOpeningKind::Link {
                use base64::Engine as _;
                let opening = new_id();
                let random: [u8; 32] = rand::random();
                let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
                let sealed = crate::routes::resources::sealing::master_key()?
                    .seal(token.as_bytes(), opening.as_bytes())
                    .map_err(ApiError::internal)?;
                let digest = orch_types::Hash::of(token.as_bytes()).as_bytes().to_vec();
                proof = Some(token);
                core::Preparation::Link {
                    id: opening,
                    digest,
                    sealed,
                    recipient: PortLinkRecipient::Person,
                    person: Some(principal.user()),
                }
            } else if value.plan.request.kind == PortOpeningKind::Host {
                let candidate = value.plan.candidate.as_ref().ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::PortsNoRange,
                        "the review found no host number: review the opening again",
                    )
                })?;
                core::Preparation::Host {
                    host: candidate.host.as_ref().map(|host| host.id),
                    machine: candidate.machine.id,
                    port: candidate.host_port,
                    revision: candidate.host_revision,
                }
            } else {
                core::Preparation::Place
            };
            let produced = match core::produce_prepared_before(
                conn,
                principal,
                &value.environment,
                Requester::Person(principal.user()),
                value.client,
                &value.full,
                &value.plan.request,
                value.plan.program_revision,
                &preparation,
                &crate::sign_in::origin(ctx),
                &mut check,
            )
            .await
            {
                Ok(produced) => produced,
                Err(error) => return Err(failure(conn, error).await),
            };
            let row = produced.opening;
            let link = if produced.created
                && let Some(proof) = proof
            {
                let helper = format!(
                    "{}/api/v1/ports/links/{}#{proof}",
                    ctx.app.config.system.public_url.trim_end_matches('/'),
                    row.id
                );
                let browser = row
                    .entry_methods
                    .iter()
                    .any(|method| method == "browser")
                    .then(|| {
                        address(ctx, id, port)
                            .map(|address| format!("{address}/?orch_token={proof}"))
                    })
                    .flatten();
                rows::delivered(conn, row.id).await?;
                Some(PortLinkDelivery {
                    recipient: PortLinkRecipient::Person,
                    url_once: Some(helper.clone()),
                    browser_url_once: browser,
                    helper_link_once: row
                        .entry_methods
                        .iter()
                        .any(|method| method == "helper")
                        .then_some(helper),
                    delivered_to: "the creating person, once".into(),
                })
            } else {
                None
            };
            if produced.created
                && value.plan.request.close_replaced == Some(true)
                && let Some(old) = value.plan.request.replacement_of
            {
                let replaced = rows::opening(conn, old)
                    .await?
                    .filter(|row| {
                        row.environment_id == id
                            && row.port == i32::from(port)
                            && row.kind == "link"
                            && !matches!(row.state.as_str(), "closed" | "refused")
                    })
                    .ok_or_else(|| {
                        ApiError::invalid(
                            "replacement_of",
                            "a replacement must name a current link on this target",
                        )
                    })?;
                rows::close(conn, old, "explicitly_replaced", principal.now().get()).await?;
                core::record(
                    conn,
                    &value.environment,
                    &replaced,
                    Actor::User(principal.user()),
                    AuditAction::PortsClosed,
                    &crate::sign_in::origin(ctx),
                    principal.now(),
                )
                .await
                .map_err(ApiError::internal)?;
                retired.push(old);
            }
            if !value.replay {
                review_operation::origin::require(ctx, conn, &value.stored, &value.plan.request)
                    .await?;
            }
            let result = Accepted {
                row,
                created: produced.created,
                request_thread: None,
                link,
                retired,
                reviewer: value.reviewer.clone(),
            };
            Ok(if first.is_none() {
                Outcome::Applied(result)
            } else {
                Outcome::Unchanged(result)
            })
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    body: EnvironmentPortOpeningCreateBody,
) -> ApiResult<Answer<PortOpenResponse>> {
    let result = run_in(&ctx, body).await;
    result.map_err(|error| {
        if ctx.key_receipt_required() {
            error.without_content()
        } else {
            error
        }
    })
}

async fn run_in(
    ctx: &Ctx,
    body: EnvironmentPortOpeningCreateBody,
) -> ApiResult<Answer<PortOpenResponse>> {
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    initial.check_deadline(ctx)?;
    let id = ctx.id("id")?;
    rows::lock_environment(&mut tx, id).await?;
    rows::lock_target(&mut tx, id, port(ctx)?).await?;
    let (_, plan, _) = stored(ctx, &mut tx, initial.principal().user(), body.preflight_id).await?;
    if plan.request.kind == PortOpeningKind::Host
        && let Some(candidate) = &plan.candidate
    {
        rows::lock_publisher_machine(&mut tx, candidate.machine.id).await?;
    }
    let completed = effects::apply::<Create>(ctx, &mut tx).await?;
    let (current, final_guard) = transaction_guard::after_wait(&completed.ctx, &mut tx).await?;
    final_guard.check_deadline(&current)?;
    if let Some(reviewer) = &completed.output.reviewer {
        reviewer.check(&completed.ctx)?;
    }
    orch_db::idempotency::creation::observe_with_status(
        &mut tx,Some(completed.accepted.user()),"port_opening",completed.output.row.id,
        serde_json::json!({"environment":completed.output.row.environment_id,"revision":completed.output.row.revision,"created":completed.output.created}),
        if completed.output.created {201}else{200},
    ).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let ctx = completed.ctx;
    let output = completed.output;
    for old in &output.retired {
        ctx.app.cache.ports().close_opening(*old);
        ctx.app.link.terminals().close_port_opening(*old);
    }
    crate::host_ports::unpublish_in_background(&ctx.app, &output.retired);
    if output.created && output.row.kind == "host" {
        crate::host_ports::publish_in_background(&ctx.app, output.row.id);
    }
    let receipt = PortOpenReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        opening_id: output.row.id,
        environment_id: output.row.environment_id,
        revision: output.row.revision,
        created: output.created,
        request_id: None,
        link: output.link.clone(),
    };
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    let status = if output.created {
        None
    } else {
        Some(http::StatusCode::OK)
    };
    let response =
        match native::reply::render::<projection::Opening>(&ctx, &completed.accepted, output)
            .await?
        {
            Some(full) => PortOpenResponse::Full(Box::new(full)),
            None => PortOpenResponse::Receipt(receipt),
        };
    let mut answer = Answer::from(response);
    answer.status = status;
    Ok(answer)
}

/// The accepted native opening is observed in the same transaction as its client fence.
pub(crate) fn creation() -> crate::middleware::idempotency::ObservedCreation<
    (),
    EnvironmentPortOpeningCreateBody,
    PortOpenResponse,
> {
    crate::middleware::idempotency::ObservedCreation {
        kind: "port_opening",
        replay: |ctx, (), _body, created| {
            Box::pin(async move {
                let environment: Id =
                    serde_json::from_value(created.data.get("environment").cloned().ok_or_else(
                        || ApiError::internal("the accepted port opening has no environment"),
                    )?)
                    .map_err(ApiError::internal)?;
                let revision: i64 =
                    serde_json::from_value(created.data.get("revision").cloned().ok_or_else(
                        || ApiError::internal("the accepted port opening has no revision"),
                    )?)
                    .map_err(ApiError::internal)?;
                let was_created: bool =
                    serde_json::from_value(created.data.get("created").cloned().ok_or_else(
                        || ApiError::internal("the accepted port opening has no native outcome"),
                    )?)
                    .map_err(ApiError::internal)?;
                let receipt = PortOpenResponse::Receipt(PortOpenReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    opening_id: created.id,
                    environment_id: environment,
                    revision,
                    created: was_created,
                    request_id: None,
                    link: None,
                });
                crate::key_operations::publication::receipt(&ctx, &receipt)?;
                let result = async {
                    if ctx.id("id")? != environment {
                        return Err(ApiError::not_found());
                    }
                    let target_port = port(&ctx)?;
                    let row = rows::opening(&mut *ctx.app.db.acquire().await?, created.id)
                        .await?
                        .filter(|row| {
                            row.environment_id == environment && row.port == i32::from(target_port)
                        })
                        .ok_or_else(ApiError::not_found)?;
                    native::reply::render_completed_replay::<projection::Opening>(
                        &ctx,
                        ctx.user()?.id(),
                        Accepted {
                            row,
                            created: was_created,
                            request_thread: None,
                            link: None,
                            retired: Vec::new(),
                            reviewer: None,
                        },
                    )
                    .await
                }
                .await;
                match result {
                    Ok(Some(value)) => Ok(PortOpenResponse::Full(Box::new(value))),
                    Ok(None) => Ok(receipt),
                    Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                        Ok(receipt)
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}
