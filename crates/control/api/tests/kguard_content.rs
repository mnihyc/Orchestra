//! Content, destinations and usage writes use the authority held when their action begins.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use axum::body::Body;
use bytes::Bytes;
use chrono::Duration;
use desk::{Desk, paragraph, put, text};
use futures::stream;
use http::{Request, StatusCode};
use orch_api_contract::common::MAX_JSON_BYTES;
use orch_api_contract::documents::{
    DocumentDelete, DocumentDiscussionCreate, DocumentDiscussionCreateBody, DocumentFile,
    DocumentFileCreate, DocumentFileCreateQuery, NewDocumentAccess,
};
use orch_api_core::registry::Registry;
use orch_db::queries::access::{self, PageKey, testing};
use orch_db::queries::{documents, threads};
use orch_types::{Id, Json, Time, new_id};
use places::{Who, person};
use support::World;
use tower::ServiceExt;

// Private test services use the VM's assigned agent port range and avoid the shared stack.
const TEST_PORT_FIRST: u16 = 41000;
const TEST_PORT_COUNT: u16 = 1000;
// The shared development stack owns these ports even when it is not listening.
const SHARED_PORTS: std::ops::RangeInclusive<u16> = 41280..=41299;

struct Receiver {
    url: String,
    posts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl Receiver {
    async fn start(catalogue: Json) -> Self {
        let offset = u16::try_from(new_id().as_u128() % u128::from(TEST_PORT_COUNT)).unwrap();
        let mut chosen = None;
        for n in 0..TEST_PORT_COUNT {
            let port = TEST_PORT_FIRST + (offset + n) % TEST_PORT_COUNT;
            if SHARED_PORTS.contains(&port) {
                continue;
            }
            match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
                Ok(listener) => {
                    chosen = Some(listener);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {}
                Err(error) => panic!("local receiver: {error}"),
            }
        }
        let listener = chosen.expect("a free port in the private test range");
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let posts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let took = posts.clone();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let took = took.clone();
                async move {
                    took.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    StatusCode::NO_CONTENT
                }
            })
            .get(move || {
                let catalogue = catalogue.clone();
                async move { axum::Json(catalogue) }
            }),
        );
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            url,
            posts,
            stop,
            task,
        }
    }

    async fn finish(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap();
    }
}

fn install_webhook() {
    let mut notifiers = orch_worker_core::channels::Notifiers::new();
    notifiers
        .register(std::sync::Arc::new(
            orch_notifier_webhook::WebhookNotifier::new(),
        ))
        .unwrap();
    orch_api_core::routes::notifications::install_notifiers(notifiers);
    orch_api_core::routes::install_master_key(&orch_db::secrets::MasterKey::new([7; 32]));
}

// Cross a one-second entry deadline unambiguously, without waiting on wall time.
const ENTRY_SECONDS: i64 = 1;
const ELAPSED_SECONDS: i64 = 2;
// Scheduling headroom on a shared machine; actual PostgreSQL lock state is observed.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Clone)]
struct Call {
    method: &'static str,
    path: String,
    body: Option<Json>,
    filename: Option<&'static str>,
}

impl Call {
    fn json(method: &'static str, path: String, body: Json) -> Self {
        Self {
            method,
            path,
            body: Some(body),
            filename: None,
        }
    }

    fn empty(method: &'static str, path: String) -> Self {
        Self {
            method,
            path,
            body: None,
            filename: None,
        }
    }

    fn bytes(path: String) -> Self {
        Self {
            method: "POST",
            path,
            body: Some(Json::String("new file bytes".into())),
            filename: Some("new.txt"),
        }
    }
}

fn router(world: &World) -> axum::Router {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    registry.into_router().with_state(world.app.app.clone())
}

fn request(who: &Who, call: &Call, body: Body) -> Request<Body> {
    let mut builder = Request::builder()
        .method(call.method)
        .uri(&call.path)
        .header("authorization", format!("Bearer {}", who.token));
    if let Some(name) = call.filename {
        builder = builder
            .header("x-orch-filename", name)
            .header("content-type", "application/octet-stream");
    } else if call.body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder.body(body).unwrap()
}

async fn read_response(response: axum::response::Response) -> (StatusCode, Json) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_JSON_BYTES)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Json::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

