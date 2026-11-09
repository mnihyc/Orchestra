//! Gives one person access to a chat and optionally tells them in the same act.

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::access::subject;

use crate::routes::nodes::invalid;

use crate::routes::nodes::policy_at;

use crate::sign_in::origin;

use orch_access_contract::pages::EntryAdd;

use orch_access_contract::pages::Holder;

use orch_access_contract::pages::PageChange;

use orch_access_contract::pages::RowChange;

use orch_access_contract::permissions::Permission;

use orch_api_contract::access::AccessChoice;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::notifications::ChatPersonGrant;

use orch_api_contract::notifications::ChatPersonGrantBody;

use orch_authz::pages;

use orch_authz::pages::Target;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_chat::chats;

use orch_chat::chats::Mode;

use orch_db::queries::access;

use orch_db::queries::chats as rows;

use orch_db::queries::hierarchy;

use orch_db::queries::threads;

use orch_db::Conn;

use orch_db::DbError;

use orch_settings::Place;

use orch_settings::get as setting;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use super::person;
pub(super) fn mount(registry: &mut Registry) {
    registry
        .creation(
            ChatPersonGrant,
            crate::middleware::idempotency::ObservedCreation::<(), ChatPersonGrantBody, ()> {
                kind: "chat_person_grant",
                replay: |_ctx, (), _body, _created| Box::pin(async { Ok(()) }),
            },
        )
        .on_with_operation(
            ChatPersonGrant,
            crate::key_operations::native::effects::prepare::<operation::Grant>,
            grant,
        );
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
    boundary: &operation::Boundary<'_>,
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
    let changed = match pages::change_fenced(
        conn,
        principal,
        Target::Live(node),
        &change,
        policy,
        &origin(ctx),
        || boundary.check(),
    )
    .await
    {
        Ok(value) => value,
        Err(pages::EffectFailure::BeforeEffect(error)) => return Err(error),
        Err(pages::EffectFailure::Native(error)) => {
            return Err(refusals::change_failed(conn, error).await);
        }
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
    boundary: &operation::Boundary<'_>,
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
    let prepared = match chats::prepare_add_member(conn, &act, &mut loaded, user, outsiders, None)
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            return Err(crate::routes::chats::failed(conn, principal, Some(chat.id), error).await);
        }
    };
    match chats::apply_add_member_before(conn, &act, &mut loaded, prepared, || boundary.check())
        .await
    {
        Ok(()) => Ok(()),
        Err(pages::EffectFailure::BeforeEffect(error)) => Err(error),
        Err(pages::EffectFailure::Native(error)) => {
            Err(crate::routes::chats::failed(conn, principal, Some(chat.id), error).await)
        }
    }
}

async fn grant_on(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &operation::Value,
    boundary: &operation::Boundary<'_>,
) -> ApiResult<()> {
    let (id, user) = (value.chat.id, value.user);
    let body = &value.body;
    let target = person::Target::Chat(value.chat.clone());
    person::destination(conn, &target, body.anchor).await?;
    let chat = &value.chat;
    orch_db::queries::transaction_guard::watch_standing(conn).await?;
    let can_read = person::person_reads(conn, &target, user, principal.now()).await?;
    let human = orch_db::queries::identity::user_by_id(conn, user)
        .await?
        .is_some_and(|row| row.kind == "human" && row.status == "active");
    let (_, choices) = choices(ctx, conn, principal, chat, user, can_read, human).await?;
    if !choices.contains(&body.choice) {
        return Err(invalid("choice", "is not available for this person"));
    }
    if let Some(node) = chat.node_id {
        give_node_chat(ctx, conn, principal, node, user, body.choice, boundary).await?;
    } else if chat.kind == "group" {
        give_group_chat(ctx, conn, principal, chat, user, body.choice, boundary).await?;
    } else {
        return Err(invalid("choice", "a direct chat cannot add another person"));
    }
    if body.notify {
        person::tell_before(
            ctx,
            conn,
            principal,
            "chats",
            id,
            &[user],
            body.anchor,
            None,
            || boundary.check(),
        )
        .await?;
    }
    // The accepted native relationship is the real chat/person pair; the reply has no content.
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(principal.user()),
        "chat_person_grant",
        id,
        serde_json::json!({"person":user,"node":chat.node_id}),
    )
    .await?;
    Ok(())
}

async fn grant(ctx: Ctx, (): (), body: ChatPersonGrantBody) -> ApiResult<()> {
    if !matches!(
        body.choice,
        AccessChoice::FullView | AccessChoice::FullInteract
    ) {
        return Err(invalid("choice", "is view or edit"));
    }
    let keyed = crate::middleware::authorization::caller_of(ctx.user()?)?
        .grant()
        .is_some();
    let result: ApiResult<()> = async {
        let (mut tx, _) = transaction_guard::begin(&ctx).await?;
        let completed =
            crate::key_operations::native::effects::apply::<operation::Grant>(&ctx, &mut tx)
                .await?;
        tx.commit().await.map_err(DbError::from)?;
        let _: () = completed.output;
        Ok(())
    }
    .await;
    if !keyed
        || result
            .as_ref()
            .err()
            .is_some_and(|error: &ApiError| error.code() == ErrorCode::RequestInvalid)
    {
        return result;
    }
    // This native empty acknowledgement has no existing-content error projection.
    // Copied-subject Read cannot lend authority to named permission-page advice.
    crate::middleware::authorization::projected_result(ctx, async move { result }, true).await
}

#[path = "chat_grants/operation.rs"]
pub mod operation;
