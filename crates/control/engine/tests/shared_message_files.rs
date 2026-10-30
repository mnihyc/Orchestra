//! Workspace files sent by real agent tools become whole pinned chat attachments and thread deliveries, or leave the entire message unsent.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use bytes::Bytes;
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_blob_contract::store::{BlobStore, read_all, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::{access, storage};
use orch_engine::tools::CallContext;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, FsFetchResult, FsMaterialize,
    FsMaterializeResult, MaterializePurpose, OrchTool,
};
use orch_settings::{Target, write};
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;
use shared::{
    a_world_whose_agent_takes_tools, call, call_as, chat_of, file_commands, ok, prepare_call_turn,
    refused, share, thread_in_topic,
};
use std::sync::Arc;

#[derive(Clone, Copy)]
enum Destination {
    Chat,
    Thread,
}

impl Destination {
    fn tool(self) -> OrchTool {
        OrchTool::SendMessage
    }
}

struct Files {
    world: common::World,
    sender: Id,
    receiver: Id,
    chat: Id,
    chat_write: Id,
    thread_write: Id,
    store: Arc<LocalBlobStore>,
    bytes: Bytes,
    hash: Hash,
    mtime_ns: i64,
    _directory: tempfile::TempDir,
}

impl Files {
    async fn new(size: usize) -> Self {
        let world = a_world_whose_agent_takes_tools().await;
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            LocalBlobStore::open(directory.path().join("content"))
                .await
                .unwrap(),
        );
        world.engine().attach_blobs(store.clone());
        let sender = world.thread("Build").await;
        let topic = world.topic("Review").await;
        let receiver = thread_in_topic(&world, topic, "Validate").await;
        let chat = chat_of(&world, topic).await;
        share(&world, "node", topic, "live", "chat.view", "any", sender).await;
        let chat_write = share(
            &world,
            "node",
            topic,
            "live",
            "chat.interact",
            "any",
            sender,
        )
        .await;
        share(
            &world,
            "node",
            receiver,
            "live",
            "thread.view",
            "any",
            sender,
        )
        .await;
        let thread_write = share(
            &world,
            "node",
            receiver,
            "live",
            "thread.interact",
            "any",
            sender,
        )
        .await;
        let bytes = Bytes::from(
            (0..size)
                .map(|index| u8::try_from(index % 256).unwrap())
                .collect::<Vec<_>>(),
        );
        let hash = Hash::of(&bytes);
        // The scripted connector's upload acknowledgement refers to actual bytes held by the store.
        // The connector's own tests cover streaming them there and enforcing real root containment.
        store
            .put(
                &hash,
                Some(i64::try_from(size).unwrap()),
                stream_of(vec![bytes.clone()]),
            )
            .await
            .unwrap();
        let size = i64::try_from(size).unwrap();
        let environment = world.environment;
        let now = Time::new(world.clock.now());
        let mtime_ns = now.get().timestamp_nanos_opt().unwrap();
        world
            .machine
            .handler
            .on(CommandKind::FsStat, move |command| {
                let Command::FsStat(body) = command else {
                    panic!("a file stat");
                };
                assert_eq!(body.environment_id, environment);
                if body.path == "web" {
                    return Ok(CommandResult::FsStat(FileStat {
                        entry: DirEntry {
                            name: "web".into(),
                            path: body.path.clone(),
                            kind: DirEntryKind::Dir,
                            size: 0,
                            mtime: now,
                            mode: 0o700,
                            link_target: None,
                            cached: false,
                        },
                        hash: None,
                        media_type: "inode/directory".into(),
                        is_text: false,
                    }));
                }
                assert!(body.path.starts_with("web/"));
                assert!(body.mount.is_none());
                Ok(CommandResult::FsStat(FileStat {
                    entry: DirEntry {
                        name: "result.bin".into(),
                        path: body.path.clone(),
                        kind: DirEntryKind::File,
                        size,
                        mtime: now,
                        mode: 0o600,
                        link_target: None,
                        cached: false,
                    },
                    hash: Some(hash),
                    media_type: "application/octet-stream".into(),
                    is_text: false,
                }))
            });
        world
            .machine
            .handler
            .on(CommandKind::FsFetch, move |command| {
                let Command::FsFetch(body) = command else {
                    panic!("a file fetch");
                };
                assert_eq!(body.environment_id, environment);
                assert_eq!(body.path, "web/out/result.bin");
                assert!(body.mount.is_none());
                assert!(body.max_bytes.is_some_and(|limit| size <= limit));
                Ok(CommandResult::FsFetch(FsFetchResult {
                    hash,
                    size,
                    mtime_ns,
                }))
            });
        Self {
            world,
            sender,
            receiver,
            chat,
            chat_write,
            thread_write,
            store,
            bytes,
            hash,
            mtime_ns,
            _directory: directory,
        }
    }

    fn arguments(&self, destination: Destination, paths: &[&str]) -> Json {
        match destination {
            Destination::Chat => {
                json!({ "target": { "kind": "chat", "path": format!("id:{}", self.chat) }, "text": "The whole result.", "files": paths })
            }
            Destination::Thread => {
                json!({ "target": { "kind": "thread", "path": format!("id:{}", self.receiver) }, "text": "The whole result.", "files": paths })
            }
        }
    }

    async fn count(&self, destination: Destination) -> i64 {
        match destination {
            Destination::Chat => {
                self.world
                    .count(
                        "SELECT count(*) FROM chat_messages WHERE chat_id=$1",
                        self.chat,
                    )
                    .await
            }
            Destination::Thread => {
                self.world
                    .count(
                        "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                        self.sender,
                    )
                    .await
            }
        }
    }

    async fn kept(&self, destination: Destination) -> Vec<Json> {
        let content: Json = match destination {
            Destination::Chat => {
                self.world
                    .scalar(
                        "SELECT content FROM chat_messages WHERE chat_id=$1",
                        self.chat,
                    )
                    .await
            }
            Destination::Thread => {
                self.world
                    .scalar(
                        "SELECT files FROM announcements WHERE from_thread_id=$1",
                        self.sender,
                    )
                    .await
            }
        };
        content
            .as_array()
            .unwrap()
            .iter()
            .filter(|part| part.get("blob").is_some())
            .cloned()
            .collect()
    }

    async fn pins(&self) -> i32 {
        let mut conn = self.world.pool.acquire().await.unwrap();
        storage::blob(&mut conn, &self.hash)
            .await
            .unwrap()
            .map_or(0, |row| row.pin_count)
    }

    async fn limit(&self, destination: Destination, bytes: i64) {
        let (target, key) = match destination {
            Destination::Chat => (
                Target::Node(self.world.root),
                keys::CHAT_ATTACHMENT_MAX_BYTES,
            ),
            Destination::Thread => (Target::System, keys::STORAGE_MAX_BLOB_BYTES),
        };
        let mut conn = self.world.pool.acquire().await.unwrap();
        write::write(
            &mut conn,
            &write::Write {
                actor: Actor::User(self.world.ada),
                target,
                key,
                value: json!(bytes),
                locked: None,
                expected_revision: None,
                may_lock: true,
                origin: &Origin::default(),
            },
            self.world.clock.now(),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn chat_and_thread_messages_keep_a_whole_binary_file_even_in_the_same_environment() {
    for destination in [Destination::Chat, Destination::Thread] {
        // Cross one upload-chunk boundary, including NUL and non-UTF-8 bytes: a preview is not the file.
        for size in [0, orch_api_contract::common::UPLOAD_CHUNK_BYTES + 1] {
            let files = Files::new(size).await;
            let mut arguments = files.arguments(destination, &["out/result.bin"]);
            arguments.as_object_mut().unwrap().remove("text"); // A file alone is a message too.
            let reply = ok(call(&files.world, files.sender, destination.tool(), arguments).await);
            assert_eq!(reply["files"].as_array().unwrap().len(), 1);
            assert_eq!(files.count(destination).await, 1);
            let kept = files.kept(destination).await;
            assert_eq!(kept.len(), 1);
            assert_eq!(kept[0]["name"], "result.bin");
            assert_eq!(kept[0]["size"], json!(files.bytes.len()));
            assert_eq!(kept[0]["blob"], json!(files.hash));
            let read = files.store.get(&files.hash, None).await.unwrap();
            assert_eq!(read_all(read.body).await.unwrap(), files.bytes);
            assert_eq!(
                files.pins().await,
                1,
                "the persisted message, not the source directory, retains the bytes"
            );
            assert_eq!(file_commands(&files.world, "fs.fetch"), 1);
            if let Destination::Thread = destination {
                let message: Id = files
                    .world
                    .scalar(
                        "SELECT id FROM announcements WHERE from_thread_id=$1",
                        files.sender,
                    )
                    .await;
                let path = format!(".orch/shared/from-Build/{message}/result.bin");
                assert_eq!(kept[0]["path"], path);
                let pending: Json = files
                    .world
                    .scalar(
                        "SELECT pending_notices FROM threads WHERE node_id=$1",
                        files.receiver,
                    )
                    .await;
                assert_eq!(pending[0]["files"], json!(kept));
                assert_eq!(reply["files"][0]["path"], path);
                let delivery = orch_engine::tools::thread_message::message_text(
                    "Build",
                    pending[0]["body"].as_str().unwrap(),
                    pending[0]["at"].as_str().unwrap(),
                );
                assert!(delivery.contains(&path));
            }
        }
    }
}

#[tokio::test]
async fn absolute_parent_and_windows_paths_are_refused_before_reading_or_posting() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        for path in [
            "/etc/private",
            "../private",
            "out/../../private",
            "C:\\private",
            "out\\..\\private",
        ] {
            let answer = call(
                &files.world,
                files.sender,
                destination.tool(),
                files.arguments(destination, &[path]),
            )
            .await;
            assert_eq!(refused(&answer).0, "invalid");
            assert!(refused(&answer).1.contains("relative path"));
        }
        assert_eq!(files.count(destination).await, 0);
        assert_eq!(file_commands(&files.world, "fs.stat"), 0);
        assert_eq!(file_commands(&files.world, "fs.fetch"), 0);
        assert_eq!(files.pins().await, 0);
    }
}

