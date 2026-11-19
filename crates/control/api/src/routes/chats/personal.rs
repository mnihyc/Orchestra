//! Exact current-Chat authority for the caller's own state and typed draft.
//! The native writer keeps the distinct read/send gates and all custody effects.

use super::operation::{prepare_work, project_current_result, refuse};
use super::*;
use crate::key_operations::{blob_sources, targets};
use crate::registry::{OperationFuture, OperationPlan};
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::operations::TargetResolver;
use orch_authz::Caller;
use orch_chat::following::BlobOrigin;
use orch_types::Hash;

const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);
const PATH: TargetResolver = TargetResolver::TypedPath {
    parameter: "id",
    kind: CHAT,
};

pub(super) struct PreparedDraft {
    pub(super) draft: Option<ChatDraft>,
    pub(super) names: names::AllowSet,
}

pub(super) struct DraftEffects {
    pub(super) blobs: Vec<(Hash, BlobOrigin)>,
}

/// The actual Chat is shared by these personal operations. The account whose row
/// changes is always the current principal, never a request operand or another caller.
async fn primary(ctx: &Ctx, conn: &mut Conn) -> ApiResult<OperationPlan> {
    let target = targets::chat(conn, ctx.id("id")?).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
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
        return Err(ApiError::internal(
            "the personal Chat target is not declared",
        ));
    }
    for slot in slots {
        plan.bind(slot, CHAT, std::slice::from_ref(&target))?;
    }
    Ok(plan)
}

/// Bind only the sources retained by the existing compact draft-name producer.
/// Authored parts and stored text are never rewritten as rendered message text.
async fn draft_names_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    plan: &mut OperationPlan,
    draft: &ChatDraft,
) -> ApiResult<names::AllowSet> {
    let candidates = names::candidate_sources(conn, &[], &draft.parts, None).await?;
    let permitted = operation::allowed_names(conn, principal.caller(), plan, candidates).await?;
    let allowed: names::AllowSet = permitted.iter().map(|(source, _)| *source).collect();
    let mut preparation = plan.clone();
    operation::bind_names(&mut preparation, &permitted)?;
    let reader = preparation
        .preparation_principal(ctx, conn, principal.caller())
        .await?;
    let mut projected = draft.clone();
    let actual =
        mentions::draft_names_with_sources(conn, &reader, &mut projected, Some(&allowed)).await?;
    let retained: Vec<_> = permitted
        .into_iter()
        .filter(|(source, _)| actual.contains(source))
        .collect();
    operation::bind_names(plan, &retained)?;
    Ok(actual)
}

pub(super) fn get<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let result = prepare_get(ctx, conn, caller).await;
        project_current_result(ctx, conn, caller, result).await
    })
}

async fn prepare_get(ctx: &Ctx, conn: &mut Conn, caller: &Caller) -> ApiResult<OperationPlan> {
    let mut plan = primary(ctx, conn).await?;
    let work = prepare_work(ctx, conn, caller, &plan).await?;
    let chat = ctx.id("id")?;
    let result = chats::load(conn, &work.acting(), chat, Mode::Read).await;
    let loaded = refuse(&work, conn, chat, result).await?;
    if loaded.standing.inspects() {
        return refuse(&work, conn, chat, Err(ChatError::Inspecting)).await;
    }
    let result = following::draft(conn, &work.acting(), chat).await;
    let draft = refuse(&work, conn, chat, result).await?;
    let allowed = if let Some(draft) = &draft {
        draft_names_plan(ctx, conn, &work.principal, &mut plan, draft).await?
    } else {
        operation::bind_names(&mut plan, &[])?;
        names::AllowSet::new()
    };
    Ok(plan.with_data(PreparedDraft {
        draft,
        names: allowed,
    }))
}

pub(super) fn set<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ChatDraftSetBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let result = prepare_set(ctx, conn, caller, body).await;
        project_current_result(ctx, conn, caller, result).await
    })
}

async fn prepare_set(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: ChatDraftSetBody,
) -> ApiResult<OperationPlan> {
    let mut plan = primary(ctx, conn).await?;
    operation::bind_names(&mut plan, &[])?;
    let work = prepare_work(ctx, conn, caller, &plan).await?;
    let chat = ctx.id("id")?;
    let result = chats::load(conn, &work.acting(), chat, Mode::Read).await;
    let loaded = refuse(&work, conn, chat, result).await?;
    let checked = following::check_draft(conn, &loaded, &body.parts, body.reply_to).await;
    refuse(&work, conn, chat, checked).await?;
    let hashes = orch_chat::attachments::blobs_in(&body.parts);
    let blobs =
        blob_sources::prepare(ctx, conn, work.principal.caller(), &plan, &hashes, &[]).await?;
    blob_sources::bind(&mut plan, &blobs.bindings)?;
    Ok(plan.with_data(DraftEffects {
        blobs: blobs.selected,
    }))
}

async fn readable(conn: &mut Conn, principal: &Principal, chat: Id) -> ApiResult<bool> {
    let audit = orch_db::audit::Origin::default();
    let act = Acting {
        principal,
        now: principal.now().get(),
        origin: &audit,
    };
    match chats::load(conn, &act, chat, Mode::Read).await {
        Ok(loaded) => Ok(!loaded.standing.inspects()),
        Err(ChatError::NotFound | ChatError::Lacks { .. } | ChatError::Suspended { .. }) => {
            Ok(false)
        }
        Err(error) => Err(ApiError::internal(error)),
    }
}

pub(super) async fn state_response(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    state: ChatState,
) -> ApiResult<Option<ChatState>> {
    if ctx.key_receipt_required() {
        return Ok(None);
    }
    primary(ctx, conn)
        .await?
        .read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                Ok(readable(conn, reader, state.chat_id)
                    .await?
                    .then_some(state))
            })
        })
        .await
        .map(Option::flatten)
}

pub(super) async fn draft_response(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    draft: ChatDraft,
) -> ApiResult<Option<ChatDraft>> {
    if ctx.key_receipt_required() {
        return Ok(None);
    }
    let mut plan = primary(ctx, conn).await?;
    // Custody effects have completed under the action guard. Readback repeats no
    // source consumption, just as the existing message writer's response reader.
    blob_sources::bind(&mut plan, &[])?;
    let allowed = draft_names_plan(ctx, conn, principal, &mut plan, &draft).await?;
    let chat = ctx.id("id")?;
    plan.read_response(ctx, conn, principal.caller(), move |conn, reader| {
        Box::pin(async move {
            if !readable(conn, reader, chat).await? {
                return Ok(None);
            }
            let mut draft = draft;
            mentions::draft_names_bounded(conn, reader, &mut draft, Some(&allowed)).await?;
            Ok(Some(draft))
        })
    })
    .await
    .map(Option::flatten)
}
