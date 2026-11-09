//! Queue the native selected archive on its guarded connection and retain the real request.
use super::*;
use crate::key_operations::{
    native::{self, effects, reply},
    publication, targets,
};
use crate::middleware::transaction_guard;
use futures::future::BoxFuture;
use orch_api_contract::exports::{ExportCreationReceipt, ExportCreationResponse};
use orch_api_contract::{
    export_sources::{self as sources, Source},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_db::{
    DbError,
    queries::{exports as rows, identity, jobs},
};
use orch_engine::exports::{Made, PreparedExportCreation, operation};

pub(super) struct Create;
pub(super) struct Ready {
    prepared: PreparedExportCreation,
    required: Vec<native::Facts<Source>>,
}
pub(super) struct Queued {
    made: Made,
    required: Vec<native::Facts<Source>>,
}
struct Accepted {
    id: Id,
    export: Id,
    node: Option<Id>,
    user: Option<Id>,
    required: Vec<native::Facts<Source>>,
    initial: Option<jobs::JobRow>,
}
impl From<Queued> for Accepted {
    fn from(value: Queued) -> Self {
        Self {
            id: value.made.job.id,
            export: value.made.export.id,
            node: value.made.export.node_id,
            user: value.made.export.user_id,
            required: value.required,
            initial: Some(value.made.job),
        }
    }
}
pub(super) fn replay_binding()
-> crate::middleware::idempotency::Creation<(), ExportRequest, ExportCreationResponse> {
    crate::middleware::idempotency::Creation {
        kind: "export_job",
        replay,
        identity: |value| {
            let id = match value {
                ExportCreationResponse::Full(value) => value.id,
                ExportCreationResponse::Receipt(value) => value.id,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "export_job".into(),
                id,
                data: serde_json::Value::Null,
            })
        },
    }
}
fn replay(
    ctx: Ctx,
    (): (),
    _: ExportRequest,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<ExportCreationResponse>> {
    Box::pin(async move {
        let parse =
            |name| {
                serde_json::from_value(created.data.get(name).cloned().ok_or_else(|| {
                    ApiError::internal("the export creation identity is incomplete")
                })?)
                .map_err(ApiError::internal)
            };
        let export: Id = parse("export")?;
        let node: Option<Id> =
            serde_json::from_value(created.data["node"].clone()).map_err(ApiError::internal)?;
        let receipt = ExportCreationResponse::Receipt(ExportCreationReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            id: created.id,
            export_id: export,
        });
        publication::receipt(&ctx, &receipt)?;
        match reply::render_completed_replay::<Job>(
            &ctx,
            ctx.user()?.id(),
            Accepted {
                id: created.id,
                export,
                node,
                user: Some(ctx.user()?.id()),
                required: Vec::new(),
                initial: None,
            },
        )
        .await
        {
            Ok(Some(value)) => Ok(ExportCreationResponse::full(value)),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}
async fn creator(conn: &mut Conn, user: Id) -> ApiResult<native::Facts<Source>> {
    let user = identity::user_by_id(conn, user)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let target = targets::from_account(conn, &user).await?;
    Ok(native::Facts::one(
        Source::Creator,
        target.object.kind,
        target,
    ))
}
impl effects::Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = ExportRequest;
    type Value = Ready;
    type Output = Queued;
    fn declaration(_: &Ready) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ExportRequest,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            rows::lock_person(conn, caller.user()).await?;
            let (selected, mut facts) = super::planning_sources::prepare_for(
                ctx,
                conn,
                caller,
                &body,
                None,
                &sources::CREATE,
            )
            .await?;
            let required = facts
                .iter()
                .filter(|fact| matches!(fact.source, Source::Place | Source::Contents))
                .cloned()
                .collect();
            facts.push(creator(conn, caller.user()).await?);
            facts.push(native::Facts::collection(Source::People, Vec::new()));
            facts.push(native::Facts::collection(Source::References, Vec::new()));
            let plan = native::bind_required(ctx, &sources::CREATE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let prepared = match ctx
                .app
                .engine
                .prepare_export_creation(conn, &principal, &selected.plan, &body)
                .await
            {
                Ok(prepared) => prepared,
                Err(error) => return Err(failed(conn, error).await),
            };
            Ok((Ready { prepared, required }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .export_place_seen(conn, principal, ctx.id("id")?)
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }
}
impl effects::Apply for Create {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Queued>>> {
        Box::pin(async move {
            let made = match ctx
                .app
                .engine
                .apply_export_creation(
                    conn,
                    principal,
                    principal.caller(),
                    &origin(ctx),
                    &value.prepared,
                    move || {
                        before.check().map_err(|error| {
                            orch_engine::EngineError::BeforeEffect(Box::new(error))
                        })
                    },
                )
                .await
            {
                Ok(made) => made,
                Err(error) => return Err(failed(conn, error).await),
            };
            Ok(effects::Outcome::Applied(Queued {
                made,
                required: value.required.clone(),
            }))
        })
    }
}
pub(super) async fn run(ctx: Ctx, (): (), _: ExportRequest) -> ApiResult<ExportCreationResponse> {
    let (credential, until) = transaction_guard::native_origin(&ctx).await?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Create>(&ctx, &mut tx).await?;
    let retained = operation::Request::retain(
        &mut tx,
        &done.output.made,
        &done.accepted,
        credential,
        until,
        ctx.app.clock.time(),
    )
    .await;
    let retained = match retained {
        Ok(value) => value,
        Err(error) => return Err(failed(&mut tx, error).await),
    };
    retained
        .caller(&mut tx, ctx.app.clock.as_ref())
        .await
        .map_err(|error| match error {
            orch_engine::operation_scope::continuing::Error::Refused
            | orch_engine::operation_scope::continuing::Error::Binding(
                orch_engine::operation_scope::BindingError::Refused,
            ) => ApiError::new(
                ErrorCode::AuthzDenied,
                "the original access no longer permits this export",
            ),
            other => ApiError::internal(other),
        })?;
    orch_db::idempotency::creation::observe_with(&mut tx,Some(ctx.user()?.id()),"export_job",done.output.made.job.id,serde_json::json!({"export":done.output.made.export.id,"node":done.output.made.export.node_id})).await?;
    guard.check_deadline(&ctx)?;
    if !done.accepted.key_deadline_allows(ctx.app.clock.as_ref()) {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the original export key has ended",
        ));
    }
    tx.commit().await.map_err(DbError::from)?;
    let receipt = ExportCreationReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        id: done.output.made.job.id,
        export_id: done.output.made.export.id,
    };
    publication::receipt(&done.ctx, &receipt)?;
    let projected = reply::render::<Job>(&done.ctx, &done.accepted, done.output.into()).await;
    ctx.app.engine.kick();
    match projected {
        Ok(Some(full)) => Ok(ExportCreationResponse::full(full)),
        Ok(None) => Ok(ExportCreationResponse::Receipt(receipt)),
        Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => {
            Ok(ExportCreationResponse::Receipt(receipt))
        }
        Err(error) => Err(error),
    }
}
struct Job;
impl reply::Projection for Job {
    type Source = Source;
    type Input = Accepted;
    type Subject = jobs::JobRow;
    type Value = ();
    type Reply = orch_api_contract::workspace::Job;
    const READ_SOURCE: Source = Source::Creator;
    fn declaration(_: &Accepted) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let job = jobs::job(conn, input.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if job.kind != jobs::kind::EXPORT
                || job.created_by != input.user
                || job.plan["export"] != serde_json::json!(input.export)
                || job.plan["node"] != serde_json::json!(input.node)
                || job.plan["user"] != serde_json::json!(input.user)
            {
                return Err(ApiError::not_found());
            }
            // These are the accepted act's real operands, not fresh source content or new admission.
            let mut facts = input.required.clone();
            // The native initial Job names no place; these response collections are genuinely empty.
            for role in [
                Source::Orgs,
                Source::Projects,
                Source::Topics,
                Source::Threads,
                Source::People,
                Source::References,
            ] {
                facts.push(native::Facts::collection(role, Vec::new()));
            }
            facts.push(creator(conn, job.created_by.ok_or_else(ApiError::not_found)?).await?);
            Ok((job, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Accepted,
        job: &'a jobs::JobRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if job.created_by == Some(principal.user()) {
                Ok(())
            } else {
                Err(ApiError::not_found())
            }
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        _: &'a jobs::JobRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Accepted,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            job_view::view(conn, input.initial.as_ref().unwrap_or(&value.subject)).await
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.kind == fresh.subject.kind
                && old.subject.created_by == fresh.subject.created_by
                && old.subject.created_at == fresh.subject.created_at
                && [
                    "export",
                    "node",
                    "user",
                    "export_operation",
                    "export_operation_required",
                ]
                .iter()
                .all(|key| old.subject.plan.get(*key) == fresh.subject.plan.get(*key)))
        })
    }
}
