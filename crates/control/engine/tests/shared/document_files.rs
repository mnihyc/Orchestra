//! A document receives a whole workspace file through the ordinary agent tool, under the calling thread.

use super::{Docs, docs_dir, edit, read_doc, with_documents};
use crate::shared::{Places, call_as, file_commands, ok, refused};
use bytes::Bytes;
use orch_api_contract::documents::{
    BlockProps, BlockType, DocumentFileChange, DocumentFileChangeKind, DocumentOp, FileProps,
    NewBlock,
};
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_blob_contract::store::{BlobStore, read_all, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::documents::batch;
use orch_db::queries::{access, documents, storage};
use orch_documents::versions::{Span, Writer};
use orch_engine::tools::CallContext;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, FsFetchResult, OrchTool,
    OrchToolResult,
};
use orch_settings::{Target, write};
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, Time, new_id};
use serde_json::json;
use std::sync::Arc;

struct FileDocument {
    docs: Docs,
    id: Id,
    path: String,
    store: Arc<LocalBlobStore>,
    bytes: Bytes,
    hash: Hash,
    _directory: tempfile::TempDir,
}

impl FileDocument {
    async fn new(size: usize) -> Self {
        let docs = with_documents(true).await;
        let id = docs
            .world
            .document_beside(docs.topic, "Results", &["Keep this text."])
            .await;
        let path = format!("{}Results.md", docs_dir(&docs).await);
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            LocalBlobStore::open(directory.path().join("content"))
                .await
                .unwrap(),
        );
        docs.world.engine().attach_blobs(store.clone());
        let bytes = Bytes::from(
            (0..size)
                .map(|index| u8::try_from(index % 256).unwrap())
                .collect::<Vec<_>>(),
        );
        let hash = Hash::of(&bytes);
        let this = Self {
            docs,
            id,
            path,
            store,
            bytes,
            hash,
            _directory: directory,
        };
        this.source(this.bytes.clone()).await;
        this
    }

    /// The connector acknowledges only bytes actually held by the store. Native connector tests
    /// separately exercise streaming and root containment; these calls test the document consumer.
    async fn source(&self, bytes: Bytes) -> Hash {
        let hash = Hash::of(&bytes);
        let size = i64::try_from(bytes.len()).unwrap();
        self.store
            .put(&hash, Some(size), stream_of(vec![bytes.clone()]))
            .await
            .unwrap();
        let is_text = orch_documents::files::looks_like_text(&bytes);
        let environment = self.docs.world.environment;
        let now = Time::new(self.docs.world.clock.now());
        let mtime_ns = now.get().timestamp_nanos_opt().unwrap();
        self.docs
            .world
            .machine
            .handler
            .on(CommandKind::FsStat, move |command| {
                let Command::FsStat(body) = command else {
                    panic!("a file stat");
                };
                assert_eq!(body.environment_id, environment);
                assert!(body.mount.is_none());
                let directory = body.path == "web";
                assert!(directory || body.path == "web/out/result.bin");
                Ok(CommandResult::FsStat(FileStat {
                    entry: DirEntry {
                        name: if directory { "web" } else { "result.bin" }.to_owned(),
                        path: body.path.clone(),
                        kind: if directory {
                            DirEntryKind::Dir
                        } else {
                            DirEntryKind::File
                        },
                        size: if directory { 0 } else { size },
                        mtime: now,
                        mode: 0o600,
                        link_target: None,
                        cached: false,
                    },
                    hash: if directory { None } else { Some(hash) },
                    media_type: "application/octet-stream".to_owned(),
                    is_text,
                }))
            });
        self.docs
            .world
            .machine
            .handler
            .on(CommandKind::FsFetch, move |command| {
                let Command::FsFetch(body) = command else {
                    panic!("a file fetch");
                };
                assert_eq!(body.environment_id, environment);
                assert_eq!(body.path, "web/out/result.bin");
                assert!(body.mount.is_none());
                assert!(body.max_bytes.is_some_and(|limit| limit >= size));
                Ok(CommandResult::FsFetch(FsFetchResult {
                    hash,
                    size,
                    mtime_ns,
                }))
            });
        hash
    }

    async fn attach(&self, name: Option<&str>, overwrite: bool) -> OrchToolResult {
        edit(
            &self.docs,
            json!({ "command": "attach", "path": self.path,
            "file": "out/result.bin", "name": name, "overwrite": overwrite }),
        )
        .await
    }

    async fn span(&self) -> Span {
        documents::get(&mut self.docs.world.pool.acquire().await.unwrap(), self.id)
            .await
            .unwrap()
            .unwrap()
            .span()
    }

    async fn pins(&self, hash: Hash) -> i32 {
        storage::blob(&mut self.docs.world.pool.acquire().await.unwrap(), &hash)
            .await
            .unwrap()
            .map_or(0, |row| row.pin_count)
    }

    async fn file_count(&self) -> i64 {
        self.docs
            .world
            .count(
                "SELECT count(*) FROM document_files WHERE document_id=$1",
                self.id,
            )
            .await
    }

    async fn seq(&self) -> i64 {
        self.docs
            .world
            .scalar("SELECT doc_seq FROM documents WHERE id=$1", self.id)
            .await
    }

    async fn limit(&self, key: &str, bytes: i64) {
        let mut conn = self.docs.world.pool.acquire().await.unwrap();
        write::write(
            &mut conn,
            &write::Write {
                actor: Actor::User(self.docs.world.ada),
                target: Target::Node(self.docs.world.root),
                key,
                value: json!(bytes),
                locked: None,
                expected_revision: None,
                may_lock: true,
                origin: &Origin::default(),
            },
            self.docs.world.clock.now(),
        )
        .await
        .unwrap();
    }

    async fn revoke_write(&self) {
        let mut conn = self.docs.world.pool.acquire().await.unwrap();
        assert!(
            access::remove_entry(
                &mut conn,
                self.docs.write.unwrap(),
                self.docs.world.ada,
                self.docs.world.clock.now()
            )
            .await
            .unwrap()
        );
        access::bump_epoch(&mut conn).await.unwrap();
    }
}

