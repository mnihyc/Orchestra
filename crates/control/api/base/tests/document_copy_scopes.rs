//! Native text copies bind their real sources before material and retain exact completed output.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use desk::Desk;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::documents::{
    DocumentDelete, DocumentFileCreate, DocumentFileCreateQuery, NewDocumentAccess,
    NodeDocumentCopy, NodeDocumentCopyBody,
};
use orch_api_contract::threads::FILENAME_HEADER;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;

const OP: &str = "NodeDocumentCopy";
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
async fn key(desk: &Desk, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Native text copy scope",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    key
}
fn body(source: Id, client: &str) -> NodeDocumentCopyBody {
    NodeDocumentCopyBody {
        source_document_id: source,
        title: Some("Scoped copied document".into()),
        access: Some(NewDocumentAccess::Private),
        client_id: client.into(),
    }
}
fn copied(desk: &Desk, destination: Id, body: &NodeDocumentCopyBody) -> Id {
    orch_db::queries::documents::copy::identity(desk.ada.id(), destination, &body.client_id, "copy")
}
fn rights(source: Id, destination: Id, read: bool) -> Vec<Json> {
    let mut rules = vec![
        rule(OP, "document", source, "self", &["read"]),
        rule(OP, "topic", destination, "self", &["write"]),
    ];
    if read {
        rules.push(rule(OP, "topic", destination, "subtree", &["read"]));
    }
    rules
}
async fn copy(desk: &Desk, token: &str, destination: Id, body: &NodeDocumentCopyBody) -> RawReply {
    let request = desk
        .world
        .app
        .call(NodeDocumentCopy, &[&destination])
        .token(token)
        .body(body);
    request.send_bytes().await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn stored(desk: &Desk, id: Id) -> Option<(String, i64, i64, i64, i64)> {
    sqlx::query_as("SELECT d.xmin::text,(SELECT count(*) FROM document_blocks WHERE document_id=d.id),(SELECT count(*) FROM document_files WHERE document_id=d.id),(SELECT count(*) FROM audit_log WHERE object_kind='document' AND object_id=d.id),(SELECT count(*) FROM document_tree_operations WHERE request_ref=d.id::text OR request_ref='text-create:' || d.id::text) FROM documents d WHERE d.id=$1").bind(id).fetch_optional(&desk.world.db.pool).await.unwrap()
}
async fn activity_wait(desk: &Desk, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    wait(desk, pid, request, "%INSERT INTO api_token_activity%").await
}
async fn wait(desk: &Desk, pid: i32, request: &tokio::task::JoinHandle<RawReply>, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            assert!(!request.is_finished(),"the native copy did not reach its real lock wait");
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&desk.world.db.pool).await.unwrap();
            if blocked {break;} tokio::task::yield_now().await;
        }
    }).await.expect("the literal blocker and real native query must be observed");
}
async fn source(desk: &Desk) -> Id {
    let mut supplied = desk::body("Private copy source");
    supplied.access = Some(NewDocumentAccess::Private);
    let id = desk.make_body(&desk.ada, supplied).await.ok().base.id;
    let _blocks = desk
        .paragraphs(&desk.ada, id, &["Current copied native text"])
        .await;
    id
}