#[tokio::test]
async fn a_native_root_refusal_never_posts_text_or_exposes_the_machines_diagnostic() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        files.world.machine.handler.on(CommandKind::FsFetch, |_| {
            Err(CommandError::new(
                ErrorCode::PolicyRootNotAllowed,
                "/private/outside-root: source_constraint_fkey",
            ))
        });
        let answer = call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin"]),
        )
        .await;
        assert_eq!(refused(&answer).0, "not_found");
        assert!(refused(&answer).1.contains("inside the environment"));
        assert!(!refused(&answer).1.contains("outside-root"));
        assert!(!refused(&answer).1.contains("_fkey"));
        assert_eq!(files.count(destination).await, 0);
        assert_eq!(files.pins().await, 0);
    }
}

#[tokio::test]
async fn the_exact_upload_limit_is_passed_to_the_machine_and_a_larger_file_is_refused_before_fetch()
{
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        files.limit(destination, 63).await;
        let answer = call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin"]),
        )
        .await;
        assert_eq!(refused(&answer).0, "invalid");
        assert!(refused(&answer).1.contains("smaller file"));
        assert_eq!(file_commands(&files.world, "fs.fetch"), 0);
        assert_eq!(files.count(destination).await, 0);
        files.limit(destination, 64).await;
        ok(call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin"]),
        )
        .await);
        assert_eq!(files.count(destination).await, 1);
        let fetches = files.world.commands.issued_bodies("fs.fetch");
        assert_eq!(fetches.len(), 1, "a file fetch");
        assert_eq!(fetches[0].1["max_bytes"], json!(64));
        assert_eq!(files.pins().await, 1);
    }
}

