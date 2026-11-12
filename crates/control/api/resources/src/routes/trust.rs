//! A workspace's own harness configuration on its environment's page: what each harness found and the answer given
//! for it, in the environment's view, and the answer given or changed by whoever manages the environment.
//!
//! A person answers, never an agent; the sessions opened after an answer carry it, the threads that waited go on,
//! and the question is asked again when the files change.

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::trust::ConfigTrust;

use orch_api_contract::trust::ConfigTrustAnswer;

use orch_api_contract::trust::ConfigTrustBody;

use orch_api_contract::trust::ConfigTrustReceipt;

use orch_api_contract::trust::ConfigTrustResponse;

use orch_api_contract::trust::EnvironmentConfigTrustSet;

use orch_db::Conn;

use orch_db::queries::trust as rows;

use orch_db::queries::trust::answer;

use orch_engine::approvals::ApprovalError;

use orch_types::Id;

use orch_types::Time;

use std::collections::HashMap;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_with_optional_operation(
        EnvironmentConfigTrustSet,
        crate::key_operations::native::stored_deferred::<native::Trust, _, _>,
        set,
    );
}
orch_api_base::__api_bind_edc9d609ea!();

/// Each environment's harnesses that found a configuration of their own in its workspace, with the answer given.
pub(crate) async fn views_of(
    conn: &mut Conn,
    environments: &[Id],
) -> ApiResult<HashMap<Id, Vec<ConfigTrust>>> {
    let kept = rows::of_each(&mut *conn, environments).await?;
    let deciders: Vec<Id> = kept
        .iter()
        .flat_map(|(_, members)| members.iter().filter_map(|member| member.by))
        .collect();
    let people = refusals::user_refs(&mut *conn, &deciders).await?;
    Ok(kept
        .into_iter()
        .map(|(environment, members)| {
            let shown = members
                .into_iter()
                .map(|member| ConfigTrust {
                    path: member.path(),
                    answer: match member.answer.as_deref() {
                        Some(answer::TRUST) => Some(ConfigTrustAnswer::Trust),
                        Some(answer::KEEP_OUT) => Some(ConfigTrustAnswer::KeepOut),
                        _ => None,
                    },
                    by: member.by.and_then(|by| people.get(&by).cloned()),
                    at: member.at.map(Time::new),
                    program: crate::programs::shipped_name(&member.harness, None),
                    harness: member.harness,
                })
                .collect();
            (environment, shown)
        })
        .collect())
}
orch_api_base::__api_bind_ec0f96e79d!();

/// Answers, or changes the answer, for one harness: Manage on the environment, by a person.
async fn set(ctx: Ctx, (): (), body: ConfigTrustBody) -> ApiResult<ConfigTrustResponse> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let (ctx, guard, subject) = native::after_wait(&ctx, &mut tx, &initial).await?;
    let accepted = guard.principal().caller().clone();
    match ctx
        .app
        .engine
        .set_config_trust_fenced(
            tx,
            guard.principal().clone(),
            subject.environment,
            &subject.harness,
            body.answer,
            || guard.before_effect(&ctx),
        )
        .await
    {
        Ok(()) => {}
        Err(ApprovalError::NotFound | ApprovalError::AlreadyDecided) => {
            return Err(ApiError::not_found());
        }
        Err(ApprovalError::Denied(refusal)) => {
            let mut conn = ctx.app.db.acquire().await?;
            return Err(denied(&ctx, &mut conn, *refusal).await?);
        }
        Err(ApprovalError::Invalid(message)) => return Err(ApiError::invalid("answer", &message)),
        Err(ApprovalError::Internal(error)) => return Err(ApiError::engine(error)),
    }
    let receipt = ConfigTrustReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        environment_id: subject.environment,
        harness: subject.harness,
        answer: body.answer,
    };
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    match crate::key_operations::native::resource_reply::stored::<native::Trust>(&ctx, &accepted)
        .await?
    {
        Some(orch_api_contract::resources::Resource::Environment(environment)) => {
            Ok(ConfigTrustResponse::Full(Box::new(environment)))
        }
        Some(_) => Err(ApiError::internal(
            "the native trust response is not an Environment",
        )),
        None => Ok(ConfigTrustResponse::Receipt(receipt)),
    }
}

async fn denied(
    ctx: &Ctx,
    conn: &mut Conn,
    refusal: orch_access_contract::decisions::Refusal,
) -> ApiResult<ApiError> {
    if ctx.key_receipt_required() {
        Ok(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "a person must manage the environment to answer",
        )
        .with("required", "environment.manage"))
    } else {
        refusals::denied(conn, refusal).await
    }
}

#[path = "trust/native.rs"]
pub mod native;
