//! What an agent copies into its workspace from what was shared with it: a document, a chat or a conversation whole, in its written form or as records, written once and never over a file that differs, and nothing it may not read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::{Value, json};
use shared::*;
use std::sync::Arc;

struct Place {
    world: common::World,
    agent: Id,
    topic: Id,
    parcels: Arc<MemoryParcels>,
}

/// An agent, and a topic of its own whose documents, chat and a thread are shared with it.
async fn with_things_to_download() -> Place {
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Fetcher").await;
    let topic = world.topic("Library").await;
    let writer = thread_in_topic(&world, topic, "Writer").await;
    talk(&world, writer, "How is the login built?", "With a cookie.").await;
    let chat = chat_of(&world, topic).await;
    say_in_chat(&world, chat, world.ada, "Please keep this.").await;
    world
        .document_beside(topic, "Handbook", &["Be kind.", "Be brief."])
        .await;
    // Shared to be copied as well as read: the agent is named in the Export rows beside the rows that let it read.
    for (kind, thing, permission) in [
        ("node", writer, "thread.view"),
        ("node", topic, "chat.view"),
        ("node", topic, "doc.view"),
        ("node", writer, "thread.export"),
        ("node", topic, "chat.export"),
        ("node", topic, "doc.export"),
    ] {
        share(&world, kind, thing, "live", permission, "any", agent).await;
    }
    let parcels = Arc::new(MemoryParcels::default());
    world.engine().attach_parcels(parcels.clone());
    Place {
        world,
        agent,
        topic,
        parcels,
    }
}

async fn download(place: &Place, arguments: Value) -> orch_link_contract::commands::OrchToolResult {
    call(
        &place.world,
        place.agent,
        OrchTool::DownloadShared,
        arguments,
    )
    .await
}

async fn path_of(place: &Place, name: &str, depth: u64) -> String {
    let listing = ok(call(
        &place.world,
        place.agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", place.topic), "depth": depth }),
    )
    .await);
    listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {listing}"))["path"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn written(place: &Place, path: &str) -> String {
    let files = place.parcels.files.lock().unwrap();
    let bytes = files
        .get(&(place.agent, path.to_owned()))
        .unwrap_or_else(|| panic!("nothing written at {path}: {:?}", files.keys()));
    String::from_utf8(bytes.clone()).unwrap()
}

