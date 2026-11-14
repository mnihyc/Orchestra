//! Driving a thread: an input sent, with its delivery and the client's own value that makes sending it again safe, then edited, put in order or cancelled while it waits; a turn stopped; and a draft kept for the composer.
//!
//! What an input needs of its sender is two things on two objects, and the
//! engine asks the access engine for both inside the transaction that takes
//! the input, telling everything that is missing in one refusal; so these
//! requests are declared with a rule and decided there, not before the
//! handler. Anybody who may drive the thread orders it and cancels in it,
//! their own inputs and other people's; the words of an input stay its
//! author's.

use super::read_failed;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Answer;

use crate::registry::Ctx;

use crate::registry::Registry;

use http::StatusCode;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::personal::Draft;

use orch_api_contract::threads::Delivery;

use orch_api_contract::threads::Item;

use orch_api_contract::threads::LimitBypass;

use orch_api_contract::threads::LimitBypassRequest;

use orch_api_contract::threads::LimitReached;

use orch_api_contract::threads::ThreadDraftDelete;

use orch_api_contract::threads::ThreadDraftGet;

use orch_api_contract::threads::ThreadDraftSet;

use orch_api_contract::threads::ThreadDraftSetBody;

use orch_api_contract::threads::ThreadInput;

use orch_api_contract::threads::ThreadInputBypass;

use orch_api_contract::threads::ThreadInputCancel;

use orch_api_contract::threads::ThreadInputCreate;

use orch_api_contract::threads::ThreadInputCreateBody;

use orch_api_contract::threads::ThreadInputCreateReply;

use orch_api_contract::threads::ThreadInputCreateResponse;

use orch_api_contract::threads::ThreadInputReceipt;

use orch_api_contract::threads::ThreadInputRetry;

use orch_api_contract::threads::ThreadInputRun;

use orch_api_contract::threads::ThreadInputUpdate;

use orch_api_contract::threads::ThreadInputUpdateBody;

use orch_api_contract::threads::ThreadInputUpdateReply;

use orch_api_contract::threads::ThreadInterrupt;

use orch_api_contract::threads::StopState;

use orch_api_contract::threads::ThreadInterruptBody;

use orch_api_contract::threads::ThreadItemPin;

use orch_api_contract::threads::ThreadItemUnpin;

use orch_api_contract::threads::ThreadLimits;

use orch_api_contract::threads::ThreadLimitsGet;

use orch_api_contract::threads::ThreadLimitsGetQuery;

use orch_api_contract::threads::ThreadQueue;

use orch_api_contract::threads::ThreadQueueResume;

use orch_api_contract::threads::ThreadQueueResumeBody;

use orch_api_contract::threads::ThreadStop;

use orch_api_contract::threads::ThreadTurnRetry;

use orch_api_contract::threads::ThreadTurnRetryBody;

use orch_authz::Caller;

use orch_db::queries::threads as rows;

use orch_engine::inputs::InputError;

use orch_engine::inputs::InputRequest;

use orch_engine::turns::InterruptError;

use orch_types::Id;

use orch_types::Time;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_initial_operation(
            ThreadInputCreate,
            super::operation::initial,
            super::operation::prepare,
            input_create,
        )
        .on_with_initial_operation(
            ThreadInputUpdate,
            super::operation::initial_update,
            super::operation::prepare_update,
            input_update,
        )
        .on(ThreadInputCancel, input_cancel)
        .on_with_initial_operation(
            ThreadInputRetry,
            super::operation::initial_retained,
            super::operation::prepare_retained,
            input_retry,
        )
        .on(ThreadInterrupt, interrupt)
        .on(ThreadQueueResume, queue_resume)
        .on_with_operation(
            ThreadInputBypass,
            super::operation::prepare_bypass,
            input_bypass,
        )
        .on_with_initial_operation(
            ThreadInputRun,
            super::operation::initial_retained,
            super::operation::prepare_retained,
            input_run,
        )
        .on_with_operation(
            orch_api_contract::threads::ThreadPlanDecide,
            super::operation::prepare_plan,
            plan_decide,
        )
        .on(ThreadLimitsGet, limits_get)
        .on_with_operation(
            ThreadTurnRetry,
            super::operation::prepare_turn_retry,
            turn_retry,
        )
        .on(ThreadItemPin, item_pin)
        .on(ThreadItemUnpin, item_unpin)
        .on(ThreadDraftGet, draft_get)
        .on(ThreadDraftSet, draft_set)
        .on_with_operation(
            ThreadDraftDelete,
            crate::key_operations::direct::declared,
            draft_delete,
        );
}

