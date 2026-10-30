//! What an agent may read, write and send: empty until a policy or a person fills it, by area and apart, inherited on both sides, left out where a node or a document was made private, and gone the moment an entry is removed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_authz::documents::{self, Document};
use orch_authz::reach::{self, Agent, AgentReach, Reach, Standing, Things, Via};
use orch_db::queries::access::{PageKey, remove_entry, testing::*};
use orch_db::queries::documents as rows;
use orch_types::{Id, new_id};
use support::{World, stored};

/// The agent of a thread, and its reach to the things named.
async fn reach_of(w: &World, thread: Id, policy: AgentReach, things: &Things) -> Reach {
    let mut conn = w.conn().await;
    let agent = Agent::of(&mut conn, thread).await.unwrap().unwrap();
    Reach::load(&mut conn, agent, policy, w.now, things)
        .await
        .unwrap()
}

fn agents(place: Id) -> Holder {
    Holder::Agents { id: place }
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
    let row = rows::get(&mut conn, id).await.unwrap().unwrap();
    Document::of_row(&row)
}

/// Names the agents of `place` in a row of the page of `thing` (a node, a document or a chat).
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

/// Names the agents of `place` in the defaults row of `holder_place`, in both profiles of a row that has two.
async fn share_inside(w: &World, holder_place: Id, permission: Permission, place: Id) {
    let profiles: &[&str] = if matches!(
        permission,
        Permission::ThreadView | Permission::ThreadInteract
    ) {
        &["managed", "connected"]
    } else {
        &["any"]
    };
    for profile in profiles {
        w.name_default(Some(holder_place), permission, profile, agents(place))
            .await;
    }
}

fn nothing() -> Standing {
    Standing::default()
}

#[tokio::test]
async fn an_agents_reach_is_empty_until_a_policy_or_a_person_fills_it() {
    let w = World::new().await;
    let other = w.node("thread", w.topic, "Repro").await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let there = w.node("thread", elsewhere, "Write").await;
    let doc = document_beside(&w, w.topic, "Plan").await;
    let reach = reach_of(
        &w,
        w.thread,
        AgentReach::None,
        &Things {
            nodes: vec![other, there, w.topic, elsewhere],
            documents: vec![doc.clone()],
            people_chats: Vec::new(),
        },
    )
    .await;
    assert_eq!(reach.conversation(other, false).read, None);
    assert_eq!(reach.conversation(there, false), nothing());
    assert_eq!(reach.node_chat(w.topic), nothing());
    assert_eq!(reach.node_chat(elsewhere), nothing());
    assert_eq!(reach.document(&doc), nothing());
    assert_eq!(reach.documents_of(w.topic), nothing());
}

#[tokio::test]
async fn an_agent_reads_its_own_conversation_and_may_always_send_to_the_other_threads_of_its_topic()
{
    let w = World::new().await;
    let sibling = w.node("thread", w.topic, "Repro").await;
    let reach = reach_of(
        &w,
        w.thread,
        AgentReach::None,
        &Things {
            nodes: vec![sibling],
            ..Things::default()
        },
    )
    .await;
    let own = reach.conversation(w.thread, false);
    assert_eq!(own.read, Some(Via::Topic));
    assert_eq!(own.write, None, "it sends to others, not to itself");
    let beside = reach.conversation(sibling, false);
    assert_eq!(beside.write, Some(Via::Topic));
    assert_eq!(beside.read, None, "it may send there and not read there");
}

