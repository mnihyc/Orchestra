//! Account changes and invitation destinations remain explicit, private and bounded by current authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use chat_world::Room;
use chrono::Duration;
use orch_api_contract::access::Role;
use orch_api_contract::admin::{AdminUserCreate, AdminUserCreateBody};
use orch_api_contract::auth::{
    AuthEmailVerify, AuthEmailVerifyBody, AuthNameAvailabilityGet, AuthNameAvailabilityGetQuery,
    AuthRecoveryConfirm, AuthRecoveryConfirmBody, AuthRegister, AuthRegisterBody,
    AuthRegisterReply, MeGet, MeUpdate, MeUpdateBody,
};
use orch_api_contract::chats::{ChatGet, ChatKind, ChatMemberRemove};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeGet;
use orch_api_contract::routes::Endpoint;
use orch_api_contract::users::{
    InvitationAccept, InvitationAcceptBody, InvitationCreate, InvitationCreateBody,
    InvitationCreateReply, InvitationKind, InvitationLookup, Me, UserGet,
};
use orch_api_core::testing::Reply;
use orch_db::queries::{access, identity};
use orch_types::{Id, Time};
use places::{Who, admin, make_org, person};
use serde_json::{Value, json};
use support::{PASSWORD, World};
use tower::ServiceExt;

// A profile reply fits the API's ordinary JSON response size limit.
const PROFILE_REPLY_BYTES: usize = 1024 * 1024;
// Scheduling headroom for an authenticated request on the shared machine.
const REQUEST_BOUND: std::time::Duration = std::time::Duration::from_secs(60);
// The shared identity palette has twelve indexes in both themes.
const IDENTITY_COLOR_COUNT: u16 = 12;

async fn profile_after_body(
    world: &World,
    by: &Who,
    value: Value,
    change: impl std::future::Future<Output = ()> + Send + 'static,
) -> (http::StatusCode, Value) {
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry.into_router().with_state(world.app.app.clone());
    // Authentication has already run when the binder begins polling this body.
    let body = axum::body::Body::from_stream(futures::stream::once(async move {
        change.await;
        Ok::<_, std::io::Error>(bytes::Bytes::from(serde_json::to_vec(&value).unwrap()))
    }));
    let request = http::Request::builder()
        .method(MeUpdate::ROUTE.method.as_str())
        .uri(MeUpdate::ROUTE.path)
        .header("authorization", format!("Bearer {}", by.token))
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let reply = tokio::time::timeout(REQUEST_BOUND, router.oneshot(request))
        .await
        .unwrap()
        .unwrap();
    let status = reply.status();
    let bytes = axum::body::to_bytes(reply.into_body(), PROFILE_REPLY_BYTES)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn a_profile_change_refuses_a_session_revoked_while_its_body_is_decoded() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let pool = world.db.pool.clone();
    let id = ada.id();
    let now = world.now();
    let chosen = (u16::try_from(identity::avatar_color(id)).unwrap() + 1) % IDENTITY_COLOR_COUNT;
    let (status, problem) = profile_after_body(
        &world,
        &ada,
        json!({ "avatar_color": chosen }),
        async move {
            sqlx::query("UPDATE sessions SET revoked_at = $2 WHERE user_id = $1")
                .bind(id)
                .bind(now)
                .execute(&pool)
                .await
                .unwrap();
        },
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(problem["code"], "auth.invalid_token");
    let color: i16 = sqlx::query_scalar("SELECT avatar_color FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(color, identity::avatar_color(id));
}

#[tokio::test]
async fn an_email_clear_rechecks_the_last_first_proof_after_the_body_is_decoded() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["sign_in"])).await;
    let ada = person(&world, "ada").await;
    let id = ada.id();
    let pool = world.db.pool.clone();
    let (status, problem) = profile_after_body(
        &world,
        &ada,
        json!({ "email": null, "current_password": PASSWORD, "display_name": "Changed" }),
        async move {
            sqlx::query("UPDATE users SET password_hash = NULL WHERE id = $1")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        },
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(problem["errors"][0]["path"], "email");
    let mut conn = world.conn().await;
    let saved = identity::user_by_id(&mut conn, id).await.unwrap().unwrap();
    assert_eq!(saved.email.as_deref(), Some(ada.person.email.as_str()));
    assert_eq!(saved.display_name, "ada");
}

