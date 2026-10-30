//! Archive reviews are read-only, fence exact subtree identities and provide an inverse that cannot overwrite a newer archive.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{
    Node, NodeArchive, NodeArchiveBody, NodeArchivePlan, NodeArchivePreview, NodeArchiveResult,
    NodeGet, NodeUnarchive, NodeUnarchivePreview,
};
use orch_api_contract::ownership::{NodeDelete, NodeDeleteBody};
use orch_api_core::testing::Reply;
use orch_types::Id;
use places::{Who, give_role, make_org, make_project, make_topic, person};
use support::World;

async fn plan(world: &World, who: &Who, node: Id, archiving: bool) -> NodeArchivePlan {
    if archiving {
        world
            .app
            .call(NodeArchivePreview, &[&node])
            .token(&who.token)
            .send()
            .await
            .ok()
    } else {
        world
            .app
            .call(NodeUnarchivePreview, &[&node])
            .token(&who.token)
            .send()
            .await
            .ok()
    }
}

async fn apply(world: &World, who: &Who, plan: &NodeArchivePlan) -> Reply<NodeArchiveResult> {
    let body = NodeArchiveBody {
        preflight_id: plan.preflight_id,
        stamp: plan.stamp.clone(),
        expected_revision: plan.revision,
    };
    if plan.archiving {
        world
            .app
            .call(NodeArchive, &[&plan.node.id])
            .token(&who.token)
            .body(&body)
            .send()
            .await
    } else {
        world
            .app
            .call(NodeUnarchive, &[&plan.node.id])
            .token(&who.token)
            .body(&body)
            .send()
            .await
    }
}

#[tokio::test]
async fn reviewing_names_the_subtree_without_archiving_it_and_the_act_requires_that_review() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let org = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, org.base.id, "Web").await;
    let topic = make_topic(&world, &boss, web.base.id, "Login").await;
    let reviewed = plan(&world, &boss, web.base.id, true).await;
    assert_eq!((reviewed.counts.projects, reviewed.counts.topics), (1, 1));
    assert_eq!(reviewed.subtree.len(), 2);
    assert!(
        reviewed
            .subtree
            .iter()
            .any(|row| row.node.id == topic.base.id && row.read_only_after)
    );
    let before = world
        .app
        .call(NodeGet, &[&web.base.id])
        .token(&boss.token)
        .send()
        .await
        .ok();
    assert!(
        matches!(before, Node::Project(ref row) if !row.base.archived && row.base.revision == web.base.revision)
    );
    let missing = world
        .app
        .call(NodeArchive, &[&web.base.id])
        .token(&boss.token)
        .send()
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::RequestInvalid);
    let archived = apply(&world, &boss, &reviewed).await.ok();
    assert!(matches!(archived.node, Node::Project(ref row) if row.base.archived));
    assert!(archived.undo.is_some());
}

#[tokio::test]
async fn replacing_a_child_with_equal_counts_refuses_every_effect_and_supplies_a_new_review() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let org = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, org.base.id, "Web").await;
    let old = make_topic(&world, &boss, web.base.id, "Old").await;
    let reviewed = plan(&world, &boss, web.base.id, true).await;
    world
        .app
        .call(NodeDelete, &[&old.base.id])
        .token(&boss.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await
        .ok();
    let new = make_topic(&world, &boss, web.base.id, "New").await;
    let stale = apply(&world, &boss, &reviewed).await.problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let current: NodeArchivePlan = serde_json::from_value(stale.extra["current"].clone()).unwrap();
    assert_eq!(current.counts, reviewed.counts);
    assert_ne!(current.stamp, reviewed.stamp);
    assert!(current.subtree.iter().any(|row| row.node.id == new.base.id));
    let still = world
        .app
        .call(NodeGet, &[&web.base.id])
        .token(&boss.token)
        .send()
        .await
        .ok();
    assert!(matches!(still, Node::Project(ref row) if !row.base.archived));
    assert_eq!(
        apply(&world, &boss, &reviewed).await.problem().code,
        ErrorCode::PreflightExpired
    );
    apply(&world, &boss, &current).await.ok();
}

#[tokio::test]
async fn an_inverse_keeps_an_independently_archived_child_and_refuses_a_newer_archive_cycle() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let org = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, org.base.id, "Web").await;
    let topic = make_topic(&world, &boss, web.base.id, "Already shelved").await;
    apply(
        &world,
        &boss,
        &plan(&world, &boss, topic.base.id, true).await,
    )
    .await
    .ok();
    let archived = apply(&world, &boss, &plan(&world, &boss, web.base.id, true).await)
        .await
        .ok();
    let undo = archived.undo.unwrap();
    assert!(
        undo.subtree
            .iter()
            .any(|row| row.node.id == topic.base.id && row.own_archived && row.read_only_after)
    );
    apply(&world, &boss, &undo).await.ok();
    let child = world
        .app
        .call(NodeGet, &[&topic.base.id])
        .token(&boss.token)
        .send()
        .await
        .ok();
    assert!(matches!(child, Node::Topic(ref row) if row.base.archived));
    let next = apply(&world, &boss, &plan(&world, &boss, web.base.id, true).await)
        .await
        .ok();
    let old_again = apply(&world, &boss, &undo).await.problem();
    assert_eq!(old_again.code, ErrorCode::PreflightExpired);
    assert!(next.undo.is_some());
}

#[tokio::test]
async fn a_review_cannot_be_used_for_another_node_or_after_it_expires() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let org = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, org.base.id, "Web").await;
    let other = make_project(&world, &boss, org.base.id, "Other").await;
    let reviewed = plan(&world, &boss, web.base.id, true).await;
    let body = NodeArchiveBody {
        preflight_id: reviewed.preflight_id,
        stamp: reviewed.stamp.clone(),
        expected_revision: reviewed.revision,
    };
    assert_eq!(
        world
            .app
            .call(NodeArchive, &[&other.base.id])
            .token(&boss.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::PreflightExpired
    );
    assert_eq!(
        world
            .app
            .call(NodeUnarchive, &[&web.base.id])
            .token(&boss.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::PreflightExpired
    );
    let manager = person(&world, "manager").await;
    give_role(&world, web.base.id, &manager, "admin", &boss).await;
    assert_eq!(
        world
            .app
            .call(NodeArchive, &[&web.base.id])
            .token(&manager.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::PreflightExpired
    );
    {
        let mut conn = world.conn().await;
        sqlx::query("UPDATE preflights SET expires_at=$2 WHERE id=$1")
            .bind(reviewed.preflight_id)
            .bind(world.now() - chrono::Duration::seconds(1))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    assert_eq!(
        apply(&world, &boss, &reviewed).await.problem().code,
        ErrorCode::PreflightExpired
    );
}
