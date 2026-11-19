//! Current mention discovery and a real, rollback-only input review under their own Read operation.
use super::*;
use crate::key_operations::{blob_sources, targets};
use crate::registry::{OperationFuture, OperationPlan};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::chats::{
    ChatMentionListQuery, ChatMentionPage, ChatMentionPreviewBody, ChatMentionTarget,
};
use orch_authz::Caller;
use orch_chat::following::BlobOrigin;
use orch_types::Hash;

const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);

pub(super) struct PreparedList {
    names: names::AllowSet,
    query: ChatMentionListQuery,
}
pub(super) struct PreviewSources {
    pub(super) count: i64,
    pub(super) names: names::AllowSet,
    pub(super) blobs: Vec<(Hash, BlobOrigin)>,
}

async fn primary(ctx: &Ctx, conn: &mut Conn) -> ApiResult<OperationPlan> {
    let target = targets::chat(conn, ctx.id("id")?).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(0, CHAT, &[target])?;
    Ok(plan)
}

fn source(target: &ChatMentionTarget) -> Option<names::NameSource> {
    match *target {
        ChatMentionTarget::User { .. } => None,
        ChatMentionTarget::Role { node_id, .. } => Some(names::NameSource::Node(node_id)),
        ChatMentionTarget::Group { id } => Some(names::NameSource::Group(id)),
        ChatMentionTarget::Agent { thread_id } => Some(names::NameSource::Node(thread_id)),
    }
}

fn bind_actual(
    plan: &mut OperationPlan,
    permitted: Vec<(names::NameSource, KeyResolvedTarget)>,
    actual: &names::AllowSet,
) -> ApiResult<()> {
    let retained: Vec<_> = permitted
        .into_iter()
        .filter(|(source, _)| actual.contains(source))
        .collect();
    operation::bind_names(plan, &retained)
}

pub(super) fn list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: ChatMentionListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let result = prepare_list(ctx, conn, caller, query).await;
        operation::project_current_result(ctx, conn, caller, result).await
    })
}

async fn prepare_list(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: ChatMentionListQuery,
) -> ApiResult<OperationPlan> {
    let mut plan = primary(ctx, conn).await?;
    let work = operation::prepare_work(ctx, conn, caller, &plan).await?;
    let id = ctx.id("id")?;
    let result = chats::load(conn, &work.acting(), id, Mode::Read).await;
    let chat = operation::refuse(&work, conn, id, result).await?;
    let all = query
        .q
        .as_deref()
        .is_some_and(|q| q.trim().eq_ignore_ascii_case("@all"));
    let mut page = mentions::native_choices(conn, &work, &chat, all).await?;
    let parts: Vec<_> = page
        .items
        .iter()
        .map(|choice| ChatPart::Mention {
            target: choice.target.clone(),
        })
        .collect();
    let candidates = names::candidate_sources(conn, &[], &parts, None).await?;
    let permitted =
        operation::allowed_names(conn, work.principal.caller(), &plan, candidates).await?;
    let allowed: names::AllowSet = permitted.iter().map(|(source, _)| *source).collect();
    // These choices expose actual Group/Role audiences or Thread facts, not just
    // labels. Public human identity/attribution remains intrinsic to this Chat.
    page.items
        .retain(|choice| source(&choice.target).is_none_or(|source| allowed.contains(&source)));
    let mut actual: names::AllowSet = page
        .items
        .iter()
        .filter_map(|choice| source(&choice.target))
        .collect();
    actual.extend(
        mentions::name_choices(conn, &work.principal, &mut page.items, Some(&allowed)).await?,
    );
    bind_actual(&mut plan, permitted, &actual)?;
    // Keep validation in its native position. The successful page is produced
    // again under the completed current principal, within these exact bounds.
    mentions::page(page, &query)?;
    Ok(plan.with_data(PreparedList {
        names: actual,
        query,
    }))
}

pub(super) async fn show_list(
    conn: &mut Conn,
    work: &Work,
    chat: Id,
    prepared: &PreparedList,
) -> ApiResult<ChatMentionPage> {
    let result = chats::load(conn, &work.acting(), chat, Mode::Read).await;
    let loaded = operation::refuse(work, conn, chat, result).await?;
    let all = prepared
        .query
        .q
        .as_deref()
        .is_some_and(|q| q.trim().eq_ignore_ascii_case("@all"));
    let mut page = mentions::native_choices(conn, work, &loaded, all).await?;
    page.items.retain(|choice| {
        source(&choice.target).is_none_or(|source| prepared.names.contains(&source))
    });
    mentions::name_choices(
        conn,
        &work.principal,
        &mut page.items,
        Some(&prepared.names),
    )
    .await?;
    mentions::page(page, &prepared.query)
}