#[tokio::test]
async fn an_offline_environment_is_refused_without_sending_a_file_command() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        // A call comes from a running turn: the agent's turn began while its machine was there.
        prepare_call_turn(&files.world, files.sender).await;
        files
            .world
            .set_machine_online(files.world.machine_id, false)
            .await;
        let answer = call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin"]),
        )
        .await;
        assert_eq!(refused(&answer).0, "closed");
        assert!(refused(&answer).1.contains("reconnect its machine"));
        assert_eq!(file_commands(&files.world, "fs.stat"), 0);
        assert_eq!(file_commands(&files.world, "fs.fetch"), 0);
        assert_eq!(files.count(destination).await, 0);
    }
}

#[tokio::test]
async fn a_repeat_keeps_the_original_file_and_does_not_fetch_or_pin_it_twice() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        let call_ref = new_id().to_string();
        let arguments = files.arguments(destination, &["out/result.bin"]);
        let first = ok(call_as(
            &files.world,
            files.sender,
            destination.tool(),
            arguments.clone(),
            &call_ref,
        )
        .await);
        files.world.machine.handler.on(CommandKind::FsFetch, |_| {
            panic!("the completed message must not reread a removed source file")
        });
        let turn: Id = files
            .world
            .scalar(
                "SELECT current_turn_id FROM threads WHERE node_id=$1",
                files.sender,
            )
            .await;
        files
            .world
            .engine()
            .answer_tool_call(&CallContext {
                machine: files.world.machine_id,
                thread: files.sender,
                turn,
                call_ref,
                attempt: Some(new_id()),
                tool: orch_agent_contract::events::OrchTool::SendMessage,
                arguments,
            })
            .await
            .unwrap();
        assert_eq!(files.count(destination).await, 1);
        assert_eq!(files.pins().await, 1);
        assert_eq!(file_commands(&files.world, "fs.fetch"), 1);
        assert_eq!(first["files"].as_array().unwrap().len(), 1);
        assert_eq!(
            read_all(files.store.get(&files.hash, None).await.unwrap().body)
                .await
                .unwrap(),
            files.bytes
        );
    }
}

