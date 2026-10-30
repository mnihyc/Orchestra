//! Fork and switch plans with their options, and execution, single and batch; side threads started and listed.
//!
//! A fork or a switch is planned first and carried out by naming the plan: the engine works the plan out
//! ([`orch_engine::fork_plan`]), and what only this layer knows is added before it is kept: the names inside
//! what the caller lacks, and the page the result would have. Carrying a plan out makes a job and, for a
//! switch, holds the thread in the same transaction; a batch makes one job for each plan without blockers.
//! A side thread is the fork with nothing to choose and is made by one request; every request on it, by
//! its own identifier, is asked of its parent.

use super::threads::one;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::environments::jobs as job_view;
use crate::routes::nodes::view as node_view;
use orch_access_contract::permissions::{ObjectKind, Permission, Profile};
use orch_api_contract::access::PageProfile;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::forks::{
    ForkBatch, ForkBatchCancel, ForkBatchCreate, ForkBatchGet, ForkBatchJob, ForkBatchPlan,
    ForkBatchPreflight, ForkBatchPreflightBody, ForkCreateBody, ForkNowBody, ForkNowReply,
    ForkPlan, ForkRequest, ForkResult, ThreadForkCreate, ThreadForkNow, ThreadForkPreflight,
};
use orch_api_contract::threads::{
    SideState, Thread, ThreadSideCreate, ThreadSideCreateBody, ThreadSideCreateReply,
    ThreadSideList, ThreadSideListQuery,
};
use orch_api_contract::workspace::Job;
use orch_authz::{Act, Policy, Principal, nodes};
use orch_db::Conn;
use orch_db::queries::jobs::JobRow;
use orch_db::queries::threads as thread_rows;
use orch_engine::fork_plan::{Draft, PlanError};
use orch_engine::jobs::batch::{BatchDraft, counts_of, percent_of_batch};
use orch_engine::side_threads::SideError;
use orch_types::Id;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(ThreadForkPreflight, preflight)
        .on(ThreadForkCreate, create)
        .on(ThreadForkNow, now)
        .on(ForkBatchPreflight, batch_preflight)
        .on(ForkBatchCreate, batch_create)
        .on(ForkBatchGet, batch_get)
        .on(ForkBatchCancel, batch_cancel)
        .on(ThreadSideCreate, side_create)
        .on(ThreadSideList, side_list);
}

// ---------------------------------------------------------------- the answer to a request that fails

/// The blockers of a plan with the names inside what the caller lacks.
async fn blockers_view(conn: &mut Conn, draft: &mut Draft) -> ApiResult<()> {
    for (index, missing) in std::mem::take(&mut draft.lacks) {
        let named = refusals::missing_access(conn, &missing).await?;
        if let Some(blocker) = draft.plan.blockers.get_mut(index) {
            blocker.missing = Some(named);
        }
    }
    Ok(())
}

/// Why a plan could not be made or carried out, as the problem a client acts on.
async fn plan_failed(conn: &mut Conn, error: PlanError) -> ApiError {
    match error {
        PlanError::NotFound | PlanError::NoSuchPlan => ApiError::not_found(),
        PlanError::Archived => ApiError::new(
            ErrorCode::NodeArchived,
            "the thread is archived, and so is read-only",
        ),
        PlanError::Side => ApiError::new(
            ErrorCode::ThreadSide,
            "a side thread is not switched, moved, shared or archived",
        ),
        PlanError::Invalid { path, message } => ApiError::invalid(&path, &message),
        PlanError::Expired => ApiError::new(
            ErrorCode::PreflightExpired,
            "the plan is older than fifteen minutes: make a new one",
        ),
        PlanError::Held(job) => ApiError::new(
            ErrorCode::ConflictExists,
            "a fork or a switch holds this thread already",
        )
        .with("job_id", job),
        PlanError::NothingExecutable => ApiError::new(
            ErrorCode::PlanBlocked,
            "none of the plans can be carried out",
        ),
        PlanError::Blocked(mut draft) => {
            if let Err(error) = blockers_view(conn, &mut draft).await {
                return error;
            }
            ApiError::new(
                ErrorCode::PlanBlocked,
                draft
                    .plan
                    .blockers
                    .first()
                    .map_or("the plan cannot be carried out", |blocker| {
                        blocker.message.as_str()
                    }),
            )
            .with("blockers", &draft.plan.blockers)
        }
        PlanError::Internal(error) => ApiError::engine(error),
    }
}

