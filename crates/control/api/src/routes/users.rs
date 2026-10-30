//! Users, and invitations into an org at any level, to one place or the system's: a link to share or a personal link, within what the root org allows.
//!
//! People are found by name among those who share an org with the caller,
//! and by an exact username or address anywhere, never all of them. An
//! invitation is a link: a shared one that up to a number of people may use,
//! or a personal one for one address. What a root org allows (which forms,
//! how long, how many) is its policy, resolved where the invitation points;
//! accepting an org invitation makes the org the person's domain, a node
//! invitation names them on one place's page without making them a member of
//! anything. Whoever made one must still be able to name an outsider where it
//! points at the moment it is accepted. A group invitation adds a guest to that chat alone.

use crate::middleware::authentication::{new_token, token_hash};
use crate::middleware::authorization::{self, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::access::admin_matters;
use crate::routes::nodes::{Placed, invalid, placed, shown, view};
use crate::sign_in::Limits;
use crate::sign_in::mail::{self, CHANNEL_KIND};
use crate::sign_in::throttle;
use chrono::{DateTime, Duration, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::pages as stored;
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::Role;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{Node, NodeKind, NodeRef};
use orch_api_contract::users::{
    Invitation, InvitationAccept, InvitationAcceptBody, InvitationAcceptReply, InvitationCreate,
    InvitationCreateBody, InvitationCreateReply, InvitationDelete, InvitationForm, InvitationKind,
    InvitationList, InvitationListQuery, InvitationLookup, InvitationPreview, InvitationPreviewOrg,
    InvitationPreviewWhat, USER_SEARCH_MAX_RESULTS, USER_SEARCH_MIN_CHARS, UserGet, UserRef,
    UserSearch, UserSearchQuery,
};
use orch_authz::pages::{self, Target};
use orch_authz::{Act, Caller, Policy, Principal, epoch, nodes};
use orch_db::queries::identity::{self, InvitationRow, NewInvitation};
use orch_db::queries::{access, hierarchy};
use orch_db::{Conn, DbError};
use orch_settings::{Place, resolve};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time, new_id};
use serde_json::json;

/// What an invitation's secret begins with.
const INVITATION_TOKEN_PREFIX: &str = "oinv_";
/// The most characters of the note of an invitation.
const NOTE_MAX_CHARS: usize = 500;
/// The rows of a node's page that a node invitation names its person in
/// when it names none: the node's chat alone.
const DEFAULT_ROWS: [&str; 2] = ["chat.view", "chat.interact"];

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(
            orch_api_contract::auth::AuthNameAvailabilityGet,
            crate::sign_in::accounts::name_availability,
        )
        .on(UserSearch, user_search)
        .on(UserGet, user_get)
        .on(InvitationCreate, invitation_create)
        .on(InvitationList, invitation_list)
        .on(InvitationDelete, invitation_delete)
        .on(InvitationLookup, invitation_lookup)
        .on(InvitationAccept, invitation_accept);
}

/// People whose name starts or holds what is asked among those who share an
/// org with the caller, and by an exact username or address anywhere.
async fn user_search(ctx: Ctx, query: UserSearchQuery, (): ()) -> ApiResult<Vec<UserRef>> {
    let text = query.q.trim();
    if text.chars().count() < usize::try_from(USER_SEARCH_MIN_CHARS).unwrap_or(2) {
        return Err(invalid(
            "q",
            &format!("must be at least {USER_SEARCH_MIN_CHARS} characters"),
        ));
    }
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    if let Some(org) = query.org_id {
        let (_, kind) = placed(&mut conn, org).await?;
        if kind != NodeKind::Org {
            return Err(invalid("org_id", "is no org"));
        }
        authorization::require(
            &mut conn,
            &principal,
            org,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
        )
        .await?;
    }
    Ok(identity::search_users(
        &mut conn,
        principal.user(),
        text,
        query.org_id,
        i64::from(USER_SEARCH_MAX_RESULTS),
    )
    .await?
    .iter()
    .map(refusals::user_ref)
    .collect())
}

/// One person as others see them.
async fn user_get(ctx: Ctx, (): (), (): ()) -> ApiResult<UserRef> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    refusals::user_refs(&mut conn, &[id])
        .await?
        .remove(&id)
        .ok_or_else(ApiError::not_found)
}

fn form_word(form: InvitationForm) -> &'static str {
    match form {
        InvitationForm::Link => "link",
        InvitationForm::Email => "email",
    }
}

/// Rejects ambiguous targets and grant shapes instead of silently ignoring them.
fn invitation_scope(body: &InvitationCreateBody) -> ApiResult<()> {
    for (field, present, permitted) in [
        (
            "org_id",
            body.org_id.is_some(),
            body.kind == InvitationKind::Org,
        ),
        (
            "node_id",
            body.node_id.is_some(),
            body.kind == InvitationKind::Node,
        ),
        (
            "chat_id",
            body.chat_id.is_some(),
            body.kind == InvitationKind::Chat,
        ),
    ] {
        if present && !permitted {
            return Err(invalid(field, "is not a target of this invitation kind"));
        }
    }
    if body.role.is_some() && matches!(body.kind, InvitationKind::System | InvitationKind::Chat) {
        return Err(invalid("role", "this kind of invitation grants no role"));
    }
    if body.permissions.is_some() && body.kind != InvitationKind::Node {
        return Err(invalid(
            "permissions",
            "only a node invitation grants permission rows",
        ));
    }
    if body.role.is_some() && body.permissions.is_some() {
        return Err(invalid(
            "permissions",
            "choose a role or permission rows, rather than both",
        ));
    }
    Ok(())
}

