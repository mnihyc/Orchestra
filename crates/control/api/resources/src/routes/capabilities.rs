//! Taking a capability back from an environment: the key removed, the removal written down, and the unit of the environment asked to be made anew without it.
//!
//! Whoever manages an environment, or the host it runs on, can give up a capability it was granted. The request
//! answers at once with the job that makes the unit anew; what that job does is the environments' part, and until
//! it runs the job waits queued. A capability is granted by a request of an agent that its policy allows or a
//! person of its scope approves (see the engine's `capabilities`), and decided through the approvals.

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::environment::CapabilityRevokeReceipt;

use orch_api_contract::environment::CapabilityRevokeResponse;

use orch_api_contract::environment::EnvironmentCapabilityRevoke;

use orch_api_contract::nodes::JobState;

use orch_api_contract::workspace::Job;

use orch_api_contract::workspace::JobProgress;

use orch_db::queries::environments::capabilities::RecreateJob;

use orch_engine::capabilities::RevokeError;

use orch_types::Id;

use orch_types::Time;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_with_operation(
        EnvironmentCapabilityRevoke,
        crate::key_operations::native::effects::prepare::<sources::Revocations>,
        revoke,
    );
}
orch_api_base::__api_bind_51c3f443d6!();

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
async fn revoke(ctx: Ctx, (): (), (): ()) -> ApiResult<CapabilityRevokeResponse> {
    let (tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done =
        crate::key_operations::native::effects::apply_owned::<sources::Revocations>(&ctx, tx)
            .await?;
    let receipt = CapabilityRevokeReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        environment_id: done.output.environment,
        key: done.output.key.clone(),
        job_id: done.output.job.as_ref().map(|job| job.id),
    };
    crate::key_operations::publication::receipt(&done.ctx, &receipt)?;
    let full = match crate::key_operations::native::reply::render::<sources::Content>(
        &done.ctx,
        &done.accepted,
        done.output,
    )
    .await
    {
        Ok(full) => full,
        Err(error)
            if matches!(
                error.code(),
                orch_api_contract::common::ErrorCode::AuthzDenied
                    | orch_api_contract::common::ErrorCode::NotFound
            ) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    Ok(match full {
        Some(job) => CapabilityRevokeResponse::Full(job.map(Box::new)),
        None => CapabilityRevokeResponse::Receipt(receipt),
    })
}

#[path = "capabilities/sources.rs"]
pub mod sources;