// ---------------------------------------------------------------- a thread

/// What the plan has that only this layer can say: the names, and the page its result would have.
async fn complete(
    conn: &mut Conn,
    principal: &Principal,
    draft: &mut Draft,
    topic: Id,
) -> ApiResult<()> {
    blockers_view(conn, draft).await?;
    let profile = match draft.plan.access.profile {
        PageProfile::Managed => Profile::Managed,
        PageProfile::Connected => Profile::Connected,
    };
    draft.plan.access.summary = super::threads::binding::summary_of(
        conn,
        principal,
        draft.request.topic_id.unwrap_or(topic),
        ObjectKind::Thread,
        profile,
    )
    .await?;
    // What the rows that follow the defaults would give differently: in another topic, or on a person's machine
    // instead of a managed environment (or back), against what they give the source where it is now.
    let source_profile = match draft
        .plan
        .from
        .environment
        .as_ref()
        .map(|environment| environment.placement)
    {
        Some(orch_api_contract::environment::Placement::Connected) => Profile::Connected,
        _ => Profile::Managed,
    };
    let before = super::threads::binding::summary_of(
        conn,
        principal,
        topic,
        ObjectKind::Thread,
        source_profile,
    )
    .await?;
    let after = &draft.plan.access.summary;
    draft.plan.access.differs = [
        ("thread.view", &before.view, &after.view),
        ("thread.interact", &before.interact, &after.interact),
        ("thread.manage", &before.manage, &after.manage),
    ]
    .into_iter()
    .filter(|(_, before, after)| before != after)
    .map(
        |(permission, before, after)| orch_api_contract::forks::ForkPlanDifference {
            permission: permission.to_owned(),
            before: before.clone(),
            after: after.clone(),
        },
    )
    .collect();
    Ok(())
}

/// Plans a fork or a switch of a thread, and keeps the plan for the minutes it is good for.
async fn preflight(ctx: Ctx, (): (), request: ForkRequest) -> ApiResult<ForkPlan> {
    let thread = ctx.id("id")?;
    let caller = authorization::caller_of(ctx.user()?)?;
    let drafted = ctx.app.engine.draft_fork(&caller, thread, &request).await;
    let mut conn = ctx.app.db.acquire().await?;
    let mut draft = match drafted {
        Ok(draft) => draft,
        Err(error) => return Err(plan_failed(&mut conn, error).await),
    };
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let topic = thread_rows::thread(&mut conn, thread)
        .await?
        .map(|row| row.topic_id)
        .ok_or_else(ApiError::not_found)?;
    complete(&mut conn, &principal, &mut draft, topic).await?;
    drop(conn);
    ctx.app
        .engine
        .keep_fork_plan(&caller, draft)
        .await
        .map_err(ApiError::internal)
}

/// Carries out a plan: the job is made and, for a switch, the thread is held, in one transaction.
async fn create(ctx: Ctx, (): (), body: ForkCreateBody) -> ApiResult<Job> {
    let thread = ctx.id("id")?;
    // The plan is confirmed in the transaction of the action guard, which asked the caller again.
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let made = ctx
        .app
        .engine
        .confirm_fork_guarded(tx, guard.into_principal(), thread, &body)
        .await;
    let mut conn = ctx.app.db.acquire().await?;
    match made {
        Ok(row) => job_view::view(&mut conn, &row).await,
        Err(error) => Err(plan_failed(&mut conn, error).await),
    }
}

