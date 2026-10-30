//! An invitation gives a role only within what its maker may give, at creation and at acceptance.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod places;
mod support;

use orch_api_contract::access::Role;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::users::{
    InvitationAccept, InvitationAcceptBody, InvitationCreate, InvitationCreateBody, InvitationKind,
};
use orch_db::queries::access::{self, PageKey, testing};
use orch_types::Id;
use places::{Who, join, make_org, make_project, person};
use std::time::Duration;
use support::World;

async fn a_manager(world: &World, owner: &Who, manager: &Who, root: Id, node: Id) {
    join(world, root, root, manager, "viewer", owner).await;
    let mut conn = world.conn().await;
    for permission in ["access.manage", "access.external"] {
        testing::add_entry(
            &mut conn,
            &PageKey::new("node", node, "live"),
            "any",
            permission,
            ("user", Some(manager.id()), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
    }
}

fn invitation(node: Id, role: Role) -> InvitationCreateBody {
    InvitationCreateBody {
        kind: InvitationKind::Node,
        org_id: None,
        node_id: Some(node),
        chat_id: None,
        role: Some(role),
        permissions: None,
        email: None,
        max_uses: None,
        expires_in_hours: None,
        note: None,
    }
}

fn org_invitation(org: Id, role: Role) -> InvitationCreateBody {
    InvitationCreateBody {
        kind: InvitationKind::Org,
        org_id: Some(org),
        node_id: None,
        chat_id: None,
        role: Some(role),
        permissions: None,
        email: None,
        max_uses: None,
        expires_in_hours: None,
        note: None,
    }
}

async fn an_org_member_manager(world: &World, owner: &Who, manager: &Who, org: Id, role: &str) {
    join(world, org, org, manager, role, owner).await;
    let mut conn = world.conn().await;
    testing::add_entry(
        &mut conn,
        &PageKey::new("node", org, "live"),
        "any",
        "members.manage",
        ("user", Some(manager.id()), None),
        None,
        world.now(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn an_org_member_manager_cannot_invite_someone_above_their_own_role() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let manager = person(&world, "manager").await;
    let guest = person(&world, "guest").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    an_org_member_manager(&world, &owner, &manager, org.base.id, "viewer").await;
    for role in [Role::Interactor, Role::Developer] {
        let problem = world
            .app
            .call(InvitationCreate, &[])
            .token(&manager.token)
            .body(&org_invitation(org.base.id, role))
            .send()
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::AuthzDenied);
    }
    let invite = world
        .app
        .call(InvitationCreate, &[])
        .token(&manager.token)
        .body(&org_invitation(org.base.id, Role::Viewer))
        .send()
        .await
        .ok();
    world
        .app
        .call(InvitationAccept, &[])
        .token(&guest.token)
        .body(&InvitationAcceptBody {
            token: invite.token.clone(),
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    assert_eq!(
        access::role_at(&mut conn, org.base.id, guest.id())
            .await
            .unwrap()
            .as_deref(),
        Some("viewer")
    );
}

#[tokio::test]
async fn a_node_invitation_can_offer_only_the_permissions_its_maker_can_give() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let manager = person(&world, "manager").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    let project = make_project(&world, &owner, org.base.id, "Work").await;
    a_manager(&world, &owner, &manager, org.base.id, project.base.id).await;
    let offer = |permissions| InvitationCreateBody {
        kind: InvitationKind::Node,
        org_id: None,
        node_id: Some(project.base.id),
        chat_id: None,
        role: None,
        permissions,
        email: None,
        max_uses: None,
        expires_in_hours: None,
        note: None,
    };
    let too_much = world
        .app
        .call(InvitationCreate, &[])
        .token(&manager.token)
        .body(&offer(Some(vec![
            "project.view".into(),
            "project.manage".into(),
        ])))
        .send()
        .await
        .problem();
    assert_eq!(too_much.code, ErrorCode::AuthzDenied);
    assert_eq!(too_much.extra["required"], "project.manage");
    let empty = world
        .app
        .call(InvitationCreate, &[])
        .token(&manager.token)
        .body(&offer(Some(vec![])))
        .send()
        .await
        .problem();
    assert_eq!(empty.code, ErrorCode::RequestInvalid);
    sqlx::query("INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, inherit) VALUES ('node', $1, 'live', 'any', 'chat.interact', false) ON CONFLICT (object_kind, object_id, page, profile, permission) DO UPDATE SET inherit = false")
        .bind(project.base.id).execute(&world.db.pool).await.unwrap();
    let default_offer = world
        .app
        .call(InvitationCreate, &[])
        .token(&manager.token)
        .body(&offer(None))
        .send()
        .await
        .problem();
    assert_eq!(default_offer.code, ErrorCode::AuthzDenied);
    assert_eq!(default_offer.extra["required"], "chat.interact");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invitations WHERE created_by = $1")
        .bind(manager.id())
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "no unusable invitation was saved");
}

#[tokio::test]
async fn a_scoped_token_cannot_invite_an_outsider_into_a_row_it_cannot_give() {
    use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate, SensitiveProof};

    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let guest = person(&world, "guest").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    let project = make_project(&world, &owner, org.base.id, "Work").await;
    let token = world
        .app
        .call(MeTokenCreate, &[])
        .token(&owner.token)
        .body(&ApiTokenCreateBody {
            name: "Access only".to_owned(),
            scopes: Some(vec![
                "project.view".to_owned(),
                "access.manage".to_owned(),
                "access.external".to_owned(),
            ]),
            node_id: Some(project.base.id),
            expires_at: None,
            proof: SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .unwrap();
    let offer = InvitationCreateBody {
        kind: InvitationKind::Node,
        org_id: None,
        node_id: Some(project.base.id),
        chat_id: None,
        role: None,
        permissions: Some(vec!["project.manage".to_owned()]),
        email: None,
        max_uses: None,
        expires_in_hours: None,
        note: None,
    };
    let problem = world
        .app
        .call(InvitationCreate, &[])
        .token(&token)
        .body(&offer)
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert_eq!(problem.extra["required"], "project.manage");
    let (invitations, uses): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM invitations WHERE created_by = $1), (SELECT count(*) FROM invitation_uses u JOIN invitations i ON i.id = u.invitation_id WHERE i.created_by = $1)",
    ).bind(owner.id()).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!((invitations, uses), (0, 0));

    let made = world
        .app
        .call(InvitationCreate, &[])
        .token(&owner.token)
        .body(&offer)
        .send()
        .await
        .ok();
    assert_eq!(made.invitation.permissions, ["project.manage"]);
    world
        .app
        .call(InvitationAccept, &[])
        .token(&guest.token)
        .body(&InvitationAcceptBody { token: made.token })
        .send()
        .await
        .ok();
    let (uses, entries): (i32, i64) = sqlx::query_as(
        "SELECT uses, (SELECT count(*) FROM permission_entries WHERE object_id = $2 AND permission = 'project.manage' AND holder_id = $3) FROM invitations WHERE id = $1",
    ).bind(made.invitation.id).bind(project.base.id).bind(guest.id())
        .fetch_one(&world.db.pool).await.unwrap();
    assert_eq!((uses, entries), (1, 1));
}

#[tokio::test]
async fn a_multi_use_node_link_rechecks_the_makers_row_authority_for_every_person() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let manager = person(&world, "manager").await;
    let first = person(&world, "first").await;
    let second = person(&world, "second").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    let project = make_project(&world, &owner, org.base.id, "Hidden work").await;
    a_manager(&world, &owner, &manager, org.base.id, project.base.id).await;
    let offer = InvitationCreateBody {
        kind: InvitationKind::Node,
        org_id: None,
        node_id: Some(project.base.id),
        chat_id: None,
        role: None,
        permissions: Some(vec!["project.view".into()]),
        email: None,
        max_uses: Some(3),
        expires_in_hours: None,
        note: None,
    };
    let invitation = world
        .app
        .call(InvitationCreate, &[])
        .token(&manager.token)
        .body(&offer)
        .send()
        .await
        .ok();
    world
        .app
        .call(InvitationAccept, &[])
        .token(&first.token)
        .body(&InvitationAcceptBody {
            token: invitation.token.clone(),
        })
        .send()
        .await
        .ok();
    sqlx::query("INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, inherit) VALUES ('node', $1, 'live', 'any', 'project.view', false) ON CONFLICT (object_kind, object_id, page, profile, permission) DO UPDATE SET inherit = false")
        .bind(project.base.id).execute(&world.db.pool).await.unwrap();
    let problem = world
        .app
        .call(InvitationAccept, &[])
        .token(&second.token)
        .body(&InvitationAcceptBody {
            token: invitation.token.clone(),
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert_eq!(problem.extra["reason"], "invitation_issuer_authority_lost");
    let public = serde_json::to_string(&problem).unwrap();
    assert!(!public.contains("Hidden work") && !public.contains("\"username\":\"owner\""));
    let preview = world
        .app
        .call(
            orch_api_contract::users::InvitationLookup,
            &[&invitation.token],
        )
        .send()
        .await
        .problem();
    assert_eq!(preview.extra["reason"], "invitation_issuer_authority_lost");
    assert!(
        !serde_json::to_string(&preview)
            .unwrap()
            .contains("Hidden work")
    );
    let (uses, entries): (i32, i64) = sqlx::query_as(
        "SELECT uses, (SELECT count(*) FROM permission_entries WHERE object_id = $2 AND holder_id = $3) FROM invitations WHERE id = $1",
    ).bind(invitation.invitation.id).bind(project.base.id).bind(second.id())
        .fetch_one(&world.db.pool).await.unwrap();
    assert_eq!((uses, entries), (1, 0));
}

#[tokio::test]
async fn a_changed_external_lifetime_refuses_the_grant_without_naming_a_hidden_policy_place() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let guest = person(&world, "guest").await;
    let org = make_org(&world, &owner, "Private office", "office").await;
    let project = make_project(&world, &owner, org.base.id, "Work").await;
    let mut offer = invitation(project.base.id, Role::Viewer);
    offer.role = None;
    offer.permissions = Some(vec!["project.view".to_owned()]);
    let made = world
        .app
        .call(InvitationCreate, &[])
        .token(&owner.token)
        .body(&offer)
        .send()
        .await
        .ok();
    world
        .set_at(
            orch_settings::Target::Node(project.base.id),
            "sharing.external_max_days",
            serde_json::json!(7),
        )
        .await;
    let problem = world
        .app
        .call(InvitationAccept, &[])
        .token(&guest.token)
        .body(&InvitationAcceptBody { token: made.token })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::SharingExternalLifetimeExceeded);
    assert!(!problem.extra.contains_key("scope"));
    let (uses, entries): (i32, i64) = sqlx::query_as(
        "SELECT uses, (SELECT count(*) FROM permission_entries WHERE object_id = $2 AND holder_id = $3) FROM invitations WHERE id = $1",
    ).bind(made.invitation.id).bind(project.base.id).bind(guest.id())
        .fetch_one(&world.db.pool).await.unwrap();
    assert_eq!((uses, entries), (0, 0));
}

#[tokio::test]
async fn a_multi_use_group_link_does_not_add_a_person_when_its_maker_leaves_during_acceptance() {
    use orch_api_contract::chats::ChatMemberRemove;

    let room = chat_world::Room::new().await;
    let group = room
        .group(&room.cai, &[&room.bea], Some("Release"))
        .await
        .base
        .id;
    let invitation = room
        .world
        .app
        .call(InvitationCreate, &[])
        .token(&room.cai.token)
        .body(&InvitationCreateBody {
            kind: InvitationKind::Chat,
            org_id: None,
            node_id: None,
            chat_id: Some(group),
            role: None,
            permissions: None,
            email: None,
            max_uses: Some(3),
            expires_in_hours: None,
            note: None,
        })
        .send()
        .await
        .ok();
    let accepted = room
        .world
        .app
        .call(InvitationAccept, &[])
        .token(&room.eve.token)
        .body(&InvitationAcceptBody {
            token: invitation.token.clone(),
        })
        .send()
        .await
        .ok();
    assert_eq!(accepted.chat.unwrap().id, group);

    // The maker's departure holds the same chat lock that acceptance must acquire.
    let mut leaving = room.world.db.pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *leaving)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM chats WHERE id = $1 FOR UPDATE")
        .bind(group)
        .execute(&mut *leaving)
        .await
        .unwrap();
    sqlx::query("UPDATE chat_members SET left_at = $3 WHERE chat_id = $1 AND user_id = $2")
        .bind(group)
        .bind(room.cai.id())
        .bind(room.world.now())
        .execute(&mut *leaving)
        .await
        .unwrap();
    let app = room.world.app.clone();
    let token = room.fay.token.clone();
    let link = invitation.token.clone();
    let pending = tokio::spawn(async move {
        app.call(InvitationAccept, &[])
            .token(&token)
            .body(&InvitationAcceptBody { token: link })
            .send()
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid)))",
            ).bind(pid).fetch_one(&room.world.db.pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the second acceptance waits for the maker's departure");
    leaving.commit().await.unwrap();
    let problem = pending.await.unwrap().problem();
    assert_eq!(problem.code, ErrorCode::NotFound);
    assert_eq!(problem.extra["reason"], "invitation_issuer_authority_lost");
    let lookup = room
        .world
        .app
        .call(
            orch_api_contract::users::InvitationLookup,
            &[&invitation.token],
        )
        .send()
        .await
        .problem();
    assert_eq!(lookup.extra["reason"], "invitation_issuer_authority_lost");
    let (uses, members): (i32, i64) = sqlx::query_as(
        "SELECT uses, (SELECT count(*) FROM chat_members WHERE chat_id = $2 AND user_id = $3 AND left_at IS NULL) FROM invitations WHERE id = $1",
    ).bind(invitation.invitation.id).bind(group).bind(room.fay.id())
        .fetch_one(&room.world.db.pool).await.unwrap();
    assert_eq!((uses, members), (1, 0));
    // The removed maker cannot use the ordinary membership endpoint either.
    let result = room
        .world
        .app
        .call(ChatMemberRemove, &[&group, &room.cai.id()])
        .token(&room.cai.token)
        .send()
        .await;
    assert!(!result.status.is_success());
}

