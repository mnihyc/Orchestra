//! Queue the same native undo under a real first-effect cut and retain its original credential.

use super::*;
use crate::key_operations::{
    native::{
        self,
        effects::{self, Apply, Effect, FirstEffect, Outcome},
        reply,
    },
    publication,
};
use futures::future::BoxFuture;
use orch_api_contract::{
    import_undo_sources::{self as sources, Source},
    imports::{ImportUndoReceipt, ImportUndoResponse},
    operation_sources::Declaration,
};
use orch_authz::{Caller, Principal};
use orch_db::queries::jobs;
use orch_engine::imports::{
    PreparedUndoJob,
    undo::{Prepared, operation},
};
use orch_types::Id;

pub(super) struct Undo;
pub(super) struct Ready {
    job: PreparedUndoJob,
}
pub(super) struct Made {
    made: orch_engine::imports::Made,
    replay: bool,
}

impl Effect for Undo {
    type Source = Source;
    type Query = ();
    type Body = ImportUndoBody;
    type Value = Ready;
    type Output = Made;
    fn declaration(_: &Ready) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        _: ImportUndoBody,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let job = match ctx
                .app
                .engine
                .prepare_undo_job(conn, caller, ctx.id("id")?)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let value = match ctx
                .app
                .engine
                .prepare_undo_effects(conn, caller.user(), job.import().id)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let (mut facts, _) = super::undo_sources::facts(conn, &value).await?;
            facts.push(
                operation::creator(conn, caller.user())
                    .await
                    .map_err(super::undo_sources::source_error)?,
            );
            Ok((Ready { job }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(super::read_sources::require_own(ctx, conn, principal))
    }
}
impl Apply for Undo {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        ready: &'a Ready,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Made>>> {
        Box::pin(async move {
            let replay = ready.job.replay().is_some();
            let prepared = ready.job.clone();
            let made = match ctx
                .app
                .engine
                .apply_undo_job_before(conn, principal.caller(), prepared, move || {
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                })
                .await
            {
                Ok(made) => made,
                Err(error) => return Err(failed(conn, error).await),
            };
            let made = Made { made, replay };
            Ok(if replay {
                Outcome::Unchanged(made)
            } else {
                Outcome::Applied(made)
            })
        })
    }
}

pub(super) async fn run(ctx: Ctx, (): (), _: ImportUndoBody) -> ApiResult<ImportUndoResponse> {
    let (credential, until) = transaction_guard::native_origin(&ctx).await?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Undo>(&ctx, &mut tx).await?;
    if !done.output.replay {
        let retained = operation::Request::retain(
            &mut tx,
            &done.output.made,
            &done.accepted,
            credential,
            until,
        )
        .await
        .map_err(ApiError::engine)?;
        retained
            .caller(&mut tx, ctx.app.clock.as_ref())
            .await
            .map_err(super::undo_sources::source_error)?;
    }
    let job = done.output.made.job.id;
    guard.check_deadline(&ctx)?;
    if !done.accepted.key_deadline_allows(ctx.app.clock.as_ref()) {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the original undo key has ended",
        ));
    }
    tx.commit().await.map_err(DbError::from)?;
    let receipt = ImportUndoReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        id: job,
    };
    publication::receipt(&done.ctx, &receipt)?;
    let projected = reply::render::<Job>(
        &done.ctx,
        &done.accepted,
        ReplyInput {
            import: done.output.made.import.id,
            job: done.output.made.job,
        },
    )
    .await;
    ctx.app.engine.kick();
    match projected {
        Ok(Some(full)) => Ok(ImportUndoResponse::full(full)),
        Ok(None) => Ok(ImportUndoResponse::Receipt(receipt)),
        Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => {
            Ok(ImportUndoResponse::Receipt(receipt))
        }
        Err(error) => Err(error),
    }
}

pub(super) struct Job;
pub(super) struct ReplyInput {
    import: Id,
    job: jobs::JobRow,
}
impl reply::Projection for Job {
    type Source = Source;
    type Input = ReplyInput;
    type Subject = (Prepared, jobs::JobRow);
    type Value = ();
    type Reply = orch_api_contract::workspace::Job;
    const READ_SOURCE: Source = Source::Import;
    fn declaration(_: &Self::Input) -> &'static Declaration<Source> {
        &sources::CREATE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let native = match ctx
                .app
                .engine
                .prepare_undo_effects(conn, ctx.user()?.id(), input.import)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let job = jobs::job(conn, input.job.id)
                .await?
                .filter(|job| job.created_by == native.import.user_id)
                .ok_or_else(ApiError::not_found)?;
            if native.import.undo_job_id != Some(job.id)
                || job.kind != jobs::kind::IMPORT
                || input.job.kind != jobs::kind::IMPORT
                || job.plan["import"] != serde_json::json!(native.import.id)
                || job.plan["user"] != serde_json::json!(ctx.user()?.id())
                || job.plan["undo"] != true
            {
                return Err(ApiError::not_found());
            }
            let (mut facts, _) = super::undo_sources::facts(conn, &native).await?;
            facts.push(
                operation::creator(conn, job.created_by.ok_or_else(ApiError::not_found)?)
                    .await
                    .map_err(super::undo_sources::source_error)?,
            );
            Ok(((native, job), facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Self::Input,
        _: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(super::read_sources::require_own(ctx, conn, principal))
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Input,
        _: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Self::Input,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { job_view::view(conn, &input.job).await })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Input,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.1.id == fresh.subject.1.id
                && opened.subject.1.created_by == fresh.subject.1.created_by
                && opened.subject.1.created_at == fresh.subject.1.created_at
                && [
                    "import",
                    "user",
                    "node",
                    "undo",
                    "undo_operation",
                    "undo_operation_required",
                ]
                .iter()
                .all(|key| opened.subject.1.plan.get(*key) == fresh.subject.1.plan.get(*key))
                && super::read_sources::preserves(
                    &opened.subject.0.import,
                    &fresh.subject.0.import,
                ))
        })
    }
}
