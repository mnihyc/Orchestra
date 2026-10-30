//! Manage is the thing itself over HTTP: deleting a node, a document, a resource or an environment, bringing back what was deleted and moving it are done by whoever holds its Manage, and are refused with that Manage named to whoever holds View or the level between.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod resources;
mod support;

use desk::Desk;
use orch_api_contract::common::{ErrorCode, Problem};
use orch_api_contract::documents::{
    DocumentDelete, DocumentMove, DocumentMoveBody, DocumentRestore, DocumentVersionRestore,
};
use orch_api_contract::environment::{EnvironmentDelete, EnvironmentDeleteQuery};
use orch_api_contract::nodes::{Node, NodeMoveBody, Org, OrgMove, Project, ProjectMove};
use orch_api_contract::ownership::{NodeDelete, NodeDeleteBody, NodeDeleted, NodeRestore};
use orch_api_contract::resources::{AgentDelete, ProviderDelete, TemplateDelete};
use orch_api_core::testing::Reply;
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::Id;
use places::{Who, make_department, person, read};
use resources::{company, name_person};
use support::World;

/// Said of a refusal: it is the refusal of the Manage that was needed, and says which.
fn refused_for(problem: &Problem, required: &str) {
    assert_eq!(problem.code, ErrorCode::AuthzDenied, "{problem:?}");
    assert_eq!(problem.extra["required"], required, "{problem:?}");
}

/// The people of an org at each role that is below an admin.
struct People {
    viewer: Who,
    worker: Who,
    developer: Who,
    admin: Who,
}

async fn people(desk: &Desk) -> People {
    People {
        viewer: desk.member("vera", "viewer").await,
        worker: desk.member("walt", "interactor").await,
        developer: desk.member("dina", "developer").await,
        admin: desk.member("axel", "admin").await,
    }
}

