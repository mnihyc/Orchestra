//! Driving a thread: an input sent, with its delivery and the client's own value that makes sending it again safe, then edited, put in order or cancelled while it waits; a turn stopped; and a draft kept for the composer.
//!
//! What an input needs of its sender is two things on two objects, and the
//! engine asks the access engine for both inside the transaction that takes
//! the input, telling everything that is missing in one refusal; so these
//! requests are declared with a rule and decided there, not before the
//! handler. Anybody who may drive the thread orders it and cancels in it,
//! their own inputs and other people's; the words of an input stay its
//! author's.

use super::{input_failed, read_failed};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Answer, Ctx, Registry};
use crate::thread_access::AuthzAccess;
use http::StatusCode;
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_agent_contract::facts::ProviderFacts;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::personal::Draft;
use orch_api_contract::threads::{
    ContentPart, Delivery, Item, LimitBypass, LimitBypassRequest, LimitReached, ProviderLimit,
    ThreadDraftDelete, ThreadDraftGet, ThreadDraftSet, ThreadDraftSetBody, ThreadInput,
    ThreadInputBypass, ThreadInputCancel, ThreadInputCreate, ThreadInputCreateBody,
    ThreadInputCreateReply, ThreadInputRetry, ThreadInputRun, ThreadInputUpdate,
    ThreadInputUpdateBody, ThreadInputUpdateReply, ThreadInterrupt, ThreadInterruptBody,
    ThreadItemPin, ThreadItemUnpin, ThreadLimits, ThreadLimitsGet, ThreadLimitsGetQuery,
    ThreadQueue, ThreadQueueResume, ThreadQueueResumeBody, ThreadTurnRetry, ThreadTurnRetryBody,
    Turn, TurnState,
};
use orch_authz::resources;
use orch_db::queries::resources::thread_engine as resource_rows;
use orch_db::queries::threads as rows;
use orch_engine::access::{Limit, ResourceAccess};
use orch_engine::inputs::{InputError, InputRequest};
use orch_engine::turns::InterruptError;
use orch_types::{Id, Time};
use std::sync::Arc;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(ThreadInputCreate, input_create)
        .on(ThreadInputUpdate, input_update)
        .on(ThreadInputCancel, input_cancel)
        .on(ThreadInputRetry, input_retry)
        .on(ThreadInterrupt, interrupt)
        .on(ThreadQueueResume, queue_resume)
        .on(ThreadInputBypass, input_bypass)
        .on(ThreadInputRun, input_run)
        .on(orch_api_contract::threads::ThreadPlanDecide, plan_decide)
        .on(ThreadLimitsGet, limits_get)
        .on(ThreadTurnRetry, turn_retry)
        .on(ThreadItemPin, item_pin)
        .on(ThreadItemUnpin, item_unpin)
        .on(ThreadDraftGet, draft_get)
        .on(ThreadDraftSet, draft_set)
        .on(ThreadDraftDelete, draft_delete);
}

