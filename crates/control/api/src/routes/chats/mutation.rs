//! Exact scope facts for five existing chat-message mutations. Their ordinary writers
//! still own send/moderator, chat-kind, message-state, no-op and lock decisions.
use crate::key_operations::targets;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyResolvedTarget, KeyTargetKind,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::{Caller, CapturedCaller};
use orch_db::Conn;
use orch_db::queries::chats::{self as rows, MessageRow};

const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);
const CHAT_PATH: TargetResolver = TargetResolver::TypedPath {
    parameter: "id",
    kind: CHAT,
};

fn slot(plan: &OperationPlan, usage: FacetUsage, action: KeyAction) -> ApiResult<usize> {
    let mut found = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| {
            facet.usage == usage
                && facet.action == action
                && facet.target == CHAT_PATH
                && facet.resources == [CHAT]
        });
    let (index, _) = found
        .next()
        .ok_or_else(|| ApiError::internal("the chat mutation target is not declared"))?;
    if found.next().is_some() {
        return Err(ApiError::internal("the chat mutation target is ambiguous"));
    }
    Ok(index)
}

/// Fresh primary and Chat Read facts. Live-name slots intentionally remain unbound
/// here so the response producer can bind its actual sources once, on separate clones.
fn target_plan(
    mut plan: OperationPlan,
    action: KeyAction,
    target: &KeyResolvedTarget,
) -> ApiResult<OperationPlan> {
    // The known shape is one actual Chat effect, one Chat response and five live-name
    // response collections. This is metadata validation, not an admission-size limit.
    if !matches!(action, KeyAction::Write | KeyAction::Manage)
        || plan.variant().facets.len() != 7
        || plan
            .variant()
            .facets
            .iter()
            .filter(|facet| facet.usage == FacetUsage::Required)
            .count()
            != 1
        || plan
            .variant()
            .facets
            .iter()
            .filter(|facet| {
                facet.usage == FacetUsage::ResponseContent && facet.action == KeyAction::Read
            })
            .count()
            != 6
    {
        return Err(ApiError::internal(
            "the chat mutation footprint is not declared",
        ));
    }
    let write = slot(&plan, FacetUsage::Required, action)?;
    let read = slot(&plan, FacetUsage::ResponseContent, KeyAction::Read)?;
    plan.bind(write, CHAT, std::slice::from_ref(target))?;
    plan.bind(read, CHAT, std::slice::from_ref(target))?;
    // Validate the same five live-name roles without binding the real response
    // plan: the common projector alone supplies its current actual name sources.
    super::operation::bind_names(&mut plan.clone(), &[])?;
    Ok(plan)
}

async fn current_caller(ctx: &Ctx, conn: &mut Conn, caller: &Caller) -> ApiResult<Caller> {
    CapturedCaller::capture(caller)
        .restore(conn, orch_types::Time::new(ctx.app.clock.now()))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the credential is no longer valid",
            )
        })
}

/// The ordinary read gate is already the first decision of every writer. Repeat it
/// without its write lock, and choose bounded errors before any rich name construction.
/// Neither send, moderation, message kind nor no-op policy is decided here.
async fn ordinary_read(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    chat: orch_types::Id,
) -> ApiResult<()> {
    let caller = current_caller(ctx, conn, caller).await?;
    let principal = plan.preparation_principal(ctx, conn, &caller).await?;
    let origin = super::origin(ctx);
    let acting = orch_chat::Acting {
        principal: &principal,
        now: principal.now().get(),
        origin: &origin,
    };
    match orch_chat::chats::load(conn, &acting, chat, orch_chat::chats::Mode::Read).await {
        Ok(_) => Ok(()),
        Err(error) => {
            // Target/ordinary reads may have waited while a legacy token became modern
            // or was narrowed. Keep every earlier ceiling and use its actual current mode.
            let caller = current_caller(ctx, conn, principal.caller()).await?;
            let current = plan.preparation_principal(ctx, conn, &caller).await?;
            let bounded = current
                .caller()
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
                || ctx.key_receipt_required();
            Err(super::failed_projected(conn, &current, Some(chat), error, bounded).await)
        }
    }
}