/// A group whose current member may still invite outsiders; an inspector never becomes a member.
async fn invitable_chat(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<(orch_chat::chats::Loaded, bool)> {
    let origin = orch_db::audit::Origin::default();
    let acting = orch_chat::Acting {
        principal,
        now: principal.now().get(),
        origin: &origin,
    };
    let loaded = match orch_chat::chats::load(conn, &acting, id, orch_chat::chats::Mode::Read).await
    {
        Ok(loaded) => loaded,
        Err(error) => {
            return Err(crate::routes::chats::failed(conn, principal, Some(id), error).await);
        }
    };
    if loaded.row.kind != orch_db::queries::chats::KIND_GROUP {
        return Err(invalid("chat_id", "must be a group chat"));
    }
    if !matches!(
        loaded.standing.reading,
        Some(orch_authz::chats::Reading::Member)
    ) {
        return Err(ApiError::not_found());
    }
    placed(conn, loaded.row.org_id).await?;
    if let Err(error) = loaded.ensure_send() {
        return Err(crate::routes::chats::failed(conn, principal, Some(id), error).await);
    }
    let outsiders = orch_settings::get::<bool>(
        conn,
        Place::Node(loaded.row.org_id),
        keys::SHARING_ALLOW_EXTERNAL,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(true);
    let naming = orch_authz::chats::may_bring_in_outsider(conn, principal, &loaded.row, outsiders)
        .await
        .map_err(ApiError::internal)?;
    if !naming.allowed() {
        return Err(refusals::external_forbidden(conn, &naming.ask, naming.ask_more).await);
    }
    Ok((loaded, outsiders))
}

/// A chat reference built only after its reader passes the chat's current access check.
async fn permitted_chat(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Option<orch_api_contract::chats::ChatRef>> {
    let origin = orch_db::audit::Origin::default();
    let acting = orch_chat::Acting {
        principal,
        now: principal.now().get(),
        origin: &origin,
    };
    let loaded = match orch_chat::chats::load(conn, &acting, id, orch_chat::chats::Mode::Read).await
    {
        Ok(loaded) => loaded,
        Err(error) => {
            let error = crate::routes::chats::failed(conn, principal, Some(id), error).await;
            return if matches!(
                error.code(),
                ErrorCode::AuthzDenied | ErrorCode::NotFound | ErrorCode::OrgSuspended
            ) {
                Ok(None)
            } else {
                Err(error)
            };
        }
    };
    Ok(Some(
        crate::routes::chats::chat_view(
            conn,
            principal,
            &loaded.row,
            &loaded.members,
            &loaded.standing,
        )
        .await?
        .base,
    ))
}

/// What the policies of the place an invitation points at allow.
struct Policies {
    forms: Vec<String>,
    forms_scope: Option<NodeRef>,
    default_hours: i64,
    max_hours: i64,
    max_uses: i64,
}

async fn policies_at(conn: &mut Conn, place: Place) -> ApiResult<Policies> {
    let get = |view: orch_settings::SettingView| view;
    let forms = resolve(conn, place, keys::INVITATIONS_FORMS)
        .await
        .map(get)
        .map_err(ApiError::internal)?;
    let scope = match forms.resolved.source.node {
        Some(node) => refusals::named_node(conn, node).await.ok(),
        None => None,
    };
    let number = async |conn: &mut Conn, key: &str| -> ApiResult<i64> {
        resolve(conn, place, key)
            .await
            .map_err(ApiError::internal)?
            .value()
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| ApiError::internal(format!("{key} is not a number")))
    };
    Ok(Policies {
        forms: forms
            .value()
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default(),
        forms_scope: scope,
        default_hours: number(conn, keys::INVITATIONS_DEFAULT_HOURS).await?,
        max_hours: number(conn, keys::INVITATIONS_MAX_HOURS).await?,
        max_uses: number(conn, keys::INVITATIONS_MAX_USES).await?,
    })
}

/// The address of a personal link, as it is kept: trimmed and in lower
/// case, with something on both sides of one `@`.
fn clean_email(email: &str) -> ApiResult<String> {
    let email = email.trim().to_lowercase();
    match email.split_once('@') {
        Some((name, host)) if !name.is_empty() && host.contains('.') && !host.contains('@') => {
            Ok(email)
        }
        _ => Err(invalid("email", "is not an address")),
    }
}

/// An invitation as the people who manage it read it.
async fn invitations_shown(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[InvitationRow],
) -> ApiResult<Vec<Invitation>> {
    let mut places: Vec<Id> = rows.iter().filter_map(|row| row.org_id).collect();
    places.extend(rows.iter().filter_map(|row| row.node_id));
    let refs = view::refs(conn, principal, &places, Policy::default()).await?;
    let makers: Vec<Id> = rows.iter().map(|row| row.created_by).collect();
    let people = refusals::user_refs(conn, &makers).await?;
    let mut chats = std::collections::HashMap::new();
    for chat in rows.iter().filter_map(|row| row.chat_id) {
        if let Some(shown) = permitted_chat(conn, principal, chat).await? {
            chats.insert(chat, shown);
        }
    }
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(Invitation {
                id: row.id,
                kind: kind_of(&row.kind)?,
                org: row.org_id.and_then(|org| refs.get(&org).cloned()),
                node: row.node_id.and_then(|node| refs.get(&node).cloned()),
                chat: row.chat_id.and_then(|chat| chats.get(&chat).cloned()),
                role: row
                    .role
                    .as_deref()
                    .and_then(|role| serde_json::from_value(json!(role)).ok()),
                permissions: row.permissions.clone(),
                form: if row.email.is_some() {
                    InvitationForm::Email
                } else {
                    InvitationForm::Link
                },
                email: row.email.clone(),
                max_uses: i64::from(row.max_uses),
                uses: i64::from(row.uses),
                note: row.note.clone(),
                expires_at: Time::new(row.expires_at),
                revoked: row.revoked_at.is_some(),
                created_by: people.get(&row.created_by)?.clone(),
                created_at: Time::new(row.created_at),
            })
        })
        .collect())
}

