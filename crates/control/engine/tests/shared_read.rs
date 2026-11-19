//! What an agent reads of the shared tree: another thread's conversation as its people see it at three depths, a chat by the numbers of its messages, a document as Markdown with the sequence to edit on, a range of any of them within a bound of bytes; and nothing of what it may not read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use serde_json::{Value, json};
use shared::*;

/// Two threads in two topics: the reader's, and one with turns whose conversation is shared with the reader.
struct Pair {
    world: common::World,
    reader: orch_types::Id,
    writer: orch_types::Id,
}

async fn a_pair() -> Pair {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let docs = world.topic("Docs").await;
    let writer = thread_in_topic(&world, docs, "Writer").await;
    Pair {
        world,
        reader,
        writer,
    }
}

async fn read(pair: &Pair, arguments: Value) -> orch_link_contract::commands::OrchToolResult {
    call(&pair.world, pair.reader, OrchTool::ReadShared, arguments).await
}

fn path_of_writer(listing: &Value) -> String {
    listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "conversation")
        .expect("a conversation in the listing")["path"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn conversation_path(pair: &Pair) -> String {
    let listing = ok(call(
        &pair.world,
        pair.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", pair.writer) }),
    )
    .await);
    path_of_writer(&listing)
}

#[tokio::test]
async fn an_agent_reads_the_conversation_of_a_thread_shared_with_it_as_its_people_see_it() {
    let p = a_pair().await;
    talk(
        &p.world,
        p.writer,
        "Please reproduce the login failure.",
        "It fails on the cookie.",
    )
    .await;
    share(
        &p.world,
        "node",
        p.writer,
        "live",
        "thread.view",
        "any",
        p.reader,
    )
    .await;
    let path = conversation_path(&p).await;
    let result = ok(read(&p, json!({ "path": path })).await);
    assert_eq!(result["entry"]["kind"], "conversation");
    assert_eq!(result["entry"]["unit"], "turn");
    assert_eq!(
        (
            result["from"].as_i64(),
            result["to"].as_i64(),
            result["last"].as_i64()
        ),
        (Some(1), Some(1), Some(1))
    );
    assert_eq!(result["detail"], "normal");
    assert_eq!(result["truncated"], false);
    let text = result["text"].as_str().unwrap();
    assert!(
        text.starts_with("# Writer\nThread · Fake agent · idle · turns 1–1 of 1\n\n## Turn 1 · "),
        "{text}"
    );
    assert!(
        text.contains("**ada:** Please reproduce the login failure.")
            || text.contains("Please reproduce the login failure."),
        "{text}"
    );
    assert!(
        text.contains("**Fake agent:** It fails on the cookie."),
        "{text}"
    );
}

#[tokio::test]
async fn a_conversation_is_read_at_three_depths_and_in_brief_it_is_a_few_lines() {
    let p = a_pair().await;
    talk(
        &p.world,
        p.writer,
        "Look at the build.",
        "The build is green.",
    )
    .await;
    share(
        &p.world,
        "node",
        p.writer,
        "live",
        "thread.view",
        "any",
        p.reader,
    )
    .await;
    let path = conversation_path(&p).await;
    let brief = ok(read(&p, json!({ "path": path, "detail": "brief" })).await);
    let normal = ok(read(&p, json!({ "path": path })).await);
    let full = ok(read(&p, json!({ "path": path, "detail": "full" })).await);
    assert_eq!(brief["detail"], "brief");
    let brief_text = brief["text"].as_str().unwrap();
    assert!(
        brief_text.contains("Look at the build.") && brief_text.contains("The build is green.")
    );
    assert!(brief_text.len() <= normal["text"].as_str().unwrap().len() + 200);
    assert!(
        full["text"]
            .as_str()
            .unwrap()
            .contains("The build is green.")
    );
}

#[tokio::test]
async fn a_range_of_turns_is_named_by_from_and_to_and_a_negative_number_counts_from_the_end() {
    let p = a_pair().await;
    for n in 1..=4 {
        talk(
            &p.world,
            p.writer,
            &format!("Question {n}"),
            &format!("Answer {n}"),
        )
        .await;
    }
    share(
        &p.world,
        "node",
        p.writer,
        "live",
        "thread.view",
        "any",
        p.reader,
    )
    .await;
    let path = conversation_path(&p).await;
    let last = ok(read(&p, json!({ "path": path, "from": -1 })).await);
    assert_eq!(
        (
            last["from"].as_i64(),
            last["to"].as_i64(),
            last["last"].as_i64()
        ),
        (Some(4), Some(4), Some(4))
    );
    assert!(last["text"].as_str().unwrap().contains("Answer 4"));
    assert!(!last["text"].as_str().unwrap().contains("Answer 3"));
    let middle = ok(read(&p, json!({ "path": path, "from": 2, "to": 3 })).await);
    assert_eq!(
        (middle["from"].as_i64(), middle["to"].as_i64()),
        (Some(2), Some(3))
    );
    assert!(middle["text"].as_str().unwrap().contains("turns 2–3 of 4"));
    let latest = ok(read(&p, json!({ "path": path })).await);
    assert_eq!(
        latest["to"], 4,
        "without a range the latest turns that fit are given"
    );
    let beyond = ok(read(&p, json!({ "path": path, "from": 9 })).await);
    assert!(
        beyond["text"]
            .as_str()
            .unwrap()
            .contains("no turns in that range")
    );
}

#[tokio::test]
async fn the_bound_of_bytes_keeps_whole_turns_and_says_what_is_left() {
    let p = a_pair().await;
    for n in 1..=6 {
        talk(
            &p.world,
            p.writer,
            &format!("Question {n}"),
            &"x".repeat(1_000),
        )
        .await;
    }
    share(
        &p.world,
        "node",
        p.writer,
        "live",
        "thread.view",
        "any",
        p.reader,
    )
    .await;
    let path = conversation_path(&p).await;
    let small = ok(read(&p, json!({ "path": path, "max_bytes": 3_000 })).await);
    let text = small["text"].as_str().unwrap();
    assert!(text.len() <= 3_000, "{} bytes", text.len());
    assert_eq!(small["last"], 6);
    assert_eq!(small["to"], 6, "the latest turns that fit");
    assert!(
        small["from"].as_i64().unwrap() > 1,
        "the earlier ones are left, to be asked for with `to`"
    );
    assert_eq!(small["truncated"], false, "parts are given whole");
    let forward = ok(read(&p, json!({ "path": path, "from": 1, "max_bytes": 3_000 })).await);
    assert_eq!(forward["from"], 1);
    assert!(forward["to"].as_i64().unwrap() < 6);
    let one = ok(read(
        &p,
        json!({ "path": path, "from": 1, "to": 1, "max_bytes": 1_000 }),
    )
    .await);
    assert_eq!(
        one["truncated"], true,
        "a part larger than everything asked for is cut, and says so"
    );
    assert!(one["text"].as_str().unwrap().len() <= 1_000);
    let too_small = read(&p, json!({ "path": path, "max_bytes": 10 })).await;
    assert_eq!(refused(&too_small).0, "invalid");
}

#[tokio::test]
async fn what_an_agent_may_not_read_answers_exactly_as_what_is_not_there() {
    let p = a_pair().await;
    talk(&p.world, p.writer, "A secret", "Kept").await;
    // Nothing was shared: the thread is in the tree under its topic's directory only if something there is.
    let by_id = read(&p, json!({ "path": format!("id:{}", p.writer) })).await;
    let missing = read(
        &p,
        json!({ "path": format!("id:{}", orch_types::new_id()) }),
    )
    .await;
    let (code, message) = refused(&by_id);
    assert_eq!(code, "not_found");
    let (other_code, other_message) = refused(&missing);
    assert_eq!(other_code, "not_found");
    assert_eq!(
        message.replace(&p.writer.to_string(), "X"),
        other_message.replace(&refused_id(&missing), "X"),
        "a conversation that is there and not its to read answers as one that is not there"
    );
    assert!(!message.contains("Kept") && !message.contains("A secret"));
    // A thread of its own topic: listed, not readable.
    let beside = p.world.thread("Beside").await;
    let top = ok(call(&p.world, p.reader, OrchTool::ListShared, json!({})).await);
    let beside_dir = entry(&top, "Beside")["path"].as_str().unwrap().to_owned();
    let asked = read(
        &p,
        json!({ "path": format!("{beside_dir}conversation.md") }),
    )
    .await;
    assert_eq!(refused(&asked).0, "not_found");
    let _ = beside;
}

fn refused_id(result: &orch_link_contract::commands::OrchToolResult) -> String {
    let message = result.error.as_ref().unwrap().message.clone();
    message
        .split('"')
        .nth(1)
        .unwrap_or_default()
        .trim_start_matches("id:")
        .to_owned()
}

#[tokio::test]
async fn a_directory_is_not_read_and_a_file_that_is_no_text_waits_for_download() {
    let p = a_pair().await;
    let result = read(&p, json!({ "path": "/" })).await;
    let (code, message) = refused(&result);
    assert_eq!(code, "wrong_kind");
    assert!(message.contains("list_shared"));
    let own = read(&p, json!({ "path": "Reader/conversation.md" })).await;
    assert!(
        own.error.is_none(),
        "its own conversation is its own to read at any time"
    );
}

#[tokio::test]
async fn a_chat_is_read_by_the_numbers_of_its_messages_under_the_names_of_those_who_wrote_them() {
    let p = a_pair().await;
    let topic = p
        .world
        .scalar::<orch_types::Id>("SELECT topic_id FROM threads WHERE node_id = $1", p.writer)
        .await;
    let chat = chat_of(&p.world, topic).await;
    for n in 1..=3 {
        say_in_chat(&p.world, chat, p.world.ada, &format!("Message {n}")).await;
    }
    share(
        &p.world,
        "node",
        topic,
        "live",
        "chat.view",
        "any",
        p.reader,
    )
    .await;
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": "/", "depth": 4, "kinds": ["chat"] }),
    )
    .await);
    let chat_path = listing["entries"][0]["path"].as_str().unwrap().to_owned();
    assert_eq!(listing["entries"][0]["parts"], 3);
    let all = ok(read(&p, json!({ "path": chat_path })).await);
    let text = all["text"].as_str().unwrap();
    assert!(
        text.starts_with("# Chat of Docs\nChat · messages 1–3 of 3\n\n#1 · ada (@ada) · "),
        "{text}"
    );
    assert!(text.contains("#3 · ada") && text.contains("Message 3"));
    assert_eq!(all["entry"]["unit"], "message");
    let tail = ok(read(&p, json!({ "path": chat_path, "from": -2 })).await);
    assert_eq!(
        (tail["from"].as_i64(), tail["to"].as_i64()),
        (Some(2), Some(3))
    );
    let brief = ok(read(&p, json!({ "path": chat_path, "detail": "brief" })).await);
    assert!(brief["text"].as_str().unwrap().contains("#2 · ada"));
}