/// Forks a thread with nothing to choose, in one request: planned with the source's topic, environment,
/// provider, profile and model and all of its history (or the history up to a turn, from a message), and carried
/// out at once. A plan that something stops is refused as it would be on confirmation, and the person plans it
/// instead.
async fn now(ctx: Ctx, (): (), body: ForkNowBody) -> ApiResult<ForkNowReply> {
    let thread = ctx.id("id")?;
    let caller = authorization::caller_of(ctx.user()?)?;
    let request = ForkRequest {
        result: ForkResult::New,
        topic_id: None,
        name: body.name,
        access: None,
        history: body
            .to_turn
            .map(|to_turn| orch_api_contract::forks::ForkHistory {
                mode: orch_api_contract::forks::HistoryMode::Range,
                from_turn: None,
                to_turn: Some(to_turn),
            }),
        to: None,
        environment: None,
        worktree: None,
        files_at: None,
        memory: None,
        directory: None,
        cwd: None,
        options: None,
        check_fingerprints: None,
        fingerprint_request_id: None,
        mounts: None,
    };
    let drafted = ctx.app.engine.draft_fork(&caller, thread, &request).await;
    let mut conn = ctx.app.db.acquire().await?;
    let mut draft = match drafted {
        Ok(draft) => draft,
        Err(error) => return Err(plan_failed(&mut conn, error).await),
    };
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let topic = thread_rows::thread(&mut conn, thread)
        .await?
        .map(|row| row.topic_id)
        .ok_or_else(ApiError::not_found)?;
    complete(&mut conn, &principal, &mut draft, topic).await?;
    drop(conn);
    let plan = ctx
        .app
        .engine
        .keep_fork_plan(&caller, draft)
        .await
        .map_err(ApiError::internal)?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let made = ctx
        .app
        .engine
        .confirm_fork_guarded(
            tx,
            guard.into_principal(),
            thread,
            &ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            },
        )
        .await;
    let mut conn = ctx.app.db.acquire().await?;
    let row = match made {
        Ok(row) => row,
        Err(error) => return Err(plan_failed(&mut conn, error).await),
    };
    let thread_id = row
        .plan
        .get("new_thread")
        .and_then(|id| id.as_str())
        .and_then(|id| id.parse::<Id>().ok())
        .ok_or_else(|| ApiError::internal("a fork's job names no thread it makes"))?;
    Ok(ForkNowReply {
        job: job_view::view(&mut conn, &row).await?,
        thread_id,
    })
}

// ---------------------------------------------------------------- several

/// Plans a switch of several threads together, and keeps the plan.
async fn batch_preflight(
    ctx: Ctx,
    (): (),
    body: ForkBatchPreflightBody,
) -> ApiResult<ForkBatchPlan> {
    let caller = authorization::caller_of(ctx.user()?)?;
    let drafted = ctx.app.engine.draft_batch(&caller, &body).await;
    let mut conn = ctx.app.db.acquire().await?;
    let mut draft: BatchDraft = match drafted {
        Ok(draft) => draft,
        Err(error) => return Err(plan_failed(&mut conn, error).await),
    };
    let principal = authorization::principal(&ctx, &mut conn).await?;
    // The page of the result is told once for each place and profile.
    let mut summaries: Vec<((Id, PageProfile), orch_api_contract::access::AccessSummary)> =
        Vec::new();
    for (index, plan) in draft.plan.plans.iter_mut().enumerate() {
        for (blocker_index, missing) in draft.lacks.get(index).cloned().unwrap_or_default() {
            let named = refusals::missing_access(&mut conn, &missing).await?;
            if let Some(blocker) = plan.blockers.get_mut(blocker_index) {
                blocker.missing = Some(named);
            }
        }
        let thread = draft.members.get(index).map(|member| member.thread);
        let Some(thread) = thread else { continue };
        let topic = thread_rows::thread(&mut conn, thread)
            .await?
            .map(|row| row.topic_id)
            .ok_or_else(ApiError::not_found)?;
        let key = (topic, plan.access.profile);
        let summary = match summaries.iter().find(|(known, _)| *known == key) {
            Some((_, summary)) => summary.clone(),
            None => {
                let profile = match plan.access.profile {
                    PageProfile::Managed => Profile::Managed,
                    PageProfile::Connected => Profile::Connected,
                };
                let summary = super::threads::binding::summary_of(
                    &mut conn,
                    &principal,
                    topic,
                    ObjectKind::Thread,
                    profile,
                )
                .await?;
                summaries.push((key, summary.clone()));
                summary
            }
        };
        plan.access.summary = summary;
    }
    drop(conn);
    ctx.app
        .engine
        .keep_batch_plan(&caller, draft)
        .await
        .map_err(ApiError::internal)
}

