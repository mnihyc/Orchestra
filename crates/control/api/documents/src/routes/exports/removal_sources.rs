//! The native archive-removal transaction and its accepted exact file cleanup.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects::ApplyOwned;

use crate::key_operations::native::effects::Effect;

use crate::key_operations::native::effects::FirstEffect;

use crate::key_operations::native::effects::Outcome;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::artifact_sources::EXPORT_DELETE;

use orch_api_contract::artifact_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_db::DbError;

use orch_db::Tx;

use orch_engine::exports::PreparedExportRemoval;
pub(super) struct Delete;

impl Effect for Delete {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = PreparedExportRemoval;
    type Output = ();

    fn declaration(_value: &PreparedExportRemoval) -> &'static Declaration<Source> {
        &EXPORT_DELETE
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(PreparedExportRemoval, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = match ctx
                .app
                .engine
                .prepare_export_removal(conn, caller.user(), ctx.id("id")?)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let facts = vec![native::Facts::one(
                Source::Artifact,
                KeyTargetKind::Identity(KeyIdentityKind::Export),
                targets::from_export(conn, value.row()).await?,
            )];
            Ok((value, facts))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a PreparedExportRemoval,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() || value.row().user_id != Some(principal.user())
            {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
}

impl ApplyOwned for Delete {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: Tx,
        _principal: &'a Principal,
        value: &'a PreparedExportRemoval,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<()>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            before.check()?;
            let removed = match ctx
                .app
                .engine
                .apply_export_removal(&mut tx, value, ctx.now)
                .await
            {
                Ok(removed) => removed,
                Err(error) => return Err(failed(&mut tx, error).await),
            };
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            ctx.app.engine.finish_export_removal(removed).await;
            Ok(Outcome::Applied(()))
        })
    }
}
