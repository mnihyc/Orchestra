//! Taking a capability back from an environment: the key removed, the removal written down, and the unit of the environment asked to be made anew without it.
//!
//! Whoever manages an environment, or the host it runs on, can give up a capability it was granted. The request
//! answers at once with the job that makes the unit anew; what that job does is the environments' part, and until
//! it runs the job waits queued. A capability is granted by a request of an agent that its policy allows or a
//! person of its scope approves (see the engine's `capabilities`), and decided through the approvals.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_api_contract::environment::EnvironmentCapabilityRevoke;
use orch_api_contract::nodes::JobState;
use orch_api_contract::workspace::{Job, JobProgress};
use orch_db::queries::environments::capabilities::RecreateJob;
use orch_engine::capabilities::RevokeError;
use orch_types::Time;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(EnvironmentCapabilityRevoke, revoke);
}

/// The job as it waits: queued, with nothing done.
fn queued(job: &RecreateJob, by: Option<orch_api_contract::users::UserRef>) -> Job {
    Job {
        call_recovery: None,
        id: job.id,
        kind: "environment_recreate".to_owned(),
        node_id: None,
        environment_id: Some(job.environment_id),
        provider_id: None,
        batch_id: None,
        state: JobState::Queued,
        progress: JobProgress {
            activity: None,
            phase: "queued".to_owned(),
            phases: Vec::new(),
            percent: 0,
            files_done: 0,
            files_total: None,
            bytes_done: 0,
            bytes_total: None,
            rate_bps: None,
            eta_seconds: None,
            message: Some("waiting for the environment to be made anew".to_owned()),
        },
        result: None,
        error: None,
        log: None,
        created_by: by,
        created_at: Time::new(job.created_at),
        started_at: None,
        finished_at: None,
    }
}

/// Takes a capability back from an environment.
async fn revoke(ctx: Ctx, (): (), (): ()) -> ApiResult<Option<Job>> {
    let environment = ctx.id("id")?;
    let key = ctx.param("key")?.to_owned();
    let caller = authorization::caller_of(ctx.user()?)?;
    let made = ctx
        .app
        .engine
        .revoke_capability(&caller, environment, &key)
        .await;
    let job = match made {
        Ok(Some(job)) => job,
        Ok(None) => return Ok(None),
        Err(RevokeError::NotFound | RevokeError::NotGranted) => return Err(ApiError::not_found()),
        Err(RevokeError::Denied(refusal)) => {
            let mut conn = ctx.app.db.acquire().await?;
            return Err(refusals::denied(&mut conn, *refusal).await?);
        }
        Err(RevokeError::Internal(error)) => return Err(ApiError::internal(error)),
    };
    let mut conn = ctx.app.db.acquire().await?;
    let by = match job.created_by {
        Some(person) => refusals::user_refs(&mut conn, &[person])
            .await?
            .remove(&person),
        None => None,
    };
    Ok(Some(queued(&job, by)))
}
