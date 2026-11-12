//! Current readable chat mention choices, filtered before pagination.

use super::*;

use orch_api_contract::chats::ChatMentionList;

use orch_api_contract::chats::ChatMentionListQuery;

use orch_api_contract::chats::ChatMentionPage;

use orch_api_contract::chats::ChatMentionPreview;

use orch_api_contract::chats::ChatMentionPreviewBody;

use orch_api_contract::chats::ChatMentionPreviewReply;

use orch_api_contract::chats::ChatMentionTarget;

use orch_api_contract::chats::MentionChoice;
pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(ChatMentionList, super::mention_operation::list, list)
        .on_with_operation(
            ChatMentionPreview,
            super::mention_operation::preview,
            preview,
        );
}

async fn list(ctx: Ctx, query: ChatMentionListQuery, (): ()) -> ApiResult<ChatMentionPage> {
    crate::key_operations::snapshot_read::run(ctx, move |ctx| list_snapshot(ctx, query.clone()))
        .await
}

async fn list_snapshot(ctx: Ctx, _query: ChatMentionListQuery) -> ApiResult<ChatMentionPage> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<super::mention_operation::PreparedList>()?;
    let work = Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(&ctx),
    };
    let page =
        super::mention_operation::show_list(&mut tx, &work, ctx.id("id")?, &prepared).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(page)
}