pub(super) fn preview<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ChatMentionPreviewBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let result = prepare_preview(ctx, conn, caller, body).await;
        operation::project_current_result(ctx, conn, caller, result).await
    })
}

async fn prepare_preview(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: ChatMentionPreviewBody,
) -> ApiResult<OperationPlan> {
    let mut plan = primary(ctx, conn).await?;
    let work = operation::prepare_work(ctx, conn, caller, &plan).await?;
    let id = ctx.id("id")?;
    let result = chats::load(conn, &work.acting(), id, Mode::Read).await;
    let chat = operation::refuse(&work, conn, id, result).await?;
    operation::refuse(&work, conn, id, chat.ensure_send()).await?;
    operation::refuse(&work, conn, id, messages::check_parts(&body.parts, None)).await?;
    let receivers = asking::targets(&body.parts)?;
    // The native effect-free audience decision precedes any trial post, source
    // materialization or model/input acceptance, including its ordinary errors.
    let result = orch_chat::mentions::resolve(
        conn,
        &work.acting(),
        &chat,
        &body.parts,
        Some(work.principal.user()),
    )
    .await;
    let named = operation::refuse(&work, conn, id, result).await?;
    let count = i64::try_from(named.users.len()).map_err(ApiError::internal)?;
    let mut required = Vec::new();
    let mut hashes = Vec::new();
    let reply = if receivers.is_empty() {
        None
    } else {
        // Same native review and ordinary ThreadView; Read scope is checked on
        // those actual receiver facts, never a fabricated input operation.
        asking::review(
            conn,
            &work.principal,
            id,
            &body.parts,
            body.reply_to,
            &receivers,
        )
        .await?;
        let quoted = orch_chat::quotes::source_of(conn, &chat.row, &body.parts).await;
        if let Some(quoted) = operation::refuse(&work, conn, id, quoted).await? {
            required.push(blob_sources::Binding {
                source: blob_sources::SourceKind::Thread,
                target: targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Thread,
                        id: quoted.thread,
                    },
                )
                .await?,
            });
        }
        required.push(blob_sources::Binding {
            source: blob_sources::SourceKind::Chat,
            target: targets::from_chat(conn, &chat.row).await?,
        });
        hashes = orch_chat::attachments::blobs_in(&body.parts);
        match body.reply_to {
            Some(reply) => rows::message(conn, id, reply).await?,
            None => None,
        }
    };
    // A human-only preview consumes none of its attachment/item-ref syntax.
    // The existing agent trial really materializes those sources and keeps the
    // selected byte witness through the unchanged ordinary message writer.
    let blobs = blob_sources::prepare(
        ctx,
        conn,
        work.principal.caller(),
        &plan,
        &hashes,
        &required,
    )
    .await?;
    blob_sources::bind(&mut plan, &blobs.bindings)?;
    let rows: Vec<_> = reply.iter().collect();
    let copy_chat = (!receivers.is_empty()).then_some(&chat.row);
    let candidates = names::candidate_sources(conn, &rows, &body.parts, copy_chat).await?;
    let permitted =
        operation::allowed_names(conn, work.principal.caller(), &plan, candidates).await?;
    let allowed: names::AllowSet = permitted.iter().map(|(source, _)| *source).collect();
    let required_sources: names::AllowSet = body
        .parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Mention { target } => source(target),
            _ => None,
        })
        .collect();
    if work
        .principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
        && !required_sources.is_subset(&allowed)
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the key does not allow a current mention source",
        ));
    }
    let mut actual = required_sources;
    if !receivers.is_empty() {
        let mut preparation = plan.clone();
        operation::bind_names(&mut preparation, &permitted)?;
        let reader = preparation
            .preparation_principal(ctx, conn, work.principal.caller())
            .await?;
        actual.extend(
            names::capture(conn, &reader, &rows, &body.parts, copy_chat, Some(&allowed))
                .await?
                .sources,
        );
        actual.extend(
            names::chat_reference(conn, &reader, &chat.row, Some(&allowed))
                .await?
                .sources,
        );
    }
    bind_actual(&mut plan, permitted, &actual)?;
    Ok(plan.with_data(PreviewSources {
        count,
        names: actual,
        blobs: blobs.selected,
    }))
}