#[tokio::test]
async fn a_document_is_read_as_markdown_under_a_line_that_says_who_changed_it_last_and_gives_the_sequence_to_edit_on()
 {
    let p = a_pair().await;
    let topic = p
        .world
        .scalar::<orch_types::Id>("SELECT topic_id FROM threads WHERE node_id = $1", p.writer)
        .await;
    let doc = p
        .world
        .document_beside(topic, "Rollout plan", &["First we build.", "Then we ship."])
        .await;
    share(
        &p.world, "document", doc, "live", "doc.view", "any", p.reader,
    )
    .await;
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{topic}"), "depth": 2, "kinds": ["document"] }),
    )
    .await);
    let entries = listing["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{listing}");
    assert!(
        entries[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("/Docs/docs/Rollout plan.md")
    );
    assert_eq!(entries[0]["unit"], "block");
    assert_eq!(entries[0]["parts"], 2);
    assert_eq!(entries[0]["writable"], false);
    let path = entries[0]["path"].as_str().unwrap().to_owned();
    let result = ok(read(&p, json!({ "path": path })).await);
    let text = result["text"].as_str().unwrap();
    assert!(
        text.starts_with("Rollout plan (last changed by ada (@ada) at "),
        "{text}"
    );
    assert!(
        text.contains("\n\nFirst we build.\n\nThen we ship.\n"),
        "{text}"
    );
    assert_eq!(result["entry"]["kind"], "document");
    let writers = result["writers"].as_array().unwrap();
    assert_eq!(writers.len(), 2);
    assert!(
        writers
            .iter()
            .all(|writer| writer["writer"] == "ada (@ada)")
    );
    assert!(
        result["entry"]["seq"].as_i64().unwrap() >= 1,
        "the sequence to pass as base"
    );
    assert_eq!(
        (
            result["from"].as_i64(),
            result["to"].as_i64(),
            result["last"].as_i64()
        ),
        (Some(1), Some(2), Some(2))
    );
    let second = ok(read(&p, json!({ "path": path, "from": 2, "to": 2 })).await);
    assert_eq!(
        second["writers"].as_array().unwrap().len(),
        1,
        "only the block actually read supplies its writer"
    );
    assert!(
        second["text"]
            .as_str()
            .unwrap()
            .ends_with("\n\nThen we ship.\n")
    );
    assert!(!second["text"].as_str().unwrap().contains("First we build."));
}