fn invitation(kind: InvitationKind, target: Id) -> InvitationCreateBody {
    InvitationCreateBody {
        kind,
        org_id: (kind == InvitationKind::Org).then_some(target),
        node_id: (kind == InvitationKind::Node).then_some(target),
        chat_id: (kind == InvitationKind::Chat).then_some(target),
        role: None,
        permissions: None,
        email: None,
        max_uses: None,
        expires_in_hours: None,
        note: None,
    }
}

async fn invite(
    world: &World,
    by: &Who,
    body: &InvitationCreateBody,
) -> Reply<InvitationCreateReply> {
    world
        .app
        .call(InvitationCreate, &[])
        .token(&by.token)
        .body(body)
        .send()
        .await
}

async fn accept(
    world: &World,
    by: &Who,
    token: &str,
) -> Reply<orch_api_contract::users::InvitationAcceptReply> {
    world
        .app
        .call(InvitationAccept, &[])
        .token(&by.token)
        .body(&InvitationAcceptBody {
            token: token.to_owned(),
        })
        .send()
        .await
}

async fn update(world: &World, by: &Who, body: Value) -> Reply<Me> {
    world
        .app
        .call(MeUpdate, &[])
        .token(&by.token)
        .raw_body(&body)
        .send()
        .await
}

async fn membership(world: &World, root: Id, user: Id) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM org_members WHERE org_id = $1 AND user_id = $2)",
    )
    .bind(root)
    .bind(user)
    .fetch_one(&world.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_group_invitation_opens_its_chat_and_adds_a_guest_without_joining_the_tree() {
    let room = Room::new().await;
    let group = room.group(&room.cai, &[&room.bea], Some("Planning")).await;
    let made = invite(
        &room.world,
        &room.cai,
        &invitation(InvitationKind::Chat, group.base.id),
    )
    .await
    .ok();
    assert_eq!(made.invitation.chat.as_ref().unwrap().id, group.base.id);
    assert!(made.invitation.org.is_none() && made.invitation.node.is_none());
    let preview = room
        .world
        .app
        .call(InvitationLookup, &[&made.token])
        .send()
        .await
        .ok();
    assert_eq!(preview.what.as_ref().unwrap().name, "Planning");
    assert!(!membership(&room.world, room.root, room.eve.id()).await);
    let target = accept(&room.world, &room.eve, &made.token).await.ok();
    assert!(target.org.is_none() && target.node.is_none());
    let chat = target.chat.unwrap();
    assert_eq!(chat.id, group.base.id);
    assert_eq!(chat.kind, ChatKind::Group);
    let opened = room
        .world
        .app
        .call(ChatGet, &[&chat.id])
        .token(&room.eve.token)
        .send()
        .await
        .ok();
    assert!(opened.can.send);
    assert!(opened.guests.contains(&room.eve.id()));
    assert!(!membership(&room.world, room.root, room.eve.id()).await);
    assert!(
        !room
            .world
            .app
            .call(NodeGet, &[&room.root])
            .token(&room.eve.token)
            .send()
            .await
            .status
            .is_success()
    );
    assert_eq!(
        accept(&room.world, &room.eve, &made.token)
            .await
            .problem()
            .code,
        ErrorCode::Gone
    );
}

#[tokio::test]
async fn a_chat_inspector_cannot_turn_reading_into_an_invitation_or_a_seat() {
    let room = Room::new().await;
    let group = room.group(&room.cai, &[&room.bea], Some("Planning")).await;
    let read = room
        .world
        .app
        .call(ChatGet, &[&group.base.id])
        .token(&room.dee.token)
        .send()
        .await
        .ok();
    assert!(!read.can.send);
    let refused = invite(
        &room.world,
        &room.dee,
        &invitation(InvitationKind::Chat, group.base.id),
    )
    .await;
    assert!(!refused.status.is_success());
    let seated: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM chat_members WHERE chat_id = $1 AND user_id = $2 AND left_at IS NULL)")
        .bind(group.base.id).bind(room.dee.id()).fetch_one(&room.world.db.pool).await.unwrap();
    assert!(!seated);
    let made = invite(
        &room.world,
        &room.cai,
        &invitation(InvitationKind::Chat, group.base.id),
    )
    .await
    .ok();
    // A retained link whose maker has become an inspector gives no membership.
    room.world
        .app
        .call(ChatMemberRemove, &[&group.base.id, &room.cai.id()])
        .token(&room.cai.token)
        .send()
        .await
        .ok();
    assert!(
        !accept(&room.world, &room.eve, &made.token)
            .await
            .status
            .is_success()
    );
    let mut conn = room.world.conn().await;
    assert_eq!(
        identity::invitation_by_id(&mut conn, made.invitation.id)
            .await
            .unwrap()
            .unwrap()
            .uses,
        0
    );
}

