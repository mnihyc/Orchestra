//! Native side preparation and publication through the shared effect and projection adapters.
use super::*;
use crate::key_operations::native::{effects, reply};
use crate::key_operations::{blob_sources, native, publication, targets};
use crate::routes::nodes::{content_sources, read_sources};
use futures::future::BoxFuture;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyTargetKind};
use orch_api_contract::{
    operation_sources::Declaration,
    thread_sources::{self as sources, Source},
    threads::{
        ThreadSideCreateBody, ThreadSideCreateReceipt, ThreadSideCreateReply,
        ThreadSideCreateResponse,
    },
};
use orch_authz::Caller;
use orch_engine::side_threads::{PreparedSide, Started};
use std::collections::BTreeMap;

const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);
async fn parent(conn: &mut Conn, id: Id) -> ApiResult<native::Facts<Source>> {
    let target = targets::node(conn, id).await?;
    if target.object.kind != THREAD {
        return Err(ApiError::not_found());
    }
    Ok(native::Facts::one(Source::Thread, THREAD, target))
}
pub(super) struct Create;
impl effects::Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = ThreadSideCreateBody;
    type Value = PreparedSide;
    type Output = Started;
    fn declaration(_: &PreparedSide) -> &'static Declaration<Source> {
        &sources::SIDE_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadSideCreateBody,
    ) -> BoxFuture<'a, ApiResult<(PreparedSide, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let mut facts = vec![parent(conn, id).await?];
            let known = native::bind_known(ctx, &sources::SIDE_CREATE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let value = {
                let mut unit = orch_engine::unit::Unit::new(conn, ctx.now);
                ctx.app
                    .engine
                    .prepare_side_in(
                        &mut unit,
                        principal.caller(),
                        id,
                        body.content,
                        body.client_id,
                        body.to_turn,
                    )
                    .await
            };
            let value = match value {
                Ok(value) => value,
                Err(error) => return Err(super::side_failed(conn, id, error).await),
            };
            if value.replay() {
                facts.extend(orch_engine::continuations::operation::byte_facts(&[]));
            } else {
                let hashes = orch_chat::following::content_blobs(value.content());
                let bytes = blob_sources::prepare(ctx, conn, caller, &known, &hashes, &[]).await?;
                facts.extend(orch_engine::continuations::operation::byte_facts(
                    &bytes.bindings,
                ));
            }
            Ok((value, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a PreparedSide,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require(
                conn,
                principal,
                value.parent(),
                authorization::exact(Permission::ThreadInteract),
                Act::Use,
                Policy::default(),
            )
            .await
        })
    }
}
impl effects::ApplyOwned for Create {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a PreparedSide,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Started>>> {
        Box::pin(async move {
            let replay = value.replay();
            let deadline = before.deadline();
            let result = ctx
                .app
                .engine
                .apply_prepared_side(
                    tx,
                    principal,
                    value.clone(),
                    move || {
                        before.check().map_err(|error| {
                            orch_engine::EngineError::BeforeEffect(Box::new(error))
                        })
                    },
                    move || {
                        deadline.check().map_err(|error| {
                            orch_engine::EngineError::BeforeEffect(Box::new(error))
                        })
                    },
                )
                .await;
            let output = match result {
                Ok(value) => value,
                Err(error) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    return Err(super::side_failed(&mut conn, value.parent(), error).await);
                }
            };
            Ok(if replay {
                effects::Outcome::Unchanged(output)
            } else {
                effects::Outcome::Applied(output)
            })
        })
    }
}
pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _body: ThreadSideCreateBody,
) -> ApiResult<ThreadSideCreateResponse> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply_owned::<Create>(&ctx, tx).await?;
    let receipt = ThreadSideCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        parent_id: ctx.id("id")?,
        thread_id: done.output.thread,
        input_id: done.output.submitted.input.id,
        seq: done.output.submitted.seq,
    };
    publication::receipt(&done.ctx, &receipt)?;
    match reply::render::<Readback>(&done.ctx, &done.accepted, done.output).await? {
        Some(value) => Ok(ThreadSideCreateResponse::Full(Box::new(value))),
        None => Ok(ThreadSideCreateResponse::Receipt(receipt)),
    }
}
struct Readback;
impl reply::Projection for Readback {
    type Source = Source;
    type Input = Started;
    type Subject = thread_rows::ThreadRow;
    type Value = content_sources::Prepared;
    type Reply = ThreadSideCreateReply;
    const READ_SOURCE: Source = Source::CreatedThread;
    fn declaration(_: &Started) -> &'static Declaration<Source> {
        &sources::SIDE_CREATE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Started,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let parent_id = ctx.id("id")?;
            let row = thread_rows::thread(conn, input.thread)
                .await?
                .filter(|row| row.deleted_at.is_none() && row.side_of == Some(parent_id))
                .ok_or_else(ApiError::not_found)?;
            let mut facts = vec![parent(conn, row.side_of.ok_or_else(ApiError::not_found)?).await?];
            facts.extend(orch_engine::continuations::operation::byte_facts(&[]));
            facts.push(native::Facts::one(
                Source::CreatedThread,
                THREAD,
                targets::node(conn, row.node_id).await?,
            ));
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Started,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require(
                conn,
                principal,
                subject.node_id,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Started,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = content_sources::prepare(conn, principal, subject.node_id).await?;
            let identities = read_sources::Sources::from_objects(
                value
                    .objects
                    .iter()
                    .map(|&(kind, id)| ObjectRef { kind, id }),
            )
            .identity_targets(conn)
            .await?;
            let mut groups = BTreeMap::<KeyTargetKind, Vec<_>>::new();
            for &(kind, id) in &value.objects {
                groups.entry(KeyTargetKind::Object(kind)).or_default().push(
                    identities
                        .get(&ObjectRef { kind, id })
                        .cloned()
                        .ok_or_else(ApiError::not_found)?,
                );
            }
            Ok((
                value,
                vec![native::Facts::collection(
                    Source::ThreadContent,
                    groups.into_iter().collect(),
                )],
            ))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Started,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ThreadSideCreateReply>> {
        Box::pin(async move {
            let thread = match content_sources::render(
                ctx,
                conn,
                principal,
                prepared.subject.node_id,
                &prepared.value,
            )
            .await?
            {
                orch_api_contract::nodes::Node::Thread(thread) => thread,
                _ => return Err(ApiError::not_found()),
            };
            Ok(ThreadSideCreateReply {
                thread,
                input: input.submitted.input.clone(),
                seq: input.submitted.seq,
            })
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Started,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.node_id == fresh.subject.node_id
                && old.subject.side_of == fresh.subject.side_of
                && old.subject.current_segment_id == fresh.subject.current_segment_id
                && old.value.preserves(&fresh.value))
        })
    }
}
