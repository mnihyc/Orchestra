//! Native consequence acknowledgement shared by atomic moving families; it grants no authority.
use super::*;
use orch_api_contract::duplicates::{
    CopySubject, MoveOutConfirmation, MoveOutDeleteEffect, MoveOutDeletePlan,
};

pub(crate) fn plan(
    subject: CopySubject,
    destination: DuplicateDestination,
    revision: i64,
    mover: Id,
    copy: &DuplicateBody,
    footprint: serde_json::Value,
    effects: Vec<MoveOutDeleteEffect>,
) -> ApiResult<MoveOutDeletePlan> {
    let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"subject":subject,"destination":destination,
        "revision":revision,"mover":mover,"name":copy.name,"footprint":footprint,"effects":effects}))
        .map_err(ApiError::internal)?;
    Ok(MoveOutDeletePlan {
        subject,
        destination,
        revision,
        stamp: blake3::hash(&bytes).to_hex().to_string(),
        effects,
    })
}
pub(crate) fn confirm(
    plan: &MoveOutDeletePlan,
    acknowledgement: &MoveOutConfirmation,
) -> ApiResult<()> {
    if acknowledgement.revision != plan.revision || acknowledgement.stamp != plan.stamp {
        return Err(stale());
    }
    Ok(())
}
pub(crate) fn stale() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::ConflictStaleRevision,
        "the native delete plan changed; review it again before moving",
    )
}