/// The batch as the caller sees it: the jobs of the threads they may view, and a count of the others.
async fn batch_view(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[JobRow],
) -> ApiResult<ForkBatch> {
    let batch_id = rows
        .first()
        .and_then(|row| row.batch_id)
        .ok_or_else(ApiError::not_found)?;
    let mut jobs = Vec::new();
    let mut hidden = 0_i64;
    for row in rows {
        let Some(thread) = row.node_id else {
            continue;
        };
        let may_view = matches!(
            nodes::check(
                conn,
                principal,
                thread,
                Permission::ThreadView,
                Act::Use,
                Policy::default(),
            )
            .await,
            Ok(orch_access_contract::decisions::Decision::Allowed { .. })
                | Err(orch_authz::Error::Archived { .. })
        );
        if !may_view {
            hidden += 1;
            continue;
        }
        let named = node_view::refs(conn, principal, &[thread], Policy::default()).await?;
        let Some(thread) = named.get(&thread).cloned() else {
            hidden += 1;
            continue;
        };
        jobs.push(ForkBatchJob {
            thread,
            job: job_view::view(conn, row).await?,
        });
    }
    Ok(ForkBatch {
        batch_id,
        jobs,
        percent: percent_of_batch(rows),
        hidden,
        counts: counts_of(rows),
    })
}

/// Carries out the plan of a batch: one job for each plan without blockers, all locked at once.
async fn batch_create(ctx: Ctx, (): (), body: ForkCreateBody) -> ApiResult<ForkBatch> {
    let caller = authorization::caller_of(ctx.user()?)?;
    let made = ctx.app.engine.confirm_batch(&caller, &body).await;
    let mut conn = ctx.app.db.acquire().await?;
    let (batch, _) = match made {
        Ok(made) => made,
        Err(error) => return Err(plan_failed(&mut conn, error).await),
    };
    let rows = ctx
        .app
        .engine
        .batch_jobs(batch)
        .await
        .map_err(ApiError::internal)?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    batch_view(&mut conn, &principal, &rows).await
}

/// Whether the caller may read a batch: they made it, or manage a thread it lists.
async fn may_read_batch(conn: &mut Conn, principal: &Principal, rows: &[JobRow]) -> ApiResult<()> {
    if rows
        .first()
        .is_some_and(|row| row.created_by == Some(principal.user()))
    {
        return Ok(());
    }
    for row in rows {
        let Some(thread) = row.node_id else { continue };
        if matches!(
            nodes::check(
                conn,
                principal,
                thread,
                Permission::ThreadManage,
                Act::Use,
                Policy::default(),
            )
            .await,
            Ok(orch_access_contract::decisions::Decision::Allowed { .. })
        ) {
            return Ok(());
        }
    }
    Err(ApiError::not_found())
}