/// With no stored author there is no final Delete variant. One actually permitted
/// declared candidate may prepare only the ordinary visibility decision before 404.
/// It never leaves this error path as selection, operation data or effect admission.
async fn missing_message(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    target: &KeyResolvedTarget,
    chat: orch_types::Id,
) -> ApiResult<OperationPlan> {
    let caller = current_caller(ctx, conn, caller).await?;
    for (selection, action) in [
        ("ChatMessageDelete.own", KeyAction::Write),
        ("ChatMessageDelete.moderate", KeyAction::Manage),
    ] {
        let mut candidate = target_plan(OperationPlan::new(ctx.route, selection)?, action, target)?;
        if !orch_engine::operation_scope::compatible_selection(&caller, candidate.variant()) {
            continue;
        }
        let primary = slot(&candidate, FacetUsage::Required, action)?;
        if !candidate.permits_binding(&caller, primary, CHAT, std::slice::from_ref(target))? {
            continue;
        }
        super::operation::bind_names(&mut candidate, &[])?;
        ordinary_read(ctx, conn, &caller, &candidate, chat).await?;
        return Err(ApiError::not_found());
    }
    Err(ApiError::new(
        ErrorCode::AuthzDenied,
        "the key does not allow this operation",
    ))
}

pub(super) fn direct<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let declaration =
            orch_api_contract::operations::for_route(&ctx.route).map_err(ApiError::internal)?;
        let [variant] = declaration.variants.as_slice() else {
            return Err(ApiError::internal(
                "the direct chat mutation is not a singleton",
            ));
        };
        // The existing ordinary chat/site builder permits archived identities and
        // leaves availability and actual send rights to the unchanged writer.
        let target = targets::chat(conn, ctx.id("id")?).await?;
        let mut plan = target_plan(
            OperationPlan::new(ctx.route, &variant.operation)?,
            KeyAction::Write,
            &target,
        )?;
        super::operation::bind_names(&mut plan, &[])?;
        ordinary_read(ctx, conn, caller, &plan, ctx.id("id")?).await?;
        Ok(plan)
    })
}

pub(super) fn delete<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        if ctx.route.name != "ChatMessageDelete" {
            return Err(ApiError::internal("this is not a chat-message deletion"));
        }
        let chat = ctx.id("id")?;
        let target = targets::chat(conn, chat).await?;
        // This existing query includes the actual chat/message association. Authorship
        // selects only the catalogue branch, never ordinary moderation/send authority.
        let Some(row) = rows::message(conn, chat, ctx.id("message_id")?).await? else {
            return missing_message(ctx, conn, caller, &target, chat).await;
        };
        let (selection, action) = if row.author_id == Some(caller.user()) {
            ("ChatMessageDelete.own", KeyAction::Write)
        } else {
            ("ChatMessageDelete.moderate", KeyAction::Manage)
        };
        let mut plan = target_plan(OperationPlan::new(ctx.route, selection)?, action, &target)?;
        super::operation::bind_names(&mut plan, &[])?;
        ordinary_read(ctx, conn, caller, &plan, chat).await?;
        Ok(plan)
    })
}

/// Preserve the exact operation already selected on the guarded caller. Deletion
/// cannot turn into another variant merely because its accepted row was changed.
pub(super) async fn response_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Caller,
    row: &MessageRow,
) -> ApiResult<OperationPlan> {
    if row.chat_id != ctx.id("id")? || row.id != ctx.id("message_id")? {
        return Err(ApiError::not_found());
    }
    let selection = current.admitted_operation().ok_or_else(|| {
        ApiError::internal("the chat mutation response has no admitted selection")
    })?;
    let plan = OperationPlan::new(ctx.route, selection)?;
    let primary = plan
        .variant()
        .facets
        .iter()
        .find(|facet| facet.usage == FacetUsage::Required)
        .ok_or_else(|| ApiError::internal("the admitted chat mutation has no effect"))?;
    let action = primary.action;
    let target = targets::chat(conn, row.chat_id).await?;
    target_plan(plan, action, &target)
}
