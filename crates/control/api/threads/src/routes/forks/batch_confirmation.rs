//! Kept native batch confirmation through the shared once-only preparation and effect factory.

use super::preparation::engine_error;

use super::preparation::error;

use super::preparation::source_error;

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::publication;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_api_contract::fork_sources as rows;

use orch_api_contract::fork_sources::PlanSource as Role;

use orch_api_contract::forks::ForkBatchCreateReceipt;

use orch_api_contract::forks::ForkBatchCreateResponse;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_engine::fork_plan::operation as source;

use orch_engine::fork_plan::operation::BatchConfirm as Intent;

use orch_engine::jobs::batch::BatchConfirmation;

use orch_types::Time;

use std::sync::Arc;
const ROW: &Declaration<Role> = &rows::BATCH_CONFIRM;

pub(super) struct Create;

pub(super) struct Input {
    native: BatchConfirmation,
    inputs: Vec<source::Input>,
}

pub(super) struct Fetched {
    native: BatchConfirmation,
    inputs: Vec<source::Input>,
    drafts: Vec<Option<Draft>>,
    origins: Vec<source::Origin>,
    until: Option<Time>,
}

pub(super) struct Value {
    fetched: Arc<Fetched>,
    native: BatchConfirmation,
    drafts: Vec<Option<Draft>>,
    origins: Vec<source::Origin>,
}

pub(super) struct Accepted {
    outcome: super::batch_reply::Outcome,
    receipt: ForkBatchCreateReceipt,
}

async fn inputs(conn: &mut Conn, native: &BatchConfirmation) -> ApiResult<Vec<source::Input>> {
    let mut values = Vec::new();
    for member in native.selections() {
        values.push(
            source::Input::selected(conn, member.selected())
                .await
                .map_err(engine_error)?,
        );
    }
    Ok(values)
}

async fn facts(
    conn: &mut Conn,
    native: &BatchConfirmation,
    inputs: &[source::Input],
) -> ApiResult<Vec<native::Facts<Role>>> {
    if inputs.len() != native.selections().len() {
        return Err(ApiError::internal(
            "the native confirmation source count changed",
        ));
    }
    let mut roots = Vec::new();
    let mut pages = Vec::new();
    for input in inputs {
        for fact in source::facts::<Intent>(conn, input)
            .await
            .map_err(source_error)?
        {
            let targets = fact.groups.into_iter().flat_map(|(_, targets)| targets);
            match fact.source {
                Role::Sources => roots.extend(targets),
                Role::CopiedPages => pages.extend(targets),
                _ => return Err(ApiError::internal("another native batch role was supplied")),
            }
        }
    }
    if let Some(id) = native.stored().body.all_of_environment {
        roots.push(
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?,
        );
    }
    Ok(vec![
        super::batch_preparation::collection(Role::Sources, roots),
        super::batch_preparation::collection(Role::CopiedPages, pages),
    ])
}

impl Fetched {
    fn check(&self, ctx: &Ctx, caller: &Caller) -> ApiResult<()> {
        if self
            .until
            .is_some_and(|until| ctx.app.clock.time() >= until)
        {
            return Err(ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the original batch request has ended",
            ));
        }
        for origin in &self.origins {
            origin
                .deadline(ctx.app.clock.as_ref(), caller)
                .map_err(engine_error)?;
        }
        Ok(())
    }
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ForkCreateBody,
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = effects::prepare::<Create>(ctx, conn, caller, query, body).await?;
        for origin in &effects::initial_value::<Create>(ctx)?.origins {
            plan = plan
                .with_origin(ctx, conn, caller, origin.captured())
                .await?;
        }
        Ok(plan)
    })
}

impl effects::Initial for Create {
    type Input = Input;
    type Fetched = Fetched;
    fn declaration(_input: &Input) -> &'static Declaration<Role> {
        ROW
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let native = match ctx
                .app
                .engine
                .select_batch_confirmation(conn, caller, &body)
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            let inputs = inputs(conn, &native).await?;
            let facts = facts(conn, &native, &inputs).await?;
            Ok((Input { native, inputs }, facts))
        })
    }
    fn require_initial<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let (credential, until) =
                crate::routes::threads::adopt::operation::http_origin(ctx).await?;
            let mut sources = Vec::new();
            for input in &input.inputs {
                sources.push(source::Preparation::new(
                    source::Origin::capture::<Intent>(
                        caller,
                        input.clone(),
                        credential.clone(),
                        until,
                    )
                    .map_err(engine_error)?,
                ));
            }
            let mut conn = ctx.app.db.acquire().await?;
            let drafts = match ctx
                .app
                .engine
                .assemble_batch_confirmation(&mut conn, &input.native, Some(&sources))
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, &mut conn, why).await),
            };
            let mut origins = Vec::new();
            for source in sources {
                origins.push(source.retained().await);
            }
            Ok(Fetched {
                native: input.native,
                inputs: input.inputs,
                drafts,
                origins,
                until,
            })
        })
    }
}