async fn delayed(world: &World, who: &Who, call: Call, revoke_admin: bool) -> (StatusCode, Json) {
    let clock = world.app.clock.clone();
    let pool = world.db.pool.clone();
    let writer = who.id();
    if let Some(body) = &call.body {
        let bytes = if call.filename.is_some() {
            body.as_str().unwrap().as_bytes().to_vec()
        } else {
            serde_json::to_vec(body).unwrap()
        };
        let polled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sent = polled.clone();
        let body = Body::from_stream(stream::once(async move {
            sent.store(true, std::sync::atomic::Ordering::SeqCst);
            clock.advance(Duration::seconds(ELAPSED_SECONDS));
            if revoke_admin {
                let mut tx = pool.begin().await.unwrap();
                access::lock_system_defaults(&mut tx).await.unwrap();
                sqlx::query("UPDATE users SET is_system_admin = false WHERE id = $1")
                    .bind(writer)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                access::bump_epoch(&mut tx).await.unwrap();
                tx.commit().await.unwrap();
            }
            Ok::<_, std::io::Error>(Bytes::from(bytes))
        }));
        let response = router(world)
            .oneshot(request(who, &call, body))
            .await
            .unwrap();
        assert!(
            polled.load(std::sync::atomic::Ordering::SeqCst),
            "the initial check passed before the body was consumed: {}",
            call.path
        );
        read_response(response).await
    } else {
        // Bodyless endpoints have no body to delay. Hold the authority row so
        // their action waits after its preflight, then end the entry before it runs.
        let mut change = pool.begin().await.unwrap();
        // The request waits on the acting person's guard row, without a shared barrier.
        orch_db::queries::transaction_guard::lock_person(&mut change, who.id())
            .await
            .unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *change)
            .await
            .unwrap();
        let app = router(world);
        let req = request(who, &call, Body::empty());
        let action = tokio::spawn(async move { app.oneshot(req).await.unwrap() });
        tokio::time::timeout(LOCK_WAIT, async {
            loop {
                let blocked: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND pid <> $1 AND wait_event_type = 'Lock')"
                ).bind(blocker).fetch_one(&pool).await.unwrap();
                if blocked { break; }
                assert!(!action.is_finished(), "the action must reach the held authority lock: {}", call.path);
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual action reached its PostgreSQL lock");
        clock.advance(Duration::seconds(ELAPSED_SECONDS));
        if revoke_admin {
            sqlx::query("UPDATE users SET is_system_admin = false WHERE id = $1")
                .bind(writer)
                .execute(&mut *change)
                .await
                .unwrap();
            access::bump_epoch(&mut change).await.unwrap();
        }
        change.commit().await.unwrap();
        read_response(action.await.unwrap()).await
    }
}

async fn give(world: &World, who: &Who, kind: &str, object: Id, permission: &str, ends: bool) {
    testing::add_entry(
        &mut *world.conn().await,
        &PageKey::new(kind, object, "live"),
        "any",
        permission,
        ("user", Some(who.id()), None),
        ends.then_some(world.now() + Duration::seconds(ENTRY_SECONDS)),
        world.now(),
    )
    .await
    .unwrap();
}

async fn cut(world: &World, node: Id, permissions: &[&str]) {
    let mut conn = world.conn().await;
    for permission in permissions {
        testing::set_row(
            &mut conn,
            &PageKey::new("node", node, "live"),
            "any",
            permission,
            false,
            false,
        )
        .await
        .unwrap();
    }
}

async fn snapshot(world: &World, tables: &[&str]) -> Vec<Json> {
    let mut held = Vec::new();
    for table in tables {
        // Each name is a fixed business table supplied by the test, never request data.
        let query = format!(
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]'::jsonb) FROM {table} t"
        );
        held.push(
            sqlx::query_scalar(sqlx::AssertSqlSafe(query))
                .fetch_one(&world.db.pool)
                .await
                .unwrap(),
        );
    }
    held
}

fn denied(status: StatusCode, value: &Json, permission: &str, case: &str) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{case}: {value}");
    assert_eq!(value["code"], "authz.denied", "{case}: {value}");
    assert!(
        value.to_string().contains(permission),
        "the refusal names the missing {permission}: {case}: {value}"
    );
}

#[derive(Debug, Clone, Copy)]
enum DocumentAct {
    Create,
    Rename,
    Ops,
    Version,
    File,
    FileDelete,
    Seen,
    Discussion,
    Answer,
    Resolve,
    CommentEdit,
    CommentDelete,
    Reaction,
    Preview,
    Move,
    Delete,
    Restore,
}

