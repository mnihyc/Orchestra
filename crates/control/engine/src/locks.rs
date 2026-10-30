//! The lock a switch, move, recreate or update holds on a thread, an environment or a provider's threads: what it waits for, queues, cancels and refuses.
//!
//! **A lock is a job.** The thread a switch changes is held from the transaction that makes the job to the
//! moment the job ends, and the thread a fork is making is held the same way: whatever open job of the kind
//! `thread_fork` names a thread as the one it switches, or as the one it is making, holds it. Nothing is
//! written on the thread for that, so there is no lock to forget: a job that ends, in any way, lets go.
//!
//! What a held thread does:
//!
//! - **A turn that is running is waited for** up to the minutes the options give, or interrupted at once.
//!   Nothing starts a new one: [`Engine::preflight`] finds the lock and the thread is `blocked(switching)`
//!   as soon as it has no turn running ([`blocked_while_held`]).
//! - **Inputs are accepted and queued**; they run when the lock is released ([`Engine::release_lock`]).
//! - **Pending approvals are cancelled** when the lock is taken (`Engine::cancel_pending_approvals`),
//!   since nobody is to be asked about work that is about to be put somewhere else.
//! - **Changes to what it runs on are refused** with the job named ([`held_by`]), and so is another fork
//!   or switch of the thread or of an environment it touches.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::jobs::runner::read_progress;
use crate::preflight::Blocked;
use crate::unit::Unit;
use orch_api_contract::nodes::{JobRef, JobState};
use orch_db::Conn;
use orch_db::queries::approvals as approval_queries;
use orch_db::queries::jobs::{JobRow, forks};
use orch_db::queries::threads::{self, StateChange, thread_state};
use orch_types::Id;

/// The `blocked_reason` of a held thread.
pub const SWITCHING: &str = "switching";

/// Why a thread is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holding {
    /// A switch is changing what it runs on.
    Switched,
    /// A fork is making it, and it cannot be used before the job is done.
    Made,
    /// A runtime change holds every thread bound to its environment or provider.
    Runtime,
    /// The same thread is preparing an exact retained prompt continuation.
    Continuation,
}

/// A hold on a thread: the job, and why it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Lock {
    /// The open job that holds the thread.
    pub job: JobRow,
    /// Why.
    pub holding: Holding,
}

impl Lock {
    /// The sentence that says why the thread cannot go on, for its `blocked_detail`.
    #[must_use]
    pub fn sentence(&self) -> String {
        match self.holding {
            Holding::Switched => {
                "It is being switched to another provider, profile or environment; what is sent to it waits."
            }
            Holding::Made if !self.job.is_open()=>"Its confirmed history preparation did not finish. Authored queued words are retained; another native history is not substituted.",
            Holding::Made => "It is being made as a fork; it can be used when that is done.",
            Holding::Runtime => "Its environment or provider is being updated, recreated or moved; what is sent to it waits.",
            Holding::Continuation=>"Its native conversation is being prepared for a prompt version; new inputs keep their original queue until selection finishes.",
        }
        .to_owned()
    }
}

/// The hold on a thread, if an open job has one.
pub async fn lock_of(conn: &mut Conn, thread: Id) -> Result<Option<Lock>, EngineError> {
    let job = match forks::lock_of(conn, thread).await? {
        Some(job) => Some(job),
        None => retained_preparation(conn, thread).await?,
    };
    let Some(job) = job else {
        return Ok(None);
    };
    let holding = if job.kind == crate::jobs::continuation::KIND {
        Holding::Continuation
    } else if job.kind != forks::THREAD_FORK {
        Holding::Runtime
    } else if job.plan.get("new_thread").and_then(|id| id.as_str())
        == Some(thread.to_string().as_str())
    {
        Holding::Made
    } else {
        Holding::Switched
    };
    Ok(Some(Lock { job, holding }))
}

/// The open job that holds the thread, if one does: what a request to change what the thread runs on
/// names when it is refused.
pub async fn held_by(conn: &mut Conn, thread: Id) -> Result<Option<Id>, EngineError> {
    Ok(lock_of(conn, thread).await?.map(|lock| lock.job.id))
}