/// An error of the engine about an input, told with the database the engine
/// does not hold.
async fn input_error(ctx: &Ctx, thread: Id, error: InputError) -> ApiError {
    match ctx.app.db.acquire().await {
        Ok(mut conn) => input_failed(&mut conn, thread, error).await,
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
) -> ApiResult<Answer<ThreadInputCreateReply>> {
    let thread = ctx.id("id")?;
    if body.native_action.is_some() {
        super::native::refresh_for_input(&ctx, thread).await?;
    }
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let submitted = ctx
        .app
        .engine
        .submit_selected_input_guarded(
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
        )
        .await;
    match submitted {
        Ok(submitted) => {
            let answer = Answer::from(ThreadInputCreateReply {
                input: submitted.input,
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

/// Changes what a waiting input says, which only its author does, and where it
/// stands in the queue and how it is delivered, which whoever drives the thread does.
async fn input_update(
    ctx: Ctx,
    (): (),
    body: ThreadInputUpdateBody,
) -> ApiResult<ThreadInputUpdateReply> {
    let thread = ctx.id("id")?;
    if body.native_action.as_ref().is_some_and(Option::is_some) {
        super::native::refresh_for_input(&ctx, thread).await?;
    }
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

/// Stops a turn that is under way. Nothing is asked of the caller on the
/// provider: stopping is driving.
async fn interrupt(ctx: Ctx, (): (), body: ThreadInterruptBody) -> ApiResult<Turn> {
    let thread = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .interrupt_guarded(tx, guard.into_principal(), thread, body.turn_id)
        .await
    {
        Ok(turn) => Ok(turn),
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
    let thread = ctx.id("id")?;
    ctx.app
        .engine
        .draft(ctx.user()?.id(), thread)
        .await
        .map_err(ApiError::internal)?
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
async fn input_bypass(ctx: Ctx, (): (), body: LimitBypassRequest) -> ApiResult<ThreadInput> {
    let thread = ctx.id("id")?;
    let input = ctx.id("input_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .bypass_input_guarded(tx, guard.into_principal(), thread, input, &body)
        .await
    {
        Ok(input) => Ok(input),
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

/// Runs a message that waits for somebody who may use the thread's model on the caller's own access. Driving
/// the thread and the caller's own use of the model are asked inside the engine's transaction.
async fn input_run(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadInput> {
    let thread = ctx.id("id")?;
    let input = ctx.id("input_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .run_input_guarded(tx, guard.into_principal(), thread, input)
        .await
    {
        Ok(input) => Ok(input),
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

async fn plan_decide(
    ctx: Ctx,
    (): (),
    body: orch_api_contract::threads::ThreadPlanDecideBody,
) -> ApiResult<orch_api_contract::threads::ThreadPlanDecideReply> {
    let thread = ctx.id("id")?;
    let item = ctx.id("item_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .decide_plan_guarded(tx, guard.into_principal(), thread, item, &body)
        .await
    {
        Ok(reply) => Ok(reply),
        Err(error) => Err(input_error(&ctx, thread, error).await),
    }
}

/// The people a record of bypasses names, as others see them, each with whether they acted with As
/// admin on when they let it past.
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
    let row = rows::thread(&mut conn, thread)
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
    let mut reached = crate::routes::usage::caps::reached_for_thread(
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
    let Some(provider) = row.provider_id else {
        return Ok(finish(reached));
    };
    let Some(provider) = resource_rows::provider(&mut conn, provider).await? else {
        return Ok(finish(reached));
    };
    if provider.status != "exhausted" {
        return Ok(finish(reached));
    }
    let acting = ctx.user()?;
    let (person, as_admin) = (acting.id(), acting.as_admin && acting.is_session());
    let access = AuthzAccess::new(Arc::clone(&ctx.app.clock));
    let can_bypass = access
        .may_pass(
            &mut conn,
            person,
            as_admin,
            &Limit::ProviderLimit {
                provider: provider.id,
            },
        )
        .await
        .map_err(ApiError::internal)?;
    let facts: ProviderFacts = serde_json::from_value(provider.facts.clone()).unwrap_or_default();
    let (window, resets_at) = facts.limit_reached.as_ref().map_or_else(
        || {
            facts
                .windows
                .iter()
                .find(|window| window.used_percent >= 100.0)
                .map_or_else(
                    || ("unknown".to_owned(), None),
                    |window| (window.label.clone(), window.resets_at),
                )
        },
        |limit| (limit.kind.clone(), limit.resets_at),
    );
    let provider_ref = serde_json::from_value(
        orch_engine::views::provider_ref_json(&mut conn, &provider)
            .await
            .map_err(ApiError::internal)?,
    )
    .map_err(ApiError::internal)?;
    let entries = bypassed_by(
        &mut conn,
        input.as_ref().and_then(|input| input.limit_bypass.as_ref()),
    )
    .await?;
    let mut passed = None;
    for (entry, as_admin) in entries
        .into_iter()
        .filter(|(entry, _)| entry.provider_limit)
    {
        if access
            .may_pass(
                &mut conn,
                entry.by.id,
                as_admin,
                &Limit::ProviderLimit {
                    provider: provider.id,
                },
            )
            .await
            .map_err(ApiError::internal)?
        {
            passed = Some(entry.by);
            break;
        }
    }
    let (ask, ask_more) = if can_bypass {
        (Vec::new(), 0)
    } else {
        let principal = authorization::principal(&ctx, &mut conn).await?;
        let policies = AuthzAccess::policies(&mut conn, provider.id)
            .await
            .map_err(ApiError::internal)?;
        match resources::check(
            &mut conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: provider.id,
            },
            Permission::ProviderBypass,
            policies.at(Some(provider.home_id)).node_policy(),
        )
        .await
        .map_err(ApiError::internal)?
        {
            Decision::Refused(refusal) => {
                let missing = refusals::missing_access(&mut conn, &refusal.missing).await?;
                missing
                    .into_iter()
                    .next()
                    .map_or((Vec::new(), 0), |first| (first.ask, first.ask_more))
            }
            Decision::Allowed { .. } => (Vec::new(), 0),
        }
    };
    reached.push(LimitReached {
        cap: None,
        provider_limit: Some(ProviderLimit {
            provider: provider_ref,
            window,
            resets_at,
            as_of: Time::new(ctx.now),
        }),
        can_bypass,
        passed_by: passed,
        ask,
        ask_more,
    });
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
    let turn = ctx.id("turn_id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let content: Vec<ContentPart> = {
        let conn = &mut *tx;
        let row = rows::turn(conn, turn)
            .await?
            .filter(|row| row.thread_id == thread)
            .ok_or_else(ApiError::not_found)?;
        if !matches!(
            serde_json::from_value(serde_json::Value::String(row.state.clone())),
            Ok(TurnState::Failed | TurnState::Interrupted)
        ) {
            return Err(ApiError::invalid(
                "turn_id",
                "only a turn that failed or was stopped is tried again",
            ));
        }
        let input = rows::input(conn, row.input_id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        serde_json::from_value(input.content).map_err(ApiError::internal)?
    };
    let submitted = ctx
        .app
        .engine
        .submit_input_guarded(
            tx,
            guard.into_principal(),
            thread,
            InputRequest {
                plan: false,
                content,
                delivery: orch_api_contract::threads::Delivery::Queue,
                client_id: body.client_id,
                bypass: body.bypass,
            },
        )
        .await;
    match submitted {
        Ok(submitted) => {
            let answer = Answer::from(ThreadInputCreateReply {
                input: submitted.input,
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
