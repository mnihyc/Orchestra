//! Mixed group/viewer sources use current ordinary View without changing cached global identities.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{ObjectKind, Permission as P};
use orch_access_contract::roles::Role;
use orch_authz::documents;
use orch_authz::pages::Target;
use orch_authz::{Caller, Policy};
use orch_db::queries::{access, documents as rows};
use orch_types::{Id, new_id};
use serde_json::json;
use support::World;

fn page(document: Id) -> Target {
    Target::Resource(ObjectRef {
        kind: ObjectKind::Document,
        id: document,
    })
}

async fn private_document(w: &World) -> Id {
    let id = new_id();
    let mut conn = w.conn().await;
    rows::containers::insert_document(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: w.topic,
            org_id: w.root,
            owner_id: w.maker,
            created_by_thread: None,
            created_by_agent: None,
            title: "Mixed audience original",
            now: w.now.get(),
        },
        None,
        &format!("Mixed-share~{}.md", id.simple()),
    )
    .await
    .unwrap();
    documents::start(&mut conn, id, w.maker, w.now)
        .await
        .unwrap();
    drop(conn);
    w.change(w.maker, page(id), json!({"whole":"private"}))
        .await
        .unwrap();
    id
}

async fn document_holds(w: &World, person: Id, document: Id, permission: P) -> bool {
    let principal = w.principal(Caller::session(person)).await;
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
async fn a_group_of_topic_viewers_follows_the_topics_current_thread_viewer_source() {
    let w = World::new().await;
    let reader = w.member("reader").await;
    w.give(w.root, w.maker, Role::Viewer).await;
    w.change(w.maker, Target::Live(w.thread), json!({"whole":"private"}))
        .await
        .unwrap();
    w.change(w.boss, Target::Live(w.topic), json!({"whole":"private"}))
        .await
        .unwrap();
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{
            "permission":"thread.view", "add":[{"holder":Holder::User{id:reader}}]
        }]}),
    )
    .await
    .unwrap();
    // The topic writer must be able to name this private source; View gives no control.
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows":[{"permission":"thread.view","add":[{"holder":Holder::User{id:w.boss}}]}]}),
    )
    .await
    .unwrap();
    w.change(w.boss, Target::Live(w.topic), json!({"rows":[{
        "permission":"topic.view", "add":[{"holder":Holder::Role{role:Role::Viewer,at:Some(w.thread)}}]
    }]})).await.unwrap();
    let group = w.group(w.root, "Current topic viewers", &[]).await;
    let mut conn = w.conn().await;
    access::add_group_holder(
        &mut conn,
        group,
        &access::GroupHolderRow {
            user_id: None,
            member_group_id: None,
            member_node_id: Some(w.topic),
            member_org_id: None,
            member_role: Some("viewer".into()),
        },
        w.boss,
        w.now.get(),
    )
    .await
    .unwrap();
    let candidates = access::group_people(&mut conn, &[group], i64::MAX, w.now.get())
        .await
        .unwrap();
    assert!(
        candidates.contains(&(group, reader)),
        "the referenced inverse walk finds this ordinary source"
    );
    drop(conn);
    assert!(w.can(reader, w.thread, P::ThreadView).await);
    assert!(w.can(reader, w.topic, P::TopicView).await);
    let principal = w.principal(Caller::session(reader)).await;
    assert!(
        !principal.groups().contains(&group),
        "thread-derived identities stay outside the global holder cache"
    );
    let document = private_document(&w).await;
    assert!(!document_holds(&w, reader, document, P::DocView).await);
    w.change(
        w.maker,
        page(document),
        json!({"rows":[{
            "permission":"doc.view", "add":[{"holder":Holder::Group{id:group}}]
        }]}),
    )
    .await
    .unwrap();
    assert!(
        document_holds(&w, reader, document, P::DocView).await,
        "the document's referenced group must honor the same current mixed viewer source"
    );
    assert!(!document_holds(&w, reader, document, P::DocEdit).await);
    assert!(!document_holds(&w, reader, document, P::DocManage).await);
}