#[tokio::test]
async fn the_policy_reaches_the_conversations_of_a_topic_or_of_a_project_and_nothing_else() {
    let w = World::new().await;
    let sibling = w.node("thread", w.topic, "Repro").await;
    let other_topic = w.node("topic", w.project, "Docs").await;
    let far = w.node("thread", other_topic, "Write").await;
    let other_project = w.node("project", w.root, "Mobile").await;
    let outside_topic = w.node("topic", other_project, "App").await;
    let outside = w.node("thread", outside_topic, "Build").await;
    let things = Things {
        nodes: vec![sibling, far, outside, w.topic],
        ..Things::default()
    };
    let topic = reach_of(&w, w.thread, AgentReach::Topic, &things).await;
    assert_eq!(topic.conversation(sibling, false).read, Some(Via::Policy));
    assert_eq!(topic.conversation(far, false).read, None);
    let project = reach_of(&w, w.thread, AgentReach::Project, &things).await;
    assert_eq!(project.conversation(sibling, false).read, Some(Via::Policy));
    assert_eq!(project.conversation(far, false).read, Some(Via::Policy));
    assert_eq!(project.conversation(outside, false).read, None);
    assert_eq!(
        project.conversation(far, false).write,
        None,
        "a policy reads and never writes"
    );
    assert_eq!(
        project.node_chat(w.topic),
        nothing(),
        "never a chat, never a document"
    );
}

#[tokio::test]
async fn a_restricted_thread_is_left_out_of_the_policy_unless_it_was_shared_itself() {
    let w = World::new().await;
    let private = w.node("thread", w.topic, "Secret").await;
    w.row(private, Permission::ThreadView, false).await;
    let things = Things {
        nodes: vec![private],
        ..Things::default()
    };
    let reach = reach_of(&w, w.thread, AgentReach::Topic, &things).await;
    assert_eq!(reach.conversation(private, false).read, None);
    share(&w, "node", private, Permission::ThreadView, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::Topic, &things).await;
    assert_eq!(reach.conversation(private, false).read, Some(Via::Share));
}

#[tokio::test]
async fn an_administrators_session_reach_never_becomes_their_agents_reach() {
    let w = World::new().await;
    let agent = w
        .node_owned_by("thread", w.topic, "The admin's agent", w.boss)
        .await;
    let private = w
        .node("thread", w.topic, "Somebody else's private work")
        .await;
    w.row(private, Permission::ThreadView, false).await;
    assert!(
        w.can_as(
            orch_authz::Caller::session(w.boss).with_admin_mode(true),
            private,
            Permission::ThreadView
        )
        .await
    );
    let things = Things {
        nodes: vec![private],
        ..Things::default()
    };
    let origin = orch_db::audit::Origin {
        as_admin: Some(w.boss),
        ..Default::default()
    };
    let before = orch_db::audit::scope_origin(
        origin.clone(),
        reach_of(&w, agent, AgentReach::Topic, &things),
    )
    .await;
    assert_eq!(
        before.conversation(private, false).read,
        None,
        "the person's mode is not an agent share"
    );
    share(&w, "node", private, Permission::ThreadView, agent).await;
    let after =
        orch_db::audit::scope_origin(origin, reach_of(&w, agent, AgentReach::Topic, &things)).await;
    assert_eq!(
        after.conversation(private, false).read,
        Some(Via::Share),
        "an explicit share still reaches the agent"
    );
}

#[tokio::test]
async fn a_place_shared_with_agents_reaches_what_is_inside_it_what_is_made_later_and_not_what_was_cut_loose()
 {
    let w = World::new().await;
    // The agents of the Docs topic are given the conversations of the whole project Web.
    let docs = w.node("topic", w.project, "Docs").await;
    let agent = w.node("thread", docs, "Write").await;
    share_inside(&w, w.project, Permission::ThreadView, docs).await;
    let early = w.node("thread", w.topic, "Repro").await;
    let private = w.node("thread", w.topic, "Secret").await;
    w.row(private, Permission::ThreadView, false).await;
    let later = w.node("thread", w.topic, "Later").await;
    let things = Things {
        nodes: vec![early, later, private, w.thread],
        ..Things::default()
    };
    let reach = reach_of(&w, agent, AgentReach::None, &things).await;
    assert_eq!(reach.conversation(early, false).read, Some(Via::Share));
    assert_eq!(
        reach.conversation(later, false).read,
        Some(Via::Share),
        "what is made later follows the defaults that carry the share"
    );
    assert_eq!(
        reach.conversation(private, false).read,
        None,
        "what was cut loose from them is not covered"
    );
    // An agent of another topic, whose way does not pass through Docs, has nothing.
    let stranger = reach_of(&w, w.thread, AgentReach::None, &things).await;
    assert_eq!(stranger.conversation(early, false).read, None);
}

