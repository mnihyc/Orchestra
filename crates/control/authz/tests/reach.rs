//! What an agent may read, write and send: empty until a person shares it, by area and apart, inherited on both sides, left out where a node or a document was made private, and gone the moment an entry is removed.
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
use serde_json::json;
use support::World;

/// The agent of a thread, and its reach to the things named.
async fn reach_of(w: &World, thread: Id, things: &Things) -> Reach {
    let mut conn = w.conn().await;
    let agent = Agent::of(&mut conn, thread).await.unwrap().unwrap();
    Reach::load_current(&mut conn, agent, w.now, things)
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
    let row = rows::get(&mut conn, id).await.unwrap().unwrap();
    Document::of_row(&row)
}

/// Names the agents of `place` in a row of the page of `thing` (a node, a document or a chat).
async fn share(w: &World, kind: &str, thing: Id, permission: Permission, place: Id) -> Id {
    w.agent_entry(
        &PageKey::new(kind, thing, "live"),
        "any",
        permission,
        place,
        None,
    )
    .await
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
        w.agent_entry(
            &PageKey::new("node", holder_place, "defaults"),
            profile,
            permission,
            place,
            None,
        )
        .await;
    }
}

fn nothing() -> Standing {
    Standing::default()
}

#[tokio::test]
async fn an_agents_reach_is_empty_until_a_person_gives_it_entries() {
    let w = World::new().await;
    let other = w.node("thread", w.topic, "Repro").await;
    let elsewhere = w.node("topic", w.project, "Docs").await;
    let there = w.node("thread", elsewhere, "Write").await;
    let doc = document_beside(&w, w.topic, "Plan").await;
    let reach = reach_of(
        &w,
        w.thread,
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
async fn an_agent_reads_its_own_conversation_and_only_explicit_entries_control_siblings() {
    let w = World::new().await;
    let sibling = w.node("thread", w.topic, "Repro").await;
    let reach = reach_of(
        &w,
        w.thread,
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
    assert_eq!(
        beside.write, None,
        "automatic topic messages are off by default"
    );
    assert_eq!(beside.read, None);
    let legacy = reach.with_topic_messages(true);
    assert_eq!(
        legacy.conversation(sibling, false),
        nothing(),
        "a retired opt-in adds no authority"
    );
    share(&w, "node", sibling, Permission::ThreadInteract, w.thread).await;
    let reach = reach_of(
        &w,
        w.thread,
        &Things {
            nodes: vec![sibling],
            ..Things::default()
        },
    )
    .await;
    let beside = reach.conversation(sibling, false);
    assert_eq!(beside.write, Some(Via::Share));
    assert_eq!(
        beside.read,
        Some(Via::Share),
        "Interact retains its normal View implication"
    );
    assert_eq!(
        reach.conversation(sibling, true),
        nothing(),
        "a side thread stays outside message reach"
    );
}

#[tokio::test]
async fn an_explicit_topic_share_allows_messages_with_the_normal_read_implication() {
    let w = World::new().await;
    share_inside(&w, w.topic, Permission::ThreadInteract, w.topic).await;
    let sibling = w.node("thread", w.topic, "Repro").await;
    let reach = reach_of(
        &w,
        w.thread,
        &Things {
            nodes: vec![sibling],
            ..Things::default()
        },
    )
    .await;
    let standing = reach.conversation(sibling, false);
    assert_eq!(standing.write, Some(Via::Share));
    assert_eq!(
        standing.read,
        Some(Via::Share),
        "an explicit Interact grant retains its existing View implication"
    );
}

#[tokio::test]
async fn explicit_topic_and_project_shares_reach_only_their_own_conversations() {
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
    share_inside(&w, w.topic, Permission::ThreadView, w.thread).await;
    let topic = reach_of(&w, w.thread, &things).await;
    assert_eq!(topic.conversation(sibling, false).read, Some(Via::Share));
    assert_eq!(topic.conversation(far, false).read, None);
    share_inside(&w, w.project, Permission::ThreadView, w.thread).await;
    let project = reach_of(&w, w.thread, &things).await;
    assert_eq!(project.conversation(sibling, false).read, Some(Via::Share));
    assert_eq!(project.conversation(far, false).read, Some(Via::Share));
    assert_eq!(project.conversation(outside, false).read, None);
    assert_eq!(
        project.conversation(far, false).write,
        None,
        "a read-only share does not give sending"
    );
    assert_eq!(
        project.node_chat(w.topic),
        nothing(),
        "never a chat, never a document"
    );
}

#[tokio::test]
async fn a_restricted_thread_is_left_out_of_an_inherited_share_unless_shared_itself() {
    let w = World::new().await;
    let private = w.node("thread", w.topic, "Secret").await;
    w.row(private, Permission::ThreadView, false).await;
    let things = Things {
        nodes: vec![private],
        ..Things::default()
    };
    share_inside(&w, w.topic, Permission::ThreadView, w.thread).await;
    let reach = reach_of(&w, w.thread, &things).await;
    assert_eq!(reach.conversation(private, false).read, None);
    share(&w, "node", private, Permission::ThreadView, w.thread).await;
    let reach = reach_of(&w, w.thread, &things).await;
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
    let before = orch_db::audit::scope_origin(origin.clone(), reach_of(&w, agent, &things)).await;
    assert_eq!(
        before.conversation(private, false).read,
        None,
        "the person's mode is not an agent share"
    );
    share(&w, "node", private, Permission::ThreadView, agent).await;
    let after = orch_db::audit::scope_origin(origin, reach_of(&w, agent, &things)).await;
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
    let reach = reach_of(&w, agent, &things).await;
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
    let stranger = reach_of(&w, w.thread, &things).await;
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
    let reach = reach_of(&w, w.thread, &things).await;
    let chat = reach.node_chat(elsewhere);
    assert_eq!((chat.read, chat.write), (Some(Via::Share), None));
    share(&w, "node", elsewhere, Permission::ChatInteract, w.thread).await;
    let reach = reach_of(&w, w.thread, &things).await;
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
    let reach = reach_of(&w, w.thread, &things).await;
    assert_eq!(reach.document(&alone).write, Some(Via::Share));
    assert_eq!(reach.document(&following), nothing());
    assert_eq!(
        reach.documents_of(elsewhere).write,
        None,
        "a document shared for editing gives no right to make another"
    );
    // The place shared for editing: it makes documents there, and edits those that still follow its row.
    share(&w, "node", elsewhere, Permission::DocEdit, w.thread).await;
    let reach = reach_of(&w, w.thread, &things).await;
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
    let reach = reach_of(&w, w.thread, &things).await;
    assert_eq!(reach.document(&following), nothing());
    assert_eq!(reach.document(&alone).write, Some(Via::Share));
    share(&w, "document", following.id, Permission::DocView, w.thread).await;
    let reach = reach_of(&w, w.thread, &things).await;
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
        reach_of(&w, w.thread, &things)
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
        !reach_of(&w, w.thread, &things)
            .await
            .node_chat(elsewhere)
            .reads(),
        "a removed entry counts for nothing"
    );
    // An entry that ends by itself.
    let mut conn = w.conn().await;
    w.agent_entry(
        &PageKey::new("node", elsewhere, "live"),
        "any",
        Permission::ChatView,
        w.thread,
        Some(orch_types::Time::new(w.now.get() + Duration::hours(1))),
    )
    .await;
    drop(conn);
    assert!(
        reach_of(&w, w.thread, &things)
            .await
            .node_chat(elsewhere)
            .reads()
    );
    let mut conn = w.conn().await;
    let agent = Agent::of(&mut conn, w.thread).await.unwrap().unwrap();
    let later = orch_types::Time::new(w.now.get() + Duration::hours(2));
    let reach = Reach::load_current(&mut conn, agent, later, &things)
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
        for person in [w.boss, w.maker] {
            orch_db::queries::chats::add_member(
                &mut conn,
                &orch_db::queries::chats::NewMember {
                    chat_id: chat,
                    user_id: person,
                    added_by: Some(w.boss),
                    guest: false,
                    seq: 0,
                    at: w.now.get(),
                    expires_at: None,
                },
            )
            .await
            .unwrap();
        }
    }
    let things = Things {
        people_chats: vec![chat],
        ..Things::default()
    };
    assert_eq!(
        reach_of(&w, w.thread, &things).await.people_chat(chat),
        nothing()
    );
    share(&w, "chat", chat, Permission::ChatView, w.topic).await;
    let reach = reach_of(&w, w.thread, &things).await;
    let standing = reach.people_chat(chat);
    assert_eq!((standing.read, standing.write), (Some(Via::Share), None));
    share(&w, "chat", chat, Permission::ChatInteract, w.thread).await;
    let reach = reach_of(&w, w.thread, &things).await;
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
    let reach = reach_of(&w, w.thread, &things()).await;
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
    let reach = reach_of(&w, w.thread, &things()).await;
    assert!(reach.may_download_conversation(other, false));
    assert!(!reach.may_download_node_chat(w.topic) && !reach.may_download_document(&doc));
    assert!(
        !reach.may_download_conversation(other, true),
        "a side thread is never copied"
    );
    share(&w, "node", w.topic, Permission::ChatExport, w.thread).await;
    share(&w, "document", doc.id, Permission::DocExport, w.thread).await;
    let reach = reach_of(&w, w.thread, &things()).await;
    assert!(reach.may_download_node_chat(w.topic));
    assert!(reach.may_download_document(&doc));

    // The Export row of another thread's agents is no row of this one.
    let stranger = w.node("thread", w.topic, "Stranger").await;
    let again = reach_of(&w, stranger, &things()).await;
    assert!(!again.may_download_conversation(other, false));
    assert!(!again.may_download_document(&doc));
}

#[tokio::test]
async fn an_external_agent_share_obeys_a_tightened_lifetime_like_an_outside_person() {
    let w = World::new().await;
    let target = w.node("thread", w.topic, "Shared target").await;
    let foreign_owner = w.user("foreignowner").await;
    let foreign_root = new_id();
    let mut conn = w.conn().await;
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        foreign_root,
        "org",
        None,
        "Other root",
        foreign_owner,
    )
    .await
    .unwrap();
    name_owner(
        &mut conn,
        foreign_root,
        ("user", foreign_owner),
        w.now.get(),
    )
    .await
    .unwrap();
    drop(conn);
    let foreign_project = w
        .node_owned_by("project", foreign_root, "Other work", foreign_owner)
        .await;
    let foreign_topic = w
        .node_owned_by("topic", foreign_project, "Other topic", foreign_owner)
        .await;
    let foreign = w
        .node_owned_by("thread", foreign_topic, "Foreign reader", foreign_owner)
        .await;
    let legal = w
        .node_owned_by(
            "thread",
            foreign_topic,
            "Current legal reader",
            foreign_owner,
        )
        .await;
    let things = Things {
        nodes: vec![target],
        ..Things::default()
    };
    let entry = share(&w, "node", target, Permission::ThreadView, foreign).await;
    share(&w, "node", target, Permission::ThreadView, w.thread).await;
    let guest = w.user("guest").await;
    w.name(target, Permission::ThreadView, Holder::User { id: guest })
        .await;
    let mut conn = w.conn().await;
    let agent = Agent::of(&mut conn, foreign).await.unwrap().unwrap();
    assert_eq!(agent.root, foreign_root);
    assert_ne!(
        agent.root, w.root,
        "the agent's place is outside the target's tree"
    );
    w.agent_entry(
        &PageKey::new("node", target, "live"),
        "any",
        Permission::ThreadView,
        legal,
        Some(orch_types::Time::new(w.later(Duration::days(20)).get())),
    )
    .await;
    let original: (Id, Option<chrono::DateTime<chrono::Utc>>, i64) =
        sqlx::query_as("SELECT id, expires_at, revision FROM permission_entries WHERE id=$1")
            .bind(entry)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    drop(conn);
    assert!(
        reach_of(&w, foreign, &things)
            .await
            .conversation(target, false)
            .reads()
    );
    assert!(w.can(guest, target, Permission::ThreadView).await);

    let mut conn = w.conn().await;
    sqlx::query(
        "INSERT INTO settings (node_id,key,value) VALUES ($1,'sharing.external_max_days','30')
         ON CONFLICT (node_id,key) DO UPDATE SET value=EXCLUDED.value",
    )
    .bind(w.root)
    .execute(&mut *conn)
    .await
    .unwrap();
    let unchanged: (Id, Option<chrono::DateTime<chrono::Utc>>, i64) =
        sqlx::query_as("SELECT id, expires_at, revision FROM permission_entries WHERE id=$1")
            .bind(entry)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    drop(conn);
    assert_eq!(
        unchanged, original,
        "tightening policy does not repair its entry"
    );
    assert!(!w.can(guest, target, Permission::ThreadView).await);
    assert!(
        reach_of(&w, w.thread, &things)
            .await
            .conversation(target, false)
            .reads(),
        "a same-root share does not become external"
    );
    assert!(
        reach_of(&w, legal, &things)
            .await
            .conversation(target, false)
            .reads(),
        "an outside entry already within the bound remains valid"
    );
    assert!(
        !reach_of(&w, foreign, &things)
            .await
            .conversation(target, false)
            .reads(),
        "the external agent's unbounded entry must be withheld too"
    );

    let end = w.later(Duration::days(5));
    w.change_as(
        orch_authz::Caller::session(w.maker),
        orch_authz::pages::Target::Live(target),
        json!({ "rows": [{ "permission": "thread.view",
            "update": [{ "entry_id": entry, "expires_at": end }] }] }),
        orch_authz::Policy {
            owners_name_outsiders: true,
            ..orch_authz::Policy::default()
        },
    )
    .await
    .unwrap();
    assert!(
        reach_of(&w, foreign, &things)
            .await
            .conversation(target, false)
            .reads()
    );
    let repaired: (Id, Option<chrono::DateTime<chrono::Utc>>, i64) =
        sqlx::query_as("SELECT id, expires_at, revision FROM permission_entries WHERE id=$1")
            .bind(entry)
            .fetch_one(&w.db.pool)
            .await
            .unwrap();
    assert_eq!(repaired.0, original.0);
    assert_eq!(repaired.1, Some(end.get()));
    assert!(repaired.2 > original.2);
}

