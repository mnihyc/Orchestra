//! Existing group writers and their own current optional Chat projection.
use super::*;
use crate::key_operations::{admission, resolver::Disclosure, targets};
use crate::registry::{OperationFuture, OperationPlan};
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::chats::{ChatChangeReceipt, ChatChangeResponse};
use orch_api_contract::operations::TargetResolver;
use orch_authz::Caller;

const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);
const PATH: TargetResolver = TargetResolver::TypedPath {
    parameter: "id",
    kind: CHAT,
};

async fn target_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    selection: &str,
    destination: Option<Id>,
) -> ApiResult<OperationPlan> {
    let target = targets::chat(conn, ctx.id("id")?).await?;
    let mut plan = OperationPlan::new(ctx.route, selection)?;
    let slots: Vec<_> = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(index, facet)| {
            (facet.target == PATH && facet.resources == [CHAT]).then_some(index)
        })
        .collect();
    if slots.is_empty() {
        return Err(ApiError::internal("the group target is not declared"));
    }
    for slot in slots {
        plan.bind(slot, CHAT, std::slice::from_ref(&target))?;
    }
    let destinations = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(index, facet)| {
            (facet.target == TargetResolver::ResolvedCollection("chat_destination"))
                .then_some(index)
        })
        .collect::<Vec<_>>();
    for index in destinations {
        match destination {
            Some(id) => {
                let target = targets::node(conn, id).await?;
                plan.bind(index, target.object.kind, &[target])?;
            }
            None => plan.bind_empty(index)?,
        }
    }
    // Response sources are truly unknown before the native mutation. Pending
    // remains pending until the same supplied-plan reader materializes them.
    Ok(plan)
}

#[derive(Clone)]
pub(super) enum Effect {
    Rename(chats::PreparedRename),
    Add(chats::PreparedAddMember),
    Remove(chats::PreparedRemoveMember),
    Manager(chats::PreparedManager),
}

/// Native operands from this exact final guarded preparation, never stored authority.
#[derive(Clone)]
pub(super) struct PreparedGroup {
    pub(super) loaded: Loaded,
    pub(super) effect: Effect,
}

enum Request {
    Rename(ChatUpdateBody),
    Add(Id),
    Remove(Id),
    Manager(Id, bool),
}

pub(super) fn update<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ChatUpdateBody,
) -> OperationFuture<'a> {
    prepare(ctx, conn, caller, Request::Rename(body))
}

pub(super) fn add<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move { prepare(ctx, conn, caller, Request::Add(ctx.id("user_id")?)).await })
}

pub(super) fn remove<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move { prepare(ctx, conn, caller, Request::Remove(ctx.id("user_id")?)).await })
}

pub(super) fn manager<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: orch_api_contract::chats::ChatManagerSetBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        prepare(
            ctx,
            conn,
            caller,
            Request::Manager(ctx.id("user_id")?, body.manager),
        )
        .await
    })
}

fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    request: Request,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let result = native_prefix(ctx, conn, caller, request).await;
        operation::project_current_result(ctx, conn, caller, result).await
    })
}

/// Complete the existing effect-free native rules inside the shared guarded producer.
/// Its final synchronous HTTP deadline fence follows every await here.
async fn native_prefix(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    request: Request,
) -> ApiResult<OperationPlan> {
    let selection = match &request {
        Request::Remove(user) if caller.user() == *user => "ChatMemberRemove.leave".to_owned(),
        Request::Remove(_) => "ChatMemberRemove.remove".to_owned(),
        _ => {
            let declaration =
                orch_api_contract::operations::for_route(&ctx.route).map_err(ApiError::internal)?;
            let [variant] = declaration.variants.as_slice() else {
                return Err(ApiError::internal("the group action is not a singleton"));
            };
            variant.operation.clone()
        }
    };
    let destination = match &request {
        Request::Rename(body) if body.home_id.is_some() => {
            let row = rows::chat(conn, ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Some(body.home_id.flatten().unwrap_or(row.org_id))
        }
        _ => None,
    };
    let plan = target_plan(ctx, conn, &selection, destination).await?;
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(ctx),
    };
    let id = ctx.id("id")?;
    let loaded = chats::load(conn, &work.acting(), id, Mode::Write).await;
    let mut loaded = operation::refuse(&work, conn, id, loaded).await?;
    let effect = match request {
        Request::Rename(body) => {
            let prepared = chats::prepare_update_visible(
                conn,
                &work.acting(),
                &mut loaded,
                body.name.as_ref().map(|name| name.as_deref()),
                body.home_id,
                body.visibility,
            )
            .await;
            Effect::Rename(operation::refuse(&work, conn, id, prepared).await?)
        }
        Request::Add(user) => {
            let outsiders = owners_name_outsiders(conn, loaded.row.home()).await?;
            let prepared =
                chats::prepare_add_member(conn, &work.acting(), &mut loaded, user, outsiders, None)
                    .await;
            Effect::Add(operation::refuse(&work, conn, id, prepared).await?)
        }
        Request::Manager(user, manager) => {
            let prepared =
                chats::prepare_manager(conn, &work.acting(), &mut loaded, user, manager).await;
            Effect::Manager(operation::refuse(&work, conn, id, prepared).await?)
        }
        Request::Remove(user) => {
            let prepared =
                chats::prepare_remove_member(conn, &work.acting(), &mut loaded, user).await;
            Effect::Remove(operation::refuse(&work, conn, id, prepared).await?)
        }
    };
    Ok(plan.with_data(PreparedGroup { loaded, effect }))
}