#[tokio::test]
async fn a_share_for_reading_does_not_let_the_agent_write_and_a_share_for_writing_lets_it_read() {
    let w = World::new().await;
    let other = w.node("thread", w.topic, "Repro").await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let things = Things {
        nodes: vec![elsewhere, other],
        ..Things::default()
    };
    share(&w, "node", elsewhere, Permission::ChatView, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things).await;
    let chat = reach.node_chat(elsewhere);
    assert_eq!((chat.read, chat.write), (Some(Via::Share), None));
    share(&w, "node", elsewhere, Permission::ChatInteract, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things).await;
    let chat = reach.node_chat(elsewhere);
    assert_eq!(
        (chat.read, chat.write),
        (Some(Via::Share), Some(Via::Share))
    );
    // Only a row of the write level: whoever may write an area may read it.
    let only_write = w.node("topic", w.project, "Ops").await;
    share(&w, "node", only_write, Permission::ChatInteract, w.thread).await;
    let reach = reach_of(
        &w,
        w.thread,
        AgentReach::None,
        &Things {
            nodes: vec![only_write],
            ..Things::default()
        },
    )
    .await;
    assert!(reach.node_chat(only_write).reads());
    // A conversation shared for reading is read, and sent to only when it is shared for that.
    let target = w.node("thread", elsewhere, "Write").await;
    share(&w, "node", target, Permission::ThreadView, w.thread).await;
    let reach = reach_of(
        &w,
        w.thread,
        AgentReach::None,
        &Things {
            nodes: vec![target],
            ..Things::default()
        },
    )
    .await;
    let conversation = reach.conversation(target, false);
    assert_eq!(
        (conversation.read, conversation.write),
        (Some(Via::Share), None)
    );
}

#[tokio::test]
async fn edit_on_a_place_makes_documents_there_and_changes_those_that_follow_its_row_and_edit_on_one_makes_no_other()
 {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let following = document_beside(&w, elsewhere, "Plan").await;
    let alone = document_beside(&w, elsewhere, "Other").await;
    let things = Things {
        nodes: vec![elsewhere],
        documents: vec![following.clone(), alone.clone()],
        people_chats: Vec::new(),
    };
    // One document shared for editing: that document only.
    share(&w, "document", alone.id, Permission::DocEdit, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things).await;
    assert_eq!(reach.document(&alone).write, Some(Via::Share));
    assert_eq!(reach.document(&following), nothing());
    assert_eq!(
        reach.documents_of(elsewhere).write,
        None,
        "a document shared for editing gives no right to make another"
    );
    // The place shared for editing: it makes documents there, and edits those that still follow its row.
    share(&w, "node", elsewhere, Permission::DocEdit, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things).await;
    assert_eq!(reach.documents_of(elsewhere).write, Some(Via::Share));
    assert_eq!(reach.document(&following).write, Some(Via::Share));
    // A document cut loose from its node's row is private: left out unless it was shared itself.
    let mut conn = w.conn().await;
    for permission in ["doc.view", "doc.edit"] {
        set_row(
            &mut conn,
            &PageKey::new("document", following.id, "live"),
            "any",
            permission,
            false,
            false,
        )
        .await
        .unwrap();
    }
    drop(conn);
    let reach = reach_of(&w, w.thread, AgentReach::None, &things).await;
    assert_eq!(reach.document(&following), nothing());
    assert_eq!(reach.document(&alone).write, Some(Via::Share));
    share(&w, "document", following.id, Permission::DocView, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things).await;
    let shared = reach.document(&following);
    assert_eq!((shared.read, shared.write), (Some(Via::Share), None));
}