/// One indexed guard/job lookup, only for a born target whose preparation never
/// committed. A failed job is meaningful status; browsing cannot make it ready.
async fn retained_preparation(conn: &mut Conn, thread: Id) -> Result<Option<JobRow>, EngineError> {
    let job: Option<Id> = sqlx::query_scalar(
        "SELECT preparation_job FROM thread_native_guards WHERE thread_id=$1 AND NOT ready",
    )
    .bind(thread)
    .fetch_optional(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?
    .flatten();
    match job {
        Some(job) => Ok(orch_db::queries::jobs::job(conn, job).await?),
        None => Ok(None),
    }
}

/// A job in the few words a locked thing carries.
#[must_use]
pub fn job_ref(row: &JobRow) -> JobRef {
    let (progress, _) = read_progress(row);
    JobRef {
        id: row.id,
        kind: row.kind.clone(),
        state: match row.state.as_str() {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "succeeded" => JobState::Succeeded,
            "cancelled" => JobState::Cancelled,
            "lost" => JobState::Lost,
            _ => JobState::Failed,
        },
        phase: progress.phase,
        percent: progress.percent,
    }
}

/// What a thread shows as its lock: the job that holds it, if one does.
pub async fn lock_ref_of(conn: &mut Conn, thread: Id) -> Result<Option<JobRef>, EngineError> {
    Ok(lock_of(conn, thread).await?.map(|lock| job_ref(&lock.job)))
}

/// What pre-flight finds in the way of a turn when the thread is held.
pub async fn blocked_while_held(
    conn: &mut Conn,
    thread: Id,
) -> Result<Option<Blocked>, EngineError> {
    Ok(lock_of(conn, thread).await?.map(|lock| Blocked {
        reason: SWITCHING,
        detail: Some(lock.sentence()),
    }))
}

impl Engine {
    /// Cancels the approvals that wait in a thread: nobody is to be asked about work that is about to
    /// be put somewhere else. The harness is told they were refused, so that a turn that waited for one
    /// goes on without it.
    pub(crate) async fn cancel_pending_approvals(&self, thread: Id) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(_row) = threads::lock(&mut *unit.conn, thread).await? else {
                return Ok(());
            };
            let now = unit.now();
            for approval in approval_queries::pending_of_thread(&mut *unit.conn, thread).await? {
                if approval.kind != "harness" {
                    continue;
                }
                let Some(refusal) = refusal_for(&approval) else {
                    // Nothing in it refuses: it is cancelled with the turn when that ends.
                    continue;
                };
                if approval_queries::decide(
                    &mut *unit.conn,
                    approval.id,
                    "denied",
                    None,
                    &refusal,
                    now,
                )
                .await?
                {
                    crate::notifications::approval_settled(&mut unit, approval.id).await?;
                    let seq = unit.stamp(thread).await?;
                    if let Some(item) = approval.item_id {
                        threads::set_item_status(
                            &mut *unit.conn,
                            item,
                            &["pending"],
                            "cancelled",
                            now,
                            seq,
                        )
                        .await?;
                    }
                }
            }
            unit.wake(thread);
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Lets go of a thread whose lock has ended: a thread that stands `blocked(switching)` is looked at
    /// again, and what was queued while it was held runs.
    pub async fn release_lock(&self, thread: Id) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(row) = threads::lock(&mut *unit.conn, thread).await? else {
                return Ok(());
            };
            if lock_of(&mut *unit.conn, thread).await?.is_some() {
                return Ok(());
            }
            if row.state == thread_state::BLOCKED
                && row.blocked_reason.as_deref() == Some(SWITCHING)
            {
                let seq = unit.stamp(thread).await?;
                crate::notifications::set_state(
                    &mut unit,
                    thread,
                    &[thread_state::BLOCKED],
                    &StateChange::plain(thread_state::IDLE),
                    seq,
                )
                .await?;
            }
            if let Err(error) = self.preflight_in(&mut unit, thread).await {
                if matches!(error, EngineError::Db(_)) {
                    return Err(error);
                }
                tracing::warn!(%error, %thread, "pre-flight could not decide after a lock was released; it is looked at again");
            }
            unit.wake(thread);
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }
}

/// The refusal the harness is given for a request nobody will decide: the option that refuses, an
/// answer that says nothing to a question, a question from a tool server dropped. A request with no way
/// to refuse is left for the end of the turn to cancel.
fn refusal_for(approval: &orch_db::queries::approvals::ApprovalRow) -> Option<orch_types::Json> {
    refusal_with_note(approval, "The thread is being switched.")
}

pub(crate) fn refusal_with_note(
    approval: &orch_db::queries::approvals::ApprovalRow,
    note: &str,
) -> Option<orch_types::Json> {
    use orch_api_contract::approvals::{
        ApprovalDecision, ElicitationAction, HarnessAnswersDecision, HarnessElicitationDecision,
        HarnessOptionDecision, HarnessRequest, HarnessRequestDetail, OptionStyle,
    };
    let request: HarnessRequest = serde_json::from_value(approval.request.clone()).ok()?;
    let decision = match &request.detail {
        HarnessRequestDetail::UserInput(_) => ApprovalDecision::Answers(HarnessAnswersDecision {
            answers: std::collections::BTreeMap::new(),
            note: Some(note.to_owned()),
        }),
        HarnessRequestDetail::McpElicitation(_) => {
            ApprovalDecision::Elicitation(HarnessElicitationDecision {
                action: ElicitationAction::Cancel,
                content: None,
            })
        }
        _ => {
            let refusing = request
                .options
                .iter()
                .find(|option| option.style == OptionStyle::Deny)?;
            ApprovalDecision::Option(HarnessOptionDecision {
                option_id: refusing.id.clone(),
                note: Some(note.to_owned()),
            })
        }
    };
    serde_json::to_value(decision).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_that_is_switched_and_one_that_is_being_made_say_why_they_are_held() {
        let row = |plan: serde_json::Value| JobRow {
            id: orch_types::new_id(),
            kind: "thread_fork".to_owned(),
            node_id: None,
            environment_id: None,
            provider_id: None,
            batch_id: None,
            state: "queued".to_owned(),
            plan,
            progress: serde_json::json!({}),
            result: None,
            error: None,
            created_by: None,
            topic_seq: None,
            env_seq: None,
            created_at: chrono::Utc::now(),
            started_at: None,
            finished_at: None,
            updated_at: chrono::Utc::now(),
        };
        let switched = Lock {
            job: row(serde_json::json!({})),
            holding: Holding::Switched,
        };
        let made = Lock {
            job: row(serde_json::json!({})),
            holding: Holding::Made,
        };
        assert!(switched.sentence().contains("switched"));
        assert!(made.sentence().contains("fork"));
        assert_eq!(job_ref(&switched.job).state, JobState::Queued);
    }
}
