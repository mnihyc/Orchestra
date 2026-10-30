//! References an agent writes and reads: one grammar for a path or an identifier with a part, resolved from its thread into typed references that writing above them does not move, given back in each reader's own terms, kept through a change written around them, and told back as what resolved and what was kept as text.
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

/// The agent's own topic holds the documents its policy reaches; an agent writes them by relative paths.
struct Desk {
    world: common::World,
    agent: Id,
    plan: Id,
}

/// An agent, and "Rollout plan" beside its own topic with three paragraphs.
async fn desk() -> Desk {
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Writer").await;
    let plan = world
        .document_beside(
            world.topic,
            "Rollout plan",
            &["Build it.", "Ship it behind the flag.", "Announce it."],
        )
        .await;
    Desk { world, agent, plan }
}

async fn edit(world: &common::World, thread: Id, arguments: Value) -> Value {
    ok(call(world, thread, OrchTool::EditDocument, arguments).await)
}

async fn read(world: &common::World, thread: Id, path: &str) -> String {
    ok(call(world, thread, OrchTool::ReadShared, json!({ "path": path })).await)["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The stored inline content of every live block of a document, in no particular order.
async fn contents(world: &common::World, document: Id) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT content FROM document_blocks WHERE document_id = $1 AND removed_at IS NULL",
    )
    .bind(document)
    .fetch_all(&mut *world.pool.acquire().await.unwrap())
    .await
    .unwrap()
}

/// The typed references the blocks of a document hold.
async fn refs_of(world: &common::World, document: Id) -> Vec<Value> {
    contents(world, document)
        .await
        .into_iter()
        .flat_map(|content| content.as_array().cloned().unwrap_or_default())
        .filter(|run| run["type"] == "ref")
        .collect()
}