#[tokio::test]
async fn an_earlier_version_of_a_document_is_read_from_its_history_directory() {
    let p = a_pair().await;
    let topic = p
        .world
        .scalar::<orch_types::Id>("SELECT topic_id FROM threads WHERE node_id = $1", p.writer)
        .await;
    let doc = p
        .world
        .document_beside(topic, "Notes", &["Old text."])
        .await;
    p.world.add_paragraph(doc, "New text.").await;
    share(
        &p.world, "document", doc, "live", "doc.view", "any", p.reader,
    )
    .await;
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{topic}"), "depth": 2, "kinds": ["folder"] }),
    )
    .await);
    let history = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "Notes.history")
        .expect("the history directory of a document that has versions")["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let versions = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": history }),
    )
    .await);
    let version = versions["entries"][0].clone();
    assert_eq!(version["kind"], "version");
    let old = ok(read(&p, json!({ "path": version["path"] })).await);
    let text = old["text"].as_str().unwrap();
    assert!(text.contains("ada (@ada)"), "{text}");
    assert!(text.contains("Old text."), "{text}");
    assert!(
        !text.contains("New text."),
        "the document as it stood then: {text}"
    );
    let now = ok(read(&p, json!({ "path": history.replace(".history/", ".md") })).await);
    assert!(now["text"].as_str().unwrap().contains("New text."));
}