#[tokio::test]
async fn a_share_that_was_removed_or_has_ended_is_gone_at_the_next_look() {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let things = Things {
        nodes: vec![elsewhere],
        ..Things::default()
    };
    let entry = share(&w, "node", elsewhere, Permission::ChatView, w.thread).await;
    assert!(
        reach_of(&w, w.thread, AgentReach::None, &things)
            .await
            .node_chat(elsewhere)
            .reads()
    );
    let mut conn = w.conn().await;
    remove_entry(&mut conn, entry, w.boss, w.now.get())
        .await
        .unwrap();
    drop(conn);
    assert!(
        !reach_of(&w, w.thread, AgentReach::None, &things)
            .await
            .node_chat(elsewhere)
            .reads(),
        "a removed entry counts for nothing"
    );
    // An entry that ends by itself.
    let mut conn = w.conn().await;
    add_entry(
        &mut conn,
        &PageKey::new("node", elsewhere, "live"),
        "any",
        "chat.view",
        stored(&agents(w.thread)),
        Some(w.now.get() + Duration::hours(1)),
        w.now.get(),
    )
    .await
    .unwrap();
    drop(conn);
    assert!(
        reach_of(&w, w.thread, AgentReach::None, &things)
            .await
            .node_chat(elsewhere)
            .reads()
    );
    let mut conn = w.conn().await;
    let agent = Agent::of(&mut conn, w.thread).await.unwrap().unwrap();
    let later = orch_types::Time::new(w.now.get() + Duration::hours(2));
    let reach = Reach::load(&mut conn, agent, AgentReach::None, later, &things)
        .await
        .unwrap();
    assert!(
        !reach.node_chat(elsewhere).reads(),
        "after its end it counts for nothing"
    );
}

