//! One stored label mutation, followed by a current native metadata projection.

use super::{label_of, snapshot_read, subject, view};
use crate::key_operations::native::{self, effects, reply};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::permissions::Permission;
use orch_api_contract::{
    common::{ErrorCode, ReceiptProjection},
    operation_sources::Declaration,
    workspace::{Snapshot, SnapshotUpdateBody, SnapshotUpdateReceipt, SnapshotUpdateResponse},
    workspace_sources::{self as sources, Source},
};
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn, DbError,
    queries::{storage, threads, transaction_guard as transactions},
};
use orch_engine::archive::snapshot_authority::Source as SnapshotSource;
use orch_types::Id;

pub(super) struct Update;
pub(super) struct Operands {
    source: SnapshotSource,
    label: String,
}
pub(super) struct Changed {
    source: SnapshotSource,
}

fn same(old: &SnapshotSource, fresh: &SnapshotSource) -> bool {
    old.witness().matches(&fresh.row)
        && old.governing_environment() == fresh.governing_environment()
}

impl effects::Effect for Update {
    type Source = Source;
    type Query = ();
    type Body = SnapshotUpdateBody;
    type Value = Operands;
    type Output = Changed;

    fn declaration(_: &Operands) -> &'static Declaration<Source> {
        &sources::SNAPSHOT_UPDATE
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: SnapshotUpdateBody,
    ) -> BoxFuture<'a, ApiResult<(Operands, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let label = label_of(ctx, Some(&body.label))?;
            if body.label.trim().is_empty() {
                return Err(ApiError::invalid("label", "a snapshot is called something"));
            }
            let original = subject::load(ctx, conn).await?;
            let facts = subject::governor_facts(conn, &original).await?;
            let known = native::bind_required(ctx, &sources::SNAPSHOT_UPDATE, &facts)?;
            let actor = native::preparation_principal(ctx, conn, caller, &known).await?;
            snapshot_read::require(conn, &actor, &original, Permission::FilesManage).await?;
            let source = if transactions::in_transaction(conn) {
                if let Some(thread) = original
                    .row
                    .thread_id
                    .filter(|_| original.row.storage != "environment")
                    && threads::lock(conn, thread).await?.is_none()
                {
                    return Err(ApiError::not_found());
                }
                let locked: Option<Id> =
                    sqlx::query_scalar("SELECT id FROM snapshots WHERE id=$1 FOR UPDATE")
                        .bind(original.row.id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                if locked.is_none() {
                    return Err(ApiError::not_found());
                }
                let fresh = subject::load(ctx, conn).await?;
                if !same(&original, &fresh) {
                    return Err(ApiError::new(
                        ErrorCode::ConflictStaleRevision,
                        "the Snapshot custody changed before its label was updated",
                    ));
                }
                fresh
            } else {
                original
            };
            let facts = subject::governor_facts(conn, &source).await?;
            Ok((Operands { source, label }, facts))
        })
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Operands,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(snapshot_read::require(
            conn,
            principal,
            &value.source,
            Permission::FilesManage,
        ))
    }
}

impl effects::Apply for Update {
    fn apply<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a Operands,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Changed>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            before.check()?;
            if !storage::relabel_snapshot(conn, value.source.row.id, &value.label).await? {
                return Err(ApiError::not_found());
            }
            deadline.check()?;
            Ok(effects::Outcome::Applied(Changed {
                source: value.source.clone(),
            }))
        })
    }
}

fn receipt(id: Id) -> SnapshotUpdateResponse {
    SnapshotUpdateResponse::Receipt(SnapshotUpdateReceipt {
        projection: ReceiptProjection::Receipt,
        snapshot_id: id,
    })
}

struct Projection;
impl reply::Projection for Projection {
    type Source = Source;
    type Input = Changed;
    type Subject = SnapshotSource;
    type Value = ();
    type Reply = Snapshot;
    const READ_SOURCE: Source = Source::SnapshotGovernor;

    fn declaration(_: &Changed) -> &'static Declaration<Source> {
        &sources::SNAPSHOT_UPDATE
    }

    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Changed,
    ) -> BoxFuture<'a, ApiResult<(SnapshotSource, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let current = subject::load(ctx, conn).await?;
            if !same(&input.source, &current) {
                return Err(ApiError::not_found());
            }
            let facts = subject::metadata_facts(conn, &current).await?;
            Ok((current, facts))
        })
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Changed,
        source: &'a SnapshotSource,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            // Native management already allowed this generated response before scoped keys.
            // Same-operation response Read is checked independently by the shared projector.
            match snapshot_read::require(conn, principal, source, Permission::FilesView).await {
                Ok(()) => Ok(()),
                Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
                    snapshot_read::require(conn, principal, source, Permission::FilesManage).await
                }
                Err(error) => Err(error),
            }
        })
    }

    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Changed,
        _: &'a SnapshotSource,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }

    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Changed,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Snapshot>> {
        Box::pin(view(conn, prepared.subject.row.clone(), principal.now()))
    }

    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Changed,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(same(&opened.subject, &fresh.subject)) })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: SnapshotUpdateBody,
) -> ApiResult<SnapshotUpdateResponse> {
    let mut tx = ctx.app.db.begin().await?;
    let completed = effects::apply::<Update>(&ctx, &mut tx).await?;
    let identity = transaction_guard::current_http_identity(
        &completed.ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    transaction_guard::guard_user(&identity.user)?;
    identity.check_deadline(&ctx.app)?;
    if !completed
        .accepted
        .key_deadline_allows(ctx.app.clock.as_ref())
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the snapshot label credential has expired",
        ));
    }
    tx.commit().await.map_err(DbError::from)?;
    let fallback = receipt(completed.output.source.row.id);
    crate::key_operations::publication::receipt(&completed.ctx, &fallback)?;
    Ok(
        match reply::render::<Projection>(&completed.ctx, &completed.accepted, completed.output)
            .await?
        {
            Some(snapshot) => SnapshotUpdateResponse::Full(Box::new(snapshot)),
            None => fallback,
        },
    )
}
