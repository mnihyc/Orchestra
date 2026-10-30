//! Human management of a chat's per-origin posting stop, serialized with the agent's own sends.

use super::*;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::chats::{ChatAgentPostingSet, ChatAgentPostingSetBody, ChatAgentReach};
use orch_db::audit::{self, Actor, Entry};
use serde_json::json;

pub(super) fn mount(registry: &mut Registry) {
    registry.on(ChatAgentPostingSet, set_posting);
}

pub(super) async fn roster(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
) -> ApiResult<Vec<ChatAgentReach>> {
    use orch_access_contract::permissions::PermissionName;
    use orch_authz::reach::{Agent, AgentReach, Reach, Things};
    let candidates = rows::posting::origins(conn, chat.id, principal.now().get()).await?;
    let visible = nodes::holding(
        conn,
        principal,
        &candidates,
        PermissionName::Exact(Permission::ThreadView),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let ids: Vec<_> = visible.iter().map(|thread| thread.id).collect();
    let refs = view::qualified_refs(conn, principal, &ids, Policy::default()).await?;
    let mut origins = Vec::new();
    for thread in visible {
        let Some(agent) = Agent::of(conn, thread.id)
            .await
            .map_err(ApiError::internal)?
        else {
            continue;
        };
        let things = Things {
            nodes: chat.node_id.into_iter().collect(),
            documents: Vec::new(),
            people_chats: chat
                .node_id
                .is_none()
                .then_some(chat.id)
                .into_iter()
                .collect(),
        };
        // Conversation policy supplies no chat rights: every chat answer comes from the shared resolver.
        let reach = Reach::load(conn, agent, AgentReach::None, principal.now(), &things)
            .await
            .map_err(ApiError::internal)?;
        let standing = chat
            .node_id
            .map_or_else(|| reach.people_chat(chat.id), |node| reach.node_chat(node));
        let stopped = rows::posting::is_stopped(conn, chat.id, thread.id).await?;
        if !standing.reads() && !stopped {
            continue;
        }
        let Some(stored) = orch_db::queries::threads::thread(conn, thread.id).await? else {
            continue;
        };
        let name = match stored.agent_id {
            Some(id) => orch_db::queries::shares::agent_names(conn, &[id])
                .await?
                .remove(&id)
                .unwrap_or_default(),
            None => String::new(),
        };
        let limit = orch_settings::get::<i64>(
            conn,
            orch_settings::Place::Node(thread.id),
            orch_settings_contract::registry::keys::BROADCAST_CHAT_MESSAGES_PER_HOUR,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(SHIPPED_POST_LIMIT);
        let next_allowed_at =
            rows::posting::next_allowed(conn, thread.id, limit, principal.now().get())
                .await?
                .map(orch_types::Time::new);
        if let Some(reference) = refs.get(&thread.id) {
            origins.push(ChatAgentReach {
                thread: reference.clone(),
                agent: name,
                readable: standing.reads(),
                writable: standing.writes(),
                stopped,
                next_allowed_at,
            });
        }
    }
    Ok(origins)
}

// The registry's shipped rolling-hour bound when a deployment has no override.
const SHIPPED_POST_LIMIT: i64 = 60;

async fn set_posting(ctx: Ctx, (): (), body: ChatAgentPostingSetBody) -> ApiResult<()> {
    let chat = ctx.id("id")?;
    let thread = ctx.id("thread_id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    // Authority first, then origin, then chat: the same order as an agent's accepted post.
    orch_db::queries::threads::lock(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let work = Work {
        principal,
        origin: origin(&ctx),
        now: ctx.now,
    };
    let loaded = load(&mut tx, &work, chat, Mode::Read).await?;
    let manages = loaded.row.node_id.is_some() && loaded.standing.moderate
        || loaded.row.kind == rows::KIND_GROUP
            && loaded.row.created_by == Some(work.principal.user());
    if !manages {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "only this chat's current manager may stop an agent's posting",
        )
        .with("required", "chat.manage"));
    }
    if rows::posting::set(
        &mut tx,
        chat,
        thread,
        body.stopped,
        work.principal.user(),
        ctx.now,
    )
    .await?
    {
        let action = if body.stopped {
            orch_db::audit::action!(AuditAction::ChatAgentPostingStopped)
        } else {
            orch_db::audit::action!(AuditAction::ChatAgentPostingAllowed)
        };
        let mut entry = Entry::new(
            Actor::User(work.principal.user()),
            action.as_str(),
            "chat",
            Some(chat),
            if body.stopped {
                "stopped an agent's posting in this chat"
            } else {
                "allowed an agent's posting in this chat"
            },
        );
        entry.org_id = Some(loaded.row.org_id);
        entry.node_id = loaded.row.node_id;
        entry.details = json!({ "thread": thread, "stopped": body.stopped });
        audit::append(&mut tx, &entry, &work.origin, ctx.now).await?;
        let seq = rows::next_seq(&mut tx, chat)
            .await?
            .ok_or_else(ApiError::not_found)?;
        rows::stamp_state(&mut tx, chat, seq).await?;
        rows::notify(&mut tx, chat, seq).await?;
        orch_authz::epoch::bump(&mut tx)
            .await
            .map_err(ApiError::internal)?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
