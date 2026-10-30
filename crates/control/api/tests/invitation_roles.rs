//! An invitation gives a role only within what its maker may give, at creation and at acceptance.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

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