/// Reads where a batch stands.
async fn batch_get(ctx: Ctx, (): (), (): ()) -> ApiResult<ForkBatch> {
    let batch = ctx.id("batch_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    read_batch_for(&mut conn, &principal, batch).await
}

/// The batch's creator/manager admission and visible members, shared by HTTP and live delivery.
pub(crate) async fn read_batch_for(
    conn: &mut Conn,
    principal: &Principal,
    batch: Id,
) -> ApiResult<ForkBatch> {
    let rows = orch_db::queries::jobs::forks::of_batch(conn, batch).await?;
    if rows.is_empty() {
        return Err(ApiError::not_found());
    }
    may_read_batch(conn, principal, &rows).await?;
    batch_view(conn, principal, &rows).await
}

/// Cancels the jobs of a batch that have not passed their commit; its creator's alone.
async fn batch_cancel(ctx: Ctx, (): (), (): ()) -> ApiResult<ForkBatch> {
    let batch = ctx.id("batch_id")?;
    let rows = ctx
        .app
        .engine
        .batch_jobs(batch)
        .await
        .map_err(ApiError::internal)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    if !rows
        .first()
        .is_some_and(|row| row.created_by == Some(principal.user()))
    {
        return Err(ApiError::not_found());
    }
    let rows = ctx
        .app
        .engine
        .cancel_batch(principal.user(), batch)
        .await
        .map_err(ApiError::internal)?;
    batch_view(&mut conn, &principal, &rows).await
}

// ---------------------------------------------------------------- side threads

/// Why a side thread could not be started, as the problem a client acts on.
async fn side_failed(conn: &mut Conn, parent: Id, error: SideError) -> ApiError {
    match error {
        SideError::NotFound => ApiError::not_found(),
        SideError::Archived => ApiError::new(
            ErrorCode::NodeArchived,
            "the thread is archived, and so is read-only",
        ),
        SideError::Side => ApiError::new(
            ErrorCode::ThreadSide,
            "a side thread has no side threads of its own",
        ),
        SideError::Unbound => ApiError::new(
            ErrorCode::BindingIncompatible,
            "the thread is bound to nothing yet, so an aside has nothing to run on",
        ),
        SideError::Limit { open, max, allowed } => ApiError::new(
            ErrorCode::ThreadSideLimit,
            if allowed {
                "no further side thread can be opened for this thread"
            } else {
                "side threads are switched off here"
            },
        )
        .with("open", open)
        .with("max", max),
        SideError::Invalid(message) => ApiError::invalid("to_turn", &message),
        SideError::NoReading(why) => {
            ApiError::new(ErrorCode::CapabilityUnsupported, why).with("capability", "side_reading")
        }
        SideError::NoHistory => ApiError::new(
            ErrorCode::CapabilityUnsupported,
            "this agent can neither fork its conversation nor take in its history, so an aside would begin without it",
        )
        .with("capability", "side_history"),
        SideError::Denied(refusal) => match refusals::denied(conn, *refusal).await {
            Ok(error) | Err(error) => error,
        },
        SideError::Input(error) => super::threads::input_failed(conn, parent, error).await,
        SideError::Internal(error) => ApiError::engine(error),
    }
}

/// Starts a side thread of a thread with a message queued in it, in one transaction.
async fn side_create(
    ctx: Ctx,
    (): (),
    body: ThreadSideCreateBody,
) -> ApiResult<ThreadSideCreateReply> {
    let parent = ctx.id("id")?;
    let caller = authorization::caller_of(ctx.user()?)?;
    let started = ctx
        .app
        .engine
        .start_side_thread(&caller, parent, body.content, body.client_id, body.to_turn)
        .await;
    let mut conn = ctx.app.db.acquire().await?;
    let started = match started {
        Ok(started) => started,
        Err(error) => return Err(side_failed(&mut conn, parent, error).await),
    };
    let thread: Thread = one(&ctx, &mut conn, started.thread).await?;
    Ok(ThreadSideCreateReply {
        thread,
        input: started.submitted.input,
        seq: started.submitted.seq,
    })
}

/// Lists the side threads of a thread, oldest first.
async fn side_list(ctx: Ctx, query: ThreadSideListQuery, (): ()) -> ApiResult<Vec<Thread>> {
    let parent = ctx.id("id")?;
    let closed = matches!(query.state, Some(SideState::Closed));
    let ids = ctx
        .app
        .engine
        .side_threads(parent, closed)
        .await
        .map_err(ApiError::internal)?;
    // A closed side thread is a deleted node, which no view shows: it is shown as it was, in a
    // transaction that is never committed.
    let mut tx = ctx.app.db.begin().await?;
    if closed {
        thread_rows::forking::show_closed(&mut tx, &ids).await?;
    }
    let mut listed = Vec::new();
    for id in ids {
        match one(&ctx, &mut tx, id).await {
            Ok(thread) => listed.push(thread),
            // Not one the caller is to see.
            Err(error) if error.code() == ErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    tx.rollback().await.map_err(orch_db::DbError::from)?;
    Ok(listed)
}