#[tokio::test]
async fn a_write_share_removed_during_reading_leaves_the_message_unsent() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        files.world.machine.handler.hold(CommandKind::FsFetch);
        let sending = call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin"]),
        );
        tokio::pin!(sending);
        tokio::select! {
            result = &mut sending => panic!("the file read must still be held: {result:?}"),
            () = files.world.wait_began(CommandKind::FsFetch, 1) => {},
        }
        let entry = match destination {
            Destination::Chat => files.chat_write,
            Destination::Thread => files.thread_write,
        };
        {
            let mut conn = files.world.pool.acquire().await.unwrap();
            assert!(
                access::remove_entry(&mut conn, entry, files.world.ada, files.world.clock.now())
                    .await
                    .unwrap()
            );
            access::bump_epoch(&mut conn).await.unwrap();
        }
        files.world.machine.handler.release();
        let refused_write = sending.await;
        let expected = match destination {
            Destination::Chat => "not_writable",
            Destination::Thread => "not_found",
        };
        assert_eq!(refused(&refused_write).0, expected);
        assert!(refused(&refused_write).1.contains("shared"));
        assert_eq!(files.count(destination).await, 0);
        assert_eq!(files.pins().await, 0);
    }
}

#[tokio::test]
async fn a_lowered_limit_during_upload_is_rechecked_before_the_message_is_written() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        files.world.machine.handler.hold(CommandKind::FsFetch);
        let sending = call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin"]),
        );
        tokio::pin!(sending);
        tokio::select! {
            result = &mut sending => panic!("the file read must still be held: {result:?}"),
            () = files.world.wait_began(CommandKind::FsFetch, 1) => {},
        }
        files.limit(destination, 63).await;
        files.world.machine.handler.release();
        let answer = sending.await;
        assert_eq!(refused(&answer).0, "invalid");
        assert!(refused(&answer).1.contains("smaller file"));
        assert_eq!(files.count(destination).await, 0);
        assert_eq!(files.pins().await, 0);
    }
}