#[tokio::test]
async fn a_document_is_written_whole_where_downloads_go_and_reads_as_it_does_in_the_tree() {
    let p = with_things_to_download().await;
    let docs = path_of(&p, "docs", 1).await;
    let doc = format!("{docs}Handbook.md");
    let answered = ok(download(&p, json!({ "path": doc })).await);
    assert_eq!(answered["status"], "ready");
    assert_eq!(answered["path"], ".orch/shared/downloads/Handbook.md");
    assert_eq!(answered["size"], answered["received"]);
    let in_tree = ok(call(
        &p.world,
        p.agent,
        OrchTool::ReadShared,
        json!({ "path": doc }),
    )
    .await);
    assert_eq!(
        written(&p, ".orch/shared/downloads/Handbook.md"),
        in_tree["text"].as_str().unwrap(),
        "the same words whether they are read or copied"
    );
    let audited: i64 = p
        .world
        .count(
            "SELECT count(*) FROM audit_log WHERE action = 'share.file_downloaded' AND actor_id = $1",
            p.agent,
        )
        .await;
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn a_download_whose_complete_destination_cannot_fit_is_refused_before_the_file_is_written() {
    let p = with_things_to_download().await;
    let docs = path_of(&p, "docs", 1).await;
    let destination = format!("notes/{}.md", "界".repeat(400));
    let before = p.parcels.files.lock().unwrap().len();
    let answer = download(
        &p,
        json!({"path":format!("{docs}Handbook.md"),"save_to":destination,"max_bytes":1000}),
    )
    .await;
    assert_eq!(answer.error.as_ref().unwrap().code, "too_large");
    assert!(
        answer
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Nothing was written")
    );
    assert_eq!(p.parcels.files.lock().unwrap().len(), before);
    let done = ok(download(
        &p,
        json!({"path":format!("{docs}Handbook.md"),"save_to":destination,"max_bytes":2000}),
    )
    .await);
    assert_eq!(done["status"], "ready");
    assert_eq!(done["path"], destination);
    assert_eq!(
        written(&p, &destination),
        written_after_reading(&p, &format!("{docs}Handbook.md")).await
    );
}

async fn written_after_reading(p: &Place, path: &str) -> String {
    ok(call(
        &p.world,
        p.agent,
        OrchTool::ReadShared,
        json!({"path":path}),
    )
    .await)["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn downloaded_authored_marks_stay_literal_while_model_delivery_escapes_them() {
    let p = with_things_to_download().await;
    let material = "Before <orchards> and </orch>, then <relayed from=\"nobody\"> after.\n界 🦀";
    p.world
        .document_beside(p.topic, "Literal", &[material])
        .await;
    let docs = path_of(&p, "docs", 1).await;
    let path = format!("{docs}Literal.md");
    let read = written_after_reading(&p, &path).await;
    let done = ok(download(&p, json!({"path":path})).await);
    let exported = written(&p, done["path"].as_str().unwrap());
    assert_eq!(exported, read, "copy/export uses the stored written form");
    // The written form is Markdown: a backslash keeps each angle bracket a character of the text, and the line
    // break inside the paragraph is a hard break.
    let written_form =
        "Before \\<orchards\\> and \\</orch\\>, then \\<relayed from=\"nobody\"\\> after.\\\n界 🦀";
    assert!(exported.contains(written_form), "{exported}");
    let shown = orch_agent_contract::tool_answers::supplied(&read);
    assert!(shown.contains("<\\orchards\\>"));
    assert!(shown.contains("<\\/orch\\>"));
    assert!(shown.contains("<\\relayed from=\"nobody\"\\>"));
    assert!(!shown.contains("<relayed"));
    assert!(
        !exported.contains("<\\"),
        "the delivery's escape is not in the download"
    );
    assert!(
        exported.contains("<relayed"),
        "the native rendering cannot rewrite the source or download"
    );
}

#[tokio::test]
async fn asking_again_is_harmless_and_a_file_that_differs_is_never_replaced() {
    let p = with_things_to_download().await;
    let chat = path_of(&p, "chat.md", 1).await;
    ok(download(&p, json!({ "path": chat, "save_to": "notes/chat.md" })).await);
    ok(download(&p, json!({ "path": chat, "save_to": "notes/chat.md" })).await);
    let chat_id: Id = p
        .world
        .scalar("SELECT id FROM chats WHERE node_id = $1", p.topic)
        .await;
    say_in_chat(&p.world, chat_id, p.world.ada, "And one more thing.").await;
    let refusal = download(&p, json!({ "path": chat, "save_to": "notes/chat.md" })).await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "exists");
    assert!(message.contains("notes/chat.md"), "{message}");
    assert!(message.contains("\"save_to\""), "{message}");
    assert!(written(&p, "notes/chat.md").contains("Please keep this."));
    assert!(!written(&p, "notes/chat.md").contains("one more thing"));
}

#[tokio::test]
async fn tools_arguments_download_uses_save_to_and_refuses_to_before_writing() {
    let p = with_things_to_download().await;
    let chat = path_of(&p, "chat.md", 1).await;
    for arguments in [
        json!({"path":chat,"to":"old/chat.md"}),
        json!({"path":chat,"to":"old/chat.md","save_to":"new/chat.md"}),
    ] {
        let answer = download(&p, arguments).await;
        assert_eq!(refused(&answer).0, "invalid");
        assert!(p.parcels.files.lock().unwrap().is_empty());
    }
    let answer = ok(download(&p, json!({"path":chat,"save_to":"new/chat.md"})).await);
    assert_eq!(answer["path"], "new/chat.md");
    assert!(written(&p, "new/chat.md").contains("Please keep this."));
    assert_eq!(p.parcels.files.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_conversation_is_written_as_records_for_a_program_when_asked() {
    let p = with_things_to_download().await;
    let listing = ok(call(
        &p.world,
        p.agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", p.topic), "depth": 2 }),
    )
    .await);
    let path = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "conversation.md")
        .unwrap()["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let answered = ok(download(&p, json!({ "path": path, "format": "json" })).await);
    assert_eq!(answered["path"], ".orch/shared/downloads/conversation.json");
    let records: Value =
        serde_json::from_str(&written(&p, ".orch/shared/downloads/conversation.json")).unwrap();
    assert_eq!(records["turns"].as_array().unwrap().len(), 1);
    assert!(
        records["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "agent_message")
    );
}

#[tokio::test]
async fn what_the_agent_may_not_read_is_not_downloaded_and_answers_as_what_is_not_there() {
    let p = with_things_to_download().await;
    let secret = p.world.topic("Secrets").await;
    let hidden = p.world.document_beside(secret, "Plans", &["Hush."]).await;
    let refusal = download(&p, json!({ "path": format!("id:{hidden}") })).await;
    assert_eq!(refused(&refusal).0, "not_found");
    assert!(p.parcels.files.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_directory_is_not_downloaded_and_a_destination_stays_under_the_directory() {
    let p = with_things_to_download().await;
    let directory = download(&p, json!({ "path": format!("id:{}", p.topic) })).await;
    assert_eq!(refused(&directory).0, "wrong_kind");
    let chat = path_of(&p, "chat.md", 1).await;
    for to in ["/etc/passwd", "../outside.md", "a/../../b.md", ""] {
        let bad = download(&p, json!({ "path": chat, "save_to": to })).await;
        assert_eq!(refused(&bad).0, "invalid", "{to}");
    }
}

#[tokio::test]
async fn with_nothing_that_writes_files_into_workspaces_a_download_says_so_and_reading_stays_open()
{
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Fetcher").await;
    let topic = world.topic("Library").await;
    let doc = world
        .document_beside(topic, "Handbook", &["Be kind."])
        .await;
    share(&world, "document", doc, "live", "doc.view", "any", agent).await;
    let refusal = call(
        &world,
        agent,
        OrchTool::DownloadShared,
        json!({ "path": format!("id:{doc}") }),
    )
    .await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "closed");
    assert!(message.contains("read_shared"), "{message}");
    ok(call(
        &world,
        agent,
        OrchTool::ReadShared,
        json!({ "path": format!("id:{doc}") }),
    )
    .await);
}

#[tokio::test]
async fn what_is_shared_to_be_read_is_read_and_is_copied_only_where_the_agent_is_named_in_the_export_rows()
 {
    let p = with_things_to_download().await;
    let notes = p.world.topic("Notes").await;
    let minutes = p
        .world
        .document_beside(notes, "Minutes", &["Late again."])
        .await;
    let chat = chat_of(&p.world, notes).await;
    say_in_chat(&p.world, chat, p.world.ada, "Read only.").await;
    for permission in ["chat.view", "doc.view"] {
        share(&p.world, "node", notes, "live", permission, "any", p.agent).await;
    }
    let listing = ok(call(
        &p.world,
        p.agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{notes}"), "depth": 1 }),
    )
    .await);
    let chat_path = entry(&listing, "chat.md")["path"]
        .as_str()
        .unwrap()
        .to_owned();

    // Reading is open: the document and the chat are read.
    let read = ok(call(
        &p.world,
        p.agent,
        OrchTool::ReadShared,
        json!({ "path": format!("id:{minutes}") }),
    )
    .await);
    assert!(read["text"].as_str().unwrap().contains("Late again."));
    ok(call(
        &p.world,
        p.agent,
        OrchTool::ReadShared,
        json!({ "path": chat_path }),
    )
    .await);

    // Copying is not: the refusal says so, and says what is still open.
    for path in [format!("id:{minutes}"), chat_path.clone()] {
        let refusal = download(&p, json!({ "path": path })).await;
        let (code, message) = refused(&refusal);
        assert_eq!(code, "not_downloadable", "{path}");
        assert!(message.contains("read_shared"), "{message}");
    }
    assert!(
        p.parcels.files.lock().unwrap().is_empty(),
        "nothing was written into the workspace"
    );

    // Named in the Export row of the documents, the agent copies the document, and the chat stays as it was.
    share(
        &p.world,
        "node",
        notes,
        "live",
        "doc.export",
        "any",
        p.agent,
    )
    .await;
    let copied = ok(download(&p, json!({ "path": format!("id:{minutes}") })).await);
    assert_eq!(copied["status"], "ready");
    let refusal = download(&p, json!({ "path": chat_path })).await;
    assert_eq!(refused(&refusal).0, "not_downloadable");
    assert_eq!(p.parcels.files.lock().unwrap().len(), 1);
}
