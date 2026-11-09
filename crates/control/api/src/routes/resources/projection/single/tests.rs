//! Real metadata invalidations after a completed singleton snapshot, before final publication.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

use super::*;
use crate::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use crate::testing::TestApp;
use orch_api_contract::document_tree::{
    DocumentContainerCreate, DocumentContainerCreateBody, DocumentTreeCommand, DocumentTreeItem,
    DocumentTreeOperate, DocumentTreeOperationBody,
};
use orch_api_contract::nodes::NodeUpdate;
use orch_api_contract::ownership::{ObjectTransfer, TransferBody};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::new_id;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::desk;

#[derive(Clone, Copy)]
enum Change {
    Hierarchy,
    Membership,
    KeepRead,
    KeepManage,
    Expire,
}

struct Boundary {
    object: ObjectRef,
    owner: Id,
    key: Id,
    root: Id,
    app: TestApp,
    token: String,
    pool: sqlx::PgPool,
    node: Id,
    sibling: Id,
    change: Change,
    reached: Arc<AtomicUsize>,
}
tokio::task_local! { static BOUNDARY: Boundary; }

pub(super) async fn after_snapshot(snapshot: &Snapshot) {
    let Some((app, token, pool, node, sibling, change, owner, key, root)) = BOUNDARY
        .try_with(|cut| {
            if cut.object != snapshot.object {
                return None;
            }
            assert!(
                snapshot.has_cut(),
                "the actual native metadata cut is retained by the factory"
            );
            assert_eq!(
                cut.reached.fetch_add(1, Ordering::SeqCst),
                0,
                "one completed snapshot is checked once"
            );
            Some((
                cut.app.clone(),
                cut.token.clone(),
                cut.pool.clone(),
                cut.node,
                cut.sibling,
                cut.change,
                cut.owner,
                cut.key,
                cut.root,
            ))
        })
        .ok()
        .flatten()
    else {
        return;
    };
    let sql = match snapshot.object.kind {
        K::Document => "SELECT owner_id FROM documents WHERE id=$1",
        K::DocumentContainer => "SELECT owner_id FROM document_containers WHERE id=$1",
        _ => panic!("this boundary is a document metadata cut"),
    };
    let actual: Id = sqlx::query_scalar(sql)
        .bind(snapshot.object.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        actual, owner,
        "the ownership effect committed before the cut was held"
    );
    let mut conn = pool.acquire().await.unwrap();
    let epoch = orch_db::queries::access::epoch(&mut conn).await.unwrap();
    let membership = orch_db::queries::documents::containers::scope_seq(&mut conn, node)
        .await
        .unwrap();
    drop(conn);
    match change {
        Change::Hierarchy => {
            let revision: i64 = sqlx::query_scalar("SELECT revision FROM nodes WHERE id=$1")
                .bind(node)
                .fetch_one(&pool)
                .await
                .unwrap();
            let call = app.call(NodeUpdate, &[&node]);
            let request = call.token(&token).raw_body(
                &json!({"name":"Changed after the native snapshot","expected_revision":revision}),
            );
            let result = request.send_bytes().await;
            assert_eq!(
                result.status,
                http::StatusCode::OK,
                "native node rename: {}",
                String::from_utf8_lossy(&result.raw)
            );
        }
        Change::Membership => {
            let result = app
                .call(DocumentTreeOperate, &[])
                .token(&token)
                .body(&DocumentTreeOperationBody {
                    command: DocumentTreeCommand::Rename,
                    items: vec![DocumentTreeItem::Document(sibling)],
                    to: None,
                    name: Some("Changed after the folder snapshot".into()),
                    client_id: new_id().to_string(),
                    version: None,
                    deletion_plan: None,
                })
                .send_bytes()
                .await;
            assert_eq!(
                result.status,
                http::StatusCode::OK,
                "native document rename: {}",
                String::from_utf8_lossy(&result.raw)
            );
        }
        Change::KeepRead | Change::KeepManage => {
            let actions = if matches!(change, Change::KeepRead) {
                vec!["read"]
            } else {
                vec!["manage"]
            };
            // Ancestor-name Read must not also cover the selected document's response Read.
            let policy: orch_access_contract::key_scope::KeyScope = serde_json::from_value(json!({"rules":[
                {"target":{"kind":snapshot.object.kind.as_str(),"id":snapshot.object.id,"reach":"self"},"allow":[{"operation":"ObjectTransfer","actions":actions}]},
                {"target":{"kind":"org","id":root,"reach":"subtree"},"resources":["org","project","topic","thread"],"allow":[{"operation":"ObjectTransfer","actions":["read"]}]}
            ]})).unwrap();
            assert!(policy.validate().is_empty());
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(key)
                .bind(serde_json::to_value(policy).unwrap())
                .execute(&pool)
                .await
                .unwrap();
        }
        Change::Expire => {
            app.clock.advance(chrono::Duration::hours(1));
        }
    }
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(
        orch_db::queries::access::epoch(&mut conn).await.unwrap(),
        epoch,
        "the deciding cut is independent of an ACL epoch change"
    );
    let current = orch_db::queries::documents::containers::scope_seq(&mut conn, node)
        .await
        .unwrap();
    match change {
        Change::Hierarchy => assert_eq!(current, membership, "only the hierarchy hash changed"),
        Change::Membership => assert_ne!(
            current, membership,
            "the actual directory generation changed"
        ),
        Change::KeepRead | Change::KeepManage | Change::Expire => assert_eq!(
            current, membership,
            "credential changes do not fabricate a directory generation change"
        ),
    }
}

