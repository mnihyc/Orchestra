//! Direct document and chat attachment downloads keep original bytes, owner Read plus Export, and no-overwrite delivery.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use async_trait::async_trait;
use bytes::Bytes;
use orch_api_contract::chats::{ChatKind, ChatPart, ChatRef};
use orch_api_contract::threads::{
    Forwarded, ForwardedAttachment, ForwardedFrom, ForwardedImage, ForwardedType, ImageMediaType,
};
use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobError, BlobRead, BlobStore, ByteStream, Stored, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::{access, documents, storage};
use orch_link_contract::commands::{OrchTool, OrchToolResult};
use orch_settings::{Target, write};
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;
use shared::{
    MemoryParcels, Places, a_world_whose_agent_takes_tools, call, call_as, chat_of, ok, refused,
    share,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

struct Pause {
    entered: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

#[derive(Clone, Copy)]
enum StreamFault {
    Longer,
    Shorter,
    Different,
}

/// Real local bytes, with a deterministic pause after get and optional malformed-body evidence.
struct ObservedStore {
    local: LocalBlobStore,
    reads: AtomicUsize,
    pause: Mutex<Option<Pause>>,
    fault: Mutex<Option<StreamFault>>,
}

impl ObservedStore {
    fn pause_once(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered, waiting) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        *self.pause.lock().unwrap() = Some(Pause { entered, resume });
        (waiting, release)
    }
}

#[async_trait]
impl BlobStore for ObservedStore {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.local.put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let mut read = self.local.get(hash, range).await?;
        let pause = self.pause.lock().unwrap().take();
        if let Some(pause) = pause {
            pause
                .entered
                .send(())
                .map_err(|()| BlobError::Io("the test reader went away".into()))?;
            pause
                .resume
                .await
                .map_err(|_| BlobError::Io("the test release went away".into()))?;
        }
        if let Some(fault) = self.fault.lock().unwrap().take() {
            // Metadata stays real. The consumer must reject a body that exceeds its cap or ends early.
            read.body = stream_of(vec![match fault {
                StreamFault::Longer => {
                    Bytes::from(vec![0; usize::try_from(read.meta.size).unwrap() + 1])
                }
                StreamFault::Shorter => Bytes::new(),
                StreamFault::Different => {
                    Bytes::from(vec![0; usize::try_from(read.meta.size).unwrap()])
                }
            }]);
        }
        Ok(read)
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.local.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        self.local.size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.local.delete(hash).await
    }
}

#[derive(Debug, Clone, Copy)]
enum Owner {
    Document,
    NodeChat,
    PeopleChat,
}

const OWNERS: [Owner; 3] = [Owner::Document, Owner::NodeChat, Owner::PeopleChat];
const FILE_NAME: &str = "report.v1.bin";
const BINARY: &[u8] = b"\0\xffreport\r\n\x80end";

type DownloadAudit = (String, Option<Id>, String, Option<Id>, Option<Id>);

struct Library {
    world: common::World,
    agent: Id,
    topic: Id,
    document: Id,
    file: Id,
    node_chat: Id,
    people_chat: Id,
    people_org: Id,
    messages: [Id; 2],
    read_entries: [Id; 3],
    export_entries: [Id; 3],
    store: Arc<ObservedStore>,
    parcels: Arc<MemoryParcels>,
    _directory: tempfile::TempDir,
}

impl Owner {
    fn index(self) -> usize {
        match self {
            Self::Document => 0,
            Self::NodeChat => 1,
            Self::PeopleChat => 2,
        }
    }
}

impl Library {
    async fn close(self) {
        tokio::time::timeout(common::PATIENCE, self.world.shutdown())
            .await
            .expect(
                "attachment fixture workers and connections stop within the original proof budget",
            );
        drop(self);
    }

    async fn new() -> Self {
        Self::build(None).await
    }

    async fn for_case(trace: &CaseTrace) -> Self {
        Self::build(Some(trace)).await
    }

