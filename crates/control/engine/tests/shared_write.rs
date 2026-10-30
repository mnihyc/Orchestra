//! What an agent writes into what was shared with it for writing: a message in a chat under its own name, a document made and changed under its name in the document's history, and a message to a thread; each only where a share allows it at the moment of the call, and never undoing what somebody wrote meanwhile.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

#[path = "shared/document_files.rs"]
mod document_files;

use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::{Value, json};
use shared::*;
use std::time::Duration;

struct Setup {
    world: common::World,
    agent: Id,
    topic: Id,
    chat: Id,
}

/// An agent, and the chat of another topic shared with it for `level` (`chat.view`, or also `chat.interact`).
async fn with_a_shared_chat(writing: bool) -> Setup {
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Writer").await;
    let topic = world.topic("Team").await;
    let chat = chat_of(&world, topic).await;
    share(&world, "node", topic, "live", "chat.view", "any", agent).await;
    if writing {
        share(&world, "node", topic, "live", "chat.interact", "any", agent).await;
    }
    Setup {
        world,
        agent,
        topic,
        chat,
    }
}

async fn chat_path(setup: &Setup) -> String {
    let listing = ok(call(
        &setup.world,
        setup.agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", setup.topic) }),
    )
    .await);
    entry(&listing, "chat.md")["path"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn send(setup: &Setup, arguments: Value) -> orch_link_contract::commands::OrchToolResult {
    call(&setup.world, setup.agent, OrchTool::SendMessage, arguments).await
}

// ---------------------------------------------------------------- send_message

#[tokio::test]
async fn a_message_is_written_in_a_chat_shared_for_writing_under_the_name_of_the_agent() {
    let s = with_a_shared_chat(true).await;
    let path = chat_path(&s).await;
    let listing = ok(call(
        &s.world,
        s.agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", s.topic) }),
    )
    .await);
    assert_eq!(entry(&listing, "chat.md")["writable"], true);
    let answered = ok(send(
        &s,
        json!({ "target": { "kind": "chat", "path": path }, "text": "The build is green." }),
    )
    .await);
    assert_eq!(answered["message"], 1);
    assert_eq!(answered["target"], json!({ "kind": "chat", "path": path }));
    assert_eq!(answered["status"], "posted");
    assert_eq!(answered["files"], json!([]));
    let (author, thread, agent): (Option<Id>, Option<Id>, Option<String>) = sqlx::query_as(
        "SELECT author_id, author_thread_id, author_agent FROM chat_messages WHERE chat_id = $1",
    )
    .bind(s.chat)
    .fetch_one(&mut *s.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    assert_eq!(
        (author, thread, agent.as_deref()),
        (None, Some(s.agent), Some("Fake agent"))
    );
    let read = ok(call(
        &s.world,
        s.agent,
        OrchTool::ReadShared,
        json!({ "path": path }),
    )
    .await);
    let text = read["text"].as_str().unwrap();
    assert!(
        text.contains("#1 · Fake agent (thread Writer) · "),
        "{text}"
    );
    assert!(text.contains("The build is green."));
    let audits: i64 = s.world.count("SELECT count(*) FROM audit_log WHERE action = 'chat.posted_by_agent' AND object_id = $1", s.chat).await;
    assert_eq!(audits, 1, "a message an agent wrote is audited");
}

#[tokio::test]
async fn a_chat_shared_for_reading_is_not_written_in_and_the_refusal_says_what_is_missing() {
    let s = with_a_shared_chat(false).await;
    let path = chat_path(&s).await;
    let refusal = send(
        &s,
        json!({ "target": { "kind": "chat", "path": path }, "text": "Hello" }),
    )
    .await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_writable");
    assert!(
        message.contains("chat.interact") && message.contains("sharing"),
        "{message}"
    );
    let rows: i64 = s
        .world
        .count(
            "SELECT count(*) FROM chat_messages WHERE chat_id = $1",
            s.chat,
        )
        .await;
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn a_chat_nobody_shared_answers_as_a_chat_that_is_not_there() {
    let s = with_a_shared_chat(true).await;
    let other = s.world.topic("Private").await;
    chat_of(&s.world, other).await;
    let hidden = send(
        &s,
        json!({ "target": { "kind": "chat", "path": format!("id:{}", chat_of(&s.world, other).await) }, "text": "Hi" }),
    )
    .await;
    let missing = send(
        &s,
        json!({ "target": { "kind": "chat", "path": format!("id:{}", orch_types::new_id()) }, "text": "Hi" }),
    )
    .await;
    assert_eq!(refused(&hidden).0, "not_found");
    assert_eq!(refused(&missing).0, "not_found");
    let wrong = send(
        &s,
        json!({ "target": { "kind": "chat", "path": "Writer/conversation.md" }, "text": "Hi" }),
    )
    .await;
    let (code, message) = refused(&wrong);
    assert_eq!(code, "wrong_kind");
    assert!(message.contains("send_message"));
}

#[tokio::test]
async fn a_reply_is_by_the_number_of_the_message_it_answers() {
    let s = with_a_shared_chat(true).await;
    say_in_chat(
        &s.world,
        s.chat,
        s.world.ada,
        "Can someone look at staging?",
    )
    .await;
    let path = chat_path(&s).await;
    let answered = ok(send(
        &s,
        json!({ "target": { "kind": "chat", "path": path }, "text": "On it.", "reply_to": 1 }),
    )
    .await);
    assert_eq!(answered["message"], 2);
    let read = ok(call(
        &s.world,
        s.agent,
        OrchTool::ReadShared,
        json!({ "path": path }),
    )
    .await);
    assert!(
        read["text"].as_str().unwrap().contains("· answers #1"),
        "{read}"
    );
    let nothing = send(
        &s,
        json!({ "target": { "kind": "chat", "path": path }, "text": "x", "reply_to": 9 }),
    )
    .await;
    let (code, message) = refused(&nothing);
    assert_eq!(code, "invalid");
    assert!(message.contains("#9"));
}

#[tokio::test]
async fn one_enabled_message_tool_routes_both_targets_without_borrowing_the_owners_reach() {
    let s = with_a_shared_chat(true).await;
    let receiver = s.world.thread("Validate").await;
    s.world
        .set(s.world.topic, "orch_tools.enabled", json!(["send_message"]))
        .await;
    let posted = ok(send(&s, json!({ "target": { "kind": "chat", "path": format!("id:{}", s.chat) }, "text": "For the people." })).await);
    assert_eq!(posted["target"]["kind"], "chat");
    assert_eq!(posted["status"], "posted");
    let queued = ok(send(&s, json!({ "target": { "kind": "thread", "path": format!("id:{receiver}") }, "text": "For the agent." })).await);
    assert_eq!(queued["target"]["kind"], "thread");
    assert_eq!(queued["status"], "queued");
    assert_eq!(queued["delivery"], "deferred");
    assert_eq!(
        queued["delivered"],
        Value::Null,
        "no harness receipt was observed"
    );
    // The same owner can read this topic, but has not shared its chat or thread with the agent.
    let hidden_topic = s.world.topic("Private work").await;
    let hidden_chat = chat_of(&s.world, hidden_topic).await;
    let hidden_thread = thread_in_topic(&s.world, hidden_topic, "Hidden").await;
    for (kind, id) in [("chat", hidden_chat), ("thread", hidden_thread)] {
        let answer = send(
            &s,
            json!({"target":{"kind":kind,"path":format!("id:{id}")},"text":"No borrowed rights."}),
        )
        .await;
        assert_eq!(refused(&answer).0, "not_found");
    }
    assert_eq!(
        s.world
            .count(
                "SELECT count(*) FROM chat_messages WHERE chat_id=$1",
                hidden_chat
            )
            .await,
        0
    );
    assert_eq!(
        s.world
            .count(
                "SELECT count(*) FROM announcements WHERE to_thread_id=$1",
                hidden_thread
            )
            .await,
        0
    );
}

#[tokio::test]
async fn a_chat_reply_or_a_mismatched_kind_never_sends_a_thread_message() {
    let s = with_a_shared_chat(true).await;
    let receiver = s.world.thread("Validate").await;
    let wrong_reply = send(&s, json!({"target":{"kind":"thread","path":format!("id:{receiver}")},"text":"No reply in a thread.","reply_to":1})).await;
    assert_eq!(refused(&wrong_reply).0, "invalid");
    assert!(refused(&wrong_reply).1.contains("reply_to"));
    let wrong_kind = send(
        &s,
        json!({"target":{"kind":"thread","path":format!("id:{}",s.chat)},"text":"Not a thread."}),
    )
    .await;
    assert_eq!(refused(&wrong_kind).0, "wrong_kind");
    assert_eq!(
        s.world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                s.agent
            )
            .await,
        0
    );
    assert_eq!(
        s.world
            .count(
                "SELECT count(*) FROM chat_messages WHERE chat_id=$1",
                s.chat
            )
            .await,
        0
    );
}

#[tokio::test]
async fn a_thread_target_keeps_the_refusal_of_side_threads() {
    let s = with_a_shared_chat(true).await;
    let parent = s.world.thread("Main review").await;
    let side = s.world.thread("Side review").await;
    sqlx::query("UPDATE threads SET side_of=$2 WHERE node_id=$1")
        .bind(side)
        .bind(parent)
        .execute(&mut *s.world.pool.acquire().await.unwrap())
        .await
        .unwrap();
    let answer = send(&s, json!({"target":{"kind":"thread","path":format!("id:{side}")},"text":"A side thread cannot receive this."})).await;
    assert_eq!(refused(&answer).0, "wrong_kind");
    assert!(refused(&answer).1.contains("side thread"));
    assert_eq!(
        s.world
            .count(
                "SELECT count(*) FROM announcements WHERE to_thread_id=$1",
                side
            )
            .await,
        0
    );
    assert_eq!(
        s.world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                side
            )
            .await,
        0
    );
}

#[tokio::test]
async fn historical_single_destination_settings_do_not_gain_the_other_destination() {
    for (old, allowed, denied) in [
        ("send_chat_message", "chat", "thread"),
        ("send_thread_message", "thread", "chat"),
    ] {
        let s = with_a_shared_chat(true).await;
        let receiver = s.world.thread("Validate").await;
        s.world
            .set(s.world.topic, "orch_tools.enabled", json!(["send_message"]))
            .await;
        // Synthetic retained settings from before the public tool consolidation; new writes reject these names.
        sqlx::query("UPDATE settings SET value=$2 WHERE node_id=$1 AND key='orch_tools.enabled'")
            .bind(s.world.topic)
            .bind(json!([old]))
            .execute(&mut *s.world.pool.acquire().await.unwrap())
            .await
            .unwrap();
        let target = |kind: &str| if kind == "chat" { s.chat } else { receiver };
        let accepted = ok(send(&s, json!({"target":{"kind":allowed,"path":format!("id:{}",target(allowed))},"text":"Retained permission."})).await);
        assert_eq!(accepted["target"]["kind"], allowed);
        let refused_call = send(&s, json!({"target":{"kind":denied,"path":format!("id:{}",target(denied))},"text":"No expanded permission."})).await;
        assert_eq!(refused(&refused_call).0, "invalid");
        assert!(refused(&refused_call).1.contains("orch_tools.enabled"));
        let (chats, threads) = (
            s.world
                .count(
                    "SELECT count(*) FROM chat_messages WHERE chat_id=$1",
                    s.chat,
                )
                .await,
            s.world
                .count(
                    "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                    s.agent,
                )
                .await,
        );
        assert_eq!(
            (chats, threads),
            if allowed == "chat" { (1, 0) } else { (0, 1) }
        );
    }
}

#[tokio::test]
async fn history_tool_names_are_readable_but_cannot_invoke_either_route() {
    let s = with_a_shared_chat(true).await;
    let receiver = s.world.thread("Validate").await;
    for (tool, arguments) in [
        (
            OrchTool::SendChatMessage,
            json!({"chat":format!("id:{}",s.chat),"text":"Old name."}),
        ),
        (
            OrchTool::SendThreadMessage,
            json!({"thread":format!("id:{receiver}"),"text":"Old name."}),
        ),
    ] {
        let answer = call(&s.world, s.agent, tool, arguments).await;
        assert_eq!(refused(&answer).0, "invalid");
        assert!(refused(&answer).1.contains("send_message"));
    }
    assert_eq!(
        s.world
            .count(
                "SELECT count(*) FROM chat_messages WHERE chat_id=$1",
                s.chat
            )
            .await,
        0
    );
    assert_eq!(
        s.world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                s.agent
            )
            .await,
        0
    );
}

#[tokio::test]
async fn a_chat_that_takes_no_messages_is_closed_and_says_so() {
    let s = with_a_shared_chat(true).await;
    let path = chat_path(&s).await;
    s.world.set(s.topic, "chat.enabled", json!(false)).await;
    let off = send(
        &s,
        json!({ "target": { "kind": "chat", "path": path }, "text": "Hi" }),
    )
    .await;
    assert_eq!(refused(&off).0, "closed");
    s.world.set(s.topic, "chat.enabled", json!(true)).await;
    s.world
        .exec("UPDATE nodes SET archived_at = now() WHERE name = 'Team'")
        .await;
    let archived = send(
        &s,
        json!({ "target": { "kind": "chat", "path": path }, "text": "Hi" }),
    )
    .await;
    let (code, message) = refused(&archived);
    assert_eq!(code, "closed");
    assert!(message.contains("Nothing can be written there"));
    // Archived chats can be read.
    let read = call(
        &s.world,
        s.agent,
        OrchTool::ReadShared,
        json!({ "path": path }),
    )
    .await;
    assert!(read.error.is_none());
}

#[tokio::test]
async fn text_files_or_both_are_needed_and_text_is_masked_like_any_stored_text() {
    let s = with_a_shared_chat(true).await;
    let path = chat_path(&s).await;
    assert_eq!(
        refused(&send(&s, json!({ "target": { "kind": "chat", "path": path }})).await).0,
        "invalid"
    );
    assert_eq!(
        refused(
            &send(
                &s,
                json!({ "target": { "kind": "chat", "path": path }, "text": "  " })
            )
            .await
        )
        .0,
        "invalid"
    );
    let with_file = send(
        &s,
        json!({ "target": { "kind": "chat", "path": path }, "text": "see", "files": vec!["out.txt"; 11] }),
    )
    .await;
    let (code, message) = refused(&with_file);
    assert_eq!(code, "invalid");
    assert!(message.contains("10"), "{message}");
    s.world
        .set(
            s.world.root,
            "redaction.patterns",
            json!([{ "name": "key", "regex": "testkey_[a-z0-9]{8,}" }]),
        )
        .await;
    ok(send(
        &s,
        json!({ "target": { "kind": "chat", "path": path }, "text": "the key is testkey_synthetic12" }),
    )
    .await);
    let stored: String =
        sqlx::query_scalar("SELECT search_text FROM chat_messages WHERE chat_id = $1")
            .bind(s.chat)
            .fetch_one(&mut *s.world.pool.acquire().await.unwrap())
            .await
            .unwrap();
    assert_eq!(stored, "the key is [masked]");
}

#[tokio::test]
async fn a_call_said_twice_writes_one_message() {
    let s = with_a_shared_chat(true).await;
    let path = chat_path(&s).await;
    let arguments = json!({ "target": { "kind": "chat", "path": path }, "text": "Once." });
    let first = call_as(
        &s.world,
        s.agent,
        OrchTool::SendMessage,
        arguments.clone(),
        "same-call",
    )
    .await;
    assert!(first.error.is_none());
    let again = call_as(
        &s.world,
        s.agent,
        OrchTool::SendMessage,
        arguments,
        "same-call",
    )
    .await;
    assert!(again.error.is_none());
    common::meanwhile(Duration::from_millis(300)).await;
    let rows: i64 = s
        .world
        .count(
            "SELECT count(*) FROM chat_messages WHERE chat_id = $1",
            s.chat,
        )
        .await;
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn an_agent_mentions_a_chat_writer_by_the_handle_its_reading_supplied_and_that_writer_is_told()
 {
    let s = with_a_shared_chat(true).await;
    let path = chat_path(&s).await;
    let bea = s.world.person("bea").await;
    let mut conn = s.world.pool.acquire().await.unwrap();
    orch_db::queries::access::testing::give_role(
        &mut conn,
        s.world.root,
        "user",
        bea,
        "viewer",
        s.world.ada,
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE users SET display_name = 'Beatrice Clark' WHERE id = $1")
        .bind(bea)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    say_in_chat(&s.world, s.chat, bea, "Please report back here.").await;
    let read = ok(call(
        &s.world,
        s.agent,
        OrchTool::ReadShared,
        json!({ "path": path }),
    )
    .await);
    let text = read["text"].as_str().unwrap();
    assert!(text.contains("Beatrice Clark (@bea)"), "{text}");
    // Use the handle the reading supplied, rather than a test's directory of people.
    let handle = text.split("(@").nth(1).unwrap().split(')').next().unwrap();
    assert_eq!(handle, "bea");
    ok(send(
        &s,
        json!({ "target": { "kind": "chat", "path": path }, "text": format!("@{handle} the logs are in ./out. @all @developers") }),
    )
    .await);
    let named: Vec<Id> =
        sqlx::query_scalar("SELECT unnest(mentions) FROM chat_messages WHERE chat_id = $1")
            .bind(s.chat)
            .fetch_all(&mut *s.world.pool.acquire().await.unwrap())
            .await
            .unwrap();
    assert_eq!(named, vec![bea]);
    let all: bool = sqlx::query_scalar(
        "SELECT mentions_all FROM chat_messages WHERE chat_id = $1 AND author_thread_id = $2",
    )
    .bind(s.chat)
    .bind(s.agent)
    .fetch_one(&mut *s.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    assert!(!all, "an agent's @all stays ordinary text");
    let told: i64 = s
        .world
        .count(
            "SELECT count(*) FROM notifications WHERE kind = 'mention' AND user_id = $1",
            bea,
        )
        .await;
    assert_eq!(told, 1);
}

// ---------------------------------------------------------------- edit_document

struct Docs {
    world: common::World,
    agent: Id,
    topic: Id,
    write: Option<Id>,
}

/// An agent, and the topic `Docs` whose documents are shared with it for `level`: `view`, or `edit`, which on a place also makes them.
async fn with_documents(edit: bool) -> Docs {
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Writer").await;
    let topic = world.topic("Docs").await;
    share(&world, "node", topic, "live", "doc.view", "any", agent).await;
    let write = if edit {
        Some(share(&world, "node", topic, "live", "doc.edit", "any", agent).await)
    } else {
        None
    };
    Docs {
        world,
        agent,
        topic,
        write,
    }
}

async fn edit(docs: &Docs, arguments: Value) -> orch_link_contract::commands::OrchToolResult {
    call(&docs.world, docs.agent, OrchTool::EditDocument, arguments).await
}

async fn docs_dir(docs: &Docs) -> String {
    let listing = ok(call(
        &docs.world,
        docs.agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", docs.topic) }),
    )
    .await);
    let folder = entry(&listing, "docs");
    assert_eq!(folder["writable"], json!(true), "{listing}");
    folder["path"].as_str().unwrap().to_owned()
}

/// The description line list_shared gives for the entry `name` of the directory `dir`.
async fn listed_description(docs: &Docs, dir: &str, name: &str) -> Value {
    let listing = ok(call(
        &docs.world,
        docs.agent,
        OrchTool::ListShared,
        json!({ "path": dir }),
    )
    .await);
    entry(&listing, name)["description"].clone()
}

async fn read_doc(docs: &Docs, path: &str) -> Value {
    ok(call(
        &docs.world,
        docs.agent,
        OrchTool::ReadShared,
        json!({ "path": path }),
    )
    .await)
}

#[tokio::test]
async fn a_document_is_made_under_the_name_of_the_agent_and_belongs_to_the_first_owner_of_its_thread()
 {
    let d = with_documents(true).await;
    let dir = docs_dir(&d).await;
    let path = format!("{dir}Week 41.md");
    let made = ok(edit(&d, json!({ "command": "create", "path": path, "markdown": "# Plan\n\nBuild it, then ship it." })).await);
    assert_eq!(made["created"], true);
    assert_eq!(made["path"], json!(path));
    assert_eq!(made["blocks"], 2);
    assert_eq!(made["files"], 0);
    let id: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let (owner, by_thread, by_agent): (Id, Option<Id>, Option<String>) = sqlx::query_as(
        "SELECT owner_id, created_by_thread, created_by_agent FROM documents WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *d.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    assert_eq!(
        owner, d.world.ada,
        "the first owner of the agent's thread answers for it"
    );
    assert_eq!(by_thread, Some(d.agent));
    assert_eq!(by_agent.as_deref(), Some("Fake agent"));
    let creation: i64 = d
        .world
        .count(
            "SELECT count(*) FROM permission_entries WHERE object_id = $1 AND object_kind = 'document' AND permission = 'doc.edit' AND holder_kind = 'agents' AND source = 'creation' AND removed_at IS NULL",
            id,
        )
        .await;
    assert_eq!(
        creation, 1,
        "the agents of that thread are named in the document's own Edit row"
    );
    let read = read_doc(&d, &path).await;
    let text = read["text"].as_str().unwrap();
    assert!(
        text.starts_with("Week 41 (last changed by Fake agent at "),
        "{text}"
    );
    assert!(text.contains("# Plan\n\nBuild it, then ship it."), "{text}");
    let blocks: i64 = d.world.count("SELECT count(*) FROM document_blocks WHERE document_id = $1 AND updated_by_thread IS NOT NULL", id).await;
    assert_eq!(blocks, 2, "each block says it is an agent's");
    let audited: i64 = d.world.count("SELECT count(*) FROM audit_log WHERE action = 'document.created_by_agent' AND object_id = $1", id).await;
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn a_path_that_is_a_document_already_is_refused_with_its_seq_so_that_nothing_is_replaced_by_mistake()
 {
    let d = with_documents(true).await;
    let dir = docs_dir(&d).await;
    let path = format!("{dir}Notes.md");
    ok(edit(
        &d,
        json!({ "command": "create", "path": path, "markdown": "One." }),
    )
    .await);
    let again = edit(
        &d,
        json!({ "command": "create", "path": path, "markdown": "Two." }),
    )
    .await;
    let (code, message) = refused(&again);
    assert_eq!(code, "exists");
    assert!(
        message.contains("seq") && message.contains("rewrite"),
        "{message}"
    );
}

#[tokio::test]
async fn tools_result_form_matches_escaped_read_text_and_preserves_authored_backslashes() {
    let d = with_documents(true).await;
    let path = format!("{}Frames.md", docs_dir(&d).await);
    let original = r"tag: `<orch>`; literal: `\<orch>`; ordinary: `\keep`";
    ok(edit(
        &d,
        json!({"command":"create","path":path,"markdown":original}),
    )
    .await);
    let read = read_doc(&d, &path).await;
    let model_text = orch_engine::instructions::escape_body(read["text"].as_str().unwrap());
    assert!(model_text.contains(r"tag: `<\orch>`"));
    assert!(model_text.contains(r"literal: `\<\orch>`"));
    ok(edit(&d,json!({"command":"replace","path":path,"old":r"tag: `<\orch>`","new":"tag: `done`","seq":read["entry"]["seq"]})).await);
    let after = read_doc(&d, &path).await;
    let stored = after["text"].as_str().unwrap();
    assert!(
        stored.contains(r"tag: `done`; literal: `\<orch>`; ordinary: `\keep`"),
        "{stored}"
    );
    assert!(
        !stored.contains(r"literal: `\<\orch>`"),
        "rendering adds no stored backslash"
    );
}

#[tokio::test]
async fn tools_arguments_edit_uses_seq_and_refuses_base_before_changing_a_document() {
    let d = with_documents(true).await;
    let path = format!("{}Arguments.md", docs_dir(&d).await);
    ok(edit(
        &d,
        json!({"command":"create","path":path,"markdown":"Original."}),
    )
    .await);
    let before = read_doc(&d, &path).await;
    let seq = before["entry"]["seq"].clone();
    for arguments in [
        json!({"command":"rewrite","path":path,"markdown":"Old key.","base":seq}),
        json!({"command":"rewrite","path":path,"markdown":"Both keys.","base":seq,"seq":seq}),
    ] {
        let answer = edit(&d, arguments).await;
        assert_eq!(refused(&answer).0, "invalid");
        let after = read_doc(&d, &path).await;
        assert_eq!(after["text"], before["text"]);
        assert_eq!(after["entry"]["seq"], seq);
    }
    ok(edit(
        &d,
        json!({"command":"rewrite","path":path,"markdown":"Current key.","seq":seq}),
    )
    .await);
    let after = read_doc(&d, &path).await;
    assert!(after["text"].as_str().unwrap().contains("Current key."));
    assert_ne!(after["entry"]["seq"], seq);
}

#[tokio::test]
async fn edit_on_a_document_alone_changes_it_and_makes_no_other_and_view_changes_nothing() {
    let d = with_documents(false).await;
    let doc = d
        .world
        .document_beside(d.topic, "Handbook", &["Be kind."])
        .await;
    let listing = ok(call(
        &d.world,
        d.agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", d.topic), "depth": 2 }),
    )
    .await);
    let docs_folder = entry(&listing, "docs");
    assert_eq!(
        docs_folder["writable"], false,
        "no right to make a document there"
    );
    let path = format!("{}Handbook.md", docs_folder["path"].as_str().unwrap());
    let read_only = edit(
        &d,
        json!({ "command": "replace", "path": path, "old": "kind", "new": "careful" }),
    )
    .await;
    let (code, message) = refused(&read_only);
    assert_eq!(code, "not_writable");
    assert!(
        message.contains("doc.edit") && message.contains("sharing"),
        "{message}"
    );
    let make = edit(&d, json!({ "command": "create", "path": format!("{}Other.md", docs_folder["path"].as_str().unwrap()), "markdown": "x" })).await;
    let (code, message) = refused(&make);
    assert_eq!(code, "not_writable");
    assert!(
        message.contains("doc.edit") && message.contains("place"),
        "{message}"
    );
    share(
        &d.world, "document", doc, "live", "doc.edit", "any", d.agent,
    )
    .await;
    ok(edit(
        &d,
        json!({ "command": "replace", "path": path, "old": "kind", "new": "careful" }),
    )
    .await);
    let now = read_doc(&d, &path).await;
    assert!(now["text"].as_str().unwrap().contains("Be careful."));
    let other = edit(&d, json!({ "command": "create", "path": format!("{}Other.md", docs_folder["path"].as_str().unwrap()), "markdown": "x" })).await;
    assert_eq!(
        refused(&other).0,
        "not_writable",
        "a document shared for editing gives no right to make another"
    );
}

#[tokio::test]
async fn replace_changes_the_block_that_holds_the_passage_and_no_other() {
    let d = with_documents(true).await;
    let doc = d
        .world
        .document_beside(
            d.topic,
            "Plan",
            &["First we build.", "Then we ship.", "Finally we rest."],
        )
        .await;
    let before = d.world.blocks_of(doc).await;
    let path = format!("{}Plan.md", docs_dir(&d).await);
    let read = read_doc(&d, &path).await;
    let seq = read["entry"]["seq"].as_i64().unwrap();
    let changed = ok(edit(&d, json!({ "command": "replace", "path": path, "old": "we ship", "new": "we test and ship", "seq": seq })).await);
    assert!(changed["seq"].as_i64().unwrap() > seq);
    assert_eq!(changed["created"], false);
    let after = d.world.blocks_of(doc).await;
    let texts: Vec<&str> = after.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "First we build.",
            "Then we test and ship.",
            "Finally we rest."
        ]
    );
    assert_eq!(
        before[0].0, after[0].0,
        "the block before is the same block"
    );
    assert_eq!(before[2].0, after[2].0, "and so is the block after");
    let history: i64 = d.world.count("SELECT count(*) FROM audit_log WHERE action = 'document.edited_by_agent' AND object_id = $1", doc).await;
    assert_eq!(history, 1);
    let by_agent: i64 = d.world.count("SELECT count(*) FROM document_blocks WHERE document_id = $1 AND updated_by_thread IS NOT NULL AND removed_at IS NULL", doc).await;
    assert_eq!(by_agent, 1, "the block that changed says who changed it");
}

#[tokio::test]
async fn a_passage_that_is_not_there_or_is_there_twice_is_refused_in_words_that_say_how_often_and_where()
 {
    let d = with_documents(true).await;
    d.world
        .document_beside(
            d.topic,
            "Cats",
            &["A cat sat.", "Another cat ran.", "No dogs."],
        )
        .await;
    let path = format!("{}Cats.md", docs_dir(&d).await);
    let none = edit(
        &d,
        json!({ "command": "replace", "path": path, "old": "fish", "new": "x" }),
    )
    .await;
    assert_eq!(refused(&none).0, "no_match");
    let twice = edit(
        &d,
        json!({ "command": "replace", "path": path, "old": "cat", "new": "dog" }),
    )
    .await;
    let (code, message) = refused(&twice);
    assert_eq!(code, "ambiguous");
    assert!(
        message.contains("2 times") && message.contains("block 1") && message.contains("block 2"),
        "{message}"
    );
    ok(edit(
        &d,
        json!({ "command": "replace", "path": path, "old": "cat", "new": "dog", "all": true }),
    )
    .await);
    let read = read_doc(&d, &path).await;
    let text = read["text"].as_str().unwrap();
    assert!(
        text.contains("A dog sat.") && text.contains("Another dog ran."),
        "{text}"
    );
}

#[tokio::test]
async fn a_change_on_a_state_somebody_changed_since_is_refused_with_changed_and_one_elsewhere_goes_through()
 {
    let d = with_documents(true).await;
    let doc = d
        .world
        .document_beside(d.topic, "Plan", &["First we build.", "Then we ship."])
        .await;
    let path = format!("{}Plan.md", docs_dir(&d).await);
    let seq = read_doc(&d, &path).await["entry"]["seq"].as_i64().unwrap();
    d.world
        .change_block(doc, "First", "First we plan, then build.")
        .await;
    let touched = edit(&d, json!({ "command": "replace", "path": path, "old": "First we", "new": "At first we", "seq": seq })).await;
    let (code, message) = refused(&touched);
    assert_eq!(code, "changed");
    assert!(
        message.contains("Read it again")
            || message.contains("read it again")
            || message.contains("again"),
        "{message}"
    );
    ok(edit(&d, json!({ "command": "replace", "path": path, "old": "we ship", "new": "we launch", "seq": seq })).await);
    let read = read_doc(&d, &path).await;
    let text = read["text"].as_str().unwrap();
    assert!(
        text.contains("First we plan, then build.") && text.contains("Then we launch."),
        "{text}"
    );
    // Without seq, it applies to the document as it stands.
    ok(edit(
        &d,
        json!({ "command": "replace", "path": path, "old": "plan, then", "new": "plan and" }),
    )
    .await);
}

#[tokio::test]
async fn rewrite_needs_the_state_it_was_written_on_cuts_a_version_first_and_replaces_the_whole() {
    let d = with_documents(true).await;
    let doc = d
        .world
        .document_beside(d.topic, "Plan", &["Old one.", "Old two."])
        .await;
    let path = format!("{}Plan.md", docs_dir(&d).await);
    let without = edit(
        &d,
        json!({ "command": "rewrite", "path": path, "markdown": "New." }),
    )
    .await;
    let (code, message) = refused(&without);
    assert_eq!(code, "invalid");
    assert!(message.contains("\"seq\""), "{message}");
    let seq = read_doc(&d, &path).await["entry"]["seq"].as_i64().unwrap();
    d.world.change_block(doc, "Old two", "Older two.").await;
    let stale = edit(
        &d,
        json!({ "command": "rewrite", "path": path, "markdown": "New.", "seq": seq }),
    )
    .await;
    assert_eq!(refused(&stale).0, "changed");
    let seq = read_doc(&d, &path).await["entry"]["seq"].as_i64().unwrap();
    let done = ok(edit(&d, json!({ "command": "rewrite", "path": path, "markdown": "# New\n\nThe whole text.", "seq": seq })).await);
    assert_eq!(done["blocks"], 2);
    assert!(
        done["version"].as_i64().unwrap() >= 1,
        "a version is cut first"
    );
    let texts: Vec<String> = d
        .world
        .blocks_of(doc)
        .await
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(texts, ["New", "The whole text."]);
    let versions: i64 = d
        .world
        .count(
            "SELECT count(*) FROM document_versions WHERE document_id = $1",
            doc,
        )
        .await;
    assert!(versions >= 1);
    let old_version = ok(call(
        &d.world,
        d.agent,
        OrchTool::ReadShared,
        json!({ "path": path.replace(".md", ".history/0001") }),
    )
    .await);
    let _ = old_version;
}

#[tokio::test]
async fn insert_adds_markdown_after_the_block_that_holds_a_text_or_at_the_end() {
    let d = with_documents(true).await;
    let doc = d
        .world
        .document_beside(d.topic, "Plan", &["One.", "Three."])
        .await;
    let path = format!("{}Plan.md", docs_dir(&d).await);
    ok(edit(
        &d,
        json!({ "command": "insert", "path": path, "markdown": "Two.", "after": "One." }),
    )
    .await);
    ok(edit(
        &d,
        json!({ "command": "insert", "path": path, "markdown": "Four." }),
    )
    .await);
    let texts: Vec<String> = d
        .world
        .blocks_of(doc)
        .await
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(texts, ["One.", "Two.", "Three.", "Four."]);
    let nowhere = edit(
        &d,
        json!({ "command": "insert", "path": path, "markdown": "x", "after": "Zero." }),
    )
    .await;
    assert_eq!(refused(&nowhere).0, "no_match");
}

#[tokio::test]
async fn rename_gives_another_title_and_each_command_takes_its_own_arguments_and_no_others() {
    let d = with_documents(true).await;
    let doc = d.world.document_beside(d.topic, "Draft", &["Text."]).await;
    let path = format!("{}Draft.md", docs_dir(&d).await);
    let renamed = ok(edit(
        &d,
        json!({ "command": "rename", "path": path, "title": "Final" }),
    )
    .await);
    assert!(
        renamed["path"]
            .as_str()
            .unwrap()
            .ends_with("/docs/Final.md")
    );
    let title: String = d
        .world
        .scalar("SELECT title FROM documents WHERE id = $1", doc)
        .await;
    assert_eq!(title, "Final");
    let wrong = edit(
        &d,
        json!({ "command": "rename", "path": renamed["path"], "title": "X", "old": "y" }),
    )
    .await;
    let (code, message) = refused(&wrong);
    assert_eq!(code, "invalid");
    assert!(message.contains("rename takes"), "{message}");
    let attach = edit(&d, json!({ "command": "attach", "path": renamed["path"] })).await;
    assert_eq!(
        refused(&attach).0,
        "invalid",
        "attach requires its source file"
    );
    assert!(refused(&attach).1.contains("file"));
    let detach = edit(
        &d,
        json!({ "command": "detach", "path": renamed["path"], "name": "none.png" }),
    )
    .await;
    assert_eq!(refused(&detach).0, "not_found");
}

#[tokio::test]
async fn an_agent_describes_a_document_and_list_shared_shows_its_first_line_or_else_its_first_paragraph()
 {
    let d = with_documents(true).await;
    let doc = d
        .world
        .document_beside(
            d.topic,
            "Draft",
            &[
                "Ship the redirect fix behind the flag.",
                "Then announce it.",
            ],
        )
        .await;
    let dir = docs_dir(&d).await;
    assert_eq!(
        listed_description(&d, &dir, "Draft.md").await,
        "Ship the redirect fix behind the flag.",
        "a document nobody described is listed with the first line of its first paragraph"
    );
    let path = format!("{dir}Draft.md");
    let described = ok(edit(
        &d,
        json!({ "command": "describe", "path": path, "description": "Release checklist\nfor the web" }),
    )
    .await);
    assert_eq!(described["created"], false);
    let (text, source): (String, String) =
        sqlx::query_as("SELECT description, description_source FROM documents WHERE id = $1")
            .bind(doc)
            .fetch_one(&mut *d.world.pool.acquire().await.unwrap())
            .await
            .unwrap();
    assert_eq!(
        (text.as_str(), source.as_str()),
        ("Release checklist\nfor the web", "user"),
        "a description an agent writes is kept as a person's is"
    );
    assert_eq!(
        listed_description(&d, &dir, "Draft.md").await,
        "Release checklist"
    );
    let audited: i64 = d
        .world
        .count(
            "SELECT count(*) FROM audit_log WHERE action = 'document.edited_by_agent' AND object_id = $1 AND details->>'command' = 'describe'",
            doc,
        )
        .await;
    assert_eq!(audited, 1);
    let made = ok(edit(
        &d,
        json!({
            "command": "create", "path": format!("{dir}Week 41.md"),
            "markdown": "Build it, then ship it.", "description": "What week 41 is for"
        }),
    )
    .await);
    let id: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let given: String = d
        .world
        .scalar("SELECT description FROM documents WHERE id = $1", id)
        .await;
    assert_eq!(
        given, "What week 41 is for",
        "create takes a description too"
    );
    assert_eq!(
        listed_description(&d, &dir, "Week 41.md").await,
        "What week 41 is for"
    );
}

#[tokio::test]
async fn a_document_shared_for_reading_is_not_described() {
    let d = with_documents(false).await;
    d.world.document_beside(d.topic, "Draft", &["Text."]).await;
    let listing = ok(call(
        &d.world,
        d.agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", d.topic) }),
    )
    .await);
    let dir = entry(&listing, "docs")["path"].as_str().unwrap().to_owned();
    let refusal = edit(
        &d,
        json!({ "command": "describe", "path": format!("{dir}Draft.md"), "description": "Mine" }),
    )
    .await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_writable");
    assert!(message.contains("doc.edit"), "{message}");
}

#[tokio::test]
async fn what_an_agent_made_it_goes_on_writing_after_the_place_was_closed_to_it_and_it_makes_no_more_there()
 {
    let d = with_documents(true).await;
    let dir = docs_dir(&d).await;
    let path = format!("{dir}Mine.md");
    ok(edit(
        &d,
        json!({ "command": "create", "path": path, "markdown": "Made by me." }),
    )
    .await);
    // The place is closed to the agent: every entry that names its agents is taken away from the topic's page.
    d.world
        .exec("UPDATE permission_entries SET removed_at = now() WHERE object_kind = 'node' AND holder_kind = 'agents'")
        .await;
    let again = edit(
        &d,
        json!({ "command": "replace", "path": path, "old": "me", "new": "the agent" }),
    )
    .await;
    assert!(
        again.error.is_none(),
        "it goes on writing what it made: {:?}",
        again.error
    );
    let id: Id = d
        .world
        .scalar("SELECT id FROM documents WHERE title = 'Mine'", Id::nil())
        .await;
    let stored: String = d.world.scalar("SELECT string_agg(text, ' ') FROM document_blocks WHERE document_id = $1 AND removed_at IS NULL", id).await;
    assert!(stored.contains("Made by the agent."), "{stored}");
    let another = edit(
        &d,
        json!({ "command": "create", "path": format!("{dir}Second.md"), "markdown": "x" }),
    )
    .await;
    assert_eq!(
        refused(&another).0,
        "not_writable",
        "and it makes no more documents there"
    );
}

#[tokio::test]
async fn a_change_that_would_pass_a_limit_of_the_place_is_refused_with_the_limit_and_where_it_is_set()
 {
    let d = with_documents(true).await;
    d.world.set(d.topic, "docs.max_bytes", json!(65_536)).await;
    d.world.document_beside(d.topic, "Big", &["x"]).await;
    let path = format!("{}Big.md", docs_dir(&d).await);
    let seq = read_doc(&d, &path).await["entry"]["seq"].as_i64().unwrap();
    let paragraphs: Vec<String> = (0..40)
        .map(|n| format!("Paragraph {n}: {}", "word ".repeat(400)))
        .collect();
    let refusal = edit(&d, json!({ "command": "rewrite", "path": path, "markdown": paragraphs.join("\n\n"), "seq": seq })).await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "limit");
    assert!(message.contains("docs.max_bytes"), "{message}");
}

#[tokio::test]
async fn a_person_named_in_what_the_agent_wrote_is_not_notified() {
    let d = with_documents(true).await;
    let bea = d.world.person("bea").await;
    let mut conn = d.world.pool.acquire().await.unwrap();
    orch_db::queries::access::testing::give_role(
        &mut conn,
        d.world.root,
        "user",
        bea,
        "viewer",
        d.world.ada,
        None,
    )
    .await
    .unwrap();
    drop(conn);
    let path = format!("{}Notes.md", docs_dir(&d).await);
    ok(edit(
        &d,
        json!({ "command": "create", "path": path, "markdown": "Please review, @bea." }),
    )
    .await);
    let told: i64 = d
        .world
        .count(
            "SELECT count(*) FROM notifications WHERE kind = 'doc.mention' AND user_id = $1",
            bea,
        )
        .await;
    assert_eq!(told, 0);
}

// ---------------------------------------------------------------- send_message to a thread shared for writing

#[tokio::test]
async fn a_thread_of_another_topic_is_handed_a_message_only_where_it_was_shared_for_writing() {
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Writer").await;
    let docs = world.topic("Docs").await;
    let other = thread_in_topic(&world, docs, "Other").await;
    share(&world, "node", other, "live", "thread.view", "any", agent).await;
    let listing = ok(call(
        &world,
        agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{docs}") }),
    )
    .await);
    let path = entry(&listing, "Other")["path"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(entry(&listing, "Other")["writable"], false);
    let reading = call(
        &world,
        agent,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": path }, "text": "Hello" }),
    )
    .await;
    let (code, message) = refused(&reading);
    assert_eq!(code, "not_found");
    assert!(message.contains("shared with you for writing"), "{message}");
    share(
        &world,
        "node",
        other,
        "live",
        "thread.interact",
        "any",
        agent,
    )
    .await;
    let listing = ok(call(
        &world,
        agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{docs}") }),
    )
    .await);
    assert_eq!(entry(&listing, "Other")["writable"], true);
    let sent = ok(call(
        &world,
        agent,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": path }, "text": "The build is in ./out" }),
    )
    .await);
    assert_eq!(sent["target"]["path"], path);
    assert_eq!(sent["status"], "queued");
    let (topic, to): (Id, Option<Id>) = sqlx::query_as(
        "SELECT topic_id, to_thread_id FROM announcements WHERE from_thread_id = $1",
    )
    .bind(agent)
    .fetch_one(&mut *world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    assert_eq!(
        (topic, to),
        (docs, Some(other)),
        "it is delivered by the settings of that thread's topic"
    );
    // By its identifier as well.
    let by_id = ok(call(
        &world,
        agent,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": format!("id:{other}") }, "text": "Again" }),
    )
    .await);
    assert_eq!(by_id["target"]["path"], path);
}