fn kind_of(word: &str) -> Option<InvitationKind> {
    match word {
        "system" => Some(InvitationKind::System),
        "org" => Some(InvitationKind::Org),
        "node" => Some(InvitationKind::Node),
        "chat" => Some(InvitationKind::Chat),
        _ => None,
    }
}

/// Makes an invitation.
async fn invitation_create(
    mut ctx: Ctx,
    (): (),
    body: InvitationCreateBody,
) -> ApiResult<InvitationCreateReply> {
    invitation_scope(&body)?;
    let email = body.email.as_deref().map(clean_email).transpose()?;
    let note = body.note.clone().unwrap_or_default();
    if note.chars().count() > NOTE_MAX_CHARS {
        return Err(invalid(
            "note",
            &format!("must be at most {NOTE_MAX_CHARS} characters"),
        ));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    let user = principal.user();
    // Where it points, and what that asks of the maker.
    let (kind_word, org, node, place, audit_place): (
        &str,
        Option<Id>,
        Option<Id>,
        Place,
        Option<Placed>,
    ) = match body.kind {
        InvitationKind::System => {
            if !principal.is_system_admin() {
                return Err(refusals::system_admin_only(
                    "only a system admin invites to the system",
                ));
            }
            ("system", None, None, Place::System, None)
        }
        InvitationKind::Org => {
            let org = body
                .org_id
                .ok_or_else(|| invalid("org_id", "is needed to invite into an org"))?;
            let (audit_place, kind) = placed(&mut tx, org).await?;
            if kind != NodeKind::Org {
                return Err(invalid("org_id", "is no org"));
            }
            authorization::require(
                &mut tx,
                principal,
                org,
                exact(Permission::MembersManage),
                Act::Use,
                Policy::default(),
            )
            .await?;
            if orch_authz::orgs::limits(&mut tx, audit_place.org)
                .await
                .map_err(ApiError::internal)?
                .is_some_and(|limits| limits.closed)
            {
                return Err(ApiError::new(
                    ErrorCode::OrgClosed,
                    "that tree takes no further member",
                )
                .with("org", refusals::named_node(&mut tx, audit_place.org).await?));
            }
            (
                "org",
                Some(org),
                None,
                Place::Node(audit_place.org),
                Some(audit_place),
            )
        }
        InvitationKind::Node => {
            let node = body
                .node_id
                .ok_or_else(|| invalid("node_id", "is needed to invite to a node"))?;
            let (audit_place, kind) = placed(&mut tx, node).await?;
            if kind == NodeKind::Org {
                return Err(invalid("node_id", "an org is invited into, not to"));
            }
            let policy = crate::routes::nodes::policy_at(&mut tx, node).await?;
            for permission in [Permission::AccessManage, Permission::AccessExternal] {
                authorization::require(
                    &mut tx,
                    principal,
                    node,
                    exact(permission),
                    Act::Use,
                    policy,
                )
                .await?;
            }
            (
                "node",
                None,
                Some(node),
                Place::Node(audit_place.org),
                Some(audit_place),
            )
        }
        InvitationKind::Chat => {
            let chat = body
                .chat_id
                .ok_or_else(|| invalid("chat_id", "is needed to invite to a group chat"))?;
            let (loaded, _) = invitable_chat(&mut tx, principal, chat).await?;
            let root = loaded.row.org_id;
            (
                "chat",
                None,
                None,
                Place::Node(root),
                Some(Placed {
                    id: root,
                    org: root,
                }),
            )
        }
    };
    let policies = policies_at(&mut tx, place).await?;
    let form = if email.is_some() {
        InvitationForm::Email
    } else {
        InvitationForm::Link
    };
    if !policies
        .forms
        .iter()
        .any(|allowed| allowed == form_word(form))
    {
        return Err(ApiError::new(
            ErrorCode::InvitationNotAllowed,
            "this form of invitation is not allowed here",
        )
        .with("form", form_word(form))
        .with("scope", policies.forms_scope));
    }
    let max_uses = match (form, body.max_uses) {
        (InvitationForm::Email, None | Some(1)) => 1,
        (InvitationForm::Email, Some(_)) => {
            return Err(invalid("max_uses", "a personal link is good for one use"));
        }
        (InvitationForm::Link, requested) => {
            let uses = requested.unwrap_or(1);
            if uses < 1 || uses > policies.max_uses {
                return Err(invalid(
                    "max_uses",
                    &format!("must be from 1 to {}", policies.max_uses),
                ));
            }
            uses
        }
    };
    let hours = body.expires_in_hours.unwrap_or(policies.default_hours);
    if hours < 1 || hours > policies.max_hours {
        return Err(invalid(
            "expires_in_hours",
            &format!("must be from 1 to {}", policies.max_hours),
        ));
    }
    let (role, rows): (Option<String>, Vec<String>) = match body.kind {
        InvitationKind::Org => {
            let role = body.role.unwrap_or(Role::Viewer);
            if role == Role::Admin
                && let Some(org) = org
            {
                admin_matters(&mut tx, principal, org, Policy::default()).await?;
            }
            (
                serde_json::to_value(role)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned)),
                Vec::new(),
            )
        }
        InvitationKind::Node => {
            let node = node.unwrap_or_else(Id::nil);
            let (_, kind) = placed(&mut tx, node).await?;
            let rows: Vec<String> = body
                .permissions
                .clone()
                .unwrap_or_else(|| DEFAULT_ROWS.iter().map(|row| (*row).to_owned()).collect());
            for (index, row) in rows.iter().enumerate() {
                let known = row.parse::<Permission>().ok().is_some_and(|permission| {
                    permission.exists_on(match kind {
                        NodeKind::Project => orch_access_contract::permissions::ObjectKind::Project,
                        NodeKind::Topic => orch_access_contract::permissions::ObjectKind::Topic,
                        NodeKind::Thread => orch_access_contract::permissions::ObjectKind::Thread,
                        NodeKind::Org => orch_access_contract::permissions::ObjectKind::Org,
                    })
                });
                if !known {
                    return Err(invalid(
                        &format!("permissions[{index}]"),
                        "is no permission of that node",
                    ));
                }
            }
            if body.role.is_some() && kind == NodeKind::Thread {
                return Err(invalid(
                    "role",
                    "a role is held in a project or a topic, not on a thread",
                ));
            }
            if let Some(role) = body.role {
                let policy = crate::routes::nodes::policy_at(&mut tx, node).await?;
                let standing = orch_authz::giving::role_standing(&mut tx, principal, node, policy)
                    .await
                    .map_err(ApiError::internal)?;
                crate::routes::access::role_matters(
                    &mut tx, principal, node, role, &standing, policy,
                )
                .await?;
            }
            (
                body.role
                    .and_then(|role| serde_json::to_value(role).ok())
                    .and_then(|v| v.as_str().map(str::to_owned)),
                rows,
            )
        }
        _ => (None, Vec::new()),
    };
    let token = new_token(INVITATION_TOKEN_PREFIX);
    let id = new_id();
    let expires_at = Duration::try_hours(hours)
        .and_then(|life| ctx.now.checked_add_signed(life))
        .ok_or_else(|| invalid("expires_in_hours", "is outside the supported clock range"))?;
    identity::create_invitation(
        &mut tx,
        &NewInvitation {
            id,
            token_hash: &token_hash(&token),
            kind: kind_word,
            org,
            node,
            chat: body.chat_id,
            role: role.as_deref(),
            permissions: &rows,
            email: email.as_deref(),
            max_uses: i32::try_from(max_uses).unwrap_or(i32::MAX),
            note: &note,
            expires_at,
            by: user,
            as_admin: principal.caller().as_admin(),
            at: ctx.now,
        },
    )
    .await?;
    let url = mail::link(&ctx.app.config, "invite", &token);
    if let Some(address) = &email
        && let Some(invitee) = identity::user_by_confirmed_email(&mut tx, address).await?
    {
        orch_engine::notifications::tell(&mut tx, &orch_engine::notifications::Event {
            kind:orch_engine::notifications::Kind::Invitation, node:None, thread:None, approval:None,
            title:format!("{} invited you", guard.user().user.display_name), body:note.clone(),
            data:json!({"invitation_id":id,"kind":kind_word,"expires_at":Time::new(expires_at),"path":format!("/invite/{token}")}),
            source:id.to_string(), actor:Some(user), at:ctx.now,
        }, &[invitee.id], None).await.map_err(ApiError::internal)?;
    }
    let mut sent = false;
    if let Some(address) = &email
        && let Some(channel) = identity::system_mail_channel(&mut tx).await?
    {
        let system = &ctx.app.config.system.name;
        let who = &guard.user().user.display_name;
        identity::queue_mail(
            &mut tx,
            CHANNEL_KIND,
            channel,
            &json!({
                "purpose": "invitation",
                "to": address,
                "user_id": null,
                "language": "en",
                "data": { "token": token, "url": url, "expires_at": Time::new(expires_at) },
                "subject": format!("{who} invited you to {system}"),
                "body": format!("{who} invited you. Open this link:\n{url}\nIt is good for one use, until {}.", expires_at.to_rfc3339()),
            }),
            ctx.now,
        )
        .await?;
        sent = true;
    }
    let who = &guard.user().user.display_name;
    let audit_place = match audit_place {
        Some(place) => place,
        None => Placed {
            id: Id::nil(),
            org: Id::nil(),
        },
    };
    audit_invitation(
        (&ctx, ctx.user()?.id()),
        &mut tx,
        orch_db::audit::action!(AuditAction::InviteCreated),
        id,
        audit_place,
        &format!("{who} made an invitation"),
        json!({ "kind": kind_word, "form": form_word(form), "uses": max_uses, "role": role }),
    )
    .await?;
    let row = identity::invitation_by_id(&mut tx, id)
        .await?
        .ok_or_else(|| ApiError::internal("an invitation was made and is not there"))?;
    let shown = invitations_shown(&mut tx, principal, &[row]).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(InvitationCreateReply {
        invitation: shown
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::internal("an invitation was made and is not shown"))?,
        token,
        url,
        sent,
    })
}