#[tokio::test]
async fn a_workspace_file_arrives_whole_and_is_named_in_history_under_its_thread() {
    for size in [0, orch_api_contract::common::UPLOAD_CHUNK_BYTES + 1] {
        let f = FileDocument::new(size).await;
        let before = f.seq().await;
        let answer = ok(f.attach(None, false).await);
        assert_eq!(answer["files"], 1);
        assert!(answer["seq"].as_i64().unwrap() > before);
        let held = documents::file_named(
            &mut f.docs.world.pool.acquire().await.unwrap(),
            f.id,
            "result.bin",
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(held.blob, f.hash);
        assert_eq!(held.file.size, i64::try_from(size).unwrap());
        assert_eq!(held.file.media_type, "application/octet-stream");
        assert_eq!(
            held.file.is_text,
            orch_documents::files::looks_like_text(&f.bytes)
        );
        assert_eq!(
            (held.file.added_by, held.file.added_by_thread),
            (None, Some(f.docs.agent))
        );
        assert_eq!(
            read_all(f.store.get(&f.hash, None).await.unwrap().body)
                .await
                .unwrap(),
            f.bytes
        );
        assert_eq!(f.pins(f.hash).await, 1);
        let span = f.span().await;
        assert_eq!(
            span.files,
            [DocumentFileChange {
                name: "result.bin".to_owned(),
                kind: DocumentFileChangeKind::Added
            }]
        );
        assert!(span.authors.contains(&Writer::Agent {
            thread: f.docs.agent,
            agent: "Fake agent".to_owned()
        }));
        let read = read_doc(&f.docs, &f.path).await;
        assert_eq!(read["files"][0]["name"], "result.bin");
        assert_eq!(read["files"][0]["size"], json!(size));
        assert!(read["text"].as_str().unwrap().contains("Keep this text."));
        assert_eq!(file_commands(&f.docs.world, "fs.fetch"), 1);
    }
}

#[tokio::test]
async fn an_outside_path_or_native_root_refusal_changes_no_file_text_or_history() {
    let f = FileDocument::new(64).await;
    let before = f.seq().await;
    let span = f.span().await;
    for path in [
        "/private/result.bin",
        "../result.bin",
        "out/../../result.bin",
        "C:\\result.bin",
        "out\\..\\result.bin",
    ] {
        let answer = edit(
            &f.docs,
            json!({ "command": "attach", "path": f.path, "file": path }),
        )
        .await;
        assert_eq!(refused(&answer).0, "invalid");
        assert!(refused(&answer).1.contains("relative path"));
    }
    assert_eq!(file_commands(&f.docs.world, "fs.stat"), 0);
    assert_eq!(file_commands(&f.docs.world, "fs.fetch"), 0);
    f.docs.world.machine.handler.on(CommandKind::FsFetch, |_| {
        Err(CommandError::new(
            ErrorCode::PolicyRootNotAllowed,
            "/private/outside-root: private_native_constraint",
        ))
    });
    let answer = f.attach(None, false).await;
    assert_eq!(refused(&answer).0, "not_found");
    assert!(refused(&answer).1.contains("inside the environment"));
    assert!(!refused(&answer).1.contains("outside-root"));
    assert!(!refused(&answer).1.contains("private_native_constraint"));
    assert_eq!(f.file_count().await, 0);
    assert_eq!(f.seq().await, before);
    assert_eq!(f.span().await, span);
    assert_eq!(f.pins(f.hash).await, 0);
    assert!(
        read_doc(&f.docs, &f.path).await["text"]
            .as_str()
            .unwrap()
            .contains("Keep this text.")
    );
}

#[tokio::test]
async fn an_away_environment_refuses_at_once_without_a_file_command() {
    let f = FileDocument::new(64).await;
    f.docs
        .world
        .set_machine_online(f.docs.world.machine_id, false)
        .await;
    let answer = f.attach(None, false).await;
    assert_eq!(refused(&answer).0, "closed");
    assert!(refused(&answer).1.contains("reconnect its machine"));
    assert_eq!(file_commands(&f.docs.world, "fs.stat"), 0);
    assert_eq!(file_commands(&f.docs.world, "fs.fetch"), 0);
    assert_eq!(f.file_count().await, 0);
    assert_eq!(f.pins(f.hash).await, 0);
}

#[tokio::test]
async fn an_archived_document_and_an_invalid_stored_name_never_ask_for_source_bytes() {
    let f = FileDocument::new(64).await;
    for name in ["../result.bin", "a/b", "bad\nname", ".", ".."] {
        let answer = f.attach(Some(name), false).await;
        assert_eq!(refused(&answer).0, "invalid");
    }
    let before = f.seq().await;
    sqlx::query("UPDATE nodes SET archived_at=now() WHERE id=$1")
        .bind(f.docs.topic)
        .execute(f.docs.world.pool.pool())
        .await
        .unwrap();
    let answer = f.attach(None, false).await;
    assert_eq!(refused(&answer).0, "not_writable");
    assert_eq!(file_commands(&f.docs.world, "fs.stat"), 0);
    assert_eq!(file_commands(&f.docs.world, "fs.fetch"), 0);
    assert_eq!(f.file_count().await, 0);
    assert_eq!(f.seq().await, before);
}

#[tokio::test]
async fn replacing_a_named_file_preserves_its_identity_and_releases_the_previous_bytes() {
    let f = FileDocument::new(64).await;
    ok(f.attach(Some("review.bin"), false).await);
    let before = documents::file_named(
        &mut f.docs.world.pool.acquire().await.unwrap(),
        f.id,
        "review.bin",
    )
    .await
    .unwrap()
    .unwrap();
    let seq = f.seq().await;
    let conflict = f.attach(Some("review.bin"), false).await;
    assert_eq!(refused(&conflict).0, "exists");
    assert!(refused(&conflict).1.contains("overwrite: true"));
    assert_eq!(f.seq().await, seq);
    assert_eq!(
        file_commands(&f.docs.world, "fs.fetch"),
        1,
        "a known conflict reads no source bytes"
    );
    let replacement = Bytes::from_static(b"the replacement is complete\0\xff");
    let replacement_hash = f.source(replacement.clone()).await;
    ok(f.attach(Some("review.bin"), true).await);
    let after = documents::file_named(
        &mut f.docs.world.pool.acquire().await.unwrap(),
        f.id,
        "review.bin",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        after.file.id, before.file.id,
        "existing blocks keep their target"
    );
    assert_eq!(after.blob, replacement_hash);
    assert_eq!(f.file_count().await, 1);
    assert_eq!(f.pins(f.hash).await, 0);
    assert_eq!(f.pins(replacement_hash).await, 1);
    assert_eq!(
        read_all(f.store.get(&replacement_hash, None).await.unwrap().body)
            .await
            .unwrap(),
        replacement
    );
    assert_eq!(
        f.span().await.files,
        [
            DocumentFileChange {
                name: "review.bin".to_owned(),
                kind: DocumentFileChangeKind::Added
            },
            DocumentFileChange {
                name: "review.bin".to_owned(),
                kind: DocumentFileChangeKind::Replaced
            },
        ]
    );
    // Replacing a file by identical bytes still keeps exactly one durable reference.
    ok(f.attach(Some("review.bin"), true).await);
    assert_eq!(f.pins(replacement_hash).await, 1);
}

#[tokio::test]
async fn file_and_aggregate_limits_leave_the_document_whole_and_overwrite_returns_its_room() {
    let f = FileDocument::new(64).await;
    f.limit(keys::DOCS_FILE_MAX_BYTES, 63).await;
    let too_large = f.attach(None, false).await;
    assert_eq!(refused(&too_large).0, "invalid");
    assert!(refused(&too_large).1.contains("smaller file"));
    assert_eq!(file_commands(&f.docs.world, "fs.fetch"), 0);
    assert_eq!(f.file_count().await, 0);
    f.limit(keys::DOCS_FILE_MAX_BYTES, 64).await;
    f.limit(keys::DOCS_FILES_MAX_BYTES, 64).await;
    ok(f.attach(None, false).await);
    let fetches = f.docs.world.commands.issued_bodies("fs.fetch");
    assert_eq!(fetches.len(), 1, "the native capped read");
    assert_eq!(fetches[0].1["max_bytes"], json!(64));
    let seq = f.seq().await;
    let full = f.attach(Some("second.bin"), false).await;
    assert_eq!(refused(&full).0, "limit");
    assert!(refused(&full).1.contains(keys::DOCS_FILES_MAX_BYTES));
    assert_eq!(f.seq().await, seq);
    assert_eq!(f.file_count().await, 1);
    assert_eq!(f.pins(f.hash).await, 1);
    ok(f.attach(None, true).await);
    assert_eq!(f.file_count().await, 1);
    assert_eq!(f.pins(f.hash).await, 1);
}

#[tokio::test]
async fn attaching_requires_edit_before_any_source_read_and_rechecks_changes_during_it() {
    let f = FileDocument::new(64).await;
    f.revoke_write().await;
    let answer = f.attach(None, false).await;
    assert_eq!(refused(&answer).0, "not_writable");
    assert_eq!(file_commands(&f.docs.world, "fs.stat"), 0);
    assert_eq!(file_commands(&f.docs.world, "fs.fetch"), 0);
    for during in ["permission", "file_limit", "total_limit", "workspace"] {
        let f = FileDocument::new(64).await;
        let before = f.seq().await;
        let span = f.span().await;
        f.docs.world.machine.handler.hold(CommandKind::FsFetch);
        let sending = f.attach(None, false);
        tokio::pin!(sending);
        tokio::select! {
            result = &mut sending => panic!("the source read is still held: {result:?}"),
            () = f.docs.world.wait_began(CommandKind::FsFetch, 1) => {},
        }
        match during {
            "permission" => f.revoke_write().await,
            "file_limit" => f.limit(keys::DOCS_FILE_MAX_BYTES, 63).await,
            "total_limit" => f.limit(keys::DOCS_FILES_MAX_BYTES, 63).await,
            "workspace" => {
                sqlx::query("UPDATE threads SET workspace_dir='another' WHERE node_id=$1")
                    .bind(f.docs.agent)
                    .execute(f.docs.world.pool.pool())
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        f.docs.world.machine.handler.release();
        let answer = sending.await;
        let expected = match during {
            "permission" => "not_writable",
            "workspace" => "changed",
            _ => "limit",
        };
        assert_eq!(refused(&answer).0, expected, "{during}");
        assert_eq!(f.seq().await, before);
        assert_eq!(f.span().await, span);
        assert_eq!(f.file_count().await, 0);
        assert_eq!(f.pins(f.hash).await, 0);
    }
}

#[tokio::test]
async fn detaching_removes_showing_blocks_and_the_pin_and_names_the_file_in_thread_history() {
    let f = FileDocument::new(64).await;
    ok(f.attach(None, false).await);
    let file = documents::file_named(
        &mut f.docs.world.pool.acquire().await.unwrap(),
        f.id,
        "result.bin",
    )
    .await
    .unwrap()
    .unwrap();
    let attached_block: Id = sqlx::query_scalar(
        "SELECT id FROM document_blocks WHERE document_id=$1 AND removed_at IS NULL AND props->>'file_id'=$2",
    )
    .bind(f.id)
    .bind(file.file.id.to_string())
    .fetch_one(f.docs.world.pool.pool())
    .await
    .unwrap();
    let block = new_id();
    {
        let mut tx = f.docs.world.pool.begin().await.unwrap();
        let outcome = batch::apply(
            &mut tx,
            f.id,
            &Writer::User(f.docs.world.ada),
            None,
            &[DocumentOp::Insert {
                block: NewBlock {
                    id: block,
                    block_type: BlockType::File,
                    props: Some(BlockProps::File(FileProps {
                        display: None,
                        file_id: file.file.id,
                    })),
                    content: None,
                    children: None,
                },
                parent_id: None,
                after_id: None,
            }],
            batch::Limits {
                max_bytes: i64::MAX,
                max_blocks: i64::MAX,
            },
            batch::Rules {
                window_secs: 600,
                kept: 200,
                history_max_bytes: i64::MAX,
            },
            f.docs.world.clock.now(),
        )
        .await
        .unwrap();
        assert!(outcome.is_kept());
        tx.commit().await.unwrap();
    }
    let before = f.seq().await;
    ok(edit(
        &f.docs,
        json!({ "command": "detach", "path": f.path, "name": "result.bin" }),
    )
    .await);
    assert_eq!(f.file_count().await, 0);
    assert_eq!(f.pins(f.hash).await, 0);
    assert!(f.seq().await > before);
    let removed: bool =
        sqlx::query_scalar("SELECT removed_at IS NOT NULL FROM document_blocks WHERE id=$1")
            .bind(block)
            .fetch_one(f.docs.world.pool.pool())
            .await
            .unwrap();
    assert!(removed);
    let attached_removed: bool =
        sqlx::query_scalar("SELECT removed_at IS NOT NULL FROM document_blocks WHERE id=$1")
            .bind(attached_block)
            .fetch_one(f.docs.world.pool.pool())
            .await
            .unwrap();
    assert!(
        attached_removed,
        "the automatically shown attachment is removed too"
    );
    assert!(
        read_doc(&f.docs, &f.path).await["text"]
            .as_str()
            .unwrap()
            .contains("Keep this text.")
    );
    let span = f.span().await;
    assert!(span.files.contains(&DocumentFileChange {
        name: "result.bin".to_owned(),
        kind: DocumentFileChangeKind::Removed
    }));
    assert!(span.authors.contains(&Writer::Agent {
        thread: f.docs.agent,
        agent: "Fake agent".to_owned()
    }));
    let removed_count: i64 = sqlx::query_scalar("SELECT (details->'blocks'->>'removed')::bigint FROM audit_log WHERE object_id=$1 AND details->>'command'='detach'")
        .bind(f.id).fetch_one(f.docs.world.pool.pool()).await.unwrap();
    assert_eq!(
        removed_count, 2,
        "the audit counts both the attachment's own block and the additional showing block"
    );
    let seq = f.seq().await;
    let absent = edit(
        &f.docs,
        json!({ "command": "detach", "path": f.path, "name": "result.bin" }),
    )
    .await;
    assert_eq!(refused(&absent).0, "not_found");
    assert_eq!(f.seq().await, seq);
}

#[tokio::test]
async fn a_completed_attachment_replay_uses_its_saved_answer_without_fetching_or_pinning_again() {
    let f = FileDocument::new(64).await;
    let call_ref = new_id().to_string();
    let arguments = json!({ "command": "attach", "path": f.path, "file": "out/result.bin" });
    let first = ok(call_as(
        &f.docs.world,
        f.docs.agent,
        OrchTool::EditDocument,
        arguments.clone(),
        &call_ref,
    )
    .await);
    f.docs.world.machine.handler.on(CommandKind::FsFetch, |_| {
        panic!("the saved attachment must not reread its source")
    });
    f.docs
        .world
        .engine()
        .answer_tool_call(&CallContext {
            machine: f.docs.world.machine_id,
            thread: f.docs.agent,
            turn: new_id(),
            call_ref,
            attempt: Some(new_id()),
            tool: orch_agent_contract::events::OrchTool::EditDocument,
            arguments,
        })
        .await
        .unwrap();
    assert_eq!(f.seq().await, first["seq"].as_i64().unwrap());
    assert_eq!(f.file_count().await, 1);
    assert_eq!(f.pins(f.hash).await, 1);
    assert_eq!(file_commands(&f.docs.world, "fs.fetch"), 1);
    assert_eq!(f.span().await.files.len(), 1);
}

#[tokio::test]
async fn attached_files_are_discoverable_and_resolvable_under_the_document_folder_path_and_copy_keeps_independent_pins()
 {
    let mut f = FileDocument::new(64).await;
    crate::shared::share(
        &f.docs.world,
        "node",
        f.docs.topic,
        "live",
        "doc.manage",
        "any",
        f.docs.agent,
    )
    .await;
    let root = docs_dir(&f.docs).await;
    let folder = format!("{root}research/");
    ok(crate::shared::call(
        &f.docs.world,
        f.docs.agent,
        OrchTool::ManageShared,
        json!({"command":"create_folder","paths":[folder]}),
    )
    .await);
    ok(crate::shared::call(
        &f.docs.world,
        f.docs.agent,
        OrchTool::ManageShared,
        json!({"command":"move","paths":[f.path],"to":folder}),
    )
    .await);
    f.path = format!("{folder}Results.md");
    ok(f.attach(Some("report.bin"), false).await);
    let read = read_doc(&f.docs, &f.path).await;
    let attachment = format!("{folder}Results.files/report.bin");
    assert_eq!(read["files"][0]["path"], json!(attachment));
    let listing = ok(crate::shared::call(
        &f.docs.world,
        f.docs.agent,
        OrchTool::ListShared,
        json!({"path":folder}),
    )
    .await);
    let directory = crate::shared::entry(&listing, "Results.files");
    assert_eq!(directory["readable"], true);
    assert_ne!(directory["writable"], true);
    assert_ne!(
        directory["manageable"], true,
        "the files directory itself is never a mutable object"
    );
    let listing = ok(crate::shared::call(
        &f.docs.world,
        f.docs.agent,
        OrchTool::ListShared,
        json!({"path":format!("{folder}Results.files/"),"ids":true}),
    )
    .await);
    let file = crate::shared::entry(&listing, "report.bin");
    assert_eq!(file["path"], json!(attachment));
    assert_eq!(file["kind"], "file");
    assert_eq!(file["size"], 64);
    let source_file = documents::files(&mut f.docs.world.pool.acquire().await.unwrap(), f.id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(file["id"], json!(source_file.id));
    let resolved = crate::shared::call(
        &f.docs.world,
        f.docs.agent,
        OrchTool::ReadShared,
        json!({"path":attachment}),
    )
    .await;
    assert_eq!(
        refused(&resolved).0,
        "wrong_kind",
        "the attachment resolves as a file, not not_found"
    );
    assert!(refused(&resolved).1.contains("download_shared"));
    ok(crate::shared::call(
        &f.docs.world,
        f.docs.agent,
        OrchTool::ManageShared,
        json!({"command":"copy","paths":[folder],"to":root,"name":"research-copy"}),
    )
    .await);
    let copied = ok(crate::shared::call(
        &f.docs.world,
        f.docs.agent,
        OrchTool::ListShared,
        json!({"path":format!("{root}research-copy/"),"ids":true}),
    )
    .await);
    let copy: Id =
        serde_json::from_value(crate::shared::entry(&copied, "Results.md")["id"].clone()).unwrap();
    assert_ne!(copy, f.id);
    let held = documents::files(&mut f.docs.world.pool.acquire().await.unwrap(), copy)
        .await
        .unwrap()
        .remove(0);
    assert_ne!(held.id, source_file.id);
    assert_eq!(held.name, source_file.name);
    assert_eq!(
        f.pins(f.hash).await,
        2,
        "copy pins the same whole bytes independently"
    );
    assert_eq!(
        file_commands(&f.docs.world, "fs.fetch"),
        1,
        "Orch copy does not fetch the workspace file again"
    );
    let read = read_doc(&f.docs, &format!("{root}research-copy/Results.md")).await;
    assert_eq!(
        read["files"][0]["path"],
        json!(format!("{root}research-copy/Results.files/report.bin"))
    );
    // Detaching the live source makes its old advertised path unavailable, while the copy stays.
    ok(edit(
        &f.docs,
        json!({"command":"detach","path":f.path,"name":"report.bin"}),
    )
    .await);
    let gone = crate::shared::call(
        &f.docs.world,
        f.docs.agent,
        OrchTool::ReadShared,
        json!({"path":format!("{folder}Results.files/report.bin")}),
    )
    .await;
    assert_eq!(refused(&gone).0, "not_found");
    assert_eq!(
        documents::files(&mut f.docs.world.pool.acquire().await.unwrap(), copy)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(f.pins(f.hash).await, 1);
}