impl effects::Effect for Create {
    type Source = Role;
    type Query = ();
    type Body = ForkCreateBody;
    type Value = Value;
    type Output = Accepted;
    fn declaration(_value: &Value) -> &'static Declaration<Role> {
        ROW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Create>(ctx)?;
            if fetched.native.body() != &body {
                return Err(ApiError::internal(
                    "the native batch confirmation has another request",
                ));
            }
            // All confirmation consumers take the private plan before its sorted native source rows.
            orch_db::queries::jobs::forks::lock_preflight(conn, body.preflight_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            source::lock_all(conn, &fetched.inputs)
                .await
                .map_err(source_error)?;
            if let Some(id) = fetched.native.stored().body.all_of_environment {
                sqlx::query("SELECT id FROM environments WHERE id=$1 FOR KEY SHARE")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
            let native = match ctx
                .app
                .engine
                .prepare_batch_confirmation(conn, caller, &body, fetched.native.batch())
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            if !fetched.native.preserves(&native) {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the native batch confirmation selection changed",
                ));
            }
            let selected = inputs(conn, &native).await?;
            let mut drafts = Vec::new();
            let mut origins = Vec::new();
            for (member, input) in native.selections().iter().zip(&selected) {
                let index = fetched
                    .native
                    .selections()
                    .iter()
                    .position(|old| old.member().thread == member.member().thread)
                    .ok_or_else(|| ApiError::internal("an unobserved batch member was supplied"))?;
                if &fetched.inputs[index] != input {
                    return Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the native batch source changed",
                    ));
                }
                drafts.push(fetched.drafts[index].clone());
                origins.push(fetched.origins[index].clone());
            }
            let facts = facts(conn, &native, &selected).await?;
            let mut plan = native::bind_required(ctx, ROW, &facts)?;
            for origin in &fetched.origins {
                plan = plan
                    .with_origin(ctx, conn, caller, origin.captured())
                    .await?;
            }
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            fetched.check(ctx, principal.caller())?;
            Ok((
                Value {
                    fetched,
                    native,
                    drafts,
                    origins,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { value.fetched.check(ctx, principal.caller()) })
    }
}

impl effects::ApplyOwned for Create {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Accepted>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let queued = ctx
                .app
                .engine
                .commit_batch_confirmation_with_source(
                    tx,
                    principal,
                    &value.native,
                    value.drafts.clone(),
                    &value.origins,
                    || {
                        value
                            .fetched
                            .check(ctx, principal.caller())
                            .map_err(|error| {
                                orch_engine::EngineError::BeforeEffect(Box::new(error))
                            })?;
                        before.check().map_err(|error| {
                            orch_engine::EngineError::BeforeEffect(Box::new(error))
                        })
                    },
                    || {
                        value
                            .fetched
                            .check(ctx, principal.caller())
                            .map_err(|error| {
                                orch_engine::EngineError::BeforeEffect(Box::new(error))
                            })?;
                        deadline.check().map_err(|error| {
                            orch_engine::EngineError::BeforeEffect(Box::new(error))
                        })
                    },
                )
                .await;
            let (batch, rows) = match queued {
                Ok(value) => value,
                Err(why) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    return Err(error(ctx, &mut conn, why).await);
                }
            };
            let roots = match value.native.stored().body.all_of_environment {
                Some(id) => vec![ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                }],
                None => rows
                    .iter()
                    .filter_map(|row| row.node_id)
                    .map(|id| ObjectRef {
                        kind: ObjectKind::Thread,
                        id,
                    })
                    .collect(),
            };
            Ok(effects::Outcome::Applied(Accepted {
                receipt: ForkBatchCreateReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    batch_id: batch,
                    queued: rows.len(),
                },
                outcome: super::batch_reply::Outcome {
                    batch,
                    rows,
                    roots,
                    actor: principal.user(),
                    replay: false,
                },
            }))
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _body: ForkCreateBody,
) -> ApiResult<ForkBatchCreateResponse> {
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    guard.check_deadline(&ctx)?;
    let done = effects::apply_owned::<Create>(&ctx, tx).await?;
    publication::receipt(&done.ctx, &done.output.receipt)?;
    let receipt = done.output.receipt;
    match super::batch_reply::render(&done.ctx, &done.accepted, done.output.outcome).await? {
        Some(value) => Ok(ForkBatchCreateResponse::Full(Box::new(value))),
        None => Ok(ForkBatchCreateResponse::Receipt(receipt)),
    }
}