/// The audit row of an invitation: its object is the invitation, and it
/// stands beside the place it points at.
async fn audit_invitation(
    (ctx, actor): (&Ctx, Id),
    conn: &mut Conn,
    action: AuditAction,
    invitation: Id,
    place: Placed,
    summary: &str,
    details: serde_json::Value,
) -> ApiResult<()> {
    use orch_db::audit::{self, Actor, Entry};
    let mut entry = Entry::new(
        Actor::User(actor),
        action.as_str(),
        action.object_kind().unwrap_or("invite"),
        Some(invitation),
        summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = (!place.id.is_nil()).then_some(place.id);
    entry.org_id = (!place.org.is_nil()).then_some(place.org);
    entry.details = details;
    audit::append(conn, &entry, &crate::sign_in::origin(ctx), ctx.now).await?;
    Ok(())
}

/// What the maker of an invitation has to hold to list or take one back.
async fn may_manage(conn: &mut Conn, principal: &Principal, row: &InvitationRow) -> ApiResult<()> {
    match (row.kind.as_str(), row.org_id, row.node_id) {
        ("system", ..) => {
            if principal.is_system_admin() {
                Ok(())
            } else {
                Err(refusals::system_admin_only(
                    "only a system admin manages the system's invitations",
                ))
            }
        }
        ("org", Some(org), _) => {
            authorization::require(
                conn,
                principal,
                org,
                exact(Permission::MembersManage),
                Act::Use,
                Policy::default(),
            )
            .await
        }
        ("node", _, Some(node)) => {
            let policy = crate::routes::nodes::policy_at(conn, node).await?;
            for permission in [Permission::AccessManage, Permission::AccessExternal] {
                authorization::require(conn, principal, node, exact(permission), Act::Use, policy)
                    .await?;
            }
            Ok(())
        }
        ("chat", _, _) => {
            let chat = row.chat_id.ok_or_else(ApiError::not_found)?;
            invitable_chat(conn, principal, chat).await?;
            Ok(())
        }
        _ => Err(ApiError::not_found()),
    }
}

/// The invitations of a place, newest first.
async fn invitation_list(
    ctx: Ctx,
    query: InvitationListQuery,
    (): (),
) -> ApiResult<Vec<Invitation>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    if let Some(org) = query.org_id {
        let (_, kind) = placed(&mut conn, org).await?;
        if kind != NodeKind::Org {
            return Err(invalid("org_id", "is no org"));
        }
        authorization::require(
            &mut conn,
            &principal,
            org,
            exact(Permission::MembersManage),
            Act::Use,
            Policy::default(),
        )
        .await?;
    }
    let rows = identity::invitations_of(
        &mut conn,
        query.org_id,
        principal.is_system_admin(),
        principal.user(),
    )
    .await?;
    invitations_shown(&mut conn, &principal, &rows).await
}