#[tokio::test]
async fn a_shared_chat_link_counts_two_people_without_spending_a_second_seat_on_a_replay() {
    let room = Room::new().await;
    let group = room.group(&room.cai, &[&room.bea], Some("Planning")).await;
    let made = invite(
        &room.world,
        &room.cai,
        &InvitationCreateBody {
            max_uses: Some(2),
            ..invitation(InvitationKind::Chat, group.base.id)
        },
    )
    .await
    .ok();
    accept(&room.world, &room.eve, &made.token).await.ok();
    assert_eq!(
        accept(&room.world, &room.eve, &made.token)
            .await
            .problem()
            .code,
        ErrorCode::Gone
    );
    accept(&room.world, &room.fay, &made.token).await.ok();
    let mut conn = room.world.conn().await;
    assert_eq!(
        identity::invitation_by_id(&mut conn, made.invitation.id)
            .await
            .unwrap()
            .unwrap()
            .uses,
        2
    );
}

#[tokio::test]
async fn chat_invitations_refuse_other_scopes_roles_permissions_and_invalid_lifetimes() {
    let room = Room::new().await;
    let group = room.group(&room.cai, &[&room.bea], Some("Planning")).await;
    let base = invitation(InvitationKind::Chat, group.base.id);
    for wrong in [
        InvitationCreateBody {
            chat_id: None,
            ..base.clone()
        },
        InvitationCreateBody {
            org_id: Some(room.root),
            ..base.clone()
        },
        InvitationCreateBody {
            node_id: Some(room.topic),
            ..base.clone()
        },
        InvitationCreateBody {
            role: Some(Role::Admin),
            ..base.clone()
        },
        InvitationCreateBody {
            permissions: Some(vec!["chat.view".to_owned()]),
            ..base.clone()
        },
        InvitationCreateBody {
            expires_in_hours: Some(0),
            ..base.clone()
        },
        InvitationCreateBody {
            expires_in_hours: Some(721),
            ..base.clone()
        },
        InvitationCreateBody {
            chat_id: Some(room.chat),
            ..base.clone()
        },
    ] {
        assert!(
            !invite(&room.world, &room.cai, &wrong)
                .await
                .status
                .is_success()
        );
    }
    let direct = room.direct(&room.cai, &room.bea).await;
    assert!(
        !invite(
            &room.world,
            &room.cai,
            &invitation(InvitationKind::Chat, direct.base.id)
        )
        .await
        .status
        .is_success()
    );
    let made = invite(
        &room.world,
        &room.cai,
        &InvitationCreateBody {
            expires_in_hours: Some(1),
            ..base
        },
    )
    .await
    .ok();
    room.world.advance(Duration::hours(1));
    let expired = accept(&room.world, &room.eve, &made.token).await.problem();
    assert_eq!(expired.code, ErrorCode::Gone);
    assert!(!expired.extra.contains_key("chat"));
    assert_eq!(
        room.world
            .app
            .call(InvitationLookup, &[&made.token])
            .send()
            .await
            .problem()
            .code,
        ErrorCode::Gone
    );
}