async fn wait_for_invitation_lock(world: &World, pid: i32) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid)))",
            ).bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("the acceptance waits for the held lock");
}

#[tokio::test]
async fn an_invitation_that_expires_while_its_capacity_lock_is_held_is_not_used() {
    let world = World::new().await;
    let issuer = places::admin(&world, "issuer").await;
    let recipient = person(&world, "recipient").await;
    let offer = InvitationCreateBody {
        kind: InvitationKind::System,
        org_id: None,
        node_id: None,
        chat_id: None,
        role: None,
        permissions: None,
        email: None,
        max_uses: Some(2),
        expires_in_hours: Some(1),
        note: None,
    };
    let invitation = world
        .app
        .call(InvitationCreate, &[])
        .token(&issuer.token)
        .body(&offer)
        .send()
        .await
        .ok();
    let mut held = world.db.pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM invitations WHERE id = $1 FOR UPDATE")
        .bind(invitation.invitation.id)
        .execute(&mut *held)
        .await
        .unwrap();
    let app = world.app.clone();
    let token = recipient.token.clone();
    let link = invitation.token;
    let pending = tokio::spawn(async move {
        app.call(InvitationAccept, &[])
            .token(&token)
            .body(&InvitationAcceptBody { token: link })
            .send()
            .await
    });
    wait_for_invitation_lock(&world, pid).await;
    world.advance(chrono::Duration::hours(2));
    held.commit().await.unwrap();
    assert_eq!(pending.await.unwrap().problem().code, ErrorCode::Gone);
    let (uses, receipts): (i32, i64) = sqlx::query_as(
        "SELECT uses, (SELECT count(*) FROM invitation_uses WHERE invitation_id = $1) FROM invitations WHERE id = $1",
    ).bind(invitation.invitation.id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!((uses, receipts), (0, 0));
}

#[tokio::test]
async fn a_makers_timed_chat_membership_is_rechecked_after_waiting_for_the_chat_lock() {
    let room = chat_world::Room::new().await;
    let group = room
        .group(&room.cai, &[&room.bea], Some("Release"))
        .await
        .base
        .id;
    sqlx::query("UPDATE chat_members SET expires_at = $3 WHERE chat_id = $1 AND user_id = $2")
        .bind(group)
        .bind(room.cai.id())
        .bind(room.world.now() + chrono::Duration::seconds(1))
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    let offer = InvitationCreateBody {
        kind: InvitationKind::Chat,
        org_id: None,
        node_id: None,
        chat_id: Some(group),
        role: None,
        permissions: None,
        email: None,
        max_uses: Some(2),
        expires_in_hours: None,
        note: None,
    };
    let invitation = room
        .world
        .app
        .call(InvitationCreate, &[])
        .token(&room.cai.token)
        .body(&offer)
        .send()
        .await
        .ok();
    let mut held = room.world.db.pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM chats WHERE id = $1 FOR UPDATE")
        .bind(group)
        .execute(&mut *held)
        .await
        .unwrap();
    let app = room.world.app.clone();
    let token = room.eve.token.clone();
    let link = invitation.token;
    let pending = tokio::spawn(async move {
        app.call(InvitationAccept, &[])
            .token(&token)
            .body(&InvitationAcceptBody { token: link })
            .send()
            .await
    });
    wait_for_invitation_lock(&room.world, pid).await;
    room.world.advance(chrono::Duration::seconds(2));
    held.commit().await.unwrap();
    let problem = pending.await.unwrap().problem();
    assert_eq!(problem.code, ErrorCode::NotFound);
    assert_eq!(problem.extra["reason"], "invitation_issuer_authority_lost");
    let (uses, members): (i32, i64) = sqlx::query_as(
        "SELECT uses, (SELECT count(*) FROM chat_members WHERE chat_id = $2 AND user_id = $3 AND left_at IS NULL) FROM invitations WHERE id = $1",
    ).bind(invitation.invitation.id).bind(group).bind(room.eve.id())
        .fetch_one(&room.world.db.pool).await.unwrap();
    assert_eq!((uses, members), (0, 0));
}

#[tokio::test]
async fn a_makers_timed_node_role_is_rechecked_after_waiting_for_the_node_lock() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let maker = person(&world, "maker").await;
    let guest = person(&world, "guest").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    let project = make_project(&world, &owner, org.base.id, "Work").await;
    a_manager(&world, &owner, &maker, org.base.id, project.base.id).await;
    {
        let mut conn = world.conn().await;
        testing::give_role(
            &mut conn,
            project.base.id,
            "user",
            maker.id(),
            "developer",
            owner.id(),
            Some(world.now() + chrono::Duration::seconds(1)),
        )
        .await
        .unwrap();
    }
    let made = world
        .app
        .call(InvitationCreate, &[])
        .token(&maker.token)
        .body(&invitation(project.base.id, Role::Developer))
        .send()
        .await
        .ok();
    let mut held = world.db.pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM nodes WHERE id = $1 FOR UPDATE")
        .bind(project.base.id)
        .execute(&mut *held)
        .await
        .unwrap();
    let app = world.app.clone();
    let token = guest.token.clone();
    let link = made.token;
    let pending = tokio::spawn(async move {
        app.call(InvitationAccept, &[])
            .token(&token)
            .body(&InvitationAcceptBody { token: link })
            .send()
            .await
    });
    wait_for_invitation_lock(&world, pid).await;
    world.advance(chrono::Duration::seconds(2));
    held.commit().await.unwrap();
    let problem = pending.await.unwrap().problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert_eq!(problem.extra["reason"], "invitation_issuer_authority_lost");
    let mut conn = world.conn().await;
    assert!(
        access::role_at(&mut conn, project.base.id, guest.id())
            .await
            .unwrap()
            .is_none()
    );
    let uses: i32 = sqlx::query_scalar("SELECT uses FROM invitations WHERE id = $1")
        .bind(made.invitation.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(uses, 0);
}

#[tokio::test]
async fn registering_with_a_node_invitation_rolls_back_the_account_and_home_when_the_maker_loses_the_role()
 {
    use orch_api_contract::auth::{AuthRegister, AuthRegisterBody};

    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let manager = person(&world, "manager").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    let project = make_project(&world, &owner, org.base.id, "Work").await;
    a_manager(&world, &owner, &manager, org.base.id, project.base.id).await;
    let role = {
        let mut conn = world.conn().await;
        testing::give_role(
            &mut conn,
            project.base.id,
            "user",
            manager.id(),
            "developer",
            owner.id(),
            None,
        )
        .await
        .unwrap()
    };
    world
        .set("orgs.personal_enabled", serde_json::json!(true))
        .await;
    let made = world
        .app
        .call(InvitationCreate, &[])
        .token(&manager.token)
        .body(&invitation(project.base.id, Role::Developer))
        .send()
        .await
        .ok();
    assert!(
        made.invitation.permissions.is_empty(),
        "a role offer does not also promise chat rows"
    );
    {
        let mut conn = world.conn().await;
        assert!(
            access::remove_role(&mut conn, role, owner.id(), world.now())
                .await
                .unwrap()
        );
        orch_authz::epoch::bump(&mut conn).await.unwrap();
    }
    let homes_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM orgs WHERE org_kind = 'personal'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let refused = world
        .app
        .call(AuthRegister, &[])
        .body(&AuthRegisterBody {
            accept_invitation: true,
            username: "blocked-new".to_owned(),
            display_name: "Blocked".to_owned(),
            password: support::PASSWORD.to_owned(),
            email: None,
            invitation: Some(made.token),
            timezone: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["reason"], "invitation_issuer_authority_lost");
    let accounts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE username = 'blocked-new'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let homes_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM orgs WHERE org_kind = 'personal'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    let uses: i32 = sqlx::query_scalar("SELECT uses FROM invitations WHERE id = $1")
        .bind(made.invitation.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!((accounts, homes_after, uses), (0, homes_before, 0));
}

#[tokio::test]
async fn accepting_an_org_invitation_rechecks_the_makers_current_role() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let manager = person(&world, "manager").await;
    let guest = person(&world, "guest").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    an_org_member_manager(&world, &owner, &manager, org.base.id, "developer").await;
    let invite = world
        .app
        .call(InvitationCreate, &[])
        .token(&manager.token)
        .body(&org_invitation(org.base.id, Role::Developer))
        .send()
        .await
        .ok();
    {
        let mut conn = world.conn().await;
        let role: Id = sqlx::query_scalar(
            "SELECT id FROM node_roles WHERE node_id = $1 AND subject_kind = 'user' AND subject_id = $2 AND removed_at IS NULL",
        )
        .bind(org.base.id)
        .bind(manager.id())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(
            access::remove_role(&mut conn, role, owner.id(), world.now())
                .await
                .unwrap()
        );
        orch_authz::epoch::bump(&mut conn).await.unwrap();
    }
    let problem = world
        .app
        .call(InvitationAccept, &[])
        .token(&guest.token)
        .body(&InvitationAcceptBody {
            token: invite.token.clone(),
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    let preview = world
        .app
        .call(orch_api_contract::users::InvitationLookup, &[&invite.token])
        .send()
        .await
        .problem();
    assert_eq!(preview.extra["reason"], "invitation_issuer_authority_lost");
    let mut conn = world.conn().await;
    assert!(
        access::memberships(&mut conn, guest.id())
            .await
            .unwrap()
            .iter()
            .all(|member| member.org_id != org.base.id)
    );
    let uses: i32 = sqlx::query_scalar("SELECT uses FROM invitations WHERE id = $1")
        .bind(invite.invitation.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(uses, 0);
}

#[tokio::test]
async fn a_personal_invitation_tells_the_existing_confirmed_invitee_and_nobody_else() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let invitee = person(&world, "invitee").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    let project = make_project(&world, &owner, org.base.id, "Work").await;
    world
        .set(
            orch_settings_contract::registry::keys::INVITATIONS_FORMS,
            serde_json::json!(["link", "email"]),
        )
        .await;
    sqlx::query(
        "UPDATE users SET email = 'invitee@example.test', email_verified_at = $2 WHERE id = $1",
    )
    .bind(invitee.id())
    .bind(world.now())
    .execute(&world.db.pool)
    .await
    .unwrap();
    let mut body = invitation(project.base.id, Role::Viewer);
    body.email = Some("invitee@example.test".to_owned());
    world
        .app
        .call(InvitationCreate, &[])
        .token(&owner.token)
        .body(&body)
        .send()
        .await
        .ok();
    let rows: Vec<(Id, serde_json::Value)> =
        sqlx::query_as("SELECT user_id, data FROM notifications WHERE kind = 'invitation'")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, invitee.id());
    assert!(rows[0].1["path"].as_str().unwrap().starts_with("/invite/"));
}

#[tokio::test]
async fn a_page_manager_who_is_no_admin_cannot_invite_somebody_as_admin() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let manager = person(&world, "manager").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    let project = make_project(&world, &owner, org.base.id, "Work").await;
    a_manager(&world, &owner, &manager, org.base.id, project.base.id).await;
    let problem = world
        .app
        .call(InvitationCreate, &[])
        .token(&manager.token)
        .body(&invitation(project.base.id, Role::Admin))
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn accepting_a_role_invitation_rechecks_the_role_its_maker_may_give_now() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let manager = person(&world, "manager").await;
    let guest = person(&world, "guest").await;
    let org = make_org(&world, &owner, "Office", "office").await;
    let project = make_project(&world, &owner, org.base.id, "Work").await;
    a_manager(&world, &owner, &manager, org.base.id, project.base.id).await;
    let role = {
        let mut conn = world.conn().await;
        testing::give_role(
            &mut conn,
            project.base.id,
            "user",
            manager.id(),
            "developer",
            owner.id(),
            None,
        )
        .await
        .unwrap()
    };
    let invite = world
        .app
        .call(InvitationCreate, &[])
        .token(&manager.token)
        .body(&invitation(project.base.id, Role::Developer))
        .send()
        .await
        .ok();
    {
        let mut conn = world.conn().await;
        access::remove_role(&mut conn, role, owner.id(), world.now())
            .await
            .unwrap();
    }
    let problem = world
        .app
        .call(InvitationAccept, &[])
        .token(&guest.token)
        .body(&InvitationAcceptBody {
            token: invite.token,
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    let mut conn = world.conn().await;
    assert!(
        access::role_at(&mut conn, project.base.id, guest.id())
            .await
            .unwrap()
            .is_none()
    );
}