/// Takes an invitation back.
async fn invitation_delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    let row = identity::invitation_by_id(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    may_manage(&mut tx, principal, &row).await?;
    if identity::revoke_invitation(&mut tx, id, ctx.now).await? {
        let target = match row.chat_id {
            Some(chat) => orch_db::queries::chats::chat(&mut tx, chat)
                .await?
                .map(|chat| chat.org_id),
            None => row.org_id.or(row.node_id),
        };
        let place = match target {
            Some(node) => placed(&mut tx, node).await?.0,
            None => Placed {
                id: Id::nil(),
                org: Id::nil(),
            },
        };
        let who = &guard.user().user.display_name;
        audit_invitation(
            (&ctx, ctx.user()?.id()),
            &mut tx,
            orch_db::audit::action!(AuditAction::InviteRevoked),
            id,
            place,
            &format!("{who} took an invitation back"),
            json!({ "kind": row.kind }),
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

fn gone() -> ApiError {
    ApiError::new(
        ErrorCode::Gone,
        "this invitation has ended, or was used up or taken back",
    )
}

/// The invitation whose secret is `token`, if it is still good.
async fn good(conn: &mut Conn, token: &str, now: DateTime<Utc>) -> ApiResult<InvitationRow> {
    let row = identity::invitation_by_hash(conn, &token_hash(token))
        .await?
        .ok_or_else(gone)?;
    if row.revoked_at.is_some() || row.expires_at <= now || row.uses >= row.max_uses {
        return Err(gone());
    }
    Ok(row)
}

/// What a link would give, before it is accepted.
async fn invitation_lookup(ctx: Ctx, (): (), (): ()) -> ApiResult<InvitationPreview> {
    let token = ctx.param("token")?.to_owned();
    let mut conn = ctx.app.db.acquire().await?;
    let limits = Limits::of(&ctx.app.config.auth);
    throttle::by_address(&ctx, &mut conn, &limits, "invitation").await?;
    let row = good(&mut conn, &token, ctx.now).await?;
    let found = hierarchy::with_ancestors(
        &mut conn,
        &[row.org_id.or(row.node_id).unwrap_or_else(Id::nil)],
    )
    .await?;
    let place = found
        .iter()
        .find(|node| Some(node.id) == row.org_id.or(row.node_id));
    let org = match (row.org_id, place) {
        (Some(_), Some(node)) => {
            hierarchy::org(&mut conn, node.id)
                .await?
                .map(|org| InvitationPreviewOrg {
                    name: node.name.clone(),
                    slug: org.slug,
                })
        }
        _ => None,
    };
    let what = match (row.node_id, place) {
        (Some(_), Some(node)) => {
            refusals::node_kind(&node.kind).map(|kind| InvitationPreviewWhat {
                kind: kind.into(),
                name: node.name.clone(),
            })
        }
        _ => None,
    };
    if let Some(target) = row.org_id.or(row.node_id) {
        placed(&mut conn, target).await.map_err(|_| gone())?;
    }
    let what = if let Some(chat) = row.chat_id {
        if row.role.is_some() || !row.permissions.is_empty() {
            return Err(gone());
        }
        let maker = Principal::load(&mut conn, issuer_caller(&row), Time::new(ctx.now))
            .await
            .map_err(|_| gone())?;
        let (loaded, _) = invitable_chat(&mut conn, &maker, chat)
            .await
            .map_err(|_| gone())?;
        Some(InvitationPreviewWhat {
            kind: orch_api_contract::users::InvitedKind::Chat,
            name: loaded.row.name.unwrap_or_else(|| "Group chat".to_owned()),
        })
    } else {
        what
    };
    let inviter = refusals::user_refs(&mut conn, &[row.created_by])
        .await?
        .remove(&row.created_by)
        .ok_or_else(gone)?;
    Ok(InvitationPreview {
        kind: kind_of(&row.kind).ok_or_else(gone)?,
        org,
        what,
        role: row
            .role
            .as_deref()
            .and_then(|role| serde_json::from_value(json!(role)).ok()),
        permissions: row.permissions.clone(),
        inviter,
        email_bound: row.email.is_some(),
        expires_at: Time::new(row.expires_at),
    })
}

/// Accepts an invitation as the existing signed-in person.
async fn invitation_accept(
    mut ctx: Ctx,
    (): (),
    body: InvitationAcceptBody,
) -> ApiResult<InvitationAcceptReply> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let row = redeem(&ctx, &mut tx, &guard.user().user, &body.token).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let reply = accepted_reply(&ctx, &mut tx, &principal, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(reply)
}

/// The authority mode the issuer delegated, independent of the accepting
/// browser's mode. A legacy system offer already implies administrative admission;
/// an older target offer without mode evidence retains ordinary permissions only.
fn issuer_caller(row: &InvitationRow) -> Caller {
    Caller::session(row.created_by)
        .with_admin_mode(row.created_as_admin.unwrap_or(row.kind == "system"))
}

/// An admission offer for registration. A system invitation admits an account
/// independently; a target invitation requires explicit acceptance composition.
pub(crate) async fn registration_offer(
    conn: &mut Conn,
    token: Option<&str>,
    accept: bool,
    now: DateTime<Utc>,
) -> ApiResult<Option<InvitationRow>> {
    let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) else {
        return if accept {
            Err(ApiError::invalid(
                "invitation",
                "is required when accepting an invitation",
            ))
        } else {
            Ok(None)
        };
    };
    let row = identity::invitation_by_hash(conn, &token_hash(token))
        .await?
        .filter(|row| row.revoked_at.is_none() && row.expires_at > now && row.uses < row.max_uses);
    if accept && row.is_none() {
        return Err(ApiError::new(
            ErrorCode::RegistrationInviteRequired,
            "the invitation to accept is no longer available",
        ));
    }
    Ok(row.filter(|row| accept || row.kind == "system"))
}

/// Redeems the invitation for one existing identity in the caller's transaction.
/// Registration may compose this with creating that identity. This helper does
/// not create accounts, personal organizations, sessions or a second access model.
pub(crate) async fn redeem(
    ctx: &Ctx,
    conn: &mut Conn,
    recipient: &identity::UserRow,
    token: &str,
) -> ApiResult<InvitationRow> {
    let row = good(conn, token, ctx.now).await?;
    // A personal link is accepted by the address it was sent to.
    if let Some(address) = &row.email {
        let confirmed = recipient
            .email
            .as_deref()
            .filter(|_| recipient.email_confirmed())
            .map(str::to_lowercase);
        if confirmed.as_deref() != Some(address.as_str()) {
            return Err(ApiError::new(
                ErrorCode::RegistrationInviteRequired,
                "this invitation is for the address it was sent to",
            ));
        }
    }
    let user = recipient.id;
    orch_db::queries::transaction_guard::watch_standing(conn).await?;
    let maker = Principal::load(conn, issuer_caller(&row), Time::new(ctx.now))
        .await
        .map_err(|_| gone())?;
    if maker.suspension().applies(&[]) {
        return Err(gone());
    }
    let delegation =
        json!({"issuer":row.created_by,"administrator_mode":maker.caller().as_admin()});
    match (row.kind.as_str(), row.org_id, row.node_id) {
        ("org", Some(org), _) => {
            let (place, _) = placed(&mut *conn, org).await.map_err(|_| gone())?;
            authorization::require(
                &mut *conn,
                &maker,
                org,
                exact(Permission::MembersManage),
                Act::Use,
                Policy::default(),
            )
            .await?;
            if row.role.as_deref() == Some("admin") {
                admin_matters(&mut *conn, &maker, org, Policy::default()).await?;
            }
            let root = place.org;
            if orch_authz::orgs::limits(&mut *conn, root)
                .await
                .map_err(ApiError::internal)?
                .is_some_and(|limits| limits.closed)
            {
                return Err(ApiError::new(
                    ErrorCode::OrgClosed,
                    "that tree takes no further member",
                )
                .with("org", refusals::named_node(&mut *conn, root).await?));
            }
            let memberships = access::memberships(&mut *conn, user).await?;
            if let Some(membership) = memberships.iter().find(|each| each.org_id == root) {
                return Err(ApiError::new(
                    ErrorCode::MemberExists,
                    "they are a member of that tree already",
                )
                .with(
                    "domain",
                    refusals::named_node(&mut *conn, membership.domain_id).await?,
                ));
            }
            if !identity::use_invitation(&mut *conn, row.id, user, ctx.now).await? {
                return Err(gone());
            }
            access::add_member(&mut *conn, root, user, org, Some(row.created_by), ctx.now).await?;
            let role = row.role.clone().unwrap_or_else(|| "viewer".to_owned());
            access::put_role(
                &mut *conn,
                &access::NewRole {
                    id: new_id(),
                    node: org,
                    subject_kind: "user",
                    subject: user,
                    subject_role: None,
                    role: &role,
                    expires_at: None,
                    note: "",
                    by: row.created_by,
                    at: ctx.now,
                },
            )
            .await?;
            epoch::bump(&mut *conn).await.map_err(ApiError::internal)?;
            audit_invitation(
                (ctx, user),
                &mut *conn,
                orch_db::audit::action!(AuditAction::InviteAccepted),
                row.id,
                place,
                &format!("{} accepted an invitation", recipient.display_name),
                json!({ "kind": "org", "org": org, "role": role, "delegation": delegation }),
            )
            .await?;
        }
        ("node", _, Some(node)) => {
            let (place, kind) = placed(&mut *conn, node).await.map_err(|_| gone())?;
            accept_on_node(ctx, conn, &maker, user, &row, node, kind).await?;
            if !identity::use_invitation(&mut *conn, row.id, user, ctx.now).await? {
                return Err(gone());
            }
            audit_invitation(
                (ctx, user),
                &mut *conn,
                AuditAction::InviteAccepted,
                row.id,
                place,
                &format!("{} accepted an invitation", recipient.display_name),
                json!({ "kind": "node", "node": node, "delegation": delegation }),
            )
            .await?;
        }
        ("system", ..) => {
            if !maker.is_system_admin() {
                return Err(gone());
            }
            if !identity::use_invitation(&mut *conn, row.id, user, ctx.now).await? {
                return Err(gone());
            }
            audit_invitation(
                (ctx, user),
                &mut *conn,
                AuditAction::InviteAccepted,
                row.id,
                Placed {
                    id: Id::nil(),
                    org: Id::nil(),
                },
                &format!("{} accepted an invitation", recipient.display_name),
                json!({ "kind": "system", "delegation": delegation }),
            )
            .await?;
        }
        ("chat", None, None) => {
            let chat = row.chat_id.ok_or_else(gone)?;
            if row.role.is_some() || !row.permissions.is_empty() {
                return Err(gone());
            }
            // A shared link counts different people; a replay must not consume another seat.
            let already_used: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM invitation_uses WHERE invitation_id = $1 AND user_id = $2)",
            ).bind(row.id).bind(user).fetch_one(&mut *conn).await.map_err(DbError::from)?;
            if already_used {
                return Err(gone());
            }
            let (mut loaded, outsiders) = invitable_chat(&mut *conn, &maker, chat).await?;
            let root = loaded.row.org_id;
            let origin = crate::sign_in::origin(ctx);
            let acting = orch_chat::Acting {
                principal: &maker,
                now: ctx.now,
                origin: &origin,
            };
            if let Err(error) =
                orch_chat::chats::add_member(&mut *conn, &acting, &mut loaded, user, outsiders)
                    .await
            {
                return Err(
                    crate::routes::chats::failed(&mut *conn, &maker, Some(chat), error).await,
                );
            }
            if !identity::use_invitation(&mut *conn, row.id, user, ctx.now).await? {
                return Err(gone());
            }
            epoch::bump(&mut *conn).await.map_err(ApiError::internal)?;
            audit_invitation(
                (ctx, user),
                &mut *conn,
                AuditAction::InviteAccepted,
                row.id,
                Placed {
                    id: root,
                    org: root,
                },
                &format!("{} accepted an invitation", recipient.display_name),
                json!({ "kind": "chat", "chat": chat, "delegation": delegation }),
            )
            .await?;
        }
        _ => return Err(gone()),
    };
    Ok(row)
}