/// The identifiers of a document's top-level blocks in the order of the page.
async fn blocks_in_order(world: &common::World, document: Id) -> Vec<Id> {
    sqlx::query_scalar(
        "SELECT id FROM document_blocks WHERE document_id = $1 AND parent_id IS NULL AND removed_at IS NULL
          ORDER BY position, id",
    )
    .bind(document)
    .fetch_all(&mut *world.pool.acquire().await.unwrap())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_link_and_a_backquoted_path_with_a_part_are_kept_typed_and_the_answer_says_what_resolved()
{
    let d = desk().await;
    let made = edit(
        &d.world,
        d.agent,
        json!({
            "command": "create", "path": "docs/Notes.md",
            "markdown": "See [the plan](<docs/Rollout plan.md#2>) and `docs/Rollout plan.md`, not [this](docs/Missing.md) nor `id:0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e09#3` nor [loose](docs/Rollout plan.md)."
        }),
    )
    .await;
    let references = &made["references"];
    assert_eq!(
        references["resolved"],
        json!([
            { "path": "docs/Rollout plan.md#2", "kind": "document", "name": "Rollout plan", "part": 2 },
            { "path": "docs/Rollout plan.md", "kind": "document", "name": "Rollout plan" },
        ])
    );
    let kept: Vec<&str> = references["kept"]
        .as_array()
        .unwrap()
        .iter()
        .map(|one| one["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        kept,
        [
            "docs/Missing.md",
            "id:0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5e09#3",
            "[loose](docs/Rollout plan.md)"
        ],
        "every reference that did not resolve is told, with why, and so is a link Markdown does not read"
    );
    let notes: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let second = blocks_in_order(&d.world, d.plan).await[1];
    assert_eq!(
        refs_of(&d.world, notes).await,
        [
            json!({ "type": "ref", "kind": "document", "id": d.plan, "part": { "block": second }, "label":"the plan" }),
            json!({ "type": "ref", "kind": "document", "id": d.plan, "label":"docs/Rollout plan.md" }),
        ],
        "the part is held by its block"
    );
    let text: String = sqlx::query_scalar(
        "SELECT string_agg(text, ' ') FROM document_blocks WHERE document_id = $1 AND removed_at IS NULL",
    )
    .bind(notes)
    .fetch_one(&mut *d.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    assert!(
        text.contains("not this nor"),
        "a link that did not resolve keeps its words: {text}"
    );
}

#[tokio::test]
async fn a_reference_is_given_back_in_the_readers_terms_and_writing_above_its_block_moves_nothing()
{
    let d = desk().await;
    edit(
        &d.world,
        d.agent,
        json!({ "command": "create", "path": "docs/Notes.md", "markdown": "See [x](<docs/Rollout plan.md#2>)." }),
    )
    .await;
    let first = read(&d.world, d.agent, "docs/Notes.md").await;
    assert!(
        first.contains("See [Rollout plan](<docs/Rollout plan.md#2>)."),
        "{first}"
    );
    // A person writes a paragraph above the block the reference names.
    d.world.add_paragraph(d.plan, "Before everything.").await;
    let again = read(&d.world, d.agent, "docs/Notes.md").await;
    assert!(
        again.contains("See [Rollout plan](<docs/Rollout plan.md#3>)."),
        "the same block, which is now the third: {again}"
    );
    let block = read(&d.world, d.agent, "docs/Rollout plan.md").await;
    assert!(block.contains("Ship it behind the flag."));
}

#[tokio::test]
async fn a_rendered_reference_quoted_in_a_replace_is_the_stored_reference_again() {
    let d = desk().await;
    let made = edit(
        &d.world,
        d.agent,
        json!({ "command": "create", "path": "docs/Notes.md", "markdown": "See [x](<docs/Rollout plan.md#2>) today." }),
    )
    .await;
    let notes: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let mut before = refs_of(&d.world, notes).await;
    before[0]["label"] = json!("Rollout plan");
    let replaced = edit(
        &d.world,
        d.agent,
        json!({
            "command": "replace", "path": "docs/Notes.md",
            "old": "See [Rollout plan](<docs/Rollout plan.md#2>) today.",
            "new": "Read [Rollout plan](<docs/Rollout plan.md#2>) now."
        }),
    )
    .await;
    assert_eq!(
        refs_of(&d.world, notes).await,
        before,
        "the same reference, the same block"
    );
    assert_eq!(
        replaced["references"]["resolved"][0]["name"], "Rollout plan",
        "what the new text names is told back"
    );
    let text = read(&d.world, d.agent, "docs/Notes.md").await;
    assert!(
        text.contains("Read [Rollout plan](<docs/Rollout plan.md#2>) now."),
        "{text}"
    );
}

#[tokio::test]
async fn a_reader_gets_only_the_shared_label_and_writing_around_it_keeps_the_reference() {
    let d = desk().await;
    let made = edit(
        &d.world,
        d.agent,
        json!({ "command": "create", "path": "docs/Notes.md", "markdown": "See [x](<docs/Rollout plan.md#2>) today." }),
    )
    .await;
    let notes: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let stored = refs_of(&d.world, notes).await;
    // Another agent, of another topic, may read and write the notes and nothing else of this topic.
    let elsewhere = d.world.topic("Elsewhere").await;
    let other = thread_in_topic(&d.world, elsewhere, "Reviewer").await;
    share(
        &d.world, "document", notes, "live", "doc.view", "any", other,
    )
    .await;
    share(
        &d.world, "document", notes, "live", "doc.edit", "any", other,
    )
    .await;
    let path = format!("id:{notes}");
    let text = read(&d.world, other, &path).await;
    assert!(text.contains("See x (no access) today."), "{text}");
    assert!(!text.contains("Rollout plan"), "nothing names it: {text}");
    edit(
        &d.world,
        other,
        json!({ "command": "replace", "path": path, "old": "today", "new": "tomorrow" }),
    )
    .await;
    assert_eq!(
        refs_of(&d.world, notes).await,
        stored,
        "the reference the words stand for is kept"
    );
    assert!(
        read(&d.world, d.agent, "docs/Notes.md")
            .await
            .contains("See [Rollout plan](<docs/Rollout plan.md#2>) tomorrow."),
        "and whoever may read it reads it"
    );
}

#[tokio::test]
async fn an_adjacent_edit_cannot_drop_or_misbind_two_hidden_references_with_the_same_shared_label()
{
    let d = desk().await;
    let _other = d
        .world
        .document_beside(d.world.topic, "Another plan", &["Different private work"])
        .await;
    let made=edit(&d.world,d.agent,json!({"command":"create","path":"docs/Twins.md","markdown":"See [x](<docs/Rollout plan.md>) and [x](<docs/Another plan.md>) today."})).await;
    let notes: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let before = contents(&d.world, notes).await;
    let refs = refs_of(&d.world, notes).await;
    assert_eq!(refs.len(), 2);
    assert_ne!(refs[0]["id"], refs[1]["id"]);
    let elsewhere = d.world.topic("Elsewhere").await;
    let reader = thread_in_topic(&d.world, elsewhere, "Reviewer").await;
    for permission in ["doc.view", "doc.edit"] {
        share(
            &d.world, "document", notes, "live", permission, "any", reader,
        )
        .await;
    }
    let path = format!("id:{notes}");
    let text = read(&d.world, reader, &path).await;
    assert_eq!(text.matches("x (no access)").count(), 2);
    assert!(!text.contains("Rollout plan") && !text.contains("Another plan"));
    let refused = call(
        &d.world,
        reader,
        OrchTool::EditDocument,
        json!({"command":"replace","path":path,"old":"today","new":"tomorrow"}),
    )
    .await;
    assert_eq!(refused.error.unwrap().code, "ambiguous");
    assert_eq!(
        contents(&d.world, notes).await,
        before,
        "both private identities and surrounding words stay intact"
    );
}

#[tokio::test]
async fn a_longer_private_label_that_contains_another_marker_keeps_its_own_identity() {
    let d = desk().await;
    let _other = d
        .world
        .document_beside(d.world.topic, "Another plan", &["Different private work"])
        .await;
    let made=edit(&d.world,d.agent,json!({"command":"create","path":"docs/Overlapping.md","markdown":"See [x](<docs/Rollout plan.md>) and [x (no access) revision](<docs/Another plan.md>) today."})).await;
    let notes: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let before = refs_of(&d.world, notes).await;
    assert_eq!(before.len(), 2);
    assert_ne!(before[0]["id"], before[1]["id"]);
    let elsewhere = d.world.topic("Elsewhere").await;
    let reader = thread_in_topic(&d.world, elsewhere, "Reviewer").await;
    for permission in ["doc.view", "doc.edit"] {
        share(
            &d.world, "document", notes, "live", permission, "any", reader,
        )
        .await;
    }
    let path = format!("id:{notes}");
    let text = read(&d.world, reader, &path).await;
    assert!(text.contains("x (no access) revision (no access)"));
    edit(
        &d.world,
        reader,
        json!({"command":"replace","path":path,"old":"today","new":"tomorrow"}),
    )
    .await;
    assert_eq!(
        refs_of(&d.world, notes).await,
        before,
        "the embedded shorter marker cannot replace the longer label's identity"
    );
}

#[tokio::test]
async fn a_chat_message_keeps_its_references_typed_and_each_reader_is_given_them_in_its_own_terms()
{
    let d = desk().await;
    let chat = chat_of(&d.world, d.world.topic).await;
    share(
        &d.world,
        "node",
        d.world.topic,
        "live",
        "chat.view",
        "any",
        d.agent,
    )
    .await;
    share(
        &d.world,
        "node",
        d.world.topic,
        "live",
        "chat.interact",
        "any",
        d.agent,
    )
    .await;
    let listing = ok(call(&d.world, d.agent, OrchTool::ListShared, json!({})).await);
    let chat_path = entry(&listing, "chat.md")["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let sent = ok(call(
        &d.world,
        d.agent,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "chat", "path": chat_path }, "text": "The plan is at [step two](<docs/Rollout plan.md#2>); `docs/Gone.md#1` is not." }),
    )
    .await);
    assert_eq!(sent["references"]["resolved"][0]["part"], 2);
    assert_eq!(sent["references"]["kept"][0]["text"], "docs/Gone.md#1");
    let stored: Value = sqlx::query_scalar("SELECT content FROM chat_messages WHERE chat_id = $1")
        .bind(chat)
        .fetch_one(&mut *d.world.pool.acquire().await.unwrap())
        .await
        .unwrap();
    let second = blocks_in_order(&d.world, d.plan).await[1];
    let text = stored[0]["text"].as_str().unwrap();
    assert!(
        text.contains(&format!("[step two](/documents/{}#block={second})", d.plan)),
        "kept typed, by its block: {text}"
    );
    let read_back = read(&d.world, d.agent, &chat_path).await;
    assert!(
        read_back.contains("[Rollout plan](<docs/Rollout plan.md#2>)"),
        "{read_back}"
    );
}

