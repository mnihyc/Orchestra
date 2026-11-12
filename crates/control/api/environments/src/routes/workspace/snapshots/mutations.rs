//! Stored Snapshot metadata mutations through the shared subject and response boundary.

use super::{Snapshot, label_of, snapshot_read, storage, view};
use crate::key_operations::native;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::workspace::{
    SnapshotUpdateBody, SnapshotUpdateReceipt, SnapshotUpdateResponse,
};
use orch_api_contract::workspace_sources::{self as sources, Source};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError};
use orch_engine::archive::snapshot_authority::Source as SnapshotSource;
use orch_engine::operation_scope::targets::{self, TargetError};
use orch_types::Id;

const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

pub(super) struct Snapshots;

async fn identity(conn: &mut Conn, id: Id) -> ApiResult<KeyResolvedTarget> {
    targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id,
        },
    )
    .await
    .map_err(|error| match error {
        TargetError::Missing => ApiError::not_found(),
        TargetError::Db(error) => error.into(),
        TargetError::Authz(error) => {
            snapshot_read::engine_error(orch_engine::EngineError::Authz(error))
        }
    })
}

async fn facts(conn: &mut Conn, source: &SnapshotSource) -> ApiResult<Vec<native::Facts<Source>>> {
    let governor = identity(conn, source.governing_environment()).await?;
    let origin = if source.governing_environment() == source.row.environment_id {
        governor.clone()
    } else {
        identity(conn, source.row.environment_id).await?
    };
    Ok(vec![
        native::Facts::one(Source::SnapshotGovernor, ENVIRONMENT, governor),
        native::Facts::one(Source::SnapshotOrigin, ENVIRONMENT, origin),
    ])
}

async fn require(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    source: &SnapshotSource,
    permission: Permission,
) -> ApiResult<()> {
    let result = snapshot_read::require(conn, principal, source, permission).await;
    if ctx.key_receipt_required() {
        result.map_err(authorization::problem_without_content)
    } else {
        result
    }
}

impl native::Stored for Snapshots {
    type Source = Source;
    type Value = SnapshotSource;
    const ROWS: &'static [Declaration<Source>] = &[sources::SNAPSHOT_UPDATE];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(SnapshotSource, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let source = SnapshotSource::load(conn, ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let facts = facts(conn, &source).await?;
            Ok((source, facts))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        source: &'a SnapshotSource,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            ctx,
            conn,
            principal,
            source,
            Permission::FilesManage,
        ))
    }
}

fn receipt(id: Id) -> SnapshotUpdateResponse {
    SnapshotUpdateResponse::Receipt(SnapshotUpdateReceipt {
        projection: ReceiptProjection::Receipt,
        snapshot_id: id,
    })
}

async fn response(ctx: &Ctx, accepted: &Caller) -> ApiResult<Option<Snapshot>> {
    let projection_accepted = accepted.clone();
    let full = native::completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let source = SnapshotSource::load(conn, ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let witness = source.witness();
            if !native::modern(current.caller()) {
                snapshot_read::require(conn, current, &source, Permission::FilesManage).await?;
                return view(conn, source.row, current.now())
                    .await
                    .map(|snapshot| Some((snapshot, witness)));
            }
            let plan = native::bind(ctx, &sources::SNAPSHOT_UPDATE, &facts(conn, &source).await?)?;
            plan.read_response(ctx, conn, &projection_accepted, move |conn, reader| {
                Box::pin(async move {
                    snapshot_read::require(conn, reader, &source, Permission::FilesView).await?;
                    view(conn, source.row, reader.now()).await
                })
            })
            .await
            .map(|full| full.map(|snapshot| (snapshot, witness)))
        })
    })
    .await?;
    let Some((snapshot, witness)) = full else {
        return Ok(None);
    };
    crate::key_operations::publication::retain(
        ctx,
        accepted,
        witness,
        |ctx, conn, reader, witness| {
            Box::pin(async move {
                let source = SnapshotSource::load(conn, ctx.id("id")?)
                    .await?
                    .filter(|source| witness.matches(&source.row))
                    .ok_or_else(ApiError::not_found)?;
                if super::environment_rows::environment(conn, source.row.environment_id)
                    .await?
                    .is_none()
                {
                    return Err(ApiError::not_found());
                }
                if !native::modern(reader.caller()) {
                    return snapshot_read::require(conn, reader, &source, Permission::FilesManage)
                        .await;
                }
                let plan =
                    native::bind(ctx, &sources::SNAPSHOT_UPDATE, &facts(conn, &source).await?)?;
                let reads = plan
                    .shared()
                    .response_facets()
                    .map_err(crate::key_operations::admission::binding_error)?;
                let caller = reader
                    .caller()
                    .admit_facets(&reads)
                    .map_err(|_| {
                        ApiError::new(
                            orch_api_contract::common::ErrorCode::AuthzDenied,
                            "the completed Snapshot response is no longer readable",
                        )
                    })?
                    .into_caller();
                plan.read_response(ctx, conn, &caller, move |conn, current| {
                    Box::pin(async move {
                        snapshot_read::require(conn, current, &source, Permission::FilesView).await
                    })
                })
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        orch_api_contract::common::ErrorCode::AuthzDenied,
                        "the completed Snapshot response is no longer readable",
                    )
                })
            })
        },
    )?;
    Ok(Some(snapshot))
}

pub(super) async fn update(
    ctx: Ctx,
    (): (),
    body: SnapshotUpdateBody,
) -> ApiResult<SnapshotUpdateResponse> {
    let label = label_of(&ctx, Some(&body.label)).and_then(|label| {
        if body.label.trim().is_empty() {
            Err(ApiError::invalid("label", "a snapshot is called something"))
        } else {
            Ok(label)
        }
    })?;
    let id = ctx.id("id")?;
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let original = SnapshotSource::load(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    require(
        &ctx,
        &mut tx,
        initial.principal(),
        &original,
        Permission::FilesManage,
    )
    .await?;
    let original = original.row;
    if let Some(thread) = original
        .thread_id
        .filter(|_| original.storage != "environment")
    {
        let thread: Option<Id> =
            sqlx::query_scalar("SELECT node_id FROM threads WHERE node_id=$1 FOR SHARE")
                .bind(thread)
                .fetch_optional(&mut *tx)
                .await
                .map_err(DbError::from)?;
        if thread.is_none() {
            return Err(ApiError::not_found());
        }
    }
    let locked: Option<Id> = sqlx::query_scalar("SELECT id FROM snapshots WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(DbError::from)?;
    if locked.is_none() {
        return Err(ApiError::not_found());
    }
    let locked = storage::snapshot(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if original.thread_id != locked.thread_id
        || original.environment_id != locked.environment_id
        || original.storage != locked.storage
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::ConflictStaleRevision,
            "the Snapshot custody changed before its label was updated",
        ));
    }
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let source = if native::modern(guard.principal().caller()) {
        guard
            .operation_data::<native::Prepared<SnapshotSource>>()?
            .value
            .clone()
    } else {
        SnapshotSource::load(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?
    };
    require(
        &ctx,
        &mut tx,
        guard.principal(),
        &source,
        Permission::FilesManage,
    )
    .await?;
    guard.check_deadline(&ctx)?;
    if !storage::relabel_snapshot(&mut tx, source.row.id, &label).await? {
        return Err(ApiError::not_found());
    }
    let accepted = guard.principal().caller().clone();
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&ctx, &receipt(id))?;
    Ok(match response(&ctx, &accepted).await? {
        Some(snapshot) => SnapshotUpdateResponse::Full(Box::new(snapshot)),
        None => receipt(id),
    })
}
