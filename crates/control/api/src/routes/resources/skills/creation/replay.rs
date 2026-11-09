//! The accepted Skill identity reuses its native singleton without any original source supplier.
use super::*;
use crate::middleware::idempotency::Creation as HttpCreation;
use orch_db::idempotency::creation::Created as Recorded;

pub(in crate::routes::resources::skills) fn binding()
-> HttpCreation<(), api::SkillCreateBody, api::SkillCreateResponse> {
    HttpCreation {
        kind: "skill",
        replay: |ctx, (), body, created| Box::pin(replay(ctx, body, created)),
        identity: |value| {
            Ok(Recorded {
                kind: "skill".into(),
                id: match value {
                    api::SkillCreateResponse::Full(value) => value.id,
                    api::SkillCreateResponse::Receipt(value) => value.skill_id,
                },
                data: serde_json::Value::Null,
            })
        },
    }
}
async fn replay(
    ctx: Ctx,
    body: api::SkillCreateBody,
    recorded: Recorded,
) -> ApiResult<api::SkillCreateResponse> {
    let version =
        serde_json::from_value(recorded.data.get("version").cloned().ok_or_else(|| {
            ApiError::internal("the Skill creation has no accepted version fact")
        })?)
        .map_err(ApiError::internal)?;
    // The canonical body selects the same declared variant; it supplies no target or writer proof.
    let completed = Completed {
        id: recorded.id,
        version,
        declaration: creation_sources::declaration(&body.from),
        required: Vec::new(),
    };
    let sparse = receipt(&completed);
    crate::key_operations::publication::receipt(&ctx, &sparse)?;
    match native::resource_reply::with_input_completed_replay::<Projection>(
        &ctx,
        ctx.user()?.id(),
        completed,
    )
    .await
    {
        Ok(Some(Resource::Skill(value))) => Ok(api::SkillCreateResponse::Full(Box::new(value))),
        Ok(None) => Ok(sparse),
        Ok(Some(_)) => Err(ApiError::internal(
            "a Skill creation replay returned another resource",
        )),
        Err(error)
            if matches!(
                error.code(),
                orch_api_contract::common::ErrorCode::NotFound
                    | orch_api_contract::common::ErrorCode::AuthzDenied
            ) =>
        {
            Ok(sparse)
        }
        Err(error) => Err(error),
    }
}