#[tokio::test]
async fn outside_grants_are_filtered_before_an_independent_valid_grant_and_chat_permissions() {
    let mut w = World::new().await;
    let owner = w.member("bridgeowner").await;
    w.give(
        w.project,
        owner,
        orch_access_contract::roles::Role::Interactor,
    )
    .await;
    let root = new_id();
    let mut conn = w.conn().await;
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        root,
        "org",
        None,
        "Other root",
        owner,
    )
    .await
    .unwrap();
    name_owner(&mut conn, root, ("user", owner), w.now.get())
        .await
        .unwrap();
    insert_member(&mut conn, root, owner, root).await.unwrap();
    drop(conn);
    let project = w.node_owned_by("project", root, "Other work", owner).await;
    let topic = w
        .node_owned_by("topic", project, "Other topic", owner)
        .await;
    let source = w
        .node_owned_by("thread", topic, "Foreign agent", owner)
        .await;
    let blocked = w.node("thread", w.topic, "No legal grant").await;
    let mixed = w.node("thread", w.topic, "Independent legal grant").await;
    assert!(w.can(owner, mixed, Permission::ThreadView).await);
    let things = Things {
        nodes: vec![blocked, mixed],
        ..Things::default()
    };
    assert!(
        !reach_of(&w, source, &things)
            .await
            .conversation(mixed, false)
            .reads(),
        "the owner's local roles are not the agent's reach"
    );
    share(&w, "node", blocked, Permission::ThreadView, source).await;
    let invalid = share(&w, "node", mixed, Permission::ThreadView, source).await;
    share(&w, "node", blocked, Permission::ThreadView, w.thread).await;
    // Separate grant moments at the clock's millisecond precision make the order deterministic.
    w.now = w.later(Duration::milliseconds(1));
    let chat = new_id();
    let mut conn = w.conn().await;
    let valid = w
        .agent_entry(
            &PageKey::new("node", mixed, "live"),
            "any",
            Permission::ThreadView,
            topic,
            Some(orch_types::Time::new(w.later(Duration::days(5)).get())),
        )
        .await;
    let ordered: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM permission_entries WHERE id=ANY($1) ORDER BY created_at,id",
    )
    .bind(vec![invalid, valid])
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        ordered,
        vec![invalid, valid],
        "the invalid matching grant is encountered first"
    );
    sqlx::query(
        "INSERT INTO chats(id,kind,org_id,direct_key,created_by) VALUES($1,'direct',$2,$3,$4)",
    )
    .bind(chat)
    .bind(w.root)
    .bind(format!("{}:{}", w.boss, w.maker))
    .bind(w.boss)
    .execute(&mut *conn)
    .await
    .unwrap();
    for person in [w.boss, w.maker] {
        orch_db::queries::chats::add_member(
            &mut conn,
            &orch_db::queries::chats::NewMember {
                chat_id: chat,
                user_id: person,
                added_by: Some(w.boss),
                guest: false,
                seq: 0,
                at: w.now.get(),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    }

    drop(conn);
    share(&w, "chat", chat, Permission::ChatInteract, source).await;
    share(&w, "chat", chat, Permission::ChatView, w.thread).await;
    let mut conn = w.conn().await;
    w.agent_entry(
        &PageKey::new("chat", chat, "live"),
        "any",
        Permission::ChatView,
        topic,
        Some(orch_types::Time::new(w.later(Duration::days(5)).get())),
    )
    .await;
    sqlx::query(
        "INSERT INTO settings(node_id,key,value) VALUES($1,'sharing.external_max_days','7')",
    )
    .bind(w.root)
    .execute(&mut *conn)
    .await
    .unwrap();
    let agent = Agent::of(&mut conn, source).await.unwrap().unwrap();
    assert_eq!(agent.root, root);
    assert_ne!(
        agent.root, w.root,
        "the local owner does not change the agent's root"
    );
    drop(conn);
    let reach = reach_of(&w, source, &things).await;
    assert!(!reach.conversation(blocked, false).reads());
    let standing = reach.conversation(mixed, false);
    assert!(
        standing.reads(),
        "an independent later valid grant still supplies reading"
    );
    assert!(!standing.writes() && !standing.manages());
    assert!(!reach.may_download_conversation(mixed, false));
    assert!(
        reach_of(&w, w.thread, &things)
            .await
            .conversation(blocked, false)
            .reads()
    );
    let chats = Things {
        people_chats: vec![chat],
        ..Things::default()
    };
    let chat_standing = reach_of(&w, source, &chats).await.people_chat(chat);
    assert!(chat_standing.reads());
    assert!(
        !chat_standing.writes(),
        "a separate bounded View grant cannot revive invalid Interact"
    );
    assert!(!chat_standing.manages());
    assert!(
        reach_of(&w, w.thread, &chats)
            .await
            .people_chat(chat)
            .reads()
    );
}

#[tokio::test]
async fn retired_policy_values_do_not_supply_document_conversation_or_message_rights() {
    let w = World::new().await;
    let sibling = w.node("thread", w.topic, "No contribution").await;
    let doc = document_beside(&w, w.topic, "Not shared").await;
    for policy in [AgentReach::None, AgentReach::Topic, AgentReach::Project] {
        let mut conn = w.conn().await;
        let agent = Agent::of(&mut conn, w.thread).await.unwrap().unwrap();
        let reach = Reach::load(
            &mut conn,
            agent,
            policy,
            w.now,
            &Things {
                nodes: vec![w.thread, w.topic, sibling],
                documents: vec![doc.clone()],
                people_chats: Vec::new(),
            },
        )
        .await
        .unwrap()
        .with_document_policy(reach::DocumentPolicy::Write)
        .with_topic_messages(true);
        assert!(reach.conversation(w.thread, false).reads());
        assert_eq!(reach.conversation(sibling, false), nothing());
        assert_eq!(reach.document(&doc), nothing());
        assert_eq!(reach.documents_of(w.topic), nothing());
        assert_eq!(reach.node_chat(w.topic), nothing());
    }
}
