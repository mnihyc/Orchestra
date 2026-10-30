//! Explicit agent asks reuse ordinary chat-source forwarding and the sender's guarded input producer in one transaction.

use super::*;
use orch_db::queries::threads as thread_rows;
use orch_engine::sharing::ForwardedToThreads;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::Hash;

/// The current receiver facts a person reviews before accepting an ordinary queued input.
pub(super) struct Receiver {
    pub thread: orch_api_contract::nodes::NodeRef,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// The optimistic review binds substance and binding/model identities, with no credential or new authority.
pub(super) struct Review {
    pub fingerprint: Hash,
    pub receivers: Vec<Receiver>,
}

/// Every multi-resource forward takes sorted thread locks before taking any destination chat lock.
/// Missing or unreadable targets are still refused by the ordinary guarded producer, after source checks.
pub(super) async fn lock_threads(conn: &mut Conn, ids: &[Id]) -> ApiResult<()> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        let _ = thread_rows::lock(conn, id).await?;
    }
    Ok(())
}

/// Reads the same current resolved model shown by the thread, while its binding and authority are pinned.
pub(super) async fn review(
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
    parts: &[ChatPart],
    reply: Option<Id>,
    ids: &[Id],
) -> ApiResult<Review> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() > usize::try_from(FORWARD_MAX_DESTINATIONS).unwrap_or(usize::MAX) {
        return Err(ApiError::invalid(
            "parts",
            "too many agents for one ordinary forward",
        ));
    }
    let reply_version = if let Some(id) = reply {
        let row = rows::message(conn, chat, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if row.is_deleted() {
            return Err(ApiError::invalid(
                "reply_to",
                "the original reply says nothing to forward",
            ));
        }
        Some((id, row.seq_updated))
    } else {
        None
    };
    let mut identities = Vec::with_capacity(ids.len());
    for id in &ids {
        authorization::require(
            conn,
            principal,
            *id,
            authorization::exact(Permission::ThreadView),
            Act::Use,
            Policy::default(),
        )
        .await?;
        let row = thread_rows::thread(conn, *id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let model = orch_settings::get::<String>(conn, Place::Node(*id), keys::AGENT_MODEL)
            .await
            .map_err(ApiError::internal)?;
        let effort = orch_settings::get::<String>(conn, Place::Node(*id), keys::AGENT_EFFORT)
            .await
            .map_err(ApiError::internal)?;
        identities.push((*id, row.agent_id, row.provider_id, model, effort));
    }
    let refs = view::refs(conn, principal, &ids, Policy::default()).await?;
    let agents = orch_db::queries::shares::agent_names(
        conn,
        &identities
            .iter()
            .filter_map(|(_, agent, _, _, _)| *agent)
            .collect::<Vec<_>>(),
    )
    .await?;
    let receivers = identities
        .iter()
        .map(|(id, agent, _, model, effort)| {
            Ok(Receiver {
                thread: refs.get(id).cloned().ok_or_else(ApiError::not_found)?,
                agent: agent.and_then(|id| agents.get(&id).cloned()),
                model: model.clone(),
                effort: effort.clone(),
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let substance = serde_json::to_vec(&serde_json::json!({"sender":principal.user(),"parts":parts,"reply":reply_version,"receivers":identities})).map_err(ApiError::internal)?;
    Ok(Review {
        fingerprint: Hash::of(&substance),
        receivers,
    })
}

/// Stable selected thread identities, counted once even if its token occurs twice.
pub(super) fn targets(parts: &[ChatPart]) -> ApiResult<Vec<Id>> {
    let mut ids: Vec<_> = parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Mention {
                target: orch_api_contract::chats::ChatMentionTarget::Agent { thread_id },
            } => Some(*thread_id),
            _ => None,
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() > usize::try_from(FORWARD_MAX_DESTINATIONS).unwrap_or(usize::MAX) {
        return Err(ApiError::invalid(
            "parts",
            "too many agents for one ordinary forward",
        ));
    }
    Ok(ids)
}

/// Item quotes already take their source thread lock; include it before any chat lock too.
pub(super) fn locks(parts: &[ChatPart], targets: &[Id]) -> Vec<Id> {
    targets
        .iter()
        .copied()
        .chain(parts.iter().filter_map(|part| match part {
            ChatPart::ItemRef { thread_id, .. } => Some(*thread_id),
            _ => None,
        }))
        .collect()
}

/// One normal forwarded input per selected receiver. A reply carries its original message first,
/// retaining that author's own snapshot and T4, followed by the newly posted human message.
pub(super) async fn forward_message(
    ctx: &Ctx,
    tx: &mut Tx,
    work: &Work,
    message: &MessageRow,
    to: &[Id],
) -> ApiResult<ForwardedToThreads> {
    let ids: Vec<Id> = message
        .reply_to_id
        .into_iter()
        .chain(std::iter::once(message.id))
        .collect();
    let prepared = forward::prepare(tx, &work.acting(), message.chat_id, &ids).await;
    let prepared = work.refuse(tx, Some(message.chat_id), prepared).await?;
    let people = forward::people_of(&prepared);
    let mut nodes: Vec<Id> = prepared
        .chat
        .row
        .node_id
        .into_iter()
        .chain(to.iter().copied())
        .collect();
    let source_rows: Vec<_> = prepared.messages.iter().collect();
    nodes.extend(views::threads_of(&source_rows));
    let mut names = lookups(
        tx,
        &work.principal,
        &people,
        &nodes,
        &views::groups_of(&source_rows),
    )
    .await?;
    names.nodes.extend(
        view::qualified_refs(
            tx,
            &work.principal,
            &views::role_places_of(&source_rows),
            Policy::default(),
        )
        .await?,
    );
    let chat = chat_view(
        tx,
        &work.principal,
        &prepared.chat.row,
        &prepared.chat.members,
        &prepared.chat.standing,
    )
    .await?;
    let parts = input_parts(&forward::copies_of(&prepared, &chat.base, &names));
    let client = format!("chat-mention:{}", message.id);
    match ctx
        .app
        .engine
        .forward_to_threads(tx, work.principal.caller(), to, &parts, &client)
        .await
    {
        Ok(done) => Ok(done),
        Err((thread, error)) => {
            let failed =
                crate::routes::threads::input_failed(tx, thread.unwrap_or_else(Id::nil), error)
                    .await;
            Err(match thread {
                Some(thread) => failed.with("object", Destination::Thread(thread).named()),
                None => failed,
            })
        }
    }
}
