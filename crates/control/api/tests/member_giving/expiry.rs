//! A move preserves the expiry of the role it carries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
use crate::{places, support};
use orch_api_contract::access::{NodeRoleList, Role};
use orch_api_contract::ownership::{MemberMoveBody, OrgMemberMove};
use orch_types::Time;
use places::{give_role, join, make_department, make_org, person};
use support::World;

#[tokio::test]
async fn moving_an_expiring_domain_role_does_not_make_it_lasting() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let member = person(&world, "member").await;
    let root = make_org(&world, &boss, "Acme", "acme").await.base.id;
    let from = make_department(&world, &boss, root, "From", "from")
        .await
        .base
        .id;
    let to = make_department(&world, &boss, root, "To", "to")
        .await
        .base
        .id;
    join(&world, root, from, &member, "developer", &boss).await;
    let end = world.now() + chrono::Duration::days(1);
    sqlx::query("UPDATE node_roles SET expires_at = $3 WHERE node_id = $1 AND subject_kind = 'user' AND subject_id = $2 AND removed_at IS NULL")
        .bind(from).bind(member.id()).bind(end).execute(&world.db.pool).await.unwrap();
    world
        .app
        .call(OrgMemberMove, &[&root, &member.id()])
        .token(&boss.token)
        .body(&MemberMoveBody { to, role: None })
        .send()
        .await
        .ok();
    let roles = world
        .app
        .call(NodeRoleList, &[&to])
        .token(&boss.token)
        .send()
        .await
        .ok();
    let carried = roles
        .own
        .iter()
        .find(|role| {
            role.subject
                .user
                .as_ref()
                .is_some_and(|user| user.id == member.id())
        })
        .unwrap();
    assert_eq!(carried.role, Role::Developer);
    assert_eq!(carried.expires_at, Some(Time::new(end)));
    world.advance(chrono::Duration::days(2));
    let later = world
        .app
        .call(NodeRoleList, &[&to])
        .token(&boss.token)
        .send()
        .await
        .ok();
    assert!(!later.own.iter().any(|role| {
        role.subject
            .user
            .as_ref()
            .is_some_and(|user| user.id == member.id())
    }));
}

#[tokio::test]
async fn an_expiring_higher_role_keeps_the_existing_lasting_role_below_it() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let member = person(&world, "member").await;
    let root = make_org(&world, &boss, "Acme", "acme").await.base.id;
    let from = make_department(&world, &boss, root, "From", "from")
        .await
        .base
        .id;
    let to = make_department(&world, &boss, root, "To", "to")
        .await
        .base
        .id;
    join(&world, root, from, &member, "viewer", &boss).await;
    give_role(&world, to, &member, "viewer", &boss).await;
    let end = world.now() + chrono::Duration::days(1);
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::give_role(
        &mut conn,
        from,
        "user",
        member.id(),
        "developer",
        boss.id(),
        Some(end),
    )
    .await
    .unwrap();
    drop(conn);
    world
        .app
        .call(OrgMemberMove, &[&root, &member.id()])
        .token(&boss.token)
        .body(&MemberMoveBody { to, role: None })
        .send()
        .await
        .ok();
    let roles = world
        .app
        .call(NodeRoleList, &[&to])
        .token(&boss.token)
        .send()
        .await
        .ok();
    let held: Vec<_> = roles
        .own
        .iter()
        .filter(|role| {
            role.subject
                .user
                .as_ref()
                .is_some_and(|user| user.id == member.id())
        })
        .collect();
    assert!(
        held.iter()
            .any(|role| role.role == Role::Viewer && role.expires_at.is_none())
    );
    assert!(
        held.iter()
            .any(|role| role.role == Role::Developer && role.expires_at == Some(Time::new(end)))
    );
    world.advance(chrono::Duration::days(2));
    let later = world
        .app
        .call(NodeRoleList, &[&to])
        .token(&boss.token)
        .send()
        .await
        .ok();
    assert!(later.own.iter().any(|role| {
        role.subject
            .user
            .as_ref()
            .is_some_and(|user| user.id == member.id())
            && role.role == Role::Viewer
    }));
    assert!(!later.own.iter().any(|role| {
        role.subject
            .user
            .as_ref()
            .is_some_and(|user| user.id == member.id())
            && role.role == Role::Developer
    }));
}

#[tokio::test]
async fn an_existing_temporary_role_does_not_erase_the_moved_lasting_role() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let member = person(&world, "member").await;
    let root = make_org(&world, &boss, "Acme", "acme").await.base.id;
    let from = make_department(&world, &boss, root, "From", "from")
        .await
        .base
        .id;
    let to = make_department(&world, &boss, root, "To", "to")
        .await
        .base
        .id;
    join(&world, root, from, &member, "viewer", &boss).await;
    let end = world.now() + chrono::Duration::days(1);
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::give_role(
        &mut conn,
        to,
        "user",
        member.id(),
        "developer",
        boss.id(),
        Some(end),
    )
    .await
    .unwrap();
    drop(conn);
    let moved = world
        .app
        .call(OrgMemberMove, &[&root, &member.id()])
        .token(&boss.token)
        .body(&MemberMoveBody { to, role: None })
        .send()
        .await
        .ok();
    assert_eq!(moved.role, Role::Developer);
    world.advance(chrono::Duration::days(2));
    let later = world
        .app
        .call(NodeRoleList, &[&to])
        .token(&boss.token)
        .send()
        .await
        .ok();
    assert!(later.own.iter().any(|role| {
        role.subject
            .user
            .as_ref()
            .is_some_and(|user| user.id == member.id())
            && role.role == Role::Viewer
            && role.expires_at.is_none()
    }));
    assert!(!later.own.iter().any(|role| {
        role.subject
            .user
            .as_ref()
            .is_some_and(|user| user.id == member.id())
            && role.role == Role::Developer
    }));
}

#[tokio::test]
async fn an_explicit_lower_role_keeps_its_own_expiry() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let member = person(&world, "member").await;
    let root = make_org(&world, &boss, "Acme", "acme").await.base.id;
    let from = make_department(&world, &boss, root, "From", "from")
        .await
        .base
        .id;
    let to = make_department(&world, &boss, root, "To", "to")
        .await
        .base
        .id;
    join(&world, root, from, &member, "viewer", &boss).await;
    let end = world.now() + chrono::Duration::days(1);
    let mut conn = world.conn().await;
    orch_db::queries::access::testing::give_role(
        &mut conn,
        from,
        "user",
        member.id(),
        "developer",
        boss.id(),
        Some(end),
    )
    .await
    .unwrap();
    drop(conn);
    world
        .app
        .call(OrgMemberMove, &[&root, &member.id()])
        .token(&boss.token)
        .body(&MemberMoveBody {
            to,
            role: Some(Role::Viewer),
        })
        .send()
        .await
        .ok();
    world.advance(chrono::Duration::days(2));
    let later = world
        .app
        .call(NodeRoleList, &[&to])
        .token(&boss.token)
        .send()
        .await
        .ok();
    assert!(later.own.iter().any(|role| {
        role.subject
            .user
            .as_ref()
            .is_some_and(|user| user.id == member.id())
            && role.role == Role::Viewer
            && role.expires_at.is_none()
    }));
}