#[tokio::test]
async fn one_unreadable_file_leaves_the_text_and_every_other_file_unsent() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        let hash = files.hash;
        let mtime_ns = files.mtime_ns;
        files
            .world
            .machine
            .handler
            .on(CommandKind::FsFetch, move |command| {
                let Command::FsFetch(body) = command else {
                    panic!("a file fetch");
                };
                if body.path.ends_with("missing.bin") {
                    return Err(CommandError::new(ErrorCode::NotFound, "not there"));
                }
                Ok(CommandResult::FsFetch(FsFetchResult {
                    hash,
                    size: 64,
                    mtime_ns,
                }))
            });
        let answer = call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin", "out/missing.bin"]),
        )
        .await;
        assert_eq!(refused(&answer).0, "not_found");
        assert!(refused(&answer).1.contains("missing.bin"));
        assert_eq!(files.count(destination).await, 0);
        assert_eq!(files.pins().await, 0);
        assert_eq!(file_commands(&files.world, "fs.fetch"), 2);
    }
}

#[tokio::test]
async fn changing_the_sending_directory_during_reading_never_completes_the_old_message() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        files.world.machine.handler.hold(CommandKind::FsFetch);
        let sending = call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin"]),
        );
        tokio::pin!(sending);
        tokio::select! {
            result = &mut sending => panic!("the file read must still be held: {result:?}"),
            () = files.world.wait_began(CommandKind::FsFetch, 1) => {},
        }
        sqlx::query("UPDATE threads SET workspace_dir='another' WHERE node_id=$1")
            .bind(files.sender)
            .execute(files.world.pool.pool())
            .await
            .unwrap();
        files.world.machine.handler.release();
        let answer = sending.await;
        assert_eq!(refused(&answer).0, "changed");
        assert!(refused(&answer).1.contains("try again"));
        assert_eq!(files.count(destination).await, 0);
        assert_eq!(files.pins().await, 0);
    }
}

#[tokio::test]
async fn an_upload_acknowledgement_for_a_truncated_file_cannot_be_attached() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        let hash = files.hash;
        let mtime_ns = files.mtime_ns;
        files
            .world
            .machine
            .handler
            .on(CommandKind::FsFetch, move |_| {
                Ok(CommandResult::FsFetch(FsFetchResult {
                    hash,
                    size: 63,
                    mtime_ns,
                }))
            });
        let answer = call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin"]),
        )
        .await;
        assert_eq!(refused(&answer).0, "changed");
        assert_eq!(files.count(destination).await, 0);
        assert_eq!(files.pins().await, 0);
    }
}

#[tokio::test]
async fn native_growth_and_version_refusals_keep_their_recovery_without_machine_details() {
    for destination in [Destination::Chat, Destination::Thread] {
        for (native, code, recovery) in [
            (ErrorCode::TooLarge, "invalid", "smaller file"),
            (ErrorCode::Changed, "changed", "try again"),
        ] {
            let files = Files::new(64).await;
            files
                .world
                .machine
                .handler
                .on(CommandKind::FsFetch, move |_| {
                    Err(CommandError::new(native, "private_native_diagnostic"))
                });
            let answer = call(
                &files.world,
                files.sender,
                destination.tool(),
                files.arguments(destination, &["out/result.bin"]),
            )
            .await;
            assert_eq!(refused(&answer).0, code);
            assert!(refused(&answer).1.contains(recovery));
            assert!(!refused(&answer).1.contains("private_native_diagnostic"));
            assert_eq!(files.count(destination).await, 0);
            assert_eq!(files.pins().await, 0);
        }
    }
}