/// The same native audiences and offered agents, before search/pagination or source filtering.
/// These private candidate facts establish no successful public projection or new permission.
pub(super) async fn native_choices(
    conn: &mut Conn,
    work: &Work,
    chat: &Loaded,
    only_followers: bool,
) -> ApiResult<ChatMentionPage> {
    let result = orch_chat::audiences::list(conn, &work.acting(), chat, only_followers).await;
    let audiences = super::operation::refuse(work, conn, chat.id(), result).await?;
    let result = orch_chat::audiences::followers(conn, &work.acting(), chat).await;
    let followers = super::operation::refuse(work, conn, chat.id(), result).await?;
    let count = i64::try_from(followers.len()).map_err(ApiError::internal)?;
    let user_ids: Vec<_> = audiences
        .iter()
        .filter_map(|audience| match audience.choice.target {
            ChatMentionTarget::User { id } => Some(id),
            _ => None,
        })
        .collect();
    let users = refusals::user_refs(conn, &user_ids).await?;
    let mut items: Vec<_> = audiences
        .into_iter()
        .map(|audience| {
            let mut choice = audience.choice;
            if let ChatMentionTarget::User { id } = choice.target {
                choice.user = users.get(&id).cloned();
            }
            choice
        })
        .collect();
    if !only_followers {
        let result = orch_chat::audiences::agent_threads(conn, &work.acting(), chat).await;
        let offered = super::operation::refuse(work, conn, chat.id(), result).await?;
        let ids: Vec<_> = offered.iter().map(|thread| thread.node_id).collect();
        let names = orch_db::queries::shares::agent_names(
            conn,
            &offered
                .iter()
                .filter_map(|thread| thread.agent_id)
                .collect::<Vec<_>>(),
        )
        .await?;
        let starts: HashMap<_, _> = rows::posting::origin_starts(conn, &ids)
            .await?
            .into_iter()
            .collect();
        let allowed = nodes::holding(
            conn,
            &work.principal,
            &ids,
            authorization::exact(Permission::ThreadInteract),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        for thread in offered {
            let available = allowed.iter().any(|node| node.id == thread.node_id)
                && thread.deleted_at.is_none()
                && thread.archived_at.is_none();
            items.push(MentionChoice {
                user: None,
                target: ChatMentionTarget::Agent {
                    thread_id: thread.node_id,
                },
                name: thread.name,
                count: 0,
                available,
                reason: (!available).then(|| "thread.interact".to_owned()),
                thread: None,
                agent: thread.agent_id.and_then(|id| names.get(&id).cloned()),
                state: Some(thread.state),
                started_at: starts
                    .get(&thread.node_id)
                    .copied()
                    .map(orch_types::Time::new),
            });
        }
    }
    Ok(ChatMentionPage {
        items,
        next_cursor: None,
        followers: count,
    })
}

/// Existing picker labels: qualified role paths, qualified groups and current thread refs.
/// A draft uses compact role labels instead; neither formatter is substituted for the other.
pub(super) async fn name_choices(
    conn: &mut Conn,
    principal: &Principal,
    items: &mut [MentionChoice],
    allowed: Option<&names::AllowSet>,
) -> ApiResult<names::AllowSet> {
    let mut groups = Vec::new();
    let mut roles = Vec::new();
    let mut threads = Vec::new();
    for choice in items.iter() {
        match choice.target {
            ChatMentionTarget::Group { id } => groups.push(id),
            ChatMentionTarget::Role { node_id, .. } => roles.push(node_id),
            ChatMentionTarget::Agent { thread_id } => threads.push(thread_id),
            ChatMentionTarget::User { .. } => {}
        }
    }
    let captured = names::lookups(conn, principal, &[], &threads, &groups, &roles, allowed).await?;
    for choice in items {
        match choice.target {
            ChatMentionTarget::Group { id } => {
                if let Some(name) = captured.lookups.groups.get(&id) {
                    choice.name = name.clone();
                }
            }
            ChatMentionTarget::Role { node_id, .. } => {
                if let Some(place) = captured
                    .lookups
                    .nodes
                    .get(&node_id)
                    .and_then(|node| node.name.as_deref())
                {
                    choice.name = format!("{} of {place}", choice.name);
                }
            }
            ChatMentionTarget::Agent { thread_id } => {
                choice.thread = captured.lookups.nodes.get(&thread_id).cloned();
            }
            ChatMentionTarget::User { .. } => {}
        }
    }
    Ok(captured.sources)
}

/// Search and the native numeric cursor see only the current projected candidate set.
pub(super) fn page(
    mut page: ChatMentionPage,
    query: &ChatMentionListQuery,
) -> ApiResult<ChatMentionPage> {
    let q = query.q.as_deref().unwrap_or_default().trim().to_lowercase();
    let all = q == "@all";
    page.items.retain(|choice| {
        if matches!(choice.target, ChatMentionTarget::Agent { .. }) {
            q.is_empty()
                || choice
                    .name
                    .to_lowercase()
                    .contains(q.trim_start_matches('@'))
                || choice
                    .agent
                    .as_ref()
                    .is_some_and(|agent| agent.to_lowercase().contains(q.trim_start_matches('@')))
        } else {
            all || q.is_empty()
                || choice.name.to_lowercase().contains(&q)
                || choice.user.as_ref().is_some_and(|user| {
                    user.username
                        .to_lowercase()
                        .contains(q.trim_start_matches('@'))
                })
        }
    });
    let offset = query
        .cursor
        .as_deref()
        .map(str::parse::<usize>)
        .transpose()
        .map_err(|_| ApiError::invalid("cursor", "is not a mention page cursor"))?
        .unwrap_or_default();
    let limit = usize::try_from(
        PageQuery {
            limit: query.limit,
            cursor: None,
        }
        .limit(),
    )
    .map_err(|_| ApiError::invalid("limit", "does not fit a mention page"))?;
    let end = offset.saturating_add(limit).min(page.items.len());
    page.next_cursor = (end < page.items.len()).then(|| end.to_string());
    page.items = if offset < page.items.len() {
        page.items.drain(offset..end).collect()
    } else {
        Vec::new()
    };
    Ok(page)
}

/// The same private-draft labels, within the opening's admitted live name sources.
pub(super) async fn draft_names_bounded(
    conn: &mut Conn,
    principal: &Principal,
    draft: &mut ChatDraft,
    allowed: Option<&names::AllowSet>,
) -> ApiResult<()> {
    draft_names_with_sources(conn, principal, draft, allowed).await?;
    Ok(())
}

/// The native compact draft labels together with the current sources actually retained.
pub(super) async fn draft_names_with_sources(
    conn: &mut Conn,
    principal: &Principal,
    draft: &mut ChatDraft,
    allowed: Option<&names::AllowSet>,
) -> ApiResult<names::AllowSet> {
    let targets: Vec<_> = draft
        .parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Mention { target } => Some(target.clone()),
            _ => None,
        })
        .collect();
    let mut people = Vec::new();
    let mut nodes = Vec::new();
    let mut groups = Vec::new();
    for target in &targets {
        match target {
            ChatMentionTarget::User { id } => people.push(*id),
            ChatMentionTarget::Role { node_id, .. } => nodes.push(*node_id),
            ChatMentionTarget::Group { id } => groups.push(*id),
            ChatMentionTarget::Agent { thread_id } => nodes.push(*thread_id),
        }
    }
    let names = names::lookups(conn, principal, &people, &nodes, &groups, &[], allowed).await?;
    draft.mention_names = views::mention_names(&targets, &names.lookups);
    Ok(names.sources)
}