#[tokio::test]
async fn copy_requires_the_actual_source_read_and_destination_write_without_operation_or_kind_loans()
 {
    let desk = Desk::new().await;
    let source = source(&desk).await;
    let destination = desk.another_topic("Copy destination").await;
    for rules in [
        vec![rule(OP, "document", source, "self", &["read"])],
        vec![rule(OP, "topic", destination, "self", &["write"])],
        vec![
            rule("DocumentGet.read", "document", source, "self", &["read"]),
            rule(OP, "topic", destination, "self", &["write"]),
        ],
        vec![
            rule(OP, "document_container", source, "self", &["read"]),
            rule(OP, "topic", destination, "self", &["write"]),
        ],
        vec![
            rule(OP, "document", source, "self", &["write"]),
            rule(OP, "topic", destination, "self", &["read"]),
        ],
    ] {
        let key = key(&desk, desk.ada.id(), rules).await;
        let body = body(source, &new_id().to_string());
        let id = copied(&desk, destination, &body);
        let reply = copy(&desk, &key.secret, destination, &body).await;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert!(stored(&desk, id).await.is_none());
    }
    let bea = desk.member("copyviewer", "developer").await;
    let key = key(&desk, bea.id(), rights(source, destination, true)).await;
    assert_eq!(
        copy(
            &desk,
            &key.secret,
            destination,
            &body(source, "native-private-door")
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn copy_write_only_receipt_keeps_native_material_and_its_own_read_restores_full_content() {
    let desk = Desk::new().await;
    let source = source(&desk).await;
    let destination = desk.another_topic("Copy destination").await;
    let mut rules = rights(source, destination, false);
    rules.push(rule(
        "DocumentGet.read",
        "topic",
        destination,
        "subtree",
        &["read"],
    ));
    let writer = key(&desk, desk.ada.id(), rules).await;
    let body = body(source, "copy-write-only");
    let id = copied(&desk, destination, &body);
    assert_eq!(
        accepted(copy(&desk, &writer.secret, destination, &body).await),
        json!({"projection":"receipt","document_id":id})
    );
    assert_eq!(
        desk.words(&desk.ada, id).await,
        ["Current copied native text"]
    );
    let before = stored(&desk, id).await.unwrap();
    assert_eq!(
        accepted(copy(&desk, &writer.secret, destination, &body).await),
        json!({"projection":"receipt","document_id":id})
    );
    assert_eq!(stored(&desk, id).await.unwrap(), before);
    let full = key(&desk, desk.ada.id(), rights(source, destination, true)).await;
    let value = accepted(copy(&desk, &full.secret, destination, &body).await);
    assert_eq!(value["id"], json!(id));
    assert_eq!(value["title"], "Scoped copied document");
    assert_eq!(stored(&desk, id).await.unwrap(), before);
}

#[tokio::test]
async fn copy_checks_source_scope_before_native_material_limits_can_reveal_existing_files() {
    let desk = Desk::new().await;
    let source = source(&desk).await;
    let destination = desk.another_topic("Copy destination").await;
    let file = desk
        .world
        .app
        .call(DocumentFileCreate, &[&source])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery { overwrite: None })
        .header(FILENAME_HEADER, "source-limit.bin")
        .bytes(b"source bytes")
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native creation control requires its full response");
    // A factual current file size above the destination limit is checked by the native material planner.
    let mut conn = desk.world.conn().await;
    orch_db::queries::settings::put(
        &mut conn,
        Some(destination),
        orch_settings_contract::registry::keys::DOCS_FILE_MAX_BYTES,
        &orch_db::queries::settings::Stored {
            value: &json!(1),
            locked: false,
            by: Some(desk.ada.id()),
            at: desk.world.now(),
        },
        None,
    )
    .await
    .unwrap();
    drop(conn);
    let body = body(source, "material-source-admission");
    let id = copied(&desk, destination, &body);
    let write = key(
        &desk,
        desk.ada.id(),
        vec![rule(OP, "topic", destination, "self", &["write"])],
    )
    .await;
    assert_eq!(
        copy(&desk, &write.secret, destination, &body).await.status,
        StatusCode::FORBIDDEN
    );
    let readable = key(&desk, desk.ada.id(), rights(source, destination, false)).await;
    let reply = copy(&desk, &readable.secret, destination, &body).await;
    assert!(!reply.status.is_success());
    let problem: orch_api_contract::common::Problem = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(
        problem.code,
        orch_api_contract::common::ErrorCode::DocsTooLarge
    );
    assert!(stored(&desk, id).await.is_none());
    assert_eq!(
        orch_db::queries::documents::file(&mut *desk.world.conn().await, source, file.id)
            .await
            .unwrap()
            .unwrap()
            .file
            .size,
        12
    );
}

#[tokio::test]
async fn copy_current_key_is_checked_after_actual_scope_source_row_and_request_namespace_waits() {
    for stage in ["scope", "source", "request"] {
        let desk = Desk::new().await;
        let source = source(&desk).await;
        let destination = desk.another_topic("Copy destination").await;
        let body = body(source, &format!("copy-{stage}-wait"));
        let id = copied(&desk, destination, &body);
        let token = key(&desk, desk.ada.id(), rights(source, destination, true)).await;
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        let pattern = match stage {
            "scope" => {
                sqlx::query("SELECT node_id FROM document_tree_scopes WHERE node_id=$1 FOR UPDATE")
                    .bind(desk.topic)
                    .fetch_one(&mut *blocker)
                    .await
                    .unwrap();
                "%SELECT document_tree_lock(%"
            }
            "source" => {
                sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
                    .bind(source)
                    .fetch_one(&mut *blocker)
                    .await
                    .unwrap();
                "%SELECT id FROM documents WHERE id=ANY%"
            }
            _ => {
                orch_db::queries::chats::lock_named(
                    &mut blocker,
                    &format!("document.tree:person:{}:{}", desk.ada.id(), id),
                )
                .await
                .unwrap();
                "%pg_advisory_xact_lock%"
            }
        };
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = desk.world.app.clone();
        let secret = token.secret.clone();
        let held = body.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(NodeDocumentCopy, &[&destination])
                .token(&secret)
                .body(&held);
            request.send_bytes().await
        });
        wait(&desk, pid, &request, pattern).await;
        desk.world.advance(chrono::Duration::hours(2));
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert!(stored(&desk, id).await.is_none());
    }
}

