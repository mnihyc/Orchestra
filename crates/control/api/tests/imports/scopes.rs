//! Native retained Import identity, live name bounds and ordered source-file custody.

use super::*;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::imports::ImportFileList;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity::{self, NewApiToken};

#[path = "creation.rs"]
mod creation;

#[path = "list.rs"]
mod history;

#[path = "reports.rs"]
mod reports;

#[path = "download.rs"]
mod download;

#[path = "preview.rs"]
mod preview;

#[path = "planning.rs"]
mod planning;

#[path = "undo_planning.rs"]
mod undo_planning;

#[path = "undoing.rs"]
mod undoing;

struct Key {
    id: Id,
    secret: String,
}

fn rule(
    operation: &str,
    kind: &str,
    id: Id,
    reach: &str,
    resources: &[&str],
    actions: &[&str],
) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":reach},"resources":resources,
        "allow":[{"operation":operation,"actions":actions}]})
}
fn artifact(operation: &str, id: Id) -> Value {
    rule(operation, "import", id, "self", &["import"], &["read"])
}
fn names(o: &Office) -> Value {
    rule(
        "ImportGet",
        "org",
        o.org(),
        "subtree",
        &["org", "project", "topic", "thread"],
        &["read"],
    )
}

async fn key(o: &Office, actor: &Who, rules: Vec<Value>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let world = &o.s.rig.world;
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: actor.id(),
            name: "Native stored import reader fixture",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}

async fn scene() -> (Office, Import) {
    let o = Office::new().await;
    let first = upload(
        &o,
        &o.s.ada,
        "first-source.md",
        b"# First stored import source\n",
    )
    .await;
    let second = upload(
        &o,
        &o.s.ada,
        "second-source.md",
        b"# Second stored import source\n",
    )
    .await;
    let mut request = asked("native-scoped-import-readers", &first);
    request.source.files.push(second);
    let import = imported(&o, &o.s.ada, o.topic(), &request).await;
    assert_eq!(import.files.len(), 2);
    (o, import)
}

