//! Real import custody and native distinct undo subjects before counts are rendered.

use super::*;

use crate::key_operations::native;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::import_undo_sources::PREFLIGHT;

use orch_api_contract::import_undo_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_engine::imports::undo::Prepared;

use orch_types::Id;

use std::collections::BTreeMap;
pub(super) struct Preview;

pub(super) struct Ready {
    native: Prepared,
    targets: BTreeMap<(KeyTargetKind, Id), KeyResolvedTarget>,
}

pub(super) async fn facts(
    conn: &mut Conn,
    native: &Prepared,
) -> ApiResult<(
    Vec<native::Facts<Source>>,
    BTreeMap<(KeyTargetKind, Id), KeyResolvedTarget>,
)> {
    orch_engine::imports::undo::operation::facts(conn, native)
        .await
        .map_err(source_error)
}

pub(super) fn source_error(error: orch_engine::operation_scope::continuing::Error) -> ApiError {
    use orch_engine::operation_scope::continuing::Error;
    match error {
        Error::Refused => ApiError::not_found(),
        Error::Binding(error) => crate::key_operations::admission::binding_error(error),
        Error::Native(error) => ApiError::engine(error),
    }
}

impl native::Selected for Preview {
    type Source = Source;
    type Query = ();
    type Value = Ready;
    type Reply = ImportUndoPlan;
    const MOVES: native::Moves = native::Moves::Repeat;
    const DECLARATION: &'static Declaration<Source> = &PREFLIGHT;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let native = if native::modern(caller) {
                match ctx
                    .app
                    .engine
                    .prepare_undo(conn, caller.user(), ctx.id("id")?)
                    .await
                {
                    Ok(value) => value,
                    Err(error) => return Err(failed(conn, error).await),
                }
            } else {
                let import = match ctx
                    .app
                    .engine
                    .import_of(conn, caller.user(), ctx.id("id")?)
                    .await
                {
                    Ok(value) => value,
                    Err(error) => return Err(failed(conn, error).await),
                };
                Prepared {
                    import,
                    subjects: Vec::new(),
                }
            };
            let (facts, targets) = if native::modern(caller) {
                facts(conn, &native).await?
            } else {
                (Vec::new(), BTreeMap::new())
            };
            Ok((Ready { native, targets }, facts))
        })
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
        opened: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let import = match ctx
                .app
                .engine
                .import_of(conn, caller.user(), ctx.id("id")?)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            let native = Prepared {
                import,
                subjects: opened.native.subjects.clone(),
            };
            let (facts, targets) = facts(conn, &native).await?;
            Ok((Ready { native, targets }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(super::read_sources::require_own(ctx, conn, principal))
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<ImportUndoPlan>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .plan_undo(conn, principal.user(), value.native.import.id)
                .await
            {
                Ok(value) => Ok(value),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a (),
        opened: &'a Ready,
        fresh: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.targets == fresh.targets
                && super::read_sources::preserves(&opened.native.import, &fresh.native.import)
                && opened.native.import.state == fresh.native.import.state
                && opened.native.import.undo_job_id == fresh.native.import.undo_job_id)
        })
    }
}