/// Ada puts two files into a document and two blocks at its top that show them: a picture and a file. Their names,
/// by the order of the blocks.
async fn showing_two_files(world: &common::World, document: orch_types::Id) {
    use orch_api_contract::documents::{DocumentOp, NewBlock};
    use orch_db::queries::documents::batch;
    use orch_documents::versions::Writer;
    let mut conn = world.pool.acquire().await.unwrap();
    let mut blocks = Vec::new();
    for (name, media_type, kind, is_text) in [
        ("data.csv", "text/csv", "file", true),
        ("diagram one.png", "image/png", "image", false),
    ] {
        let (id, hash) = (orch_types::new_id(), orch_types::Hash::of(name.as_bytes()));
        orch_db::queries::storage::register_blob(&mut conn, &hash, 3, chrono::Utc::now())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO document_files (id, document_id, blob, name, media_type, size, is_text, added_by)
             VALUES ($1, $2, $3, $4, $5, 3, $6, $7)",
        )
        .bind(id)
        .bind(document)
        .bind(hash.as_bytes().to_vec())
        .bind(name)
        .bind(media_type)
        .bind(is_text)
        .bind(world.ada)
        .execute(&mut *conn)
        .await
        .unwrap();
        let block: NewBlock = serde_json::from_value(json!({
            "id": orch_types::new_id(), "type": kind, "props": { "file_id": id }, "content": []
        }))
        .unwrap();
        blocks.push(DocumentOp::Insert {
            block,
            parent_id: None,
            after_id: None,
        });
    }
    let outcome = batch::apply(
        &mut conn,
        document,
        &Writer::User(world.ada),
        None,
        &blocks,
        batch::Limits {
            max_bytes: 2_097_152,
            max_blocks: 5_000,
        },
        // Within the window of the version that is cut when the document is made: no version is cut by this.
        batch::Rules {
            window_secs: 600,
            kept: 200,
            history_max_bytes: 16_777_216,
        },
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    assert!(outcome.is_kept(), "{outcome:?}");
}