async fn control(kind: K, change: Change) {
    crate::routes::resources::sealing::install_master_key(orch_db::secrets::MasterKey::new(
        [36; 32],
    ));
    let desk = desk::Desk::new().await;
    let receiver = desk.member("metadata-recipient", "viewer").await;
    let sibling = desk.make(&desk.ada, "Sibling before the cut").await.base.id;
    let id = if kind == K::Document {
        desk.make(&desk.ada, "Document before the cut")
            .await
            .base
            .id
    } else {
        let made = desk
            .world
            .app
            .call(DocumentContainerCreate, &[&desk.topic])
            .token(&desk.ada.token)
            .body(&DocumentContainerCreateBody {
                parent_container_id: None,
                name: "Folder before the cut".into(),
                client_id: new_id().to_string(),
            })
            .send()
            .await
            .ok()
            .into_full()
            .unwrap();
        let Some(DocumentTreeItem::Container(id)) = made.entries.first().map(|entry| entry.item)
        else {
            panic!("a real native folder")
        };
        id
    };
    let scope: orch_access_contract::key_scope::KeyScope = serde_json::from_value(json!({"rules":[
        {"target":{"kind":kind.as_str(),"id":id,"reach":"self"},"allow":[{"operation":"ObjectTransfer","actions":["manage","read"]}]},
        {"target":{"kind":"org","id":desk.org,"reach":"subtree"},"resources":["org","project","topic","thread"],"allow":[{"operation":"ObjectTransfer","actions":["read"]}]}
    ]})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    let key = new_id();
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: key,
            user_id: desk.ada.id(),
            name: "Metadata cut scope",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    let reached = Arc::new(AtomicUsize::new(0));
    let boundary = Boundary {
        object: ObjectRef { kind, id },
        owner: receiver.id(),
        key,
        root: desk.org,
        app: desk.world.app.clone(),
        token: desk.ada.token.clone(),
        pool: desk.world.db.pool.clone(),
        node: desk.topic,
        sibling,
        change,
        reached: reached.clone(),
    };
    let plural = if kind == K::Document {
        "documents"
    } else {
        "document-containers"
    };
    let reply = BOUNDARY
        .scope(boundary, async {
            desk.world
                .app
                .call(ObjectTransfer, &[&plural, &id])
                .token(&token)
                .body(&TransferBody {
                    user_id: receiver.id(),
                    keep_previous: Some(true),
                })
                .send_bytes()
                .await
        })
        .await;
    assert_eq!(
        reached.load(Ordering::SeqCst),
        1,
        "the actual post-snapshot cut must be reached"
    );
    assert_eq!(reply.status, http::StatusCode::OK);
    let value: orch_types::Json = serde_json::from_slice(&reply.raw).unwrap();
    if matches!(change, Change::KeepRead) {
        assert_ne!(
            value["projection"], "receipt",
            "the metadata finisher reads without re-admitting completed Manage"
        );
        let _: Resource = serde_json::from_value(value).unwrap();
    } else {
        assert_eq!(
            value,
            json!({"projection":"receipt","object_id":id,"kind":kind.as_str(),"owner_id":receiver.id(),"changed":true}),
            "unavailable metadata is withheld without replaying or losing ownership"
        );
    }
}

#[tokio::test]
async fn document_cut_rejects_a_changed_hierarchy_after_snapshot_commit() {
    control(K::Document, Change::Hierarchy).await;
}

#[tokio::test]
async fn folder_cut_rejects_a_changed_membership_generation_after_snapshot_commit() {
    control(K::DocumentContainer, Change::Membership).await;
}

#[tokio::test]
async fn document_metadata_finisher_keeps_read_after_completed_manage_is_removed() {
    control(K::Document, Change::KeepRead).await;
}
#[tokio::test]
async fn folder_metadata_finisher_keeps_read_after_completed_manage_is_removed() {
    control(K::DocumentContainer, Change::KeepRead).await;
}
#[tokio::test]
async fn document_metadata_finisher_withholds_read_lost_after_commit() {
    control(K::Document, Change::KeepManage).await;
}
#[tokio::test]
async fn folder_metadata_finisher_withholds_read_lost_after_commit() {
    control(K::DocumentContainer, Change::KeepManage).await;
}
#[tokio::test]
async fn a_key_expiring_after_metadata_commit_keeps_only_its_accepted_receipt() {
    control(K::Document, Change::Expire).await;
}