/// Same canonical action guard, after the exact row wait already taken by
/// native ModeWrite. No send/member rule is duplicated or added here.
pub(super) async fn begin(ctx: &Ctx) -> ApiResult<(Ctx, Tx, transaction_guard::Guard)> {
    let (mut tx, _initial) = transaction_guard::begin(ctx).await?;
    rows::lock(&mut tx, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    Ok((current, tx, guard))
}

pub(super) async fn response(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<Option<Chat>> {
    response_at(ctx, conn, principal, false).await
}

pub(super) async fn response_at(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    moved: bool,
) -> ApiResult<Option<Chat>> {
    if ctx.disclosure() == Disclosure::Denied {
        return Ok(None);
    }
    let selection = principal
        .caller()
        .admitted_operation()
        .ok_or_else(|| ApiError::internal("the group writer has no admitted operation"))?;
    let Some(identity) = admission::response_caller(
        ctx,
        conn,
        principal.caller(),
        selection,
        ctx.app.clock.time(),
    )
    .await?
    else {
        return Ok(None);
    };
    let destination = if moved {
        Some(
            rows::chat(conn, ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?
                .home(),
        )
    } else {
        None
    };
    let plan = target_plan(ctx, conn, selection, destination).await?;
    let reader = plan
        .preparation_principal(ctx, conn, &identity.caller)
        .await?;
    let audit = orch_db::audit::Origin::default();
    let act = Acting {
        principal: &reader,
        now: reader.now().get(),
        origin: &audit,
    };
    let loaded = match chats::load(conn, &act, ctx.id("id")?, Mode::Read).await {
        Ok(loaded) if !loaded.standing.inspects() => loaded,
        Ok(_)
        | Err(ChatError::NotFound | ChatError::Lacks { .. } | ChatError::Suspended { .. }) => {
            return Ok(None);
        }
        Err(error) => return Err(ApiError::internal(error)),
    };
    let (plan, prepared) = read_operation::prepare_projection_with_plan(
        conn,
        &reader,
        plan,
        loaded,
        messages::Synced {
            messages: Vec::new(),
            complete: true,
            has_older: false,
        },
        true,
        None,
    )
    .await?;
    plan.read_response(ctx, conn, principal.caller(), move |conn, current| {
        Box::pin(async move {
            let audit = orch_db::audit::Origin::default();
            let act = Acting {
                principal: current,
                now: current.now().get(),
                origin: &audit,
            };
            match chats::load(conn, &act, prepared.loaded.id(), Mode::Read).await {
                Ok(loaded) if !loaded.standing.inspects() => {}
                Ok(_)
                | Err(
                    ChatError::NotFound | ChatError::Lacks { .. } | ChatError::Suspended { .. },
                ) => return Ok(None),
                Err(error) => return Err(ApiError::internal(error)),
            }
            Ok(read_operation::project(conn, current, &prepared)
                .await?
                .chat)
        })
    })
    .await
    .map(Option::flatten)
}

pub(super) fn finish(ctx: &Ctx, id: Id, full: Option<Chat>) -> ChatChangeResponse {
    match (full, ctx.key_receipt_required()) {
        (Some(full), false) => ChatChangeResponse::full(full),
        _ => ChatChangeResponse::Receipt(ChatChangeReceipt {
            projection: ReceiptProjection::Receipt,
            chat_id: id,
        }),
    }
}