#[tokio::test]
async fn the_agents_of_a_place_that_is_not_on_its_way_give_it_nothing() {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let neighbour = w.node("thread", elsewhere, "Write").await;
    // Shared with the agents of one thread, not of this one; and with a thread's own agents only.
    share(&w, "node", elsewhere, Permission::ChatView, neighbour).await;
    let reach = reach_of(
        &w,
        w.thread,
        AgentReach::None,
        &Things {
            nodes: vec![elsewhere],
            ..Things::default()
        },
    )
    .await;
    assert_eq!(reach.node_chat(elsewhere), nothing());
    // The agents of the topic it works in are on its way, and so are those of its project and its org.
    for place in [w.topic, w.project, w.root] {
        share(&w, "node", elsewhere, Permission::ChatView, place).await;
        let reach = reach_of(
            &w,
            w.thread,
            AgentReach::None,
            &Things {
                nodes: vec![elsewhere],
                ..Things::default()
            },
        )
        .await;
        assert!(reach.node_chat(elsewhere).reads(), "{place}");
        let mut conn = w.conn().await;
        sqlx::query("UPDATE permission_entries SET removed_at = now() WHERE holder_id = $1")
            .bind(place)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_direct_chat_is_reached_only_by_the_entries_on_its_own_page() {
    let w = World::new().await;
    let chat = new_id();
    {
        let mut conn = w.conn().await;
        sqlx::query("INSERT INTO chats (id, kind, org_id, direct_key, created_by) VALUES ($1, 'direct', $2, $3, $4)")
            .bind(chat)
            .bind(w.root)
            .bind(format!("{}:{}", w.boss, w.maker))
            .bind(w.boss)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let things = Things {
        people_chats: vec![chat],
        ..Things::default()
    };
    assert_eq!(
        reach_of(&w, w.thread, AgentReach::Project, &things)
            .await
            .people_chat(chat),
        nothing()
    );
    share(&w, "chat", chat, Permission::ChatView, w.topic).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things).await;
    let standing = reach.people_chat(chat);
    assert_eq!((standing.read, standing.write), (Some(Via::Share), None));
    share(&w, "chat", chat, Permission::ChatInteract, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things).await;
    assert_eq!(reach.people_chat(chat).write, Some(Via::Share));
}

#[tokio::test]
async fn the_entries_given_to_the_agents_on_a_way_are_listed_with_who_wrote_them() {
    let w = World::new().await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    share(&w, "node", elsewhere, Permission::ChatView, w.project).await;
    share_inside(&w, elsewhere, Permission::DocView, w.thread).await;
    let neighbour = w.node("thread", elsewhere, "Write").await;
    share(&w, "node", elsewhere, Permission::ChatView, neighbour).await;
    let mut conn = w.conn().await;
    let agent = Agent::of(&mut conn, w.thread).await.unwrap().unwrap();
    let grants = reach::grants(&mut conn, &agent, w.now).await.unwrap();
    assert_eq!(
        grants.len(),
        2,
        "the entry for another thread's agents is not among them"
    );
    assert!(grants.iter().any(|grant| grant.holder == w.project
        && grant.permission == Permission::ChatView
        && !reach::is_defaults(grant)));
    assert!(
        grants
            .iter()
            .any(|grant| grant.holder == w.thread && reach::is_defaults(grant))
    );
}

#[tokio::test]
async fn nothing_is_the_agent_of_a_thread_that_is_gone() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    assert!(Agent::of(&mut conn, new_id()).await.unwrap().is_none());
    assert!(
        Agent::of(&mut conn, w.topic).await.unwrap().is_none(),
        "a topic has no agent"
    );
    let agent = Agent::of(&mut conn, w.thread).await.unwrap().unwrap();
    assert_eq!(agent.topic, w.topic);
    assert_eq!(agent.project, Some(w.project));
    assert_eq!(agent.root, w.root);
    assert_eq!(agent.path.last(), Some(&w.thread));
}

/// A document beside `node` that the agent of `thread` made.
async fn document_made_by(w: &World, node: Id, thread: Id, title: &str) -> Document {
    let mut conn = w.conn().await;
    let id = new_id();
    rows::insert(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: node,
            org_id: w.root,
            owner_id: w.boss,
            created_by_thread: Some(thread),
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
    let row = rows::get(&mut conn, id).await.unwrap().unwrap();
    Document::of_row(&row)
}

#[tokio::test]
async fn an_agent_copies_into_its_workspace_what_is_its_own_and_what_the_export_rows_name_it_for() {
    let w = World::new().await;
    let other = w.node("thread", w.topic, "Repro").await;
    let doc = document_beside(&w, w.topic, "Plan").await;
    let mine = document_made_by(&w, w.topic, w.thread, "Notes").await;
    let things = || Things {
        nodes: vec![other, w.topic],
        documents: vec![doc.clone(), mine.clone()],
        people_chats: Vec::new(),
    };

    // Shared to be read: read, and not copied. What it is, or made itself, is its own.
    share(&w, "node", other, Permission::ThreadView, w.thread).await;
    share(&w, "node", w.topic, Permission::ChatView, w.thread).await;
    share(&w, "document", doc.id, Permission::DocView, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things()).await;
    assert!(reach.conversation(other, false).read.is_some());
    assert!(!reach.may_download_conversation(other, false));
    assert!(!reach.may_download_node_chat(w.topic));
    assert!(!reach.may_download_document(&doc));
    assert!(
        reach.may_download_conversation(w.thread, false),
        "its own conversation"
    );
    assert!(reach.may_download_document(&mine), "what its thread made");

    // Named in the Export rows: copied, each kind by the row of its own kind.
    share(&w, "node", other, Permission::ThreadExport, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things()).await;
    assert!(reach.may_download_conversation(other, false));
    assert!(!reach.may_download_node_chat(w.topic) && !reach.may_download_document(&doc));
    assert!(
        !reach.may_download_conversation(other, true),
        "a side thread is never copied"
    );
    share(&w, "node", w.topic, Permission::ChatExport, w.thread).await;
    share(&w, "document", doc.id, Permission::DocExport, w.thread).await;
    let reach = reach_of(&w, w.thread, AgentReach::None, &things()).await;
    assert!(reach.may_download_node_chat(w.topic));
    assert!(reach.may_download_document(&doc));

    // The Export row of another thread's agents is no row of this one.
    let stranger = w.node("thread", w.topic, "Stranger").await;
    let again = reach_of(&w, stranger, AgentReach::None, &things()).await;
    assert!(!again.may_download_conversation(other, false));
    assert!(!again.may_download_document(&doc));
}
