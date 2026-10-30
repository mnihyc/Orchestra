//! Current readable chat mention choices, filtered before pagination.

use super::*;
use orch_api_contract::chats::{
    ChatMentionList, ChatMentionListQuery, ChatMentionPage, ChatMentionPreview,
    ChatMentionPreviewBody, ChatMentionPreviewReply, ChatMentionTarget, MentionChoice,
};

pub(super) fn mount(registry: &mut Registry) {
    registry
        .on(ChatMentionList, list)
        .on(ChatMentionPreview, preview);
}

async fn list(ctx: Ctx, query: ChatMentionListQuery, (): ()) -> ApiResult<ChatMentionPage> {
    let id = ctx.id("id")?;
    let mut tx = ctx.app.db.begin().await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let chat = load(&mut tx, &work, id, Mode::Read).await?;
    let q = query.q.as_deref().unwrap_or_default().trim().to_lowercase();
    let all = q == "@all";
    let result = orch_chat::audiences::list(&mut tx, &work.acting(), &chat, all).await;
    let audiences = work.refuse(&mut tx, Some(id), result).await?;
    let current_followers = orch_chat::audiences::followers(&mut tx, &work.acting(), &chat).await;
    let followers = work.refuse(&mut tx, Some(id), current_followers).await?;
    let count = i64::try_from(followers.len()).map_err(ApiError::internal)?;
    let user_ids: Vec<_> = audiences
        .iter()
        .filter_map(|audience| match audience.choice.target {
            ChatMentionTarget::User { id } => Some(id),
            _ => None,
        })
        .collect();
    let users = refusals::user_refs(&mut tx, &user_ids).await?;
    let group_ids: Vec<_> = audiences
        .iter()
        .filter_map(|audience| match audience.choice.target {
            ChatMentionTarget::Group { id } => Some(id),
            _ => None,
        })
        .collect();
    let groups = qualified_groups(&mut tx, &work.principal, &group_ids).await?;
    let places: Vec<_> = audiences
        .iter()
        .filter_map(|audience| match audience.choice.target {
            ChatMentionTarget::Role { node_id, .. } => Some(node_id),
            _ => None,
        })
        .collect();
    let places = view::qualified_refs(&mut tx, &work.principal, &places, Policy::default()).await?;
    let mut items: Vec<MentionChoice> = audiences
        .into_iter()
        .map(|audience| audience.choice)
        .map(|mut choice| {
            match &choice.target {
                ChatMentionTarget::User { id } => {
                    choice.user = users.get(id).cloned();
                }
                ChatMentionTarget::Group { id } => {
                    if let Some(name) = groups.get(id) {
                        choice.name = name.clone();
                    }
                }
                ChatMentionTarget::Role { node_id, .. } => {
                    if let Some(place) = places.get(node_id).and_then(|place| place.name.as_deref())
                    {
                        choice.name = format!("{} of {place}", choice.name);
                    }
                }
                ChatMentionTarget::Agent { .. } => {}
            }
            choice
        })
        .filter(|choice| {
            all || q.is_empty()
                || choice.name.to_lowercase().contains(&q)
                || choice.user.as_ref().is_some_and(|user| {
                    user.username
                        .to_lowercase()
                        .contains(q.trim_start_matches('@'))
                })
        })
        .collect();
    if !all {
        let offered = orch_chat::audiences::agent_threads(&mut tx, &work.acting(), &chat).await;
        let offered = work.refuse(&mut tx, Some(id), offered).await?;
        let ids: Vec<_> = offered.iter().map(|thread| thread.node_id).collect();
        let refs = view::refs(&mut tx, &work.principal, &ids, Policy::default()).await?;
        let names = orch_db::queries::shares::agent_names(
            &mut tx,
            &offered
                .iter()
                .filter_map(|thread| thread.agent_id)
                .collect::<Vec<_>>(),
        )
        .await?;
        let starts: HashMap<_, _> = rows::posting::origin_starts(&mut tx, &ids)
            .await?
            .into_iter()
            .collect();
        let allowed = nodes::holding(
            &mut tx,
            &work.principal,
            &ids,
            orch_access_contract::permissions::PermissionName::Exact(Permission::ThreadInteract),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        for thread in offered {
            let agent = thread.agent_id.and_then(|id| names.get(&id).cloned());
            if !q.is_empty()
                && !thread
                    .name
                    .to_lowercase()
                    .contains(q.trim_start_matches('@'))
                && !agent
                    .as_deref()
                    .is_some_and(|name| name.to_lowercase().contains(q.trim_start_matches('@')))
            {
                continue;
            }
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
                thread: refs.get(&thread.node_id).cloned(),
                agent,
                state: Some(thread.state),
                started_at: starts
                    .get(&thread.node_id)
                    .copied()
                    .map(orch_types::Time::new),
            });
        }
    }
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
    let end = offset.saturating_add(limit).min(items.len());
    let next_cursor = (end < items.len()).then(|| end.to_string());
    items = if offset < items.len() {
        items.drain(offset..end).collect()
    } else {
        Vec::new()
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(ChatMentionPage {
        items,
        next_cursor,
        followers: count,
    })
}

