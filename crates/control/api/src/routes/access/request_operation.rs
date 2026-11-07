//! The existing request/batch producers under their actual objects, with one common card projection.
use super::requests;
use crate::key_operations::{admission, resolver::Disclosure, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan, OptionalOperationFuture};
use crate::routes::approvals;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::access::{
    AccessRequestCreateBody, AccessRequestCreateReply, AccessRequestGetReply, AccessRequestReceipt,
    AccessRequestWriteResponse,
};
use orch_api_contract::chats::ChatJoinRequestBody;
use orch_api_contract::common::{ErrorCode, ReceiptProjection};
use orch_api_contract::operations::TargetResolver;
use orch_authz::requests::Whom;
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn,
    queries::approvals::{self as rows, ApprovalRow},
};
use orch_engine::approvals::access::Raised;
use orch_types::{Id, Time};
use std::collections::BTreeMap;

const APPROVAL: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Approval);
const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);
fn scoped(caller: &Caller) -> bool {
    caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}
fn denied() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the key does not read this request batch",
    )
}
fn slot(plan: &OperationPlan, name: &'static str) -> ApiResult<usize> {
    plan.variant().facets.iter().position(|facet|matches!(facet.target,TargetResolver::Resolved(found)|TargetResolver::ResolvedCollection(found) if found==name))
        .ok_or_else(||ApiError::internal("the request producer role is undeclared"))
}
fn bind(
    plan: &mut OperationPlan,
    name: &'static str,
    kind: KeyTargetKind,
    values: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let index = slot(plan, name)?;
    if values.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, kind, values)
    }
}
fn collection(
    plan: &mut OperationPlan,
    name: &'static str,
    groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
) -> ApiResult<()> {
    let index = slot(plan, name)?;
    if groups.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind_collection(index, &groups.into_iter().collect::<Vec<_>>())
    }
}
async fn governors(conn: &mut Conn, rows: &[ApprovalRow]) -> ApiResult<Vec<KeyResolvedTarget>> {
    let mut found = Vec::with_capacity(rows.len());
    for row in rows {
        found.push(approvals::card_governor(conn, row).await?);
    }
    Ok(found)
}
async fn batch(conn: &mut Conn, id: Id, requester: Id) -> ApiResult<Vec<ApprovalRow>> {
    let found = rows::access::batch(conn, id).await?;
    if found.is_empty()
        || found
            .iter()
            .any(|row| row.requester_user_id != Some(requester))
    {
        return Err(ApiError::not_found());
    }
    Ok(found)
}

pub(super) fn create<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: AccessRequestCreateBody,
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let asks = requests::read_body(&body, caller.user())?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        let mut requested = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
        let mut work = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
        let mut cards = Vec::with_capacity(asks.len());
        for ask in &asks {
            let target = targets::page_object(conn, ask.object).await?;
            requested
                .entry(KeyTargetKind::Object(ask.object.kind))
                .or_default()
                .push(target.clone());
            cards.push(target);
            if let Whom::Work { id } = ask.whom {
                let target = targets::node(conn, id).await?;
                work.entry(target.object.kind).or_default().push(target);
            }
        }
        collection(&mut plan, "requested_objects", requested)?;
        collection(&mut plan, "request_work_holders", work)?;
        // New card IDs do not exist yet; their native governance is already these exact objects.
        bind(&mut plan, "request_batch", APPROVAL, &cards)?;
        Ok(Some(plan))
    })
}
pub(crate) fn join<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    _body: ChatJoinRequestBody,
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let target = targets::chat_identity(conn, ctx.id("id")?).await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        bind(
            &mut plan,
            "requested_chat",
            CHAT,
            std::slice::from_ref(&target),
        )?;
        bind(&mut plan, "request_batch", APPROVAL, &[target])?;
        Ok(Some(plan))
    })
}
pub(super) fn get<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let found = batch(conn, ctx.id("batch_id")?, caller.user()).await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        bind(
            &mut plan,
            "request_batch",
            APPROVAL,
            &governors(conn, &found).await?,
        )?;
        Ok(Some(plan))
    })
}

