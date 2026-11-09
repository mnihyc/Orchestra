//! One native shell planner/reservation, retained creator, and once-only response suppliers.

use super::*;
use crate::key_operations::{
    native::{
        self,
        effects::{self, ApplyOwned, Effect, FirstEffect, Outcome},
    },
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    operation_sources::Declaration,
    terminal_sources::{self as sources, Source},
    terminals::{EnvironmentTerminalCreateResponse, TerminalCreateReceipt},
};
use orch_authz::Caller;
use orch_engine::{
    commands::TerminalCreationOrigin,
    shells::{Opened, PreparingOpen},
};
use std::sync::Arc;

pub(super) struct Create;
pub(super) struct Work {
    request: OpenRequest,
    prepared: Result<PreparingOpen, Arc<OpenError>>,
}
pub(super) enum ResultOf {
    Blocked {
        request: Box<OpenRequest>,
        error: Arc<OpenError>,
    },
    Failed(Box<OpenError>),
    Opened(Box<Created>),
}
pub(super) struct Created {
    opened: Opened,
    creator: TerminalCreationOrigin,
}

async fn facts(conn: &mut Conn, environment: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![native::Facts::one(
        Source::OpeningEnvironment,
        KeyTargetKind::Object(ObjectKind::Environment),
        targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
        )
        .await?,
    )])
}

async fn ordinary(conn: &mut Conn, principal: &Principal, request: &OpenRequest) -> ApiResult<()> {
    match may_open_shell(conn, principal, request.environment, Policy::default()).await {
        Ok(Decision::Allowed { .. }) => {}
        Ok(Decision::Refused(refusal)) => return Err(refusals::denied(conn, refusal).await?),
        Err(error) => return Err(refusals::problem(conn, error).await),
    }
    if let Some(thread) = request.through {
        door::beside(conn, principal, request.environment, thread).await?;
    }
    Ok(())
}

fn raw_session(ctx: &Ctx) -> ApiResult<Option<Id>> {
    Ok(match ctx.user()?.via {
        crate::middleware::authentication::Via::Session { session_id } => Some(session_id),
        crate::middleware::authentication::Via::Token { .. }
        | crate::middleware::authentication::Via::OwnerRead { .. } => None,
    })
}

impl Effect for Create {
    type Source = Source;
    type Query = ThroughThreadQuery;
    type Body = EnvironmentTerminalCreateBody;
    type Value = Work;
    type Output = ResultOf;
    fn declaration(_value: &Work) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: ThroughThreadQuery,
        body: EnvironmentTerminalCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Work, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if !(1..=SIZE_MAX).contains(&body.cols) {
                return Err(ApiError::invalid(
                    "cols",
                    "is the width of the screen, 1 to 1000",
                ));
            }
            if !(1..=SIZE_MAX).contains(&body.rows) {
                return Err(ApiError::invalid(
                    "rows",
                    "is the height of the screen, 1 to 1000",
                ));
            }
            let environment = ctx.id("id")?;
            if rows::site(conn, environment).await?.is_none() {
                return Err(ApiError::not_found());
            }
            let facts = facts(conn, environment).await?;
            let known = native::bind_required(ctx, &sources::CREATE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let request = OpenRequest {
                person: principal.user(),
                environment,
                through: query.thread,
                cols: body.cols,
                rows: body.rows,
                directory: body.cwd,
                close: body.close.unwrap_or_default(),
                origin: origin(ctx),
            };
            ordinary(conn, &principal, &request).await?;
            let prepared = match ctx.app.engine.shells().preparing_open(conn, &request).await {
                Ok(prepared) => Ok(prepared),
                Err(OpenError::Engine(error)) => return Err(ApiError::engine(error)),
                Err(error) => Err(Arc::new(error)),
            };
            Ok((Work { request, prepared }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Work,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(ordinary(conn, principal, &value.request))
    }
}

impl ApplyOwned for Create {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Work,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<ResultOf>>> {
        Box::pin(async move {
            let prepared = match &value.prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    before.check()?;
                    tx.rollback().await.map_err(orch_db::DbError::from)?;
                    return Ok(Outcome::Unchanged(ResultOf::Blocked {
                        request: Box::new(value.request.clone()),
                        error: Arc::clone(error),
                    }));
                }
            };
            let creator = TerminalCreationOrigin::capture(
                principal.caller(),
                raw_session(ctx)?,
                prepared.site(),
            )
            .map_err(ApiError::engine)?;
            let check: orch_engine::commands::BeforeCommand<'_> = Box::new(move || {
                before
                    .check()
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
            });
            let shells = ctx.app.engine.shells();
            let opening = match shells
                .reserve_open_before(&mut tx, prepared, Some(check))
                .await
            {
                Ok(opening) => opening,
                Err(OpenError::Engine(error)) => return Err(ApiError::engine(error)),
                Err(error) => return Ok(Outcome::Unchanged(ResultOf::Failed(Box::new(error)))),
            };
            let creator = creator.with_opening(&opening).map_err(ApiError::engine)?;
            let mut receipt = serde_json::json!({"environment":opening.site().id});
            if value.request.close.is_empty() {
                receipt["closed"] = serde_json::json!([]);
            }
            orch_db::idempotency::creation::observe_with(
                &mut tx,
                Some(principal.user()),
                "terminal_creation",
                opening.id(),
                receipt,
            )
            .await?;
            let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
            guard.check_deadline(&current)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            let sockets = ctx.app.realtime.terminals();
            let about = |row: &SessionRow, _: Ending| {
                sockets.tell(row.id, Told::Over(TerminalClosedReason::ClosedByUser))
            };
            let result = match shells
                .continue_open(opening, Some(creator.clone()), &about)
                .await
            {
                Ok(opened) => ResultOf::Opened(Box::new(Created { opened, creator })),
                Err(error) => ResultOf::Failed(Box::new(error)),
            };
            Ok(Outcome::Applied(result))
        })
    }
}