#[tokio::test]
async fn a_chat_link_rechecks_the_makers_policy_and_the_groups_capacity_before_use() {
    let room = Room::new().await;
    let group = room.group(&room.bea, &[&room.cai], Some("Planning")).await;
    let made = invite(
        &room.world,
        &room.bea,
        &invitation(InvitationKind::Chat, group.base.id),
    )
    .await
    .ok();
    room.world
        .set_at(
            orch_settings::Target::Node(room.root),
            "sharing.allow_external",
            json!(false),
        )
        .await;
    assert_eq!(
        accept(&room.world, &room.eve, &made.token)
            .await
            .problem()
            .code,
        ErrorCode::SharingExternalForbidden
    );
    room.world
        .set_at(
            orch_settings::Target::Node(room.root),
            "sharing.allow_external",
            json!(true),
        )
        .await;
    room.world.set("chat.group_max_members", json!(2)).await;
    assert_eq!(
        accept(&room.world, &room.eve, &made.token)
            .await
            .problem()
            .code,
        ErrorCode::ChatNotAllowed
    );
    let mut conn = room.world.conn().await;
    assert_eq!(
        identity::invitation_by_id(&mut conn, made.invitation.id)
            .await
            .unwrap()
            .unwrap()
            .uses,
        0
    );
}

#[tokio::test]
async fn node_invitations_return_only_the_target_the_recipient_can_actually_open() {
    let room = Room::new().await;
    let chat_only = invite(
        &room.world,
        &room.ada,
        &invitation(InvitationKind::Node, room.topic),
    )
    .await
    .ok();
    let accepted = accept(&room.world, &room.eve, &chat_only.token).await.ok();
    assert!(accepted.org.is_none() && accepted.node.is_none());
    assert_eq!(accepted.chat.as_ref().unwrap().id, room.chat);
    assert!(
        room.world
            .app
            .call(ChatGet, &[&room.chat])
            .token(&room.eve.token)
            .send()
            .await
            .ok()
            .can
            .send
    );
    assert!(
        !room
            .world
            .app
            .call(NodeGet, &[&room.topic])
            .token(&room.eve.token)
            .send()
            .await
            .status
            .is_success()
    );
    let visible = invite(
        &room.world,
        &room.ada,
        &InvitationCreateBody {
            role: Some(Role::Viewer),
            ..invitation(InvitationKind::Node, room.topic)
        },
    )
    .await
    .ok();
    let target = accept(&room.world, &room.fay, &visible.token)
        .await
        .ok()
        .node
        .unwrap();
    assert_eq!(target.id, room.topic);
    assert!(!target.redacted);
    assert!(
        room.world
            .app
            .call(NodeGet, &[&target.id])
            .token(&room.fay.token)
            .send()
            .await
            .status
            .is_success()
    );
    assert!(!membership(&room.world, room.root, room.fay.id()).await);
}

