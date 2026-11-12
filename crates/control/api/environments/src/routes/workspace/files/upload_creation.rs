//! One native upload admission, with its current native job and machine-name cuts on replay.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::idempotency::ObservedCreation;

use crate::registry::OperationFuture;

use crate::routes::environments::jobs;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::environment::ThroughThreadQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::workspace::Job;

use orch_api_contract::workspace::JobCreationReceipt;

use orch_api_contract::workspace::JobCreationResponse;

use orch_api_contract::workspace_sources as sources;

use orch_api_contract::workspace_sources::Source;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::idempotency::creation::Created;

use orch_db::queries::jobs::JobRow;

use std::collections::HashMap;
const KIND: &str = "workspace_upload_job";

const ROW: &Declaration<Source> = &sources::ENVIRONMENT_UPLOAD_WRITE;

pub(super) fn binding()
-> ObservedCreation<ThroughThreadQuery, EnvironmentUploadWriteBody, JobCreationResponse> {
    ObservedCreation { kind: KIND, replay }
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _: &'a Caller,
    _: ThroughThreadQuery,
    _: EnvironmentUploadWriteBody,
) -> OperationFuture<'a> {
    Box::pin(async move { native::bind_required(ctx, ROW, &facts(conn, ctx.id("id")?).await?) })
}

async fn facts(conn: &mut Conn, environment: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await?;
    let kind = KeyTargetKind::Object(ObjectKind::Environment);
    Ok(vec![
        native::Facts::one(Source::UploadEnvironment, kind, target.clone()),
        native::Facts::one(Source::UploadContent, kind, target),
    ])
}

pub(super) async fn observed(conn: &mut Conn, account: Id, id: Id) -> ApiResult<()> {
    orch_db::idempotency::creation::observe(conn, Some(account), KIND, id).await?;
    Ok(())
}

fn replay(
    ctx: Ctx,
    _: ThroughThreadQuery,
    _: EnvironmentUploadWriteBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<JobCreationResponse>> {
    Box::pin(async move {
        if created.kind != KIND || created.id != ctx.id("upload_id")? {
            return Err(ApiError::internal(
                "the accepted upload job changed its identity",
            ));
        }
        respond(&ctx, created.id, None).await
    })
}

struct Input {
    id: Id,
    first: Option<Job>,
}

struct Content;

struct Value {
    native: jobs::PreparedRead,
    names: Option<HashMap<Id, String>>,
}

impl reply::Projection for Content {
    type Source = Source;
    type Input = Input;
    type Subject = JobRow;
    type Value = Value;
    type Reply = Job;
    const READ_SOURCE: Source = Source::UploadContent;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        ROW
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<(JobRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            let account = ctx.user()?.id();
            let row = orch_db::queries::jobs::job(conn, input.id)
                .await?
                .filter(|row| {
                    row.kind == orch_db::queries::jobs::kind::DELIVERY
                        && row.plan.get("workspace_upload").is_some()
                        && row.environment_id == Some(environment)
                        && row.created_by == Some(account)
                })
                .ok_or_else(ApiError::not_found)?;
            Ok((row, facts(conn, environment).await?))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Input,
        row: &'a JobRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            jobs::prepare_read_for(conn, principal, row.id).await?;
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Input,
        row: &'a JobRow,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let native = jobs::prepare_read_for(conn, principal, row.id).await?;
            let (names, facts) = jobs::read::named_machines(
                ctx,
                conn,
                principal.caller(),
                row,
                ROW,
                native.progress_only(),
            )
            .await?;
            Ok((Value { native, names }, vec![facts]))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            prepared
                .value
                .native
                .require_current(conn, principal)
                .await?;
            if !native::modern(principal.caller())
                && let Some(first) = &input.first
            {
                return Ok(first.clone());
            }
            prepared
                .value
                .native
                .render(conn, prepared.value.names.as_ref())
                .await
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let names = match (&opened.value.names, &fresh.value.names) {
                (Some(old), Some(current)) => {
                    old.keys().all(|id| current.contains_key(id))
                        && (old.is_empty()
                            || opened.value.native.machine_source()
                                == fresh.value.native.machine_source())
                }
                (None, _) => true,
                _ => false,
            };
            Ok(opened.value.native.preserves(&fresh.value.native) && names)
        })
    }
}

pub(super) async fn respond(
    ctx: &Ctx,
    id: Id,
    first: Option<Job>,
) -> ApiResult<JobCreationResponse> {
    let receipt = JobCreationResponse::Receipt(JobCreationReceipt {
        projection: ReceiptProjection::Receipt,
        job_id: id,
    });
    publication::receipt(ctx, &receipt)?;
    match reply::render_completed_replay::<Content>(ctx, ctx.user()?.id(), Input { id, first })
        .await
    {
        Ok(Some(job)) => Ok(JobCreationResponse::full(job)),
        Ok(None) => Ok(receipt),
        Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
            Ok(receipt)
        }
        Err(error) => Err(error),
    }
}
