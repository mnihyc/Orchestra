//! A workspace's own harness configuration on its environment's page: what each harness found and the answer given
//! for it, in the environment's view, and the answer given or changed by whoever manages the environment.
//!
//! A person answers, never an agent; the sessions opened after an answer carry it, the threads that waited go on,
//! and the question is asked again when the files change.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use orch_api_contract::environment::Environment;
use orch_api_contract::trust::{
    ConfigTrust, ConfigTrustAnswer, ConfigTrustBody, EnvironmentConfigTrustSet,
};
use orch_db::Conn;
use orch_db::queries::trust::{self as rows, answer};
use orch_engine::approvals::ApprovalError;
use orch_types::{Id, Time};
use std::collections::HashMap;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(EnvironmentConfigTrustSet, set);
}

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

/// Answers, or changes the answer, for one harness: Manage on the environment, by a person.
async fn set(ctx: Ctx, (): (), body: ConfigTrustBody) -> ApiResult<Environment> {
    let environment = ctx.id("id")?;
    let harness = ctx.param("harness")?.to_owned();
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.into_principal();
    match ctx
        .app
        .engine
        .set_config_trust_guarded(tx, principal.clone(), environment, &harness, body.answer)
        .await
    {
        Ok(()) => {}
        Err(ApprovalError::NotFound | ApprovalError::AlreadyDecided) => {
            return Err(ApiError::not_found());
        }
        Err(ApprovalError::Denied(refusal)) => {
            let mut conn = ctx.app.db.acquire().await?;
            return Err(refusals::denied(&mut conn, *refusal).await?);
        }
        Err(ApprovalError::Invalid(message)) => return Err(ApiError::invalid("answer", &message)),
        Err(ApprovalError::Internal(error)) => return Err(ApiError::internal(error)),
    }
    let mut conn = ctx.app.db.acquire().await?;
    crate::routes::environments::show(&ctx, &mut conn, &principal, environment).await
}