pub(super) async fn qualified_groups(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<HashMap<Id, String>> {
    let mut names = orch_chat::audiences::visible_group_names(conn, principal, ids)
        .await
        .map_err(ApiError::internal)?;
    let rows =
        orch_db::queries::access::groups_full(conn, &names.keys().copied().collect::<Vec<_>>())
            .await?;
    let places: Vec<_> = rows.iter().map(|group| group.org_id).collect();
    let places = view::qualified_refs(conn, principal, &places, Policy::default()).await?;
    for group in rows {
        if let Some(place) = places
            .get(&group.org_id)
            .and_then(|place| place.name.as_deref())
        {
            names.insert(group.id, format!("{}, {place}", group.name));
        }
    }
    Ok(names)
}

pub(super) async fn draft_names(
    conn: &mut Conn,
    principal: &Principal,
    draft: &mut ChatDraft,
) -> ApiResult<()> {
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
    let names = lookups(conn, principal, &people, &nodes, &groups).await?;
    draft.mention_names = views::mention_names(&targets, &names);
    Ok(())
}

async fn preview(
    ctx: Ctx,
    (): (),
    body: ChatMentionPreviewBody,
) -> ApiResult<ChatMentionPreviewReply> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let work = Work {
        principal,
        origin: origin(&ctx),
        now: ctx.now,
    };
    let mut chat = load(&mut tx, &work, id, Mode::Read).await?;
    work.refuse(&mut tx, Some(id), chat.ensure_send()).await?;
    work.refuse(&mut tx, Some(id), messages::check_parts(&body.parts, None))
        .await?;
    let targets = asking::targets(&body.parts)?;
    if !targets.is_empty() {
        asking::lock_threads(&mut tx, &asking::locks(&body.parts, &targets)).await?;
        chat.lock(&mut tx).await.map_err(ApiError::internal)?;
    }
    let result = orch_chat::mentions::resolve(
        &mut tx,
        &work.acting(),
        &chat,
        &body.parts,
        Some(work.principal.user()),
    )
    .await;
    let named = work.refuse(&mut tx, Some(id), result).await?;
    let count = i64::try_from(named.users.len()).map_err(ApiError::internal)?;
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
        let written = messages::post(
            &mut tx,
            &work.acting(),
            &mut chat,
            Post {
                parts: body.parts,
                reply_to: body.reply_to,
                client_id: format!("agent-preview:{}", orch_types::new_id()),
            },
            &masking,
        )
        .await;
        let written = work.refuse(&mut tx, Some(id), written).await?;
        let done =
            asking::forward_message(&ctx, &mut tx, &work, &written.message, &targets).await?;
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
        // The actual normal producer validated this input; none of its writes or wakes survive a preview.
        tx.rollback().await.map_err(DbError::from)?;
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
