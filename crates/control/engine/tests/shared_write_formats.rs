//! What an agent writes as a file and as Orch Markdown: identities kept when a document is written again, block commands and batches, pictures placed, a file of the workspace imported with the files it names, a checked-out file committed back, and a directory imported into folders.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use bytes::Bytes;
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_blob_contract::store::{BlobStore, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, FsFetchResult, FsListResult,
    OrchTool, OrchToolResult,
};
use orch_types::{Hash, Id, Time};
use serde_json::{Value, json};
use shared::*;
use std::collections::HashMap;
use std::sync::Arc;

struct Docs {
    world: common::World,
    agent: Id,
    topic: Id,
}

/// An agent that may make and change the documents of the topic `Docs`.
async fn with_documents() -> Docs {
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Writer").await;
    let topic = world.topic("Docs").await;
    share(&world, "node", topic, "live", "doc.view", "any", agent).await;
    share(&world, "node", topic, "live", "doc.edit", "any", agent).await;
    Docs {
        world,
        agent,
        topic,
    }
}

async fn edit(docs: &Docs, arguments: Value) -> OrchToolResult {
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
    entry(&listing, "docs")["path"].as_str().unwrap().to_owned()
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

/// The text of a document as the agent reads it, without the line that says who changed it.
async fn text_of(docs: &Docs, path: &str) -> String {
    let read = read_doc(docs, path).await;
    let text = read["text"].as_str().unwrap();
    text.split_once("\n\n")
        .map_or("", |(_, rest)| rest)
        .trim_end()
        .to_owned()
}

async fn seq_of(docs: &Docs, path: &str) -> i64 {
    read_doc(docs, path).await["entry"]["seq"].as_i64().unwrap()
}

/// A document made by the agent, with the ids of its top-level blocks by their words.
async fn made(docs: &Docs, name: &str, markdown: &str) -> (String, Id, HashMap<String, Id>) {
    let path = format!("{}{name}.md", docs_dir(docs).await);
    let answer = ok(edit(
        docs,
        json!({ "command": "create", "path": path, "markdown": markdown }),
    )
    .await);
    let id: Id = serde_json::from_value(answer["id"].clone()).unwrap();
    let ids = docs
        .world
        .blocks_of(id)
        .await
        .into_iter()
        .map(|(id, text)| (text, id))
        .collect();
    (path, id, ids)
}

// ---------------------------------------------------------------- identities are kept

#[tokio::test]
async fn a_rewrite_keeps_every_block_the_text_leaves_as_it_is_and_the_one_whose_words_changed() {
    let d = with_documents().await;
    let (path, id, before) = made(
        &d,
        "Plan",
        "# Plan\n\nFirst we build.\n\nThen we ship.\n\nLast, rest.\n",
    )
    .await;
    // Something Markdown has no form for: a colour on the first paragraph.
    sqlx::query("UPDATE document_blocks SET props = '{\"background\": 3}'::jsonb WHERE id = $1")
        .bind(before["First we build."])
        .execute(&mut *d.world.pool.acquire().await.unwrap())
        .await
        .unwrap();
    let seq = seq_of(&d, &path).await;
    let answer = ok(edit(
        &d,
        json!({ "command": "rewrite", "path": path, "seq": seq,
            "markdown": "# Plan\n\nFirst we build.\n\nThen we test and ship.\n\nA new one.\n\nLast, rest.\n" }),
    )
    .await);
    assert_eq!(
        answer["merged"],
        json!({ "kept": 3, "changed": 1, "removed": 0, "inserted": 1 })
    );
    let after: HashMap<String, Id> = d
        .world
        .blocks_of(id)
        .await
        .into_iter()
        .map(|(id, text)| (text, id))
        .collect();
    for kept in ["Plan", "First we build.", "Last, rest."] {
        assert_eq!(after[kept], before[kept], "{kept} is the block it was");
    }
    assert_eq!(
        after["Then we test and ship."], before["Then we ship."],
        "a block whose words changed is the same block"
    );
    assert!(!after.contains_key("Then we ship."));
    let background: Option<i64> = sqlx::query_scalar(
        "SELECT (props->>'background')::bigint FROM document_blocks WHERE id = $1",
    )
    .bind(before["First we build."])
    .fetch_one(&mut *d.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    assert_eq!(background, Some(3), "what Markdown cannot say is kept");
    assert_eq!(
        text_of(&d, &path).await,
        "# Plan\n\n<!-- orch:block {\"kind\":\"paragraph\",\"props\":{\"background\":3}} -->\n\nFirst we build.\n\n<!-- /orch:block -->\n\nThen we test and ship.\n\nA new one.\n\nLast, rest."
    );
}

#[tokio::test]
async fn a_passage_replaced_keeps_the_blocks_it_does_not_change_and_the_block_it_changes() {
    let d = with_documents().await;
    let (path, id, before) = made(&d, "Notes", "Alpha one.\n\nBeta two.\n\nGamma three.\n").await;
    ok(edit(
        &d,
        json!({ "command": "replace", "path": path, "old": "Beta two.", "new": "Beta, now longer." }),
    )
    .await);
    let after: HashMap<String, Id> = d
        .world
        .blocks_of(id)
        .await
        .into_iter()
        .map(|(id, text)| (text, id))
        .collect();
    assert_eq!(after["Beta, now longer."], before["Beta two."]);
    assert_eq!(after["Alpha one."], before["Alpha one."]);
    assert_eq!(after["Gamma three."], before["Gamma three."]);
}

#[tokio::test]
async fn a_change_that_would_touch_a_block_that_changed_since_the_read_is_refused() {
    let d = with_documents().await;
    let (path, id, _) = made(&d, "Race", "One.\n\nTwo.\n").await;
    let seq = seq_of(&d, &path).await;
    d.world
        .change_block(id, "Two.", "Two, by somebody else.")
        .await;
    let refusal = edit(
        &d,
        json!({ "command": "rewrite", "path": path, "seq": seq, "markdown": "One.\n\nTwo, by me.\n" }),
    )
    .await;
    assert_eq!(refused(&refusal).0, "changed");
    // What the change leaves alone does not matter.
    ok(edit(
        &d,
        json!({ "command": "rewrite", "path": path, "seq": seq, "markdown": "One, changed.\n\nTwo, by somebody else.\n" }),
    )
    .await);
}

// ---------------------------------------------------------------- block commands

#[tokio::test]
async fn blocks_are_changed_by_their_addresses_and_a_batch_is_made_whole_or_not_at_all() {
    let d = with_documents().await;
    let (path, id, before) = made(&d, "Blocks", "One.\n\nTwo.\n\nThree.\n\n- a\n  - b\n").await;
    let seq = seq_of(&d, &path).await;
    ok(edit(
        &d,
        json!({ "command": "block_replace", "path": path, "block": "#2", "markdown": "Two, changed." }),
    )
    .await);
    ok(edit(
        &d,
        json!({ "command": "block_insert_after", "path": path, "block": "#1", "markdown": "Inserted." }),
    )
    .await);
    assert_eq!(
        text_of(&d, &path).await,
        "One.\n\nInserted.\n\nTwo, changed.\n\nThree.\n\n- a\n  - b"
    );
    ok(edit(
        &d,
        json!({ "command": "block_move_after", "path": path, "block": "#1", "after": "#3" }),
    )
    .await);
    ok(edit(
        &d,
        json!({ "command": "block_delete", "path": path, "block": "#5.1" }),
    )
    .await);
    assert_eq!(
        text_of(&d, &path).await,
        "Inserted.\n\nTwo, changed.\n\nOne.\n\nThree.\n\n- a"
    );
    let after: HashMap<String, Id> = d
        .world
        .blocks_of(id)
        .await
        .into_iter()
        .map(|(id, text)| (text, id))
        .collect();
    assert_eq!(
        after["Two, changed."], before["Two."],
        "a replaced block keeps its identity"
    );
    assert_eq!(
        after["One."], before["One."],
        "a moved block is the same block"
    );
    // A block that is not there is told, with how many there are.
    let refusal = edit(
        &d,
        json!({ "command": "block_delete", "path": path, "block": "#40" }),
    )
    .await;
    assert_eq!(refused(&refusal).0, "no_match");
    // A batch that cannot be made whole is not made at all.
    let seq_now = seq_of(&d, &path).await;
    let clash = edit(
        &d,
        json!({ "command": "batch", "path": path, "seq": seq_now, "edits": [
            { "command": "block_replace", "block": "#1", "markdown": "First." },
            { "command": "block_delete", "block": "#1" }
        ] }),
    )
    .await;
    assert_eq!(refused(&clash).0, "invalid");
    assert!(text_of(&d, &path).await.starts_with("Inserted."));
    let done = ok(edit(
        &d,
        json!({ "command": "batch", "path": path, "seq": seq_now, "edits": [
            { "command": "replace", "old": "Three.", "new": "Third." },
            { "command": "block_insert_after", "block": "#0", "markdown": "# Title" },
            { "command": "block_delete", "block": "#3" }
        ] }),
    )
    .await);
    assert_eq!(done["merged"]["inserted"], 1);
    assert_eq!(
        text_of(&d, &path).await,
        "# Title\n\nInserted.\n\nTwo, changed.\n\nThird.\n\n- a"
    );
    let _ = seq;
}

// ---------------------------------------------------------------- Orch Markdown through the tools

#[tokio::test]
async fn what_orch_markdown_says_is_what_the_agent_reads_back() {
    let d = with_documents().await;
    let markdown = "> [!WARNING]\n> Mind the gap\n\n<details>\n<summary>More</summary>\n\ninside\n\n</details>\n\n<columns>\n\n<column ratio=\"2\">\n\nleft\n\n</column>\n\n<column>\n\nright\n\n</column>\n\n</columns>\n\nWords in <span color=\"red\" background=\"amber\">colour</span> and <u>under</u>.\n\n<page-break/>\n\nAfter.";
    let (path, _, _) = made(&d, "Tags", markdown).await;
    assert_eq!(text_of(&d, &path).await, markdown);
}

// ---------------------------------------------------------------- files of the workspace

/// A workspace the machine answers for: files by their path from the thread's directory.
struct Workspace {
    _directory: tempfile::TempDir,
}

fn entry_of(path: &str, kind: DirEntryKind, size: i64, now: Time) -> DirEntry {
    DirEntry {
        name: path.rsplit('/').next().unwrap().to_owned(),
        path: path.to_owned(),
        kind,
        size,
        mtime: now,
        mode: 0o600,
        link_target: None,
        cached: false,
    }
}

impl Workspace {
    async fn install(docs: &Docs, files: &[(&str, &[u8])]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            LocalBlobStore::open(directory.path().join("content"))
                .await
                .unwrap(),
        );
        docs.world.engine().attach_blobs(store.clone());
        let now = Time::new(docs.world.clock.now());
        let mtime_ns = now.get().timestamp_nanos_opt().unwrap();
        let mut table: HashMap<String, (i64, Hash, Vec<u8>)> = HashMap::new();
        for (path, bytes) in files {
            let hash = Hash::of(bytes);
            let size = i64::try_from(bytes.len()).unwrap();
            store
                .put(
                    &hash,
                    Some(size),
                    stream_of(vec![Bytes::copy_from_slice(bytes)]),
                )
                .await
                .unwrap();
            table.insert(format!("web/{path}"), (size, hash, bytes.to_vec()));
        }
        // The directories, and what each lists.
        let mut listing: HashMap<String, Vec<DirEntry>> = HashMap::new();
        for path in table.keys() {
            let mut child = path.clone();
            while let Some((parent, _)) = child.rsplit_once('/') {
                let kind = if &child == path {
                    DirEntryKind::File
                } else {
                    DirEntryKind::Dir
                };
                let size = table.get(&child).map_or(0, |entry| entry.0);
                let entries = listing.entry(parent.to_owned()).or_default();
                if !entries.iter().any(|each| each.path == child) {
                    entries.push(entry_of(&child, kind, size, now));
                }
                child = parent.to_owned();
            }
        }
        let table = Arc::new(table);
        let listing = Arc::new(listing);
        let environment = docs.world.environment;
        let (stat_table, stat_listing) = (table.clone(), listing.clone());
        docs.world
            .machine
            .handler
            .on(CommandKind::FsStat, move |command| {
                let Command::FsStat(body) = command else {
                    panic!("a file stat");
                };
                assert_eq!(body.environment_id, environment);
                if let Some((size, hash, bytes)) = stat_table.get(&body.path) {
                    return Ok(CommandResult::FsStat(FileStat {
                        entry: entry_of(&body.path, DirEntryKind::File, *size, now),
                        hash: Some(*hash),
                        media_type: orch_documents::files::media_type_of(&body.path, None),
                        is_text: orch_documents::files::looks_like_text(bytes),
                    }));
                }
                if body.path == "web" || stat_listing.contains_key(&body.path) {
                    return Ok(CommandResult::FsStat(FileStat {
                        entry: entry_of(&body.path, DirEntryKind::Dir, 0, now),
                        hash: None,
                        media_type: "inode/directory".to_owned(),
                        is_text: false,
                    }));
                }
                Err(CommandError::new(ErrorCode::NotFound, "no such file"))
            });
        let fetch_table = table.clone();
        docs.world
            .machine
            .handler
            .on(CommandKind::FsFetch, move |command| {
                let Command::FsFetch(body) = command else {
                    panic!("a file fetch");
                };
                match fetch_table.get(&body.path) {
                    Some((size, hash, _)) => Ok(CommandResult::FsFetch(FsFetchResult {
                        hash: *hash,
                        size: *size,
                        mtime_ns,
                    })),
                    None => Err(CommandError::new(ErrorCode::NotFound, "no such file")),
                }
            });
        docs.world
            .machine
            .handler
            .on(CommandKind::FsList, move |command| {
                let Command::FsList(body) = command else {
                    panic!("a listing");
                };
                match listing.get(&body.path) {
                    Some(entries) => Ok(CommandResult::FsList(FsListResult {
                        entries: entries.clone(),
                        truncated: false,
                        watch_instance: None,
                        page: None,
                    })),
                    None => Err(CommandError::new(ErrorCode::NotFound, "no such directory")),
                }
            });
        Self {
            _directory: directory,
        }
    }
}

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[tokio::test]
async fn a_markdown_file_with_a_picture_beside_it_becomes_a_document_with_the_picture_as_a_file_and_a_block()
 {
    let d = with_documents().await;
    let _workspace = Workspace::install(
        &d,
        &[
            (
                "out/report.md",
                b"# Report\n\nResults below.\n\n![the chart](images/chart.png)\n\nAlso ![gone](images/gone.png) and [the data](data.csv).\n",
            ),
            ("out/images/chart.png", PNG),
            ("out/data.csv", b"a,b\n1,2\n"),
        ],
    )
    .await;
    let path = format!("{}Report.md", docs_dir(&d).await);
    let answer = ok(edit(
        &d,
        json!({ "command": "create", "path": path, "file": "out/report.md" }),
    )
    .await);
    assert_eq!(
        answer["files"], 1,
        "the picture; a link inside a line cannot be a block, so its file is not held"
    );
    assert_eq!(answer["missing"], json!(["images/gone.png"]));
    assert!(
        answer["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|each| each.as_str().unwrap().contains("images/gone.png"))
    );
    let id: Id = serde_json::from_value(answer["id"].clone()).unwrap();
    let files: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, media_type FROM document_files WHERE document_id = $1 ORDER BY name",
    )
    .bind(id)
    .fetch_all(&mut *d.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    assert_eq!(files, [("chart.png".to_owned(), "image/png".to_owned())]);
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT type FROM document_blocks WHERE document_id = $1 AND removed_at IS NULL ORDER BY position",
    )
    .bind(id)
    .fetch_all(&mut *d.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    assert!(kinds.contains(&"image".to_owned()), "{kinds:?}");
    let text = text_of(&d, &path).await;
    assert!(
        text.contains("![the chart](Report.files/chart.png)"),
        "{text}"
    );
    assert!(
        text.contains("gone"),
        "a picture that was not found keeps its words: {text}"
    );
}

#[tokio::test]
async fn a_checked_out_file_is_committed_back_with_its_comments_kept_and_a_file_of_another_document_is_refused()
 {
    let d = with_documents().await;
    let (path, id, before) = made(&d, "Spec", "Alpha.\n\nBeta.\n\nGamma.\n").await;
    let seq = seq_of(&d, &path).await;
    let body = text_of(&d, &path).await;
    let edited = body.replace("Beta.", "Beta, edited.");
    let checkout = orch_documents::checkout::write(
        &orch_documents::checkout::Checkout {
            document: id,
            seq,
            title: Some("Spec".to_owned()),
        },
        &edited,
    );
    let other = orch_documents::checkout::write(
        &orch_documents::checkout::Checkout {
            document: orch_types::new_id(),
            seq,
            title: None,
        },
        &edited,
    );
    let _workspace = Workspace::install(
        &d,
        &[
            ("work/Spec.md", checkout.as_bytes()),
            ("work/Other.md", other.as_bytes()),
        ],
    )
    .await;
    // The file carries the sequence it was written at, so none is given.
    let answer = ok(edit(
        &d,
        json!({ "command": "rewrite", "path": path, "file": "work/Spec.md" }),
    )
    .await);
    assert_eq!(
        answer["merged"],
        json!({ "kept": 2, "changed": 1, "removed": 0, "inserted": 0 })
    );
    let after: HashMap<String, Id> = d
        .world
        .blocks_of(id)
        .await
        .into_iter()
        .map(|(id, text)| (text, id))
        .collect();
    assert_eq!(after["Beta, edited."], before["Beta."]);
    let refusal = edit(
        &d,
        json!({ "command": "rewrite", "path": path, "file": "work/Other.md" }),
    )
    .await;
    assert_eq!(refused(&refusal).0, "invalid");
    assert!(refused(&refusal).1.contains("another document"));
}

#[tokio::test]
async fn attach_shows_the_file_in_a_block_unless_asked_not_to_and_a_text_places_a_file_the_document_holds()
 {
    let d = with_documents().await;
    let (path, id, _) = made(&d, "Gallery", "Intro.\n\nOutro.\n").await;
    let _workspace = Workspace::install(
        &d,
        &[("pic/a.png", PNG), ("pic/b.png", PNG), ("pic/c.png", PNG)],
    )
    .await;
    ok(edit(
        &d,
        json!({ "command": "attach", "path": path, "file": "pic/a.png", "caption": "Figure A" }),
    )
    .await);
    ok(edit(
        &d,
        json!({ "command": "attach", "path": path, "file": "pic/b.png", "place": false }),
    )
    .await);
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT type FROM document_blocks WHERE document_id = $1 AND removed_at IS NULL ORDER BY position",
    )
    .bind(id)
    .fetch_all(&mut *d.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    assert_eq!(kinds, ["paragraph", "paragraph", "image"]);
    // The file b.png is held and shown by nothing; a text of the agent shows it where it likes.
    let seq = seq_of(&d, &path).await;
    ok(edit(
        &d,
        json!({ "command": "insert", "path": path, "after": "Intro.", "seq": seq,
            "markdown": "![Figure B](<Gallery.files/b.png>)" }),
    )
    .await);
    let text = text_of(&d, &path).await;
    assert!(
        text.starts_with("Intro.\n\n![Figure B](Gallery.files/b.png)\n\nOutro."),
        "{text}"
    );
    assert!(text.contains("![Figure A](Gallery.files/a.png)"), "{text}");
}

#[tokio::test]
async fn a_document_reader_can_list_an_empty_shared_directory_but_cannot_create_in_it() {
    let d = with_documents().await;
    let readonly = d.world.topic("Empty shared documents").await;
    let grant = share(
        &d.world, "node", readonly, "live", "doc.view", "any", d.agent,
    )
    .await;
    let address = format!("id:{readonly}");
    let listing = ok(call(
        &d.world,
        d.agent,
        OrchTool::ListShared,
        json!({"path": address}),
    )
    .await);
    let directory = entry(&listing, "docs");
    assert_eq!(directory["readable"], true);
    assert_eq!(directory["writable"], false);
    assert_eq!(directory["children"], 0);
    let path = directory["path"].as_str().unwrap();
    let empty = ok(call(
        &d.world,
        d.agent,
        OrchTool::ListShared,
        json!({"path": path}),
    )
    .await);
    assert!(empty["entries"].as_array().unwrap().is_empty());
    let denied = edit(
        &d,
        json!({"command":"create","path":format!("{path}Denied.md"),"markdown":"Denied."}),
    )
    .await;
    assert_eq!(refused(&denied).0, "not_writable");
    sqlx::query("UPDATE permission_entries SET removed_at=now() WHERE id=$1")
        .bind(grant)
        .execute(d.world.pool.pool())
        .await
        .unwrap();
    let revoked = call(
        &d.world,
        d.agent,
        OrchTool::ListShared,
        json!({"path": address}),
    )
    .await;
    assert_eq!(refused(&revoked).0, "not_found");
    assert_eq!(
        d.world
            .count("SELECT count(*) FROM documents WHERE node_id=$1", readonly)
            .await,
        0
    );
}

#[tokio::test]
async fn a_directory_is_imported_into_document_folders_with_links_between_its_files_made_references()
 {
    let d = with_documents().await;
    let _workspace = Workspace::install(
        &d,
        &[
            (
                "site/Home.md",
                b"# Home\n\nGo to [the guide](guide/Guide.md) or [the logo](logo.png).\n",
            ),
            (
                "site/guide/Guide.md",
                b"# Guide\n\nBack to [home](../Home.md).\n\n![shot](shot.png)\n",
            ),
            ("site/guide/shot.png", PNG),
            ("site/logo.png", PNG),
            ("site/.hidden/Secret.md", b"# Secret\n"),
        ],
    )
    .await;
    let dir = docs_dir(&d).await;
    let answer = ok(edit(
        &d,
        json!({ "command": "import", "path": dir, "file": "site" }),
    )
    .await);
    assert_eq!(answer["status"], "done", "{answer}");
    assert_eq!(answer["created"], 2, "{answer}");
    assert_eq!(answer["failed"], 0, "{answer}");
    assert_eq!(answer["folders"], 1);
    // A reference is written for the reader by the name of what it names.
    let home = read_doc(&d, &format!("{dir}Home.md")).await;
    assert!(
        home["text"].as_str().unwrap().contains("[Guide]("),
        "{home}"
    );
    let guide = read_doc(&d, &format!("{dir}guide/Guide.md")).await;
    assert!(
        guide["text"].as_str().unwrap().contains("[Home]("),
        "{guide}"
    );
    assert!(
        guide["text"]
            .as_str()
            .unwrap()
            .contains("Guide.files/shot.png"),
        "{guide}"
    );
    // Asking again makes nothing twice.
    let again = ok(edit(
        &d,
        json!({ "command": "import", "path": dir, "file": "site" }),
    )
    .await);
    assert_eq!(again["created"], 0, "{again}");
    assert_eq!(again["existing"], 2, "{again}");
    let absent = edit(
        &d,
        json!({"command":"import","path":dir,"file":"site","cancel":true}),
    )
    .await;
    assert_eq!(
        refused(&absent).0,
        "not_found",
        "cancel must not start another import"
    );
    let resumed = ok(edit(
        &d,
        json!({"command":"import","path":dir,"file":"site","resume":true}),
    )
    .await);
    assert_eq!(resumed["created"], 0, "resume keeps committed documents");
    assert_eq!(resumed["existing"], 2);
    assert_eq!(
        resumed["total"], 2,
        "companion files do not inflate document progress"
    );
    assert_eq!(resumed["done"], 2);

    // Moving planning into the job must preserve permission refusals and release admission without reading files.
    let readonly = d.world.topic("Read only imports").await;
    share(
        &d.world, "node", readonly, "live", "doc.view", "any", d.agent,
    )
    .await;
    let listing = ok(call(
        &d.world,
        d.agent,
        OrchTool::ListShared,
        json!({"path":format!("id:{readonly}")}),
    )
    .await);
    let readonly_dir = entry(&listing, "docs")["path"].as_str().unwrap();
    let denied = edit(
        &d,
        json!({"command":"import","path":readonly_dir,"file":"site"}),
    )
    .await;
    assert_eq!(refused(&denied).0, "not_writable");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE node_id=$1")
        .bind(readonly)
        .fetch_one(d.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    let no_job = edit(
        &d,
        json!({"command":"import","path":readonly_dir,"file":"site","cancel":true}),
    )
    .await;
    assert_eq!(refused(&no_job).0, "not_found");
}
