//! Completed starts read the real Snapshot's current custody without its spent command source.
use super::*;
use crate::middleware::idempotency::ObservedCreation;
use orch_db::queries::jobs::{self, JobRow};
use orch_engine::archive::snapshot_authority::{Access, Source as SnapshotSource};
struct Input {
    snapshot: Id,
    job: Id,
    thread: bool,
}
struct Subject {
    source: SnapshotSource,
    job: JobRow,
}
struct Content;
pub(crate) fn binding<B: Send + 'static, const THREAD: bool>()
-> ObservedCreation<(), B, SnapshotStartResponse> {
    ObservedCreation {
        kind: "snapshot_start",
        replay: |ctx, (), _body, created| {
            Box::pin(async move {
                let job: Id =
                    serde_json::from_value(created.data.get("job").cloned().ok_or_else(|| {
                        ApiError::internal("the accepted snapshot has no native job")
                    })?)
                    .map_err(ApiError::internal)?;
                let receipt = SnapshotStartResponse::Receipt(SnapshotStartReceipt {
                    projection: ReceiptProjection::Receipt,
                    job_id: job,
                    snapshot_id: created.id,
                });
                crate::key_operations::publication::receipt(&ctx, &receipt)?;
                let thread = THREAD;
                match reply::render_completed_replay::<Content>(
                    &ctx,
                    ctx.user()?.id(),
                    Input {
                        snapshot: created.id,
                        job,
                        thread,
                    },
                )
                .await
                {
                    Ok(Some(value)) => Ok(SnapshotStartResponse::Full(Box::new(value))),
                    Ok(None) => Ok(receipt),
                    Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                        Ok(receipt)
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}
impl reply::Projection for Content {
    type Source = Source;
    type Input = Input;
    type Subject = Subject;
    type Value = ();
    type Reply = Job;
    const READ_SOURCE: Source = Source::SnapshotContent;
    fn declaration(input: &Input) -> &'static Declaration<Source> {
        declaration(input.thread)
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let source = SnapshotSource::load(conn, input.snapshot)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if input.thread {
                if source.row.thread_id != Some(ctx.id("id")?) {
                    return Err(ApiError::not_found());
                }
            } else if source.row.thread_id.is_some() || source.row.environment_id != ctx.id("id")? {
                return Err(ApiError::not_found());
            }
            let job = jobs::job(conn, input.job)
                .await?
                .filter(|row| {
                    row.kind == jobs::kind::SNAPSHOT
                        && row.plan.get("snapshot_id") == Some(&serde_json::json!(input.snapshot))
                        && row.environment_id == Some(source.row.environment_id)
                        && row.node_id == source.row.thread_id
                        && row.created_by == source.row.created_by
                })
                .ok_or_else(ApiError::not_found)?;
            let origin = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: source.row.environment_id,
                },
            )
            .await?;
            let governor = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: source.governing_environment(),
                },
            )
            .await?;
            Ok((
                Subject { source, job },
                vec![
                    native::Facts::one(
                        Source::SnapshotEnvironment,
                        KeyTargetKind::Object(ObjectKind::Environment),
                        origin,
                    ),
                    native::Facts::one(
                        Source::SnapshotContent,
                        KeyTargetKind::Object(ObjectKind::Environment),
                        governor,
                    ),
                ],
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Input,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match subject
                .source
                .check(conn, p, Permission::FilesView)
                .await
                .map_err(ApiError::internal)?
            {
                Access::Allowed => return Ok(()),
                Access::MakerOnly => return Err(ApiError::not_found()),
                Access::Refused(_) => {}
            }
            let permission = if input.thread {
                Permission::FilesManage
            } else {
                Permission::EnvironmentManage
            };
            match subject
                .source
                .check(conn, p, permission)
                .await
                .map_err(ApiError::internal)?
            {
                Access::Allowed => Ok(()),
                Access::MakerOnly | Access::Refused(_) => Err(ApiError::not_found()),
            }
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            crate::routes::environments::jobs::view_with_machine_sources(
                conn,
                &opened.subject.job,
                Some(&HashMap::new()),
            )
            .await
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
            Ok(
                old.subject.source.witness() == fresh.subject.source.witness()
                    && old.subject.source.governing_environment()
                        == fresh.subject.source.governing_environment()
                    && old.subject.job.id == fresh.subject.job.id
                    && old.subject.job.created_by == fresh.subject.job.created_by,
            )
        })
    }
}