    async fn build(trace: Option<&CaseTrace>) -> Self {
        let phase = |name| {
            if let Some(trace) = trace {
                trace.phase("setup", name);
            }
        };
        phase("creating world and database");
        let world = a_world_whose_agent_takes_tools().await;
        phase("creating requesting thread and attachment owners");
        let agent = world.thread("Fetcher").await;
        let topic = world.topic("Library").await;
        let document = world
            .document_beside(topic, "Results", &["Stored attachments."])
            .await;
        let node_chat = chat_of(&world, topic).await;
        let people_chat = new_id();
        let people_org = new_id();
        // Removing the source chat's org must not remove the requesting thread or its bindings.
        let mut conn = world.pool.acquire().await.unwrap();
        insert_node(
            &mut conn,
            people_org,
            "org",
            None,
            "Review archive",
            world.ada,
        )
        .await
        .unwrap();
        access::testing::insert_member(&mut conn, people_org, world.ada, people_org)
            .await
            .unwrap();
        sqlx::query("INSERT INTO chats (id,kind,org_id,name,created_by) VALUES ($1,'group',$2,'Review files',$3)")
            .bind(people_chat).bind(people_org).bind(world.ada)
            .execute(&mut *conn).await.unwrap();
        sqlx::query(
            "INSERT INTO chat_members (chat_id,user_id,added_by,seq_updated) VALUES ($1,$2,$2,1)",
        )
        .bind(people_chat)
        .bind(world.ada)
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(ObservedStore {
            local: LocalBlobStore::open(directory.path().join("files"))
                .await
                .unwrap(),
            reads: AtomicUsize::new(0),
            pause: Mutex::new(None),
            fault: Mutex::new(None),
        });
        world.engine().attach_blobs(store.clone());
        let parcels = Arc::new(MemoryParcels::default());
        world.engine().attach_parcels(parcels.clone());
        phase("granting owner-specific read and export");
        let mut read_entries = [Id::nil(); 3];
        let mut export_entries = [Id::nil(); 3];
        for (index, kind, object, read, export) in [
            (0, "document", document, "doc.view", "doc.export"),
            (1, "node", topic, "chat.view", "chat.export"),
            (2, "chat", people_chat, "chat.view", "chat.export"),
        ] {
            read_entries[index] = share(&world, kind, object, "live", read, "any", agent).await;
            export_entries[index] = share(&world, kind, object, "live", export, "any", agent).await;
        }
        let file = new_id();
        let messages = [new_id(), new_id()];
        let this = Self {
            world,
            agent,
            topic,
            document,
            file,
            node_chat,
            people_chat,
            people_org,
            messages,
            read_entries,
            export_entries,
            store,
            parcels,
            _directory: directory,
        };
        phase("storing original binary and image blobs");
        let hash = this.keep(BINARY).await;
        documents::add_file(
            &mut this.world.pool.acquire().await.unwrap(),
            &documents::NewFile {
                id: file,
                document,
                blob: hash,
                name: FILE_NAME,
                media_type: "application/octet-stream",
                size: i64::try_from(BINARY.len()).unwrap(),
                is_text: false,
                added_by: Some(this.world.ada),
                added_by_thread: None,
                at: this.world.clock.now(),
            },
        )
        .await
        .unwrap();
        let parts = this.parts(hash).await;
        phase("storing live messages and their tombstone ordinals");
        for (chat, message) in [(node_chat, messages[0]), (people_chat, messages[1])] {
            // A tombstone occupies ordinal one. Seq 90 is ordinal two; carried slots are not part offsets.
            sqlx::query("INSERT INTO chat_messages (id,chat_id,author_id,content,seq_created,seq_updated,deleted_at)
                VALUES ($1,$2,$3,'[]',40,40,$4)")
                .bind(new_id()).bind(chat).bind(this.world.ada).bind(this.world.clock.now())
                .execute(&mut *this.world.pool.acquire().await.unwrap()).await.unwrap();
            sqlx::query(
                "INSERT INTO chat_messages (id,chat_id,author_id,content,seq_created,seq_updated)
                VALUES ($1,$2,$3,$4,90,90)",
            )
            .bind(message)
            .bind(chat)
            .bind(this.world.ada)
            .bind(serde_json::to_value(&parts).unwrap())
            .execute(&mut *this.world.pool.acquire().await.unwrap())
            .await
            .unwrap();
            sqlx::query("UPDATE chats SET next_seq=91,last_message_seq=90 WHERE id=$1")
                .bind(chat)
                .execute(&mut *this.world.pool.acquire().await.unwrap())
                .await
                .unwrap();
        }
        phase("ready");
        this
    }

    async fn keep(&self, bytes: &[u8]) -> Hash {
        let hash = Hash::of(bytes);
        let size = i64::try_from(bytes.len()).unwrap();
        self.store
            .put(
                &hash,
                Some(size),
                stream_of(vec![Bytes::copy_from_slice(bytes)]),
            )
            .await
            .unwrap();
        let mut conn = self.world.pool.acquire().await.unwrap();
        storage::register_blob(&mut conn, &hash, size, self.world.clock.now())
            .await
            .unwrap();
        storage::pin_blobs(&mut conn, &[hash]).await.unwrap();
        hash
    }

    async fn parts(&self, binary: Hash) -> Vec<ChatPart> {
        let png = self.keep(b"\x89PNG\r\n\x1a\n\0\xffdirect").await;
        let jpeg = self.keep(b"\xff\xd8\0\x80forwarded image").await;
        let forwarded = self.keep(b"\0\xfea forwarded file").await;
        let tail = self.keep(b"\0\xfdtail").await;
        vec![
            ChatPart::Text {
                text: "Files from this chat; no access to the original chat is required.".into(),
            },
            ChatPart::Attachment {
                blob: binary,
                name: FILE_NAME.into(),
                size: i64::try_from(BINARY.len()).unwrap(),
                media_type: "application/octet-stream".into(),
            },
            ChatPart::Image {
                blob: png,
                media_type: ImageMediaType::Png,
                name: "a/screen.png".into(),
            },
            ChatPart::Forwarded(Box::new(Forwarded {
                r#type: ForwardedType::Forwarded,
                from: ForwardedFrom::Chat {
                    chat: ChatRef {
                        id: new_id(),
                        kind: ChatKind::Group,
                        org_id: self.world.root,
                        node: None,
                        name: None,
                    },
                },
                source_id: new_id(),
                author: None,
                at: Time::new(self.world.clock.now()),
                text: "A retained snapshot.".into(),
                images: vec![ForwardedImage {
                    blob: jpeg,
                    media_type: "image/jpeg".into(),
                    name: "same.jpg".into(),
                }],
                attachments: vec![ForwardedAttachment {
                    blob: forwarded,
                    media_type: "application/octet-stream".into(),
                    name: FILE_NAME.into(),
                    size: i64::try_from(b"\0\xfea forwarded file".len()).unwrap(),
                }],
            })),
            ChatPart::Attachment {
                blob: tail,
                name: "last.bin".into(),
                size: i64::try_from(b"\0\xfdtail".len()).unwrap(),
                media_type: "application/octet-stream".into(),
            },
        ]
    }

    fn id(&self, owner: Owner) -> Id {
        match owner {
            Owner::Document => self.document,
            Owner::NodeChat => self.node_chat,
            Owner::PeopleChat => self.people_chat,
        }
    }

    async fn paths(&self, owner: Owner) -> Vec<String> {
        let read = ok(call(
            &self.world,
            self.agent,
            OrchTool::ReadShared,
            json!({"path":format!("id:{}",self.id(owner))}),
        )
        .await);
        let path = read["entry"]["path"].as_str().unwrap();
        let parent = path.rsplit_once('/').unwrap().0;
        let directory = format!("{parent}/{}/", orch_engine::written::files_beside(path));
        let listing = ok(call(
            &self.world,
            self.agent,
            OrchTool::ListShared,
            json!({"path":directory,"sort":"name"}),
        )
        .await);
        listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                assert_eq!(entry["kind"], "file");
                entry["path"].as_str().unwrap().to_owned()
            })
            .collect()
    }

    async fn download(&self, path: &str, extra: Json) -> OrchToolResult {
        let mut arguments = extra;
        arguments["path"] = json!(path);
        call(&self.world, self.agent, OrchTool::DownloadShared, arguments).await
    }

    fn written(&self, path: &str) -> Vec<u8> {
        self.parcels
            .files
            .lock()
            .unwrap()
            .get(&(self.agent, path.to_owned()))
            .unwrap()
            .clone()
    }

    fn empty(&self) {
        assert!(
            self.parcels.files.lock().unwrap().is_empty(),
            "a refused file makes no workspace write"
        );
    }

    async fn revoke(&self, entry: Id) {
        let mut conn = self.world.pool.acquire().await.unwrap();
        assert!(
            access::remove_entry(&mut conn, entry, self.world.ada, self.world.clock.now())
                .await
                .unwrap()
        );
        access::bump_epoch(&mut conn).await.unwrap();
    }

    async fn limit(&self, largest: i64) {
        let mut conn = self.world.pool.acquire().await.unwrap();
        write::write(
            &mut conn,
            &write::Write {
                actor: orch_db::audit::Actor::User(self.world.ada),
                target: Target::System,
                key: keys::STORAGE_MAX_BLOB_BYTES,
                value: json!(largest),
                locked: None,
                expected_revision: None,
                may_lock: true,
                origin: &orch_db::audit::Origin::default(),
            },
            self.world.clock.now(),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn document_and_both_chat_kinds_download_original_nul_and_non_utf8_bytes_with_the_canonical_filename()
 {
    for owner in OWNERS {
        let trace = CaseTrace::new("binary and canonical filename", owner, None);
        let library = Library::for_case(&trace).await;
        trace.phase("read", "discovering exact attachment path");
        let paths = library.paths(owner).await;
        let first = &paths[0];
        assert!(
            first.ends_with(FILE_NAME),
            "{owner:?}: the discovered name keeps its original safe filename"
        );
        let name = first.rsplit('/').next().unwrap();
        for format in ["markdown", "json"] {
            trace.phase("read", format);
            let answer = ok(library
                .download(first, json!({"format":format,"detail":"brief"}))
                .await);
            assert_eq!(answer["status"], "ready", "{owner:?} {format}");
            assert_eq!(
                answer["path"],
                format!(".orch/shared/downloads/{name}"),
                "{owner:?} {format}: canonical destination"
            );
            assert_eq!(
                answer["size"],
                json!(BINARY.len()),
                "{owner:?} {format}: original byte count"
            );
            assert_eq!(
                answer["size"], answer["received"],
                "{owner:?} {format}: complete delivery"
            );
            assert_eq!(
                library.written(answer["path"].as_str().unwrap()),
                BINARY,
                "{owner:?} {format}: original NUL and non-UTF8 bytes"
            );
        }
        trace.phase("read", "checking original owner audit and parent");
        let audited: i64 = library.world.count(
            "SELECT count(*) FROM audit_log WHERE action='share.file_downloaded' AND object_id=$1", library.id(owner)).await;
        assert_eq!(
            audited, 2,
            "downloads audit the owning document/chat, not the attachment or a person"
        );
        let audits: Vec<DownloadAudit> = sqlx::query_as(
            "SELECT actor_kind,actor_id,object_kind,object_id,node_id FROM audit_log WHERE action='share.file_downloaded' AND object_id=$1")
            .bind(library.id(owner)).fetch_all(&mut *library.world.pool.acquire().await.unwrap()).await.unwrap();
        let kind = if matches!(owner, Owner::Document) {
            "document"
        } else {
            "chat"
        };
        // Audit enrichment records a document's actual parent topic, just as
        // it records a node chat's owner; a people chat has no owning node.
        let node = match owner {
            Owner::Document | Owner::NodeChat => Some(library.topic),
            Owner::PeopleChat => None,
        };
        for audit in audits {
            assert_eq!(
                audit,
                (
                    "agent".into(),
                    Some(library.agent),
                    kind.into(),
                    Some(library.id(owner)),
                    node
                ),
                "{owner:?}: original actor, owner and parent"
            );
        }
        trace.drop_library(library).await;
    }
}

#[tokio::test]
async fn images_and_forwarded_files_follow_the_rendered_carried_order_and_message_ordinal() {
    for owner in [Owner::NodeChat, Owner::PeopleChat] {
        let library = Library::new().await;
        let paths = library.paths(owner).await;
        let expected: [(&str, &[u8]); 5] = [
            ("0002-01-report.v1.bin", BINARY),
            ("0002-02-a_screen.png", b"\x89PNG\r\n\x1a\n\0\xffdirect"),
            ("0002-03-same.jpg", b"\xff\xd8\0\x80forwarded image"),
            ("0002-04-report.v1.bin", b"\0\xfea forwarded file"),
            ("0002-05-last.bin", b"\0\xfdtail"),
        ];
        assert_eq!(paths.len(), expected.len());
        for (path, (name, bytes)) in paths.iter().zip(expected) {
            assert!(path.ends_with(name), "{path}");
            let answer = ok(library.download(path, json!({})).await);
            assert_eq!(answer["path"], format!(".orch/shared/downloads/{name}"));
            assert_eq!(library.written(answer["path"].as_str().unwrap()), bytes);
        }
        library.close().await;
    }
}

#[tokio::test]
async fn equal_retries_are_harmless_and_other_bytes_at_save_to_are_never_replaced() {
    for owner in OWNERS {
        let library = Library::new().await;
        let paths = library.paths(owner).await;
        let arguments = json!({"path":paths[0],"save_to":"inputs/exact.bin"});
        let reference = new_id().to_string();
        let first = ok(call_as(
            &library.world,
            library.agent,
            OrchTool::DownloadShared,
            arguments.clone(),
            &reference,
        )
        .await);
        let replay = ok(call_as(
            &library.world,
            library.agent,
            OrchTool::DownloadShared,
            arguments,
            &reference,
        )
        .await);
        assert_eq!(first, replay);
        ok(library
            .download(&paths[0], json!({"save_to":"inputs/exact.bin"}))
            .await);
        assert_eq!(library.written("inputs/exact.bin"), BINARY);
        assert_eq!(library.parcels.files.lock().unwrap().len(), 1);
        // A distinct destination already differs before its first delivery attempt.
        library.parcels.files.lock().unwrap().insert(
            (library.agent, "inputs/conflict.bin".into()),
            b"existing local changes\0\xff".to_vec(),
        );
        let answer = library
            .download(&paths[0], json!({"save_to":"inputs/conflict.bin"}))
            .await;
        assert_eq!(refused(&answer).0, "exists");
        assert_eq!(
            library.written("inputs/conflict.bin"),
            b"existing local changes\0\xff"
        );
        assert_eq!(library.written("inputs/exact.bin"), BINARY);
        library.close().await;
    }
}

#[tokio::test]
async fn owner_read_without_export_refuses_file_bytes_even_when_the_person_owns_the_source() {
    for owner in OWNERS {
        let library = Library::new().await;
        let paths = library.paths(owner).await;
        library.revoke(library.export_entries[owner.index()]).await;
        let answer = library.download(&paths[0], json!({})).await;
        assert_eq!(refused(&answer).0, "not_downloadable");
        assert!(refused(&answer).1.contains("export"));
        assert_eq!(library.store.reads.load(Ordering::SeqCst), 0);
        library.empty();
        library.close().await;
    }
}

#[tokio::test]
async fn export_without_read_and_revoked_file_paths_refuse_without_private_metadata() {
    for owner in OWNERS {
        let library = Library::new().await;
        let paths = library.paths(owner).await;
        library.revoke(library.read_entries[owner.index()]).await;
        let answer = library.download(&paths[0], json!({})).await;
        assert_eq!(refused(&answer).0, "not_found");
        assert!(!refused(&answer).1.contains(FILE_NAME));
        assert_eq!(library.store.reads.load(Ordering::SeqCst), 0);
        library.empty();
        library.close().await;
    }
}

#[tokio::test]
async fn a_missing_blob_refuses_in_words_without_its_hash_and_without_workspace_bytes() {
    for owner in OWNERS {
        let library = Library::new().await;
        let path = library.paths(owner).await.remove(0);
        let hash = Hash::of(BINARY);
        assert!(library.store.delete(&hash).await.unwrap());
        let answer = library.download(&path, json!({})).await;
        assert_eq!(refused(&answer).0, "not_found");
        assert!(!refused(&answer).1.contains(&hash.to_string()));
        assert!(!refused(&answer).1.contains(FILE_NAME));
        library.empty();
        library.close().await;
    }
}

#[tokio::test]
async fn the_full_outcome_budget_is_checked_before_an_attachment_is_written() {
    let library = Library::new().await;
    let path = library.paths(Owner::Document).await.remove(0);
    let destination = format!("inputs/{}", "界".repeat(400));
    let refused_answer = library
        .download(&path, json!({"save_to":destination,"max_bytes":1000}))
        .await;
    assert_eq!(refused(&refused_answer).0, "too_large");
    library.empty();
    let answer = ok(library
        .download(&path, json!({"save_to":destination,"max_bytes":2000}))
        .await);
    assert_eq!(answer["path"], destination);
    assert_eq!(library.written(&destination), BINARY);
    library.close().await;
}

#[tokio::test]
async fn file_limits_bound_metadata_and_every_stream_piece_and_a_short_body_is_never_delivered() {
    for (limit, fault, expected) in [
        (i64::try_from(BINARY.len()).unwrap() - 1, None, "too_large"),
        (
            i64::try_from(BINARY.len()).unwrap(),
            Some(StreamFault::Longer),
            "too_large",
        ),
        (
            i64::try_from(BINARY.len()).unwrap(),
            Some(StreamFault::Shorter),
            "changed",
        ),
        (
            i64::try_from(BINARY.len()).unwrap(),
            Some(StreamFault::Different),
            "changed",
        ),
    ] {
        let library = Library::new().await;
        let path = library.paths(Owner::Document).await.remove(0);
        library.limit(limit).await;
        *library.store.fault.lock().unwrap() = fault;
        let answer = library.download(&path, json!({})).await;
        assert_eq!(refused(&answer).0, expected);
        library.empty();
        library.close().await;
    }
    let library = Library::new().await;
    let path = library.paths(Owner::Document).await.remove(0);
    library.limit(i64::try_from(BINARY.len()).unwrap()).await;
    let answer = ok(library.download(&path, json!({})).await);
    assert_eq!(library.written(answer["path"].as_str().unwrap()), BINARY);
    library.close().await;
    let library = Library::new().await;
    let image = library.paths(Owner::NodeChat).await.remove(1);
    library.limit(0).await;
    let answer = library.download(&image, json!({})).await;
    assert_eq!(
        refused(&answer).0,
        "too_large",
        "an image has no declared size and is bounded by the actual blob metadata"
    );
    assert_eq!(library.store.reads.load(Ordering::SeqCst), 1);
    library.empty();
    library.close().await;
}

#[tokio::test]
async fn an_attachment_path_cannot_be_used_as_the_chat_target_of_a_message() {
    for owner in OWNERS {
        let library = Library::new().await;
        let path = library.paths(owner).await.remove(0);
        let answer = call(
            &library.world,
            library.agent,
            OrchTool::SendMessage,
            json!({"target":{"kind":"chat","path":path},"text":"This is a file, not a chat."}),
        )
        .await;
        assert_eq!(refused(&answer).0, "wrong_kind");
        assert!(refused(&answer).1.contains("download_shared"));
        for chat in [library.node_chat, library.people_chat] {
            assert_eq!(
                library
                    .world
                    .count("SELECT count(*) FROM chat_messages WHERE chat_id=$1", chat)
                    .await,
                2
            );
        }
        library.empty();
        library.close().await;
    }
}

#[tokio::test]
async fn an_empty_original_attachment_is_a_real_zero_byte_file_at_a_zero_byte_cap() {
    let library = Library::new().await;
    let hash = library.keep(b"").await;
    assert!(
        documents::replace_file(
            &mut library.world.pool.acquire().await.unwrap(),
            &documents::NewFile {
                id: library.file,
                document: library.document,
                blob: hash,
                name: FILE_NAME,
                media_type: "application/octet-stream",
                size: 0,
                is_text: false,
                added_by: Some(library.world.ada),
                added_by_thread: None,
                at: library.world.clock.now(),
            }
        )
        .await
        .unwrap()
    );
    library.limit(0).await;
    let path = library.paths(Owner::Document).await.remove(0);
    let answer = ok(library.download(&path, json!({})).await);
    assert_eq!(answer["size"], 0);
    assert_eq!(answer["received"], 0);
    assert_eq!(answer["status"], "ready");
    assert_eq!(library.written(answer["path"].as_str().unwrap()), b"");
    library.close().await;
}

#[derive(Debug, Clone, Copy)]
enum Change {
    RevokeRead,
    RevokeExport,
    DeleteFile,
    DeleteMessage,
    DeleteOwner,
    DeleteParent,
    ReplaceFile,
    ReplaceMessage,
    LowerLimit,
}

/// Phases carry only synthetic case names and timings, never file contents,
/// private paths or SQL. Run with --nocapture to retain the last phase even if
/// an external target watchdog interrupts setup or synchronous database cleanup.
struct CaseTrace {
    suite: &'static str,
    owner: Owner,
    change: Option<Change>,
    started: Instant,
    phases: Mutex<std::collections::BTreeMap<&'static str, &'static str>>,
}

impl CaseTrace {
    fn new(suite: &'static str, owner: Owner, change: Option<Change>) -> Self {
        Self {
            suite,
            owner,
            change,
            started: Instant::now(),
            phases: Mutex::new(Default::default()),
        }
    }

    fn phase(&self, lane: &'static str, phase: &'static str) {
        self.phases.lock().unwrap().insert(lane, phase);
        eprintln!(
            "attachment case={} owner={:?} change={:?} phase={lane}/{phase} elapsed={:?}",
            self.suite,
            self.owner,
            self.change,
            self.started.elapsed()
        );
    }

    async fn failure(&self, library: &Library, what: &str) -> ! {
        // Evidence only after the original proof deadline. This separate test
        // pool cannot wait behind a saturated application pool indefinitely.
        let waits = tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar::<_, Json>(
                "SELECT COALESCE(jsonb_agg(jsonb_build_object(
                'state',state,'wait_type',wait_event_type,'wait',wait_event,
                'blocked_by',pg_blocking_pids(pid))), '[]'::jsonb)
                FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()",
            )
            .fetch_one(&library.world.db.pool),
        )
        .await;
        panic!(
            "{what}; case={} owner={:?} change={:?} elapsed={:?} phases={:?} database_waits={waits:?}",
            self.suite,
            self.owner,
            self.change,
            self.started.elapsed(),
            self.phases.lock().unwrap()
        );
    }

    async fn drop_library(&self, library: Library) {
        self.phase(
            "teardown",
            "joining fixture workers and closing connections",
        );
        tokio::time::timeout(common::PATIENCE, library.world.shutdown())
            .await
            .expect("attachment fixture shutdown stays within the original proof budget");
        self.phase("teardown", "dropping quiescent owned database");
        drop(library);
        self.phase("teardown", "complete");
    }
}

