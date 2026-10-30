//! Deleting a node, bringing it back until it is purged, and passing a node to another owner, over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use chrono::Duration;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::common::PageQuery;
use orch_api_contract::nodes::{Node, NodeGet, NodeKind};
use orch_api_contract::ownership::{
    NodeDelete, NodeDeleteBody, NodeDeleteMembers, NodeDeleted, NodeDeletedList, NodeRestore,
    NodeTransfer, OrgDeletedList, TransferBody,
};
use orch_types::Id;
use places::{
    Who, children, join, make_department, make_org, make_project, make_topic, person, read,
};
use support::World;

struct Setup {
    boss: Who,
    dan: Who,
    acme: Id,
    web: Id,
    login: Id,
}

async fn setup(world: &World) -> Setup {
    let boss = person(world, "boss").await;
    let dan = person(world, "dan").await;
    let acme = make_org(world, &boss, "Acme", "acme").await;
    join(world, acme.base.id, acme.base.id, &dan, "viewer", &boss).await;
    let web = make_project(world, &boss, acme.base.id, "Web").await;
    let login = make_topic(world, &boss, web.base.id, "Login").await;
    Setup {
        boss,
        dan,
        acme: acme.base.id,
        web: web.base.id,
        login: login.base.id,
    }
}

async fn delete(world: &World, who: &Who, node: Id) -> orch_api_core::testing::Reply<NodeDeleted> {
    world
        .app
        .call(NodeDelete, &[&node])
        .token(&who.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await
}

#[tokio::test]
async fn a_project_is_deleted_with_what_is_in_it_and_brought_back_archived_until_it_is_purged() {
    let world = World::new().await;
    let s = setup(&world).await;
    // Dan, a viewer, deletes nothing.
    let refused = delete(&world, &s.dan, s.web).await.problem();
    assert_eq!(refused.extra["required"], "project.manage");
    let gone = delete(&world, &s.boss, s.web).await;
    assert_eq!(gone.status, 200);
    // It and its topic are gone from view, and not found.
    for node in [s.web, s.login] {
        let missing = world
            .app
            .call(NodeGet, &[&node])
            .token(&s.boss.token)
            .send()
            .await
            .problem();
        assert_eq!(missing.code, ErrorCode::NotFound, "{node}");
    }
    assert_eq!(
        children(&world, &s.boss, s.acme).await.len(),
        1,
        "only General is left"
    );
    // Listed under its parent by whoever manages that, with when it is gone for good.
    let listed = world
        .app
        .call(NodeDeletedList, &[&s.acme])
        .token(&s.boss.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(listed.items.len(), 1);
    let item = &listed.items[0];
    assert_eq!(
        (item.node.id, item.kind, item.children),
        (s.web, NodeKind::Project, 1)
    );
    assert_eq!(item.deleted_by.as_ref().unwrap().username, "boss");
    assert_eq!(
        item.purge_at.get() - item.deleted_at.get(),
        Duration::days(30),
        "the retention is thirty days as shipped"
    );
    let refused = world
        .app
        .call(NodeDeletedList, &[&s.acme])
        .token(&s.dan.token)
        .query(&PageQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.extra["required"], "org.manage");
    // Brought back by whoever manages its parent: archived, with its topic.
    let refused = world
        .app
        .call(NodeRestore, &[&s.web])
        .token(&s.dan.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let back = world
        .app
        .call(NodeRestore, &[&s.web])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    let Node::Project(back) = back else {
        panic!("a project comes back one");
    };
    assert!(back.base.archived && back.base.archived_at.is_some());
    assert_eq!(back.topic_count, 1);
    assert!(matches!(
        read(&world, &s.boss, s.login).await,
        Node::Topic(_)
    ));
    let audit = world.audit().await;
    assert!(audit.iter().any(|action| action == "node.deleted"));
    assert!(audit.iter().any(|action| action == "node.restored"));
    // A node that is not deleted is nothing to restore.
    let nothing = world
        .app
        .call(NodeRestore, &[&s.web])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(nothing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn what_was_deleted_earlier_stays_deleted_when_the_node_above_it_is_brought_back() {
    let world = World::new().await;
    let s = setup(&world).await;
    let billing = make_topic(&world, &s.boss, s.web, "Billing").await;
    assert_eq!(delete(&world, &s.boss, billing.base.id).await.status, 200);
    world.advance(Duration::days(1));
    assert_eq!(delete(&world, &s.boss, s.web).await.status, 200);
    world
        .app
        .call(NodeRestore, &[&s.web])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert!(matches!(
        read(&world, &s.boss, s.login).await,
        Node::Topic(_)
    ));
    let missing = world
        .app
        .call(NodeGet, &[&billing.base.id])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(
        missing.code,
        ErrorCode::NotFound,
        "Billing went first and is not brought back with Web"
    );
}

#[tokio::test]
async fn a_node_deleted_longer_ago_than_the_retention_is_gone_for_good() {
    let world = World::new().await;
    let s = setup(&world).await;
    delete(&world, &s.boss, s.web).await.ok();
    world.advance(Duration::days(31));
    let gone = world
        .app
        .call(NodeRestore, &[&s.web])
        .token(&s.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::Gone);
    let listed = world
        .app
        .call(NodeDeletedList, &[&s.acme])
        .token(&s.boss.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert!(listed.items.is_empty());
}

#[tokio::test]
async fn a_personal_org_is_never_deleted() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let refused = delete(&world, &ada, ada.person.personal_org)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_department_gives_its_members_to_the_org_above_before_it_goes() {
    let world = World::new().await;
    let s = setup(&world).await;
    let research = make_department(&world, &s.boss, s.acme, "Research", "research").await;
    let eve = person(&world, "eve").await;
    join(&world, s.acme, research.base.id, &eve, "developer", &s.boss).await;
    // Dan, a viewer, deletes nothing; a manager of the department without
    // `members.manage` above cannot take its members along either.
    assert_eq!(
        delete(&world, &s.dan, research.base.id)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let gone = world
        .app
        .call(NodeDelete, &[&research.base.id])
        .token(&s.boss.token)
        .body(&NodeDeleteBody {
            members: Some(NodeDeleteMembers {
                role: Some(orch_api_contract::access::Role::Interactor),
            }),
            resources: None,
            preflight_id: None,
            expected_revision: None,
        })
        .send()
        .await;
    assert_eq!(gone.status, 200);
    // Eve is a member of Acme with the department's org above as her domain.
    let members = world
        .app
        .call(orch_api_contract::directory::OrgMemberList, &[&s.acme])
        .token(&s.boss.token)
        .query(&orch_api_contract::directory::OrgMemberListQuery::default())
        .send()
        .await
        .ok();
    let eve_member = members
        .items
        .iter()
        .find(|member| member.user.username == "eve")
        .expect("she is still a member of the tree");
    assert_eq!(eve_member.domain.id, s.acme);
    assert_eq!(eve_member.role, orch_api_contract::access::Role::Interactor);
    assert!(matches!(read(&world, &eve, s.web).await, Node::Project(_)));
}

#[tokio::test]
async fn a_root_org_is_deleted_listed_for_its_owners_and_restored_by_one_of_them() {
    let world = World::new().await;
    let s = setup(&world).await;
    assert_eq!(delete(&world, &s.boss, s.acme).await.status, 200);
    let listed = world
        .app
        .call(OrgDeletedList, &[])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].slug, "acme");
    assert!(
        world
            .app
            .call(OrgDeletedList, &[])
            .token(&s.dan.token)
            .send()
            .await
            .ok()
            .is_empty()
    );
    // Neither Dan nor a stranger brings it back; its owner does.
    for who in [&s.dan] {
        let refused = world
            .app
            .call(NodeRestore, &[&s.acme])
            .token(&who.token)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
    }
    let back = world
        .app
        .call(NodeRestore, &[&s.acme])
        .token(&s.boss.token)
        .send()
        .await
        .ok();
    assert!(matches!(back, Node::Org(_)));
    // A system admin brings back one deleted by its owner.
    let root = places::admin(&world, "root").await;
    delete(&world, &s.boss, s.acme).await;
    let back = world
        .app
        .call(NodeRestore, &[&s.acme])
        .token(&root.token)
        .send()
        .await
        .ok();
    assert!(matches!(back, Node::Org(_)));
}

#[tokio::test]
async fn the_first_place_among_the_owners_is_handed_on_and_the_previous_stays_unless_told_not_to() {
    let world = World::new().await;
    let s = setup(&world).await;
    let transfer = |who: &Who, node: Id, user: Id, keep: Option<bool>| {
        world
            .app
            .call(NodeTransfer, &[&node])
            .token(&who.token)
            .body(&TransferBody {
                user_id: user,
                keep_previous: keep,
            })
            .send()
    };
    let handed = transfer(&s.boss, s.web, s.dan.id(), None).await.ok();
    let Node::Project(handed) = handed else {
        panic!("a project stays one");
    };
    assert_eq!(handed.base.owner.username, "dan");
    assert_eq!(handed.base.owners, 2, "the previous owner stays");
    // Dan, now an owner, hands it on and takes the previous out.
    let again = transfer(&s.dan, s.web, s.boss.id(), Some(false)).await.ok();
    let Node::Project(again) = again else {
        panic!("a project stays one");
    };
    assert_eq!(again.base.owner.username, "boss");
    assert_eq!(again.base.owners, 1);
    assert!(
        world
            .audit()
            .await
            .iter()
            .any(|action| action == "node.transferred")
    );
    // Somebody who owns nothing there cannot.
    let eve = person(&world, "eve").await;
    join(&world, s.acme, s.acme, &eve, "viewer", &s.boss).await;
    let refused = transfer(&eve, s.web, eve.id(), None).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    // And the new owner must be a member of the root org.
    let zed = person(&world, "zed").await;
    let refused = transfer(&s.boss, s.web, zed.id(), None).await.problem();
    assert!(
        refused.code == ErrorCode::SharingExternalForbidden
            || refused.code == ErrorCode::RequestInvalid,
        "{:?}",
        refused.code
    );
}