async fn preview(
    ctx: Ctx,
    (): (),
    body: ChatMentionPreviewBody,
) -> ApiResult<ChatMentionPreviewReply> {
    let caller = ctx
        .admitted_caller()
        .cloned()
        .ok_or_else(|| ApiError::internal("the mention preview has no admitted caller"))?;
    match preview_once(ctx.clone(), body).await {
        Ok(value) => Ok(value),
        Err(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            // An ordinary provider/model-use refusal can contain labels outside
            // this Read result. Reuse the same current-caller Problem boundary;
            // no restored caller is loaned back into the native trial.
            super::operation::project_current_result(&ctx, &mut conn, &caller, Err(error)).await
        }
    }
}

async fn preview_once(
    mut ctx: Ctx,
    body: ChatMentionPreviewBody,
) -> ApiResult<ChatMentionPreviewReply> {
    let id = ctx.id("id")?;
    let (mut tx, mut guard) = transaction_guard::begin(&ctx).await?;
    let targets = asking::targets(&body.parts)?;
    if !targets.is_empty() {
        asking::lock_threads(&mut tx, &asking::locks(&body.parts, &targets)).await?;
        rows::lock(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        // The same operation, current source paths and current ordinary actor
        // are resolved after the actual sorted recipient/quote/Chat waits.
        ctx.now = ctx.app.clock.now();
        guard = transaction_guard::check_conn(&ctx, &mut tx).await?;
    }
    let sources = guard.operation_data::<super::mention_operation::PreviewSources>()?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        now: guard.now().get(),
        origin: origin(&ctx),
    };
    let loaded = chats::load(&mut tx, &work.acting(), id, Mode::Read).await;
    let mut chat = super::operation::refuse(&work, &mut tx, id, loaded).await?;
    let count = sources.count;
    if !targets.is_empty() {
        let review = asking::review(
            &mut tx,
            &work.principal,
            id,
            &body.parts,
            body.reply_to,
            &targets,
        )
        .await?;
        let masking = redactor(&mut tx, &work, &chat).await?;
        rows::begin_mention_preview(&mut tx).await?;
        let written = messages::post_with_blob_origins(
            &mut tx,
            &work.acting(),
            &mut chat,
            Post {
                parts: body.parts.clone(),
                reply_to: body.reply_to,
                client_id: format!("agent-preview:{}", orch_types::new_id()),
            },
            &masking,
            &sources.blobs,
        )
        .await;
        let written = super::operation::refuse(&work, &mut tx, id, written).await?;
        let done = asking::forward_message(
            &ctx,
            &mut tx,
            &work,
            &written.message,
            &targets,
            Some(&sources.names),
        )
        .await?;
        let agent_asks = review
            .receivers
            .into_iter()
            .map(|receiver| {
                let input = done
                    .inputs
                    .iter()
                    .find(|input| input.thread == receiver.thread.id)
                    .ok_or_else(ApiError::not_found)?;
                Ok(orch_api_contract::chats::ChatAgentAskPreview {
                    thread: receiver.thread,
                    agent: receiver.agent,
                    model: receiver.model,
                    effort: receiver.effort,
                    input: input.input.clone(),
                })
            })
            .collect::<ApiResult<Vec<_>>>()?;
        // The trial must not become its own fresh byte/custody source during
        // final admission. Remove all of it before repeating the current Read.
        rows::discard_mention_preview(&mut tx).await?;
        ctx.now = ctx.app.clock.now();
        let current = transaction_guard::check_conn(&ctx, &mut tx).await?;
        let current_sources =
            current.operation_data::<super::mention_operation::PreviewSources>()?;
        let current_review = asking::review(
            &mut tx,
            current.principal(),
            id,
            &body.parts,
            body.reply_to,
            &targets,
        )
        .await?;
        if !sources.names.is_subset(&current_sources.names)
            || sources.blobs != current_sources.blobs
            || review.fingerprint != current_review.fingerprint
        {
            return Err(DbError::Retry.into());
        }
        // Only outer read/authority bookkeeping commits through its normal epoch
        // fence. No post/input/pin/audit/outbox/notification or wake survives;
        // the native writer's Finished value is never handed to after_forward.
        tx.commit().await.map_err(DbError::from)?;
        return Ok(ChatMentionPreviewReply {
            count,
            agent_asks,
            agent_review: Some(review.fingerprint),
        });
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(ChatMentionPreviewReply {
        count,
        agent_asks: Vec::new(),
        agent_review: None,
    })
}