#[tokio::test]
async fn a_removed_node_or_a_withdrawn_makers_role_returns_no_navigation_target() {
    use orch_api_contract::access::{NodeRoleList, RoleUpdate, RoleUpdateBody};
    use orch_api_contract::auth::{AdminModeSetBody, MeAdminModeSet};
    let room = Room::new().await;
    for by in [&room.dee, &room.ada] {
        room.world
            .app
            .call(MeAdminModeSet, &[])
            .token(&by.token)
            .body(&AdminModeSetBody { as_admin: true })
            .send()
            .await
            .ok();
    }
    let made = invite(
        &room.world,
        &room.dee,
        &InvitationCreateBody {
            role: Some(Role::Viewer),
            ..invitation(InvitationKind::Node, room.topic)
        },
    )
    .await
    .ok();
    let roles = room
        .world
        .app
        .call(NodeRoleList, &[&room.root])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let held = roles
        .own
        .into_iter()
        .find(|held| held.subject.id == room.dee.id())
        .unwrap();
    assert_eq!(held.role, Role::Admin);
    // Domain membership keeps a role; withdraw administration through its normal revision-checked update.
    room.world
        .app
        .call(RoleUpdate, &[&held.id])
        .token(&room.ada.token)
        .body(&RoleUpdateBody {
            role: Some(Role::Viewer),
            expires_at: None,
            note: None,
            expected_revision: held.revision,
        })
        .send()
        .await
        .ok();
    let mut conn = room.world.conn().await;
    assert!(
        !accept(&room.world, &room.eve, &made.token)
            .await
            .status
            .is_success()
    );
    assert_eq!(
        identity::invitation_by_id(&mut conn, made.invitation.id)
            .await
            .unwrap()
            .unwrap()
            .uses,
        0
    );
    sqlx::query("UPDATE nodes SET deleted_at = $2 WHERE id = $1")
        .bind(room.topic)
        .bind(room.world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        accept(&room.world, &room.eve, &made.token)
            .await
            .problem()
            .code,
        ErrorCode::Gone
    );
}

#[tokio::test]
async fn an_avatar_color_is_seen_by_another_reader_resettable_and_writable_only_for_oneself() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bea = person(&world, "bea").await;
    let changed = update(&world, &ada, json!({ "avatar_color": 7 }))
        .await
        .ok();
    assert_eq!(changed.user.avatar.color, 7);
    assert_eq!(
        world
            .app
            .call(UserGet, &[&ada.id()])
            .token(&bea.token)
            .send()
            .await
            .ok()
            .avatar
            .color,
        7
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&ada.token)
            .send()
            .await
            .ok()
            .user
            .avatar
            .color,
        7
    );
    assert_eq!(
        update(&world, &ada, json!({ "avatar_color": 12 }))
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        update(
            &world,
            &ada,
            json!({ "avatar_color": 3, "user_id": bea.id() })
        )
        .await
        .problem()
        .code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&ada.token)
            .send()
            .await
            .ok()
            .user
            .avatar
            .color,
        7
    );
    assert_eq!(
        update(&world, &ada, json!({ "avatar_color": null }))
            .await
            .ok()
            .user
            .avatar
            .color,
        u32::try_from(identity::avatar_color(ada.id())).unwrap()
    );
}

