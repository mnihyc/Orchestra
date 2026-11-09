//! Human management of a chat's per-origin posting stop, serialized with the agent's own sends.

use super::*;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::chats::{ChatAgentPostingSet, ChatAgentPostingSetBody, ChatAgentReach};
use orch_db::audit::{self, Actor, Entry};
use serde_json::json;

pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_operation(
        ChatAgentPostingSet,
        crate::key_operations::direct::declared,
        set_posting,
    );
}

pub(super) async fn roster(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
) -> ApiResult<Vec<ChatAgentReach>> {
    let visible = visible_origins(conn, principal, chat).await?;
    let prepared = prepare(conn, principal, chat, &visible).await?;
    project(conn, principal, &prepared, None).await
}

/// The original ordinary ThreadView cut, without reading the agents' current payloads.
pub(super) async fn visible_origins(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
) -> ApiResult<Vec<Id>> {
    use orch_access_contract::permissions::PermissionName;
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
    Ok(visible.into_iter().map(|thread| thread.id).collect())
}

/// Current roster facts retained for the same snapshot's final projection.
pub(super) struct PreparedOrigin {
    pub(super) thread: Id,
    agent: Option<Id>,
    readable: bool,
    writable: bool,
    stopped: bool,
    next_allowed_at: Option<orch_types::Time>,
}

/// Computes the existing reach facts only for the ordinary-visible and operation-admitted origins.
pub(super) async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
    visible: &[Id],
) -> ApiResult<Vec<PreparedOrigin>> {
    use orch_authz::reach::{Agent, Reach, Things};
    let mut origins = Vec::new();
    for thread in visible {
        let Some(agent) = Agent::of(conn, *thread).await.map_err(ApiError::internal)? else {
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
        // Every chat answer comes from the current shared resolver.
        let reach = Reach::load_current(conn, agent, principal.now(), &things)
            .await
            .map_err(ApiError::internal)?;
        let standing = chat
            .node_id
            .map_or_else(|| reach.people_chat(chat.id), |node| reach.node_chat(node));
        let stopped = rows::posting::is_stopped(conn, chat.id, *thread).await?;
        if !standing.reads() && !stopped {
            continue;
        }
        let Some(stored) = orch_db::queries::threads::thread(conn, *thread).await? else {
            continue;
        };
        let limit = orch_settings::get::<i64>(
            conn,
            orch_settings::Place::Node(*thread),
            orch_settings_contract::registry::keys::BROADCAST_CHAT_MESSAGES_PER_HOUR,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(SHIPPED_POST_LIMIT);
        let next_allowed_at =
            rows::posting::next_allowed(conn, *thread, limit, principal.now().get())
                .await?
                .map(orch_types::Time::new);
        origins.push(PreparedOrigin {
            thread: *thread,
            agent: stored.agent_id,
            readable: standing.reads(),
            writable: standing.writes(),
            stopped,
            next_allowed_at,
        });
    }
    Ok(origins)
}

/// Renders the existing roster, with protected qualified paths bounded before their names are read.
pub(super) async fn project(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &[PreparedOrigin],
    bounds: Option<&crate::routes::access::page::NameBounds>,
) -> ApiResult<Vec<ChatAgentReach>> {
    let ids: Vec<_> = prepared.iter().map(|row| row.thread).collect();
    let refs = match bounds {
        Some(bounds) => bounds.qualified_refs(conn, principal, &ids).await?,
        None => view::qualified_refs(conn, principal, &ids, Policy::default()).await?,
    };
    let ids: Vec<_> = prepared.iter().filter_map(|row| row.agent).collect();
    let names = orch_db::queries::shares::agent_names(conn, &ids).await?;
    Ok(prepared
        .iter()
        .filter_map(|row| {
            Some(ChatAgentReach {
                thread: refs.get(&row.thread)?.clone(),
                agent: row
                    .agent
                    .and_then(|id| names.get(&id))
                    .cloned()
                    .unwrap_or_default(),
                readable: row.readable,
                writable: row.writable,
                stopped: row.stopped,
                next_allowed_at: row.next_allowed_at,
            })
        })
        .collect())
}

// The registry's shipped rolling-hour bound when a deployment has no override.
const SHIPPED_POST_LIMIT: i64 = 60;

async fn set_posting(ctx: Ctx, (): (), body: ChatAgentPostingSetBody) -> ApiResult<()> {
    let chat = ctx.id("id")?;
    let thread = ctx.id("thread_id")?;
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    // Authority first, then origin, then chat: the same order as an agent's accepted post.
    orch_db::queries::threads::lock(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    rows::lock(&mut tx, chat)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        origin: origin(&ctx),
        now: guard.now().get(),
    };
    let loaded = load(&mut tx, &work, chat, Mode::Read).await?;
    let manages = loaded.standing.manage;
    if !manages {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "only this chat's current manager may stop an agent's posting",
        )
        .with("required", "chat.manage"));
    }
    guard.check_deadline(&ctx)?;
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
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
