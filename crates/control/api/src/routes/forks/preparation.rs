//! The native preflight selector, once-only suppliers and accepted private-plan publication.
use super::*;
use crate::key_operations::{
    native::{self, effects, reply},
    publication, targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyTargetKind};
use orch_api_contract::{
    fork_sources::PlanSource as Role,
    forks::{ForkPlanReceipt, ForkPlanResponse},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_engine::fork_plan::{PreparedFork, operation as source};
use std::{collections::BTreeMap, sync::Arc};

pub(super) struct Preview<I: source::Intent = source::Preview>(std::marker::PhantomData<I>);
pub(super) struct Input {
    prepared: PreparedFork,
    source: source::Input,
}
pub(super) struct Fetched {
    prepared: PreparedFork,
    draft: Draft,
    source: source::Input,
    origin: source::Origin,
    audience: Option<orch_engine::fork_plan::moving::Audience>,
    open_sides: Vec<orch_api_contract::nodes::NodeRef>,
}
pub(super) struct Value {
    fetched: Arc<Fetched>,
    draft: Draft,
}
pub(super) struct Accepted {
    plan: ForkPlan,
    source: source::Input,
    disclosure: Vec<ObjectRef>,
    actor: Id,
    origin: source::Origin,
    audience: Option<orch_engine::fork_plan::moving::Audience>,
    open_sides: Vec<orch_api_contract::nodes::NodeRef>,
}
fn declaration<I: source::Intent>(result: ForkResult) -> &'static Declaration<Role> {
    if result == ForkResult::New {
        I::ROW.fork.unwrap_or(I::ROW.default)
    } else {
        I::ROW.switch.unwrap_or(I::ROW.default)
    }
}
pub(super) fn source_error(error: orch_engine::operation_scope::continuing::Error) -> ApiError {
    match error {
        orch_engine::operation_scope::continuing::Error::Native(error) => ApiError::engine(error),
        error => ApiError::new(ErrorCode::AuthzDenied, error.to_string()),
    }
}
pub(super) async fn error(ctx: &Ctx, conn: &mut Conn, error: PlanError) -> ApiError {
    let error = match error {
        PlanError::Internal(error) => engine_error(error),
        error => plan_failed(conn, error).await,
    };
    if ctx.key_receipt_required() {
        error.without_content()
    } else {
        error
    }
}
pub(super) fn engine_error(error: orch_engine::EngineError) -> ApiError {
    match error {
        orch_engine::EngineError::BeforeEffect(error) => match error.downcast::<ApiError>() {
            Ok(error) => *error,
            Err(_) => ApiError::new(
                ErrorCode::AuthzDenied,
                "the original native operation is no longer available",
            ),
        },
        error => ApiError::engine(error),
    }
}
async fn primary(conn: &mut Conn, id: Id) -> ApiResult<native::Facts<Role>> {
    let target = targets::node(conn, id).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
        return Err(ApiError::not_found());
    }
    Ok(native::Facts::one(Role::Thread, target.object.kind, target))
}
pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ForkRequest,
) -> crate::registry::OperationFuture<'a> {
    prepare_for::<source::Preview>(ctx, conn, caller, query, body)
}
pub(super) fn prepare_move<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ForkRequest,
) -> crate::registry::OperationFuture<'a> {
    prepare_for::<source::MovePreview>(ctx, conn, caller, query, body)
}
fn prepare_for<'a, I: source::Intent>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ForkRequest,
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        let plan = effects::prepare::<Preview<I>>(ctx, conn, caller, query, body).await?;
        let fetched = effects::initial_value::<Preview<I>>(ctx)?;
        plan.with_origin(ctx, conn, caller, fetched.origin.captured())
            .await
    })
}
impl<I: source::Intent> effects::Initial for Preview<I> {
    type Input = Input;
    type Fetched = Fetched;
    fn declaration(input: &Input) -> &'static Declaration<Role> {
        declaration::<I>(input.prepared.request().result)
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkRequest,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let body = source::request_for::<I>(body).map_err(engine_error)?;
            let id = ctx.id("id")?;
            let row = declaration::<I>(body.result);
            let mut prefix = vec![primary(conn, id).await?];
            prefix.extend(
                source::move_prefix::<I>(conn, id, &body)
                    .await
                    .map_err(engine_error)?,
            );
            let known = native::bind_known(ctx, row, &prefix)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let prepared = match ctx
                .app
                .engine
                .prepare_fork_in(
                    conn,
                    principal.caller(),
                    id,
                    &body,
                    orch_engine::fork_plan::Reach::Seen,
                    None,
                    None,
                )
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            let input = source::Input::selected_for::<I>(conn, &prepared)
                .await
                .map_err(ApiError::engine)?;
            let facts = source::facts::<I>(conn, &input)
                .await
                .map_err(source_error)?;
            Ok((
                Input {
                    prepared,
                    source: input,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if let Some(review) = input.source.moving() {
                orch_engine::fork_plan::moving::require::<I>(conn, principal, review)
                    .await
                    .map_err(engine_error)?;
            }
            Ok(())
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let (credential, until) =
                crate::routes::threads::adopt::operation::http_origin(ctx).await?;
            let origin =
                source::Origin::capture::<I>(caller, input.source.clone(), credential, until)
                    .map_err(ApiError::engine)?;
            let preparation = source::Preparation::new(origin.clone());
            let mut conn = ctx.app.db.acquire().await?;
            let mut draft = match ctx
                .app
                .engine
                .assemble_prepared_fork_with_source(&mut conn, &input.prepared, Some(&preparation))
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, &mut conn, why).await),
            };
            let open_sides = match input.source.moving() {
                Some(review) => {
                    orch_engine::fork_plan::moving::review_sides(&mut conn, review, &mut draft)
                        .await
                        .map_err(engine_error)?
                }
                None => Vec::new(),
            };
            let audience = if I::KIND.is_move() {
                Some(
                    orch_engine::fork_plan::moving::audience(
                        &mut conn,
                        input.source.thread(),
                        caller.user(),
                        ctx.app.clock.time(),
                    )
                    .await
                    .map_err(engine_error)?,
                )
            } else {
                None
            };
            Ok(Fetched {
                prepared: input.prepared,
                draft,
                source: input.source,
                origin: preparation.retained().await,
                audience,
                open_sides,
            })
        })
    }
}
impl<I: source::Intent> effects::Effect for Preview<I> {
    type Source = Role;
    type Query = ();
    type Body = ForkRequest;
    type Value = Value;
    type Output = Accepted;
    fn declaration(value: &Value) -> &'static Declaration<Role> {
        declaration::<I>(value.draft.plan.result)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkRequest,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let body = source::request_for::<I>(body).map_err(engine_error)?;
            let fetched = effects::initial_value::<Preview<I>>(ctx)?;
            if fetched.source.request() != &body || fetched.source.thread() != ctx.id("id")? {
                return Err(ApiError::internal(
                    "the native fork observation has another request",
                ));
            }
            source::lock(conn, &fetched.source)
                .await
                .map_err(source_error)?;
            let facts = source::facts::<I>(conn, &fetched.source)
                .await
                .map_err(source_error)?;
            let plan = native::bind_required(ctx, declaration::<I>(body.result), &facts)?
                .with_origin(ctx, conn, caller, fetched.origin.captured())
                .await?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let actual = match ctx
                .app
                .engine
                .prepare_fork_in(
                    conn,
                    principal.caller(),
                    ctx.id("id")?,
                    &body,
                    orch_engine::fork_plan::Reach::Seen,
                    None,
                    None,
                )
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            if !fetched.prepared.preserves(&actual)
                || source::Input::selected_for::<I>(conn, &actual)
                    .await
                    .map_err(ApiError::engine)?
                    != fetched.source
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the native fork selection changed",
                ));
            }
            let mut draft = fetched.draft.clone();
            let topic = actual.source().topic_id;
            complete(conn, &principal, &mut draft, topic).await?;
            Ok((Value { fetched, draft }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if let Some(review) = value.fetched.origin.input().moving() {
                orch_engine::fork_plan::moving::require::<I>(conn, principal, review)
                    .await
                    .map_err(engine_error)?;
            }
            value
                .fetched
                .origin
                .deadline(ctx.app.clock.as_ref(), principal.caller())
                .map_err(engine_error)
        })
    }
}
impl<I: source::Intent> effects::Apply for Preview<I> {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Accepted>>> {
        Box::pin(async move {
            let plan = match ctx
                .app
                .engine
                .keep_fork_plan_on(conn, principal.caller(), value.draft.clone(), move || {
                    value
                        .fetched
                        .origin
                        .deadline(ctx.app.clock.as_ref(), principal.caller())?;
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                })
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            if let Some(review) = value.fetched.source.moving() {
                sqlx::query("UPDATE preflights SET plan=jsonb_set(jsonb_set(plan,'{moving}',$2),'{move_people}',$3) WHERE id=$1")
                    .bind(plan.preflight_id)
                    .bind(serde_json::to_value(review).map_err(ApiError::internal)?)
                    .bind(serde_json::to_value(&value.fetched.audience).map_err(ApiError::internal)?)
                    .execute(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
            Ok(effects::Outcome::Applied(Accepted {
                plan,
                source: value.fetched.source.clone(),
                disclosure: value.draft.disclosure.clone(),
                actor: principal.user(),
                origin: value.fetched.origin.clone(),
                audience: value.fetched.audience.clone(),
                open_sides: value.fetched.open_sides.clone(),
            }))
        })
    }
}
async fn run_for<I: source::Intent>(
    ctx: Ctx,
) -> ApiResult<orch_api_contract::duplicates::ThreadMoveOutPreviewResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Preview<I>>(&ctx, &mut tx).await?;
    guard.check_deadline(&done.ctx)?;
    done.output
        .origin
        .deadline(done.ctx.app.clock.as_ref(), &done.accepted)
        .map_err(engine_error)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let executable = usize::from(done.output.plan.blockers.is_empty());
    let receipt = ForkPlanReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        preflight_id: done.output.plan.preflight_id,
        planned: 1,
        executable,
        blocked: 1 - executable,
    };
    if I::KIND.is_move() {
        publication::receipt(
            &done.ctx,
            &orch_api_contract::duplicates::ThreadMoveOutPreviewResponse {
                plan: ForkPlanResponse::Receipt(receipt.clone()),
                delete_plan: None,
                notification: None,
                open_side_threads: None,
            },
        )?;
    } else {
        publication::receipt(&done.ctx, &receipt)?;
    }
    let delete = done
        .output
        .source
        .moving()
        .map(orch_engine::fork_plan::moving::Review::public);
    let notification = done.output.audience.as_ref().map(|audience| {
        orch_api_contract::duplicates::ChatDuplicateAudience {
            people: audience.count,
            notify_limit: orch_engine::fork_plan::moving::NOTICE_LIMIT,
            preview_id: (audience.count > 0
                && audience.count <= u64::from(orch_engine::fork_plan::moving::NOTICE_LIMIT))
            .then_some(done.output.plan.preflight_id),
        }
    });
    let open_sides = Some(done.output.open_sides.clone());
    match reply::render::<Readback<I>>(&done.ctx, &done.accepted, done.output).await? {
        Some(plan) => Ok(
            orch_api_contract::duplicates::ThreadMoveOutPreviewResponse {
                plan: ForkPlanResponse::Full(Box::new(plan)),
                delete_plan: delete,
                notification,
                open_side_threads: open_sides,
            },
        ),
        None => Ok(
            orch_api_contract::duplicates::ThreadMoveOutPreviewResponse {
                plan: ForkPlanResponse::Receipt(receipt),
                delete_plan: None,
                notification: None,
                open_side_threads: None,
            },
        ),
    }
}
pub(super) async fn run(ctx: Ctx, (): (), _: ForkRequest) -> ApiResult<ForkPlanResponse> {
    run_for::<source::Preview>(ctx)
        .await
        .map(|reply| reply.plan)
}
pub(super) async fn run_move(
    ctx: Ctx,
    (): (),
    _: ForkRequest,
) -> ApiResult<orch_api_contract::duplicates::ThreadMoveOutPreviewResponse> {
    run_for::<source::MovePreview>(ctx).await
}
struct Readback<I>(std::marker::PhantomData<I>);
impl<I: source::Intent> reply::Projection for Readback<I> {
    type Source = Role;
    type Input = Accepted;
    type Subject = orch_db::queries::jobs::forks::PreflightRow;
    type Value = Vec<ObjectRef>;
    type Reply = ForkPlan;
    const READ_SOURCE: Role = Role::Subjects;
    fn declaration(input: &Accepted) -> &'static Declaration<Role> {
        declaration::<I>(input.plan.result)
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let row = orch_db::queries::jobs::forks::preflight(conn, input.plan.preflight_id)
                .await?
                .filter(|row| {
                    row.created_by == input.actor && row.node_id == Some(input.source.thread())
                })
                .ok_or_else(ApiError::not_found)?;
            let stored: orch_engine::fork_plan::StoredPlan =
                serde_json::from_value(row.plan.clone()).map_err(ApiError::internal)?;
            if stored.plan != input.plan {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the native retained plan changed",
                ));
            }
            // Publication binds the completed subject, without repeating native admission.
            let mut facts = vec![primary(conn, input.source.thread()).await?];
            if let Some(review) = input.source.moving() {
                facts.push(native::Facts::one(
                    Role::MoveDestination,
                    KeyTargetKind::Object(ObjectKind::Thread),
                    targets::node(conn, review.destination()).await?,
                ));
                facts.push(native::Facts::collection(
                    Role::MoveResources,
                    orch_engine::fork_plan::moving::resources(conn, review)
                        .await
                        .map_err(engine_error)?,
                ));
            }

            let pages = match &input.source.request().access {
                Some(orch_api_contract::access::ThreadAccess::As(page))
                    if page.source != input.source.thread() =>
                {
                    vec![targets::node(conn, page.source).await?]
                }
                _ => Vec::new(),
            };
            facts.push(native::Facts::collection(
                Role::CopiedPages,
                if pages.is_empty() {
                    vec![]
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Thread), pages)]
                },
            ));
            let target = targets::node(conn, input.source.thread()).await?;
            facts.push(native::Facts::collection(
                Role::Subjects,
                vec![(target.object.kind, vec![target])],
            ));
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        _subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require(
                conn,
                principal,
                input.source.thread(),
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Accepted,
        _subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let mut groups = BTreeMap::<KeyTargetKind, Vec<_>>::new();
            for object in &input.disclosure {
                let target = targets::page_object(conn, *object).await?;
                groups
                    .entry(KeyTargetKind::Object(object.kind))
                    .or_default()
                    .push(target);
            }
            Ok((
                input.disclosure.clone(),
                vec![native::Facts::collection(
                    Role::Content,
                    groups.into_iter().collect(),
                )],
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Accepted,
        _prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ForkPlan>> {
        Box::pin(async move { Ok(input.plan.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.plan == fresh.subject.plan
                && old.value == fresh.value)
        })
    }
}
