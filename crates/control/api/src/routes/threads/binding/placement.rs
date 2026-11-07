//! Existing source and destination environments, native first mutation, optional Thread content.

use super::*;
use crate::key_operations::native::effects::{self, Apply, Effect, FirstEffect, Outcome};
use crate::key_operations::native::reply::{self, Projection};
use crate::key_operations::{native, publication, targets};
use crate::routes::nodes::{content_sources, read_sources};
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::binding::{ThreadPlacementReceipt, ThreadPlacementResponse};
use orch_api_contract::binding_sources::{self as sources, Source};
use orch_api_contract::nodes::Node;
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Caller;
use orch_engine::binding::PreparedPlacement;
use std::collections::{BTreeMap, BTreeSet};

const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);
const ENV: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

pub(super) struct PlaceThread;
pub(super) struct Accepted {
    receipt: ThreadPlacementReceipt,
    from: Option<Id>,
}

async fn facts(
    conn: &mut Conn,
    thread: Id,
    from: Option<Id>,
    to: Id,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = targets::node(conn, thread).await?;
    if target.object.kind != THREAD {
        return Err(ApiError::not_found());
    }
    let mut environments = Vec::new();
    let ids = from.into_iter().chain([to]).collect::<BTreeSet<_>>();
    for id in ids {
        environments.push(
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?,
        );
    }
    Ok(vec![
        native::Facts::one(Source::Thread, THREAD, target),
        native::Facts::collection(Source::PlacementEnvironments, vec![(ENV, environments)]),
    ])
}

impl Effect for PlaceThread {
    type Source = Source;
    type Query = ();
    type Body = ThreadEnvironmentSetBody;
    type Value = PreparedPlacement;
    type Output = Accepted;

    fn declaration(_value: &PreparedPlacement) -> &'static Declaration<Source> {
        &sources::PLACEMENT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadEnvironmentSetBody,
    ) -> BoxFuture<'a, ApiResult<(PreparedPlacement, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let thread = ctx.id("id")?;
            let environment = match body.choice {
                EnvironmentChoice::Use(id) => id,
                EnvironmentChoice::New(_) => {
                    return Err(invalid(
                        "new",
                        "an environment is made by its own request, and named here with `use`",
                    ));
                }
            };
            let row = thread_rows::thread(conn, thread)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let known_facts = facts(conn, thread, row.environment_id, environment).await?;
            let known = native::bind_required(ctx, &sources::PLACEMENT, &known_facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let value = match ctx
                .app
                .engine
                .preparing_placement(
                    conn,
                    &principal,
                    thread,
                    &Replace {
                        environment,
                        directory: body.directory,
                    },
                )
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(rebind_failed(conn, thread, error).await),
            };
            // The preliminary row only primes effect-free ordinary preparation. Bind
            // the native decision's actual operands, even if that row changed meanwhile.
            let facts = facts(
                conn,
                value.thread().node_id,
                value.thread().environment_id,
                value.environment().id,
            )
            .await?;
            Ok((value, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a PreparedPlacement,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .require_placement(conn, principal, value)
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(rebind_failed(conn, value.thread().node_id, error).await),
            }
        })
    }
}
impl Apply for PlaceThread {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a PreparedPlacement,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Accepted>>> {
        Box::pin(async move {
            let result = match ctx
                .app
                .engine
                .write_placement_before(conn, value, || {
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                })
                .await
            {
                Ok(result) => result,
                Err(error) => return Err(rebind_failed(conn, value.thread().node_id, error).await),
            };
            Ok(Outcome::Applied(Accepted {
                receipt: ThreadPlacementReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    thread_id: result.thread,
                    environment_id: value.environment().id,
                    seq: result.seq,
                },
                from: value.thread().environment_id,
            }))
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _body: ThreadEnvironmentSetBody,
) -> ApiResult<ThreadPlacementResponse> {
    let result = change(&ctx).await;
    result.map_err(|error| {
        if ctx.key_receipt_required() {
            error.without_content()
        } else {
            error
        }
    })
}
async fn change(ctx: &Ctx) -> ApiResult<ThreadPlacementResponse> {
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    initial.check_deadline(ctx)?;
    thread_rows::lock(&mut tx, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let completed = effects::apply::<PlaceThread>(ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    completed
        .ctx
        .app
        .engine
        .wake(completed.output.receipt.thread_id);
    publication::receipt(&completed.ctx, &completed.output.receipt)?;
    let receipt = completed.output.receipt.clone();
    match reply::render::<Placed>(&completed.ctx, &completed.accepted, completed.output).await? {
        Some(value) => Ok(ThreadPlacementResponse::Full(Box::new(value))),
        None => Ok(ThreadPlacementResponse::Receipt(receipt)),
    }
}

struct Placed;
impl Projection for Placed {
    type Source = Source;
    type Input = Accepted;
    type Subject = thread_rows::ThreadRow;
    type Value = content_sources::Prepared;
    type Reply = Thread;
    const READ_SOURCE: Source = Source::Thread;
    fn declaration(_input: &Accepted) -> &'static Declaration<Source> {
        &sources::PLACEMENT
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(thread_rows::ThreadRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = thread_rows::thread(conn, input.receipt.thread_id)
                .await?
                .filter(|row| {
                    row.deleted_at.is_none()
                        && row.environment_id == Some(input.receipt.environment_id)
                })
                .ok_or_else(ApiError::not_found)?;
            let facts = facts(conn, row.node_id, input.from, input.receipt.environment_id).await?;
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Accepted,
        subject: &'a thread_rows::ThreadRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if native::modern(principal.caller()) {
                authorization::require(
                    conn,
                    principal,
                    subject.node_id,
                    authorization::VIEW,
                    Act::Use,
                    Policy::default(),
                )
                .await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Accepted,
        subject: &'a thread_rows::ThreadRow,
    ) -> BoxFuture<'a, ApiResult<(content_sources::Prepared, Vec<native::Facts<Source>>)>> {
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
        _input: &'a Accepted,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Thread>> {
        Box::pin(async move {
            match content_sources::render(
                ctx,
                conn,
                principal,
                prepared.subject.node_id,
                &prepared.value,
            )
            .await?
            {
                Node::Thread(thread) => Ok(thread),
                _ => Err(ApiError::not_found()),
            }
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.node_id == fresh.subject.node_id
                && old.subject.environment_id == fresh.subject.environment_id
                && old.subject.agent_id == fresh.subject.agent_id
                && old.subject.provider_id == fresh.subject.provider_id
                && old.subject.workspace_dir == fresh.subject.workspace_dir
                && old.value.preserves(&fresh.value))
        })
    }
}