fn associated(old: &SessionRow, current: &SessionRow) -> bool {
    old.id == current.id
        && old.environment_id == current.environment_id
        && old.env_generation == current.env_generation
        && old.opened_by == current.opened_by
        && old.thread_id == current.thread_id
}

async fn require_opened(conn: &mut Conn, principal: &Principal, row: &SessionRow) -> ApiResult<()> {
    if row.opened_by != principal.user() || row.state != rows::state::OPEN {
        return Err(ApiError::not_found());
    }
    match may_open_shell(conn, principal, row.environment_id, Policy::default()).await {
        Ok(Decision::Allowed { .. }) => Ok(()),
        Ok(Decision::Refused(refusal)) => Err(refusals::denied(conn, refusal).await?),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

async fn current_creator(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Caller,
    creator: &TerminalCreationOrigin,
) -> ApiResult<Option<Caller>> {
    let Some((source, _)) = creator
        .current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(ApiError::engine)?
    else {
        return Ok(None);
    };
    current
        .narrow_current_admission(source.caller())
        .map(Some)
        .map_err(|_| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the native shell control source is no longer current",
            )
        })
}

/// A successful current native source frame before once-only status IO or control-ticket issue.
async fn opened_frame(
    ctx: &Ctx,
    accepted: &Caller,
    original: SessionRow,
    creator: TerminalCreationOrigin,
) -> ApiResult<Option<(Ctx, SessionRow, rows::Site)>> {
    native::completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let Some(caller) = current_creator(ctx, conn, current.caller(), &creator).await? else {
                return Ok(None);
            };
            let row = rows::get(conn, original.id)
                .await?
                .filter(|row| associated(&original, row))
                .ok_or_else(ApiError::not_found)?;
            let site = rows::site(conn, row.environment_id)
                .await?
                .filter(|site| site.generation == row.env_generation)
                .ok_or_else(ApiError::not_found)?;
            let plan = native::bind(
                ctx,
                &sources::CREATE,
                &facts(conn, row.environment_id).await?,
            )?;
            let project = ctx.clone();
            plan.read_response(ctx, conn, &caller, move |conn, reader| {
                Box::pin(async move {
                    require_opened(conn, reader, &row).await?;
                    let ctx = project.with_admitted_caller(reader.caller().clone())?;
                    Ok((ctx, row, site))
                })
            })
            .await
        })
    })
    .await
}