#[tokio::test]
async fn a_machine_receipt_without_the_stored_bytes_never_creates_an_attachment() {
    for destination in [Destination::Chat, Destination::Thread] {
        let files = Files::new(64).await;
        assert!(files.store.delete(&files.hash).await.unwrap());
        let answer = call(
            &files.world,
            files.sender,
            destination.tool(),
            files.arguments(destination, &["out/result.bin"]),
        )
        .await;
        assert_eq!(refused(&answer).0, "closed");
        assert!(refused(&answer).1.contains("file storage"));
        assert_eq!(files.count(destination).await, 0);
        assert_eq!(files.pins().await, 0);
    }
}

#[tokio::test]
async fn a_saved_thread_file_is_materialized_before_the_receiving_agents_next_turn() {
    let files = Files::new(64).await;
    files
        .world
        .machine
        .handler
        .on(CommandKind::FsMaterialize, |_| {
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: 1,
                bytes: 64,
                skipped: 0,
            }))
        });
    let reply = ok(call(
        &files.world,
        files.sender,
        OrchTool::SendMessage,
        files.arguments(Destination::Thread, &["out/result.bin"]),
    )
    .await);
    let path = reply["files"][0]["path"].as_str().unwrap();
    let (turn, _) = files
        .world
        .running_turn(files.receiver, "Use the result that Build sent.")
        .await;
    assert!(turn.text.contains(path));
    let writes: Vec<FsMaterialize> = files
        .world
        .commands
        .issued_bodies("fs.materialize")
        .into_iter()
        .map(|(_, body)| serde_json::from_value(body).unwrap())
        .collect();
    assert_eq!(writes.len(), 1);
    let write = &writes[0];
    assert_eq!(write.environment_id, files.world.environment);
    assert_eq!(write.purpose, MaterializePurpose::Delivery);
    assert_eq!(write.target_root, "/workspace/web/.orch");
    let manifest = read_all(
        files
            .store
            .get(&write.manifest_blob, None)
            .await
            .unwrap()
            .body,
    )
    .await
    .unwrap();
    let manifest = orch_engine::manifest::Manifest::parse(&manifest).unwrap();
    let entry = manifest.get(path.strip_prefix(".orch/").unwrap()).unwrap();
    assert_eq!(entry.hash, Some(files.hash));
    assert_eq!(entry.size, 64);
    assert_eq!(
        read_all(
            files
                .store
                .get(&entry.hash.unwrap(), None)
                .await
                .unwrap()
                .body
        )
        .await
        .unwrap(),
        files.bytes
    );
    let order: Vec<_> = files
        .world
        .machine
        .handler
        .executions()
        .iter()
        .map(|execution| execution.kind)
        .collect();
    let materialized = order
        .iter()
        .position(|candidate| *candidate == CommandKind::FsMaterialize)
        .unwrap();
    // The sender's own turn, which sent the file, started first; the receiving agent's next turn is the
    // last one started.
    let next_turn = order
        .iter()
        .rposition(|candidate| *candidate == CommandKind::TurnStart)
        .unwrap();
    assert!(materialized < next_turn, "{order:?}");
}

#[tokio::test]
async fn separate_messages_with_the_same_filename_keep_separate_deliveries() {
    let files = Files::new(64).await;
    let first = ok(call(
        &files.world,
        files.sender,
        OrchTool::SendMessage,
        files.arguments(Destination::Thread, &["out/result.bin"]),
    )
    .await);
    let second = ok(call(
        &files.world,
        files.sender,
        OrchTool::SendMessage,
        files.arguments(Destination::Thread, &["out/result.bin"]),
    )
    .await);
    assert_eq!(first["files"][0]["name"], "result.bin");
    assert_eq!(second["files"][0]["name"], "result.bin");
    assert_ne!(first["files"][0]["path"], second["files"][0]["path"]);
    assert_eq!(files.count(Destination::Thread).await, 2);
    assert_eq!(
        files.pins().await,
        2,
        "both independently saved messages retain their file"
    );
    assert_eq!(
        read_all(files.store.get(&files.hash, None).await.unwrap().body)
            .await
            .unwrap(),
        files.bytes
    );
}
