//! Native job identities replay through the one creation seam and current completed-read factory.
use super::*;
use crate::{
    key_operations::{
        native::{self, reply},
        publication, targets,
    },
    middleware::idempotency::Creation,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    environment_sources::job_creation::{self as sources, Source},
    forks::{ForkCreateBody, UpdateConfirmQuery},
    operation_sources::Declaration,
    workspace::{JobCreationReceipt, JobCreationResponse},
};
use orch_db::idempotency::creation::Created;
use std::marker::PhantomData;

pub(crate) trait Mode: Send + Sync + 'static {
    const KIND: &'static str;
    const OBJECT: ObjectKind;
    const ROW: &'static Declaration<Source>;
    fn governor(row: &JobRow) -> Option<Id>;
}
pub(crate) struct EnvironmentUpdate;
impl Mode for EnvironmentUpdate {
    const KIND: &'static str = "environment_update_job";
    const OBJECT: ObjectKind = ObjectKind::Environment;
    const ROW: &'static Declaration<Source> = &sources::ENVIRONMENT_UPDATE;
    fn governor(row: &JobRow) -> Option<Id> {
        row.environment_id
    }
}
pub(crate) struct ProviderUpdate;
impl Mode for ProviderUpdate {
    const KIND: &'static str = "provider_runtime_update_job";
    const OBJECT: ObjectKind = ObjectKind::Provider;
    const ROW: &'static Declaration<Source> = &sources::PROVIDER_UPDATE;
    fn governor(row: &JobRow) -> Option<Id> {
        row.provider_id
    }
}
pub(crate) fn binding<M: Mode>() -> Creation<UpdateConfirmQuery, ForkCreateBody, JobCreationResponse>
{
    Creation {
        kind: M::KIND,
        replay: replay::<M>,
        identity: |value| {
            let id = match value {
                JobCreationResponse::Full(j) => j.id,
                JobCreationResponse::Receipt(r) => r.job_id,
            };
            Ok(Created {
                kind: M::KIND.into(),
                id,
                data: serde_json::Value::Null,
            })
        },
    }
}
fn replay<M: Mode>(
    ctx: Ctx,
    _: UpdateConfirmQuery,
    _: ForkCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<JobCreationResponse>> {
    Box::pin(async move {
        if created.kind != M::KIND {
            return Err(ApiError::internal("the accepted native job kind changed"));
        }
        respond::<M>(&ctx, created.id, None).await
    })
}
struct Input {
    id: Id,
    first: Option<Job>,
}
struct Content<M>(PhantomData<M>);
impl<M: Mode> reply::Projection for Content<M> {
    type Source = Source;
    type Input = Input;
    type Subject = JobRow;
    type Value = ();
    type Reply = Job;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        M::ROW
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<(JobRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let account = ctx.user()?.id();
            let row = rows::job(conn, input.id)
                .await?
                .filter(|r| r.created_by == Some(account))
                .ok_or_else(ApiError::not_found)?;
            let governor = M::governor(&row)
                .filter(|id| ctx.id("id").ok() == Some(*id))
                .ok_or_else(ApiError::not_found)?;
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: M::OBJECT,
                    id: governor,
                },
            )
            .await?;
            Ok((
                row,
                vec![
                    native::Facts::one(
                        Source::Governor,
                        KeyTargetKind::Object(M::OBJECT),
                        target.clone(),
                    ),
                    native::Facts::one(Source::Content, KeyTargetKind::Object(M::OBJECT), target),
                ],
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        _: &'a Input,
        row: &'a JobRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let choice = subject::read(conn, p, row, None).await?;
            if choice.cut {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        _: &'a JobRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            match &input.first {
                Some(j) => Ok(j.clone()),
                None => super::view(conn, &prepared.subject).await,
            }
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.kind == fresh.subject.kind
                && old.subject.created_by == fresh.subject.created_by
                && M::governor(&old.subject) == M::governor(&fresh.subject))
        })
    }
}
pub(crate) async fn respond<M: Mode>(
    ctx: &Ctx,
    id: Id,
    first: Option<Job>,
) -> ApiResult<JobCreationResponse> {
    let receipt = JobCreationResponse::Receipt(JobCreationReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        job_id: id,
    });
    publication::receipt(ctx, &receipt)?;
    match reply::render_completed_replay::<Content<M>>(ctx, ctx.user()?.id(), Input { id, first })
        .await
    {
        Ok(Some(value)) => Ok(JobCreationResponse::full(value)),
        Ok(None) => Ok(receipt),
        Err(e) if matches!(e.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => Ok(receipt),
        Err(e) => Err(e),
    }
}

/// The real native queue transaction records the accepted identity before its commit.
pub(crate) async fn observed<M: Mode>(conn: &mut Conn, account: Id, id: Id) -> ApiResult<()> {
    orch_db::idempotency::creation::observe(conn, Some(account), M::KIND, id).await?;
    Ok(())
}
