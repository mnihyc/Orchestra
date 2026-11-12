//! Fork and switch plans with their options, and execution, single and batch; side threads started and listed.
//!
//! A fork or a switch is planned first and carried out by naming the plan: the engine works the plan out
//! ([`orch_engine::fork_plan`]), and what only this layer knows is added before it is kept: the names inside
//! what the caller lacks, and the page the result would have. Carrying a plan out makes a job and, for a
//! switch, holds the thread in the same transaction; a batch makes one job for each plan without blockers.
//! A side thread is the fork with nothing to choose and is made by one request; every request on it, by
//! its own identifier, is asked of its parent.

use super::threads::one;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::environments::jobs as job_view;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::Profile;

use orch_api_contract::access::PageProfile;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::forks::ForkBatch;

use orch_api_contract::forks::ForkBatchCancel;

use orch_api_contract::forks::ForkBatchCreate;

use orch_api_contract::forks::ForkBatchGet;

use orch_api_contract::forks::ForkBatchJob;

use orch_api_contract::forks::ForkBatchPlan;

use orch_api_contract::forks::ForkBatchPreflight;

use orch_api_contract::forks::ForkBatchPreflightBody;

use orch_api_contract::forks::ForkCreateBody;

use orch_api_contract::forks::ForkNowBody;

use orch_api_contract::forks::ForkNowReply;

use orch_api_contract::forks::ForkPlan;

use orch_api_contract::forks::ForkRequest;

use orch_api_contract::forks::ForkResult;

use orch_api_contract::forks::ThreadForkCreate;

use orch_api_contract::forks::ThreadForkNow;

use orch_api_contract::forks::ThreadForkPreflight;

use orch_api_contract::threads::SideState;

use orch_api_contract::threads::Thread;

use orch_api_contract::threads::ThreadSideCreate;

use orch_api_contract::threads::ThreadSideList;

use orch_api_contract::threads::ThreadSideListQuery;

use orch_api_contract::workspace::Job;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::nodes;

use orch_db::Conn;

use orch_db::queries::jobs::JobRow;

use orch_db::queries::threads as thread_rows;

use orch_engine::fork_plan::Draft;

use orch_engine::fork_plan::PlanError;

use orch_engine::jobs::batch::BatchDraft;

use orch_engine::jobs::batch::counts_of;

use orch_engine::jobs::batch::percent_of_batch;

use orch_engine::side_threads::SideError;