/// An error of the engine about an input, told with the database the engine
/// does not hold.
async fn input_error(ctx: &Ctx, thread: Id, error: InputError) -> ApiError {
    match ctx.app.db.acquire().await {
        Ok(mut conn) => {
            super::input_failed_projected(&mut conn, thread, error, ctx.key_receipt_required())
                .await
        }
        Err(error) => error.into(),
    }
}

/// Says something to the agent: queued, put into the turn that runs, or put
/// first with the turn stopped. A value the sender already used for this
/// thread answers with the input that stands.
async fn input_create(
    ctx: Ctx,
    (): (),
    body: ThreadInputCreateBody,
) -> ApiResult<Answer<ThreadInputCreateResponse>> {
    let thread = ctx.id("id")?;
    let builtin = {
        let mut conn = ctx.app.db.acquire().await?;
        orch_db::queries::ask::program_root(&mut conn, thread)
            .await?
            .is_some()
    };
    if builtin {
        return crate::ask::program::input(ctx, body).await;
    }
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let operands = guard.operation_data::<orch_engine::inputs::InputSourceOperands>()?;
    let submitted = ctx
        .app
        .engine
        .submit_selected_input_projected_guarded(
            tx,
            guard.into_principal(),
            thread,
            InputRequest {
                plan: body.plan,
                content: body.content,
                delivery: body.delivery,
                client_id: body.client_id,
                bypass: body.bypass,
            },
            body.native_action,
            ctx.key_receipt_required(),
            Some(operands.as_ref()),
        )
        .await;
    match submitted {
        Ok(submitted) => {
            let response = match (submitted.input, ctx.key_receipt_required()) {
                (Some(input), false) => ThreadInputCreateResponse::full(ThreadInputCreateReply {
                    input,
                    seq: submitted.seq,
                }),
                _ => ThreadInputCreateResponse::Receipt(ThreadInputReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    thread_id: submitted.thread_id,
                    input_id: submitted.input_id,
                    seq: submitted.seq,
                }),
            };
            let answer = Answer::from(response);
            Ok(if submitted.created {
                answer
            } else {
                answer.status(StatusCode::OK)
            })
        }
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

/// Changes what a waiting input says, which only its author does, and where it
/// stands in the queue and how it is delivered, which whoever drives the thread does.
async fn input_update(
    ctx: Ctx,
    (): (),
    body: ThreadInputUpdateBody,
) -> ApiResult<ThreadInputUpdateReply> {
    let thread = ctx.id("id")?;
    let input = ctx.id("input_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let changed = ctx
        .app
        .engine
        .update_input_guarded(tx, guard.into_principal(), thread, input, body)
        .await;
    match changed {
        Ok(input) => Ok(input),
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

/// Cancels a waiting input, whoever sent it.
async fn input_cancel(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadInput> {
    let thread = ctx.id("id")?;
    let input = ctx.id("input_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .cancel_input_guarded(tx, guard.into_principal(), thread, input)
        .await
    {
        Ok(input) => Ok(input),
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

/// Tries again the files that hold an input, for its author: the same input, held again if they still
/// cannot go.
async fn input_retry(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadInput> {
    let thread = ctx.id("id")?;
    let input = ctx.id("input_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .retry_input_guarded(tx, guard.into_principal(), thread, input)
        .await
    {
        Ok(input) => Ok(input),
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

/// Resumes what a person's Stop held, naming the revision of the queue the caller saw.
async fn queue_resume(ctx: Ctx, (): (), body: ThreadQueueResumeBody) -> ApiResult<ThreadQueue> {
    let thread = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .resume_queue_guarded(tx, guard.into_principal(), thread, body.revision)
        .await
    {
        Ok(queue) => Ok(queue),
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

/// Stops a turn that is under way, or the start of the next one. Nothing is asked of the caller on the
/// provider: stopping is driving. The Stop is recorded and answered at once, whatever else holds the thread: where
/// the thread cannot carry it out that moment, the answer says that it is stopping and the engine carries it out
/// as soon as it can.
async fn interrupt(ctx: Ctx, (): (), body: ThreadInterruptBody) -> ApiResult<ThreadStop> {
    let thread = ctx.id("id")?;
    let builtin = {
        let mut conn = ctx.app.db.acquire().await?;
        orch_db::queries::ask::program_root(&mut conn, thread)
            .await?
            .is_some()
    };
    if builtin {
        return crate::ask::program::interrupt(ctx, body).await;
    }
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .stop_guarded(tx, guard.into_principal(), thread, body.turn_id)
        .await
    {
        Ok(stopped) => Ok(stop_answer(stopped)),
        Err(InterruptError::NotFound) => Err(ApiError::not_found()),
        Err(InterruptError::NotRunning { current_turn_id }) => {
            let refusal = ApiError::new(ErrorCode::ThreadNotRunning, "that turn is not under way");
            Err(match current_turn_id {
                Some(current) => refusal.with("current_turn_id", current),
                None => refusal,
            })
        }
        Err(InterruptError::Denied(refusal)) => {
            let mut conn = ctx.app.db.acquire().await?;
            Err(refusals::denied(&mut conn, *refusal).await?)
        }
        Err(InterruptError::Internal(error)) => {
            Err(read_failed(orch_engine::views::ReadError::Internal(error)))
        }
    }
}

/// What a Stop is answered with: stopping until the program has stopped, stopped when nothing is left to wait for.
fn stop_answer(stopped: orch_engine::turns::Stopped) -> ThreadStop {
    let state = match &stopped.turn {
        Some(turn)
            if matches!(
                turn.state,
                orch_api_contract::threads::TurnState::Starting
                    | orch_api_contract::threads::TurnState::Running
            ) =>
        {
            StopState::Stopping
        }
        Some(_) => StopState::Stopped,
        None if stopped.applied => StopState::Stopped,
        None => StopState::Stopping,
    };
    ThreadStop {
        state,
        requested_at: Time::new(stopped.since),
        turn: stopped.turn,
    }
}

/// A stored draft as the wire has it.
fn draft_of(row: rows::DraftRow) -> ApiResult<Draft> {
    let content = serde_json::from_value(row.content).map_err(ApiError::internal)?;
    let delivery = serde_json::from_value(serde_json::Value::String(row.delivery))
        .map_err(ApiError::internal)?;
    Ok(Draft {
        native_action: row
            .native_action
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?,
        content,
        delivery,
        review_comments: serde_json::from_value(row.review_comments).map_err(ApiError::internal)?,
        updated_at: Time::new(row.updated_at),
    })
}

/// The caller's draft for the thread, if they have one.
pub(super) async fn draft_get(ctx: Ctx, (): (), (): ()) -> ApiResult<Option<Draft>> {
    let mut conn = ctx.app.db.acquire().await?;
    draft_in(&ctx, &mut conn).await
}

/// Read the caller's own draft without changing the enclosing operation or connection.
pub(super) async fn draft_in(ctx: &Ctx, conn: &mut orch_db::Conn) -> ApiResult<Option<Draft>> {
    rows::draft(conn, ctx.user()?.id(), ctx.id("id")?)
        .await?
        .map(draft_of)
        .transpose()
}

/// Keeps what the caller has begun to write; drafts are private to their
/// author.
async fn draft_set(ctx: Ctx, (): (), body: ThreadDraftSetBody) -> ApiResult<Draft> {
    let thread = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    rows::lock(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let content_blobs = orch_chat::following::content_blobs(&body.content);
    let mut locked =
        orch_db::queries::storage::private_thread_blobs(&mut tx, guard.user().id(), thread).await?;
    locked.extend_from_slice(&content_blobs);
    locked.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    locked.dedup();
    orch_db::queries::uploads::lock_blobs(&mut tx, &locked).await?;
    for hash in content_blobs {
        crate::attachments::require(&mut tx, guard.principal(), &hash).await?;
        orch_db::queries::uploads::remember(&mut tx, guard.user().id(), &hash, guard.now().get())
            .await?;
    }
    let stored = serde_json::to_value(&body.content).map_err(ApiError::internal)?;
    if let Some(comments) = &body.review_comments {
        let mut ids = std::collections::BTreeSet::new();
        for comment in comments {
            crate::routes::workspace::paths::clean("review_comments.path", &comment.path, false)?;
            if comment.from_line < 1 || comment.to_line < comment.from_line {
                return Err(ApiError::invalid(
                    "review_comments",
                    "a reviewed range starts at one and ends at or after its first line",
                ));
            }
            if !ids.insert(comment.id) {
                return Err(ApiError::invalid(
                    "review_comments",
                    "each frozen comment has one identity",
                ));
            }
        }
    }
    let comments = body
        .review_comments
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(ApiError::internal)?;
    let delivery = match body.delivery {
        Delivery::Queue => "queue",
        Delivery::Steer => "steer",
        Delivery::Interrupt => "interrupt",
    };
    let mut row = rows::set_draft_with_comments(
        &mut tx,
        guard.user().id(),
        thread,
        &stored,
        delivery,
        comments.as_ref(),
        guard.now().get(),
    )
    .await?;
    sqlx::query("DELETE FROM thread_attachment_uploads u WHERE thread_id=$1 AND created_by=$2 AND blob IS NOT NULL AND expires_at IS NULL
      AND NOT EXISTS(SELECT 1 FROM jsonb_array_elements($3::jsonb) part WHERE (part->>'path'=u.path OR (part->>'type'='image' AND part->>'name'=u.name)) AND part->>'blob'=encode(u.blob,'hex'))")
        .bind(thread).bind(guard.user().id()).bind(&stored).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
    sqlx::query("UPDATE thread_attachment_uploads u SET expires_at=NULL WHERE thread_id=$1 AND created_by=$2 AND blob IS NOT NULL
      AND EXISTS(SELECT 1 FROM jsonb_array_elements($3::jsonb) part WHERE (part->>'path'=u.path OR (part->>'type'='image' AND part->>'name'=u.name)) AND part->>'blob'=encode(u.blob,'hex'))")
        .bind(thread).bind(guard.user().id()).bind(&stored).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
    if let Some(selection) = body.native_action {
        let raw = selection
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(ApiError::internal)?;
        row = rows::set_draft_selection(&mut tx, guard.user().id(), thread, raw.as_ref()).await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    draft_of(row)
}

/// Throws the caller's draft away.
async fn draft_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let thread = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    rows::lock(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let locked =
        orch_db::queries::storage::private_thread_blobs(&mut tx, guard.user().id(), thread).await?;
    orch_db::queries::uploads::lock_blobs(&mut tx, &locked).await?;
    rows::delete_draft(&mut tx, guard.user().id(), thread).await?;
    sqlx::query("DELETE FROM thread_attachment_uploads WHERE thread_id=$1 AND created_by=$2 AND blob IS NOT NULL")
        .bind(thread).bind(guard.user().id()).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Pins an item to the thread's outline.
async fn item_pin(ctx: Ctx, (): (), (): ()) -> ApiResult<Item> {
    pin(&ctx, true).await
}

/// Takes the pin off an item.
async fn item_unpin(ctx: Ctx, (): (), (): ()) -> ApiResult<Item> {
    pin(&ctx, false).await
}

async fn pin(ctx: &Ctx, pinned: bool) -> ApiResult<Item> {
    let (thread, item) = (ctx.id("id")?, ctx.id("item_id")?);
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let now = guard.now().get();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, now);
    rows::lock(unit.conn, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    rows::item(unit.conn, item)
        .await?
        .filter(|row| row.thread_id == thread)
        .ok_or_else(ApiError::not_found)?;
    let seq = unit.stamp(thread).await.map_err(ApiError::internal)?;
    rows::pin_item(
        unit.conn,
        item,
        pinned.then_some((guard.user().id(), now)),
        seq,
    )
    .await?;
    let row = rows::item(unit.conn, item)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let people = orch_engine::views::People::load(
        unit.conn,
        [row.author_id, row.pinned_by].into_iter().flatten(),
    )
    .await
    .map_err(ApiError::internal)?
    .with_approvals(unit.conn, orch_engine::views::source_approval(&row))
    .await
    .map_err(ApiError::internal)?;
    let shown = orch_engine::views::item_view(&row, &people);
    unit.finish().await.map_err(ApiError::internal)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(shown)
}

/// Lets a waiting input go past the limits named, in the caller's name. The
/// bypass row of each limit is asked of the access engine inside the
/// engine's transaction, and so is driving the thread.
async fn input_bypass(
    ctx: Ctx,
    (): (),
    body: LimitBypassRequest,
) -> ApiResult<orch_api_contract::threads::ThreadInputMutationResponse> {
    let thread = ctx.id("id")?;
    let input = ctx.id("input_id")?;
    let (mut tx, _initial_guard) = transaction_guard::begin(&ctx).await?;
    ctx.app
        .engine
        .lock_source_control(
            &mut tx,
            thread,
            orch_db::queries::notifications::inbox::RequestSource::Input(input),
            false,
        )
        .await
        .map_err(ApiError::engine)?
        .ok_or_else(ApiError::not_found)?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let homes = guard.operation_data::<Vec<orch_authz::caps::Home>>()?;
    let caller = guard.principal().caller().clone();
    guard.check_deadline(&ctx)?;
    let result = ctx
        .app
        .engine
        .bypass_input_projected_guarded(
            tx,
            guard.principal().clone(),
            thread,
            input,
            &body,
            ctx.key_receipt_required(),
            homes.as_ref(),
            || guard.before_effect(&ctx),
        )
        .await;
    match result {
        Ok(projected) => input_mutation_response(&ctx, &caller, projected).await,
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

/// Runs the same actual held input on this caller's current model access.
async fn input_run(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<orch_api_contract::threads::ThreadInputMutationResponse> {
    let thread = ctx.id("id")?;
    let input = ctx.id("input_id")?;
    let (mut tx, _initial_guard) = transaction_guard::begin(&ctx).await?;
    ctx.app
        .engine
        .lock_source_control(
            &mut tx,
            thread,
            orch_db::queries::notifications::inbox::RequestSource::Input(input),
            true,
        )
        .await
        .map_err(ApiError::engine)?
        .ok_or_else(ApiError::not_found)?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let caller = guard.principal().caller().clone();
    guard.check_deadline(&ctx)?;
    match ctx
        .app
        .engine
        .run_input_projected_guarded(
            tx,
            guard.principal().clone(),
            thread,
            input,
            ctx.key_receipt_required(),
            || guard.before_effect(&ctx),
        )
        .await
    {
        Ok(projected) => input_mutation_response(&ctx, &caller, projected).await,
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

async fn plan_decide(
    ctx: Ctx,
    (): (),
    body: orch_api_contract::threads::ThreadPlanDecideBody,
) -> ApiResult<orch_api_contract::threads::ThreadPlanDecideResponse> {
    let thread = ctx.id("id")?;
    let item = ctx.id("item_id")?;
    let (mut tx, _initial_guard) = transaction_guard::begin(&ctx).await?;
    ctx.app
        .engine
        .lock_source_control(
            &mut tx,
            thread,
            orch_db::queries::notifications::inbox::RequestSource::Plan(item),
            true,
        )
        .await
        .map_err(ApiError::engine)?
        .ok_or_else(ApiError::not_found)?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let caller = guard.principal().caller().clone();
    guard.check_deadline(&ctx)?;
    match ctx
        .app
        .engine
        .decide_plan_projected_guarded(
            tx,
            guard.principal().clone(),
            thread,
            item,
            &body,
            ctx.key_receipt_required(),
            || guard.before_effect(&ctx),
        )
        .await
    {
        Ok((projected, decision)) => {
            let receipt = input_receipt(&projected);
            match projected.input {
                Some(input) if control_content(&ctx, &caller, thread).await? => {
                    Ok(orch_api_contract::threads::ThreadPlanDecideResponse::full(
                        orch_api_contract::threads::ThreadPlanDecideReply { input, decision },
                    ))
                }
                _ => Ok(orch_api_contract::threads::ThreadPlanDecideResponse::Receipt(receipt)),
            }
        }
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

fn input_receipt(projected: &orch_engine::inputs::SubmittedProjection) -> ThreadInputReceipt {
    ThreadInputReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        thread_id: projected.thread_id,
        input_id: projected.input_id,
        seq: projected.seq,
    }
}

async fn input_mutation_response(
    ctx: &Ctx,
    caller: &Caller,
    projected: orch_engine::inputs::SubmittedProjection,
) -> ApiResult<orch_api_contract::threads::ThreadInputMutationResponse> {
    let receipt = input_receipt(&projected);
    match projected.input {
        Some(input) if control_content(ctx, caller, projected.thread_id).await? => {
            Ok(orch_api_contract::threads::ThreadInputMutationResponse::full(input))
        }
        _ => Ok(orch_api_contract::threads::ThreadInputMutationResponse::Receipt(receipt)),
    }
}

/// Only optional response Read is revisited after commit; no effect or native writer is replayed.
async fn control_content(ctx: &Ctx, caller: &Caller, thread: Id) -> ApiResult<bool> {
    use crate::key_operations::{admission::Prepared, resolver::Disclosure, targets};
    use orch_access_contract::permissions::Permission;
    use orch_authz::{Act, Policy};
    if ctx.key_receipt_required() {
        return Ok(false);
    }
    let content = crate::key_operations::completed_projection::run(ctx, async {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let target = targets::node_identity(&mut tx, thread).await?;
        let selection = caller.admitted_operation().unwrap_or(ctx.route.name);
        let mut plan = Prepared::new(ctx.route, selection)?;
        orch_engine::inputs::operation::bind_thread(plan.shared_mut(), &target)
            .map_err(crate::key_operations::admission::binding_error)?;
        let read = plan
            .read_response(ctx, &mut tx, caller, move |conn, reader| {
                Box::pin(async move {
                    authorization::require(
                        conn,
                        reader,
                        thread,
                        authorization::exact(Permission::ThreadView),
                        Act::Use,
                        Policy::default(),
                    )
                    .await
                })
            })
            .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(read)
    })
    .await;
    match content {
        Ok(content) => Ok(content.is_some()),
        // This is only the completed operation's ordinary optional Read stage.
        // Required effect failures keep their original status through input_error.
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

/// The people a record of bypasses names, as others see them, each with whether they acted with As
/// admin on when they let it past.
#[expect(
    dead_code,
    reason = "retained native bypass projection during shared reader integration"
)]
async fn bypassed_by(
    conn: &mut orch_db::Conn,
    stored: Option<&serde_json::Value>,
) -> ApiResult<Vec<(LimitBypass, bool)>> {
    let entries = stored
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ids: Vec<Id> = entries
        .iter()
        .filter_map(|entry| entry.get("by").and_then(serde_json::Value::as_str))
        .filter_map(|id| id.parse().ok())
        .collect();
    let people = refusals::user_refs(conn, &ids).await?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let by = entry
                .get("by")
                .and_then(serde_json::Value::as_str)?
                .parse::<Id>()
                .ok()?;
            let as_admin = entry
                .get("as_admin")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            Some((
                LimitBypass {
                    by: people.get(&by)?.clone(),
                    at: serde_json::from_value(entry.get("at")?.clone()).ok()?,
                    caps: serde_json::from_value(entry.get("caps")?.clone()).unwrap_or_default(),
                    provider_limit: entry
                        .get("provider_limit")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                },
                as_admin,
            ))
        })
        .collect())
}

/// The limits that stand in the way of a turn of the thread, and which of them
/// the caller may let past, evaluated for the input's author and the next turn's model and tier.
pub(super) async fn limits_get(
    ctx: Ctx,
    query: ThreadLimitsGetQuery,
    (): (),
) -> ApiResult<ThreadLimits> {
    let thread = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let _row = rows::thread(&mut conn, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let input = match query.input_id {
        Some(id) => Some(
            rows::input(&mut conn, id)
                .await?
                .filter(|input| input.thread_id == thread)
                .ok_or_else(ApiError::not_found)?,
        ),
        None => None,
    };
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let reached = crate::routes::usage::caps::reached_for_thread(
        &ctx,
        &mut conn,
        &principal,
        thread,
        input.as_ref(),
    )
    .await?;
    let finish = |reached: Vec<LimitReached>| ThreadLimits {
        bypass_all: reached.iter().any(|limit| limit.passed_by.is_none())
            && reached
                .iter()
                .filter(|limit| limit.passed_by.is_none())
                .all(|limit| limit.can_bypass),
        reached,
    };
    // Only configured Orch caps belong in the bypass prompt. Vendor reports
    // remain on the provider's observational view and require no override.
    Ok(finish(reached))
}

/// Queues a copy of the input of a turn that failed or was stopped, as the
/// caller's own input, checked like any other.
async fn turn_retry(
    ctx: Ctx,
    (): (),
    body: ThreadTurnRetryBody,
) -> ApiResult<Answer<ThreadInputCreateReply>> {
    let thread = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let operands = guard.operation_data::<super::operation::TurnRetryOperands>()?;
    if orch_db::queries::ask::program_root(&mut tx, thread)
        .await?
        .is_some()
    {
        return crate::ask::program::retry_in(ctx, tx, guard, body, operands.content.clone()).await;
    }
    let submitted = ctx
        .app
        .engine
        .submit_selected_input_projected_guarded(
            tx,
            guard.into_principal(),
            thread,
            InputRequest {
                plan: false,
                content: operands.content.clone(),
                delivery: orch_api_contract::threads::Delivery::Queue,
                client_id: body.client_id,
                bypass: body.bypass,
            },
            None,
            false,
            Some(&operands.sources),
        )
        .await;
    match submitted {
        Ok(submitted) => {
            let answer = Answer::from(ThreadInputCreateReply {
                input: submitted.input.ok_or_else(|| {
                    ApiError::internal("the full retry projection is unavailable")
                })?,
                seq: submitted.seq,
            });
            Ok(if submitted.created {
                answer
            } else {
                answer.status(StatusCode::OK)
            })
        }
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}