/// The original/current real HTTP identity and this operation's exact batch Read root.
/// The resulting principal is used only by the ordinary projection, never by a writer.
async fn reader(
    ctx: &Ctx,
    conn: &mut Conn,
    original: &Caller,
    plan: &OperationPlan,
    governors: &[KeyResolvedTarget],
) -> ApiResult<
    Option<(
        Ctx,
        Principal,
        crate::middleware::transaction_guard::CurrentHttpIdentity,
    )>,
> {
    let Some(seed) = approvals::card_read_caller(original, plan, "request_batch", governors)?
    else {
        ctx.complete_disclosure(Disclosure::Denied);
        return Ok(None);
    };
    let Some(identity) = admission::response_caller(
        ctx,
        conn,
        &seed,
        &plan.variant().operation,
        Time::new(ctx.app.clock.now()),
    )
    .await?
    else {
        return Ok(None);
    };
    let current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
    let Some(caller) =
        approvals::card_read_caller(&identity.caller, plan, "request_batch", governors)?
    else {
        ctx.complete_disclosure(Disclosure::Denied);
        return Ok(None);
    };
    let principal = authorization::load_principal(conn, caller, identity.now).await?;
    let principal = crate::sign_in::second_step::hold(conn, &identity.user, principal).await?;
    let current = current.with_admitted_caller(principal.caller().clone())?;
    Ok(Some((current, principal, identity)))
}

/// Native browser/legacy projection keeps its existing card doors, without adding
/// foreign source/liveness gates merely to enforce a raw HTTP credential deadline.
async fn native_reader(
    ctx: &Ctx,
    conn: &mut Conn,
    accepted: &Caller,
) -> ApiResult<
    Option<(
        Ctx,
        Principal,
        crate::middleware::transaction_guard::CurrentHttpIdentity,
    )>,
> {
    let mut identity = crate::middleware::transaction_guard::current_http_identity(
        ctx,
        conn,
        crate::middleware::transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.caller = identity
        .caller
        .intersect_restrictions(accepted)
        .map_err(ApiError::internal)?;
    identity.user.as_admin = identity.caller.as_admin();
    if scoped(&identity.caller) {
        return Ok(None);
    }
    let current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
    let principal =
        authorization::admitted_principal(conn, &identity.caller, &identity.user, identity.now)
            .await?;
    Ok(Some((current, principal, identity)))
}

async fn native_created_projection(
    ctx: Ctx,
    accepted: Caller,
    id: Id,
) -> ApiResult<Option<AccessRequestCreateReply>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let Some((ctx, principal, identity)) = native_reader(&ctx, &mut tx, &accepted).await? else {
        return Ok(None);
    };
    let result = async {
        let found = batch(&mut tx, id, principal.user()).await?;
        let approvals =
            approvals::views(&mut tx, &principal, &found, ctx.now, &ctx.app.engine).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Some(AccessRequestCreateReply {
            batch_id: id,
            approvals,
        }))
    }
    .await;
    identity.check_deadline(&ctx.app)?;
    result
}