/// Projects a redemption after open_session has persisted the actual session.
/// The same second-step holds apply as on subsequent authenticated reads.
pub(crate) async fn accepted_for_session(
    ctx: &Ctx,
    conn: &mut Conn,
    user: &identity::UserRow,
    session: Id,
    row: &InvitationRow,
) -> ApiResult<InvitationAcceptReply> {
    let actual = crate::middleware::authentication::Authed {
        user: user.clone(),
        via: crate::middleware::authentication::Via::Session {
            session_id: session,
        },
        as_admin: false,
    };
    let principal = Principal::load(
        conn,
        Caller::session(user.id).with_admin_mode(false),
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::internal)?;
    let principal = crate::sign_in::second_step::hold(conn, &actual, principal).await?;
    accepted_reply(ctx, conn, &principal, row).await
}

/// The ordinary readable route of an accepted invitation; no second redemption.
pub(crate) fn accepted_path(reply: &InvitationAcceptReply) -> String {
    if let Some(node) = &reply.node
        && !node.redacted
        && node.name.is_some()
    {
        let kind = match node.kind {
            NodeKind::Org => "org",
            NodeKind::Project => "project",
            NodeKind::Topic => "topic",
            NodeKind::Thread => "thread",
        };
        return format!("/{kind}/{}", node.id);
    }
    if let Some(chat) = &reply.chat {
        return format!("/org/{}/messages?chat={}", chat.org_id, chat.id);
    }
    if let Some(org) = &reply.org {
        return format!("/org/{}", org.address);
    }
    "/".to_owned()
}