async fn delete(world: &World, who: &Who, node: Id) -> Reply<NodeDeleted> {
    world
        .app
        .call(NodeDelete, &[&node])
        .token(&who.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await
}

async fn restore(world: &World, who: &Who, node: Id) -> Reply<Node> {
    world
        .app
        .call(NodeRestore, &[&node])
        .token(&who.token)
        .send()
        .await
}

async fn revision(world: &World, who: &Who, id: Id) -> i64 {
    match read(world, who, id).await {
        Node::Org(node) => node.base.revision,
        Node::Project(node) => node.base.revision,
        Node::Topic(node) => node.base.revision,
        Node::Thread(node) => node.base.revision,
    }
}

async fn move_project(world: &World, who: &Who, project: Id, under: Id) -> Reply<Project> {
    let body = NodeMoveBody {
        parent_id: under,
        preflight_id: None,
        expected_revision: revision(world, who, project).await,
    };
    world
        .app
        .call(ProjectMove, &[&project])
        .token(&who.token)
        .body(&body)
        .send()
        .await
}

async fn move_org(world: &World, who: &Who, org: Id, under: Id) -> Reply<Org> {
    let body = NodeMoveBody {
        parent_id: under,
        preflight_id: None,
        expected_revision: revision(world, who, org).await,
    };
    world
        .app
        .call(OrgMove, &[&org])
        .token(&who.token)
        .body(&body)
        .send()
        .await
}

#[tokio::test]
async fn a_topic_a_project_and_a_department_are_deleted_brought_back_and_moved_by_their_manage_alone()
 {
    let desk = Desk::new().await;
    let world = &desk.world;
    let who = people(&desk).await;
    let lower = [&who.viewer, &who.worker, &who.developer];
    let ops = make_department(world, &desk.ada, desk.org, "Ops", "ops")
        .await
        .base
        .id;
    let sales = make_department(world, &desk.ada, desk.org, "Sales", "sales")
        .await
        .base
        .id;

    // A topic: a Developer manages it, a Viewer and an Interactor do not.
    for each in [&who.viewer, &who.worker] {
        refused_for(
            &delete(world, each, desk.topic).await.problem(),
            "topic.manage",
        );
    }
    assert_eq!(delete(world, &who.developer, desk.topic).await.status, 200);
    // Bringing it back is the Manage of what it lay in: of the project, which is an admin's.
    for each in lower {
        refused_for(
            &restore(world, each, desk.topic).await.problem(),
            "project.manage",
        );
    }
    restore(world, &who.admin, desk.topic).await.ok();

    // A project: deleting and moving it are its own Manage, which is an admin's.
    for each in lower {
        refused_for(
            &delete(world, each, desk.project).await.problem(),
            "project.manage",
        );
        refused_for(
            &move_project(world, each, desk.project, ops).await.problem(),
            "project.manage",
        );
    }
    let moved = move_project(world, &who.admin, desk.project, ops)
        .await
        .ok();
    assert_eq!(moved.base.parent_id, Some(ops));
    assert_eq!(delete(world, &who.admin, desk.project).await.status, 200);
    // It lay in Ops now, and it is brought back by the Manage of Ops.
    for each in lower {
        refused_for(
            &restore(world, each, desk.project).await.problem(),
            "org.manage",
        );
    }
    restore(world, &who.admin, desk.project).await.ok();

    // A department of the org: the same.
    for each in lower {
        refused_for(&delete(world, each, sales).await.problem(), "org.manage");
        refused_for(
            &move_org(world, each, sales, ops).await.problem(),
            "org.manage",
        );
    }
    move_org(world, &who.admin, sales, ops).await.ok();
    assert_eq!(delete(world, &who.admin, sales).await.status, 200);
    for each in lower {
        refused_for(&restore(world, each, sales).await.problem(), "org.manage");
    }
    restore(world, &who.admin, sales).await.ok();
}

#[tokio::test]
async fn a_document_is_deleted_brought_back_moved_and_put_back_to_an_earlier_version_by_its_manage_alone()
 {
    let desk = Desk::new().await;
    let world = &desk.world;
    let who = people(&desk).await;
    let id = desk.make(&desk.ada, "Handbook").await.base.id;
    let elsewhere = desk.another_topic("Elsewhere").await;
    for each in [&who.viewer, &who.worker] {
        let deleted = world
            .app
            .call(DocumentDelete, &[&id])
            .token(&each.token)
            .send()
            .await
            .problem();
        refused_for(&deleted, "doc.manage");
        let moved = world
            .app
            .call(DocumentMove, &[&id])
            .token(&each.token)
            .body(&DocumentMoveBody { node_id: elsewhere })
            .send()
            .await
            .problem();
        refused_for(&moved, "doc.manage");
        let put_back = world
            .app
            .call(DocumentVersionRestore, &[&id, &1])
            .token(&each.token)
            .send()
            .await
            .problem();
        refused_for(&put_back, "doc.manage");
    }
    // A Developer manages the documents beside a node.
    world
        .app
        .call(DocumentMove, &[&id])
        .token(&who.developer.token)
        .body(&DocumentMoveBody { node_id: elsewhere })
        .send()
        .await
        .ok();
    let gone = world
        .app
        .call(DocumentDelete, &[&id])
        .token(&who.developer.token)
        .send()
        .await;
    assert_eq!(gone.status, 204);
    for each in [&who.viewer, &who.worker] {
        let refused = world
            .app
            .call(DocumentRestore, &[&id])
            .token(&each.token)
            .raw_body(&serde_json::json!({}))
            .send()
            .await
            .problem();
        refused_for(&refused, "doc.manage");
    }
    let back = world
        .app
        .call(DocumentRestore, &[&id])
        .token(&who.developer.token)
        .raw_body(&serde_json::json!({}))
        .send()
        .await
        .ok();
    assert!(!back.base.deleted);
}

async fn delete_resource(world: &World, kind: &str, id: Id, who: &Who) -> Reply<()> {
    let token = who.token.as_str();
    match kind {
        "provider" => {
            world
                .app
                .call(ProviderDelete, &[&id])
                .token(token)
                .send()
                .await
        }
        "agent" => {
            world
                .app
                .call(AgentDelete, &[&id])
                .token(token)
                .send()
                .await
        }
        "template" => {
            world
                .app
                .call(TemplateDelete, &[&id])
                .token(token)
                .send()
                .await
        }
        other => panic!("no request deletes a {other} in this test"),
    }
}

#[tokio::test]
async fn a_provider_an_agent_profile_and_a_template_are_deleted_by_their_manage_alone() {
    let world = World::new().await;
    let c = company(&world).await;
    let (reader, worker, lead) = (
        person(&world, "reader").await,
        person(&world, "worker").await,
        person(&world, "lead").await,
    );
    for each in [&reader, &worker, &lead] {
        places::join(&world, c.acme, c.acme, each, "viewer", &c.boss).await;
    }
    for (kind, view, using, manage) in [
        (
            "provider",
            "provider.view",
            "provider.use",
            "provider.manage",
        ),
        ("agent", "agent.view", "agent.use", "agent.manage"),
        (
            "template",
            "template.view",
            "template.use",
            "template.manage",
        ),
    ] {
        let mut conn = world.conn().await;
        let id = fixtures::insert_resource(&mut conn, kind, Some(c.boss.id()), Some(c.acme), kind)
            .await
            .unwrap();
        drop(conn);
        name_person(&world, kind, id, view, reader.id()).await;
        name_person(&world, kind, id, using, worker.id()).await;
        name_person(&world, kind, id, manage, lead.id()).await;
        for each in [&reader, &worker] {
            let refused = delete_resource(&world, kind, id, each).await.problem();
            refused_for(&refused, manage);
        }
        let gone = delete_resource(&world, kind, id, &lead).await;
        assert_eq!(gone.status, 204, "{kind}: {:?}", gone.body);
    }
}

#[tokio::test]
async fn an_environment_is_deleted_by_its_manage_alone() {
    let world = World::new().await;
    let c = company(&world).await;
    let (reader, lead) = (person(&world, "reader").await, person(&world, "lead").await);
    for each in [&reader, &lead] {
        places::join(&world, c.acme, c.acme, each, "viewer", &c.boss).await;
    }
    let mut conn = world.conn().await;
    let id =
        fixtures::insert_environment(&mut conn, c.boss.id(), Some(c.acme), "Main", "connected")
            .await
            .unwrap();
    drop(conn);
    name_person(&world, "environment", id, "environment.view", reader.id()).await;
    name_person(&world, "environment", id, "environment.manage", lead.id()).await;
    let query = EnvironmentDeleteQuery {
        confirm: "Main".to_owned(),
        at: None,
    };
    let refused = world
        .app
        .call(EnvironmentDelete, &[&id])
        .token(&reader.token)
        .query(&query)
        .send()
        .await
        .problem();
    refused_for(&refused, "environment.manage");
    world
        .app
        .call(EnvironmentDelete, &[&id])
        .token(&lead.token)
        .query(&query)
        .send()
        .await
        .ok();
}
