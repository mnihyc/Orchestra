//! Actual chat export scope, ordinary membership, blob waits and bounded publication controls.
use super::*;
use futures::StreamExt;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{chats as rows, identity};
use orch_types::{Hash, Json, new_id};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, action: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":[action]}]})
}
async fn key(room: &Room, actor: &Who, rules: Vec<Json>) -> Key {
    let policy: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(policy.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *room.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: actor.id(),
            name: "Native chat export",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(policy)),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn take(room: &Room, secret: &str, chat: Id, query: ChatExportGetQuery) -> RawReply {
    room.world
        .app
        .call(ChatExportGet, &[&chat])
        .token(secret)
        .query(&query)
        .send_bytes()
        .await
}
fn ok(reply: RawReply) -> RawReply {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    reply
}

async fn without_native_export(room: &Room) {
    name(room, &room.bea, Some(false)).await;
    room.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &room.topic])
        .token(&room.ada.token)
        .body(&PageChange {
            rows: Some(vec![PageChangeRow {
                permission: "chat.export".into(),
                profile: None,
                inherit: Some(false),
                locked: None,
                add: None,
                update: None,
                remove: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .full_permission_page()
        .ok();
    assert!(!room.chat(&room.bea, room.chat).await.can.export);
}

#[tokio::test]
async fn exact_chat_read_keeps_native_formats_and_account_authority_without_lending_other_operations()
 {
    let room = Room::new().await;
    room.say(&room.ada, room.chat, "First native export words")
        .await;
    room.say(&room.ada, room.chat, "Second native export words")
        .await;
    for rules in [
        vec![rule("ChatExportGet", "manage", "chat", room.chat)],
        vec![rule("ChatGet", "read", "chat", room.chat)],
        vec![rule(
            "ChatExportGet",
            "read",
            "chat",
            room.chat_in_another_topic().await,
        )],
    ] {
        let k = key(&room, &room.ada, rules).await;
        assert_eq!(
            take(&room, &k.secret, room.chat, ChatExportGetQuery::default())
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(audited(&room).await, 0);
    let k = key(
        &room,
        &room.ada,
        vec![rule("ChatExportGet", "read", "chat", room.chat)],
    )
    .await;
    for format in [ExportFormat::Markdown, ExportFormat::Json] {
        let query = ChatExportGetQuery {
            format: Some(format),
            from: Some(2),
            to: Some(2),
            files: None,
        };
        let native = ok(take(&room, &room.ada.token, room.chat, query.clone()).await);
        let scoped = ok(take(&room, &k.secret, room.chat, query).await);
        assert_eq!(scoped.raw, native.raw);
        assert_eq!(
            scoped.headers["content-type"],
            native.headers["content-type"]
        );
        assert_eq!(
            scoped.headers["content-disposition"],
            native.headers["content-disposition"]
        );
    }
    let scoped = key(
        &room,
        &room.bea,
        vec![rule("ChatExportGet", "read", "chat", room.chat)],
    )
    .await;
    without_native_export(&room).await;
    assert_eq!(
        take(
            &room,
            &scoped.secret,
            room.chat,
            ChatExportGetQuery::default()
        )
        .await
        .status,
        StatusCode::FORBIDDEN,
        "scope never supplies the native node Export row"
    );
    assert_eq!(audited(&room).await, 4);
}

#[tokio::test]
async fn node_subtree_read_uses_the_actual_chat_placement_and_people_chats_keep_their_member_gate()
{
    let room = Room::new().await;
    let subtree = json!({"target":{"kind":"topic","id":room.topic,"reach":"subtree"},"resources":["chat"],"allow":[{"operation":"ChatExportGet","actions":["read"]}]});
    let local = key(&room, &room.ada, vec![subtree]).await;
    ok(take(
        &room,
        &local.secret,
        room.chat,
        ChatExportGetQuery::default(),
    )
    .await);
    let elsewhere = room.chat_in_another_topic().await;
    assert_eq!(
        take(
            &room,
            &local.secret,
            elsewhere,
            ChatExportGetQuery::default()
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let direct = room.direct(&room.bea, &room.cai).await.base.id;
    room.say(&room.bea, direct, "Native private export").await;
    let participant = key(
        &room,
        &room.bea,
        vec![rule("ChatExportGet", "read", "chat", direct)],
    )
    .await;
    ok(take(
        &room,
        &participant.secret,
        direct,
        ChatExportGetQuery::default(),
    )
    .await);
    let admin = key(
        &room,
        &room.dee,
        vec![rule("ChatExportGet", "read", "chat", direct)],
    )
    .await;
    assert_eq!(
        take(&room, &admin.secret, direct, ChatExportGetQuery::default())
            .await
            .status,
        StatusCode::FORBIDDEN,
        "ordinary admin inspection does not grant native people-chat export"
    );
}

async fn agent_message(room: &Room) -> Id {
    // This exported reference needs an actual stored thread, with no running model session.
    let thread = new_id();
    let mut setup = room.world.db.pool.begin().await.unwrap();
    assert!(
        orch_db::queries::threads::insert_thread(
            &mut setup,
            thread,
            room.topic,
            "Private export speaker",
            room.ada.id(),
            room.world.now()
        )
        .await
        .unwrap()
    );
    orch_authz::pages::start(
        &mut setup,
        orch_access_contract::pages::StoredKind::Node,
        thread,
        room.ada.id(),
        orch_types::Time::new(room.world.now()),
    )
    .await
    .unwrap();
    orch_db::queries::threads::make_node_chat(&mut setup, thread, room.ada.id(), room.world.now())
        .await
        .unwrap();
    setup.commit().await.unwrap();
    let mut tx = room.world.db.pool.begin().await.unwrap();
    let _ = rows::lock(&mut tx, room.chat).await.unwrap().unwrap();
    let seq = rows::next_seq(&mut tx, room.chat).await.unwrap().unwrap();
    rows::insert_message(
        &mut tx,
        &rows::NewMessage {
            id: new_id(),
            chat_id: room.chat,
            kind: "message",
            author_id: None,
            author_thread_id: Some(thread),
            author_agent: Some("Native agent"),
            content: &json!([text("An agent's exported words")]),
            reply_to_id: None,
            mentions: &[],
            mentions_all: false,
            client_id: None,
            search_text: Some("export words"),
            seq,
            at: room.world.now(),
        },
    )
    .await
    .unwrap();
    rows::note_message(&mut tx, room.chat, seq, room.world.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    thread
}

#[tokio::test]
async fn live_node_labels_need_this_exports_own_read_before_any_native_format_is_built() {
    let room = Room::new().await;
    let speaker = agent_message(&room).await;
    for format in [ExportFormat::Markdown, ExportFormat::Json] {
        let q = ChatExportGetQuery {
            format: Some(format),
            ..ChatExportGetQuery::default()
        };
        let native = ok(take(&room, &room.ada.token, room.chat, q.clone()).await);
        assert!(String::from_utf8_lossy(&native.raw).contains("Private export speaker"));
        for operation in ["NodeGet.read", "ChatExportGet"] {
            let k = key(
                &room,
                &room.ada,
                vec![
                    rule("ChatExportGet", "read", "chat", room.chat),
                    rule(operation, "read", "thread", speaker),
                ],
            )
            .await;
            let scoped = ok(take(&room, &k.secret, room.chat, q.clone()).await);
            assert_eq!(
                String::from_utf8_lossy(&scoped.raw).contains("Private export speaker"),
                operation == "ChatExportGet"
            );
            if operation == "ChatExportGet" {
                assert_eq!(scoped.raw, native.raw);
            }
        }
    }
}

#[tokio::test]
async fn the_actual_chat_row_wait_refreshes_key_lifetime_before_supply_or_audit() {
    for expire in [false, true] {
        let room = Room::new().await;
        room.say(&room.ada, room.chat, "Native locked export").await;
        let k = key(
            &room,
            &room.ada,
            vec![rule("ChatExportGet", "read", "chat", room.chat)],
        )
        .await;
        let mut lock = room.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let _ = rows::lock(&mut lock, room.chat).await.unwrap();
        let app = room.world.app.clone();
        let chat = room.chat;
        let secret = k.secret;
        let request = tokio::spawn(async move {
            let call = app
                .call(ChatExportGet, &[&chat])
                .token(&secret)
                .query(&ChatExportGetQuery::default());
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{assert!(!request.is_finished());let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM chats WHERE id%' AND query LIKE '%FOR UPDATE%')").bind(blocker).fetch_one(&room.world.db.pool).await.unwrap();if blocked{break;}tokio::task::yield_now().await;}}).await.expect("the native Chat lock must be reached");
        if expire {
            room.world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(audited(&room).await, 0);
        } else {
            ok(reply);
            assert_eq!(audited(&room).await, 1);
        }
    }
}

use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::{BlobError, BlobRead, BlobStore, ByteStream, Stored};
struct OpenGate {
    inner: Arc<dyn BlobStore>,
    hash: Hash,
    used: AtomicBool,
    calls: AtomicUsize,
    reached: Notify,
    release: Notify,
}
#[async_trait::async_trait]
impl BlobStore for OpenGate {
    async fn put(
        &self,
        hash: &Hash,
        size: Option<i64>,
        body: ByteStream,
    ) -> Result<Stored, BlobError> {
        self.inner.put(hash, size, body).await
    }
    async fn get(&self, hash: &Hash, range: Option<ByteRange>) -> Result<BlobRead, BlobError> {
        let opened = self.inner.get(hash, range).await?;
        if *hash == self.hash {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.used.swap(true, Ordering::SeqCst) {
                self.reached.notify_one();
                self.release.notified().await;
            }
        }
        Ok(opened)
    }
    async fn exists(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.exists(hash).await
    }
    async fn size(&self, hash: &Hash) -> Result<Option<i64>, BlobError> {
        self.inner.size(hash).await
    }
    async fn delete(&self, hash: &Hash) -> Result<bool, BlobError> {
        self.inner.delete(hash).await
    }
}
async fn attachment(room: &Room) -> Hash {
    let blobs = &room.world.app.app.blobs;
    let meta = blobs
        .put_bytes(Bytes::from_static(b"Native chat export file"))
        .await
        .unwrap();
    let mut conn = room.world.conn().await;
    blobs
        .register(&mut conn, meta, room.world.now())
        .await
        .unwrap();
    orch_db::queries::uploads::remember(&mut conn, room.ada.id(), &meta.hash, room.world.now())
        .await
        .unwrap();
    drop(conn);
    room.send(
        &room.ada,
        room.chat,
        vec![
            text("A supplied native file"),
            ChatPart::Attachment {
                blob: meta.hash,
                name: "export file.txt".into(),
                size: meta.size,
                media_type: "text/plain".into(),
            },
        ],
        "native-export-file",
    )
    .await
    .ok();
    meta.hash
}

fn archive_gate(room: &mut Room, hash: Hash) -> Arc<OpenGate> {
    use orch_api_core::{registry::Registry, state::AppState, testing::TestApp};
    let old = room.world.app.app.clone();
    let gate = Arc::new(OpenGate {
        inner: old.blobs.store(),
        hash,
        used: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        reached: Notify::new(),
        release: Notify::new(),
    });
    let engine =
        orch_api_core::thread_engine::detached(&old.db, &old.clock, &old.config, &old.link);
    engine.attach_blobs(gate.clone());
    let app = Arc::new(AppState {
        db: old.db.clone(),
        config: old.config.clone(),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine,
        realtime: old.realtime.clone(),
        build: old.build,
        blobs: old.blobs.clone(),
        cache: orch_api_core::cache::Cache::new(&old.config),
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
    });
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    room.world.app = TestApp::over(app, room.world.app.clock.clone(), registry);
    gate
}

#[tokio::test]
async fn archive_blob_io_is_once_only_and_a_closed_key_records_no_export_after_the_store_wait() {
    for expire in [false, true] {
        let mut room = Room::new().await;
        let hash = attachment(&room).await;
        let gate = archive_gate(&mut room, hash);
        let k = key(
            &room,
            &room.ada,
            vec![rule("ChatExportGet", "read", "chat", room.chat)],
        )
        .await;
        let app = room.world.app.clone();
        let chat = room.chat;
        let secret = k.secret;
        let request = tokio::spawn(async move {
            let call =
                app.call(ChatExportGet, &[&chat])
                    .token(&secret)
                    .query(&ChatExportGetQuery {
                        files: Some(ChatExportFiles::Include),
                        ..ChatExportGetQuery::default()
                    });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), gate.reached.notified())
            .await
            .expect("the actual native archive blob store get must be observed");
        if expire {
            room.world.advance(chrono::Duration::hours(2));
        }
        gate.release.notify_one();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        if expire {
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
            assert_eq!(audited(&room).await, 0);
        } else {
            let reply = ok(reply);
            assert_eq!(reply.headers["content-type"], "application/zip");
            let zip = zip::ZipArchive::new(std::io::Cursor::new(reply.raw.to_vec())).unwrap();
            assert!(zip.len() >= 2);
            assert_eq!(audited(&room).await, 1);
        }
        assert_eq!(
            gate.calls.load(Ordering::SeqCst),
            1,
            "neither archive supply nor blob IO is retried"
        );
    }
}

#[tokio::test]
async fn an_exported_snapshot_survives_unrelated_new_messages_but_its_bytes_stop_after_key_loss() {
    for mode in ["unchanged", "append", "expired", "scope", "deleted"] {
        let room = Room::new().await;
        let words = "Native bounded chat output ".repeat(900);
        let mut messages = Vec::new();
        for _ in 0..4 {
            // ASCII source: each native message respects its text bound while total bytes stay unchanged.
            for chunk in words.as_bytes().chunks(
                usize::try_from(orch_api_contract::chats::CHAT_MESSAGE_MAX_TEXT_CHARS).unwrap(),
            ) {
                messages.push(
                    room.say(&room.ada, room.chat, std::str::from_utf8(chunk).unwrap())
                        .await
                        .id,
                );
            }
        }
        let k = key(
            &room,
            &room.ada,
            vec![rule("ChatExportGet", "read", "chat", room.chat)],
        )
        .await;
        let response = room
            .world
            .app
            .call(ChatExportGet, &[&room.chat])
            .token(&k.secret)
            .query(&ChatExportGetQuery::default())
            .send_response()
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        let first = body.next().await.unwrap().unwrap();
        assert!(first.len() <= 32 * 1024);
        assert_eq!(audited(&room).await, 1);
        match mode {
            "append" => {
                room.say(
                    &room.ada,
                    room.chat,
                    "A later message outside the original native range",
                )
                .await;
            }
            "deleted" => {
                use orch_api_contract::chats::ChatMessageDelete;
                let reply = room
                    .world
                    .app
                    .call(ChatMessageDelete, &[&room.chat, &messages[0]])
                    .token(&room.ada.token)
                    .send_bytes()
                    .await;
                assert!(
                    reply.status.is_success(),
                    "{}",
                    String::from_utf8_lossy(&reply.raw)
                );
            }
            "expired" => room.world.advance(chrono::Duration::hours(2)),
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(k.id)
                    .bind(json!({"rules":[]}))
                    .execute(&room.world.db.pool)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let second = body
            .next()
            .await
            .expect("the original native export has more than one bounded frame");
        if mode == "expired" || mode == "scope" || mode == "deleted" {
            assert!(
                second.is_err(),
                "no old buffered body grants a delivery lease"
            );
        } else {
            let mut output = first.to_vec();
            output.extend_from_slice(&second.unwrap());
            while let Some(next) = body.next().await {
                output.extend_from_slice(&next.unwrap());
            }
            assert!(!String::from_utf8_lossy(&output).contains("A later message outside"));
        }
        assert_eq!(
            audited(&room).await,
            1,
            "delivery cannot replay the native export audit"
        );
    }
}

#[tokio::test]
async fn completed_export_audit_cannot_publish_old_bytes_after_an_actual_activity_wait_loses_authority()
 {
    for mode in ["unchanged", "append", "expired", "scope", "native-export"] {
        let room = Room::new().await;
        room.say(&room.ada, room.chat, "Private final export words")
            .await;
        let k = key(
            &room,
            &room.bea,
            vec![rule("ChatExportGet", "read", "chat", room.chat)],
        )
        .await;
        let mut lock = room.world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let app = room.world.app.clone();
        let chat = room.chat;
        let secret = k.secret;
        let request = tokio::spawn(async move {
            let call = app
                .call(ChatExportGet, &[&chat])
                .token(&secret)
                .query(&ChatExportGetQuery::default());
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{
            assert!(!request.is_finished(),"the native completed audit must reach final activity publication");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(blocker).fetch_one(&room.world.db.pool).await.unwrap();
            if waiting{break;}tokio::task::yield_now().await;
        }}).await.expect("the actual activity INSERT wait must be observed");
        assert_eq!(audited(&room).await, 1);
        match mode {
            "append" => {
                room.say(&room.ada, room.chat, "A new unrelated export message")
                    .await;
            }
            "expired" => room.world.advance(chrono::Duration::hours(2)),
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(k.id)
                    .bind(json!({"rules":[]}))
                    .execute(&room.world.db.pool)
                    .await
                    .unwrap();
            }
            "native-export" => without_native_export(&room).await,
            _ => {}
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if mode == "unchanged" || mode == "append" {
            let reply = ok(reply);
            assert!(String::from_utf8_lossy(&reply.raw).contains("Private final export words"));
            assert!(
                !String::from_utf8_lossy(&reply.raw).contains("A new unrelated export message")
            );
        } else {
            assert!(!reply.status.is_success());
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Private final export words"));
        }
        assert_eq!(
            audited(&room).await,
            1,
            "accepted audit is neither retried nor undone by a later read refusal"
        );
    }
}
