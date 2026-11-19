//! Gives one person access to a chat and optionally tells them in the same act.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::access::subject;
use crate::routes::nodes::{invalid, policy_at};
use crate::sign_in::origin;
use orch_access_contract::pages::{EntryAdd, Holder, PageChange, RowChange};
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::AccessChoice;
use orch_api_contract::common::{ErrorCode, IDEMPOTENCY_HEADER};
use orch_api_contract::notifications::{ChatPersonGrant, ChatPersonGrantBody};
use orch_authz::pages::{self, Target};
use orch_authz::{Policy, Principal};
use orch_chat::chats::{self, Mode};
use orch_db::queries::{access, chats as rows, hierarchy, threads};
use orch_db::{Conn, DbError};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::Id;

use super::person;

pub(super) fn mount(registry: &mut Registry) {
    registry.on(ChatPersonGrant, grant);
}

/// What the sender may grant to this non-reader without widening the chat's reach.
pub(super) async fn choices(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    chat: &rows::ChatRow,
    user: Id,
    can_read: bool,
    human: bool,
) -> ApiResult<(bool, Vec<AccessChoice>)> {
    if let Some(node) = chat.node_id {
        let thing = orch_authz::things::Thing::Node(node);
        match subject::require_page(conn, principal, thing, Policy::default()).await {
            Ok(()) => {}
            Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => {
                return Ok((false, Vec::new()));
            }
            Err(error) => return Err(error),
        }
        let policy = policy_at(conn, node).await?;
        let page = pages::read(conn, principal, Target::Live(node), policy)
            .await
            .map_err(ApiError::internal)?;
        let local = access::local_holders_among(conn, chat.org_id, &[user]).await?;
        if can_read || !human || !page.external && local.is_empty() {
            return Ok((true, Vec::new()));
        }
        let can = |permission| {
            page.rows
                .iter()
                .any(|row| row.permission == permission && row.can_give)
        };
        let mut gives = Vec::new();
        if can(Permission::ChatView) {
            gives.push(AccessChoice::FullView);
            if can(Permission::ChatInteract) {
                gives.push(AccessChoice::FullInteract);
            }
        }
        return Ok((true, gives));
    }
    if can_read || !human || chat.kind != "group" {
        return Ok((true, Vec::new()));
    }
    let source = origin(ctx);
    let act = orch_chat::Acting {
        principal,
        now: principal.now().get(),
        origin: &source,
    };
    let loaded = match chats::load(conn, &act, chat.id, Mode::Read).await {
        Ok(value) => value,
        Err(error) => {
            return Err(crate::routes::chats::failed(conn, principal, Some(chat.id), error).await);
        }
    };
    let outsiders = setting::<bool>(conn, Place::Node(chat.org_id), keys::SHARING_ALLOW_EXTERNAL)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(true);
    let allowed = match chats::can_add_member(conn, &act, &loaded, user, outsiders).await {
        Ok(_) => true,
        Err(orch_chat::Error::Db(error)) => return Err(error.into()),
        Err(orch_chat::Error::Authz(error)) => return Err(ApiError::internal(error)),
        Err(orch_chat::Error::Settings(error)) => return Err(ApiError::internal(error)),
        Err(orch_chat::Error::Internal(error)) => return Err(ApiError::internal(error)),
        Err(_) => false,
    };
    let max = chats::group_max(conn).await.map_err(ApiError::internal)?;
    let present = i64::try_from(
        loaded
            .members
            .iter()
            .filter(|member| member.active_at(principal.now().get()))
            .count(),
    )
    .map_err(ApiError::internal)?;
    Ok((
        true,
        if allowed && present < max {
            vec![AccessChoice::FullInteract]
        } else {
            Vec::new()
        },
    ))
}

fn row(permission: Permission, user: Id) -> RowChange {
    RowChange {
        permission: permission.as_str().to_owned(),
        profile: None,
        inherit: None,
        locked: None,
        add: Some(vec![EntryAdd {
            holder: Holder::User { id: user },
            options: None,
            expires_at: None,
            note: None,
        }]),
        update: None,
        remove: None,
    }
}