/// Projects the accepted destination through the caller's current permissions.
pub(crate) async fn accepted_reply(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &InvitationRow,
) -> ApiResult<InvitationAcceptReply> {
    let org = match row.org_id {
        Some(org)
            if view::build(conn, principal, &[org], ctx.now, Policy::default())
                .await?
                .iter()
                .any(|built| built.viewable) =>
        {
            match shown(ctx, &mut *conn, principal, org).await? {
                Node::Org(org) => Some(org),
                _ => None,
            }
        }
        _ => None,
    };
    let mut node = None;
    let mut chat = None;
    if let Some(target) = row.node_id {
        if view::build(&mut *conn, principal, &[target], ctx.now, Policy::default())
            .await?
            .iter()
            .any(|built| built.viewable)
        {
            node = view::refs(&mut *conn, principal, &[target], Policy::default())
                .await?
                .remove(&target);
        } else if let Some(room) = orch_db::queries::chats::chat_of_node(&mut *conn, target).await?
        {
            chat = permitted_chat(&mut *conn, principal, room.id).await?;
        }
    } else if let Some(target) = row.chat_id {
        chat = permitted_chat(&mut *conn, principal, target).await?;
    }
    Ok(InvitationAcceptReply { org, node, chat })
}

/// Names the person on the node an invitation points at, as its maker
/// would: only if the maker may still name an outsider there.
async fn accept_on_node(
    ctx: &Ctx,
    conn: &mut Conn,
    maker: &Principal,
    user: Id,
    row: &InvitationRow,
    node: Id,
    kind: NodeKind,
) -> ApiResult<()> {
    let policy = crate::routes::nodes::policy_at(conn, node).await?;
    for permission in [Permission::AccessManage, Permission::AccessExternal] {
        match nodes::check(conn, maker, node, permission, Act::Use, policy).await {
            Ok(decision) if decision.is_allowed() => {}
            Ok(_) => {
                return Err(refusals::external_forbidden(conn, &[], 0)
                    .await
                    .with("required", permission.as_str())
                    .with("object", json!({ "kind": kind, "id": node })));
            }
            Err(error) => return Err(refusals::problem(conn, error).await),
        }
    }
    if let Some(role) = row.role.as_deref() {
        if kind == NodeKind::Thread {
            return Err(gone());
        }
        let wanted: Role = serde_json::from_value(json!(role)).map_err(ApiError::internal)?;
        let standing = orch_authz::giving::role_standing(conn, maker, node, policy)
            .await
            .map_err(ApiError::internal)?;
        crate::routes::access::role_matters(conn, maker, node, wanted, &standing, policy).await?;
        access::put_role(
            conn,
            &access::NewRole {
                id: new_id(),
                node,
                subject_kind: "user",
                subject: user,
                subject_role: None,
                role,
                expires_at: None,
                note: "",
                by: row.created_by,
                at: ctx.now,
            },
        )
        .await
        .map_err(|error| match error {
            DbError::Unique { .. } => ApiError::new(
                ErrorCode::ConflictExists,
                "you hold a role in that place already",
            ),
            other => other.into(),
        })?;
        epoch::bump(conn).await.map_err(ApiError::internal)?;
        return Ok(());
    }
    let rows: Vec<stored::RowChange> = row
        .permissions
        .iter()
        .map(|permission| stored::RowChange {
            permission: permission.clone(),
            profile: None,
            inherit: None,
            locked: None,
            add: Some(vec![stored::EntryAdd {
                holder: stored::Holder::User { id: user },
                options: None,
                expires_at: None,
                note: None,
            }]),
            update: None,
            remove: None,
        })
        .collect();
    match pages::change(
        conn,
        maker,
        Target::Live(node),
        &stored::PageChange {
            rows: Some(rows),
            ..stored::PageChange::default()
        },
        policy,
        &crate::sign_in::origin(ctx),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(error) => Err(refusals::change_failed(conn, error).await),
    }
}