#[tokio::test]
async fn an_omitted_email_preserves_it_and_a_literal_or_typed_null_clears_it_with_a_password() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let initial = ada.person.email.as_str();
    assert_eq!(
        update(&world, &ada, json!({ "display_name": "Ada Lovelace" }))
            .await
            .ok()
            .email
            .as_deref(),
        Some(initial)
    );
    let set = update(
        &world,
        &ada,
        json!({ "email": "new@example.org", "current_password": PASSWORD }),
    )
    .await
    .ok();
    assert_eq!(set.email.as_deref(), Some("new@example.org"));
    assert!(!set.email_verified);
    assert_eq!(
        update(&world, &ada, json!({ "email": null }))
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let clear = world
        .app
        .call(MeUpdate, &[])
        .token(&ada.token)
        .body(&MeUpdateBody {
            email: Some(None),
            current_password: Some(PASSWORD.to_owned()),
            ..MeUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert!(clear.email.is_none() && !clear.email_verified && clear.email_pending.is_none());
    assert!(clear.has_password && clear.email_code.is_empty());
    assert!(
        world
            .password_login("ada", PASSWORD, "198.51.100.8", "")
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn clearing_an_address_cancels_pending_confirmations_without_spending_a_second_step() {
    let world = World::new().await;
    world.mail_server().await;
    let ada = person(&world, "ada").await;
    let waiting = update(
        &world,
        &ada,
        json!({ "email": "new@example.org", "current_password": PASSWORD }),
    )
    .await
    .ok();
    assert_eq!(waiting.email.as_deref(), Some(ada.person.email.as_str()));
    assert_eq!(waiting.email_pending.as_deref(), Some("new@example.org"));
    let mails = world.mails("verify_email", "new@example.org").await;
    let token = mails.last().unwrap()["data"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut conn = world.conn().await;
    identity::set_email_code_second_step(&mut conn, ada.id(), Some(world.now()), world.now())
        .await
        .unwrap();
    let refused = update(
        &world,
        &ada,
        json!({ "email": null, "current_password": PASSWORD }),
    )
    .await;
    assert!(!refused.status.is_success());
    assert!(
        identity::user_by_id(&mut conn, ada.id())
            .await
            .unwrap()
            .unwrap()
            .email_code_on()
    );
    identity::set_email_code_second_step(&mut conn, ada.id(), None, world.now())
        .await
        .unwrap();
    assert!(
        update(
            &world,
            &ada,
            json!({ "email": null, "current_password": PASSWORD })
        )
        .await
        .ok()
        .email_pending
        .is_none()
    );
    assert_eq!(
        world
            .app
            .call(AuthEmailVerify, &[])
            .body(&AuthEmailVerifyBody { token })
            .send()
            .await
            .problem()
            .code,
        ErrorCode::Gone
    );
}

#[tokio::test]
async fn an_email_cannot_be_cleared_when_it_is_the_only_allowed_first_proof() {
    let world = World::new().await;
    world.mail_server().await;
    let ada = person(&world, "ada").await;
    let mut conn = world.conn().await;
    sqlx::query("UPDATE users SET password_hash = NULL WHERE id = $1")
        .bind(ada.id())
        .execute(&mut *conn)
        .await
        .unwrap();
    world.set("auth.email_code", json!(["sign_in"])).await;
    assert!(
        !update(&world, &ada, json!({ "email": null }))
            .await
            .status
            .is_success()
    );
    assert_eq!(
        identity::user_by_id(&mut conn, ada.id())
            .await
            .unwrap()
            .unwrap()
            .email
            .as_deref(),
        Some(ada.person.email.as_str())
    );
    world.set("auth.local_login", json!(false)).await;
    let bea = person(&world, "bea").await;
    assert!(
        !update(
            &world,
            &bea,
            json!({ "email": null, "current_password": PASSWORD })
        )
        .await
        .status
        .is_success()
    );
}

#[tokio::test]
async fn public_name_conflicts_identify_the_username_and_availability_never_checks_email() {
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    let ada = person(&world, "ada").await;
    make_org(&world, &ada, "Team", "team").await;
    for name in ["ada", "team"] {
        assert!(
            !world
                .app
                .call(AuthNameAvailabilityGet, &[])
                .query(&AuthNameAvailabilityGetQuery {
                    name: name.to_owned()
                })
                .send()
                .await
                .ok()
                .available
        );
        let refused = world
            .app
            .call(AuthRegister, &[])
            .body(&AuthRegisterBody {
                accept_invitation: false,
                username: name.to_owned(),
                display_name: "Person".to_owned(),
                password: PASSWORD.to_owned(),
                email: None,
                invitation: None,
                timezone: None,
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::ConflictExists);
        assert_eq!(refused.extra["errors"][0]["path"], "username");
    }
    assert!(
        world
            .app
            .call(AuthNameAvailabilityGet, &[])
            .query(&AuthNameAvailabilityGetQuery {
                name: "free_name".to_owned()
            })
            .send()
            .await
            .ok()
            .available
    );
    for address in ["ada@example.org", "unknown@example.org"] {
        let refused = world
            .app
            .call(AuthNameAvailabilityGet, &[])
            .query(&AuthNameAvailabilityGetQuery {
                name: address.to_owned(),
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
    }
    let email = world
        .app
        .call(AuthRegister, &[])
        .body(&AuthRegisterBody {
            accept_invitation: false,
            username: "freshperson".to_owned(),
            display_name: "Person".to_owned(),
            password: PASSWORD.to_owned(),
            email: Some(ada.person.email),
            invitation: None,
            timezone: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(email.code, ErrorCode::ConflictExists);
    assert!(!email.extra.contains_key("errors") && !email.extra.contains_key("email"));
    assert!(!email.detail.contains("address"));
}

#[tokio::test]
async fn confirmation_registration_has_no_session_and_a_known_email_has_the_same_pending_outcome() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("registration.mode", json!("open")).await;
    person(&world, "ada").await;
    for (name, email) in [("newone", "new@example.org"), ("newtwo", "ada@example.org")] {
        let pending = world
            .app
            .call(AuthRegister, &[])
            .body(&AuthRegisterBody {
                accept_invitation: false,
                username: name.to_owned(),
                display_name: "Person".to_owned(),
                password: PASSWORD.to_owned(),
                email: Some(email.to_owned()),
                invitation: None,
                timezone: None,
            })
            .send()
            .await;
        assert_eq!(pending.status, 202);
        assert!(matches!(pending.body, Ok(AuthRegisterReply::Pending(_))));
        assert!(!pending.headers.contains_key(http::header::SET_COOKIE));
        assert!(!String::from_utf8_lossy(&pending.raw).contains("access_token"));
    }
}

async fn create_with_reset(
    world: &World,
    by: &Who,
    name: &str,
) -> orch_api_contract::admin::AdminUserCreateReply {
    world
        .app
        .call(AdminUserCreate, &[])
        .token(&by.token)
        .body(&AdminUserCreateBody {
            username: name.to_owned(),
            display_name: name.to_owned(),
            email: None,
            is_system_admin: None,
            personal_org: None,
        })
        .send()
        .await
        .ok()
}

async fn redeem(world: &World, url: &str) -> Reply<()> {
    world
        .app
        .call(AuthRecoveryConfirm, &[])
        .body(&AuthRecoveryConfirmBody {
            token: url.rsplit('/').next().unwrap().to_owned(),
            password: "an entirely new phrase".to_owned(),
        })
        .send()
        .await
}

#[tokio::test]
async fn an_admin_created_reset_link_reports_its_real_expiry_and_can_be_redeemed_only_once_before_it()
 {
    let world = World::new().await;
    let boss = admin(&world, "boss").await;
    let timely = create_with_reset(&world, &boss, "timely").await;
    let late = create_with_reset(&world, &boss, "late").await;
    let ends: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT expires_at FROM email_tokens WHERE user_id = $1")
            .bind(timely.user.base.id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(timely.expires_at, Time::new(ends));
    assert_eq!(late.expires_at, timely.expires_at);
    world.advance(ends - world.now() - Duration::milliseconds(1));
    assert_eq!(redeem(&world, &timely.reset_url).await.status, 204);
    assert_eq!(
        redeem(&world, &timely.reset_url).await.problem().code,
        ErrorCode::Gone
    );
    world.advance(Duration::milliseconds(1));
    assert_eq!(
        redeem(&world, &late.reset_url).await.problem().code,
        ErrorCode::Gone
    );
    let trail: Vec<Value> = sqlx::query_scalar("SELECT details FROM audit_log ORDER BY at")
        .fetch_all(&world.db.pool)
        .await
        .unwrap();
    for link in [&timely.reset_url, &late.reset_url] {
        let secret = link.rsplit('/').next().unwrap();
        assert!(!serde_json::to_string(&trail).unwrap().contains(secret));
    }
    let mut conn = world.conn().await;
    assert!(
        identity::user_by_id(&mut conn, timely.user.base.id)
            .await
            .unwrap()
            .unwrap()
            .password_hash
            .is_some()
    );
    assert!(
        identity::user_by_id(&mut conn, late.user.base.id)
            .await
            .unwrap()
            .unwrap()
            .password_hash
            .is_none()
    );
    assert!(
        !access::memberships(&mut conn, timely.user.base.id)
            .await
            .unwrap()
            .is_empty()
    );
}