#[tokio::test]
async fn every_document_action_refuses_authority_lost_during_arrival_and_accepts_authority_still_held()
 {
    use DocumentAct::*;
    for act in [
        Create,
        Rename,
        Ops,
        Version,
        File,
        FileDelete,
        Seen,
        Discussion,
        Answer,
        Resolve,
        CommentEdit,
        CommentDelete,
        Reaction,
        Preview,
        Move,
        Delete,
        Restore,
    ] {
        for loss in [true, false] {
            let desk = Desk::new().await;
            let writer = desk.member("writer", "viewer").await;
            let mut body = desk::body("Original");
            body.access = Some(NewDocumentAccess::Private);
            let made = desk.make_body(&desk.ada, body).await.ok();
            let id = made.base.id;
            let block = desk
                .paragraphs(&desk.ada, id, &["original words"])
                .await
                .remove(0);
            let permission = match act {
                Create => "doc.create",
                Rename | Ops | File | FileDelete | Resolve => "doc.edit",
                Version | CommentDelete | Preview | Move | Delete | Restore => "doc.manage",
                Seen | CommentEdit => "doc.view",
                Discussion | Answer | Reaction => "doc.comment",
            };
            if matches!(act, Create) {
                cut(
                    &desk.world,
                    desk.topic,
                    &["doc.create", "doc.edit", "doc.manage"],
                )
                .await;
                give(&desk.world, &writer, "node", desk.topic, permission, loss).await;
            } else {
                give(&desk.world, &writer, "document", id, permission, loss).await;
                if permission != "doc.view" {
                    give(&desk.world, &writer, "document", id, "doc.view", false).await;
                }
            }
            let root = format!("/documents/{id}");
            let call = match act {
                Create => Call::json(
                    "POST",
                    format!("/nodes/{}/documents", desk.topic),
                    serde_json::json!({"title": "New document"}),
                ),
                Rename => Call::json(
                    "PATCH",
                    root.clone(),
                    serde_json::json!({"title": "Renamed"}),
                ),
                Ops => Call::json(
                    "POST",
                    format!("{root}/ops"),
                    serde_json::json!({"client_id": new_id().to_string(), "ops": [put(paragraph("inserted"))]}),
                ),
                Version => {
                    let mut tx = desk.world.db.pool.begin().await.unwrap();
                    let row = documents::lock(&mut tx, id).await.unwrap().unwrap();
                    let version = documents::history::cut(
                        &mut tx,
                        &row,
                        orch_api_contract::documents::VersionCause::Idle,
                        desk.world.now(),
                    )
                    .await
                    .unwrap();
                    tx.commit().await.unwrap();
                    desk.write(&desk.ada, id, vec![desk::rewrite(&block, "changed")])
                        .await
                        .ok();
                    Call::empty("POST", format!("{root}/versions/{version}/restore"))
                }
                File => Call::bytes(format!("{root}/files")),
                FileDelete => {
                    let file: DocumentFile = desk
                        .world
                        .app
                        .call(DocumentFileCreate, &[&id])
                        .token(&desk.ada.token)
                        .query(&DocumentFileCreateQuery::default())
                        .header("x-orch-filename", "old.txt")
                        .header("content-type", "text/plain")
                        .bytes(b"old file")
                        .send()
                        .await
                        .ok();
                    Call::empty("DELETE", format!("{root}/files/{}", file.id))
                }
                Seen => Call::json(
                    "POST",
                    format!("{root}/seen"),
                    serde_json::json!({"seq": block.seq_updated}),
                ),
                Discussion => Call::json(
                    "POST",
                    format!("{root}/discussions"),
                    serde_json::json!({"block_id": block.id, "content": text("first"), "client_id": new_id().to_string()}),
                ),
                Answer | Resolve | CommentEdit | CommentDelete | Reaction => {
                    let discussion = desk
                        .world
                        .app
                        .call(DocumentDiscussionCreate, &[&id])
                        .token(&desk.ada.token)
                        .body(&DocumentDiscussionCreateBody {
                            block_id: block.id,
                            quote: None,
                            content: text("original comment"),
                            client_id: new_id().to_string(),
                        })
                        .send()
                        .await
                        .ok();
                    let comment = discussion.comments[0].id;
                    match act {
                        Answer => Call::json(
                            "POST",
                            format!("{root}/discussions/{}/comments", discussion.id),
                            serde_json::json!({"content": text("answer"), "client_id": new_id().to_string()}),
                        ),
                        Resolve => Call::json(
                            "POST",
                            format!("{root}/discussions/{}/resolve", discussion.id),
                            serde_json::json!({"resolved": true}),
                        ),
                        CommentEdit => {
                            sqlx::query(
                                "UPDATE document_comments SET author_id = $2 WHERE id = $1",
                            )
                            .bind(comment)
                            .bind(writer.id())
                            .execute(&desk.world.db.pool)
                            .await
                            .unwrap();
                            Call::json(
                                "PATCH",
                                format!("{root}/comments/{comment}"),
                                serde_json::json!({"content": text("edited")}),
                            )
                        }
                        CommentDelete => {
                            Call::empty("DELETE", format!("{root}/comments/{comment}"))
                        }
                        Reaction => Call::json(
                            "PUT",
                            format!("{root}/comments/{comment}/reaction"),
                            serde_json::json!({"emoji": "👍", "on": true}),
                        ),
                        _ => unreachable!(),
                    }
                }
                Preview | Move => {
                    let to = desk.another_topic("Destination").await;
                    Call::json(
                        "POST",
                        if matches!(act, Preview) {
                            format!("{root}/move/preview")
                        } else {
                            format!("{root}/move")
                        },
                        serde_json::json!({"node_id": to}),
                    )
                }
                Delete => Call::empty("DELETE", root),
                Restore => {
                    desk.world
                        .app
                        .call(DocumentDelete, &[&id])
                        .token(&desk.ada.token)
                        .send()
                        .await
                        .ok();
                    Call::empty("POST", format!("{root}/restore"))
                }
            };
            let tables = [
                "documents",
                "document_blocks",
                "document_ops",
                "document_versions",
                "document_files",
                "document_discussions",
                "document_comments",
                "document_states",
                "blobs",
                "audit_log",
            ];
            let before = snapshot(&desk.world, &tables).await;
            let (status, reply) = delayed(&desk.world, &writer, call, false).await;
            let after = snapshot(&desk.world, &tables).await;
            let case = format!("{act:?}, loss={loss}");
            if loss {
                denied(status, &reply, permission, &case);
                assert_eq!(
                    before, after,
                    "a refusal changes no document, block, discussion, version, file, pin, seen position or audit row: {case}"
                );
            } else {
                assert!(status.is_success(), "{case}: {status} {reply}");
                if matches!(act, Preview) {
                    assert_eq!(reply["pages"], 1);
                    assert_eq!(before, after, "preview leaves the document as it is");
                } else {
                    assert_ne!(
                        before, after,
                        "the authorized action actually changes its stored state: {case}"
                    );
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ThreadAct {
    Read,
    Attachment,
    Pin,
    Unpin,
    Draft,
    DraftDelete,
}

#[tokio::test]
async fn thread_read_upload_pin_and_draft_writes_use_current_authority_in_their_storage_transaction()
 {
    use ThreadAct::*;
    for act in [Read, Attachment, Pin, Unpin, Draft, DraftDelete] {
        for loss in [true, false] {
            let desk = Desk::new().await;
            let writer = desk.member("writer", "viewer").await;
            let thread = new_id();
            let item = new_id();
            let segment = new_id();
            {
                let mut tx = desk.world.db.pool.begin().await.unwrap();
                assert!(
                    threads::insert_thread(
                        &mut tx,
                        thread,
                        desk.topic,
                        "Work",
                        desk.ada.id(),
                        desk.world.now()
                    )
                    .await
                    .unwrap()
                );
                orch_authz::pages::start(
                    &mut tx,
                    orch_access_contract::pages::StoredKind::Node,
                    thread,
                    desk.ada.id(),
                    Time::new(desk.world.now()),
                )
                .await
                .unwrap();
                let seq = orch_db::sequences::next(&mut tx, thread)
                    .await
                    .unwrap()
                    .unwrap();
                threads::insert_segment(
                    &mut tx,
                    &threads::NewSegment {
                        id: segment,
                        thread_id: thread,
                        agent_id: new_id(),
                        provider_id: new_id(),
                        environment_id: new_id(),
                        env_generation: 1,
                        arrangement: "together",
                        executor: None,
                        tool_set: None,
                        credential_path: "native",
                        native_session_ref: None,
                        started_by: Some(desk.ada.id()),
                        started_at: desk.world.now(),
                    },
                    seq,
                )
                .await
                .unwrap();
                threads::insert_item(&mut tx, &threads::NewItem {
                    id: item, thread_id: thread, segment_id: segment, turn_id: None, stream: "main", parent_item_id: None,
                    kind: "user_message", status: "completed", author_id: Some(desk.ada.id()),
                    payload: &serde_json::json!({"parts": [{"type": "text", "text": "fixture"}], "input_id": new_id(), "delivery": "queue", "source": "user"}),
                    native_ref: None, search_text: None, at: desk.world.now(),
                }, seq).await.unwrap();
                if matches!(act, Unpin) {
                    threads::pin_item(&mut tx, item, Some((desk.ada.id(), desk.world.now())), seq)
                        .await
                        .unwrap();
                }
                if matches!(act, DraftDelete) {
                    threads::set_draft(
                        &mut tx,
                        writer.id(),
                        thread,
                        &serde_json::json!([{"type": "text", "text": "original draft"}]),
                        "queue",
                        desk.world.now(),
                    )
                    .await
                    .unwrap();
                }
                tx.commit().await.unwrap();
            }
            cut(
                &desk.world,
                thread,
                &["thread.view", "thread.interact", "thread.manage"],
            )
            .await;
            let permission = if matches!(act, Read) {
                "thread.view"
            } else {
                "thread.interact"
            };
            give(&desk.world, &writer, "node", thread, permission, loss).await;
            let root = format!("/threads/{thread}");
            let call = match act {
                Read => Call::json(
                    "POST",
                    format!("{root}/read"),
                    serde_json::json!({"seq": 1}),
                ),
                Attachment => Call::bytes(format!("{root}/attachments")),
                Pin => Call::empty("PUT", format!("{root}/items/{item}/pin")),
                Unpin => Call::empty("DELETE", format!("{root}/items/{item}/pin")),
                Draft => Call::json(
                    "PUT",
                    format!("{root}/draft"),
                    serde_json::json!({"content": [{"type": "text", "text": "draft"}], "delivery": "queue"}),
                ),
                DraftDelete => Call::empty("DELETE", format!("{root}/draft")),
            };
            let tables = ["threads", "items", "thread_reads", "drafts", "blobs"];
            let before = snapshot(&desk.world, &tables).await;
            let (status, reply) = delayed(&desk.world, &writer, call, false).await;
            let after = snapshot(&desk.world, &tables).await;
            let case = format!("{act:?}, loss={loss}");
            if loss {
                denied(status, &reply, permission, &case);
                assert_eq!(
                    before, after,
                    "refusal changes no read, draft, pin, sequence or blob reference: {case}"
                );
            } else {
                assert!(status.is_success(), "{case}: {status} {reply}");
                assert_ne!(
                    before, after,
                    "the authorized action changes its state: {case}"
                );
                match act {
                    Pin => assert_eq!(reply["pinned"]["by"]["id"], writer.id().to_string()),
                    Unpin => assert!(reply["pinned"].is_null()),
                    Read => assert_eq!(reply["last_read_seq"], 1),
                    _ => {}
                }
            }
        }
    }
}

#[tokio::test]
async fn a_recent_visit_is_written_only_while_the_person_can_still_view_the_place() {
    for loss in [true, false] {
        let desk = Desk::new().await;
        let writer = desk.member("writer", "viewer").await;
        cut(
            &desk.world,
            desk.topic,
            &["topic.view", "topic.interact", "topic.manage"],
        )
        .await;
        give(&desk.world, &writer, "node", desk.topic, "topic.view", loss).await;
        let tables = ["recents"];
        let before = snapshot(&desk.world, &tables).await;
        let (status, reply) = delayed(
            &desk.world,
            &writer,
            Call::json(
                "POST",
                "/me/recents".into(),
                serde_json::json!({"node_id": desk.topic}),
            ),
            false,
        )
        .await;
        let after = snapshot(&desk.world, &tables).await;
        if loss {
            denied(status, &reply, "topic.view", "recent visit");
            assert_eq!(before, after);
        } else {
            assert_eq!(status, StatusCode::NO_CONTENT);
            assert_ne!(before, after);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum NotificationAct {
    SystemChannel,
    OrgChannel,
    ChannelUpdate,
    ChannelDelete,
    ChannelTest,
    Destination,
    DestinationUpdate,
    DestinationDelete,
    DestinationTest,
}

#[tokio::test]
async fn every_channel_and_destination_action_rechecks_its_current_manager_before_writing_or_sending()
 {
    use NotificationAct::*;
    install_webhook();
    let receiver = Receiver::start(Json::Null).await;
    for act in [
        SystemChannel,
        OrgChannel,
        ChannelUpdate,
        ChannelDelete,
        ChannelTest,
        Destination,
        DestinationUpdate,
        DestinationDelete,
        DestinationTest,
    ] {
        for loss in [true, false] {
            let desk = Desk::new().await;
            let writer = desk.member("writer", "viewer").await;
            desk.world
                .set(
                    "outbound.allowed_private_hosts",
                    serde_json::json!(["127.0.0.1"]),
                )
                .await;
            let system = matches!(act, SystemChannel);
            if system {
                desk.world.make_admin(&writer.person).await;
                // A system admin acts as one with the session's As admin switch on.
                places::admin_mode(&desk.world, &writer, true).await;
            } else {
                give(
                    &desk.world,
                    &writer,
                    "node",
                    desk.org,
                    "settings.manage",
                    loss,
                )
                .await;
            }
            let channel_body = serde_json::json!({"kind": "webhook", "name": "Team channel", "config": {"url": receiver.url}});
            let destination_body =
                serde_json::json!({"name": "Team destination", "config": {"url": receiver.url}});
            let channel = if matches!(act, ChannelUpdate | ChannelDelete | ChannelTest) {
                Some(
                    desk.world
                        .app
                        .call(
                            orch_api_contract::notifications::OrgNotificationChannelCreate,
                            &[&desk.org],
                        )
                        .token(&desk.ada.token)
                        .raw_body(&channel_body)
                        .send()
                        .await
                        .ok()
                        .id,
                )
            } else {
                None
            };
            let destination =
                if matches!(act, DestinationUpdate | DestinationDelete | DestinationTest) {
                    Some(
                        desk.world
                            .app
                            .call(
                                orch_api_contract::notifications::NodeDestinationCreate,
                                &[&desk.org],
                            )
                            .token(&desk.ada.token)
                            .raw_body(&destination_body)
                            .send()
                            .await
                            .ok()
                            .id,
                    )
                } else {
                    None
                };
            let call = match act {
                SystemChannel => {
                    Call::json("POST", "/admin/notification-channels".into(), channel_body)
                }
                OrgChannel => Call::json(
                    "POST",
                    format!("/orgs/{}/notification-channels", desk.org),
                    channel_body,
                ),
                ChannelUpdate => Call::json(
                    "PATCH",
                    format!("/notification-channels/{}", channel.unwrap()),
                    serde_json::json!({"name": "Changed channel"}),
                ),
                ChannelDelete => Call::empty(
                    "DELETE",
                    format!("/notification-channels/{}", channel.unwrap()),
                ),
                ChannelTest => Call::json(
                    "POST",
                    format!("/notification-channels/{}/test", channel.unwrap()),
                    serde_json::json!({}),
                ),
                Destination => Call::json(
                    "POST",
                    format!("/nodes/{}/destinations", desk.org),
                    destination_body,
                ),
                DestinationUpdate => Call::json(
                    "PATCH",
                    format!("/destinations/{}", destination.unwrap()),
                    serde_json::json!({"name": "Changed destination"}),
                ),
                DestinationDelete => {
                    Call::empty("DELETE", format!("/destinations/{}", destination.unwrap()))
                }
                DestinationTest => Call::empty(
                    "POST",
                    format!("/destinations/{}/test", destination.unwrap()),
                ),
            };
            let tables = [
                "notification_channels",
                "destinations",
                "secrets",
                "audit_log",
            ];
            let before = snapshot(&desk.world, &tables).await;
            let posts = receiver.posts.load(std::sync::atomic::Ordering::SeqCst);
            let (status, reply) = delayed(&desk.world, &writer, call, loss && system).await;
            let after = snapshot(&desk.world, &tables).await;
            let sent = receiver.posts.load(std::sync::atomic::Ordering::SeqCst) - posts;
            let case = format!("{act:?}, loss={loss}");
            if loss {
                denied(
                    status,
                    &reply,
                    if system {
                        "system.admin"
                    } else {
                        "settings.manage"
                    },
                    &case,
                );
                assert_eq!(
                    before, after,
                    "the refused action writes no channel, destination, secret, result or audit row: {case}"
                );
                assert_eq!(sent, 0, "the refused action sends no sample: {case}");
            } else {
                assert!(status.is_success(), "{case}: {status} {reply}");
                assert_ne!(before, after, "the action changes its stored state: {case}");
                if matches!(act, ChannelTest | DestinationTest) {
                    assert_eq!(reply["ok"], true, "the real webhook accepted the sample");
                    assert_eq!(sent, 1);
                } else {
                    assert_eq!(sent, 0);
                }
            }
        }
    }
    receiver.finish().await;
}

#[derive(Debug, Clone, Copy)]
enum CapHome {
    Node,
    Provider,
    ProviderAndNode,
    System,
}

#[tokio::test]
async fn creating_updating_and_removing_a_cap_checks_the_authority_of_its_current_stored_home() {
    for home in [
        CapHome::Node,
        CapHome::Provider,
        CapHome::ProviderAndNode,
        CapHome::System,
    ] {
        for method in ["POST", "PATCH", "DELETE"] {
            for loss in [true, false] {
                let desk = Desk::new().await;
                let writer = desk.member("writer", "viewer").await;
                let provider = if matches!(home, CapHome::Provider | CapHome::ProviderAndNode) {
                    let mut conn = desk.world.conn().await;
                    let id = orch_db::queries::resources::access_fixtures::insert_resource(
                        &mut conn,
                        "provider",
                        Some(desk.ada.id()),
                        Some(desk.org),
                        "Provider",
                    )
                    .await
                    .unwrap();
                    orch_authz::pages::start(
                        &mut conn,
                        orch_access_contract::pages::StoredKind::Provider,
                        id,
                        desk.ada.id(),
                        Time::new(desk.world.now()),
                    )
                    .await
                    .unwrap();
                    Some(id)
                } else {
                    None
                };
                let node =
                    matches!(home, CapHome::Node | CapHome::ProviderAndNode).then_some(desk.org);
                let permission = match home {
                    CapHome::Node => {
                        give(&desk.world, &writer, "node", desk.org, "usage.manage", loss).await;
                        "usage.manage"
                    }
                    CapHome::Provider => {
                        give(
                            &desk.world,
                            &writer,
                            "provider",
                            provider.unwrap(),
                            "provider.manage",
                            loss,
                        )
                        .await;
                        "provider.manage"
                    }
                    CapHome::ProviderAndNode => {
                        give(
                            &desk.world,
                            &writer,
                            "provider",
                            provider.unwrap(),
                            "provider.manage",
                            false,
                        )
                        .await;
                        cut(
                            &desk.world,
                            desk.org,
                            &["org.view", "org.interact", "org.manage"],
                        )
                        .await;
                        give(&desk.world, &writer, "node", desk.org, "org.view", loss).await;
                        "org.view"
                    }
                    CapHome::System => {
                        desk.world.make_admin(&writer.person).await;
                        desk.world.make_admin(&desk.ada.person).await;
                        // A system admin acts as one with the session's As admin switch on.
                        places::admin_mode(&desk.world, &writer, true).await;
                        places::admin_mode(&desk.world, &desk.ada, true).await;
                        "system.admin"
                    }
                };
                let body = serde_json::json!({"node_id": node, "provider_id": provider, "subject": {"kind": "everyone"}, "split": "shared", "meter": "tokens", "period": "day", "amount": "1000"});
                let cap = if method == "POST" {
                    None
                } else {
                    Some(
                        desk.world
                            .app
                            .call(orch_api_contract::usage::UsageCapCreate, &[])
                            .token(&desk.ada.token)
                            .raw_body(&body)
                            .send()
                            .await
                            .ok(),
                    )
                };
                let call = match (method, cap) {
                    ("POST", _) => Call::json("POST", "/usage/caps".into(), body),
                    ("PATCH", Some(cap)) => Call::json(
                        "PATCH",
                        format!("/usage/caps/{}", cap.id),
                        serde_json::json!({"amount": "2000", "expected_revision": cap.revision}),
                    ),
                    ("DELETE", Some(cap)) => {
                        Call::empty("DELETE", format!("/usage/caps/{}", cap.id))
                    }
                    _ => unreachable!(),
                };
                let tables = ["usage_caps", "audit_log"];
                let before = snapshot(&desk.world, &tables).await;
                let (status, reply) = delayed(
                    &desk.world,
                    &writer,
                    call,
                    loss && matches!(home, CapHome::System),
                )
                .await;
                let after = snapshot(&desk.world, &tables).await;
                let case = format!("{home:?} {method}, loss={loss}");
                if loss {
                    denied(status, &reply, permission, &case);
                    assert_eq!(
                        before, after,
                        "no cap or audit row is written after authority ends: {case}"
                    );
                } else {
                    assert!(status.is_success(), "{case}: {status} {reply}");
                    assert_ne!(
                        before, after,
                        "the cap action actually changes state: {case}"
                    );
                }
            }
        }
    }
}

fn catalogue() -> Json {
    serde_json::json!({"format": 1, "as_of": "2026-10-10", "unit": "USD", "families": [{"match": "model*", "vendor": "Example", "source": "https://example.test/pricing", "as_of": "2026-10-10", "rate": {"input_per_mtok": "1", "cached_input_per_mtok": "0.1", "output_per_mtok": "5"}}]})
}

#[tokio::test]
async fn uploading_fetching_and_restoring_the_catalogue_requires_a_current_browser_administrator() {
    let receiver = Receiver::start(catalogue()).await;
    for mode in ["upload", "fetch", "shipped"] {
        for loss in [true, false] {
            // Keep the tiny release fixture inside this checkout; its migrations
            // setting selects the same shipped-file path a release uses.
            let scratch = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.tmp");
            std::fs::create_dir_all(&scratch).unwrap();
            let release = tempfile::tempdir_in(scratch).unwrap();
            std::fs::create_dir_all(release.path().join("share")).unwrap();
            std::fs::write(
                release.path().join("share/model-rates.json"),
                catalogue().to_string(),
            )
            .unwrap();
            let mut config = orch_config::ServerConfig::default();
            config.database.migrations_dir =
                release.path().join("migrations").display().to_string();
            let world = World::with(config).await;
            let writer = person(&world, "writer").await;
            world.make_admin(&writer.person).await;
            // A system admin acts as one with the session's As admin switch on.
            places::admin_mode(&world, &writer, true).await;
            // Seed the loopback transport directly: this tests the real fetch
            // and guarded loader, while the settings writer requires HTTPS.
            sqlx::query("INSERT INTO settings (node_id, key, value) VALUES (NULL, 'usage.catalogue_url', $1)")
                .bind(Json::String(receiver.url.clone()))
                .execute(&world.db.pool)
                .await
                .unwrap();
            let body = match mode {
                "upload" => catalogue(),
                "fetch" => serde_json::json!({"fetch": true}),
                "shipped" => serde_json::json!({"shipped": true}),
                _ => unreachable!(),
            };
            let tables = ["rate_catalogue", "audit_log"];
            let before = snapshot(&world, &tables).await;
            let (status, reply) = delayed(
                &world,
                &writer,
                Call::json("POST", "/admin/usage/catalogue".into(), body),
                loss,
            )
            .await;
            let after = snapshot(&world, &tables).await;
            if loss {
                denied(status, &reply, "system.admin", mode);
                assert_eq!(
                    before, after,
                    "the refused catalogue replacement writes no figures or audit row"
                );
            } else {
                assert_eq!(status, StatusCode::OK, "{mode}: {reply}");
                assert_eq!(
                    reply["origin"],
                    if mode == "upload" {
                        "uploaded"
                    } else if mode == "fetch" {
                        "fetched"
                    } else {
                        "shipped"
                    }
                );
                assert_eq!(reply["loaded_by"]["id"], writer.id().to_string());
                assert_ne!(before, after);
            }
        }
    }
    receiver.finish().await;
}

#[tokio::test]
async fn a_document_move_and_its_preview_recheck_creation_at_the_destination_after_the_body_arrives()
 {
    for preview in [true, false] {
        for loss in [true, false] {
            let desk = Desk::new().await;
            let writer = desk.member("writer", "viewer").await;
            let mut body = desk::body("Movable");
            body.access = Some(NewDocumentAccess::Private);
            let made = desk.make_body(&desk.ada, body).await.ok();
            let id = made.base.id;
            let to = desk.another_topic("Destination").await;
            give(&desk.world, &writer, "document", id, "doc.manage", false).await;
            cut(&desk.world, to, &["doc.create", "doc.edit", "doc.manage"]).await;
            give(&desk.world, &writer, "node", to, "doc.create", loss).await;
            let before = snapshot(&desk.world, &["documents", "audit_log"]).await;
            let call = Call::json(
                "POST",
                format!(
                    "/documents/{id}/move{}",
                    if preview { "/preview" } else { "" }
                ),
                serde_json::json!({"node_id": to}),
            );
            let (status, reply) = delayed(&desk.world, &writer, call, false).await;
            let after = snapshot(&desk.world, &["documents", "audit_log"]).await;
            if loss {
                denied(status, &reply, "doc.create", "move destination");
                assert_eq!(before, after);
            } else {
                assert_eq!(status, StatusCode::OK, "{reply}");
                if preview {
                    assert_eq!(before, after);
                } else {
                    assert_eq!(
                        documents::get(&mut *desk.world.conn().await, id)
                            .await
                            .unwrap()
                            .unwrap()
                            .node_id,
                        to
                    );
                }
            }
        }
    }
}
