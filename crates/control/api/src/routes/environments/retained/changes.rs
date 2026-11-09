//! Native kept-file reservations use the shared effect and independent job projection adapters.

use super::*;
use crate::key_operations::{
    native::{self, effects, reply},
    publication, targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};
use orch_api_contract::{
    environment_sources::{self as declarations, RetainedChangeSource as Source},
    operation_sources::Declaration,
    operations::FacetUsage,
};
use orch_authz::{Caller, CapturedOperation};

pub(super) struct Restore;
pub(super) struct Purge;
pub(super) struct Ready {
    environment: Id,
    revision: i64,
    purge: bool,
    existing: Option<job_rows::JobRow>,
}
pub(super) struct Saved {
    row: job_rows::JobRow,
    purge: bool,
}
fn declaration(purge: bool) -> &'static Declaration<Source> {
    if purge {
        &declarations::PURGE
    } else {
        &declarations::RESTORE
    }
}
async fn facts(conn: &mut Conn, id: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![native::Facts::one(
        Source::Environment,
        KeyTargetKind::Object(ObjectKind::Environment),
        targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id,
            },
        )
        .await?,
    )])
}
async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    revision: i64,
    purge: bool,
    confirm: Option<String>,
) -> ApiResult<(Ready, Vec<native::Facts<Source>>)> {
    let id = ctx.id("id")?;
    let known = native::bind_required(ctx, declaration(purge), &facts(conn, id).await?)?;
    let principal = known.preparation_principal(ctx, conn, caller).await?;
    may_read(conn, &principal, id).await?;
    let env = hosting::lock_environment(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let actual = facts(conn, id).await?;
    let known = native::bind_required(ctx, declaration(purge), &actual)?;
    let principal = known.preparation_principal(ctx, conn, caller).await?;
    let (manager, admin) = may_read(conn, &principal, id).await?;
    let removal = retained::removal(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if removal.purged_at.is_some() {
        return Err(ApiError::new(
            ErrorCode::Gone,
            "the owned files were removed for good",
        ));
    }
    if removal.removal_revision != revision {
        return Err(
            ApiError::new(ErrorCode::ConflictStaleRevision, "the removal changed")
                .with("current", removal.removal_revision),
        );
    }
    if purge
        && !admin
        && !(manager
            && removal
                .volumes_purge_at
                .is_some_and(|at| at <= ctx.app.clock.now()))
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "only a current home admin may shorten the kept-file window",
        ));
    }
    if confirm.is_some_and(|name| name != env.name) {
        return Err(super::super::invalid(
            "confirm",
            "must be the environment's name",
        ));
    }
    let existing = job_rows::retained::open_of_environment(conn, id)
        .await?
        .into_iter()
        .next();
    if let Some(job) = &existing {
        if job.kind
            != if purge {
                job_rows::retained::PURGE
            } else {
                job_rows::retained::RESTORE
            }
        {
            return Err(ApiError::new(
                ErrorCode::ResourceInUse,
                "another lifecycle job owns the kept files",
            )
            .with("job", job.id));
        }
    } else if removal.removal_phase.as_deref() != Some("kept") {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "the kept files are unavailable during their current phase",
        ));
    }
    Ok((
        Ready {
            environment: id,
            revision,
            purge,
            existing,
        },
        actual,
    ))
}
macro_rules! effect {
    ($type:ty,$query:ty,$body:ty,$purge:expr,$input:expr) => {
        impl effects::Effect for $type {
            type Source = Source;
            type Query = $query;
            type Body = $body;
            type Value = Ready;
            type Output = Saved;
            fn declaration(_: &Ready) -> &'static Declaration<Source> {
                declaration($purge)
            }
            fn prepare<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                caller: &'a Caller,
                query: Self::Query,
                body: Self::Body,
            ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
                Box::pin(async move {
                    let (revision, confirm) = ($input)(query, body);
                    prepare(ctx, conn, caller, revision, $purge, confirm).await
                })
            }
            fn require<'a>(
                _: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Ready,
            ) -> BoxFuture<'a, ApiResult<()>> {
                Box::pin(async move {
                    may_read(conn, principal, value.environment).await?;
                    Ok(())
                })
            }
        }
        impl effects::Apply for $type {
            fn apply<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Ready,
                before: effects::FirstEffect<'a>,
            ) -> BoxFuture<'a, ApiResult<effects::Outcome<Saved>>> {
                Box::pin(apply(ctx, conn, principal, value, before))
            }
        }
    };
}
effect!(
    Restore,
    (),
    api::EnvironmentRestoreBody,
    false,
    |(): (), body: api::EnvironmentRestoreBody| (body.expected_revision, None)
);
effect!(
    Purge,
    api::EnvironmentPurgeQuery,
    (),
    true,
    |query: api::EnvironmentPurgeQuery, (): ()| (query.expected_revision, Some(query.confirm))
);
async fn apply(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Ready,
    before: effects::FirstEffect<'_>,
) -> ApiResult<effects::Outcome<Saved>> {
    let deadline = before.deadline();
    if let Some(row) = &value.existing {
        deadline.check()?;
        return Ok(effects::Outcome::Unchanged(Saved {
            row: row.clone(),
            purge: value.purge,
        }));
    }
    let origin = CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
    let row = orch_engine::jobs::retained::operation::queue_for_operation(
        conn,
        value.environment,
        value.revision,
        value.purge,
        principal.user(),
        identity(ctx)?,
        &origin,
        ctx.app.clock.now(),
        move || {
            before
                .check()
                .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
        },
    )
    .await
    .map_err(|error| match error {
        orch_engine::jobs::runner::MakeError::Running(job) => ApiError::new(
            ErrorCode::ResourceInUse,
            "another job owns this environment",
        )
        .with("job", job.id),
        orch_engine::jobs::runner::MakeError::Internal(error) => ApiError::engine(error),
        _ => ApiError::new(ErrorCode::ConflictStaleRevision, "the removal changed"),
    })?;
    deadline.check()?;
    Ok(effects::Outcome::Applied(Saved {
        row,
        purge: value.purge,
    }))
}
pub(super) async fn restore(
    ctx: Ctx,
    (): (),
    _: api::EnvironmentRestoreBody,
) -> ApiResult<api::RetainedChangeResponse> {
    run::<Restore>(ctx).await
}
pub(super) async fn purge(
    ctx: Ctx,
    _: api::EnvironmentPurgeQuery,
    (): (),
) -> ApiResult<api::RetainedChangeResponse> {
    run::<Purge>(ctx).await
}
async fn run<F: effects::Apply<Value = Ready, Output = Saved>>(
    ctx: Ctx,
) -> ApiResult<api::RetainedChangeResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<F>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::RetainedChangeReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        environment_id: done
            .output
            .row
            .environment_id
            .ok_or_else(ApiError::not_found)?,
        job_id: done.output.row.id,
    };
    publication::receipt(&done.ctx, &receipt)?;
    ctx.app.engine.kick();
    let full = match reply::render::<Content>(&done.ctx, &done.accepted, done.output).await {
        Ok(full) => full,
        Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => None,
        Err(error) => return Err(error),
    };
    Ok(match full {
        Some(job) => api::RetainedChangeResponse::Full(Box::new(job)),
        None => api::RetainedChangeResponse::Receipt(receipt),
    })
}
struct Content;
impl reply::Projection for Content {
    type Source = Source;
    type Input = Saved;
    type Subject = job_rows::JobRow;
    type Value = HashMap<Id, String>;
    type Reply = Job;
    const READ_SOURCE: Source = Source::Environment;
    fn declaration(input: &Saved) -> &'static Declaration<Source> {
        declaration(input.purge)
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Saved,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = job_rows::job(conn, input.row.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let actual = facts(
                conn,
                input.row.environment_id.ok_or_else(ApiError::not_found)?,
            )
            .await?;
            Ok((row, actual))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Saved,
        row: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if row.id != input.row.id
                || row.kind != input.row.kind
                || row.environment_id != input.row.environment_id
                || row.created_by != input.row.created_by
                || row.created_at != input.row.created_at
            {
                return Err(ApiError::not_found());
            }
            may_read(
                conn,
                principal,
                input.row.environment_id.ok_or_else(ApiError::not_found)?,
            )
            .await?;
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Saved,
        _: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let declaration = declaration(input.purge);
            let known = native::bind_known(
                ctx,
                declaration,
                &facts(
                    conn,
                    input.row.environment_id.ok_or_else(ApiError::not_found)?,
                )
                .await?,
            )?;
            let mut names =
                jobs::machine_name_sources(conn, std::slice::from_ref(&input.row)).await?;
            let ids = names.keys().copied().collect::<Vec<_>>();
            let identities = targets::machine_identities(conn, &ids).await?;
            let mut allowed = Vec::new();
            for id in ids {
                if let Some(target) = identities.get(&id)
                    && native::permits_usage(
                        &known,
                        declaration,
                        principal.caller(),
                        &Source::WaitingMachines,
                        FacetUsage::ResponseContent,
                        KeyTargetKind::Object(ObjectKind::Machine),
                        target,
                    )?
                {
                    allowed.push(target.clone());
                } else {
                    names.remove(&id);
                }
            }
            let groups = if allowed.is_empty() {
                Vec::new()
            } else {
                vec![(KeyTargetKind::Object(ObjectKind::Machine), allowed)]
            };
            Ok((
                names,
                vec![native::Facts::collection(Source::WaitingMachines, groups)],
            ))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Saved,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            jobs::view_with_machine_sources(conn, &input.row, Some(&opened.value)).await
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Saved,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.value.keys().all(|id| fresh.value.contains_key(id))) })
    }
}
