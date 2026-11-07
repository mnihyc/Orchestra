//! Typed native continuation admission over the same prepared prefix/branch producer.
use super::*;
use crate::key_operations::{blob_sources, native, targets};
use futures::future::BoxFuture;
use orch_api_contract::{
    operation_sources::Declaration,
    thread_sources::{self as sources, Source},
};
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_engine::continuations::{admission, recipe};
use std::marker::PhantomData;

pub(super) trait Intent: Send + Sync + 'static {
    type Body: Send + 'static;
    const DECLARATION: &'static Declaration<Source>;
    fn request(ctx: &Ctx, body: Self::Body) -> ApiResult<(String, recipe::Request)>;
}
pub(super) struct Edit;
pub(super) struct Activate;
impl Intent for Edit {
    type Body = ThreadPromptVersionCreateBody;
    const DECLARATION: &'static Declaration<Source> = &sources::PROMPT_CREATE;
    fn request(ctx: &Ctx, body: Self::Body) -> ApiResult<(String, recipe::Request)> {
        Ok(recipe::edit(ctx.id("input_id")?, body))
    }
}
impl Intent for Activate {
    type Body = ThreadContinuationActivateBody;
    const DECLARATION: &'static Declaration<Source> = &sources::CONTINUATION_ACTIVATE;
    fn request(ctx: &Ctx, body: Self::Body) -> ApiResult<(String, recipe::Request)> {
        Ok(recipe::activate(ctx.id("continuation_id")?, body))
    }
}
pub(super) struct Change<I>(PhantomData<I>);
impl<I: Intent> native::effects::Effect for Change<I> {
    type Source = Source;
    type Query = ();
    type Body = I::Body;
    type Value = admission::Prepared;
    type Output = ThreadContinuationAccepted;
    fn declaration(_value: &Self::Value) -> &'static Declaration<Source> {
        I::DECLARATION
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: I::Body,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let thread = ctx.id("id")?;
            let (client, request) = I::request(ctx, body)?;
            let target = targets::node(conn, thread).await?;
            if target.object.kind
                != orch_access_contract::key_scope::KeyTargetKind::Object(
                    orch_access_contract::permissions::ObjectKind::Thread,
                )
            {
                return Err(ApiError::not_found());
            }
            let primary = native::Facts::one(Source::Thread, target.object.kind, target);
            let base = native::bind_known(ctx, I::DECLARATION, std::slice::from_ref(&primary))?;
            let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
            let opening = {
                let mut unit = orch_engine::unit::Unit::new(conn, ctx.now);
                admission::header(&mut unit, &principal, thread, &client, &request).await
            };
            let opening = super::refused_in_result(conn, thread, opening).await?;
            let mut facts = vec![primary];
            let selected = if opening.replay() {
                facts.extend(orch_engine::continuations::operation::byte_facts(&[]));
                Vec::new()
            } else {
                let hashes = request
                    .content
                    .as_deref()
                    .map(orch_chat::following::content_blobs)
                    .unwrap_or_default();
                let prepared =
                    blob_sources::prepare(ctx, conn, caller, &base, &hashes, &[]).await?;
                facts.extend(orch_engine::continuations::operation::byte_facts(
                    &prepared.bindings,
                ));
                prepared.selected
            };
            let plan = native::bind_required(ctx, I::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let result = {
                let mut unit = orch_engine::unit::Unit::new(conn, ctx.now);
                admission::prepare_from_header(
                    &ctx.app.engine,
                    &mut unit,
                    &principal,
                    opening,
                    request,
                )
                .await
            };
            let prepared = super::refused_in_result(conn, thread, result).await?;
            Ok((prepared.with_byte_sources(selected), facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if value.thread() != ctx.id("id")? {
                return Err(ApiError::internal(
                    "the native continuation changed its thread",
                ));
            }
            crate::routes::access::subject::require_view(
                conn,
                principal,
                orch_authz::things::Thing::Node(value.thread()),
                orch_authz::Policy::default(),
            )
            .await
        })
    }
}
impl<I: Intent> native::effects::ApplyOwned for Change<I> {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: native::effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<native::effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            let replay = value.replay();
            let result = ctx
                .app
                .engine
                .apply_prepared_continuation(tx, principal, value.clone(), move || {
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                })
                .await;
            let result = match result {
                Ok(result) => result,
                Err(error) => {
                    return Err(super::admission_refusal(
                        ctx,
                        "continuation_prepared_admission",
                        error,
                    )
                    .await);
                }
            };
            Ok(if replay {
                native::effects::Outcome::Unchanged(result)
            } else {
                native::effects::Outcome::Applied(result)
            })
        })
    }
}