use orch_types::Id;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(
            orch_api_contract::duplicates::ThreadMoveOut,
            replay::moving(),
        )
        .on_with_initial_operation(
            orch_api_contract::duplicates::ThreadMoveOutPreview,
            crate::key_operations::native::effects::initial::<
                preparation::Preview<orch_engine::fork_plan::operation::MovePreview>,
            >,
            preparation::prepare_move,
            preparation::run_move,
        )
        .on_with_initial_operation(
            orch_api_contract::duplicates::ThreadMoveOut,
            crate::key_operations::native::effects::initial::<
                confirmation::Create<orch_engine::fork_plan::operation::MoveConfirm>,
            >,
            confirmation::prepare_move,
            confirmation::run_move,
        )
        .creation(ThreadForkCreate, replay::confirmation())
        .creation(ThreadForkNow, replay::immediate())
        .creation(ForkBatchCreate, replay::batch())
        .creation(ThreadSideCreate, side_operation::creation_binding())
        .on_with_initial_operation(
            ThreadForkPreflight,
            crate::key_operations::native::effects::initial::<preparation::Preview>,
            preparation::prepare,
            preparation::run,
        )
        .on_with_initial_operation(
            ThreadForkCreate,
            crate::key_operations::native::effects::initial::<confirmation::Create>,
            confirmation::prepare,
            confirmation::run,
        )
        .on_with_initial_operation(
            ThreadForkNow,
            crate::key_operations::native::effects::initial::<immediate::Now>,
            immediate::prepare,
            immediate::run,
        )
        .on_with_initial_operation(
            ForkBatchPreflight,
            crate::key_operations::native::effects::initial::<batch_preparation::Preview>,
            batch_preparation::prepare,
            batch_preparation::run,
        )
        .on_with_initial_operation(
            ForkBatchCreate,
            crate::key_operations::native::effects::initial::<batch_confirmation::Create>,
            batch_confirmation::prepare,
            batch_confirmation::run,
        )
        .on_snapshot_read_with_optional_operation(
            ForkBatchGet,
            crate::key_operations::native::selected::<batch_read::Batch>,
            crate::key_operations::native::selected_read::<batch_read::Batch>,
        )
        .on_with_optional_operation(
            ForkBatchCancel,
            crate::key_operations::native::stored_deferred::<batch_cancel::Cancel, (), ()>,
            batch_cancel::run,
        )
        .on_with_operation(
            ThreadSideCreate,
            crate::key_operations::native::effects::prepare::<side_operation::Create>,
            side_operation::run,
        )
        .on(ThreadSideList, side_list);
}
orch_api_base::__api_bind_e570164848!();

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
        PlanError::AgentAccess(error) => crate::routes::share::failed(conn, *error).await,
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
        .environment()
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
    if draft.plan.result == ForkResult::New {
        let destination = draft.request.topic_id.unwrap_or(topic);
        let copy_from = orch_engine::jobs::fork::fork_page_source_in(
            conn,
            draft.plan.source.id,
            draft.request.access.as_ref(),
            destination,
        )
        .await
        .map_err(ApiError::engine)?;
        if let Some(source) = copy_from.filter(|source| *source != draft.plan.source.id) {
            // The ordinary planner requires ThreadManage to carry another thread's page.
            // A blocked plan may still be shown, but cannot reveal bounds derived from
            // that unadmitted page through this optional metadata.
            match nodes::check(
                conn,
                principal,
                source,
                Permission::ThreadManage,
                Act::Use,
                Policy::default(),
            )
            .await
            {
                Ok(decision) if decision.is_allowed() => {}
                Ok(_) => return Ok(()),
                Err(error) => {
                    return Err(authorization::engine_problem(conn, principal, error).await);
                }
            }
        }
        let page = orch_engine::jobs::fork::fork_page(draft.request.access.as_ref());
        let environment = match &draft.plan.to.environment {
            orch_api_contract::forks::ForkPlanTarget::Existing(environment) => Some(
                orch_db::queries::environments::thread_engine::environment(conn, environment.id)
                    .await?
                    .ok_or_else(ApiError::not_found)?,
            ),
            // The normal fork plan's new hosted environment has the managed placement.
            orch_api_contract::forks::ForkPlanTarget::Made { .. } => None,
        };
        draft.plan.agent_access = Some(
            super::threads::binding::creation::existing_topic(
                conn,
                principal,
                destination,
                page.as_ref(),
                copy_from,
                environment.as_ref(),
            )
            .await?,
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- several

/// Plans a switch of several threads together, and keeps the plan.
async fn complete_batch(
    conn: &mut Conn,
    principal: &Principal,
    draft: &mut BatchDraft,
) -> ApiResult<()> {
    // The page of the result is told once for each place and profile.
    let mut summaries: Vec<((Id, PageProfile), orch_api_contract::access::AccessSummary)> =
        Vec::new();
    for (index, plan) in draft.plan.plans.iter_mut().enumerate() {
        for (blocker_index, missing) in draft.lacks.get(index).cloned().unwrap_or_default() {
            let named = refusals::missing_access(conn, &missing).await?;
            if let Some(blocker) = plan.blockers.get_mut(blocker_index) {
                blocker.missing = Some(named);
            }
        }
        let thread = draft.members.get(index).map(|member| member.thread);
        let Some(thread) = thread else { continue };
        let topic = thread_rows::thread(conn, thread)
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
                    conn,
                    principal,
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
    Ok(())
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
    batch_read::view(
        conn,
        principal,
        batch_id,
        rows,
        &crate::routes::access::page::NameBounds::default(),
        None,
    )
    .await
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
orch_api_base::__api_bind_020ffd9110!();

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

/// Lists the actual native side threads as the caller sees them.
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

#[path = "forks/batch_cancel.rs"]
pub mod batch_cancel;

#[path = "forks/batch_confirmation.rs"]
pub mod batch_confirmation;

#[path = "forks/batch_preparation.rs"]
pub mod batch_preparation;

#[path = "forks/batch_read.rs"]
pub mod batch_read;

#[path = "forks/batch_reply.rs"]
pub mod batch_reply;

#[path = "forks/confirmation.rs"]
pub mod confirmation;

#[path = "forks/immediate.rs"]
pub mod immediate;

#[path = "forks/job_reply.rs"]
pub mod job_reply;

#[path = "forks/preparation.rs"]
pub mod preparation;

#[path = "forks/replay.rs"]
pub mod replay;

#[path = "forks/side_operation.rs"]
pub mod side_operation;