#[tokio::test]
async fn copy_completed_reply_does_not_reauthorize_original_reads_or_repeat_committed_material() {
    for change in [
        "none",
        "original_read_closed",
        "result_read_closed",
        "native_view_closed",
        "expired",
    ] {
        let desk = Desk::new().await;
        let source = source(&desk).await;
        let destination = desk.another_topic("Copy destination").await;
        let body = body(source, &format!("copy-held-{change}"));
        let id = copied(&desk, destination, &body);
        let token = key(&desk, desk.ada.id(), rights(source, destination, true)).await;
        let new_owner = desk.member("copyrecipient", "viewer").await;
        let mut blocker = desk.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = desk.world.app.clone();
        let secret = token.secret.clone();
        let held = body.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(NodeDocumentCopy, &[&destination])
                .token(&secret)
                .body(&held);
            request.send_bytes().await
        });
        activity_wait(&desk, pid, &request).await;
        let before = stored(&desk, id).await.unwrap();
        if change == "expired" {
            desk.world.advance(chrono::Duration::hours(2));
        } else if change == "native_view_closed" {
            let unreadable = desk
                .world
                .app
                .call(orch_api_contract::documents::DocumentGet, &[&id])
                .token(&new_owner.token)
                .send_bytes()
                .await;
            assert_eq!(
                unreadable.status,
                StatusCode::FORBIDDEN,
                "place membership alone must not open this private copy"
            );
            let mut reassigned = desk.world.db.pool.begin().await.unwrap();
            let changed = sqlx::query("UPDATE permission_entries SET holder_id=$2 WHERE object_kind='document' AND object_id=$1 AND permission='access.own' AND holder_id=$3")
                .bind(id).bind(new_owner.id()).bind(desk.ada.id()).execute(&mut *reassigned).await.unwrap().rows_affected();
            assert!(changed > 0);
            orch_db::queries::access::bump_epoch(&mut reassigned)
                .await
                .unwrap();
            reassigned.commit().await.unwrap();
        } else if change != "none" {
            let rules = if change == "original_read_closed" {
                vec![
                    rule(OP, "topic", destination, "self", &["write"]),
                    rule(OP, "topic", destination, "subtree", &["read"]),
                ]
            } else {
                rights(source, destination, false)
            };
            let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(json!(scope))
                .execute(&desk.world.db.pool)
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let value = accepted(
            tokio::time::timeout(std::time::Duration::from_secs(30), request)
                .await
                .unwrap()
                .unwrap(),
        );
        if change == "none" || change == "original_read_closed" {
            assert_eq!(value["id"], json!(id));
            assert_eq!(value["title"], "Scoped copied document");
        } else {
            assert_eq!(value, json!({"projection":"receipt","document_id":id}));
        }
        assert_eq!(stored(&desk, id).await.unwrap(), before);
        assert_eq!(
            desk.words(
                if change == "native_view_closed" {
                    &new_owner
                } else {
                    &desk.ada
                },
                id
            )
            .await,
            ["Current copied native text"]
        );
    }
}