#[tokio::test]
async fn a_note_to_another_thread_is_written_in_the_receiving_agents_terms() {
    let d = desk().await;
    let other = d.world.thread("Validator").await;
    let sent = ok(call(
        &d.world,
        d.agent,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": format!("id:{other}") }, "text": "Check [this](<docs/Rollout plan.md#3>)." }),
    )
    .await);
    assert_eq!(sent["references"]["resolved"][0]["kind"], "document");
    let body: String = sqlx::query_scalar(
        "SELECT COALESCE(reference_body,body) AS body FROM announcements WHERE to_thread_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(other)
    .fetch_one(&mut *d.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    let block = blocks_in_order(&d.world, d.plan).await[2];
    assert!(
        body.contains(&format!(
            "Check [this](/documents/{}#block={block}).",
            d.plan
        )),
        "people keep the typed identity: {body}"
    );
    let pending: Value = sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id=$1")
        .bind(other)
        .fetch_one(&mut *d.world.pool.acquire().await.unwrap())
        .await
        .unwrap();
    let body = pending
        .as_array()
        .unwrap()
        .iter()
        .find(|note| note["kind"] == "announcement")
        .unwrap()["body"]
        .as_str()
        .unwrap();
    assert!(
        body.contains("Check [Rollout plan](<docs/Rollout plan.md#3>)."),
        "the receiving agent reads its own path: {body}"
    );
}

#[tokio::test]
async fn relative_absolute_and_copied_workspace_paths_keep_one_identity_and_missing_paths_stay_words()
 {
    use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
    use orch_link_contract::commands::{
        Command, CommandError, CommandKind, CommandResult, ErrorCode,
    };
    use std::sync::{Arc, Mutex};
    let d = desk().await;
    prepare_call_turn(&d.world, d.agent).await;
    let other = d.world.thread("Another environment").await;
    let other_environment = orch_types::new_id();
    sqlx::query("INSERT INTO environments (id,name,owner_id,org_id,home_node_id,workspace_root,placement,status,machine_id,template_id,generation,facts,config,granted_capabilities) SELECT $1,'Other',owner_id,org_id,home_node_id,workspace_root,placement,status,machine_id,template_id,generation,facts,config,granted_capabilities FROM environments WHERE id=$2")
        .bind(other_environment).bind(d.world.environment).execute(&mut *d.world.pool.acquire().await.unwrap()).await.unwrap();
    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
        .bind(other)
        .bind(other_environment)
        .execute(&mut *d.world.pool.acquire().await.unwrap())
        .await
        .unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut d.world.pool.acquire().await.unwrap(),
        &orch_db::queries::access::PageKey::new("environment", other_environment, "live"),
        "any",
        "access.own",
        ("user", Some(d.world.ada), None),
        None,
        d.world.clock.now(),
    )
    .await
    .unwrap();
    prepare_call_turn(&d.world, other).await;
    let (directory, root): (String, String) = sqlx::query_as("SELECT t.workspace_dir,e.workspace_root FROM threads t JOIN environments e ON e.id=t.environment_id WHERE t.node_id=$1")
        .bind(d.agent).fetch_one(&mut *d.world.pool.acquire().await.unwrap()).await.unwrap();
    let relative = if directory.is_empty() {
        "src/auth.rs".to_owned()
    } else {
        format!("{directory}/src/auth.rs")
    };
    let absolute = format!("{}/{relative}", root.trim_end_matches('/'));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let expected = relative.clone();
    let at = orch_types::Time::new(d.world.clock.now());
    d.world
        .machine
        .handler
        .on(CommandKind::FsStat, move |command| {
            let Command::FsStat(body) = command else {
                panic!("file stat");
            };
            recorded.lock().unwrap().push(body.path.clone());
            if body.path != expected {
                return Err(CommandError::new(ErrorCode::NotFound, "missing"));
            }
            Ok(CommandResult::FsStat(FileStat {
                entry: DirEntry {
                    name: "auth.rs".into(),
                    path: body.path.clone(),
                    kind: DirEntryKind::File,
                    size: 50,
                    mtime: at,
                    mode: 0o644,
                    link_target: None,
                    cached: false,
                },
                hash: None,
                media_type: "text/plain".into(),
                is_text: true,
            }))
        });
    let made = edit(&d.world, d.agent, json!({ "command":"create", "path":"docs/Files.md",
        "markdown":format!("`src/auth.rs:7` and [absolute]({absolute}#7), [copied](id:{}/{relative}#7), [missing](src/missing.rs).", d.world.environment)
    })).await;
    let id: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let refs = refs_of(&d.world, id).await;
    assert_eq!(refs.len(), 3);
    for (typed, label) in refs.iter().zip(["src/auth.rs:7", "absolute", "copied"]) {
        assert_eq!(
            typed,
            &json!({"type":"ref","kind":"environment_file","id":d.world.environment,"path":relative,"part":{"line":7},"label":label})
        );
    }
    let paths = seen.lock().unwrap().clone();
    assert_eq!(
        paths.iter().filter(|path| **path == relative).count(),
        1,
        "one stat for the normalized file: {paths:?}"
    );
    assert_eq!(made["references"]["kept"][0]["text"], "src/missing.rs");
    let written = read(&d.world, d.agent, "docs/Files.md").await;
    assert!(written.contains("[auth.rs](src/auth.rs#7)"), "{written}");
    assert!(written.contains("missing"), "{written}");
    let elsewhere = read(&d.world, other, "docs/Files.md").await;
    assert!(
        elsewhere.contains("src/auth.rs:7 (no access)"),
        "{elsewhere}"
    );
    assert!(
        !elsewhere.contains(&d.world.environment.to_string()),
        "{elsewhere}"
    );
}

#[tokio::test]
async fn an_attachment_by_path_or_identifier_is_the_same_reference_with_its_line() {
    let d = desk().await;
    let file = orch_types::new_id();
    let blob = orch_types::Hash::of(b"first\nsecond\n");
    let mut conn = d.world.pool.acquire().await.unwrap();
    orch_db::queries::storage::register_blob(&mut conn, &blob, 13, d.world.clock.now())
        .await
        .unwrap();
    orch_db::queries::documents::add_file(
        &mut conn,
        &orch_db::queries::documents::NewFile {
            id: file,
            document: d.plan,
            blob,
            name: "result.txt",
            media_type: "text/plain",
            size: 13,
            is_text: true,
            added_by: None,
            added_by_thread: Some(d.agent),
            at: d.world.clock.now(),
        },
    )
    .await
    .unwrap();
    drop(conn);
    let made = edit(&d.world, d.agent, json!({"command":"create","path":"docs/Files.md",
        "markdown":format!("[attachment](<docs/Rollout plan.files/result.txt#2>) and `id:{file}#2`.") })).await;
    let id = serde_json::from_value(made["id"].clone()).unwrap();
    let refs = refs_of(&d.world, id).await;
    assert_eq!(
        refs,
        vec![
            json!({"type":"ref","kind":"file","id":file,"part":{"line":2},"label":"attachment"}),
            json!({"type":"ref","kind":"file","id":file,"part":{"line":2},"label":format!("id:{file}#2")}),
        ]
    );
    assert_eq!(made["references"]["resolved"][0]["kind"], "file");
    let written = read(&d.world, d.agent, "docs/Files.md").await;
    assert!(
        written.contains("[result.txt](<docs/Rollout plan.files/result.txt#2>)"),
        "{written}"
    );
}

#[tokio::test]
async fn a_workspace_line_is_not_mistaken_for_a_shared_document_block_with_the_same_path() {
    use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
    use orch_link_contract::commands::{Command, CommandKind, CommandResult};
    let d = desk().await;
    prepare_call_turn(&d.world, d.agent).await;
    let (directory, root): (String, String) = sqlx::query_as("SELECT t.workspace_dir,e.workspace_root FROM threads t JOIN environments e ON e.id=t.environment_id WHERE t.node_id=$1")
        .bind(d.agent).fetch_one(&mut *d.world.pool.acquire().await.unwrap()).await.unwrap();
    let relative = format!("{directory}/docs/Rollout plan.md")
        .trim_start_matches('/')
        .to_owned();
    let expected = relative.clone();
    let at = orch_types::Time::new(d.world.clock.now());
    d.world
        .machine
        .handler
        .on(CommandKind::FsStat, move |command| {
            let Command::FsStat(body) = command else {
                panic!("a file stat");
            };
            assert_eq!(body.path, expected);
            Ok(CommandResult::FsStat(FileStat {
                entry: DirEntry {
                    name: "Rollout plan.md".into(),
                    path: body.path.clone(),
                    kind: DirEntryKind::File,
                    size: 50,
                    mtime: at,
                    mode: 0o644,
                    link_target: None,
                    cached: false,
                },
                hash: None,
                media_type: "text/plain".into(),
                is_text: true,
            }))
        });
    let made = edit(&d.world,d.agent,json!({"command":"create","path":"docs/Native files.md",
        "markdown":format!("`docs/Rollout plan.md:2` and [absolute](<{}/{relative}#2>).",root.trim_end_matches('/'))})).await;
    let document: Id = serde_json::from_value(made["id"].clone()).unwrap();
    assert_eq!(
        refs_of(&d.world, document).await,
        vec![
            json!({"type":"ref","kind":"environment_file","id":d.world.environment,"path":relative,"part":{"line":2},"label":"docs/Rollout plan.md:2"}),
            json!({"type":"ref","kind":"environment_file","id":d.world.environment,"path":relative,"part":{"line":2},"label":"absolute"}),
        ]
    );
}

#[tokio::test]
async fn an_announcement_keeps_typed_references_and_writes_each_agents_own_paths_in_both_delivery_states()
 {
    let d = desk().await;
    let same = d.world.thread("Receiver here").await;
    let elsewhere = d.world.topic("Elsewhere").await;
    let other = thread_in_topic(&d.world, elsewhere, "Receiver there").await;
    let hidden_topic = d.world.topic("No target access").await;
    let hidden = thread_in_topic(&d.world, hidden_topic, "Receiver without target").await;
    share(
        &d.world, "document", d.plan, "live", "doc.view", "any", other,
    )
    .await;
    d.world
        .set(d.world.topic, "broadcast.agent_scope", json!("project"))
        .await;
    prepare_call_turn(&d.world, same).await;
    let sent = ok(call(&d.world, d.agent, OrchTool::Announce, json!({
        "audience":"project", "title":"Ready", "body":"Read [the plan](<docs/Rollout plan.md#2>) and keep [these words](docs/Gone.md#1)."
    })).await);
    assert_eq!(sent["references"]["resolved"][0]["kind"], "document");
    assert_eq!(sent["references"]["resolved"][0]["part"], 2);
    assert_eq!(sent["references"]["kept"][0]["text"], "docs/Gone.md#1");
    let bodies: Vec<String> = sqlx::query_scalar(
        "SELECT COALESCE(reference_body,body) AS body FROM announcements WHERE from_thread_id=$1",
    )
    .bind(d.agent)
    .fetch_all(&mut *d.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    let block = blocks_in_order(&d.world, d.plan).await[1];
    assert!(!bodies.is_empty());
    for body in bodies {
        assert!(
            body.contains(&format!("/documents/{}#block={block}", d.plan)),
            "{body}"
        );
        assert!(
            !body.contains("(docs/Gone.md#1)"),
            "a kept link is words, not another receiver's path: {body}"
        );
    }
    let input: Value = sqlx::query_scalar("SELECT content FROM thread_inputs WHERE thread_id=$1 AND source='announcement' ORDER BY created_at DESC LIMIT 1")
        .bind(same).fetch_one(&mut *d.world.pool.acquire().await.unwrap()).await.unwrap();
    let theirs = input.to_string();
    assert!(
        theirs.contains("[Rollout plan](<docs/Rollout plan.md#2>)"),
        "{theirs}"
    );
    async fn notice(world: &common::World, thread: Id) -> String {
        let notes: Value =
            sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id=$1")
                .bind(thread)
                .fetch_one(&mut *world.pool.acquire().await.unwrap())
                .await
                .unwrap();
        notes
            .as_array()
            .unwrap()
            .iter()
            .find(|note| note["kind"] == "announcement")
            .unwrap()["body"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    let there = notice(&d.world, other).await;
    let references = orch_documents::references::find(&there);
    let target = references
        .iter()
        .find(|found| found.label == "Rollout plan")
        .unwrap();
    let written = orch_documents::references::parse(&target.text).unwrap();
    assert!(
        written.path.starts_with('/'),
        "another topic reads its absolute shared path: {there}"
    );
    assert_eq!(written.part, Some(2));
    let followed = read(&d.world, other, &written.path).await;
    assert!(followed.contains("Ship it behind the flag."), "{followed}");
    let denied = notice(&d.world, hidden).await;
    assert!(denied.contains("the plan (no access)"), "{denied}");
    assert!(!denied.contains("Rollout plan"), "{denied}");
    assert!(!denied.contains("/documents/"), "{denied}");
}

#[tokio::test]
async fn canonical_reference_overhead_does_not_reduce_the_authored_announcement_limit() {
    let d = desk().await;
    let link = "[the plan](<docs/Rollout plan.md#2>)";
    let limit = orch_engine::tools::definitions::BODY_MAX_CHARS;
    let body = format!("{}{}", "a".repeat(limit - link.chars().count()), link);
    let sent = call(
        &d.world,
        d.agent,
        OrchTool::Announce,
        json!({"title":"Ready","body":body}),
    )
    .await;
    ok(sent);
    let (authored, resolved): (String, Option<String>) =
        sqlx::query_as("SELECT body,reference_body FROM announcements WHERE from_thread_id=$1")
            .bind(d.agent)
            .fetch_one(d.world.pool.pool())
            .await
            .unwrap();
    assert_eq!(authored, body);
    let resolved = resolved.unwrap();
    assert!(resolved.chars().count() > limit);
    assert!(resolved.contains(&format!("/documents/{}#block=", d.plan)));
    let id: Id = d
        .world
        .scalar(
            "SELECT id FROM announcements WHERE from_thread_id=$1",
            d.agent,
        )
        .await;
    let saved = orch_db::queries::threads::tools::announcement(
        &mut d.world.pool.acquire().await.unwrap(),
        id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        saved.body, resolved,
        "ordinary readers get the canonical form"
    );
    let refused = call(
        &d.world,
        d.agent,
        OrchTool::Announce,
        json!({"title":"Too long","body":format!("{body}a")}),
    )
    .await;
    assert!(
        refused.error.is_some(),
        "the original input limit still holds"
    );
    assert_eq!(
        d.world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                d.agent
            )
            .await,
        1
    );
}

#[tokio::test]
async fn person_snapshots_and_agent_exports_write_references_for_each_receiving_agent() {
    use orch_api_contract::share::{
        ShareTo, ShareToThread, ShareWhat, ShareWhatDocument, ShareWithAgents,
    };
    use orch_api_contract::threads::ShareMode;
    use orch_authz::Caller;
    use orch_db::audit::Origin;
    use std::sync::Arc;
    let d = desk().await;
    let elsewhere = d.world.topic("Elsewhere").await;
    let other = thread_in_topic(&d.world, elsewhere, "Copy reader").await;
    let hidden_place = d.world.topic("Hidden target").await;
    let hidden = thread_in_topic(&d.world, hidden_place, "Copy without target").await;
    share(
        &d.world, "document", d.plan, "live", "doc.view", "any", other,
    )
    .await;
    let made = edit(&d.world,d.agent,json!({"command":"create","path":"docs/Notes.md","markdown":"Read [this](<docs/Rollout plan.md#2>)."})).await;
    let notes: Id = serde_json::from_value(made["id"].clone()).unwrap();
    sqlx::query("UPDATE document_blocks SET content=content || $2::jsonb WHERE document_id=$1 AND removed_at IS NULL")
        .bind(notes)
        .bind(json!([
            {"type":"text","text":" Legacy: "},
            {"type":"ref","kind":"document","id":d.plan}
        ]))
        .execute(d.world.pool.pool())
        .await
        .unwrap();
    let parcels = Arc::new(MemoryParcels::default());
    d.world.engine().attach_parcels(parcels.clone());
    let records = d
        .world
        .engine()
        .share_with_agents(
            &Caller::session(d.world.ada),
            &Origin::default(),
            &ShareWithAgents {
                read_first: None,
                download: None,
                what: ShareWhat::Document(ShareWhatDocument { document_id: notes }),
                inside: None,
                parts: None,
                mode: ShareMode::Snapshot,
                to: ShareTo::Threads(vec![
                    ShareToThread { thread_id: other },
                    ShareToThread { thread_id: hidden },
                ]),
                comment: None,
                expires_at: None,
                client_id: "reference-snapshot".into(),
            },
        )
        .await
        .unwrap();
    let kept = parcels.kept.lock().unwrap().clone();
    for record in &records {
        let (_, _, files) = kept.iter().find(|(id, _, _)| *id == record.id).unwrap();
        let saved = record.copy.as_ref().unwrap();
        assert_eq!(
            (saved.file_count, saved.total_bytes),
            (i32::try_from(files.files.len()).unwrap(), files.bytes()),
            "snapshot metadata counts the recipient's actual bytes"
        );
    }
    let text_for = |thread| {
        let record = records.iter().find(|record| record.to == thread).unwrap();
        let (_, _, files) = kept.iter().find(|(id, _, _)| *id == record.id).unwrap();
        String::from_utf8(
            files
                .files
                .iter()
                .find(|file| file.path == "Notes.md")
                .unwrap()
                .bytes
                .clone(),
        )
        .unwrap()
    };
    let theirs = text_for(other);
    let found = orch_documents::references::find(&theirs);
    let target = found
        .iter()
        .find(|found| found.label == "Rollout plan")
        .unwrap();
    let written = orch_documents::references::parse(&target.text).unwrap();
    assert!(written.path.starts_with('/'), "{theirs}");
    assert_eq!(written.part, Some(2));
    assert!(
        read(&d.world, other, &written.path)
            .await
            .contains("Ship it behind the flag.")
    );
    let unavailable = text_for(hidden);
    assert!(unavailable.contains("this (no access)"), "{unavailable}");
    assert!(
        unavailable.contains("a document (no access)"),
        "{unavailable}"
    );
    assert!(!unavailable.contains("Rollout plan"), "{unavailable}");
    for permission in ["doc.view", "doc.export"] {
        share(
            &d.world, "document", notes, "live", permission, "any", other,
        )
        .await;
    }
    let exported = ok(call(
        &d.world,
        other,
        OrchTool::DownloadShared,
        json!({"path":format!("id:{notes}")}),
    )
    .await);
    let path = exported["path"].as_str().unwrap();
    let downloaded = parcels
        .files
        .lock()
        .unwrap()
        .get(&(other, path.to_owned()))
        .unwrap()
        .clone();
    let downloaded = String::from_utf8(downloaded).unwrap();
    assert!(
        downloaded.contains(&orch_documents::references::destination(
            &written.path,
            Some(2)
        )),
        "{downloaded}"
    );
    assert!(!downloaded.contains("/documents/"), "{downloaded}");
}

#[tokio::test]
async fn a_persons_copied_file_and_document_references_reach_the_native_agent_in_its_own_terms() {
    use orch_authz::Caller;
    let world = common::World::new().await;
    let thread = world.thread("Copied references").await;
    let plan = world
        .document_beside(world.topic, "Rollout plan", &["Read this opening."])
        .await;
    let foreign = orch_types::new_id();
    let workspace_dir: String = world
        .scalar("SELECT workspace_dir FROM threads WHERE node_id=$1", thread)
        .await;
    let path = if workspace_dir.is_empty() {
        "src/auth.rs".to_owned()
    } else {
        format!("{workspace_dir}/src/auth.rs")
    };
    let prefix = if workspace_dir.is_empty() {
        String::new()
    } else {
        format!("{workspace_dir}/")
    };
    let orch_path = format!("{prefix}orch files/settings.rs");
    let relayed_path = format!("{prefix}relayed files/run.rs");
    let root: String = world
        .scalar(
            "SELECT workspace_root FROM environments WHERE id=$1",
            world.environment,
        )
        .await;
    let text = format!(
        "Read [auth.rs:42](id:{}/{path}#42), [settings](<id:{}/{orch_path}#1>), [run](<id:{}/{relayed_path}#3>), [the plan](id:{plan}#1) and [a foreign file](id:{foreign}/src/private.rs#3).",
        world.environment, world.environment, world.environment
    );
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, common::queued(&text))
        .await
        .unwrap();
    let native = world.turn_start(thread, 0).await;
    let delivered = common::words_after_where(&native.text);
    assert!(
        delivered.contains("[auth.rs](src/auth.rs#42)"),
        "{}",
        native.text
    );
    assert!(
        delivered.contains("[Rollout plan](<docs/Rollout plan.md#1>)"),
        "{}",
        native.text
    );
    for (name, path, line) in [("settings.rs", &orch_path, 1), ("run.rs", &relayed_path, 3)] {
        let absolute = orch_engine::sessions::absolute_directory(&root, path);
        assert!(
            delivered.contains(&orch_documents::references::agent_link(
                name,
                &absolute,
                Some(line)
            )),
            "{}",
            native.text
        );
    }
    assert!(
        delivered.contains("a foreign file (no access)"),
        "{}",
        native.text
    );
    assert!(!delivered.contains(&foreign.to_string()), "{}", native.text);
    assert!(!delivered.contains("id:"), "{}", native.text);
    let saved: Value = world
        .scalar(
            "SELECT content FROM thread_inputs WHERE id=$1",
            submitted.input.id,
        )
        .await;
    assert_eq!(saved[0]["text"], text, "the authored input remains intact");
}