impl Library {
    async fn change(&self, owner: Owner, change: Change) {
        match change {
            Change::RevokeRead => self.revoke(self.read_entries[owner.index()]).await,
            Change::RevokeExport => self.revoke(self.export_entries[owner.index()]).await,
            Change::LowerLimit => self.limit(0).await,
            Change::DeleteFile => {
                assert!(matches!(owner, Owner::Document));
                assert!(
                    documents::remove_file(
                        &mut self.world.pool.acquire().await.unwrap(),
                        self.document,
                        self.file
                    )
                    .await
                    .unwrap()
                );
            }
            Change::DeleteMessage => {
                let message = match owner {
                    Owner::NodeChat => self.messages[0],
                    Owner::PeopleChat => self.messages[1],
                    Owner::Document => panic!("a chat target"),
                };
                sqlx::query("UPDATE chat_messages SET deleted_at=$2,content='[]' WHERE id=$1")
                    .bind(message)
                    .bind(self.world.clock.now())
                    .execute(&mut *self.world.pool.acquire().await.unwrap())
                    .await
                    .unwrap();
            }
            Change::DeleteOwner => match owner {
                Owner::Document => {
                    sqlx::query("UPDATE documents SET deleted_at=$2 WHERE id=$1")
                        .bind(self.document)
                        .bind(self.world.clock.now())
                        .execute(&mut *self.world.pool.acquire().await.unwrap())
                        .await
                        .unwrap();
                }
                Owner::NodeChat | Owner::PeopleChat => {
                    sqlx::query("DELETE FROM chats WHERE id=$1")
                        .bind(self.id(owner))
                        .execute(&mut *self.world.pool.acquire().await.unwrap())
                        .await
                        .unwrap();
                }
            },
            Change::DeleteParent => {
                let parent = if matches!(owner, Owner::PeopleChat) {
                    self.people_org
                } else {
                    self.topic
                };
                let mut conn = self.world.pool.acquire().await.unwrap();
                let caller = orch_authz::reach::Agent::of(&mut conn, self.agent)
                    .await
                    .unwrap()
                    .expect("the requesting agent is live before deleting the source parent");
                assert!(
                    !caller.path.contains(&parent),
                    "only the attachment's source parent is removed"
                );
                let before: Id =
                    sqlx::query_scalar("SELECT current_turn_id FROM threads WHERE node_id=$1")
                        .bind(self.agent)
                        .fetch_one(&mut *conn)
                        .await
                        .unwrap();
                assert_eq!(
                    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
                        .bind(parent)
                        .bind(self.world.clock.now())
                        .execute(&mut *conn)
                        .await
                        .unwrap()
                        .rows_affected(),
                    1
                );
                assert!(
                    orch_authz::reach::Agent::of(&mut conn, self.agent)
                        .await
                        .unwrap()
                        .is_some(),
                    "removing the source parent leaves the actual caller reachable"
                );
                drop(conn);
                let healthy = ok(call(
                    &self.world,
                    self.agent,
                    OrchTool::ListShared,
                    json!({"path":format!("id:{}",self.world.topic),"sort":"name","ids":true}),
                )
                .await);
                assert!(
                    healthy["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry["id"] == json!(self.agent)),
                    "the same native caller can still list its own topic after the source parent is removed"
                );
                let after: Id = self
                    .world
                    .scalar(
                        "SELECT current_turn_id FROM threads WHERE node_id=$1",
                        self.agent,
                    )
                    .await;
                assert_eq!(
                    after, before,
                    "the positive control uses the original admitted turn"
                );
            }
            Change::ReplaceFile => {
                assert!(matches!(owner, Owner::Document));
                let hash = self.keep(b"new contents after the lookup").await;
                assert!(
                    documents::replace_file(
                        &mut self.world.pool.acquire().await.unwrap(),
                        &documents::NewFile {
                            id: self.file,
                            document: self.document,
                            blob: hash,
                            name: FILE_NAME,
                            media_type: "application/octet-stream",
                            size: i64::try_from(b"new contents after the lookup".len()).unwrap(),
                            is_text: false,
                            added_by: Some(self.world.ada),
                            added_by_thread: None,
                            at: self.world.clock.now(),
                        }
                    )
                    .await
                    .unwrap()
                );
            }
            Change::ReplaceMessage => {
                let message = match owner {
                    Owner::NodeChat => self.messages[0],
                    Owner::PeopleChat => self.messages[1],
                    Owner::Document => panic!("a chat target"),
                };
                let hash = self.keep(&[0; BINARY.len()]).await;
                let mut conn = self.world.pool.acquire().await.unwrap();
                let row = orch_db::queries::chats::message(&mut conn, self.id(owner), message)
                    .await
                    .unwrap()
                    .unwrap();
                let mut parts: Vec<ChatPart> = serde_json::from_value(row.content).unwrap();
                let ChatPart::Attachment { blob, .. } = &mut parts[1] else {
                    panic!("the first carried file is a direct attachment")
                };
                *blob = hash;
                sqlx::query("UPDATE chat_messages SET content=$2 WHERE id=$1")
                    .bind(message)
                    .bind(serde_json::to_value(parts).unwrap())
                    .execute(&mut *conn)
                    .await
                    .unwrap();
            }
        }
    }
}

#[tokio::test]
async fn removed_files_messages_owners_and_parents_cannot_reuse_a_discovered_attachment_path() {
    for (owner, change) in [
        (Owner::Document, Change::DeleteFile),
        (Owner::Document, Change::DeleteOwner),
        (Owner::Document, Change::DeleteParent),
        (Owner::NodeChat, Change::DeleteMessage),
        (Owner::NodeChat, Change::DeleteOwner),
        (Owner::NodeChat, Change::DeleteParent),
        (Owner::PeopleChat, Change::DeleteMessage),
        (Owner::PeopleChat, Change::DeleteOwner),
        (Owner::PeopleChat, Change::DeleteParent),
    ] {
        let trace = CaseTrace::new("removed attachment", owner, Some(change));
        let library = Library::for_case(&trace).await;
        trace.phase("read", "discovering exact attachment path");
        let path = library.paths(owner).await.remove(0);
        trace.phase("mutation", "changing discovered owner or slot");
        library.change(owner, change).await;
        trace.phase("mutation", "committed");
        trace.phase("read", "waiting for native download refusal");
        let answer = library.download(&path, json!({})).await;
        trace.phase("read", "native refusal received");
        assert_eq!(refused(&answer).0, "not_found", "{owner:?} {change:?}");
        assert!(!refused(&answer).1.contains(FILE_NAME));
        assert_eq!(library.store.reads.load(Ordering::SeqCst), 0);
        library.empty();
        trace.drop_library(library).await;
    }
}

#[tokio::test]
async fn changes_while_the_blob_is_read_are_rechecked_before_any_workspace_write() {
    let mut cases = Vec::new();
    for owner in OWNERS {
        for change in [
            Change::RevokeRead,
            Change::RevokeExport,
            Change::DeleteOwner,
            Change::DeleteParent,
            Change::LowerLimit,
        ] {
            cases.push((owner, change));
        }
    }
    cases.extend([
        (Owner::Document, Change::DeleteFile),
        (Owner::Document, Change::ReplaceFile),
        (Owner::NodeChat, Change::DeleteMessage),
        (Owner::PeopleChat, Change::DeleteMessage),
        (Owner::NodeChat, Change::ReplaceMessage),
        (Owner::PeopleChat, Change::ReplaceMessage),
    ]);
    for (owner, change) in cases {
        let trace = CaseTrace::new("held blob authority recheck", owner, Some(change));
        let library = Library::for_case(&trace).await;
        trace.phase("read", "discovering exact attachment path");
        let path = library.paths(owner).await.remove(0);
        let (entered, resume) = library.store.pause_once();
        let reading = async {
            trace.phase("read", "waiting for native download result");
            let answer = library.download(&path, json!({})).await;
            trace.phase("read", "native result received");
            answer
        };
        let changing = async {
            trace.phase("mutation", "waiting for blob get");
            tokio::time::timeout(common::PATIENCE, entered)
                .await
                .expect("the attachment reaches the blob read")
                .unwrap();
            trace.phase("mutation", "blob get held; changing authority or slot");
            library.change(owner, change).await;
            trace.phase("mutation", "change committed; releasing blob get");
            resume.send(()).unwrap();
            trace.phase("mutation", "blob get released");
        };
        let (answer, ()) =
            match tokio::time::timeout(common::PATIENCE, async { tokio::join!(reading, changing) })
                .await
            {
                Ok(result) => result,
                Err(_) => {
                    trace
                        .failure(
                            &library,
                            "the rechecked attachment did not return within the original deadline",
                        )
                        .await
                }
            };
        let expected = match change {
            Change::RevokeExport => "not_downloadable",
            Change::ReplaceFile | Change::ReplaceMessage => "changed",
            Change::LowerLimit => "too_large",
            _ => "not_found",
        };
        assert_eq!(
            refused(&answer).0,
            expected,
            "{owner:?} {change:?}: {answer:?}"
        );
        if expected == "not_found" {
            assert!(!refused(&answer).1.contains(FILE_NAME));
        }
        library.empty();
        trace.drop_library(library).await;
    }
}