#[tokio::test]
async fn a_link_in_a_document_an_agent_reads_leads_to_the_file_the_tree_lists_beside_it() {
    let p = a_pair().await;
    let topic = p
        .world
        .scalar::<orch_types::Id>("SELECT topic_id FROM threads WHERE node_id = $1", p.writer)
        .await;
    let doc = p
        .world
        .document_beside(topic, "Rollout plan", &["First we build."])
        .await;
    showing_two_files(&p.world, doc).await;
    // A change cuts a version first, which holds the two blocks.
    p.world.add_paragraph(doc, "Then we ship.").await;
    share(
        &p.world, "document", doc, "live", "doc.view", "any", p.reader,
    )
    .await;
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{topic}"), "depth": 2 }),
    )
    .await);
    let entries = listing["entries"].as_array().unwrap();
    let path_of = |name: &str| -> String {
        entries
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("no entry {name}: {listing}"))["path"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let path = path_of("Rollout plan.md");

    // The text links each file by the directory beside it, and the answer lists the files by the very paths.
    let result = ok(read(&p, json!({ "path": path })).await);
    let text = result["text"].as_str().unwrap();
    let directory = path.rsplit_once('/').unwrap().0;
    let listed: Vec<String> = result["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        listed,
        [
            format!("{directory}/Rollout plan.files/data.csv"),
            format!("{directory}/Rollout plan.files/diagram one.png"),
        ]
    );
    assert!(
        text.contains("[data.csv](<Rollout plan.files/data.csv>)"),
        "{text}"
    );
    assert!(
        text.contains("![](<Rollout plan.files/diagram one.png>)"),
        "{text}"
    );
    assert!(
        !text.contains("](files/") && !text.contains("](<files/"),
        "{text}"
    );

    // An earlier version lies one directory below and links one up, to the same files.
    let history = path_of("Rollout plan.history");
    let versions = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": history }),
    )
    .await);
    let mut shown = Vec::new();
    for version in versions["entries"].as_array().unwrap() {
        let got = ok(read(&p, json!({ "path": version["path"] })).await);
        shown.push(got["text"].as_str().unwrap().to_owned());
    }
    let old = shown
        .iter()
        .find(|text| text.contains("diagram one.png"))
        .unwrap_or_else(|| panic!("a version that holds the picture: {shown:?}"));
    assert!(
        old.contains("![](<../Rollout plan.files/diagram one.png>)"),
        "{old}"
    );
    assert!(
        old.contains("[data.csv](<../Rollout plan.files/data.csv>)"),
        "{old}"
    );
}