/// Uses the ordinary page producer for only the chat's own two permissions.
async fn give_node_chat(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    user: Id,
    choice: AccessChoice,
) -> ApiResult<()> {
    let mut rows = vec![row(Permission::ChatView, user)];
    if choice == AccessChoice::FullInteract {
        rows.push(row(Permission::ChatInteract, user));
    }
    let change = PageChange {
        rows: Some(rows),
        ..PageChange::default()
    };
    let before = threads::thread(conn, node)
        .await?
        .map(|thread| thread.owner_id);
    let policy = policy_at(conn, node).await?;
    let changed = match pages::change(
        conn,
        principal,
        Target::Live(node),
        &change,
        policy,
        &origin(ctx),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return Err(refusals::change_failed(conn, error).await),
    };
    if changed.changed {
        hierarchy::stamp(conn, node).await?;
        if let Some(first) = before {
            orch_chat::following::refresh_thread_people(conn, node, &[user], principal.now().get())
                .await
                .map_err(ApiError::internal)?;
            crate::thread_markers::page_changed(
                conn,
                principal.now().get(),
                node,
                principal.user(),
                first,
                &change,
            )
            .await?;
        }
    }
    Ok(())
}

async fn give_group_chat(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    chat: &rows::ChatRow,
    user: Id,
    choice: AccessChoice,
) -> ApiResult<()> {
    if choice != AccessChoice::FullInteract {
        return Err(invalid("choice", "a group member may participate"));
    }
    let source = origin(ctx);
    let act = orch_chat::Acting {
        principal,
        now: principal.now().get(),
        origin: &source,
    };
    let mut loaded = match chats::load(conn, &act, chat.id, Mode::Write).await {
        Ok(value) => value,
        Err(error) => {
            return Err(crate::routes::chats::failed(conn, principal, Some(chat.id), error).await);
        }
    };
    let outsiders = setting::<bool>(conn, Place::Node(chat.org_id), keys::SHARING_ALLOW_EXTERNAL)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(true);
    match chats::add_member(conn, &act, &mut loaded, user, outsiders).await {
        Ok(()) => Ok(()),
        Err(error) => {
            Err(crate::routes::chats::failed(conn, principal, Some(chat.id), error).await)
        }
    }
}

async fn grant(mut ctx: Ctx, (): (), body: ChatPersonGrantBody) -> ApiResult<()> {
    if !matches!(
        body.choice,
        AccessChoice::FullView | AccessChoice::FullInteract
    ) {
        return Err(invalid("choice", "is view or edit"));
    }
    let (id, user) = (ctx.id("id")?, ctx.id("user_id")?);
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    person::lock_sender(&mut tx, principal.user()).await?;
    let target = person::target(&mut tx, "chats", id).await?;
    if !person::reads(&mut tx, principal, &target).await? {
        return Err(ApiError::not_found());
    }
    let key = ctx
        .headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let path = format!("/chats/{id}/permissions/people/{user}");
    let written = serde_json::to_value(&body).map_err(ApiError::internal)?;
    if let Some(key) = key {
        let hours =
            i64::try_from(ctx.app.config.api.idempotency_ttl_hours).map_err(ApiError::internal)?;
        if let Some(previous) = orch_db::idempotency::find(
            &mut tx,
            principal.user(),
            key,
            ctx.now - chrono::Duration::hours(hours),
        )
        .await?
        {
            if previous.method != "POST" || previous.path != path || previous.response != written {
                return Err(ApiError::new(
                    ErrorCode::ConflictIdempotencyMismatch,
                    "the key was used for another request",
                ));
            }
            tx.commit().await.map_err(DbError::from)?;
            return Ok(());
        }
    }
    person::destination(&mut tx, &target, body.anchor).await?;
    let chat = rows::chat(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    orch_db::queries::transaction_guard::watch_standing(&mut tx).await?;
    let can_read = person::person_reads(&mut tx, &target, user, principal.now()).await?;
    let human = orch_db::queries::identity::user_by_id(&mut tx, user)
        .await?
        .is_some_and(|row| row.kind == "human" && row.status == "active");
    let (_, choices) = choices(&ctx, &mut tx, principal, &chat, user, can_read, human).await?;
    if !choices.contains(&body.choice) {
        return Err(invalid("choice", "is not available for this person"));
    }
    if let Some(node) = chat.node_id {
        give_node_chat(&ctx, &mut tx, principal, node, user, body.choice).await?;
    } else if chat.kind == "group" {
        give_group_chat(&ctx, &mut tx, principal, &chat, user, body.choice).await?;
    } else {
        return Err(invalid("choice", "a direct chat cannot add another person"));
    }
    if body.notify {
        person::tell(
            &ctx,
            &mut tx,
            principal,
            "chats",
            id,
            &[user],
            body.anchor,
            None,
        )
        .await?;
    }
    if let Some(key) = key {
        orch_db::idempotency::keep(
            &mut tx,
            principal.user(),
            key,
            &orch_db::idempotency::Replay {
                method: "POST".to_owned(),
                path,
                status: 204,
                response: written,
            },
            ctx.now,
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
