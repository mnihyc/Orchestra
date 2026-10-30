//! Manage is the thing itself: of every kind of node, of documents, of each kind of resource and of environments, the permission that deletes, restores and moves it is its own Manage, which neither View nor the level between (interact, edit, use) holds, while Manage holds both of those.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_authz::pages::Target;
use orch_authz::{Caller, Policy, documents};
use orch_db::queries::documents as rows;
use orch_types::{Id, new_id};
use serde_json::json;
use support::{World, user_holder};

/// The three people of one kind of thing: one who reads it, one who works with it, and one who
/// manages it, each named in the row of their own level and in no other.
struct Levels {
    reader: Id,
    worker: Id,
    lead: Id,
}

async fn people(w: &World, label: &str) -> Levels {
    Levels {
        reader: w.member(&format!("{label}-reader")).await,
        worker: w.member(&format!("{label}-worker")).await,
        lead: w.member(&format!("{label}-lead")).await,
    }
}

/// What the three levels of a thing hold, as the rows say. `answers` is, for the reader, the worker
/// and the manager in turn, whether they hold View, the level between and Manage: Manage is held by
/// the person named in it and by nobody lower, and each higher level holds the lower ones.
fn ladder(label: &str, answers: &[bool]) {
    assert_eq!(
        answers,
        [
            true, false, false, // the reader reads and does no more
            true, true, false, // the one in the middle reads and works, and manages nothing
            true, true, true, // the manager holds all three
        ],
        "{label}"
    );
}

#[tokio::test]
async fn each_kind_of_node_is_managed_by_its_manage_alone() {
    let w = World::new().await;
    for (label, node, view, middle, manage) in [
        ("org", w.root, P::OrgView, P::OrgInteract, P::OrgManage),
        (
            "project",
            w.project,
            P::ProjectView,
            P::ProjectInteract,
            P::ProjectManage,
        ),
        (
            "topic",
            w.topic,
            P::TopicView,
            P::TopicInteract,
            P::TopicManage,
        ),
        (
            "thread",
            w.thread,
            P::ThreadView,
            P::ThreadInteract,
            P::ThreadManage,
        ),
    ] {
        let who = people(&w, label).await;
        w.name(node, view, Holder::User { id: who.reader }).await;
        w.name(node, middle, Holder::User { id: who.worker }).await;
        w.name(node, manage, Holder::User { id: who.lead }).await;
        let mut answers = Vec::new();
        for user in [who.reader, who.worker, who.lead] {
            for permission in [view, middle, manage] {
                answers.push(w.can(user, node, permission).await);
            }
        }
        ladder(label, &answers);
    }
}

#[tokio::test]
async fn each_kind_of_resource_and_an_environment_is_managed_by_its_manage_alone() {
    let w = World::new().await;
    // A resource of each kind that has a use between viewing and managing, and an environment, which
    // has none: whoever works in it does so through the threads that run there.
    for (kind, home, view, middle, manage) in [
        (
            "provider",
            w.root,
            P::ProviderView,
            P::ProviderUse,
            P::ProviderManage,
        ),
        ("agent", w.root, P::AgentView, P::AgentUse, P::AgentManage),
        (
            "template",
            w.root,
            P::TemplateView,
            P::TemplateUse,
            P::TemplateManage,
        ),
        ("host", w.root, P::HostView, P::HostUse, P::HostManage),
        ("mcp_server", w.root, P::McpView, P::McpUse, P::McpManage),
        ("mount", w.root, P::MountView, P::MountUse, P::MountManage),
    ] {
        let thing = w.thing(kind, w.boss, home, kind).await;
        let who = people(&w, kind).await;
        w.grant(kind, thing, view, Holder::User { id: who.reader })
            .await;
        w.grant(kind, thing, middle, Holder::User { id: who.worker })
            .await;
        w.grant(kind, thing, manage, Holder::User { id: who.lead })
            .await;
        let mut answers = Vec::new();
        for user in [who.reader, who.worker, who.lead] {
            for permission in [view, middle, manage] {
                answers.push(w.can_on(user, kind, thing, permission).await);
            }
        }
        ladder(kind, &answers);
        // The owner holds it, as the owner.
        assert!(
            w.can_on(w.boss, kind, thing, manage).await,
            "{kind}: the owner"
        );
    }
    let environment = w.thing("environment", w.boss, w.topic, "Main").await;
    let who = people(&w, "environment").await;
    w.grant(
        "environment",
        environment,
        P::EnvironmentView,
        Holder::User { id: who.reader },
    )
    .await;
    w.grant(
        "environment",
        environment,
        P::EnvironmentManage,
        Holder::User { id: who.lead },
    )
    .await;
    assert!(
        w.can_on(who.reader, "environment", environment, P::EnvironmentView)
            .await
    );
    assert!(
        !w.can_on(who.reader, "environment", environment, P::EnvironmentManage)
            .await
    );
    assert!(
        w.can_on(who.lead, "environment", environment, P::EnvironmentManage)
            .await
    );
    assert!(
        w.can_on(who.lead, "environment", environment, P::EnvironmentView)
            .await,
        "managing holds viewing"
    );
}

/// A document written beside `node`, by the boss.
async fn document_beside(w: &World, node: Id, title: &str) -> Id {
    let mut conn = w.conn().await;
    let id = new_id();
    rows::insert(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: node,
            org_id: w.root,
            owner_id: w.boss,
            created_by_thread: None,
            created_by_agent: None,
            title,
            now: w.now.get(),
        },
    )
    .await
    .unwrap();
    documents::start(&mut conn, id, w.boss, w.now)
        .await
        .unwrap();
    id
}

async fn holds_on_document(w: &World, user: Id, document: Id, permission: P) -> bool {
    let principal = w.principal(Caller::session(user)).await;
    let mut conn = w.conn().await;
    matches!(
        documents::check(
            &mut conn,
            &principal,
            document,
            permission,
            Policy::default()
        )
        .await
        .unwrap(),
        Decision::Allowed { .. }
    )
}

#[tokio::test]
async fn a_document_is_managed_by_its_manage_alone() {
    let w = World::new().await;
    let document = document_beside(&w, w.topic, "Handbook").await;
    let who = people(&w, "document").await;
    for (user, choice) in [
        (who.reader, "full_view"),
        (who.worker, "full_interact"),
        (who.lead, "full_manage"),
    ] {
        w.change(
            w.boss,
            Target::Resource(ObjectRef {
                kind: ObjectKind::Document,
                id: document,
            }),
            json!({ "name": [{ "holders": [user_holder(user)], "choice": choice }] }),
        )
        .await
        .unwrap();
    }
    let mut answers = Vec::new();
    for user in [who.reader, who.worker, who.lead] {
        for permission in [P::DocView, P::DocEdit, P::DocManage] {
            answers.push(holds_on_document(&w, user, document, permission).await);
        }
    }
    ladder("document", &answers);
}