async fn opened_reply(
    ctx: &Ctx,
    accepted: &Caller,
    opened: Opened,
    creator: TerminalCreationOrigin,
) -> ApiResult<EnvironmentTerminalCreateResponse> {
    let receipt = TerminalCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        terminal_id: opened.session.id,
        environment_id: opened.session.environment_id,
        closed: Some(opened.closed.clone()),
    };
    crate::key_operations::publication::receipt(ctx, &receipt)?;
    let Some((_, row, _)) =
        opened_frame(ctx, accepted, opened.session.clone(), creator.clone()).await?
    else {
        return Ok(EnvironmentTerminalCreateResponse::Receipt(receipt));
    };
    // This native supplier runs exactly once, outside the producing/retrying snapshot.
    let busy = {
        let mut conn = ctx.app.db.acquire().await?;
        match ctx
            .app
            .engine
            .shells()
            .busy_for_creator(&mut conn, std::slice::from_ref(&row), &creator)
            .await
        {
            Ok(busy) => busy,
            Err(error) => {
                drop(conn);
                if opened_frame(ctx, accepted, row.clone(), creator.clone())
                    .await?
                    .is_none()
                {
                    return Ok(EnvironmentTerminalCreateResponse::Receipt(receipt));
                }
                return Err(ApiError::engine(error));
            }
        }
    };
    let Some((project, row, site)) = opened_frame(ctx, accepted, row, creator.clone()).await?
    else {
        return Ok(EnvironmentTerminalCreateResponse::Receipt(receipt));
    };
    let ticket_creator = creator.clone();
    let result = native::completed(&project, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let Some(caller) =
                current_creator(ctx, conn, current.caller(), &ticket_creator).await?
            else {
                return Ok(None);
            };
            let plan = native::bind(
                ctx,
                &sources::CREATE,
                &facts(conn, row.environment_id).await?,
            )?;
            let project = ctx.clone();
            plan.read_response(ctx, conn, &caller, move |conn, reader| {
                Box::pin(async move {
                    require_opened(conn, reader, &row).await?;
                    let session = views_observed(
                        conn,
                        &project.app.engine.shells(),
                        std::slice::from_ref(&row),
                        Some(&busy),
                    )
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| ApiError::internal("a created shell has no metadata"))?;
                    let identity = transaction_guard::current_http_identity(
                        &project,
                        conn,
                        transaction_guard::HttpIdentityMode::Read,
                    )
                    .await?;
                    identity.check_deadline(&project.app)?;
                    if !reader
                        .caller()
                        .key_deadline_allows(project.app.clock.as_ref())
                    {
                        return Err(ApiError::new(
                            ErrorCode::AuthInvalidToken,
                            "the creating key has ended",
                        ));
                    }
                    let caller = current_creator(&project, conn, reader.caller(), &ticket_creator)
                        .await?
                        .ok_or_else(ApiError::not_found)?;
                    identity.check_deadline(&project.app)?;
                    if !caller.key_deadline_allows(project.app.clock.as_ref()) {
                        return Err(ApiError::new(
                            ErrorCode::AuthInvalidToken,
                            "the creating key has ended",
                        ));
                    }
                    let project = project.with_admitted_caller(caller)?;
                    let ticket = project.app.realtime.terminals().issue_shell(
                        &project,
                        row.id,
                        row.environment_id,
                        site.machine_id,
                        row.env_generation,
                    )?;
                    Ok((
                        EnvironmentTerminalCreateReply {
                            session,
                            ticket: ticket.ticket,
                            ticket_expires_at: ticket.ticket_expires_at,
                            closed: opened.closed,
                        },
                        Arc::new(row),
                    ))
                })
            })
            .await
        })
    })
    .await?;
    let Some((reply, witness)) = result else {
        return Ok(EnvironmentTerminalCreateResponse::Receipt(receipt));
    };
    crate::key_operations::publication::retain(
        ctx,
        accepted,
        (witness, creator, accepted.clone()),
        |ctx, conn, _current, (old, creator, accepted)| {
            Box::pin(async move {
                if creator
                    .current(conn, ctx.app.clock.as_ref())
                    .await
                    .map_err(ApiError::engine)?
                    .is_none()
                {
                    return Err(ApiError::not_found());
                }
                let row = rows::get(conn, old.id)
                    .await?
                    .filter(|row| associated(old, row))
                    .ok_or_else(ApiError::not_found)?;
                let plan = native::bind(
                    ctx,
                    &sources::CREATE,
                    &facts(conn, row.environment_id).await?,
                )?;
                plan.read_response(ctx, conn, accepted, move |conn, reader| {
                    Box::pin(async move { require_opened(conn, reader, &row).await })
                })
                .await?
                .ok_or_else(ApiError::not_found)
            })
        },
    )?;
    Ok(EnvironmentTerminalCreateResponse::Full(Box::new(reply)))
}

