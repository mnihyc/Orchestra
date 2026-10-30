//! What an agent may manage: a document by `doc.manage` in a share, on the document or on the place it lies beside; never a conversation or a chat; held with the levels below it; seen for a document that was deleted, which is how an agent is asked whether it may bring it back; and the places a document is shared with.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_authz::documents::{self, Document};
use orch_authz::reach::{Agent, AgentReach, Reach, Standing, Things, Via};
use orch_db::queries::access::{PageKey, testing::*};
use orch_db::queries::documents as rows;
use orch_types::{Id, new_id};
use support::{World, stored};

fn agents(place: Id) -> Holder {
    Holder::Agents { id: place }
}

async fn reach_of(w: &World, thread: Id, things: &Things) -> Reach {
    let mut conn = w.conn().await;
    let agent = Agent::of(&mut conn, thread).await.unwrap().unwrap();
    Reach::load(&mut conn, agent, AgentReach::None, w.now, things)
        .await
        .unwrap()
}

/// A document beside `node`, made by the boss.
async fn document_beside(w: &World, node: Id, title: &str) -> Document {
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
    Document::of_row(&rows::get(&mut conn, id).await.unwrap().unwrap())
}

async fn delete(w: &World, document: &Document) -> Document {
    let mut conn = w.conn().await;
    assert!(
        rows::soft_delete_by_agent(&mut conn, document.id, w.now.get())
            .await
            .unwrap()
    );
    Document::of_row(&rows::get(&mut conn, document.id).await.unwrap().unwrap())
}

/// Names the agents of `place` in a row of the page of `thing` (a node or a document).
async fn share(w: &World, kind: &str, thing: Id, permission: Permission, place: Id) -> Id {
    let mut conn = w.conn().await;
    add_entry(
        &mut conn,
        &PageKey::new(kind, thing, "live"),
        "any",
        permission.as_str(),
        stored(&agents(place)),
        None,
        w.now.get(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn manage_on_a_place_manages_the_documents_beside_it_that_follow_its_row_and_edit_does_not() {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let plan = document_beside(&w, elsewhere, "Plan").await;
    let things = Things {
        nodes: vec![elsewhere],
        documents: vec![plan.clone()],
        people_chats: Vec::new(),
    };
    share(&w, "node", elsewhere, Permission::DocEdit, w.thread).await;
    let reach = reach_of(&w, w.thread, &things).await;
    assert_eq!(reach.document(&plan).write, Some(Via::Share));
    assert!(
        !reach.document(&plan).manages(),
        "to edit a document is not to delete it, move it or bring it back"
    );
    assert!(!reach.documents_of(elsewhere).manages());
    share(&w, "node", elsewhere, Permission::DocManage, w.thread).await;
    let reach = reach_of(&w, w.thread, &things).await;
    assert_eq!(reach.document(&plan).manage, Some(Via::Share));
    assert!(reach.documents_of(elsewhere).manages());
}

#[tokio::test]
async fn manage_holds_the_levels_below_it_as_a_person_who_manages_a_document_edits_and_reads_it() {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let plan = document_beside(&w, elsewhere, "Plan").await;
    share(&w, "document", plan.id, Permission::DocManage, w.thread).await;
    let reach = reach_of(
        &w,
        w.thread,
        &Things {
            nodes: vec![elsewhere],
            documents: vec![plan.clone()],
            people_chats: Vec::new(),
        },
    )
    .await;
    let held = reach.document(&plan);
    assert_eq!(
        (held.read, held.write, held.manage),
        (Some(Via::Share), Some(Via::Share), Some(Via::Share))
    );
    // The row was on the document: the place beside it is none of the agent's.
    assert_eq!(reach.documents_of(elsewhere), Standing::default());
}

#[tokio::test]
async fn manage_on_one_document_manages_that_one_and_no_other_beside_it() {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let mine = document_beside(&w, elsewhere, "Mine").await;
    let other = document_beside(&w, elsewhere, "Other").await;
    share(&w, "document", mine.id, Permission::DocManage, w.thread).await;
    let reach = reach_of(
        &w,
        w.thread,
        &Things {
            nodes: vec![elsewhere],
            documents: vec![mine.clone(), other.clone()],
            people_chats: Vec::new(),
        },
    )
    .await;
    assert!(reach.document(&mine).manages());
    assert_eq!(reach.document(&other), Standing::default());
}

#[tokio::test]
async fn a_conversation_and_a_chat_are_never_managed_whatever_the_agents_were_named_in() {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let thread = w.node("thread", elsewhere, "Write").await;
    for permission in [
        Permission::ThreadView,
        Permission::ThreadInteract,
        Permission::ChatView,
        Permission::ChatInteract,
        Permission::DocManage,
    ] {
        share(&w, "node", thread, permission, w.thread).await;
        share(&w, "node", elsewhere, permission, w.thread).await;
    }
    let reach = reach_of(
        &w,
        w.thread,
        &Things {
            nodes: vec![elsewhere, thread],
            ..Things::default()
        },
    )
    .await;
    assert!(!reach.conversation(thread, false).manages());
    assert!(!reach.node_chat(elsewhere).manages());
    assert!(!reach.node_chat(thread).manages());
    assert!(
        reach.documents_of(thread).manages(),
        "only the documents beside it are, which are an area of their own"
    );
}

#[tokio::test]
async fn a_document_that_was_deleted_is_managed_as_it_was_so_that_it_can_be_brought_back() {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let plan = document_beside(&w, elsewhere, "Plan").await;
    share(&w, "node", elsewhere, Permission::DocManage, w.thread).await;
    let gone = delete(&w, &plan).await;
    assert!(gone.deleted);
    let reach = reach_of(
        &w,
        w.thread,
        &Things {
            nodes: vec![elsewhere],
            documents: vec![gone.clone()],
            people_chats: Vec::new(),
        },
    )
    .await;
    assert!(reach.document(&gone).manages());
    // One that nobody gave the agent a right to is not its to bring back.
    let stranger = w.node("topic", w.project, "Private").await;
    let hidden = delete(&w, &document_beside(&w, stranger, "Secret").await).await;
    let reach = reach_of(
        &w,
        w.thread,
        &Things {
            nodes: vec![stranger],
            documents: vec![hidden.clone()],
            people_chats: Vec::new(),
        },
    )
    .await;
    assert_eq!(reach.document(&hidden), Standing::default());
}

#[tokio::test]
async fn the_places_a_document_is_shared_with_are_those_named_on_its_page_its_node_and_the_places_above()
 {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let plan = document_beside(&w, elsewhere, "Plan").await;
    let ops = w.node("topic", w.project, "Ops").await;
    let other = w.node("thread", ops, "Other").await;
    let things = Things {
        nodes: vec![elsewhere],
        documents: vec![plan.clone()],
        people_chats: Vec::new(),
    };
    // The agent's own share, and the shares of three other places: on the document, on its node, and on the
    // defaults of the project above it.
    share(&w, "node", elsewhere, Permission::DocManage, w.thread).await;
    share(&w, "document", plan.id, Permission::DocView, other).await;
    share(&w, "node", elsewhere, Permission::DocEdit, ops).await;
    w.name_default(
        Some(w.project),
        Permission::DocView,
        "any",
        agents(w.project),
    )
    .await;
    let reach = reach_of(&w, w.thread, &things).await;
    let places = reach.shared_with(&plan);
    assert_eq!(
        places,
        [w.thread, other, ops, w.project]
            .into_iter()
            .collect::<std::collections::BTreeSet<Id>>(),
        "every place whose agents reach it, the agent's own among them"
    );
}
