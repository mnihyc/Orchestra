//! Native export plan candidates, scoped before aggregate construction and response rendering.
use super::*;
use crate::key_operations::{native, targets};
use futures::future::BoxFuture;
use orch_api_contract::{
    export_sources::{self as sources, Source},
    exports::ExportPlan,
    operation_sources::Declaration,
};
use orch_authz::Caller;

pub(super) struct Plan;
pub(super) type Ready = orch_engine::exports::ScopedExportPlan;

pub(super) async fn prepare_for(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    request: &ExportRequest,
    opened: Option<&Ready>,
    declaration: &'static Declaration<Source>,
) -> ApiResult<(Ready, Vec<native::Facts<Source>>)> {
    let place = ctx.id("id")?;
    let target = targets::node_identity(conn, place).await?;
    let root = native::Facts::one(Source::Place, target.object.kind, target);
    let plan = native::bind_known(ctx, declaration, std::slice::from_ref(&root))?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    let prepared = match ctx
        .app
        .engine
        .prepare_export_plan(conn, &principal, place, request)
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => return Err(failed(conn, error).await),
    };
    match ctx
        .app
        .engine
        .bound_export_plan(conn, caller, prepared, plan.shared(), declaration, opened)
        .await
    {
        Ok(value) => Ok(value),
        Err(error) => Err(failed(conn, error).await),
    }
}
impl native::Selected for Plan {
    type Source = Source;
    type Query = ((), ExportRequest);
    type Value = Ready;
    type Reply = ExportPlan;
    const DECLARATION: &'static Declaration<Source> = &sources::PREFLIGHT;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare_for(
            ctx,
            conn,
            caller,
            &query.1,
            None,
            &sources::PREFLIGHT,
        ))
    }
    fn refresh<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
        opened: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare_for(
            ctx,
            conn,
            caller,
            &query.1,
            Some(opened),
            &sources::PREFLIGHT,
        ))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
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
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<ExportPlan>> {
        Box::pin(async move {
            match ctx.app.engine.render_export_plan(conn, &value.plan).await {
                Ok(plan) => Ok(plan),
                Err(error) => Err(failed(conn, error).await),
            }
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        old: &'a Ready,
        fresh: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.same_sources(fresh)) })
    }
}
