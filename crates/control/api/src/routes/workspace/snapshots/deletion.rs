//! Native Snapshot deletion, with one manifest read before the guarded pin transaction.

use super::{
    snapshot_read,
    subject::{governor_facts as facts, load},
};
use crate::key_operations::native;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, HttpIdentityMode};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use native::effects::{self, Effect, Initial};
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::workspace_sources::{self as sources, Source};
use orch_authz::{Caller, Principal};
use orch_db::queries::{source, storage, threads, transaction_guard as transactions, uploads};
use orch_db::{Conn, DbError};
use orch_engine::archive::snapshot_authority::Source as SnapshotSource;
use orch_types::{Hash, Id};
use std::collections::HashSet;

pub(super) struct Delete;

pub(super) struct Manifest {
    source: SnapshotSource,
    listed: Vec<Hash>,
}

pub(super) struct Operands {
    source: SnapshotSource,
    released: Vec<Hash>,
}

async fn require(conn: &mut Conn, principal: &Principal, source: &SnapshotSource) -> ApiResult<()> {
    snapshot_read::require(conn, principal, source, Permission::FilesManage).await?;
    if source.row.state == storage::snapshot_state::RUNNING {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "the snapshot is being taken; cancel its job first",
        ));
    }
    Ok(())
}

fn same_manifest(opened: &Manifest, fresh: &SnapshotSource) -> ApiResult<()> {
    if !opened.source.witness().matches(&fresh.row) {
        return Err(ApiError::not_found());
    }
    Ok(())
}

async fn released(
    conn: &mut Conn,
    row: &storage::SnapshotRow,
    listed: &[Hash],
) -> ApiResult<Vec<Hash>> {
    let mut hashes = storage::snapshot_pins(conn, row.id).await?;
    if hashes.is_empty() {
        // Preserve the native fallback for retained states predating tracked logical pins.
        hashes.extend_from_slice(listed);
        hashes.extend(row.manifest_blob);
    }
    Ok(hashes)
}

impl Effect for Delete {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = Operands;
    type Output = ();

    fn declaration(_: &Operands) -> &'static Declaration<Source> {
        &sources::SNAPSHOT_DELETE
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Operands, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let opened = effects::initial_value::<Self>(ctx)?;
            let mut source = load(ctx, conn).await?;
            same_manifest(&opened, &source)?;
            let preliminary = facts(conn, &source).await?;
            let known = native::bind_required(ctx, &sources::SNAPSHOT_DELETE, &preliminary)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(conn, &principal, &source).await?;
            if transactions::in_transaction(conn) {
                // Native order: pin the actual Thread binding before the Snapshot row.
                if let Some(thread) = source.row.thread_id
                    && threads::lock(conn, thread).await?.is_none()
                {
                    return Err(ApiError::not_found());
                }
                let locked: Option<Id> =
                    sqlx::query_scalar("SELECT id FROM snapshots WHERE id=$1 FOR UPDATE")
                        .bind(source.row.id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                if locked.is_none() {
                    return Err(ApiError::not_found());
                }
                source = load(ctx, conn).await?;
                same_manifest(&opened, &source)?;
                sqlx::query("SELECT blob FROM snapshot_blob_refs WHERE snapshot_id=$1 ORDER BY blob FOR UPDATE")
                    .bind(source.row.id).execute(&mut *conn).await.map_err(DbError::from)?;
            }
            let released = released(conn, &source.row, &opened.listed).await?;
            if transactions::in_transaction(conn) {
                // Pin the native blob update footprint before admitting the first effect.
                uploads::lock_blobs(conn, &released).await?;
            }
            let facts = facts(conn, &source).await?;
            Ok((Operands { source, released }, facts))
        })
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Operands,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, &value.source))
    }
}

impl Initial for Delete {
    type Input = SnapshotSource;
    type Fetched = Manifest;

    fn declaration(_: &SnapshotSource) -> &'static Declaration<Source> {
        &sources::SNAPSHOT_DELETE
    }

    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(SnapshotSource, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let source = load(ctx, conn).await?;
            let facts = facts(conn, &source).await?;
            Ok((source, facts))
        })
    }

    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        source: &'a SnapshotSource,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, source))
    }

    fn fetch<'a>(
        ctx: &'a Ctx,
        _: &'a Caller,
        source: SnapshotSource,
    ) -> BoxFuture<'a, ApiResult<Manifest>> {
        Box::pin(async move {
            // This supplier owns no connection and is outside every retryable source callback.
            let listed = match source
                .row
                .manifest_blob
                .filter(|_| source.row.storage == "blobs")
            {
                Some(manifest) => ctx
                    .app
                    .cache
                    .manifest_of(&ctx.app.blobs, source.row.id, &manifest)
                    .await
                    .map(|tree| tree.hashes().collect::<HashSet<_>>().into_iter().collect())
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            Ok(Manifest { source, listed })
        })
    }
}

impl effects::Apply for Delete {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a Operands,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<()>>> {
        Box::pin(async move {
            before.check()?;
            let removed = if value.source.row.storage == "environment" {
                source::expire_recorded(conn, value.source.row.id, ctx.now).await?
            } else {
                storage::delete_snapshot(conn, value.source.row.id).await?
            };
            if removed.is_none() {
                return Err(ApiError::not_found());
            }
            ctx.app.blobs.unpin(conn, &value.released, ctx.now).await?;
            Ok(effects::Outcome::Applied(()))
        })
    }
}

pub(super) async fn run(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let mut tx = ctx.app.db.begin().await?;
    let completed = effects::apply::<Delete>(&ctx, &mut tx).await?;
    // The body-free acknowledgement needs no re-admission of its now-deleted subject.
    // These SQL effects are still rollbackable until the actual native commit.
    let identity =
        transaction_guard::current_http_identity(&completed.ctx, &mut tx, HttpIdentityMode::Read)
            .await?;
    transaction_guard::guard_user(&identity.user)?;
    identity.check_deadline(&ctx.app)?;
    if !completed
        .accepted
        .key_deadline_allows(ctx.app.clock.as_ref())
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the snapshot deletion credential has expired",
        ));
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