#[tokio::test]
async fn copy_replay_keeps_its_real_outcome_without_rereading_a_deleted_original() {
    let desk = Desk::new().await;
    let source = source(&desk).await;
    let destination = desk.another_topic("Copy destination").await;
    let body = body(source, "copy-replay-source-gone");
    let id = copied(&desk, destination, &body);
    let writer = key(&desk, desk.ada.id(), rights(source, destination, false)).await;
    assert_eq!(
        accepted(copy(&desk, &writer.secret, destination, &body).await),
        json!({"projection":"receipt","document_id":id})
    );
    let before = stored(&desk, id).await.unwrap();
    desk.world
        .app
        .call(DocumentDelete, &[&source])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let replay = key(
        &desk,
        desk.ada.id(),
        vec![rule(OP, "topic", destination, "self", &["write"])],
    )
    .await;
    assert_eq!(
        accepted(copy(&desk, &replay.secret, destination, &body).await),
        json!({"projection":"receipt","document_id":id})
    );
    assert_eq!(stored(&desk, id).await.unwrap(), before);
    let changed = NodeDocumentCopyBody {
        title: Some("Different replay payload".into()),
        ..body.clone()
    };
    assert_eq!(
        copy(&desk, &replay.secret, destination, &changed)
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(stored(&desk, id).await.unwrap(), before);
    let fresh = NodeDocumentCopyBody {
        client_id: "new-copy-of-gone-original".into(),
        ..body
    };
    assert_eq!(
        copy(&desk, &replay.secret, destination, &fresh)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn copy_file_references_are_fresh_independent_pins_under_write_only_scope() {
    let desk = Desk::new().await;
    let source = source(&desk).await;
    let destination = desk.another_topic("Copy destination").await;
    let file = desk
        .world
        .app
        .call(DocumentFileCreate, &[&source])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery { overwrite: None })
        .header(FILENAME_HEADER, "copied-native-file.bin")
        .bytes(b"native retained copy bytes")
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native creation control requires its full response");
    let hash = orch_db::queries::documents::file(&mut *desk.world.conn().await, source, file.id)
        .await
        .unwrap()
        .unwrap()
        .blob;
    let before: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    let body = body(source, "copy-write-only-files");
    let id = copied(&desk, destination, &body);
    let key = key(&desk, desk.ada.id(), rights(source, destination, false)).await;
    assert_eq!(
        accepted(copy(&desk, &key.secret, destination, &body).await),
        json!({"projection":"receipt","document_id":id})
    );
    let copied: (Id, Vec<u8>, String) =
        sqlx::query_as("SELECT id,blob,name FROM document_files WHERE document_id=$1")
            .bind(id)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_ne!(copied.0, file.id);
    assert_eq!(copied.1.as_slice(), hash.as_bytes().as_slice());
    assert_eq!(copied.2, "copied-native-file.bin");
    let after: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(after, before + 1);
    let state = stored(&desk, id).await.unwrap();
    accepted(copy(&desk, &key.secret, destination, &body).await);
    assert_eq!(stored(&desk, id).await.unwrap(), state);
    let again: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(again, after);
}

#[tokio::test]
async fn cross_org_document_copy_requires_export_beyond_view_and_the_source_policy() {
    use orch_api_contract::access::{
        AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate,
    };
    let d = Desk::new().await;
    let reader = d.stranger("outside-reader").await;
    let source = d.make(&d.ada, "Governed source").await;
    d.paragraphs(&d.ada, source.base.id, &["Actual governed content"])
        .await;
    let root = places::make_org(&d.world, &reader, "Recipient", "recipient")
        .await
        .base
        .id;
    let project = places::make_project(&d.world, &reader, root, "Copied work")
        .await
        .base
        .id;
    let destination = places::make_topic(&d.world, &reader, project, "Copied documents")
        .await
        .base
        .id;
    let rules = vec![
        rule(OP, "document", source.base.id, "self", &["read"]),
        rule(OP, "topic", destination, "self", &["write"]),
    ];
    let token = key(&d, reader.id(), rules).await;
    for export in [false, true] {
        d.world
            .app
            .call(PermissionPageUpdate, &[&"documents", &source.base.id])
            .token(&d.ada.token)
            .body(&PageChange {
                name: Some(vec![PageChangeName {
                    holders: vec![HolderInput::User { id: reader.id() }],
                    choice: AccessChoice::FullView,
                    export: Some(export),
                    options: None,
                    expires_at: Some(orch_types::Time::new(
                        d.world.now() + chrono::Duration::days(1),
                    )),
                    note: None,
                }]),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        let request = body(
            source.base.id,
            if export {
                "cross-root-with-export"
            } else {
                "cross-root-view-only"
            },
        );
        let reply = copy(&d, &token.secret, destination, &request).await;
        let target = orch_db::queries::documents::copy::identity(
            reader.id(),
            destination,
            &request.client_id,
            "copy",
        );
        if export {
            assert_eq!(reply.status, StatusCode::CREATED, "{:?}", reply);
            let row: Json = sqlx::query_scalar("SELECT jsonb_build_object('home',node_id,'org',org_id,'owner',owner_id) FROM documents WHERE id=$1")
                .bind(target).fetch_one(&d.world.db.pool).await.unwrap();
            assert_eq!(
                row,
                json!({"home":destination,"org":root,"owner":reader.id()})
            );
            let recorded: Json = sqlx::query_scalar(
                "SELECT details FROM audit_log WHERE action='content.copied_out' AND object_id=$1",
            )
            .bind(source.base.id)
            .fetch_one(&d.world.db.pool)
            .await
            .unwrap();
            assert_eq!(recorded["destination_org_id"], json!(root));
            assert_eq!(recorded["created_id"], json!(target));
        } else {
            assert_eq!(reply.status, StatusCode::FORBIDDEN, "{:?}", reply);
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM documents WHERE id=$1)")
                    .bind(target)
                    .fetch_one(&d.world.db.pool)
                    .await
                    .unwrap();
            assert!(
                !exists,
                "View alone cannot remove content from its original governance"
            );
        }
    }
    let mut tx = d.world.db.pool.begin().await.unwrap();
    orch_settings::write::write(
        &mut tx,
        &orch_settings::write::Write {
            actor: orch_db::audit::Actor::User(d.ada.id()),
            target: orch_settings::Target::Node(d.org),
            key: orch_settings_contract::registry::keys::SHARING_ALLOW_COPY_OUT,
            value: json!(false),
            locked: Some(true),
            expected_revision: None,
            may_lock: true,
            origin: &Default::default(),
        },
        d.world.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let refused = copy(
        &d,
        &token.secret,
        destination,
        &body(source.base.id, "cross-root-policy-refused"),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}
