//! A reviewed deletion: the plan counts every node and document it removes, says until when and by
//! whom it can be brought back, and the deletion that names it removes exactly that set or nothing.
//! The plan is one-use and the caller's own; a replaced child refuses it even when the totals are
//! the same. What ending a membership touches counts documents, those owned with others by kind.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::nodes::NodeOrSystem;
use orch_api_contract::ownership::{
    NodeDelete, NodeDeleteBody, NodeDeletePlan, NodeDeletePreview, NodeDeleted, OrgMemberHoldings,
};
use orch_api_core::testing::Reply;
use orch_types::Id;
use places::{Who, make_org, make_project, make_topic, person};
use support::World;

async fn preview(world: &World, by: &Who, node: Id) -> Reply<NodeDeletePlan> {
    world
        .app
        .call(NodeDeletePreview, &[&node])
        .token(&by.token)
        .send()
        .await
}

async fn delete(
    world: &World,
    by: &Who,
    node: Id,
    plan: Option<&NodeDeletePlan>,
) -> Reply<NodeDeleted> {
    world
        .app
        .call(NodeDelete, &[&node])
        .token(&by.token)
        .body(&NodeDeleteBody {
            preflight_id: plan.map(|plan| plan.preflight_id),
            expected_revision: plan.map(|plan| plan.revision),
            ..NodeDeleteBody::default()
        })
        .send()
        .await
}

async fn document(world: &World, by: &Who, node: Id, title: &str) {
    world
        .app
        .call(NodeDocumentCreate, &[&node])
        .token(&by.token)
        .body(&NodeDocumentCreateBody {
            title: title.to_owned(),
            blocks: None,
            markdown: Some(format!("# {title}")),
            access: None,
        })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn a_plan_counts_documents_and_says_until_when_and_by_whom_and_its_deletion_removes_that() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let login = make_topic(&world, &boss, web.base.id, "Login").await;
    make_topic(&world, &boss, web.base.id, "Signup").await;
    document(&world, &boss, web.base.id, "Plan").await;
    document(&world, &boss, login.base.id, "Notes").await;

    let plan = preview(&world, &boss, web.base.id).await.ok();
    assert_eq!(
        (
            plan.counts.orgs,
            plan.counts.projects,
            plan.counts.topics,
            plan.counts.threads,
            plan.counts.documents
        ),
        (0, 1, 2, 0, 2),
        "the project, its two topics and the documents beside it and below it"
    );
    assert_eq!(plan.node.id, web.base.id);
    assert!(
        plan.purge_at.get() > world.now(),
        "it can be brought back until a later day"
    );
    assert_eq!(plan.restore_by.permission, "org.manage");
    match &plan.restore_by.place {
        NodeOrSystem::Node(place) => assert_eq!(place.id, acme.base.id),
        NodeOrSystem::System(_) => panic!("a project is brought back by whoever manages its org"),
        NodeOrSystem::DocumentContainer(_) => {
            panic!("a project is brought back by whoever manages its org")
        }
    }
    let root = preview(&world, &boss, acme.base.id).await.ok();
    assert_eq!(
        root.restore_by.permission, "access.own",
        "a root org: one of its owners"
    );
    assert_eq!(root.counts.orgs, 1);

    assert_eq!(
        delete(&world, &boss, web.base.id, Some(&plan)).await.status,
        200
    );
    let gone = world
        .app
        .call(orch_api_contract::nodes::NodeGet, &[&web.base.id])
        .token(&boss.token)
        .send()
        .await;
    assert_eq!(gone.status, 404, "the reviewed set is deleted");
}

#[tokio::test]
async fn a_replaced_child_with_the_same_totals_refuses_the_plan_with_the_plan_as_it_now_is() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let login = make_topic(&world, &boss, web.base.id, "Login").await;
    let plan = preview(&world, &boss, web.base.id).await.ok();
    assert_eq!(plan.counts.topics, 1);

    // One topic goes and another comes: the totals are what the plan showed, the set is not.
    assert_eq!(delete(&world, &boss, login.base.id, None).await.status, 200);
    make_topic(&world, &boss, web.base.id, "Signup").await;

    let stale = delete(&world, &boss, web.base.id, Some(&plan))
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let current: NodeDeletePlan = serde_json::from_value(stale.extra["current"].clone()).unwrap();
    assert_eq!(current.counts.topics, 1);
    assert_ne!(
        current.preflight_id, plan.preflight_id,
        "a fresh plan to confirm"
    );
    let still = world
        .app
        .call(orch_api_contract::nodes::NodeGet, &[&web.base.id])
        .token(&boss.token)
        .send()
        .await;
    assert_eq!(still.status, 200, "nothing was deleted");

    // The plan was used up by that refusal; the fresh one deletes.
    let used = delete(&world, &boss, web.base.id, Some(&plan))
        .await
        .problem();
    assert_eq!(used.code, ErrorCode::PreflightExpired);
    assert_eq!(
        delete(&world, &boss, web.base.id, Some(&current))
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn a_plan_is_its_callers_own_and_of_its_node() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let eve = person(&world, "eve").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    let api = make_project(&world, &boss, acme.base.id, "Api").await;
    let plan = preview(&world, &boss, web.base.id).await.ok();

    let elsewhere = delete(&world, &boss, api.base.id, Some(&plan))
        .await
        .problem();
    assert_eq!(
        elsewhere.code,
        ErrorCode::PreflightExpired,
        "a plan of another node"
    );
    let refused = preview(&world, &eve, web.base.id).await.problem();
    assert!(
        matches!(refused.code, ErrorCode::NotFound | ErrorCode::AuthzDenied),
        "eve may not delete the project, so she reads no plan of it: {refused:?}"
    );
    // The plan that was refused for another node is still boss's to confirm for its own.
    assert_eq!(
        delete(&world, &boss, web.base.id, Some(&plan)).await.status,
        200
    );
}

#[tokio::test]
async fn holdings_count_documents_and_what_is_owned_with_others_by_kind() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let acme = make_org(&world, &boss, "Acme", "acme").await;
    let web = make_project(&world, &boss, acme.base.id, "Web").await;
    document(&world, &boss, web.base.id, "Plan").await;
    let held = world
        .app
        .call(OrgMemberHoldings, &[&acme.base.id, &"me"])
        .token(&boss.token)
        .send()
        .await
        .ok();
    assert_eq!(held.nodes.documents, 1, "the document is hers alone");
    assert_eq!(held.shared_by_kind.documents, 0);
    assert_eq!(
        held.shared,
        held.shared_by_kind.orgs
            + held.shared_by_kind.projects
            + held.shared_by_kind.topics
            + held.shared_by_kind.threads
            + held.shared_by_kind.documents
            + held.shared_by_kind.resources,
        "the kinds add up to what is shared"
    );
}