async fn blocked(
    ctx: &Ctx,
    accepted: &Caller,
    request: OpenRequest,
    error: Arc<OpenError>,
) -> ApiResult<EnvironmentTerminalCreateResponse> {
    let mut conn = ctx.app.db.acquire().await?;
    // Non-content failures keep their original mapping and contain only input/static refusal data.
    if !matches!(error.as_ref(), OpenError::AtLimit(_)) {
        return Err(open_failed_observed(
            &mut conn,
            &ctx.app.engine.shells(),
            error.as_ref(),
            None,
        )
        .await?);
    }
    drop(conn);
    let original = accepted.clone();
    let authorizing = request.clone();
    let frame = native::completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let plan = native::bind(
                ctx,
                &sources::CREATE,
                &facts(conn, authorizing.environment).await?,
            )?;
            let project = ctx.clone();
            let source = plan.clone();
            let caller = source.clone().admit(current.caller())?.caller;
            plan.read_response(ctx, conn, &caller, move |conn, reader| {
                Box::pin(async move {
                    ordinary(conn, reader, &authorizing).await?;
                    let caller = source.admit(reader.caller())?.caller;
                    let site = rows::site(conn, authorizing.environment)
                        .await?
                        .ok_or_else(ApiError::not_found)?;
                    TerminalCreationOrigin::capture(&caller, raw_session(&project)?, &site)
                        .map_err(ApiError::engine)
                })
            })
            .await
        })
    })
    .await?;
    let Some(creator) = frame else {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the creation's current metadata authority is closed",
        ));
    };
    let OpenError::AtLimit(at) = error.as_ref() else {
        return Err(ApiError::internal("the native limit source changed"));
    };
    let mut conn = ctx.app.db.acquire().await?;
    let busy = ctx
        .app
        .engine
        .shells()
        .busy_for_creator(&mut conn, &at.mine, &creator)
        .await
        .map_err(ApiError::engine)?;
    let failure = open_failed_observed(
        &mut conn,
        &ctx.app.engine.shells(),
        error.as_ref(),
        Some(&busy),
    )
    .await?;
    drop(conn);
    let witness = at.mine.clone();
    let native_request = request.clone();
    crate::key_operations::publication::retain(
        ctx,
        &original,
        (native_request, witness, original.clone(), creator),
        |ctx, conn, _current, (request, old, accepted, creator)| {
            Box::pin(async move {
                if creator
                    .current(conn, ctx.app.clock.as_ref())
                    .await
                    .map_err(ApiError::engine)?
                    .is_none()
                {
                    return Err(ApiError::not_found());
                }
                let plan = native::bind(
                    ctx,
                    &sources::CREATE,
                    &facts(conn, request.environment).await?,
                )?;
                let request = request.clone();
                let old = old.clone();
                plan.read_response(ctx, conn, accepted, move |conn, reader| {
                    Box::pin(async move {
                        ordinary(conn, reader, &request).await?;
                        let current =
                            rows::get_many(conn, &old.iter().map(|row| row.id).collect::<Vec<_>>())
                                .await?;
                        if old
                            .iter()
                            .any(|row| !current.iter().any(|fresh| associated(row, fresh)))
                        {
                            return Err(ApiError::not_found());
                        }
                        Ok(())
                    })
                })
                .await?
                .ok_or_else(ApiError::not_found)
            })
        },
    )?;
    Err(failure)
}

pub(super) async fn run(ctx: Ctx) -> ApiResult<EnvironmentTerminalCreateResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    guard.check_deadline(&ctx)?;
    rows::lock(&mut tx, ctx.id("id")?).await?;
    let completed = effects::apply_owned::<Create>(&ctx, tx).await?;
    match completed.output {
        ResultOf::Opened(created) => {
            let Created { opened, creator } = *created;
            opened_reply(&completed.ctx, &completed.accepted, opened, creator).await
        }
        ResultOf::Blocked { request, error } => {
            blocked(&completed.ctx, &completed.accepted, *request, error).await
        }
        ResultOf::Failed(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(open_failed(&mut conn, &ctx.app.engine.shells(), *error).await?)
        }
    }
}

/// A shell creation's live control ticket is issued only in the original response.
/// Retrying the accepted creation keeps its real identity and reported close outcomes.
pub(super) fn replay_binding() -> crate::middleware::idempotency::Creation<
    ThroughThreadQuery,
    EnvironmentTerminalCreateBody,
    EnvironmentTerminalCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: "terminal_creation",
        replay: |_ctx, _query, _body, created| {
            Box::pin(async move {
                Ok(EnvironmentTerminalCreateResponse::Receipt(
                    TerminalCreateReceipt {
                        projection: orch_api_contract::common::ReceiptProjection::Receipt,
                        terminal_id: created.id,
                        environment_id: serde_json::from_value(
                            created.data.get("environment").cloned().ok_or_else(|| {
                                ApiError::internal("the accepted shell has no native environment")
                            })?,
                        )
                        .map_err(ApiError::internal)?,
                        closed: created
                            .data
                            .get("closed")
                            .cloned()
                            .map(serde_json::from_value)
                            .transpose()
                            .map_err(ApiError::internal)?,
                    },
                ))
            })
        },
        identity: |reply| {
            let (id, environment, closed) = match reply {
                EnvironmentTerminalCreateResponse::Full(value) => (
                    value.session.id,
                    value.session.environment_id,
                    Some(value.closed.clone()),
                ),
                EnvironmentTerminalCreateResponse::Receipt(value) => (
                    value.terminal_id,
                    value.environment_id,
                    value.closed.clone(),
                ),
            };
            let mut data = serde_json::json!({"environment":environment});
            if let Some(closed) = closed {
                data["closed"] = serde_json::json!(closed);
            }
            Ok(orch_db::idempotency::creation::Created {
                kind: "terminal_creation".into(),
                id,
                data,
            })
        },
    }
}