async fn created_projection(
    ctx: Ctx,
    original: Caller,
    id: Id,
) -> ApiResult<Option<AccessRequestCreateReply>> {
    if !scoped(&original) {
        return native_created_projection(ctx, original, id).await;
    }
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let found = batch(&mut tx, id, original.user()).await?;
    let selected = governors(&mut tx, &found).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    bind(&mut plan, "request_batch", APPROVAL, &selected)?;
    let Some((ctx, principal, identity)) =
        reader(&ctx, &mut tx, &original, &plan, &selected).await?
    else {
        return Ok(None);
    };
    let result = async {
        let cards = approvals::views(&mut tx, &principal, &found, ctx.now, &ctx.app.engine).await?;
        approvals::bind_card_sources(&mut tx, &mut plan, &cards).await?;
        let value = AccessRequestCreateReply {
            batch_id: id,
            approvals: cards,
        };
        let shown = plan
            .read_response(&ctx, &mut tx, principal.caller(), move |_, _| {
                Box::pin(async move { Ok(value) })
            })
            .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(shown)
    }
    .await;
    identity.check_deadline(&ctx.app)?;
    result
}
/// Only existing content is optional. Newly accepted IDs and the original successful outcome survive read loss.
pub(crate) async fn created_reply(
    ctx: &Ctx,
    accepted: &Caller,
    raised: Raised,
) -> ApiResult<AccessRequestWriteResponse> {
    let receipt = AccessRequestReceipt {
        projection: ReceiptProjection::Receipt,
        batch_id: raised.batch_id,
        approval_ids: raised.approvals.iter().map(|row| row.id).collect(),
    };
    if ctx.disclosure() == Disclosure::Denied {
        return Ok(AccessRequestWriteResponse::Receipt(receipt));
    }
    let full = crate::key_operations::completed_projection::run(
        ctx,
        created_projection(ctx.clone(), accepted.clone(), raised.batch_id),
    )
    .await;
    match full {
        Ok(Some(full)) => Ok(AccessRequestWriteResponse::full(full)),
        Ok(None) | Err(_) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(AccessRequestWriteResponse::Receipt(receipt))
        }
    }
}

pub(super) async fn get_reply(ctx: Ctx) -> ApiResult<AccessRequestGetReply> {
    crate::key_operations::snapshot_read::run(ctx, |ctx| async move {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let principal = ctx.operation_principal(&mut tx).await?;
        if !scoped(principal.caller()) {
            let Some((ctx, principal, identity)) =
                native_reader(&ctx, &mut tx, principal.caller()).await?
            else {
                return Err(denied());
            };
            let result = async {
                let found = batch(&mut tx, ctx.id("batch_id")?, principal.user()).await?;
                let approvals =
                    approvals::views(&mut tx, &principal, &found, ctx.now, &ctx.app.engine).await?;
                let complete = ctx
                    .app
                    .engine
                    .access_batch_is_held_in(
                        &mut tx,
                        principal.user(),
                        ctx.id("batch_id")?,
                        Time::new(ctx.now),
                    )
                    .await
                    .map_err(ApiError::engine)?;
                let value = AccessRequestGetReply {
                    batch_id: ctx.id("batch_id")?,
                    approvals,
                    complete,
                };
                tx.commit().await.map_err(orch_db::DbError::from)?;
                Ok(value)
            }
            .await;
            identity.check_deadline(&ctx.app)?;
            return result;
        }
        let found = batch(&mut tx, ctx.id("batch_id")?, principal.user()).await?;
        let selected = governors(&mut tx, &found).await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        bind(&mut plan, "request_batch", APPROVAL, &selected)?;
        let Some((ctx, principal, identity)) =
            reader(&ctx, &mut tx, principal.caller(), &plan, &selected).await?
        else {
            return Err(denied());
        };
        let result = async {
            let cards =
                approvals::views(&mut tx, &principal, &found, ctx.now, &ctx.app.engine).await?;
            let complete = ctx
                .app
                .engine
                .access_batch_is_held_in(
                    &mut tx,
                    principal.user(),
                    ctx.id("batch_id")?,
                    Time::new(ctx.now),
                )
                .await
                .map_err(ApiError::engine)?;
            approvals::bind_card_sources(&mut tx, &mut plan, &cards).await?;
            let value = AccessRequestGetReply {
                batch_id: ctx.id("batch_id")?,
                approvals: cards,
                complete,
            };
            let shown = plan
                .read_response(&ctx, &mut tx, principal.caller(), move |_, _| {
                    Box::pin(async move { Ok(value) })
                })
                .await?
                .ok_or_else(denied)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok(shown)
        }
        .await;
        identity.check_deadline(&ctx.app)?;
        result
    })
    .await
}
