//! Actual native capability removal and its independent optional job acknowledgement.

use super::*;
use crate::key_operations::{
    native::{self, effects, reply},
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    environment_sources::{CAPABILITY_REVOKE, CapabilitySource as Source},
    operation_sources::Declaration,
};
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_engine::capabilities::PreparedCapabilityRevocation;

pub(super) struct Revocations;
pub(super) struct Revoked {
    pub environment: Id,
    pub key: String,
    pub job: Option<RecreateJob>,
}
async fn failed(ctx: &Ctx, conn: &mut Conn, error: RevokeError) -> ApiResult<ApiError> {
    Ok(match error {
        RevokeError::NotFound | RevokeError::NotGranted => ApiError::not_found(),
        RevokeError::Denied(refusal) => {
            let error = refusals::denied(conn, *refusal).await?;
            let caller = crate::middleware::authorization::caller_of(ctx.user()?)?;
            if native::modern(&caller) {
                crate::middleware::authorization::problem_without_content(error)
            } else {
                error
            }
        }
        RevokeError::Internal(error) => ApiError::engine(error),
    })
}
async fn facts(conn: &mut Conn, environment: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    orch_db::queries::environments::capabilities::environment_facts(conn, environment)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    Ok(vec![native::Facts::one(
        Source::Environment,
        KeyTargetKind::Object(ObjectKind::Environment),
        targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
        )
        .await?,
    )])
}
impl effects::Effect for Revocations {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = PreparedCapabilityRevocation;
    type Output = Revoked;
    fn declaration(_: &Self::Value) -> &'static Declaration<Source> {
        &CAPABILITY_REVOKE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            let known =
                native::bind_required(ctx, &CAPABILITY_REVOKE, &facts(conn, environment).await?)?;
            let principal = known.preparation_principal(ctx, conn, caller).await?;
            let value = ctx
                .app
                .engine
                .prepare_capability_revocation(conn, &principal, environment, ctx.param("key")?)
                .await;
            let value = match value {
                Ok(value) => value,
                Err(error) => return Err(failed(ctx, conn, error).await?),
            };
            let facts = facts(conn, value.environment()).await?;
            Ok((value, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .may_revoke_capability(conn, principal, value.environment(), value.key())
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(failed(ctx, conn, error).await?),
            }
        })
    }
}
impl effects::ApplyOwned for Revocations {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Revoked>>> {
        Box::pin(async move {
            let job = match ctx
                .app
                .engine
                .apply_capability_revocation(tx, principal, value, move || {
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                })
                .await
            {
                Ok(job) => job,
                Err(error) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    return Err(failed(ctx, &mut conn, error).await?);
                }
            };
            Ok(effects::Outcome::Applied(Revoked {
                environment: value.environment(),
                key: value.key().to_owned(),
                job,
            }))
        })
    }
}
pub(super) struct Content;
impl reply::Projection for Content {
    type Source = Source;
    type Input = Revoked;
    type Subject = ();
    type Value = Option<orch_api_contract::users::UserRef>;
    type Reply = Option<Job>;
    const READ_SOURCE: Source = Source::Environment;
    fn declaration(_: &Revoked) -> &'static Declaration<Source> {
        &CAPABILITY_REVOKE
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Revoked,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async move { Ok(((), facts(conn, input.environment).await?)) })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Revoked,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if let Err(error) = ctx
                .app
                .engine
                .may_revoke_capability(conn, principal, input.environment, &input.key)
                .await
            {
                return Err(failed(ctx, conn, error).await?);
            }
            if let Some(job) = &input.job {
                let row = orch_db::queries::jobs::job(conn, job.id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if row.environment_id != Some(input.environment)
                    || row.created_by != job.created_by
                    || row.created_at != job.created_at
                    || !matches!(
                        row.kind.as_str(),
                        "environment_recreate" | "capability_change"
                    )
                {
                    return Err(ApiError::not_found());
                }
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Revoked,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let by = match input.job.as_ref().and_then(|job| job.created_by) {
                Some(by) => refusals::user_refs(conn, &[by]).await?.remove(&by),
                None => None,
            };
            Ok((by, Vec::new()))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Revoked,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Option<Job>>> {
        Box::pin(async move {
            Ok(input
                .job
                .as_ref()
                .map(|job| queued(job, prepared.value.clone())))
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Revoked,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.value == fresh.value) })
    }
}