async fn metadata(o: &Office, token: &str, id: Id) -> RawReply {
    let call = o.s.rig.world.app.call(ImportGet, &[&id]);
    call.token(token).send_bytes().await
}
async fn files(o: &Office, token: &str, id: Id, query: &PageQuery) -> RawReply {
    let call = o.s.rig.world.app.call(ImportFileList, &[&id]);
    call.token(token).query(query).send_bytes().await
}
fn full(reply: RawReply) -> Import {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn page(reply: RawReply) -> orch_api_contract::common::Page<String> {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn import_metadata_and_file_pages_keep_native_bodies_own_operations_and_live_name_bounds() {
    let (o, import) = scene().await;
    let reader = key(
        &o,
        &o.s.ada,
        vec![
            artifact("ImportGet", import.id),
            artifact("ImportFileList", import.id),
            names(&o),
        ],
    )
    .await;
    assert_eq!(full(metadata(&o, &reader.secret, import.id).await), import);
    let query = PageQuery {
        limit: Some(1),
        cursor: None,
    };
    let native = page(files(&o, &o.s.ada.token, import.id, &query).await);
    let first = page(files(&o, &reader.secret, import.id, &query).await);
    assert_eq!(first, native);
    assert_eq!(first.items, vec![import.files[0].clone()]);
    assert_eq!(first.next_cursor.as_deref(), Some("1"));
    let second = page(
        files(
            &o,
            &reader.secret,
            import.id,
            &PageQuery {
                limit: Some(1),
                cursor: first.next_cursor,
            },
        )
        .await,
    );
    assert_eq!(second.items, vec![import.files[1].clone()]);
    assert!(second.next_cursor.is_none());
    let root_only = key(&o, &o.s.ada, vec![artifact("ImportGet", import.id)]).await;
    let mut expected = import.clone();
    let node = expected.node.as_mut().unwrap();
    assert!(node.name.is_some());
    node.name = None;
    node.redacted = true;
    assert_eq!(
        full(metadata(&o, &root_only.secret, import.id).await),
        expected
    );
    let get_only = key(&o, &o.s.ada, vec![artifact("ImportGet", import.id)]).await;
    assert_eq!(
        files(&o, &get_only.secret, import.id, &query).await.status,
        StatusCode::FORBIDDEN
    );
    let files_only = key(&o, &o.s.ada, vec![artifact("ImportFileList", import.id)]).await;
    assert_eq!(
        metadata(&o, &files_only.secret, import.id).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        files(
            &o,
            &reader.secret,
            import.id,
            &PageQuery {
                limit: Some(1),
                cursor: Some("invalid".into())
            }
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn import_readers_keep_typed_identity_creator_custody_and_purged_destination_absence() {
    let (o, import) = scene().await;
    for operation in ["ImportGet", "ImportFileList"] {
        for rule in [
            rule(
                operation,
                "export",
                import.id,
                "self",
                &["import"],
                &["read"],
            ),
            rule(
                operation,
                "import",
                import.id,
                "self",
                &["import"],
                &["manage"],
            ),
            rule(
                operation,
                "account",
                o.s.ada.id(),
                "self",
                &["import"],
                &["read"],
            ),
        ] {
            let denied = key(&o, &o.s.ada, vec![rule]).await;
            let reply = if operation == "ImportGet" {
                metadata(&o, &denied.secret, import.id).await
            } else {
                files(&o, &denied.secret, import.id, &PageQuery::default()).await
            };
            assert_eq!(reply.status, StatusCode::FORBIDDEN);
        }
    }
    let other = key(
        &o,
        &o.s.cai,
        vec![
            artifact("ImportGet", import.id),
            artifact("ImportFileList", import.id),
        ],
    )
    .await;
    assert_eq!(
        metadata(&o, &other.secret, import.id).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        files(&o, &other.secret, import.id, &PageQuery::default())
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let linked = key(
        &o,
        &o.s.ada,
        vec![rule(
            "ImportGet",
            "org",
            o.org(),
            "subtree",
            &["import"],
            &["read"],
        )],
    )
    .await;
    assert_eq!(
        full(metadata(&o, &linked.secret, import.id).await).id,
        import.id
    );
    sqlx::query("UPDATE imports SET node_id=NULL WHERE id=$1")
        .bind(import.id)
        .execute(o.s.rig.db.pool())
        .await
        .unwrap();
    let exact = key(
        &o,
        &o.s.ada,
        vec![
            artifact("ImportGet", import.id),
            artifact("ImportFileList", import.id),
        ],
    )
    .await;
    let native = read(&o, &o.s.ada, import.id).await;
    assert!(native.node.is_none());
    assert_eq!(full(metadata(&o, &exact.secret, import.id).await), native);
    assert_eq!(
        page(files(&o, &exact.secret, import.id, &PageQuery::default()).await).items,
        native.files
    );
    assert_eq!(
        metadata(&o, &linked.secret, import.id).await.status,
        StatusCode::FORBIDDEN,
        "neither source wording nor human membership reconstructs purged destination ancestry"
    );
}

async fn activity_wait(o: &Office, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            assert!(!request.is_finished(),"the actual native import reader must reach activity publication");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(o.s.rig.db.pool()).await.unwrap();
            if waiting {break;} tokio::task::yield_now().await;
        }
    }).await.expect("the exact native import reader activity INSERT wait");
}

#[tokio::test]
async fn import_metadata_publication_keeps_original_names_after_current_scope_narrows() {
    for narrow in [false, true] {
        let (o, import) = scene().await;
        let reader = key(
            &o,
            &o.s.ada,
            vec![artifact("ImportGet", import.id), names(&o)],
        )
        .await;
        assert_eq!(full(metadata(&o, &reader.secret, import.id).await), import);
        let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = o.s.rig.world.app.clone();
        let id = import.id;
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(ImportGet, &[&id]);
            call.token(&token).send_bytes().await
        });
        activity_wait(&o, pid, &request).await;
        if narrow {
            let scope: KeyScope =
                serde_json::from_value(json!({"rules":[artifact("ImportGet",id)]})).unwrap();
            assert!(scope.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(reader.id)
                .bind(json!(scope))
                .execute(o.s.rig.db.pool())
                .await
                .unwrap();
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            if narrow {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
        if narrow {
            assert!(
                full(metadata(&o, &reader.secret, id).await)
                    .node
                    .unwrap()
                    .name
                    .is_none()
            );
        }
    }
}

#[tokio::test]
async fn import_file_publication_keeps_original_saved_names_and_live_key_after_activity_waits() {
    for mode in ["unchanged", "replaced", "expired"] {
        let (o, import) = scene().await;
        let reader = key(&o, &o.s.ada, vec![artifact("ImportFileList", import.id)]).await;
        assert_eq!(
            page(files(&o, &reader.secret, import.id, &PageQuery::default()).await).items,
            import.files
        );
        let mut blocker = o.s.rig.db.pool().begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = o.s.rig.world.app.clone();
        let id = import.id;
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(ImportFileList, &[&id]);
            call.token(&token)
                .query(&PageQuery::default())
                .send_bytes()
                .await
        });
        activity_wait(&o, pid, &request).await;
        match mode {
            "replaced" => {
                sqlx::query("UPDATE imports SET files=ARRAY['replacement-only.md'] WHERE id=$1")
                    .bind(id)
                    .execute(o.s.rig.db.pool())
                    .await
                    .unwrap();
            }
            "expired" => o.s.rig.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            match mode {
                "replaced" => StatusCode::FORBIDDEN,
                "expired" => StatusCode::UNAUTHORIZED,
                _ => StatusCode::OK,
            }
        );
    }
}
